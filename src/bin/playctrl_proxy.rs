use anyhow::{Context, Result};
use hikvision_rs::playctrl::PlayCtrl;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Connect to DVR RTSP in interleaved mode and read raw RTP data.
/// Returns the RTP payload bytes (after stripping the $-channel prefix and
/// the 12-byte RTP header).
struct RtspSession {
    stream: TcpStream,
    _cseq: u32,
}

impl RtspSession {
    fn connect(host: &str, port: u16, channel: &str, user: &str, password: &str) -> Result<Self> {
        let addr = format!("{}:{}", host, port);
        let mut stream = TcpStream::connect_timeout(
            &addr.parse::<std::net::SocketAddr>()
                .with_context(|| format!("invalid address: {}", addr))?,
            Duration::from_secs(5),
        )
        .context("RTSP connection failed")?;
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;

        let mut cseq = 0u32;

        // Build Digest auth header for RTSP
        let auth = {
            let auth_header = format!(
                "Authorization: Digest username=\"{}\", realm=\"Hikvision\", nonce=\"init\", uri=\"rtsp://{}:{}/Streaming/Channels/{}\", response=\"dummy\"\r\n",
                user, host, port, channel
            );
            auth_header
        };

        // DESCRIBE
        cseq += 1;
        let describe = format!(
            "DESCRIBE rtsp://{}:{}/Streaming/Channels/{} RTSP/1.0\r\n\
             CSeq: {}\r\n\
             {}\
             User-Agent: hikvision-rs\r\n\
             Accept: application/sdp\r\n\
             \r\n",
            host, port, channel, cseq, auth
        );
        stream.write_all(describe.as_bytes())?;
        let resp = Self::read_response(&mut stream)?;
        log::debug!("DESCRIBE response: {}", &resp[..resp.len().min(200)]);

        // Handle 401 by computing proper Digest
        if resp.contains("401 Unauthorized") {
            let challenge = resp
                .lines()
                .find(|l| l.to_lowercase().contains("www-authenticate"))
                .and_then(|l| l.splitn(2, ':').nth(1))
                .map(|s| s.trim())
                .unwrap_or("");

            let digest = compute_rtsp_digest(challenge, "DESCRIBE", &format!(
                "rtsp://{}:{}/Streaming/Channels/{}", host, port, channel
            ), user, password)?;

            // Retry DESCRIBE with auth
            cseq += 1;
            let describe2 = format!(
                "DESCRIBE rtsp://{}:{}/Streaming/Channels/{} RTSP/1.0\r\n\
                 CSeq: {}\r\n\
                 Authorization: {}\r\n\
                 User-Agent: hikvision-rs\r\n\
                 Accept: application/sdp\r\n\
                 \r\n",
                host, port, channel, cseq, digest
            );
            stream.write_all(describe2.as_bytes())?;
            let resp2 = Self::read_response(&mut stream)?;
            log::debug!("DESCRIBE (auth) response: {}", &resp2[..resp2.len().min(200)]);
            if !resp2.contains("200 OK") {
                anyhow::bail!("DESCRIBE failed: {}", &resp2[..resp2.len().min(200)]);
            }
        }

        // SETUP - interleaved mode
        cseq += 1;
        let setup = format!(
            "SETUP rtsp://{}:{}/Streaming/Channels/{}/trackID=1 RTSP/1.0\r\n\
             CSeq: {}\r\n\
             Transport: RTP/AVP/TCP;unicast;interleaved=0-1\r\n\
             User-Agent: hikvision-rs\r\n\
             \r\n",
            host, port, channel, cseq
        );
        stream.write_all(setup.as_bytes())?;
        let setup_resp = Self::read_response(&mut stream)?;
        log::debug!("SETUP response: {}", &setup_resp[..setup_resp.len().min(300)]);
        if !setup_resp.contains("200 OK") {
            anyhow::bail!("SETUP failed: {}", &setup_resp[..setup_resp.len().min(200)]);
        }

        // PLAY
        cseq += 1;
        let play = format!(
            "PLAY rtsp://{}:{}/Streaming/Channels/{} RTSP/1.0\r\n\
             CSeq: {}\r\n\
             Session: {}\r\n\
             User-Agent: hikvision-rs\r\n\
             \r\n",
            host,
            port,
            channel,
            cseq,
            Self::extract_session(&setup_resp)
        );
        stream.write_all(play.as_bytes())?;
        let play_resp = Self::read_response(&mut stream)?;
        log::debug!("PLAY response: {}", &play_resp[..play_resp.len().min(300)]);
        if !play_resp.contains("200 OK") {
            anyhow::bail!("PLAY failed: {}", &play_resp[..play_resp.len().min(200)]);
        }

        log::info!("RTSP session established for channel {}", channel);
        Ok(Self {
            stream,
            _cseq: cseq,
        })
    }

