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
///
/// Suporta também Canal Zero via `NET_DVR_RealPlaySpecial` com URL RTSP.
pub struct HCNetSDKX11Multi {
    sdk: Arc<HCNetSDK>,
    user_id: i32,
    device_info: NET_DVR_DEVICEINFO_V40,
    channels: Vec<X11ChannelState>,
    zero_channel: Option<X11ChannelState>,
    host: String,
    user: String,
    password: String,
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
            zero_channel: None,
            host: host.to_string(),
            user: username.to_string(),
            password: password.to_string(),
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

    /// Inicia o Canal Zero com fallback em cascata.
    ///
    /// Estratégia (mesma do `zero_channel_hcnetsdk.rs`):
    /// 1. Ativa Canal Zero via `ensure_zero_channel_enabled`
    /// 2. [a] RealPlaySpecial com múltiplas URLs RTSP (LINK_RTSP e LINK_TCP)
    /// 3. [b] Canais confirmados por ZeroMakeKeyFrame com try_variations_for
    /// 4. [c] V30/V40 RTSP em ch=1 e ch=0 (try_zero_rtsp_variations)
    /// 5. [d] RealPlaySpecial com callback (fallback sem janela)
    /// 6. [e] Candidatos gerais (zero_channel_candidates)
    ///
    /// Usa a mesma sessão SDK (`user_id`) do login regular.
    /// A senha NÃO é URL-encoded (o cliente RTSP do SDK não decodifica %XX).
    pub fn start_zero_channel(&mut self, x11_window_id: u32) -> Result<()> {
        if self.zero_channel.is_some() {
            let _ = self.stop_zero_channel();
        }

        let v30 = &self.device_info.struDeviceV30;
        log::info!("Starting Canal Zero (byZeroChanNum={})", v30.byZeroChanNum);

        if v30.byZeroChanNum == 0 {
            anyhow::bail!("Device does not support Canal Zero (byZeroChanNum=0)");
        }

        // --- 1. Ensure Canal Zero está ativo ---
        match self.sdk.ensure_zero_channel_enabled(self.user_id, true) {
            Ok((enabled, was_us)) => {
                log::info!("Canal Zero enabled={}, activated_by_us={}", enabled, was_us);
            }
            Err(e) => log::warn!("ensure_zero_channel_enabled: {}", e),
        }

        // --- 2. Probar ZeroMakeKeyFrame ---
        let mut zero_key_ok: Vec<i32> = Vec::new();
        for ch in [1, 33, 34, 35, 51, 65, 129, 257] {
            match self.sdk.zero_make_key_frame(self.user_id, ch) {
                Ok(true) => {
                    log::info!("ZeroMakeKeyFrame({}) = success", ch);
                    zero_key_ok.push(ch);
                }
                _ => {}
            }
        }
        if !zero_key_ok.is_empty() {
            log::info!("ZeroMakeKeyFrame confirmed channels: {:?}", zero_key_ok);
        }

        // --- 3. Fallback cascade ---
        let mut handle: i32 = -1;

        // [a] RealPlaySpecial com todas as URLs RTSP
        if handle < 0 {
            match self.try_zero_special_rtsp(x11_window_id) {
                Ok(h) => { handle = h; log::info!("  [a] SUCESSO: RealPlaySpecial handle={}", h); }
                Err(_) => {}
            }
        }

        // [b] Canais confirmados por ZeroMakeKeyFrame
        if handle < 0 && !zero_key_ok.is_empty() {
            for ch in &zero_key_ok {
                match self.try_variations_for(*ch, x11_window_id) {
                    Ok(h) => { handle = h; log::info!("  [b] SUCESSO: ZeroMakeKeyFrame ch={} handle={}", ch, h); break; }
                    Err(_) => {}
                }
            }
        }

        // [c] V30/V40 RTSP em ch=1 e ch=0
        if handle < 0 {
            for ch in [1, 0] {
                match self.try_zero_rtsp_variations(ch, x11_window_id) {
                    Ok(h) => { handle = h; log::info!("  [c] SUCESSO: V30/V40 RTSP ch={} handle={}", ch, h); break; }
                    Err(_) => {}
                }
            }
        }

        // [d] RealPlaySpecial com callback (sem janela)
        if handle < 0 {
            match self.try_zero_special_rtsp_callback() {
                Ok(h) => { handle = h; log::info!("  [d] SUCESSO: RealPlaySpecial callback handle={}", h); }
                Err(_) => {}
            }
        }

        // [e] Candidatos gerais
        if handle < 0 {
            let candidates = Self::zero_channel_candidates(v30);
            for ch in &candidates {
                match self.try_variations_for(*ch, x11_window_id) {
                    Ok(h) => { handle = h; log::info!("  [e] SUCESSO: candidato geral ch={} handle={}", ch, h); break; }
                    Err(_) => {}
                }
            }
        }

        if handle < 0 {
            anyhow::bail!("All zero channel attempts failed");
        }

        self.zero_channel = Some(X11ChannelState {
            sdk_channel: 0,
            realplay_handle: handle,
            x11_window_id,
        });

        log::info!("Canal Zero started (handle={})", handle);
        Ok(())
    }

