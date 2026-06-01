//! Stream usando HCNetSDK com descriptografia automática
//!
//! Este módulo implementa streaming via HCNetSDK (libhcnetsdk.so) que
//! descriptografia automaticamente quando a chave é configurada via
//! NET_DVR_SetSDKSecretKey.

use crate::hcnetsdk::{self, HCNetSDK, NET_DVR_DEVICEINFO_V40};
use anyhow::{Context, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::collections::VecDeque;
use std::time::Instant;

const MAX_FRAME_QUEUE: usize = 30;

/// Frame de vídeo recebido do SDK
#[derive(Clone)]
pub struct VideoFrame {
    pub data: Vec<u8>,
    pub timestamp: Instant,
}

/// Estado compartilhado entre callback e thread principal
struct StreamState {
    frames: VecDeque<VideoFrame>,
    error: Option<String>,
}

/// Stream HCNetSDK
pub struct HCNetSDKStream {
    sdk: Arc<HCNetSDK>,
    user_id: i32,
    realplay_handle: Option<i32>,
    state: Arc<Mutex<StreamState>>,
    device_info: NET_DVR_DEVICEINFO_V40,
}

impl HCNetSDKStream {
    /// Cria novo stream HCNetSDK
    pub fn new(
        host: &str,
        http_port: u16,
        username: &str,
        password: &str,
        verification_code: Option<&str>,
    ) -> Result<Self> {
        log::info!("HCNetSDK stream: {}:{} user={}", host, http_port, username);

        // Carrega SDK
        let sdk = hcnetsdk::search_and_load()
            .context("Failed to load HCNetSDK. Install Hikvision SDK or copy libhcnetsdk.so to hikvision-libs/")?;

        // Inicializa SDK
        sdk.init().context("NET_DVR_Init failed")?;

        // Login
        let (user_id, device_info) = sdk.login(host, http_port, username, password)
            .context("NET_DVR_Login_V40 failed")?;

        log::info!("Logged in, user_id={}, channels={}, zero_chan={}",
            user_id, device_info.struDeviceV30.byChanNum, device_info.struDeviceV30.byZeroChanNum);

        // Configura chave de descriptografia se fornecida
        if let Some(key) = verification_code {
            if !key.trim().is_empty() {
                log::info!("Setting SDK secret key: '{}'", key);
                sdk.set_sdk_secret_key(user_id, key)
                    .context("NET_DVR_SetSDKSecretKey failed")?;
            }
        }

        let state = Arc::new(Mutex::new(StreamState {
            frames: VecDeque::new(),
            error: None,
        }));

        Ok(Self {
            sdk: Arc::new(sdk),
            user_id,
            realplay_handle: None,
            state,
            device_info,
        })
    }

    /// Inicia preview de um canal
    pub fn start(&mut self, channel: i32, main_stream: bool) -> Result<()> {
        if self.realplay_handle.is_some() {
            anyhow::bail!("Stream already started");
        }

        let stream_type = if main_stream {
            hcnetsdk::STREAM_MAIN
        } else {
            hcnetsdk::STREAM_SUB
        };

        log::info!("Starting preview: channel={}, stream_type={}", channel, stream_type);

        // Cria callback
        let state = Arc::clone(&self.state);

        extern "C" fn data_callback(
            _handle: i32,
            data_type: u32,
            buffer: *mut u8,
            buf_size: u32,
            user_data: *mut std::ffi::c_void,
        ) {
            if buffer.is_null() || buf_size == 0 {
                return;
            }

            let state = unsafe { &*(user_data as *const Arc<Mutex<StreamState>>) };

            // NET_DVR_SYSHEAD (1) = header, NET_DVR_STREAMDATA (2) = stream data
            if data_type == hcnetsdk::NET_DVR_STREAMDATA {
                let data = unsafe { std::slice::from_raw_parts(buffer, buf_size as usize) };

                let mut state = state.lock().unwrap();

                // Limita fila
                if state.frames.len() >= MAX_FRAME_QUEUE {
                    state.frames.pop_front();
                }

                state.frames.push_back(VideoFrame {
                    data: data.to_vec(),
                    timestamp: Instant::now(),
                });
            }
        }

        // Passa ponteiro para state como user_data
        let user_data = Arc::into_raw(Arc::clone(&self.state)) as *mut std::ffi::c_void;

        let handle = self.sdk.realplay(
            self.user_id,
            channel,
            stream_type,
            data_callback,
            user_data,
        )?;

        self.realplay_handle = Some(handle);
        log::info!("Preview started, handle={}", handle);

        Ok(())
    }

    /// Para o preview
    pub fn stop(&mut self) -> Result<()> {
        if let Some(handle) = self.realplay_handle.take() {
            log::info!("Stopping preview, handle={}", handle);
            self.sdk.stop_realplay(handle)?;
        }
        Ok(())
    }

    /// Obtém próximo frame (não-bloqueante)
    pub fn next_frame(&self) -> Option<VideoFrame> {
        let mut state = self.state.lock().unwrap();
        state.frames.pop_front()
    }

    /// Verifica se há erro
    pub fn check_error(&self) -> Option<String> {
        let state = self.state.lock().unwrap();
        state.error.clone()
    }

    /// Retorna informações do dispositivo
    pub fn device_info(&self) -> &NET_DVR_DEVICEINFO_V40 {
        &self.device_info
    }

    /// Número de canais disponíveis
    pub fn channel_count(&self) -> u8 {
        self.device_info.struDeviceV30.byChanNum
    }

    /// Suporta Canal Zero?
    pub fn supports_channel_zero(&self) -> bool {
        self.device_info.struDeviceV30.byZeroChanNum > 0
    }
}

impl Drop for HCNetSDKStream {
    fn drop(&mut self) {
        // Para preview se ainda estiver rodando
        let _ = self.stop();

        // Logout
        if let Err(e) = self.sdk.logout(self.user_id) {
            log::warn!("Logout failed: {}", e);
        }
    }
}

/// Loop de streaming para integração com UI egui
///
/// NOTA: Este é um placeholder. O HCNetSDK requer implementação completa
/// de decoder H264/H265 para converter frames brutos em RGBA.
/// Por enquanto, apenas loga os frames recebidos para debug.
pub fn hcnetsdk_stream_loop(
    host: &str,
    port: u16,
    username: &str,
    password: &str,
    channel_id: &str,
    verification_code: &str,
    _library_path: Option<&str>,
    _tx: mpsc::SyncSender<crate::rtsp::RtspFrame>,
    stop: Arc<AtomicBool>,
    ctx: egui::Context,
) {
    log::info!("HCNetSDK stream loop starting for channel {}", channel_id);

    let channel: i32 = channel_id.parse().unwrap_or(1);

    // Cria stream
    let mut stream = match HCNetSDKStream::new(host, port, username, password, Some(verification_code)) {
        Ok(s) => s,
        Err(e) => {
            log::error!("Failed to create HCNetSDK stream: {}", e);
            return;
        }
    };

    // Inicia preview
    if let Err(e) = stream.start(channel, true) {
        log::error!("Failed to start preview: {}", e);
        return;
    }

    log::info!("HCNetSDK preview started, waiting for frames...");

    // Loop de frames - recebe H264/H265 raw
    // TODO: Implementar decoder H264/H265 para RGBA
    let mut frame_count = 0u64;
    while !stop.load(Ordering::Relaxed) {
        if let Some(frame) = stream.next_frame() {
            frame_count += 1;
            if frame_count % 30 == 0 {
                log::info!("HCNetSDK received {} frames, last: {} bytes", frame_count, frame.data.len());
            }
        }

        // Verifica erro
        if let Some(err) = stream.check_error() {
            log::error!("Stream error: {}", err);
            break;
        }

        // Pequena pausa para não consumir 100% CPU
        std::thread::sleep(std::time::Duration::from_millis(10));
        ctx.request_repaint();
    }

    log::info!("HCNetSDK stream loop ending, total frames: {}", frame_count);
}
