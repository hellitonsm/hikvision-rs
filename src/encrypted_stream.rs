use aes::Aes256;
use anyhow::{bail, Context, Result};
use cbc::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use cbc::{Decryptor, Encryptor};
use num_bigint::BigUint;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tungstenite::protocol::Message;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::WebSocket;

pub const MSG_TYPE_HELLO: u8 = 0x01;
pub const MSG_TYPE_AUTH_REQUEST: u8 = 0x02;
pub const MSG_TYPE_AUTH_RESPONSE: u8 = 0x03;
pub const MSG_TYPE_KEY_EXCHANGE: u8 = 0x04;
pub const MSG_TYPE_SESSION_ERROR: u8 = 0x05;
pub const MSG_TYPE_KEEPALIVE: u8 = 0x06;
pub const MSG_TYPE_START_PREVIEW: u8 = 0x20;
pub const MSG_TYPE_VIDEO_DATA: u8 = 0x40;
pub const MSG_TYPE_AUDIO_DATA: u8 = 0x41;

const HIK_FIXED_KEY: [u8; 32] = [
    0x12, 0x34, 0x56, 0x78, 0x91, 0x23, 0x45, 0x67, 0x12, 0x34, 0x56, 0x78, 0x91, 0x23, 0x45,
    0x67, 0x12, 0x34, 0x56, 0x78, 0x91, 0x23, 0x45, 0x67, 0x12, 0x34, 0x56, 0x78, 0x91, 0x23,
    0x45, 0x67,
];

