use anyhow::{Context, Result};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use crate::encrypted_stream::{decrypt_rtp_payload, derive_verification_code_key};
use crate::netstream::NetStream;
use crate::playctrl::{self, PlayCtrl};
use crate::rtsp::RtspFrame;

/// Classified error for zero channel operations.
///
/// Used to decide fallback and retry strategy: whether to try the next channel ID,
/// retry the same one with backoff, or abort entirely.
#[derive(Debug)]
#[allow(dead_code)]
enum ZeroChannelError {
    /// Device does not have zero channel capability (404, 461, SDK error 953)
    NotSupported,
    /// Device has zero channel but it's not enabled in settings
    NotEnabled,
    /// Authentication failed (401, 403) — retrying other IDs won't help
    AuthFailed,
    /// Verification code is required but missing
    VerificationCodeRequired,
    /// Network unreachable (connection refused, timeout)
    ConnectionRefused,
    /// Generic stream/decode error — may be transient
    StreamError(String),
}

impl std::fmt::Display for ZeroChannelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotSupported => write!(f, "Canal Zero não suportado pelo dispositivo"),
            Self::NotEnabled => write!(f, "Canal Zero não está ativado nas configurações do DVR"),
            Self::AuthFailed => write!(f, "Falha de autenticação (credenciais inválidas)"),
            Self::VerificationCodeRequired => write!(f, "Verification Code obrigatório para Canal Zero"),
            Self::ConnectionRefused => write!(f, "Conexão recusada/host inalcançável"),
            Self::StreamError(msg) => write!(f, "Erro de stream: {}", msg),
        }
    }
}

fn classify_zero_channel_error(e: &anyhow::Error) -> ZeroChannelError {
    let err_str = e.to_string().to_lowercase();
    let err_source = e.source().map(|s| s.to_string()).unwrap_or_default().to_lowercase();
    let combined = format!("{} {}", err_str, err_source);

    // SDK error 953 = NET_ERR_NO_ZERO_CHAN
    if combined.contains("953") || combined.contains("no_zero_chan") {
        return ZeroChannelError::NotSupported;
    }
    // RTSP/HTTP: not found, unsupported transport, not ready
    if combined.contains("404") || combined.contains("461") || combined.contains("462") {
        return ZeroChannelError::NotSupported;
    }
    // Auth failures — no point trying other IDs
    if combined.contains("401") || combined.contains("403") || combined.contains("unauthorized") {
        return ZeroChannelError::AuthFailed;
    }
    // Connection refused / timeout
    if combined.contains("refused") || combined.contains("timed out") || combined.contains("timeout") {
        return ZeroChannelError::ConnectionRefused;
    }
    // Describe/setup/play RTSP failures that indicate channel doesn't exist
    if combined.contains("describe failed") || combined.contains("setup failed") {
        return ZeroChannelError::NotSupported;
    }

    ZeroChannelError::StreamError(e.to_string())
}

use ffmpeg_next::codec::{self as codec_mod, Context as CodecContext, Id as CodecId};
use ffmpeg_next::packet::Packet;
use ffmpeg_next::software::scaling::Context as SwsContext;
use ffmpeg_next::format::Pixel;
use ffmpeg_next::frame::Video as VideoFrame;

static PLAYCTRL: OnceLock<PlayCtrl> = OnceLock::new();

fn get_cached_playctrl(library_path: Option<&str>) -> Result<&'static PlayCtrl> {
    if let Some(pc) = PLAYCTRL.get() {
        return Ok(pc);
    }
    log::info!("Loading libPlayCtrl.so (cached once)");
    let pc = if let Some(path) = library_path {
        PlayCtrl::load_from(std::path::Path::new(path))?
    } else {
        playctrl::search_and_load()?
    };
    // Race-safe: if another thread already set it, ours is dropped (harmless)
    let _ = PLAYCTRL.set(pc);
    Ok(PLAYCTRL.get().unwrap())
}

struct RtspSession {
    stream: TcpStream,
}

