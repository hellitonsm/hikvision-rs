//! Multi-câmera HCNetSDK com renderização X11 direta.
//!
//! Diferente do `hcnetsdk_multi_stream` (que usa callback + PlayM4 + JPEG),
//! este módulo segue a abordagem do ARCHITECTURE.md original:
//!
//! - NET_DVR_RealPlay_V40 com hPlayWnd=X11 window, bBlocked=1, callback=NULL
//! - O SDK renderiza vídeo diretamente via overlay X11
//! - Sem decodificação manual, sem PlayM4, sem cópia de frames
//! - Cada câmera = uma janela X11 filha da janela principal
//!
//! Esta é a abordagem mais simples e compatível com o SDK Hikvision,
//! usada por softwares como iVMS.

use crate::hcnetsdk::{self, HCNetSDK, NET_DVR_DEVICEINFO_V40};
use anyhow::{Context, Result};
use std::sync::Arc;

/// Estado de uma câmera individual no modo X11 direto.
pub struct X11ChannelState {
    pub sdk_channel: i32,
    pub realplay_handle: i32,
    pub x11_window_id: u32,
}

/// Multi-câmera HCNetSDK via janelas X11 diretas.
///
/// Cada câmera tem:
/// - Um `NET_DVR_RealHandle` (stream ativo)
/// - Uma janela X11 (criada pelo X11WindowManager)
/// - O SDK renderiza direto na janela via overlay
pub struct HCNetSDKX11Multi {
    sdk: Arc<HCNetSDK>,
    user_id: i32,
    device_info: NET_DVR_DEVICEINFO_V40,
    channels: Vec<X11ChannelState>,
}

impl HCNetSDKX11Multi {
    pub fn new(
        host: &str,
        sdk_port: u16,
        username: &str,
        password: &str,
        verification_code: Option<&str>,
    ) -> Result<Self> {
        let sdk = hcnetsdk::global_sdk();
        let (user_id, device_info) = sdk
            .login(host, sdk_port, username, password)
            .context("NET_DVR_Login_V40 failed")?;

        if let Some(key) = verification_code {
            if !key.trim().is_empty() {
                sdk.set_sdk_secret_key(user_id, key)
                    .context("NET_DVR_SetSDKSecretKey failed")?;
            }
        }

        log::info!(
            "HCNetSDKX11Multi logged in. channels={}, zero={}",
            device_info.struDeviceV30.byChanNum,
            device_info.struDeviceV30.byZeroChanNum,
        );

        Ok(Self {
            sdk: sdk.clone(),
            user_id,
            device_info,
            channels: Vec::new(),
        })
    }

    /// Inicia o preview de um canal usando janela X11 direta.
    ///
    /// `x11_window_id` deve ser uma janela X11 já criada e mapeada,
    /// filha da janela principal da aplicação.
    pub fn start_channel(
        &mut self,
        sdk_channel: i32,
        main_stream: bool,
        x11_window_id: u32,
    ) -> Result<()> {
        // Verifica se já está rodando
        if self.channels.iter().any(|c| c.sdk_channel == sdk_channel) {
            log::warn!("Channel {} already started, stopping first", sdk_channel);
            self.stop_channel(sdk_channel);
        }

        let stream_type = if main_stream {
            hcnetsdk::STREAM_MAIN
        } else {
            hcnetsdk::STREAM_SUB
        };

        log::info!(
            "Starting HCNetSDK X11 direct: channel={}, stream={}, x11_win=0x{:x}",
            sdk_channel, stream_type, x11_window_id
        );

        let handle = self
            .sdk
            .realplay_with_window(self.user_id, sdk_channel, stream_type, x11_window_id)?;

        self.channels.push(X11ChannelState {
            sdk_channel,
            realplay_handle: handle,
            x11_window_id,
        });

        log::info!(
            "Channel {} started (handle={}, active={})",
            sdk_channel,
            handle,
            self.channels.len()
        );

        Ok(())
    }

    /// Para o preview de um canal específico.
    pub fn stop_channel(&mut self, sdk_channel: i32) -> bool {
        let pos = self
            .channels
            .iter()
            .position(|c| c.sdk_channel == sdk_channel);
        let Some(idx) = pos else {
            return false;
        };
        let ch = &self.channels[idx];
        let _ = self.sdk.stop_realplay(ch.realplay_handle);
        self.channels.remove(idx);
        log::info!("Channel {} stopped", sdk_channel);
        true
    }

    /// Para todos os previews ativos.
    pub fn stop_all(&mut self) {
        let count = self.channels.len();
        for ch in self.channels.drain(..) {
            let _ = self.sdk.stop_realplay(ch.realplay_handle);
        }
        log::info!("All {} X11 channels stopped", count);
    }

    pub fn device_info(&self) -> &NET_DVR_DEVICEINFO_V40 {
        &self.device_info
    }

    pub fn channel_count(&self) -> u8 {
        self.device_info.struDeviceV30.byChanNum
    }

    pub fn zero_channel_count(&self) -> u8 {
        self.device_info.struDeviceV30.byZeroChanNum
    }

    pub fn supports_zero_channel(&self) -> bool {
        self.device_info.struDeviceV30.byZeroChanNum > 0
    }

    pub fn active_count(&self) -> usize {
        self.channels.len()
    }

    pub fn is_channel_active(&self, sdk_channel: i32) -> bool {
        self.channels.iter().any(|c| c.sdk_channel == sdk_channel)
    }
}

impl Drop for HCNetSDKX11Multi {
    fn drop(&mut self) {
        self.stop_all();
        if let Err(e) = self.sdk.logout(self.user_id) {
            log::warn!("Logout failed: {}", e);
        }
    }
}
