use anyhow::{Context, Result};
use hikvision_rs::netstream::NetStream;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

struct RtspSession {
    stream: TcpStream,
}

struct AuthCtx {
    realm: String,
    nonce: String,
    opaque: String,
    user: String,
    password: String,
}

impl AuthCtx {
    fn from_challenge(challenge: &str, user: &str, password: &str) -> Self {
        let body = challenge.strip_prefix("Digest ")
            .or_else(|| challenge.strip_prefix("digest "))
            .unwrap_or(challenge);
        let mut realm = String::new();
        let mut nonce = String::new();
        let mut opaque = String::new();
        for part in body.split(',') {
            let part = part.trim();
            if let Some(eq) = part.find('=') {
                let key = part[..eq].trim();
                let val = part[eq + 1..].trim().trim_matches('"');
                match key.to_lowercase().as_str() {
                    "realm" => realm = val.to_string(),
                    "nonce" => nonce = val.to_string(),
                    "opaque" => opaque = val.to_string(),
                    _ => {}
                }
            }
        }
        Self { realm, nonce, opaque, user: user.to_string(), password: password.to_string() }
    }

    fn header(&self, method: &str, uri: &str) -> String {
        let ha1 = format!("{:x}", md5::compute(format!("{}:{}:{}", self.user, self.realm, self.password)));
        let ha2 = format!("{:x}", md5::compute(format!("{}:{}", method, uri)));
        let response = format!("{:x}", md5::compute(format!("{}:{}:{}", ha1, self.nonce, ha2)));
        let mut auth = format!(
            "Digest username=\"{}\", realm=\"{}\", nonce=\"{}\", uri=\"{}\", response=\"{}\"",
            self.user, self.realm, self.nonce, uri, response
        );
        if !self.opaque.is_empty() {
            auth.push_str(&format!(", opaque=\"{}\"", self.opaque));
        }
        auth
    }
}

impl RtspSession {
    fn connect(host: &str, port: u16, channel: &str, user: &str, password: &str) -> Result<Self> {
        let addr = format!("{}:{}", host, port);
        let mut stream = TcpStream::connect_timeout(
            &addr.parse::<std::net::SocketAddr>()
                .with_context(|| format!("invalid address: {}", addr))?,
            Duration::from_secs(5),
        ).context("RTSP connection failed")?;
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;

        let channels_uri = format!("rtsp://{}:{}/Streaming/Channels/{}", host, port, channel);
        let mut cseq = 0u32;

        // DESCRIBE (no auth)
        cseq += 1;
        Self::send(&mut stream, &format!(
            "DESCRIBE {} RTSP/1.0\r\nCSeq: {}\r\nUser-Agent: hikvision-rs\r\nAccept: application/sdp\r\n\r\n",
            channels_uri, cseq
        ))?;
        let resp = Self::read_response(&mut stream)?;

        let auth = if resp.contains("401 Unauthorized") {
            let challenge = resp.lines()
                .find(|l| l.to_lowercase().contains("www-authenticate"))
                .and_then(|l| l.splitn(2, ':').nth(1))
                .map(|s| s.trim())
                .unwrap_or("");
            let ctx = AuthCtx::from_challenge(challenge, user, password);

            // DESCRIBE with auth
            cseq += 1;
            let hdr = ctx.header("DESCRIBE", &channels_uri);
            Self::send(&mut stream, &format!(
                "DESCRIBE {} RTSP/1.0\r\nCSeq: {}\r\nAuthorization: {}\r\nUser-Agent: hikvision-rs\r\nAccept: application/sdp\r\n\r\n",
                channels_uri, cseq, hdr
            ))?;
            let resp = Self::read_response(&mut stream)?;
            if !resp.contains("200 OK") {
                let excerpt: String = resp.chars().take(200).collect();
                anyhow::bail!("DESCRIBE failed: {}", excerpt);
            }
            ctx
        } else if !resp.contains("200 OK") {
            let excerpt: String = resp.chars().take(200).collect();
            anyhow::bail!("DESCRIBE failed (no auth): {}", excerpt);
        } else {
            anyhow::bail!("DVR does not require authentication, unexpected");
        };

        // SETUP interleaved (video track)
        let track_uri = format!("{}/trackID=video", channels_uri);
        cseq += 1;
        Self::send(&mut stream, &format!(
            "SETUP {} RTSP/1.0\r\nCSeq: {}\r\nAuthorization: {}\r\nTransport: RTP/AVP/TCP;unicast;interleaved=0-1\r\nUser-Agent: hikvision-rs\r\n\r\n",
            track_uri, cseq, auth.header("SETUP", &track_uri)
        ))?;
        let setup_resp = Self::read_response(&mut stream)?;
        if !setup_resp.contains("200 OK") {
            let excerpt: String = setup_resp.chars().take(200).collect();
            anyhow::bail!("SETUP failed: {}", excerpt);
        }

        // PLAY
        cseq += 1;
        let session = Self::extract_session(&setup_resp);
        Self::send(&mut stream, &format!(
            "PLAY {} RTSP/1.0\r\nCSeq: {}\r\nAuthorization: {}\r\nSession: {}\r\nUser-Agent: hikvision-rs\r\n\r\n",
            channels_uri, cseq, auth.header("PLAY", &channels_uri), session
        ))?;
        let play_resp = Self::read_response(&mut stream)?;
        if !play_resp.contains("200 OK") {
            let excerpt: String = play_resp.chars().take(200).collect();
            anyhow::bail!("PLAY failed: {}", excerpt);
        }

        log::info!("RTSP channel {} established", channel);
        Ok(Self { stream })
    }