/// Store the most recent Digest challenge fields so we can pre-emptively
/// include an Authorization header without first getting a 401.
struct DigestState {
    realm: String,
    nonce: String,
    opaque: String,
    qop: String,
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
        let mut digest: Option<DigestState> = None;

        // Helper closure: send request, optionally with pre-computed auth.
        // On 401, compute fresh auth and retry.
        let mut send_request = |method: &str, uri: &str, extra_headers: &str| -> Result<String> {
            cseq += 1;

            // Build an Authorization header from cached challenge, if we have one
            let auth_header = digest.as_ref().map(|d| {
                let ha1 = format!("{:x}", md5::compute(format!("{}:{}:{}", user, d.realm, password).as_bytes()));
                let ha2 = format!("{:x}", md5::compute(format!("{}:{}", method, uri).as_bytes()));
                let (response, suffix) = if !d.qop.is_empty() {
                    let cnonce = format!("{:x}", md5::compute(format!("{}{}", d.nonce, user).as_bytes()));
                    let resp = format!(
                        "{:x}",
                        md5::compute(format!("{}:{}:00000001:{}:auth:{}", ha1, d.nonce, cnonce, ha2).as_bytes())
                    );
                    (resp, format!(", nc=00000001, cnonce=\"{}\", qop=auth", cnonce))
                } else {
                    let resp = format!(
                        "{:x}",
                        md5::compute(format!("{}:{}:{}", ha1, d.nonce, ha2).as_bytes())
                    );
                    (resp, String::new())
                };
                let mut h = format!(
                    "Digest username=\"{}\", realm=\"{}\", nonce=\"{}\", uri=\"{}\", response=\"{}\"{}",
                    user, d.realm, d.nonce, uri, response, suffix,
                );
                if !d.opaque.is_empty() {
                    h.push_str(&format!(", opaque=\"{}\"", d.opaque));
                }
                h
            });

            let auth_line = auth_header
                .map(|a| format!("Authorization: {}\r\n", a))
                .unwrap_or_default();

            let req = format!(
                "{} {} RTSP/1.0\r\n\
                 CSeq: {}\r\n\
                 {}\
                 {}\
                 User-Agent: hikvision-rs\r\n\
                 \r\n",
                method, uri, cseq, auth_line, extra_headers,
            );
            log::warn!(">>> {} CSeq:{}", method, cseq);
            stream.write_all(req.as_bytes())?;
            let resp = Self::read_response(&mut stream)?;
            let status = resp.lines().next().unwrap_or("?").to_string();
            log::warn!("<<< {} ({} bytes)", status, resp.len());

            let parse_challenge = |resp: &str| -> Option<DigestState> {
                let challenge = resp
                    .lines()
                    .find(|l| l.to_lowercase().contains("www-authenticate"))
                    .and_then(|l| l.splitn(2, ':').nth(1))
                    .map(|s| s.trim())?;

                let body = challenge
                    .strip_prefix("Digest ")
                    .or_else(|| challenge.strip_prefix("digest "))
                    .unwrap_or(challenge);

                let mut realm = String::new();
                let mut nonce = String::new();
                let mut opaque = String::new();
                let mut qop = String::new();
                for part in body.split(',') {
                    let part = part.trim();
                    if let Some(eq) = part.find('=') {
                        let key = part[..eq].trim();
                        let val = part[eq + 1..].trim().trim_matches('"');
                        match key.to_lowercase().as_str() {
                            "realm" => realm = val.to_string(),
                            "nonce" => nonce = val.to_string(),
                            "opaque" => opaque = val.to_string(),
                            "qop" => qop = val.to_string(),
                            _ => {}
                        }
                    }
                }
                if realm.is_empty() || nonce.is_empty() {
                    None
                } else {
                    Some(DigestState { realm, nonce, opaque, qop })
                }
            };

            if resp.contains("401 Unauthorized") {
                let challenge = parse_challenge(&resp)
                    .ok_or_else(|| anyhow::anyhow!("401 without valid WWW-Authenticate"))?;

                log::warn!("WWW-Authenticate: realm={} nonce={}...", challenge.realm, &challenge.nonce[..challenge.nonce.len().min(8)]);
                let ha1 = format!("{:x}", md5::compute(format!("{}:{}:{}", user, challenge.realm, password).as_bytes()));
                let ha2 = format!("{:x}", md5::compute(format!("{}:{}", method, uri).as_bytes()));
                let (response, suffix) = if !challenge.qop.is_empty() {
                    let cnonce = format!("{:x}", md5::compute(format!("{}{}", challenge.nonce, user).as_bytes()));
                    let resp = format!(
                        "{:x}",
                        md5::compute(format!("{}:{}:00000001:{}:auth:{}", ha1, challenge.nonce, cnonce, ha2).as_bytes())
                    );
                    (resp, format!(", nc=00000001, cnonce=\"{}\", qop=auth", cnonce))
                } else {
                    let resp = format!(
                        "{:x}",
                        md5::compute(format!("{}:{}:{}", ha1, challenge.nonce, ha2).as_bytes())
                    );
                    (resp, String::new())
                };
                let mut digest_str = format!(
                    "Digest username=\"{}\", realm=\"{}\", nonce=\"{}\", uri=\"{}\", response=\"{}\"{}",
                    user, challenge.realm, challenge.nonce, uri, response, suffix,
                );
                if !challenge.opaque.is_empty() {
                    digest_str.push_str(&format!(", opaque=\"{}\"", challenge.opaque));
                }
                log::warn!("Authorization: {}", digest_str);

                // Cache the challenge for future requests
                digest = Some(DigestState {
                    realm: challenge.realm.clone(),
                    nonce: challenge.nonce.clone(),
                    opaque: challenge.opaque.clone(),
                    qop: challenge.qop.clone(),
                });

                cseq += 1;
                let req2 = format!(
                    "{} {} RTSP/1.0\r\n\
                     CSeq: {}\r\n\
                     Authorization: {}\r\n\
                     {}\
                     User-Agent: hikvision-rs\r\n\
                     \r\n",
                    method, uri, cseq, digest_str, extra_headers,
                );
                log::warn!(">>> {} CSeq:{} (with auth)", method, cseq);
                stream.write_all(req2.as_bytes())?;
                let resp2 = Self::read_response(&mut stream)?;
                let status2 = resp2.lines().next().unwrap_or("?").to_string();
                log::warn!("<<< {} ({} bytes)", status2, resp2.len());
                Ok(resp2)
            } else {
                Ok(resp)
            }
        };