const HIK_FIXED_IV: [u8; 16] = [
    0x12, 0x34, 0x56, 0x78, 0x91, 0x23, 0x45, 0x67, 0x12, 0x34, 0x56, 0x78, 0x91, 0x23, 0x45,
    0x67,
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthRequest {
    pub username: String,
    pub password: String,
    #[serde(rename = "clientType")]
    pub client_type: String,
    #[serde(rename = "keyVersion")]
    pub key_version: String,
    pub random: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerKeyExchange {
    #[serde(rename = "PKD")]
    pub pkd: Option<String>,
    pub rand: Option<String>,
    #[serde(rename = "type")]
    pub msg_type: Option<String>,
    #[serde(rename = "cipherSuite")]
    pub cipher_suite: Option<serde_json::Value>,
    #[serde(rename = "errorCode")]
    pub error_code: Option<u64>,
    #[serde(rename = "errorMsg")]
    pub error_msg: Option<String>,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RealplayRequest {
    pub sequence: u32,
    pub cmd: String,
    pub url: String,
    pub key: String,
    pub authorization: String,
    pub token: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RealplayResponse {
    #[serde(rename = "errorCode")]
    pub error_code: Option<u64>,
    #[serde(rename = "errorMsg")]
    pub error_msg: Option<String>,
    pub sdp: Option<String>,
    pub sequence: Option<u32>,
}

pub fn pack_message(msg_type: u8, data: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(5 + data.len());
    buf.push(msg_type);
    buf.extend_from_slice(&(data.len() as u32).to_be_bytes());
    buf.extend_from_slice(data);
    buf
}

pub fn unpack_message(data: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    if data.len() < 5 {
        return None;
    }
    let msg_type = data[0];
    let length = u32::from_be_bytes([data[1], data[2], data[3], data[4]]) as usize;
    if data.len() < 5 + length {
        return None;
    }
    Some((msg_type, &data[5..5 + length], &data[5 + length..]))
}

fn pkcs7_pad(data: &[u8], block_size: usize) -> Vec<u8> {
    let padding = block_size - (data.len() % block_size);
    let mut padded = data.to_vec();
    padded.extend(std::iter::repeat(padding as u8).take(padding));
    padded
}

#[allow(dead_code)]
fn pkcs7_unpad(data: &[u8]) -> Result<&[u8]> {
    if data.is_empty() {
        bail!("empty data for unpad");
    }
    let padding = *data.last().unwrap() as usize;
    if padding == 0 || padding > 16 || padding > data.len() {
        bail!("invalid PKCS7 padding: {}", padding);
    }
    if !data[data.len() - padding..]
        .iter()
        .all(|&b| b as usize == padding)
    {
        bail!("invalid PKCS7 padding bytes");
    }
    Ok(&data[..data.len() - padding])
}

fn aes_encrypt_cbc(plaintext: &[u8], key: &[u8], iv: &[u8]) -> Result<Vec<u8>> {
    let padded = pkcs7_pad(plaintext, 16);
    let encryptor = Encryptor::<Aes256>::new_from_slices(key, iv)
        .map_err(|e| anyhow::anyhow!("AES encrypt init: {}", e))?;
    let len = padded.len();
    let mut buf = padded;
    let ct = encryptor
        .encrypt_padded_mut::<cbc::cipher::block_padding::NoPadding>(&mut buf, len)
        .map_err(|e| anyhow::anyhow!("AES encrypt: {}", e))?;
    Ok(ct.to_vec())
}

#[allow(dead_code)]
fn aes_decrypt_cbc(ciphertext: &[u8], key: &[u8], iv: &[u8]) -> Result<Vec<u8>> {
    let decryptor = Decryptor::<Aes256>::new_from_slices(key, iv)
        .map_err(|e| anyhow::anyhow!("AES decrypt init: {}", e))?;
    let mut buf = ciphertext.to_vec();
    let pt = decryptor
        .decrypt_padded_mut::<cbc::cipher::block_padding::NoPadding>(&mut buf)
        .map_err(|e| anyhow::anyhow!("AES decrypt: {}", e))?;
    let unpadded = pkcs7_unpad(pt)?;
    Ok(unpadded.to_vec())
}

pub fn generate_client_iv_key() -> Result<(String, String)> {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis()
        .to_string();

    let iv = aes_encrypt_cbc(now_ms.as_bytes(), &HIK_FIXED_KEY, &HIK_FIXED_IV)?;
    let iv_hex = hex_encode(&iv);
    let key_hex = iv_hex.clone();

    let iv_final = if iv_hex.len() < 64 {
        format!("{}{}", iv_hex, iv_hex)
    } else {
        iv_hex
    };
    let key_final = if key_hex.len() < 64 {
        format!("{}{}", key_hex, key_hex)
    } else {
        key_hex
    };

    Ok((iv_final, key_final))
}

pub fn generate_realplay_key(iv: &str, key: &str, pkd: &str) -> Result<String> {
    let plaintext = format!("{}:{}", iv, key);
    let pkd_bytes = hex_decode(pkd)?;
    let n = BigUint::from_bytes_be(&pkd_bytes);
    let e = BigUint::from(65537u32);
    let key_len = (n.bits() as usize + 7) / 8;

    let msg = plaintext.as_bytes();
    let msg_len = msg.len();

    if msg_len + 11 > key_len {
        bail!("message too long for RSA key size");
    }

    let mut block = vec![0u8; key_len];
    let mut t = key_len;
    for i in (0..msg_len).rev() {
        t -= 1;
        block[t] = msg[i];
    }
    t -= 1;
    block[t] = 0;
    while t > 2 {
        t -= 1;
        let mut r: u8 = rand::random();
        while r == 0 {
            r = rand::random();
        }
        block[t] = r;
    }
    block[0] = 0x00;
    block[1] = 0x02;

    let m = BigUint::from_bytes_be(&block);
    let c = m.modpow(&e, &n);
    let c_bytes = c.to_bytes_be();

    let mut result = vec![0u8; key_len];
    let offset = key_len.saturating_sub(c_bytes.len());
    result[offset..].copy_from_slice(&c_bytes);

    Ok(hex_encode(&result))
}

pub fn generate_authorization(rand: &str, auth: &str, key: &str, iv: &str) -> Result<String> {
    let plaintext = format!("{}:{}", rand, auth);
    let key_bytes = hex_decode(&key[..64.min(key.len())])?;
    let iv_bytes = hex_decode(&iv[..32.min(iv.len())])?;

    let mut key_padded = key_bytes;
    key_padded.resize(32, 0);
    let ciphertext = aes_encrypt_cbc(plaintext.as_bytes(), &key_padded, &iv_bytes)?;
    Ok(hex_encode(&ciphertext))
}

pub fn generate_token(token_plain: &str, key: &str, iv: &str) -> Result<String> {
    let key_bytes = hex_decode(&key[..64.min(key.len())])?;
    let iv_bytes = hex_decode(&iv[..32.min(iv.len())])?;

    let mut key_padded = key_bytes;
    key_padded.resize(32, 0);
    let ciphertext = aes_encrypt_cbc(token_plain.as_bytes(), &key_padded, &iv_bytes)?;
    Ok(hex_encode(&ciphertext))
}

pub fn generate_secret_key(pkd: &str, rand: &str, username: &str, password: &str) -> String {
    let password_md5 = md5::compute(password.as_bytes());
    let mut salt_input = Vec::new();
    salt_input.extend_from_slice(&password_md5.0);
    salt_input.extend_from_slice(b"rtsp");
    salt_input.extend_from_slice(username.as_bytes());
    let salt_data = Sha256::digest(&salt_input);

    let mut auth_input = Vec::new();
    auth_input.extend_from_slice(pkd.as_bytes());
    auth_input.extend_from_slice(rand.as_bytes());
    let auth_data = Sha256::digest(&auth_input);

    let mut combined_input = Vec::new();
    combined_input.extend_from_slice(&salt_data);
    combined_input.extend_from_slice(&auth_data);
    combined_input.extend_from_slice(&salt_data);
    let combined = Sha256::digest(&combined_input);

    hex_encode(&combined)
}

fn hex_encode(data: &[u8]) -> String {
    data.iter().map(|b| format!("{:02x}", b)).collect()
}

fn detect_keyframe(data: &[u8]) -> bool {
    if data.len() < 5 {
        return false;
    }

    let mut offset = 0;
    while offset < data.len().saturating_sub(4) {
        if data[offset] == 0x00 && data[offset + 1] == 0x00 {
            let start_code_len = if offset + 2 < data.len() && data[offset + 2] == 0x01 {
                3
            } else if offset + 3 < data.len() && data[offset + 2] == 0x00 && data[offset + 3] == 0x01 {
                4
            } else {
                offset += 1;
                continue;
            };

            let nal_start = offset + start_code_len;
            if nal_start >= data.len() {
                break;
            }

            let nal_type = data[nal_start] & 0x1F;
            
            if nal_type == 5 || nal_type == 7 || nal_type == 8 {
                return true;
            }

            let hevc_nal_type = (data[nal_start] >> 1) & 0x3F;
            if hevc_nal_type >= 16 && hevc_nal_type <= 21 {
                return true;
            }
            if hevc_nal_type == 32 || hevc_nal_type == 33 || hevc_nal_type == 34 {
                return true;
            }

            offset = nal_start + 1;
        } else {
            offset += 1;
        }
    }

    false
}

fn hex_decode(hex: &str) -> Result<Vec<u8>> {
    if hex.len() % 2 != 0 {
        bail!("odd-length hex string");
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&hex[i..i + 2], 16)
                .with_context(|| format!("invalid hex at position {}", i))
        })
        .collect()
}

fn url_encode(s: &str) -> String {
    let mut result = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(b as char);
            }
            _ => {
                result.push_str(&format!("%{:02X}", b));
            }
        }
    }
    result
}