    /// [a] RealPlaySpecial com URLs RTSP dedicadas ao Canal Zero.
    fn try_zero_special_rtsp(&self, hwnd: u32) -> Result<i32> {
        let urls = self.generate_zero_channel_rtsp_urls();
        let total = urls.len();
        log::info!("  trying {} URLs via RealPlaySpecial...", total);

        for (mode, link) in [("RTSP", crate::hcnetsdk::LINK_RTSP), ("TCP", crate::hcnetsdk::LINK_TCP)] {
            for url in &urls {
                match self.sdk.realplay_special(self.user_id, url, link, hwnd) {
                    Ok(h) => {
                        log::info!("  >>> RealPlaySpecial {}: handle={}", mode, h);
                        return Ok(h);
                    }
                    Err(e) => log::debug!("  RealPlaySpecial {} failed: {}", mode, e),
                }
            }
        }

        anyhow::bail!("RealPlaySpecial failed on all {} URLs", total)
    }

    /// [d] RealPlaySpecial com callback (sem janela).
    fn try_zero_special_rtsp_callback(&self) -> Result<i32> {
        let urls = self.generate_zero_channel_rtsp_urls();
        log::info!("  trying {} URLs via RealPlaySpecial callback...", urls.len());

        extern "C" fn dummy_cb(_h: i32, _t: u32, _b: *mut u8, _s: u32, _u: *mut std::ffi::c_void) {}

        for url in &urls {
            match self.sdk.realplay_special_with_callback(self.user_id, url, crate::hcnetsdk::LINK_RTSP, dummy_cb, std::ptr::null_mut()) {
                Ok(h) => {
                    log::info!("  >>> RealPlaySpecial callback handle={}", h);
                    return Ok(h);
                }
                Err(e) => log::debug!("  RealPlaySpecial callback failed: {}", e),
            }
        }

        anyhow::bail!("RealPlaySpecial callback failed")
    }