    fn send(stream: &mut TcpStream, req: &str) -> Result<()> {
        stream.write_all(req.as_bytes())?;
        Ok(())
    }

    fn read_response(stream: &mut TcpStream) -> Result<String> {
        let mut buf = Vec::new();
        let mut tmp = [0u8; 1];
        loop {
            match stream.read(&mut tmp) {
                Ok(0) => break,
                Ok(_) => {
                    buf.push(tmp[0]);
                    if buf.len() >= 4 && buf[buf.len() - 4..] == *b"\r\n\r\n" {
                        let header_str = String::from_utf8_lossy(&buf);
                        if let Some(cl) = header_str.lines()
                            .find(|l| l.to_lowercase().starts_with("content-length:"))
                            .and_then(|l| l.splitn(2, ':').nth(1))
                            .and_then(|s| s.trim().parse::<usize>().ok())
                        {
                            let mut body = vec![0u8; cl];
                            let mut read = 0;
                            while read < cl {
                                match stream.read(&mut body[read..]) {
                                    Ok(0) => break,
                                    Ok(n) => read += n,
                                    Err(_) => break,
                                }
                            }
                            buf.extend_from_slice(&body[..read]);
                        }
                        break;
                    }
                }
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::TimedOut || e.kind() == std::io::ErrorKind::WouldBlock {
                        continue;
                    }
                    anyhow::bail!("read error: {}", e);
                }
            }
        }
        Ok(String::from_utf8_lossy(&buf).to_string())
    }

    fn extract_session(response: &str) -> &str {
        response.lines()
            .find(|l| l.to_lowercase().starts_with("session:"))
            .and_then(|l| l.splitn(2, ':').nth(1))
            .map(|s| s.trim().split(';').next().unwrap_or(s.trim()))
            .unwrap_or("1")
    }

    fn read_rtp_frame(&mut self) -> Result<Option<(u8, Vec<u8>)>> {
        let mut marker = [0u8; 1];
        match self.stream.read(&mut marker) {
            Ok(0) => return Ok(None),
            Ok(_) => {}
            Err(e) => {
                if e.kind() == std::io::ErrorKind::TimedOut {
                    return Ok(None);
                }
                anyhow::bail!("read error: {}", e);
            }
        }
        if marker[0] != 0x24 { return Ok(None); }

        let mut ch_len = [0u8; 3];
        if self.stream.read_exact(&mut ch_len).is_err() { return Ok(None); }
        let chan = ch_len[0];
        let length = u16::from_be_bytes([ch_len[1], ch_len[2]]) as usize;

        let mut payload = vec![0u8; length];
        if length > 0 && self.stream.read_exact(&mut payload).is_err() {
            return Ok(None);
        }
        Ok(Some((chan, payload)))
    }
}