#[derive(Debug, Clone)]
pub struct VideoFrame {
    pub data: Vec<u8>,
    pub is_keyframe: bool,
    pub timestamp: u64,
}

pub struct EncryptedStreamClient {
    ws: WebSocket<MaybeTlsStream<std::net::TcpStream>>,
    pkd: String,
    rand: String,
    cipher_suite: String,
    #[allow(dead_code)]
    username: String,
    password: String,
    #[allow(dead_code)]
    verification_code: String,
    device_host: String,
    device_port: u16,
}

impl EncryptedStreamClient {
    pub fn connect(
        host: &str,
        ws_port: u16,
        device_port: u16,
        _channel_id: &str,
        username: &str,
        password: &str,
        verification_code: &str,
    ) -> Result<Self> {
        let url = format!(
            "ws://{}:{}/media?version=0.1&cipherSuites=0&sessionID=&proxy={}:{}",
            host, ws_port, host, device_port
        );
        log::info!("Connecting to WebSocket: {}", url);

        let (ws, response) = match tungstenite::connect(&url) {
            Ok(r) => r,
            Err(e) => {
                let err_msg = format!("WebSocket connection to {}:{} failed: {}", host, ws_port, e);
                log::error!("{}", err_msg);
                log::error!("Possible causes:");
                log::error!("  1. DVR does not have 'Transmission Encryption' enabled");
                log::error!("  2. Port {} is blocked by firewall", ws_port);
                log::error!("  3. DVR is unreachable at {}:{}", host, ws_port);
                log::error!("  4. WebSocket/encryption feature not supported on this firmware");
                log::error!("Solution: Either enable encryption on DVR, or use normal RTSP without this proxy");
                bail!("{}", err_msg);
            }
        };

        log::info!("WebSocket connected, status: {:?}", response.status());

        let mut client = Self {
            ws,
            pkd: String::new(),
            rand: String::new(),
            cipher_suite: "0".to_string(),
            username: username.to_string(),
            password: password.to_string(),
            verification_code: verification_code.to_string(),
            device_host: host.to_string(),
            device_port,
        };

        client.receive_key_exchange()?;
        Ok(client)
    }