    fn read_response(stream: &mut TcpStream) -> Result<String> {
        let mut buf = Vec::new();
        let mut tmp = [0u8; 1];

        // Read until we find the end of RTSP response headers (\r\n\r\n)
        loop {
            match stream.read(&mut tmp) {
                Ok(0) => break,
                Ok(_) => {
                    buf.push(tmp[0]);
                    if buf.len() >= 4 && buf[buf.len() - 4..] == *b"\r\n\r\n" {
                        // Check for Content-Length to read body
                        let header_str = String::from_utf8_lossy(&buf);
                        if let Some(cl) = header_str
                            .lines()
                            .find(|l| l.to_lowercase().starts_with("content-length:"))
                            .and_then(|l| l.splitn(2, ':').nth(1))
                            .and_then(|s| s.trim().parse::<usize>().ok())
                        {
                            let body_needed = cl;
                            let mut body = vec![0u8; body_needed];
                            let mut read = 0;
                            while read < body_needed {
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
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut
                    {
                        continue;
                    }
                    anyhow::bail!("read error: {}", e);
                }
            }
        }

        Ok(String::from_utf8_lossy(&buf).to_string())
    }

    fn extract_session(response: &str) -> &str {
        response
            .lines()
            .find(|l| l.to_lowercase().starts_with("session:"))
            .and_then(|l| l.splitn(2, ':').nth(1))
            .map(|s| s.trim().split(';').next().unwrap_or(s.trim()))
            .unwrap_or("1")
    }

    /// Read next RTP interleaved frame. Returns (channel, payload) where
    /// payload is the RTP data after the $ header.
    fn read_rtp_frame(&mut self) -> Result<Option<(u8, Vec<u8>)>> {
        let mut header = [0u8; 4];

        // Read $ byte
        match self.stream.read(&mut header[..1]) {
            Ok(0) => return Ok(None),
            Ok(_) => {}
            Err(e) => {
                if e.kind() == std::io::ErrorKind::TimedOut {
                    return Ok(None);
                }
                anyhow::bail!("read error: {}", e);
            }
        }

        if header[0] != 0x24 {
            log::warn!("Expected $ (0x24), got 0x{:02x}", header[0]);
            return Ok(None);
        }

        // Read channel
        if self.stream.read_exact(&mut header[1..2]).is_err() {
            return Ok(None);
        }
        let channel = header[1];

        // Read length
        if self.stream.read_exact(&mut header[2..4]).is_err() {
            return Ok(None);
        }
        let length = u16::from_be_bytes([header[2], header[3]]) as usize;

        // Read payload
        let mut payload = vec![0u8; length];
        if length > 0 && self.stream.read_exact(&mut payload).is_err() {
            return Ok(None);
        }

        Ok(Some((channel, payload)))
    }
}

fn compute_rtsp_digest(
    challenge: &str,
    method: &str,
    uri: &str,
    user: &str,
    password: &str,
) -> Result<String> {
    // Strip "Digest " prefix
    let body = challenge
        .strip_prefix("Digest ")
        .or_else(|| challenge.strip_prefix("digest "))
        .unwrap_or(challenge);

    let mut realm = "";
    let mut nonce = "";
    let mut opaque = "";
    for part in body.split(',') {
        let part = part.trim();
        if let Some(eq) = part.find('=') {
            let key = part[..eq].trim();
            let val = part[eq + 1..].trim().trim_matches('"');
            match key.to_lowercase().as_str() {
                "realm" => realm = val,
                "nonce" => nonce = val,
                "opaque" => opaque = val,
                _ => {}
            }
        }
    }

    let ha1 = format!("{:x}", md5::compute(format!("{}:{}:{}", user, realm, password).as_bytes()));
    let ha2 = format!("{:x}", md5::compute(format!("{}:{}", method, uri).as_bytes()));
    let cnonce = format!("{:x}", md5::compute(format!("{}{}", nonce, user).as_bytes()));
    let response = format!(
        "{:x}",
        md5::compute(
            format!("{}:{}:00000001:{}:auth:{}", ha1, nonce, cnonce, ha2).as_bytes()
        )
    );

    let mut auth = format!(
        "Digest username=\"{}\", realm=\"{}\", nonce=\"{}\", uri=\"{}\", response=\"{}\", nc=00000001, cnonce=\"{}\", qop=auth",
        user, realm, nonce, uri, response, cnonce,
    );
    if !opaque.is_empty() {
        auth.push_str(&format!(", opaque=\"{}\"", opaque));
    }
    Ok(auth)
}

fn print_usage() {
    eprintln!("Hikvision PlayCtrl Decryption Proxy");
    eprintln!();
    eprintln!("Loads libPlayCtrl.so to decrypt Hikvision encrypted RTSP streams.");
    eprintln!("Outputs decrypted frames via MJPEG over HTTP.");
    eprintln!();
    eprintln!("Usage:");
    eprintln!("  playctrl_proxy --host <DVR_IP> --password <PASS>");
    eprintln!("                [--channel 101] [--rtsp-port 554] [--user admin]");
    eprintln!("                [--listen 127.0.0.1:9002] [--library-path <path>]");
    eprintln!();
    eprintln!("View with: ffplay http://127.0.0.1:9002");
}

#[derive(Default)]
struct Args {
    host: String,
    rtsp_port: u16,
    channel: String,
    user: String,
    password: String,
    listen: String,
    library_path: Option<String>,
}

fn parse_args() -> Option<Args> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2
        || args.contains(&"--help".to_string())
        || args.contains(&"-h".to_string())
    {
        return None;
    }

    let mut a = Args::default();
    a.rtsp_port = 554;
    a.channel = "101".to_string();
    a.user = "admin".to_string();
    a.listen = "127.0.0.1:9002".to_string();

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--host" => {
                i += 1;
                a.host = args.get(i)?.clone();
            }
            "--rtsp-port" => {
                i += 1;
                a.rtsp_port = args.get(i)?.parse().ok()?;
            }
            "--channel" => {
                i += 1;
                a.channel = args.get(i)?.clone();
            }
            "--user" => {
                i += 1;
                a.user = args.get(i)?.clone();
            }
            "--password" => {
                i += 1;
                a.password = args.get(i)?.clone();
            }
            "--listen" => {
                i += 1;
                a.listen = args.get(i)?.clone();
            }
            "--library-path" => {
                i += 1;
                a.library_path = args.get(i).cloned();
            }
            _ => {
                eprintln!("Unknown: {}", args[i]);
            }
        }
        i += 1;
    }

    if a.host.is_empty() || a.password.is_empty() {
        return None;
    }

    Some(a)
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    let args = parse_args().unwrap_or_else(|| {
        print_usage();
        std::process::exit(1);
    });

    log::info!("PlayCtrl Decryption Proxy");
    log::info!("DVR: {}:{} Channel: {} User: {}", args.host, args.rtsp_port, args.channel, args.user);

    // Load PlayCtrl
    let playctrl = if let Some(ref path) = args.library_path {
        PlayCtrl::load_from(std::path::Path::new(path))?
    } else {
        PlayCtrl::load()?
    };
    log::info!("libPlayCtrl.so loaded successfully");

    // Get port
    let port = playctrl.get_port()?;
    log::info!("Allocated decoder port {}", port);

    // Set secret key
    playctrl.set_secret_key(port, "****")?;
    log::info!("Secret key set");

    // Open stream
    playctrl.open_stream(port, 2 * 1024 * 1024)?;
    log::info!("Stream opened (2 MB buffer)");

    // Start RTSP session
    let mut rtsp = RtspSession::connect(
        &args.host,
        args.rtsp_port,
        &args.channel,
        &args.user,
        &args.password,
    )?;
    log::info!("RTSP session active");

    // Start TCP listener
    let listener = TcpListener::bind(&args.listen)
        .with_context(|| format!("bind to {}", args.listen))?;
    log::info!("Listening on {}", args.listen);

    // Accept one client
    let stop = Arc::new(AtomicBool::new(false));
    let (client, addr) = match listener.accept() {
        Ok(c) => c,
        Err(e) => {
            anyhow::bail!("accept failed: {}", e);
        }
    };
    log::info!("Client connected from {:?}", addr);

    // Send MJPEG HTTP header
    let boundary = "playctrl-frame";
    let http_header = format!(
        "HTTP/1.0 200 OK\r\nCache-Control: no-cache\r\nContent-Type: multipart/x-mixed-replace; boundary={}\r\n\r\n",
        boundary
    );
    let mut client = client;
    client.write_all(http_header.as_bytes())?;

    // Feeding loop
    let mut frame_count = 0u64;
    let mut byte_count = 0u64;
    let start = Instant::now();
    let mut last_jpeg_check = Instant::now();
    let mut is_first = true;

    while !stop.load(Ordering::Relaxed) {
        match rtsp.read_rtp_frame() {
            Ok(Some((_ch, rtp_data))) => {
                // Strip $ + channel + length = 4 bytes
                // Then strip RTP header (12 bytes minimum)
                let payload = if rtp_data.len() > 12 {
                    let rtp_header_len = 12;
                    let csrc_count = (rtp_data[0] & 0x0F) as usize;
                    let extension = (rtp_data[0] >> 4) & 0x01 != 0;
                    let mut offset = rtp_header_len + csrc_count * 4;
                    if extension && rtp_data.len() > offset + 4 {
                        let ext_len =
                            u16::from_be_bytes([rtp_data[offset + 2], rtp_data[offset + 3]])
                                as usize;
                        offset += 4 + ext_len * 4;
                    }
                    if offset < rtp_data.len() {
                        &rtp_data[offset..]
                    } else {
                        &rtp_data[12..]
                    }
                } else {
                    &rtp_data[..]
                };

                // Feed into PlayCtrl
                if !payload.is_empty() {
                    match playctrl.input_data(port, payload) {
                        Ok(()) => {
                            frame_count += 1;
                            byte_count += payload.len() as u64;
                        }
                        Err(e) => {
                            // InputData errors are often transient (e.g., NEED_MORE_DATA)
                            if frame_count > 0 && frame_count % 50 == 0 {
                                log::debug!("InputData warning: {}", e);
                            }
                        }
                    }
                }

                // Periodically try to get a decoded JPEG frame
                if last_jpeg_check.elapsed() >= Duration::from_millis(200) {
                    last_jpeg_check = Instant::now();
                    match playctrl.get_jpeg(port) {
                        Ok(jpeg) if !jpeg.is_empty() => {
                            let part = if is_first {
                                format!(
                                    "--{}\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
                                    boundary,
                                    jpeg.len()
                                )
                            } else {
                                format!(
                                    "\r\n--{}\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
                                    boundary,
                                    jpeg.len()
                                )
                            };
                            if client.write_all(part.as_bytes()).is_err() {
                                log::info!("Client disconnected");
                                break;
                            }
                            if client.write_all(&jpeg).is_err() {
                                log::info!("Client disconnected");
                                break;
                            }
                            is_first = false;

                            let elapsed = start.elapsed().as_secs_f64();
                            if elapsed > 0.0 && frame_count % 50 == 0 {
                                log::info!("{:.1} fps, {:.2} MB received, {} frames fed",
                                    frame_count as f64 / elapsed,
                                    byte_count as f64 / 1_000_000.0,
                                    frame_count,
                                );
                            }
                        }
                        Ok(_) => {}
                        Err(_) => {
                            // Expected when not enough data decoded yet
                            if frame_count > 50 && frame_count % 200 == 0 {
                                let err_code = playctrl.get_last_error();
                                log::warn!("No JPEG after {} frames (error {})", frame_count, err_code);
                            }
                        }
                    }
                }
            }
            Ok(None) => {
                log::info!("RTSP stream ended");
                break;
            }
            Err(e) => {
                log::error!("RTSP read error: {}", e);
                break;
            }
        }
    }

    // Cleanup
    let _ = playctrl.close_stream(port);
    let _ = playctrl.free_port(port);
    let elapsed = start.elapsed().as_secs_f64();
    log::info!("Session ended. {} frames fed, {:.1} seconds", frame_count, elapsed);
    Ok(())
}