fn strip_rtp_header(data: &[u8]) -> &[u8] {
    if data.len() < 12 { return data; }
    let csrc_count = (data[0] & 0x0F) as usize;
    let extension = (data[0] >> 4) & 0x01 != 0;
    let mut offset = 12 + csrc_count * 4;
    if extension && data.len() > offset + 4 {
        let ext_len = u16::from_be_bytes([data[offset + 2], data[offset + 3]]) as usize;
        offset += 4 + ext_len * 4;
    }
    if offset < data.len() { &data[offset..] } else { &data[12..] }
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis().init();

    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 || args.contains(&"--help".to_string()) || args.contains(&"-h".to_string()) {
        eprintln!("Decryption Proxy (libnet_stream)");
        eprintln!("Usage: decrypt_proxy --host <DVR_IP> --password <PASS> [options]");
        eprintln!("Options:");
        eprintln!("  --host               DVR IP (required)");
        eprintln!("  --password           DVR password (required)");
        eprintln!("  --channel            Channel (default: 101)");
        eprintln!("  --rtsp-port          RTSP port (default: 554)");
        eprintln!("  --user               Username (default: admin)");
        eprintln!("  --listen             Listen addr (default: 127.0.0.1:9000)");
        eprintln!("  --verification-code  VCode (default: ****)");
        std::process::exit(1);
    }

    let mut host = String::new();
    let mut rtsp_port = 554u16;
    let mut channel = "101".to_string();
    let mut user = "admin".to_string();
    let mut password = String::new();
    let mut listen = "127.0.0.1:9000".to_string();
    let mut vcode = "KeZtid".to_string();

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--host" => { i += 1; host = args.get(i).cloned().unwrap_or_default(); }
            "--rtsp-port" => { i += 1; rtsp_port = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(554); }
            "--channel" => { i += 1; channel = args.get(i).cloned().unwrap_or_else(|| "101".to_string()); }
            "--user" => { i += 1; user = args.get(i).cloned().unwrap_or_else(|| "admin".to_string()); }
            "--password" => { i += 1; password = args.get(i).cloned().unwrap_or_default(); }
            "--listen" => { i += 1; listen = args.get(i).cloned().unwrap_or_else(|| "127.0.0.1:9000".to_string()); }
            "--verification-code" => { i += 1; vcode = args.get(i).cloned().unwrap_or_else(|| "****".to_string()); }
            _ => {}
        }
        i += 1;
    }

    if host.is_empty() || password.is_empty() {
        eprintln!("--host and --password are required");
        std::process::exit(1);
    }

    log::info!("Decryption Proxy | {}:{} ch={} vcode={}", host, rtsp_port, channel, vcode);

    let ns = NetStream::load()?;
    ns.set_secret_key(0, &vcode)?;
    log::info!("Key set");

    let mut rtsp = RtspSession::connect(&host, rtsp_port, &channel, &user, &password)?;
    log::info!("RTSP active");

    let listener = TcpListener::bind(&listen).with_context(|| format!("bind {}", listen))?;
    log::info!("Listening on {}", listen);

    let (mut client, addr) = listener.accept().context("accept")?;
    log::info!("Client: {:?}", addr);

    let mut fc = 0u64;
    let mut dc = 0u64;
    let mut db = 0u64;
    let start = Instant::now();

    loop {
        match rtsp.read_rtp_frame() {
            Ok(Some((_ch, data))) => {
                let payload = strip_rtp_header(&data);
                fc += 1;
                if !payload.is_empty() {
                    match ns.decrypt(payload, &[], 0) {
                        Ok(out) => if !out.is_empty() {
                            let _ = client.write_all(&out);
                            dc += 1;
                            db += out.len() as u64;
                        },
                        Err(e) => {
                            if fc % 100 == 0 { log::warn!("decrypt fail #{}: {}", fc, e); }
                        }
                    }
                }
                if fc % 100 == 0 {
                    let e = start.elapsed().as_secs_f64();
                    log::info!("{} frames {} dec {:.1}MB {:.1}s {:.0}fps", fc, dc, db as f64/1e6, e, fc as f64/e);
                }
            }
            Ok(None) => { log::info!("RTSP ended"); break; }
            Err(e) => { log::error!("RTSP: {}", e); break; }
        }
    }

    log::info!("Session: {} frames / {} decrypted / {:.1}s", fc, dc, start.elapsed().as_secs_f64());
    Ok(())
}