    /// [c] V30/V40 RTSP variations.
    fn try_zero_rtsp_variations(&self, ch: i32, hwnd: u32) -> Result<i32> {
        if let Ok(h) = self.sdk.realplay_v30_with_window_rtsp(self.user_id, ch, false, hwnd) {
            log::info!("    c: V30 RTSP main ch={} handle={}", ch, h);
            return Ok(h);
        }
        if let Ok(h) = self.sdk.realplay_v30_with_window_rtsp(self.user_id, ch, true, hwnd) {
            log::info!("    c: V30 RTSP sub ch={} handle={}", ch, h);
            return Ok(h);
        }

        if let Ok(h) = self.sdk.realplay_with_window_ex2(self.user_id, ch, crate::hcnetsdk::STREAM_MAIN, crate::hcnetsdk::LINK_RTSP, 0, hwnd) {
            log::info!("    c: V40 RTSP MAIN pm=0 ch={} handle={}", ch, h);
            return Ok(h);
        }
        if let Ok(h) = self.sdk.realplay_with_window_ex2(self.user_id, ch, crate::hcnetsdk::STREAM_SUB, crate::hcnetsdk::LINK_RTSP, 0, hwnd) {
            log::info!("    c: V40 RTSP SUB pm=0 ch={} handle={}", ch, h);
            return Ok(h);
        }

        if let Ok(h) = self.sdk.realplay_with_window_ex2(self.user_id, ch, crate::hcnetsdk::STREAM_MAIN, crate::hcnetsdk::LINK_TCP, 1, hwnd) {
            log::info!("    c: V40 TCP MAIN pm=1 ch={} handle={}", ch, h);
            return Ok(h);
        }
        if let Ok(h) = self.sdk.realplay_with_window_ex2(self.user_id, ch, crate::hcnetsdk::STREAM_MAIN, crate::hcnetsdk::LINK_TCP, 2, hwnd) {
            log::info!("    c: V40 TCP MAIN pm=2 ch={} handle={}", ch, h);
            return Ok(h);
        }

        anyhow::bail!("RTSP variations failed for ch={}", ch)
    }

    /// [b/e] V40/V30 variations for a specific channel number.
    fn try_variations_for(&self, ch: i32, hwnd: u32) -> Result<i32> {
        macro_rules! try_one {
            ($desc:expr, $expr:expr) => {
                match $expr {
                    Ok(h) => {
                        log::info!("  >>> {} handle={}", $desc, h);
                        return Ok(h);
                    }
                    Err(_) => {}
                }
            };
        }

        try_one!("V40 TCP MAIN pm=0", self.sdk.realplay_with_window_ex2(self.user_id, ch, crate::hcnetsdk::STREAM_MAIN, crate::hcnetsdk::LINK_TCP, 0, hwnd));
        try_one!("V40 TCP MAIN pm=1", self.sdk.realplay_with_window_ex2(self.user_id, ch, crate::hcnetsdk::STREAM_MAIN, crate::hcnetsdk::LINK_TCP, 1, hwnd));
        try_one!("V40 TCP MAIN pm=2", self.sdk.realplay_with_window_ex2(self.user_id, ch, crate::hcnetsdk::STREAM_MAIN, crate::hcnetsdk::LINK_TCP, 2, hwnd));
        try_one!("V40 TCP MAIN dt=1 pm=0", self.sdk.realplay_with_window_ex3(self.user_id, ch, crate::hcnetsdk::STREAM_MAIN, crate::hcnetsdk::LINK_TCP, 0, 1, 0, hwnd));
        try_one!("V40 RTSP MAIN dt=0 pt=1 pm=0", self.sdk.realplay_with_window_ex3(self.user_id, ch, crate::hcnetsdk::STREAM_MAIN, crate::hcnetsdk::LINK_RTSP, 0, 0, 1, hwnd));
        try_one!("V40 RTSP MAIN dt=1 pt=1 pm=0", self.sdk.realplay_with_window_ex3(self.user_id, ch, crate::hcnetsdk::STREAM_MAIN, crate::hcnetsdk::LINK_RTSP, 0, 1, 1, hwnd));
        try_one!("V40 RTSP SUB pm=0", self.sdk.realplay_with_window_ex2(self.user_id, ch, crate::hcnetsdk::STREAM_SUB, crate::hcnetsdk::LINK_RTSP, 0, hwnd));
        try_one!("V40 TCP SUB pm=0", self.sdk.realplay_with_window_ex2(self.user_id, ch, crate::hcnetsdk::STREAM_SUB, crate::hcnetsdk::LINK_TCP, 0, hwnd));
        try_one!("V40 TCP SUB dt=1 pm=0", self.sdk.realplay_with_window_ex3(self.user_id, ch, crate::hcnetsdk::STREAM_SUB, crate::hcnetsdk::LINK_TCP, 0, 1, 0, hwnd));
        try_one!("V40 RTSP SUB dt=1 pt=1 pm=0", self.sdk.realplay_with_window_ex3(self.user_id, ch, crate::hcnetsdk::STREAM_SUB, crate::hcnetsdk::LINK_RTSP, 0, 1, 1, hwnd));
        try_one!("V30 main", self.sdk.realplay_v30_with_window(self.user_id, ch, false, hwnd));
        try_one!("V30 sub", self.sdk.realplay_v30_with_window(self.user_id, ch, true, hwnd));
        try_one!("V30 RTSP main", self.sdk.realplay_v30_with_window_rtsp(self.user_id, ch, false, hwnd));
        try_one!("V30 RTSP sub", self.sdk.realplay_v30_with_window_rtsp(self.user_id, ch, true, hwnd));

        anyhow::bail!("ch={} failed in all variations", ch)
    }

