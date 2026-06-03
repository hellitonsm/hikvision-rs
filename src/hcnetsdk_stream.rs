//! Streaming via HCNetSDK oficial com renderização X11 direta.
//!
//! Segue exatamente a abordagem documentada no ARCHITECTURE.md do rustdemo:
//! NET_DVR_RealPlay_V40 com hPlayWnd=janela X11, bBlocked=1, callback=NULL.
//! O SDK renderiza vídeo diretamente na janela X11 via overlay.

use crate::hcnetsdk::{self, HCNetSDK, NET_DVR_DEVICEINFO_V40};
use crate::x11_window::PreviewWindow;
use anyhow::{Context, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Wrapper do stream HCNetSDK com janela X11 para renderização direta.
pub struct HCNetSDKStream {
    sdk: Arc<HCNetSDK>,
    user_id: i32,
    realplay_handle: Option<i32>,
    device_info: NET_DVR_DEVICEINFO_V40,
    preview_wnd: Option<PreviewWindow>,
}

impl HCNetSDKStream {
    pub fn new(
        host: &str,
        sdk_port: u16,
        username: &str,
        password: &str,
        verification_code: Option<&str>,
    ) -> Result<Self> {
        log::info!("HCNetSDK stream: {}:{} user={}", host, sdk_port, username);

        let sdk = hcnetsdk::global_sdk();
        let (user_id, device_info) = sdk.login(host, sdk_port, username, password)
            .context("NET_DVR_Login_V40 failed")?;

        log::info!("Logged in, user_id={}, channels={}, zero_chan={}",
            user_id, device_info.struDeviceV30.byChanNum, device_info.struDeviceV30.byZeroChanNum);

        if let Some(key) = verification_code {
            if !key.trim().is_empty() {
                log::info!("Setting SDK secret key");
                sdk.set_sdk_secret_key(user_id, key)
                    .context("NET_DVR_SetSDKSecretKey failed")?;
            }
        }

        Ok(Self {
            sdk: sdk.clone(),
            user_id,
            realplay_handle: None,
            device_info,
            preview_wnd: None,
        })
    }

    /// Inicia o preview usando janela X11 direta (conforme ARCHITECTURE.md).
    ///
    /// Passos:
    /// 1. Cria janela X11 via PreviewWindow::new()
    /// 2. Obtém window ID (hwnd)
    /// 3. Mapeia a janela (map_window + flush) — CRÍTICO: janela visível antes do RealPlay
    /// 4. Chama NET_DVR_RealPlay_V40 com hPlayWnd=hwnd, bBlocked=1, callback=NULL
    pub fn start(&mut self, channel: i32, main_stream: bool) -> Result<()> {
        if self.realplay_handle.is_some() {
            anyhow::bail!("Stream already started");
        }

        let stream_type = if main_stream { hcnetsdk::STREAM_MAIN } else { hcnetsdk::STREAM_SUB };
        log::info!("Starting HCNetSDK preview (X11 window): channel={}, stream_type={}", channel, stream_type);

        // 1. Cria janela X11
        let wnd = PreviewWindow::new()
            .ok_or_else(|| anyhow::anyhow!("Failed to create X11 preview window"))?;
        let hwnd = wnd.window_id();
        log::info!("Preview window created: 0x{:x}", hwnd);

        // 2. Mapeia a janela — CRÍTICO: deve estar visível antes do RealPlay
        wnd.show().map_err(|e| anyhow::anyhow!("Failed to show preview window: {}", e))?;

        // 3. Pequeno delay para o servidor X11 processar o map
        std::thread::sleep(std::time::Duration::from_millis(100));

        // 4. Inicia o preview com a janela X11 — callback NULL, bBlocked=1
        let handle = self.sdk.realplay_with_window(self.user_id, channel, stream_type, hwnd)?;

        self.realplay_handle = Some(handle);
        self.preview_wnd = Some(wnd);

        log::info!("HCNetSDK preview started, handle={}, hwnd=0x{:x}", handle, hwnd);
        Ok(())
    }

    /// Processa eventos X11 da janela de preview.
    /// Retorna false se a janela foi fechada pelo usuário.
    pub fn poll_window_events(&mut self) -> bool {
        if let Some(ref mut wnd) = self.preview_wnd {
            if !wnd.poll_events() {
                log::info!("Preview window closed by user");
                let _ = self.stop();
                return false;
            }
        }
        true
    }

    pub fn stop(&mut self) -> Result<()> {
        if let Some(handle) = self.realplay_handle.take() {
            log::info!("Stopping preview, handle={}", handle);
            self.sdk.stop_realplay(handle)?;
        }
        self.preview_wnd = None;
        Ok(())
    }

    pub fn device_info(&self) -> &NET_DVR_DEVICEINFO_V40 { &self.device_info }
    pub fn channel_count(&self) -> u8 { self.device_info.struDeviceV30.byChanNum }
    pub fn supports_channel_zero(&self) -> bool { self.device_info.struDeviceV30.byZeroChanNum > 0 }
}

impl Drop for HCNetSDKStream {
    fn drop(&mut self) {
        let _ = self.stop();
        if let Err(e) = self.sdk.logout(self.user_id) {
            log::warn!("Logout failed: {}", e);
        }
    }
}

/// Loop de stream para um canal específico.
pub fn hcnetsdk_stream_loop(
    host: &str,
    port: u16,
    username: &str,
    password: &str,
    channel_id: &str,
    sdk_channel: i32,
    verification_code: &str,
    _library_path: Option<&str>,
    stop: Arc<AtomicBool>,
) {
    log::info!("HCNetSDK X11 stream loop starting for channel {} (sdk_channel={})", channel_id, sdk_channel);

    let mut stream = match HCNetSDKStream::new(
        host, port, username, password,
        if verification_code.is_empty() { None } else { Some(verification_code) },
    ) {
        Ok(s) => s,
        Err(e) => {
            log::error!("Failed to create HCNetSDK stream: {}", e);
            return;
        }
    };

    if let Err(e) = stream.start(sdk_channel, true) {
        log::error!("Failed to start preview: {}", e);
        return;
    }

    log::info!("HCNetSDK X11 preview active, processing events...");

    while !stop.load(Ordering::Relaxed) {
        if !stream.poll_window_events() {
            log::info!("Preview window closed, stopping stream");
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    log::info!("HCNetSDK X11 stream loop ending");
}