    fn receive_key_exchange(&mut self) -> Result<()> {
        let msg = self.ws.read().context("failed to read key exchange")?;

        match msg {
            Message::Text(text) => {
                let resp: ServerKeyExchange =
                    serde_json::from_str(&text).context("failed to parse key exchange JSON")?;

                if let Some(code) = resp.error_code {
                    if code != 0 {
                        bail!(
                            "server error {}: {}",
                            code,
                            resp.error_msg.unwrap_or_default()
                        );
                    }
                }

                self.pkd = resp.pkd.unwrap_or_default();
                self.rand = resp.rand.unwrap_or_default();
                if let Some(cs) = resp.cipher_suite {
                    self.cipher_suite = cs.to_string().trim_matches('"').to_string();
                }

                log::info!(
                    "Key exchange received: PKD={}..., rand={}, cipherSuite={}",
                    &self.pkd[..self.pkd.len().min(32)],
                    self.rand,
                    self.cipher_suite
                );
                Ok(())
            }
            _ => bail!("expected TEXT message for key exchange, got {:?}", msg),
        }
    }

    pub fn start_stream(&mut self, _channel_id: &str) -> Result<()> {
        let encoded_password = url_encode(&self.password);
        let device_url = format!(
            "ws://{}:{}/openUrl/{}",
            self.device_host, self.device_port, encoded_password
        );

        log::info!("Device URL: {}", device_url);

        let (iv, key) = generate_client_iv_key()?;
        let _realplay_key = if !self.pkd.is_empty() {
            generate_realplay_key(&iv, &key, &self.pkd)?
        } else {
            String::new()
        };

        let _authorization = if !self.rand.is_empty() {
            generate_authorization(&self.rand, &self.password, &key, &iv)?
        } else {
            String::new()
        };

        let url_hash = {
            let hash = Sha256::digest(device_url.as_bytes());
            hex_encode(&hash)
        };
        let _token = generate_token(&url_hash, &key, &iv)?;

        let realplay = RealplayRequest {
            sequence: 0,
            cmd: "realplay".to_string(),
            url: device_url,
            key: String::new(),
            authorization: String::new(),
            token: String::new(),
        };

        let json = serde_json::to_string(&realplay)?;
        log::info!("Sending realplay request");
        self.ws.send(Message::Text(json.into()))?;

        let response = self.ws.read().context("failed to read realplay response")?;
        match response {
            Message::Text(text) => {
                let resp: RealplayResponse =
                    serde_json::from_str(&text).context("failed to parse realplay response")?;

                if let Some(code) = resp.error_code {
                    if code != 0 {
                        bail!(
                            "realplay failed {}: {}",
                            code,
                            resp.error_msg.unwrap_or_default()
                        );
                    }
                }

                if resp.sdp.is_some() {
                    log::info!("Stream started, SDP received");
                } else {
                    log::info!("Stream started (no SDP)");
                }
                Ok(())
            }
            _ => bail!("expected TEXT response for realplay, got {:?}", response),
        }
    }

    pub fn receive_frame(&mut self) -> Result<Option<VideoFrame>> {
        loop {
            let msg = self.ws.read().context("failed to read WebSocket message")?;

            match msg {
                Message::Binary(data) => {
                    if let Some((msg_type, payload, _)) = unpack_message(&data) {
                        match msg_type {
                            MSG_TYPE_VIDEO_DATA => {
                                return Ok(Some(self.parse_video_frame(payload)?));
                            }
                            MSG_TYPE_AUDIO_DATA => {
                                log::debug!("Audio data: {} bytes", payload.len());
                                continue;
                            }
                            MSG_TYPE_KEEPALIVE => {
                                log::debug!("Keepalive received");
                                continue;
                            }
                            MSG_TYPE_SESSION_ERROR => {
                                let err = String::from_utf8_lossy(payload);
                                bail!("session error: {}", err);
                            }
                            _ => {
                                log::debug!("Unknown message type: 0x{:02x}", msg_type);
                                continue;
                            }
                        }
                    } else {
                        return Ok(Some(self.parse_raw_video_frame(&data)?));
                    }
                }
                Message::Text(text) => {
                    log::debug!("Server TEXT: {}", &text[..text.len().min(200)]);
                    if let Ok(resp) = serde_json::from_str::<RealplayResponse>(&text) {
                        if let Some(code) = resp.error_code {
                            if code != 0 {
                                bail!(
                                    "stream error {}: {}",
                                    code,
                                    resp.error_msg.unwrap_or_default()
                                );
                            }
                        }
                    }
                    continue;
                }
                Message::Ping(data) => {
                    self.ws.send(Message::Pong(data))?;
                    continue;
                }
                Message::Close(_) => {
                    log::info!("Server closed connection");
                    return Ok(None);
                }
                _ => continue,
            }
        }
    }