        // DESCRIBE
        let resp = send_request("DESCRIBE", &channels_uri, "Accept: application/sdp\r\n")?;
        if !resp.contains("200 OK") {
            anyhow::bail!("DESCRIBE failed: {}", &resp[..resp.len().min(200)]);
        }
        log::warn!("SDP response:\n{}", &resp[..resp.len().min(2048)]);

        // Extract track control from SDP
        let track_id = resp.lines()
            .find(|l| l.trim().starts_with("a=control:trackID="))
            .and_then(|l| l.split('=').nth(2))
            .unwrap_or("1");
        log::warn!("Using trackID={}", track_id);

        // SETUP with correct trackID from SDP
        let track_uri = format!("{}/trackID={}", channels_uri, track_id);
        let setup_resp = send_request("SETUP", &track_uri,
            "Transport: RTP/AVP/TCP;unicast;interleaved=0-1\r\n")?;
        if !setup_resp.contains("200 OK") {
            anyhow::bail!("SETUP failed: {}", &setup_resp[..setup_resp.len().min(200)]);
        }
        log::warn!("SETUP response:\n{}", &setup_resp[..setup_resp.len().min(1024)]);

        // PLAY — use the same track_uri (not channels_uri), required for non-standard trackIDs like "video"
        let session = Self::extract_session(&setup_resp);
        log::warn!("Extracted session: '{}'", session);
        let play_extra = format!("Session: {}\r\nRange: npt=now-\r\n", session);
        let play_resp = send_request("PLAY", &track_uri, &play_extra)?;
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

fn _strip_rtp_header(data: &[u8]) -> &[u8] {
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
    let playctrl = match get_cached_playctrl(library_path) {
        Ok(p) => p,
        Err(e) => {
            log::error!("Failed to load PlayCtrl: {}", e);
            return;
        }
    };

    loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        match run_stream(
            playctrl, host, rtsp_port, channel, user, password, verification_code,
            &tx, &stop, &repaint,
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
    playctrl: &PlayCtrl,
    host: &str,
    rtsp_port: u16,
    channel: &str,
    user: &str,
    password: &str,
    verification_code: &str,
    tx: &SyncSender<RtspFrame>,
    stop: &Arc<AtomicBool>,
    repaint: &egui::Context,
) -> Result<()> {
    log::warn!(
        "PlayCtrl stream: {}:{} ch={}",
        host, rtsp_port, channel
    );

    let port = playctrl.get_port()?;
    log::warn!("Allocated decoder port {}", port);

    // Derive AES key from verification code for manual decryption fallback
    let aes_key = if !verification_code.is_empty() {
        let key = derive_verification_code_key(verification_code);
        log::info!("Derived AES-256 key from verification code (SHA256): {:02x?}...", &key[..4]);
        Some(key)
    } else {
        None
    };

    if !verification_code.is_empty() {
        log::warn!("Setting PlayM4_SetSecretKey (verification code '{}') on port {} (before OpenStream)", verification_code, port);
        match playctrl.set_secret_key(port, verification_code) {
            Ok(()) => log::info!("PlayM4_SetSecretKey OK"),
            Err(e) => log::warn!("PlayM4_SetSecretKey failed: {} (error {})", e, playctrl.get_last_error(port)),
        }
    }

    playctrl.open_stream(port, 2 * 1024 * 1024)?;
    log::info!("Stream opened (2 MB buffer)");

    // Try to load NetStream for decryption fallback
    let netstream = (|| -> Option<NetStream> {
        match NetStream::load() {
            Ok(ns) => {
                log::info!("NetStream loaded successfully");
                match ns.set_secret_key(0, verification_code) {
                    Ok(()) => {
                        log::info!("NetStream key set successfully (key_idx=0)");
                        Some(ns)
                    }
                    Err(e) => {
                        log::warn!("NetStream key set failed: {}", e);
                        None
                    }
                }
            }
            Err(e) => {
                log::warn!("NetStream load failed: {}", e);
                None
            }
        }
    })();
    if netstream.is_none() && !verification_code.is_empty() {
        log::warn!("No decryption available (PlayCtrl failed, NetStream unavailable)");
    }

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
                if rtp_data.len() <= 12 { continue; }

                // Manual AES decryption of RTP payload before feeding to PlayCtrl
                let data_to_feed = if let Some(ref key) = aes_key {
                    let payload = &rtp_data[12..];
                    let decrypted = decrypt_rtp_payload(payload, key);
                    if decrypted != payload {
                        if frame_count < 3 {
                            log::warn!("Decrypted RTP payload ({} bytes -> {} bytes)", payload.len(), decrypted.len());
                            if !decrypted.is_empty() {
                                log::warn!("First 16 decrypted bytes: {:02x?}", &decrypted[..decrypted.len().min(16)]);
                            }
                        }
                        // Rebuild RTP packet with decrypted payload
                        let mut modified = Vec::with_capacity(12 + decrypted.len());
                        modified.extend_from_slice(&rtp_data[..12]);
                        modified.extend_from_slice(&decrypted);
                        modified
                    } else {
                        if frame_count < 3 {
                            log::warn!("Decryption returned same payload ({} bytes) - no change", payload.len());
                        }
                        rtp_data.to_vec()
                    }
                } else {
                    rtp_data.to_vec()
                };

                frame_count += 1;
                byte_count += data_to_feed.len() as u64;
                if let Err(e) = playctrl.input_data(port, &data_to_feed) {
                    let err_code = playctrl.get_last_error(port);
                    if frame_count < 5 {
                        log::warn!("InputData error {}: {} (error {}: {})", frame_count, e, err_code,
                            crate::playctrl::last_error_name(err_code));
                    }
                    if err_code == 29 {
                        log::warn!(
                            "PLAYM4_SECRET_KEY_ERROR (29): verification code '{}' may be incorrect",
                            verification_code
                        );
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
                            let err_code = playctrl.get_last_error(port);
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

/// FFmpeg-based decoder for channel zero with manual AES decryption.
///
/// Bypasses PlayCtrl entirely (which can't handle channel 001's multiplexed stream).
/// Derives AES key from verification code, decrypts RTP payloads, then feeds
/// the raw H.264 NAL units to FFmpeg's decoder.
///
/// # Channel Zero Notes
///
/// O Canal Zero (Channel Zero) é um recurso de NVRs/DVRs Hikvision que permite
/// visualizar múltiplas câmeras em um único stream multiplexado (formato grid).
///
/// - **ID do canal**: Geralmente "0", "1" ou "001" dependendo do modelo
/// - **Stream**: Apenas stream principal (dwStreamType=0), sub-stream não disponível
/// - **Erro 953**: Indica que o dispositivo não suporta Canal Zero ou não está ativado
///
/// Para ativar: Acesse a interface web do DVR > Configurações > Visualização > Canal Zero
pub fn zero_channel_stream_loop(
    host: &str,
    rtsp_port: u16,
    channel: &str,
    user: &str,
    password: &str,
    verification_code: &str,
    tx: SyncSender<RtspFrame>,
    stop: Arc<AtomicBool>,
    repaint: egui::Context,
) {
    log::info!("Zero channel stream loop started for channel {}", channel);

    if verification_code.trim().is_empty() {
        log::error!("Zero channel requer Verification Code para descriptografia");
        let _ = tx.try_send(RtspFrame {
            width: 640,
            height: 480,
            rgba: vec![0u8; 640 * 480 * 4],
        });
        return;
    }

    let channel_ids_to_try: Vec<&str> = if channel == "001" || channel == "0" || channel == "1" {
        vec!["001", "0", "1"]
    } else {
        vec![channel]
    };

    // Track the last error per channel ID for final summary
    let mut id_errors: Vec<(&str, ZeroChannelError)> = Vec::new();

    for channel_id in &channel_ids_to_try {
        if stop.load(Ordering::Relaxed) {
            return;
        }

        log::info!("Tentando Canal Zero com ID: {}", channel_id);

        let mut backoff_secs: f64 = 1.0;
        const MAX_BACKOFF_SECS: f64 = 30.0;

        loop {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            match run_zero_channel(
                host, rtsp_port, channel_id, user, password, verification_code,
                &tx, &stop, &repaint,
            ) {
                Ok(()) => {
                    log::info!("Zero channel session ended normally (ID={})", channel_id);
                    return;
                }
                Err(e) => {
                    let classified = classify_zero_channel_error(&e);

                    match classified {
                        ZeroChannelError::NotSupported | ZeroChannelError::NotEnabled => {
                            log::warn!("Canal Zero ID {} → {}: {}", channel_id, classified, e);
                            id_errors.push((*channel_id, classified));
                            break; // Try next channel ID
                        }
                        ZeroChannelError::AuthFailed => {
                            log::error!("Autenticação falhou para Canal Zero ID {}: {} — abortando (outros IDs também falharão)", channel_id, e);
                            id_errors.push((*channel_id, ZeroChannelError::AuthFailed));
                            // No point trying other IDs with same credentials
                            log_id_errors_summary(&id_errors);
                            return;
                        }
                        ZeroChannelError::VerificationCodeRequired => {
                            log::error!("Verification Code obrigatório para Canal Zero");
                            id_errors.push((*channel_id, ZeroChannelError::VerificationCodeRequired));
                            log_id_errors_summary(&id_errors);
                            return;
                        }
                        ZeroChannelError::ConnectionRefused => {
                            log::warn!("Canal Zero ID {} → conexão recusada: {}", channel_id, e);
                            id_errors.push((*channel_id, ZeroChannelError::ConnectionRefused));
                            break; // Try next ID (different model may use different port/path)
                        }
                        ZeroChannelError::StreamError(_) => {
                            log::error!("Zero channel stream error (ID={}): {}, reconectando em {:.0}s...", channel_id, e, backoff_secs);
                            sleep_with_stop(&stop, backoff_secs);
                            backoff_secs = (backoff_secs * 2.0).min(MAX_BACKOFF_SECS);
                        }
                    }
                }
            }
        }
    }

    log_id_errors_summary(&id_errors);
}

fn log_id_errors_summary(errors: &[(impl AsRef<str>, ZeroChannelError)]) {
    if errors.is_empty() {
        log::error!("Todos os IDs de Canal Zero falharam (sem detalhes)");
        return;
    }
    log::error!("Todos os IDs de Canal Zero falharam:");
    for (id, err) in errors {
        log::error!("  ID {} → {}", id.as_ref(), err);
    }
}

fn sleep_with_stop(stop: &AtomicBool, seconds: f64) {
    let millis = (seconds * 1000.0) as u64;
    let check_interval = Duration::from_millis(100);
    let mut remaining = Duration::from_millis(millis);
    while remaining > Duration::ZERO {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let sleep_time = remaining.min(check_interval);
        std::thread::sleep(sleep_time);
        remaining = remaining.saturating_sub(check_interval);
    }
}

fn run_zero_channel(
    host: &str,
    rtsp_port: u16,
    channel: &str,
    user: &str,
    password: &str,
    verification_code: &str,
    tx: &SyncSender<RtspFrame>,
    stop: &Arc<AtomicBool>,
    repaint: &egui::Context,
) -> Result<()> {
    log::warn!("Zero channel (FFmpeg+decrypt): {}:{} ch={}", host, rtsp_port, channel);

    // Derive AES key from verification code (SHA256)
    let aes_key = if !verification_code.is_empty() {
        let key = derive_verification_code_key(verification_code);
        log::info!("Zero ch: derived AES-256 key from verification code (SHA256): {:02x?}...", &key[..8]);
        Some(key)
    } else {
        log::warn!("Zero ch: no verification code provided, decryption disabled");
        None
    };

    if verification_code.is_empty() {
        log::error!("Zero ch: Verification code é obrigatório para Canal Zero criptografado");
        anyhow::bail!("Verification code é obrigatório para Canal Zero");
    }

    // Connect RTSP
    log::info!("Conectando RTSP para Canal Zero: {}:{}/Streaming/Channels/{}", host, rtsp_port, channel);
    let mut rtsp = RtspSession::connect(host, rtsp_port, channel, user, password)?;
    log::info!("Zero ch: RTSP session active");

    // Create FFmpeg H.264 decoder
    let h264_codec = codec_mod::decoder::find(CodecId::H264)
        .ok_or_else(|| anyhow::anyhow!("H.264 decoder not found"))?;
    let codec_ctx = CodecContext::new_with_codec(h264_codec);
    let mut decoder = codec_ctx.decoder().video()?;
    unsafe {
        (*decoder.as_mut_ptr()).thread_count = 2;
    }

    let mut scaler: Option<SwsContext> = None;
    let mut decoded_frame = VideoFrame::empty();
    let mut rgba_frame = VideoFrame::empty();

    let mut frame_count = 0u64;
    let mut byte_count = 0u64;
    let start = Instant::now();
    let mut pts: i64 = 0;

    // Buffer for accumulating NAL units into Annex B format
    let mut nal_buf = Vec::with_capacity(65536);

    while !stop.load(Ordering::Relaxed) {
        match rtsp.read_rtp_frame() {
            Ok(Some((_ch, rtp_data))) => {
                if rtp_data.len() <= 12 { continue; }
                let payload = &rtp_data[12..];

                // Decrypt RTP payload if we have a key
                let decrypted = if let Some(ref key) = aes_key {
                    let d = decrypt_rtp_payload(payload, key);
                    if d != payload && frame_count < 2 {
                        log::warn!("Zero ch: decrypted {} -> {} bytes", payload.len(), d.len());
                        if !d.is_empty() {
                            let first = d[0];
                            let nal_type = first & 0x1F;
                            log::warn!("Zero ch: first decrypted byte = 0x{:02x} (NAL type {})", first, nal_type);
                        }
                    }
                    d
                } else {
                    payload.to_vec()
                };

                if decrypted.is_empty() { continue; }

                if frame_count < 3 {
                    log::info!("Zero ch: Frame {} - {} bytes, NAL type: {}, keyframe: {}",
                        frame_count,
                        decrypted.len(),
                        decrypted[0] & 0x1F,
                        (decrypted[0] & 0x1F) == 5
                    );
                }

                // Parse RTP payload format and extract NAL units
                // Single NAL unit: [NAL header | NAL body]
                // FU-A: [FU indicator | FU header | NAL body fragments]
                // STAP-A: [STAP header | NALU1 size | NALU1 | ...]
                let nal_type = decrypted[0] & 0x1F;

                match nal_type {
                    1..=23 => {
                        // Single NAL unit packet
                        nal_buf.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
                        nal_buf.extend_from_slice(&decrypted);
                    }
                    24 => {
                        // STAP-A: aggregation packet
                        let mut off = 1; // Skip STAP-A NAL header
                        while off + 2 < decrypted.len() {
                            let nalu_size = u16::from_be_bytes([decrypted[off], decrypted[off + 1]]) as usize;
                            off += 2;
                            if off + nalu_size > decrypted.len() { break; }
                            nal_buf.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
                            nal_buf.extend_from_slice(&decrypted[off..off + nalu_size]);
                            off += nalu_size;
                        }
                    }
                    28 => {
                        // FU-A: fragmentation unit
                        if decrypted.len() < 2 { continue; }
                        let fu_header = decrypted[1];
                        let start_bit = (fu_header >> 7) & 0x01;
                        let end_bit = (fu_header >> 6) & 0x01;
                        // Reconstruct NAL header: keep forbidden+ref_idc from FU indicator, NAL type from FU header
                        let nal_header = (decrypted[0] & 0xE0) | (fu_header & 0x1F);
                        if start_bit != 0 {
                            // Start of fragmented NAL unit
                            if !nal_buf.is_empty() {
                                // Flush previous incomplete NAL unit
                                log::warn!("Zero ch: flushing {} bytes (incomplete NAL)", nal_buf.len());
                            }
                            nal_buf.clear();
                            nal_buf.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
                            nal_buf.push(nal_header);
                        }
                        if decrypted.len() > 2 {
                            nal_buf.extend_from_slice(&decrypted[2..]);
                        }
                        if end_bit != 0 {
                            // Complete NAL unit ready - flush below via pts increment
                        }
                    }
                    _ => {
                        log::debug!("Zero ch: unknown NAL type {} (0x{:02x})", nal_type, decrypted[0]);
                    }
                }

                frame_count += 1;
                byte_count += rtp_data.len() as u64;

                // FU-A end bit completou o NAL unit
                if nal_type == 28 && decrypted.len() >= 2 {
                    let fu_header = decrypted[1];
                    let end_bit = (fu_header >> 6) & 0x01;
                    if end_bit != 0 {
                        log::trace!("Zero ch: FU-A end bit detected, flushing {} bytes", nal_buf.len());
                    }
                }

                // When we have accumulated data and see a FU-A end bit, or periodically,
                // flush the buffer to the decoder
                if nal_buf.len() > 1024 || (frame_count % 30 == 0 && nal_buf.len() > 100) {
                    let mut packet = Packet::new(nal_buf.len());
                    if let Some(data) = packet.data_mut() {
                        data.copy_from_slice(&nal_buf);
                    }
                    packet.set_pts(Some(pts));
                    packet.set_stream(0);

                    match decoder.send_packet(&packet) {
                        Ok(()) => {
                            loop {
                                match decoder.receive_frame(&mut decoded_frame) {
                                    Ok(()) => {
                                        // Create scaler lazily on first frame
                                        let sws = match scaler.as_mut() {
                                            Some(s) => s,
                                            None => {
                                                scaler = Some(
                                                    SwsContext::get(
                                                        decoded_frame.format(),
                                                        decoded_frame.width(),
                                                        decoded_frame.height(),
                                                        Pixel::RGBA,
                                                        decoded_frame.width(),
                                                        decoded_frame.height(),
                                                        ffmpeg_next::software::scaling::Flags::BILINEAR,
                                                    )
                                                    .context("Zero ch: failed to create scaler")?,
                                                );
                                                scaler.as_mut().unwrap()
                                            }
                                        };

                                        match sws.run(&decoded_frame, &mut rgba_frame) {
                                            Ok(()) => {
                                                let w = rgba_frame.width();
                                                let h = rgba_frame.height();
                                                let stride = rgba_frame.stride(0);
                                                let data = rgba_frame.data(0);
                                                let row_bytes = w as usize * 4;

                                                let rgba = if stride == row_bytes {
                                                    data[..row_bytes * h as usize].to_vec()
                                                } else {
                                                    let mut buf = Vec::with_capacity(row_bytes * h as usize);
                                                    for row in 0..h as usize {
                                                        let start = row * stride;
                                                        buf.extend_from_slice(&data[start..start + row_bytes]);
                                                    }
                                                    buf
                                                };

                                                let rtsp_frame = RtspFrame {
                                                    width: w,
                                                    height: h,
                                                    rgba,
                                                };
                                                let _ = tx.try_send(rtsp_frame);
                                                repaint.request_repaint();

                                                let elapsed = start.elapsed().as_secs_f64();
                                                if elapsed > 0.0 && frame_count % 50 == 0 {
                                                    log::info!(
                                                        "Zero ch: {:.1} fps, {:.2} MB, {}x{}",
                                                        frame_count as f64 / elapsed,
                                                        byte_count as f64 / 1_000_000.0,
                                                        w, h,
                                                    );
                                                }
                                            }
                                            Err(e) => {
                                                if frame_count % 100 == 0 {
                                                    log::warn!("Zero ch: scaler error: {}", e);
                                                }
                                            }
                                        }
                                    }
                                    Err(ffmpeg_next::Error::Eof | ffmpeg_next::Error::InvalidData) => break,
                                    Err(e) => {
                                        if frame_count % 50 == 0 {
                                            log::debug!("Zero ch: receive_frame error: {}", e);
                                        }
                                        break;
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            if frame_count < 5 {
                                log::warn!("Zero ch: send_packet error: {}", e);
                            }
                        }
                    }

                    pts += 1;
                    nal_buf.clear();
                }
            }
            Ok(None) => {
                log::info!("Zero ch: RTSP stream ended");
                break;
            }
            Err(e) => {
                log::error!("Zero ch: RTSP read error: {}", e);
                break;
            }
        }
    }

    let elapsed = start.elapsed().as_secs_f64();
    let fps = if elapsed > 0.0 { frame_count as f64 / elapsed } else { 0.0 };
    log::info!(
        "Zero ch: session ended. {} frames, {:.1} seconds, {:.1} FPS",
        frame_count, elapsed, fps
    );
    log::info!(
        "Zero ch: total received: {:.2} MB, decoder frames: {}",
        byte_count as f64 / 1_000_000.0,
        frame_count
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
