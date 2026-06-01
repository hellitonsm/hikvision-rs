use anyhow::{Context, Result};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::playctrl::{self, PlayCtrl};
use crate::rtsp::RtspFrame;

struct RtspSession {
    stream: TcpStream,
}

impl RtspSession {
    fn connect(host: &str, port: u16, channel: &str, user: &str, password: &str) -> Result<Self> {
        let addr = format!("{}:{}", host, port);
        let mut stream = TcpStream::connect_timeout(
            &addr
                .parse::<std::net::SocketAddr>()
                .with_context(|| format!("invalid address: {}", addr))?,
            Duration::from_secs(5),
        )
        .context("RTSP connection failed")?;
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;

        let channels_uri = format!("rtsp://{}:{}/Streaming/Channels/{}", host, port, channel);
        let mut cseq = 0u32;

        {
            let cseq_local = {
                cseq += 1;
                cseq
            };
            let describe = format!(
                "DESCRIBE {} RTSP/1.0\r\n\
                 CSeq: {}\r\n\
                 User-Agent: hikvision-rs\r\n\
                 Accept: application/sdp\r\n\
                 \r\n",
                channels_uri, cseq_local
            );
            stream.write_all(describe.as_bytes())?;
            let resp = Self::read_response(&mut stream)?;

            if resp.contains("401 Unauthorized") {
                let challenge = resp
                    .lines()
                    .find(|l| l.to_lowercase().contains("www-authenticate"))
                    .and_then(|l| l.splitn(2, ':').nth(1))
                    .map(|s| s.trim())
                    .unwrap_or("");

                let digest = compute_rtsp_digest(challenge, "DESCRIBE", &channels_uri, user, password)?;

                cseq += 1;
                let describe2 = format!(
                    "DESCRIBE {} RTSP/1.0\r\n\
                     CSeq: {}\r\n\
                     Authorization: {}\r\n\
                     User-Agent: hikvision-rs\r\n\
                     Accept: application/sdp\r\n\
                     \r\n",
                    channels_uri, cseq, digest
                );
                stream.write_all(describe2.as_bytes())?;
                let resp2 = Self::read_response(&mut stream)?;
                if !resp2.contains("200 OK") {
                    anyhow::bail!("DESCRIBE failed: {}", &resp2[..resp2.len().min(200)]);
                }
            } else if !resp.contains("200 OK") {
                anyhow::bail!("DESCRIBE failed: {}", &resp[..resp.len().min(200)]);
            }
        }

        let track_uri = format!("{}/trackID=1", channels_uri);
        cseq += 1;
        let setup = format!(
            "SETUP {} RTSP/1.0\r\n\
             CSeq: {}\r\n\
             Transport: RTP/AVP/TCP;unicast;interleaved=0-1\r\n\
             User-Agent: hikvision-rs\r\n\
             \r\n",
            track_uri, cseq
        );
        stream.write_all(setup.as_bytes())?;
        let setup_resp = Self::read_response(&mut stream)?;
        if !setup_resp.contains("200 OK") {
            anyhow::bail!("SETUP failed: {}", &setup_resp[..setup_resp.len().min(200)]);
        }

        cseq += 1;
        let session = Self::extract_session(&setup_resp);
        let play = format!(
            "PLAY {} RTSP/1.0\r\n\
             CSeq: {}\r\n\
             Session: {}\r\n\
             User-Agent: hikvision-rs\r\n\
             \r\n",
            channels_uri, cseq, session
        );
        stream.write_all(play.as_bytes())?;
        let play_resp = Self::read_response(&mut stream)?;
        if !play_resp.contains("200 OK") {
            anyhow::bail!("PLAY failed: {}", &play_resp[..play_resp.len().min(200)]);
        }

        log::info!("RTSP session established for channel {}", channel);
        Ok(Self { stream })
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
                        if let Some(cl) = header_str
                            .lines()
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

    fn read_rtp_frame(&mut self) -> Result<Option<(u8, Vec<u8>)>> {
        let mut header = [0u8; 4];

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
            return Ok(None);
        }

        if self.stream.read_exact(&mut header[1..2]).is_err() {
            return Ok(None);
        }
        let channel = header[1];

        if self.stream.read_exact(&mut header[2..4]).is_err() {
            return Ok(None);
        }
        let length = u16::from_be_bytes([header[2], header[3]]) as usize;

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

    if realm.is_empty() || nonce.is_empty() {
        anyhow::bail!("incomplete Digest challenge");
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

fn strip_rtp_header(data: &[u8]) -> &[u8] {
    if data.len() < 12 {
        return data;
    }
    let csrc_count = (data[0] & 0x0F) as usize;
    let extension = (data[0] >> 4) & 0x01 != 0;
    let mut offset = 12 + csrc_count * 4;
    if extension && data.len() > offset + 4 {
        let ext_len = u16::from_be_bytes([data[offset + 2], data[offset + 3]]) as usize;
        offset += 4 + ext_len * 4;
    }
    if offset < data.len() {
        &data[offset..]
    } else {
        &data[12..]
    }
}

pub fn stream_loop(
    host: &str,
    rtsp_port: u16,
    channel: &str,
    user: &str,
    password: &str,
    verification_code: &str,
    library_path: Option<&str>,
    tx: SyncSender<RtspFrame>,
    stop: Arc<AtomicBool>,
    repaint: egui::Context,
) {
    loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        match run_stream(
            host, rtsp_port, channel, user, password, verification_code,
            library_path, &tx, &stop, &repaint,
        ) {
            Ok(()) => return,
            Err(e) => {
                log::error!("PlayCtrl stream error: {}, reconnecting in 2s...", e);
                for _ in 0..20 {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }
    }
}

fn run_stream(
    host: &str,
    rtsp_port: u16,
    channel: &str,
    user: &str,
    password: &str,
    verification_code: &str,
    library_path: Option<&str>,
    tx: &SyncSender<RtspFrame>,
    stop: &Arc<AtomicBool>,
    repaint: &egui::Context,
) -> Result<()> {
    log::info!(
        "PlayCtrl stream: {}:{} ch={}",
        host, rtsp_port, channel
    );

    let playctrl = if let Some(path) = library_path {
        PlayCtrl::load_from(std::path::Path::new(path))?
    } else {
        playctrl::search_and_load()?
    };
    log::info!("libPlayCtrl.so loaded");

    let port = playctrl.get_port()?;
    log::info!("Allocated decoder port {}", port);

    if !verification_code.is_empty() {
        playctrl.set_secret_key(port, verification_code)?;
        log::info!("Secret key set ({} chars)", verification_code.len());
    }

    playctrl.open_stream(port, 2 * 1024 * 1024)?;
    log::info!("Stream opened (2 MB buffer)");

    let mut rtsp = RtspSession::connect(host, rtsp_port, channel, user, password)?;
    log::info!("RTSP session active");

    let mut frame_count = 0u64;
    let mut byte_count = 0u64;
    let start = Instant::now();
    let mut last_frame_extract = Instant::now();
    let extract_interval = Duration::from_millis(100);

    while !stop.load(Ordering::Relaxed) {
        match rtsp.read_rtp_frame() {
            Ok(Some((_ch, rtp_data))) => {
                let payload = strip_rtp_header(&rtp_data);
                if !payload.is_empty() {
                    frame_count += 1;
                    byte_count += payload.len() as u64;
                    if let Err(e) = playctrl.input_data(port, payload) {
                        let err_code = playctrl.get_last_error();
                        if err_code == 29 {
                            log::warn!(
                                "PLAYM4_SECRET_KEY_ERROR (29): verification code '{}' may be incorrect",
                                verification_code
                            );
                        } else if frame_count % 100 == 0 {
                            log::debug!("InputData warning: {} (error {})", e, err_code);
                        }
                    }
                }

                if last_frame_extract.elapsed() >= extract_interval {
                    last_frame_extract = Instant::now();
                    match playctrl.get_jpeg(port) {
                        Ok(jpeg) if !jpeg.is_empty() => {
                            match jpeg_to_rgba(&jpeg) {
                                Ok(frame) => {
                                    let _ = tx.try_send(frame);
                                    repaint.request_repaint();
                                }
                                Err(e) => {
                                    log::debug!("JPEG decode error: {}", e);
                                }
                            }

                            let elapsed = start.elapsed().as_secs_f64();
                            if elapsed > 0.0 && frame_count % 100 == 0 {
                                log::info!(
                                    "{:.1} fps, {:.2} MB received, {} frames fed",
                                    frame_count as f64 / elapsed,
                                    byte_count as f64 / 1_000_000.0,
                                    frame_count,
                                );
                            }
                        }
                        Ok(_) => {}
                        Err(e) => {
                            let err_code = playctrl.get_last_error();
                            if frame_count > 50 && frame_count % 200 == 0 {
                                log::warn!(
                                    "No JPEG after {} frames (error {}: {})",
                                    frame_count,
                                    err_code,
                                    crate::playctrl::last_error_name(err_code),
                                );
                            }
                            if frame_count % 50 == 0 {
                                log::debug!("GetJPEG error: {}", e);
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

    let _ = playctrl.close_stream(port);
    let _ = playctrl.free_port(port);
    let elapsed = start.elapsed().as_secs_f64();
    log::info!(
        "Session ended. {} frames fed, {:.1} seconds",
        frame_count, elapsed
    );
    Ok(())
}

fn jpeg_to_rgba(jpeg_data: &[u8]) -> Result<RtspFrame> {
    let mut decoder = jpeg_decoder::Decoder::new(std::io::Cursor::new(jpeg_data));
    decoder.read_info().context("JPEG read info failed")?;

    let info = match decoder.info() {
        Some(i) => i,
        None => anyhow::bail!("JPEG no info after read"),
    };
    let w = info.width as usize;
    let h = info.height as usize;

    let pixels = decoder.decode().context("JPEG decode failed")?;

    let rgba = match info.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 => {
            let mut rgba = Vec::with_capacity(w * h * 4);
            for rgb in pixels.chunks(3) {
                rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
            }
            rgba
        }
        jpeg_decoder::PixelFormat::CMYK32 => {
            let mut rgba = Vec::with_capacity(w * h * 4);
            for c in pixels.chunks(4) {
                let k = c[3] as f32 / 255.0;
                let r = (c[0] as f32 * k) as u8;
                let g = (c[1] as f32 * k) as u8;
                let b = (c[2] as f32 * k) as u8;
                rgba.extend_from_slice(&[r, g, b, 255]);
            }
            rgba
        }
        jpeg_decoder::PixelFormat::L8 => {
            let mut rgba = Vec::with_capacity(w * h * 4);
            for &l in &pixels {
                rgba.extend_from_slice(&[l, l, l, 255]);
            }
            rgba
        }
        jpeg_decoder::PixelFormat::L16 => {
            let mut rgba = Vec::with_capacity(w * h * 4);
            for c in pixels.chunks(2) {
                let l = u16::from_be_bytes([c[0], c[1]]) as u8;
                rgba.extend_from_slice(&[l, l, l, 255]);
            }
            rgba
        }
    };

    Ok(RtspFrame {
        width: w as u32,
        height: h as u32,
        rgba,
    })
}