    /// Gera URLs RTSP exclusivas para Canal Zero.
    /// A senha NÃO é URL-encoded (o cliente RTSP do SDK não decodifica %XX).
    fn generate_zero_channel_rtsp_urls(&self) -> Vec<String> {
        let hosts = vec![
            format!("{}:554", self.host),
            self.host.clone(),
        ];

        let paths = [
            ("/Streaming/channels/001", &[""][..]),
            ("/Streaming/channels/002", &[""][..]),
            ("/Streaming/channels/0", &["?zeroChannel=1", "?zeroChannel=1&transportmode=unicast"][..]),
            ("/ISAPI/Streaming/channels/0", &["?zeroChannel=1"][..]),
            ("/zeroChannel=1", &[""][..]),
        ];

        let mut urls = Vec::new();
        for host in &hosts {
            for (path, params) in &paths {
                for param in *params {
                    urls.push(format!("rtsp://{}:{}@{}{}{}", self.user, self.password, host, path, param));
                }
            }
        }
        urls
    }

    /// Calcula candidatos a channel number do Canal Zero, do mais provável para o menos.
    fn zero_channel_candidates(v30: &crate::hcnetsdk::NET_DVR_DEVICEINFO_V30) -> Vec<i32> {
        let mut cand = Vec::new();

        let base129 = 129i32;
        for i in 0..v30.byZeroChanNum as i32 {
            let ch = base129 + i;
            if !cand.contains(&ch) { cand.push(ch); }
        }

        let c1 = v30.byStartDChan as i32 + v30.byIPChanNum as i32;
        if !cand.contains(&c1) { cand.push(c1); }

        let c2 = v30.byStartChan as i32 + v30.byChanNum as i32 + v30.byIPChanNum as i32;
        if !cand.contains(&c2) { cand.push(c2); }

        let c3 = v30.byStartChan as i32 + v30.byChanNum as i32;
        if !cand.contains(&c3) { cand.push(c3); }

        let c4 = v30.byStartDChan as i32;
        if !cand.contains(&c4) { cand.push(c4); }

        if !cand.contains(&1) { cand.push(1); }

        cand
    }

    /// Para o Canal Zero, se ativo.
    pub fn stop_zero_channel(&mut self) -> Result<()> {
        if let Some(zc) = self.zero_channel.take() {
            self.sdk.stop_realplay(zc.realplay_handle)?;
            log::info!("Canal Zero stopped (handle={})", zc.realplay_handle);
        }
        Ok(())
    }

    /// Retorna `true` se o Canal Zero está ativo.
    pub fn zero_channel_active(&self) -> bool {
        self.zero_channel.is_some()
    }

    /// Retorna o handle do Canal Zero, se ativo.
    pub fn zero_channel_handle(&self) -> Option<i32> {
        self.zero_channel.as_ref().map(|zc| zc.realplay_handle)
    }
}

impl Drop for HCNetSDKX11Multi {
    fn drop(&mut self) {
        self.stop_all();
        let _ = self.stop_zero_channel();
        if let Err(e) = self.sdk.logout(self.user_id) {
            log::warn!("Logout failed: {}", e);
        }
    }
}