    fn parse_video_frame(&self, data: &[u8]) -> Result<VideoFrame> {
        if data.len() < 8 {
            bail!("video frame too short: {} bytes", data.len());
        }

        let mut offset = 0;

        if data.len() >= 2 && data[0] == 0x24 && data[1] == 0x34 {
            if data.len() >= 8 {
                let frame_len = u32::from_be_bytes([data[4], data[5], data[6], data[7]]) as usize;
                log::debug!("Proprietary header: magic=0x2434, frame_len={}", frame_len);
                offset = 8;
            }
        }

        let video_data = if offset > 0 && offset < data.len() {
            &data[offset..]
        } else {
            data
        };

        let is_rtp = video_data.len() >= 12 && (video_data[0] & 0xC0) == 0x80;
        
        let (nal_data, is_keyframe) = if is_rtp {
            let rtp_header_len = 12;
            let csrc_count = (video_data[0] & 0x0F) as usize;
            let extension = (video_data[1] & 0x10) != 0;
            let mut rtp_offset = rtp_header_len + csrc_count * 4;
            
            if extension && video_data.len() > rtp_offset + 4 {
                let ext_len = u16::from_be_bytes([video_data[rtp_offset + 2], video_data[rtp_offset + 3]]) as usize;
                rtp_offset += 4 + ext_len * 4;
            }
            
            if rtp_offset < video_data.len() {
                let payload = &video_data[rtp_offset..];
                let keyframe = detect_keyframe(payload);
                (payload.to_vec(), keyframe)
            } else {
                (video_data.to_vec(), false)
            }
        } else {
            let keyframe = detect_keyframe(video_data);
            (video_data.to_vec(), keyframe)
        };

        let timestamp = if data.len() >= 4 {
            u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as u64
        } else {
            0
        };

        Ok(VideoFrame {
            data: nal_data,
            is_keyframe,
            timestamp,
        })
    }

    fn parse_raw_video_frame(&self, data: &[u8]) -> Result<VideoFrame> {
        let is_keyframe = detect_keyframe(data);

        Ok(VideoFrame {
            data: data.to_vec(),
            is_keyframe,
            timestamp: 0,
        })
    }

    pub fn close(&mut self) -> Result<()> {
        self.ws.close(None)?;
        Ok(())
    }
}

pub fn decrypt_video_data(
    encrypted: &[u8],
    secret_key_hex: &str,
) -> Result<Vec<u8>> {
    if encrypted.len() < 16 {
        return Ok(encrypted.to_vec());
    }

    let key = hex_decode(&secret_key_hex[..64.min(secret_key_hex.len())])?;
    let iv = &encrypted[..16];
    let ciphertext = &encrypted[16..];

    if ciphertext.len() % 16 != 0 {
        log::warn!(
            "ciphertext length {} not multiple of 16, returning raw",
            ciphertext.len()
        );
        return Ok(encrypted.to_vec());
    }

    match aes_decrypt_cbc_raw(ciphertext, &key, iv) {
        Ok(decrypted) => Ok(decrypted),
        Err(e) => {
            log::warn!("AES decryption failed: {}, returning raw data", e);
            Ok(encrypted.to_vec())
        }
    }
}

/// Derive an AES-256 key from the Hikvision verification code.
///
/// Uses SHA256(verification_code) as the key derivation function.
/// This is the standard approach used by Hikvision for transmission encryption.
pub fn derive_verification_code_key(verification_code: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(verification_code.as_bytes());
    let result = hasher.finalize();
    let mut key = [0u8; 32];
    key.copy_from_slice(&result);
    key
}

/// Decrypt an RTP payload (bytes after the 12-byte RTP header) using AES-256-CBC.
///
/// Tries multiple IV strategies and returns the result that looks most valid:
/// 1. HIK_FIXED_IV (the standard Hikvision WebSocket IV)
/// 2. First 16 bytes of payload as IV (common in Hikvision proprietary formats)
/// 3. Zero IV (16 bytes of 0x00)
///
/// If all strategies fail or produce invalid output, returns the original payload.
pub fn decrypt_rtp_payload(payload: &[u8], key: &[u8; 32]) -> Vec<u8> {
    if payload.is_empty() {
        return payload.to_vec();
    }

    // Helper: validate that output looks like H.264 NAL data
    let looks_valid = |data: &[u8]| -> bool {
        if data.len() < 4 {
            return false;
        }
        // Check for H.264 NAL start code (0x00 0x00 0x01 or 0x00 0x00 0x00 0x01)
        if data[0] == 0x00 && data[1] == 0x00 {
            if data.len() > 2 && data[2] == 0x01 {
                return true;
            }
            if data.len() > 3 && data[2] == 0x00 && data[3] == 0x01 {
                return true;
            }
        }
        // Check for valid NAL header byte (NAL unit type 1-29)
        let nal_type = data[0] & 0x1F;
        if (1..=29).contains(&nal_type) {
            return true;
        }
        false
    };

    // Pad to 16 bytes for ciphertext that isn't block-aligned
    let pad_to_block = |data: &[u8]| -> Vec<u8> {
        if data.len() % 16 == 0 {
            data.to_vec()
        } else {
            let padded_len = (data.len() + 15) & !15;
            let mut padded = data.to_vec();
            padded.resize(padded_len, 0);
            padded
        }
    };

    // Strategy 1: HIK_FIXED_IV, decrypt entire payload
    let padded = pad_to_block(payload);
    if let Ok(decrypted) = aes_decrypt_cbc_raw(&padded, key, &HIK_FIXED_IV) {
        let trimmed = &decrypted[..payload.len().min(decrypted.len())];
        if looks_valid(trimmed) {
            return trimmed.to_vec();
        }
    }

    // Strategy 2: IV = first 16 bytes of payload, ciphertext = rest
    if payload.len() > 32 {
        let iv = &payload[..16];
        let ciphertext = &payload[16..];
        let padded_ct = pad_to_block(ciphertext);
        if let Ok(decrypted) = aes_decrypt_cbc_raw(&padded_ct, key, iv) {
            let trimmed = &decrypted[..ciphertext.len().min(decrypted.len())];
            let mut result = Vec::with_capacity(16 + trimmed.len());
            result.extend_from_slice(iv);
            result.extend_from_slice(trimmed);
            if looks_valid(&result) {
                return result;
            }
        }
    }

    // Strategy 3: Zero IV
    let zero_iv = [0u8; 16];
    if let Ok(decrypted) = aes_decrypt_cbc_raw(&padded, key, &zero_iv) {
        let trimmed = &decrypted[..payload.len().min(decrypted.len())];
        if looks_valid(trimmed) {
            return trimmed.to_vec();
        }
    }

    // All strategies failed - NAL header may be unencrypted, try decrypting from byte 1
    if payload.len() > 1 {
        let body = &payload[1..];
        let padded_body = pad_to_block(body);
        // Try HIK_FIXED_IV on body
        if let Ok(decrypted) = aes_decrypt_cbc_raw(&padded_body, key, &HIK_FIXED_IV) {
            let trimmed = &decrypted[..body.len().min(decrypted.len())];
            let mut result = Vec::with_capacity(1 + trimmed.len());
            result.push(payload[0]); // Keep NAL header
            result.extend_from_slice(trimmed);
            if looks_valid(&result) {
                return result;
            }
        }
        // Try zero IV on body
        if let Ok(decrypted) = aes_decrypt_cbc_raw(&padded_body, key, &zero_iv) {
            let trimmed = &decrypted[..body.len().min(decrypted.len())];
            let mut result = Vec::with_capacity(1 + trimmed.len());
            result.push(payload[0]);
            result.extend_from_slice(trimmed);
            return result;
        }
    }

    // Fallback: return original payload
    log::warn!("All decryption strategies failed, returning original payload");
    payload.to_vec()
}

fn aes_decrypt_cbc_raw(ciphertext: &[u8], key: &[u8], iv: &[u8]) -> Result<Vec<u8>> {
    let decryptor = Decryptor::<Aes256>::new_from_slices(key, iv)
        .map_err(|e| anyhow::anyhow!("AES decrypt init: {}", e))?;
    let mut buf = ciphertext.to_vec();
    let pt = decryptor
        .decrypt_padded_mut::<cbc::cipher::block_padding::NoPadding>(&mut buf)
        .map_err(|e| anyhow::anyhow!("AES decrypt: {}", e))?;
    Ok(pt.to_vec())
}
