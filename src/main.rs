use hikvision_rs::api::{check_tls_fingerprint, Channel, DeviceInfo, FingerprintCheck, HikvisionAPI};
use hikvision_rs::hcnetsdk;
use hikvision_rs::hcnetsdk_x11_multi;
use hikvision_rs::playctrl_stream;
use hikvision_rs::rtsp;
use hikvision_rs::snapshot_stream;
use hikvision_rs::hcnetsdk_multi_stream;
use hikvision_rs::x11_embed;
use eframe::egui;
use raw_window_handle::HasWindowHandle;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
enum StreamMethod {
    Rtsp,
    Snapshot,
    PlayCtrl,
    HCNetSDK,
    #[allow(non_camel_case_types)]
    HCNetSDK_X11,
}

impl StreamMethod {
    fn label(&self) -> &'static str {
        match self {
            StreamMethod::Rtsp => "RTSP (direto)",
            StreamMethod::Snapshot => "Snapshot (JPEG polling)",
            StreamMethod::PlayCtrl => "PlayCtrl (descriptografia)",
            StreamMethod::HCNetSDK => "HCNetSDK (callback + PlayM4)",
            StreamMethod::HCNetSDK_X11 => "HCNetSDK X11 (overlay direto)",
        }
    }

    fn needs_verification_code(&self) -> bool {
        matches!(self, StreamMethod::PlayCtrl | StreamMethod::HCNetSDK | StreamMethod::HCNetSDK_X11)
    }

    fn needs_sdk_library(&self) -> bool {
        matches!(self, StreamMethod::PlayCtrl | StreamMethod::HCNetSDK | StreamMethod::HCNetSDK_X11)
    }

    fn show_sdk_port(&self) -> bool {
        matches!(self, StreamMethod::HCNetSDK | StreamMethod::HCNetSDK_X11)
    }

    /// Retorna true se o método renderiza via overlay X11 direto (sem egui texture).
    fn is_x11_overlay(&self) -> bool {
        matches!(self, StreamMethod::HCNetSDK_X11)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum LayoutMode {
    Single,
    Grid2x2,
    Grid3x3,
    Grid4x4,
}

impl LayoutMode {
    fn cols(self) -> usize {
        match self {
            LayoutMode::Single => 1,
            LayoutMode::Grid2x2 => 2,
            LayoutMode::Grid3x3 => 3,
            LayoutMode::Grid4x4 => 4,
        }
    }

    fn rows(self) -> usize {
        match self {
            LayoutMode::Single => 1,
            LayoutMode::Grid2x2 => 2,
            LayoutMode::Grid3x3 => 3,
            LayoutMode::Grid4x4 => 4,
        }
    }

    fn capacity(self) -> usize {
        self.cols() * self.rows()
    }

    fn label(self) -> &'static str {
        match self {
            LayoutMode::Single => "1x1",
            LayoutMode::Grid2x2 => "2x2",
            LayoutMode::Grid3x3 => "3x3",
            LayoutMode::Grid4x4 => "4x4",
        }
    }
}

fn config_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    PathBuf::from(home).join(".config/hikvision-rs")
}

fn config_path() -> PathBuf {
    config_dir().join("config.json")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Config {
    host: String,
    port: u16,
    sdk_port: u16,
    rtsp_port: u16,
    user: String,
    password: String,
    verification_code: String,
    library_path: String,
    use_substream: bool,
    use_https: bool,
    stream_method: StreamMethod,
    snapshot_interval: u64,
    #[serde(default)]
    cert_fingerprint: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host: "192.168.5.75".to_string(),
            port: 80,
            sdk_port: 8000,
            rtsp_port: 554,
            user: "admin".to_string(),
            password: String::new(),
            verification_code: String::new(),
            library_path: String::new(),
            use_substream: false,
            use_https: false,
            stream_method: StreamMethod::Snapshot,
            snapshot_interval: 300,
            cert_fingerprint: None,
        }
    }
}

impl Config {
    fn load() -> Self {
        let path = config_path();
        if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(contents) => {
                    // Tenta parser novo formato (com stream_method)
                    if let Ok(cfg) = serde_json::from_str::<Config>(&contents) {
                        log::info!("Config loaded from {}", path.display());
                        return cfg;
                    }
                    // Fallback: migra do formato antigo (bool individuais)
                    #[derive(Deserialize)]
                    struct OldConfig {
                        host: String,
                        port: u16,
                        sdk_port: Option<u16>,
                        rtsp_port: u16,
                        user: String,
                        password: String,
                        verification_code: Option<String>,
                        library_path: Option<String>,
                        use_substream: Option<bool>,
                        use_snapshot: Option<bool>,
                        use_playctrl: Option<bool>,
                        use_hcnetsdk: Option<bool>,
                        snapshot_interval: Option<u64>,
                    }
                    if let Ok(old) = serde_json::from_str::<OldConfig>(&contents) {
                        log::info!("Migrating config from old format");
                        let stream_method = if old.use_hcnetsdk.unwrap_or(false) {
                            StreamMethod::HCNetSDK
                        } else if old.use_playctrl.unwrap_or(false) {
                            StreamMethod::PlayCtrl
                        } else if old.use_snapshot.unwrap_or(true) {
                            StreamMethod::Snapshot
                        } else {
                            StreamMethod::Rtsp
                        };
                        let cfg = Config {
                            host: old.host,
                            port: old.port,
                            sdk_port: old.sdk_port.unwrap_or(8000),
                            rtsp_port: old.rtsp_port,
                            user: old.user,
                            password: old.password,
                            verification_code: old.verification_code.unwrap_or_default(),
                            library_path: old.library_path.unwrap_or_default(),
                            use_substream: old.use_substream.unwrap_or(false),
                            use_https: false,
                            stream_method,
                            snapshot_interval: old.snapshot_interval.unwrap_or(300),
                            cert_fingerprint: None,
                        };
                        return cfg;
                    }
                    log::warn!("Failed to parse config (tried old format too), using defaults");
                }
                Err(e) => log::warn!("Failed to read config: {}", e),
            }
        }
        Default::default()
    }

    fn save(&self) {
        let dir = config_dir();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            log::warn!("Failed to create config dir {}: {}", dir.display(), e);
            return;
        }
        let path = config_path();
        match serde_json::to_string_pretty(self) {
            Ok(contents) => {
                match std::fs::write(&path, &contents) {
                    Ok(()) => log::info!("Config saved to {}", path.display()),
                    Err(e) => log::warn!("Failed to save config: {}", e),
                }
            }
            Err(e) => log::warn!("Failed to serialize config: {}", e),
        }
    }

    fn apply(&self, app: &mut HikvisionApp) {
        app.host = self.host.clone();
        app.port = self.port.to_string();
        app.sdk_port = self.sdk_port.to_string();
        app.rtsp_port = self.rtsp_port.to_string();
        app.user = self.user.clone();
        app.password = self.password.clone();
        app.verification_code = self.verification_code.clone();
        app.library_path = self.library_path.clone();
        app.use_substream = self.use_substream;
        app.use_https = self.use_https;
        app.stream_method = self.stream_method;
        app.snapshot_interval = self.snapshot_interval;
        app.cert_fingerprint = self.cert_fingerprint.clone();
    }

    fn from_app(app: &HikvisionApp) -> Self {
        Self {
            host: app.host.trim().to_string(),
            port: app.port.trim().parse().unwrap_or(80),
            sdk_port: app.sdk_port.trim().parse().unwrap_or(8000),
            rtsp_port: app.rtsp_port.trim().parse().unwrap_or(554),
            user: app.user.trim().to_string(),
            password: app.password.clone(),
            verification_code: app.verification_code.clone(),
            library_path: app.library_path.clone(),
            use_substream: app.use_substream,
            use_https: app.use_https,
            stream_method: app.stream_method,
            snapshot_interval: app.snapshot_interval,
            cert_fingerprint: app.cert_fingerprint.clone(),
        }
    }
}

struct StreamState {
    texture: Option<egui::TextureHandle>,
    frame_width: usize,
    frame_height: usize,
    frame_rx: Option<mpsc::Receiver<rtsp::RtspFrame>>,
    stream_stop: Option<Arc<AtomicBool>>,
    stream_handle: Option<thread::JoinHandle<()>>,
    frame_count: u64,
    fps_timer: Instant,
    fps: f32,
}

impl StreamState {
    fn new() -> Self {
        Self {
            texture: None,
            frame_width: 0,
            frame_height: 0,
            frame_rx: None,
            stream_stop: None,
            stream_handle: None,
            frame_count: 0,
            fps_timer: Instant::now(),
            fps: 0.0,
        }
    }
}

struct HikvisionApp {
    host: String,
    port: String,
    sdk_port: String,
    rtsp_port: String,
    user: String,
    password: String,
    verification_code: String,
    library_path: String,
    use_substream: bool,
    use_https: bool,
    stream_method: StreamMethod,
    snapshot_interval: u64,

    api: Option<HikvisionAPI>,
    channels: Vec<Channel>,
    device_name: String,
    error: Option<String>,
    cert_fingerprint: Option<String>,
    pending_new_fingerprint: Option<String>,

    layout_mode: LayoutMode,
    prev_layout: LayoutMode,
    streams: Vec<StreamState>,
    focused_channel: Option<usize>,

    hcnetsdk_multi: Option<hcnetsdk_multi_stream::HCNetSDKMultiStream>,
    hcnetsdk_x11_multi: Option<hcnetsdk_x11_multi::HCNetSDKX11Multi>,
    x11_manager: Option<x11_embed::X11WindowManager>,
    x11_main_xid: Option<u32>,
    x11_window_xid_obtained: bool,
    /// Canais que precisam iniciar stream X11 mas a janela ainda não existia.
    /// Resolvido em try_start_pending_x11_streams() após ensure_window.
    x11_pending: Vec<usize>,
    /// Mapeia slot do grid (0..capacity) -> índice em channels[].
    /// Ex: grid_slots[0] = Some(4) significa que a câmera channels[4] aparece no slot 0.
    grid_slots: Vec<Option<usize>>,

    /// Número de canais zero suportados pelo DVR (lido do ISAPI após login).
    zero_channel_available: u8,
    /// Canal Zero está ativo no momento (toggle runtime).
    zero_channel_active: bool,
    /// Cópia do grid_slots antes de ativar o Canal Zero (para restaurar ao desativar).
    saved_grid_slots: Vec<Option<usize>>,
}

impl Default for HikvisionApp {
    fn default() -> Self {
        Self {
            host: "192.168.5.75".into(),
            port: "80".into(),
            sdk_port: "8000".into(),
            rtsp_port: "554".into(),
            user: "admin".into(),
            password: String::new(),
            verification_code: String::new(),
            library_path: String::new(),
            use_substream: false,
            use_https: false,
            stream_method: StreamMethod::Snapshot,
            snapshot_interval: 300,
            api: None,
            channels: Vec::new(),
            device_name: String::new(),
            error: None,
            cert_fingerprint: None,
            pending_new_fingerprint: None,
            layout_mode: LayoutMode::Single,
            prev_layout: LayoutMode::Single,
            streams: Vec::new(),
            focused_channel: None,
            hcnetsdk_multi: None,
            hcnetsdk_x11_multi: None,
            x11_manager: None,
            x11_main_xid: None,
            x11_window_xid_obtained: false,
            x11_pending: Vec::new(),
            grid_slots: Vec::new(),
            zero_channel_available: 0,
            zero_channel_active: false,
            saved_grid_slots: Vec::new(),
        }
    }
}

impl HikvisionApp {
    fn connect(&mut self) {
        let host = self.host.trim().to_string();
        let port: u16 = match self.port.trim().parse() {
            Ok(p) => p,
            Err(_) => {
                self.error = Some("Invalid port".into());
                return;
            }
        };
        let user = self.user.trim().to_string();
        let password = self.password.clone();

        if host.is_empty() {
            self.error = Some("Host is required".into());
            return;
        }

        if self.stream_method.needs_verification_code() && self.verification_code.trim().is_empty() {
            self.error = Some("Verification Code é obrigatório para este método de streaming".into());
            return;
        }

        if self.use_https {
            let stored = self.cert_fingerprint.as_deref();
            match check_tls_fingerprint(&host, port, stored) {
                Ok(FingerprintCheck::Match) => {}
                Ok(FingerprintCheck::New(fp)) => {
                    self.pending_new_fingerprint = Some(fp.clone());
                    self.error = Some(format!(
                        "Primeira conexão HTTPS.\nFingerprint do certificado:\n{}\n\nConfie neste certificado?",
                        fp
                    ));
                    return;
                }
                Ok(FingerprintCheck::Mismatch(old, new)) => {
                    self.pending_new_fingerprint = Some(new);
                    self.error = Some(format!(
                        "Certificate fingerprint changed!\nOld: {}\nNew: {}\nAccept the new fingerprint to proceed.",
                        old, self.pending_new_fingerprint.as_ref().unwrap()
                    ));
                    return;
                }
                Err(e) => {
                    self.error = Some(format!("Certificate fingerprint check failed: {}", e));
                    return;
                }
            }
        }

        let api = HikvisionAPI::new(&host, port, &user, &password, self.use_https);
        match api.device_info() {
            Ok(info) => {
                self.device_name =
                    format!("{} | {} | {}", info.name, info.model, info.firmware);
                self.zero_channel_available = info.zero_chan_num;

                match api.channels() {
                    Ok(chs) => {
                        let mut seen = std::collections::HashSet::new();
                        let mut deduped = Vec::new();
                        for ch in chs {
                            let ch_num = ch.id.parse::<u32>().unwrap_or(0) / 100;
                            if seen.insert(ch_num) {
                                deduped.push(ch);
                                if deduped.len() >= 17 {
                                    break;
                                }
                            }
                        }
                        self.channels = deduped;
                        self.streams = (0..self.channels.len())
                            .map(|_| StreamState::new())
                            .collect();
                        self.focused_channel = if self.channels.is_empty() {
                            None
                        } else {
                            Some(0)
                        };
                        self.grid_slots = vec![None; self.layout_mode.capacity()];
                        self.api = Some(api);
                        self.error = None;

                        Config::from_app(self).save();
                    }
                    Err(e) => self.error = Some(format!("Channels failed: {}", e)),
                }
            }
            Err(e) => self.error = Some(format!("Connection failed: {}", e)),
        }
    }

    fn rtsp_url(&self, channel_id: &str, force_substream: bool) -> String {
        let cid = if self.use_substream || force_substream {
            if channel_id.ends_with('1') {
                let mut s = channel_id[..channel_id.len() - 1].to_string();
                s.push('2');
                s
            } else {
                channel_id.to_string()
            }
        } else {
            channel_id.to_string()
        };

        let mut clean_host = self.host.trim();
        if clean_host.starts_with("http://") {
            clean_host = &clean_host[7..];
        } else if clean_host.starts_with("https://") {
            clean_host = &clean_host[8..];
        }
        clean_host = clean_host.trim_end_matches('/');

        let port_str = self.rtsp_port.trim();
        let final_port = if port_str.is_empty() {
            "554"
        } else {
            port_str
        };
        let host_port = format!("{}:{}", clean_host, final_port);

        let url_encode = |s: &str| -> String {
            let mut out = String::with_capacity(s.len() * 3);
            for b in s.bytes() {
                match b {
                    b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                        out.push(b as char);
                    }
                    _ => out.push_str(&format!("%{:02X}", b)),
                }
            }
            out
        };

        let safe_user = url_encode(self.user.trim());
        let safe_password = url_encode(&self.password);

        format!(
            "rtsp://{}:{}@{}/Streaming/Channels/{}",
            safe_user,
            safe_password,
            host_port,
            cid,
        )
    }

    /// Compute the SDK channel number for NET_DVR_PREVIEWINFO.lChannel.
    fn sdk_channel_for(&self, channel_id: &str) -> i32 {
        channel_id.trim().parse::<i32>().unwrap_or(100) / 100
    }

    fn start_stream(&mut self, channel_index: usize, ctx: &egui::Context) {
        if channel_index >= self.channels.len() {
            return;
        }
        if self.streams[channel_index].stream_handle.is_some() || self.streams[channel_index].frame_rx.is_some() {
            return;
        }

        let channel_id = self.channels[channel_index].id.clone();
        let channel_name = self.channels[channel_index].name.clone();
        let force_sub = matches!(
            self.layout_mode,
            LayoutMode::Grid2x2 | LayoutMode::Grid3x3 | LayoutMode::Grid4x4
        );

        let host = self.host.trim().to_string();
        let port: u16 = self.port.trim().parse().unwrap_or(80);
        let use_https = self.use_https;
        let sdk_port: u16 = self.sdk_port.trim().parse().unwrap_or(8000);
        let rtsp_port: u16 = self.rtsp_port.trim().parse().unwrap_or(554);
        let user = self.user.trim().to_string();
        let password = self.password.clone();
        let interval = self.snapshot_interval;
        let method = self.stream_method;
        let verification_code = self.verification_code.clone();
        let library_path = self.library_path.clone();
        let url = self.rtsp_url(&channel_id, force_sub);

        let (tx, rx) = mpsc::sync_channel::<rtsp::RtspFrame>(2);
        let stop = Arc::new(AtomicBool::new(false));
        let repaint_ctx = ctx.clone();

        // Pre-compute SDK channel before mutable borrow of self.streams
        let sdk_channel = self.sdk_channel_for(&channel_id);

        let state = &mut self.streams[channel_index];
        state.frame_rx = Some(rx);
        state.stream_stop = Some(stop.clone());
        state.frame_count = 0;
        state.fps_timer = Instant::now();
        state.fps = 0.0;

        match method {
            StreamMethod::Snapshot => {
                log::info!("Starting snapshot stream for channel {}: {}", channel_id, channel_name);
                let cid = channel_id.clone();
                let handle = thread::spawn(move || {
                    snapshot_stream::snapshot_stream_loop(
                        &cid, &host, port, &user, &password, use_https, tx, stop, repaint_ctx, interval,
                    );
                });
                state.stream_handle = Some(handle);
            }
            StreamMethod::HCNetSDK => {
                let main_stream = !force_sub;
                log::info!("Starting HCNetSDK callback stream ch {} (sdk={})", channel_id, sdk_channel);

                let vc = if verification_code.is_empty() {
                    None
                } else {
                    Some(verification_code.as_str())
                };

                if self.hcnetsdk_multi.is_none() {
                    let lp = if library_path.is_empty() {
                        None
                    } else {
                        Some(library_path.as_str())
                    };
                    match hcnetsdk_multi_stream::HCNetSDKMultiStream::new(
                        &host, sdk_port, &user, &password, vc, lp,
                    ) {
                        Ok(ms) => {
                            self.hcnetsdk_multi = Some(ms);
                        }
                        Err(e) => {
                            log::error!("Failed to create HCNetSDK multi-stream: {}", e);
                            self.error = Some(e.to_string());
                            return;
                        }
                    }
                }

                let multi = self.hcnetsdk_multi.as_mut().unwrap();
                match multi.start_channel(sdk_channel, main_stream) {
                    Ok(rx) => {
                        state.frame_rx = Some(rx);
                        log::info!("HCNetSDK callback ch {} started", sdk_channel);
                    }
                    Err(e) => {
                        log::error!("HCNetSDK start_channel {} failed: {}", sdk_channel, e);
                        self.error = Some(e.to_string());
                    }
                }
            }
            StreamMethod::HCNetSDK_X11 => {
                log::info!("Starting HCNetSDK X11 direct stream ch {} (sdk={})", channel_id, sdk_channel);

                if self.x11_main_xid.is_some() {
                    let vc = if verification_code.is_empty() {
                        None
                    } else {
                        Some(verification_code.as_str())
                    };

                    if self.hcnetsdk_x11_multi.is_none() {
                        match hcnetsdk_x11_multi::HCNetSDKX11Multi::new(
                            &host, sdk_port, &user, &password, vc,
                        ) {
                            Ok(ms) => {
                                self.hcnetsdk_x11_multi = Some(ms);
                            }
                            Err(e) => {
                                log::error!("Failed to create HCNetSDK X11 multi: {}", e);
                                self.error = Some(e.to_string());
                                return;
                            }
                        }
                    }

                    if !self.x11_pending.contains(&channel_index) {
                        self.x11_pending.push(channel_index);
                        log::info!("Channel index {} marked as X11 pending (window not yet created)", channel_index);
                    }

                    self.try_start_pending_x11_streams();
                } else {
                    log::warn!("X11 main window XID not available yet, deferring X11 stream start");
                    if !self.x11_pending.contains(&channel_index) {
                        self.x11_pending.push(channel_index);
                    }
                }
            }
            StreamMethod::PlayCtrl => {
                log::info!("Starting PlayCtrl stream for channel {}: {}", channel_id, channel_name);
                let lp = if library_path.is_empty() {
                    None
                } else {
                    Some(library_path)
                };
                let cid = channel_id.clone();
                let handle = thread::spawn(move || {
                    playctrl_stream::stream_loop(
                        &host, rtsp_port, &cid, &user, &password,
                        &verification_code, lp.as_deref(),
                        tx, stop, repaint_ctx,
                    );
                });
                state.stream_handle = Some(handle);
            }
            StreamMethod::Rtsp => {
                log::info!("Starting RTSP stream for channel {}: {}", channel_id, channel_name);
                let handle = thread::spawn(move || {
                    rtsp::stream_loop(&url, tx, stop, repaint_ctx);
                });
                state.stream_handle = Some(handle);
            }
        }
    }

    fn stop_stream(&mut self, channel_index: usize) {
        if self.stream_method == StreamMethod::HCNetSDK_X11 {
            if channel_index < self.channels.len() {
                let sdk_channel = self.sdk_channel_for(&self.channels[channel_index].id);
                if let Some(ref mut multi) = self.hcnetsdk_x11_multi {
                    multi.stop_channel(sdk_channel);
                }
                // Remove a janela X11 filha pelo slot onde este canal está alocado
                if let Some(ref mut mgr) = self.x11_manager {
                    if let Some(slot) = self.grid_slots.iter().position(|s| *s == Some(channel_index)) {
                        mgr.remove_window(slot);
                    }
                }
                // Remove dos pendentes
                self.x11_pending.retain(|&i| i != channel_index);
            }
            // Limpar stream state para que start_stream() não bloqueie na guarda
            if channel_index < self.streams.len() {
                self.streams[channel_index].stream_handle = None;
                self.streams[channel_index].frame_rx = None;
                self.streams[channel_index].texture = None;
            }
            return;
        }

        if self.stream_method == StreamMethod::HCNetSDK {
            if channel_index < self.channels.len() {
                let sdk_channel = self.sdk_channel_for(&self.channels[channel_index].id);
                if let Some(ref mut multi) = self.hcnetsdk_multi {
                    multi.stop_channel(sdk_channel);
                }
                let state = &mut self.streams[channel_index];
                state.frame_rx = None;
                state.texture = None;
                state.frame_width = 0;
                state.frame_height = 0;
            }
            return;
        }

        if channel_index >= self.streams.len() {
            return;
        }
        let state = &mut self.streams[channel_index];
        if let Some(stop) = state.stream_stop.take() {
            stop.store(true, Ordering::Relaxed);
        }
        if let Some(handle) = state.stream_handle.take() {
            let _ = handle.join();
        }
        state.frame_rx = None;
        state.texture = None;
        state.frame_width = 0;
        state.frame_height = 0;
    }

    fn stop_all_streams(&mut self) {
        if self.stream_method == StreamMethod::HCNetSDK_X11 {
            // Limpar grid_slots
            for slot in &mut self.grid_slots {
                *slot = None;
            }
            // Stop all SDK streams (zero channel + individuais) (logout via Drop)
            log::info!("hcnetsdk_x11_multi = None (stop_all_streams)");
            self.hcnetsdk_x11_multi = None;
            // Destroy all X11 overlay windows completely
            self.x11_manager = None;
            self.x11_pending.clear();
            self.zero_channel_active = false;
            // Limpar stream states para que start_stream() não bloqueie
            for state in &mut self.streams {
                state.stream_handle = None;
                state.frame_rx = None;
                state.texture = None;
                state.frame_width = 0;
                state.frame_height = 0;
            }
            return;
        }

        if self.stream_method == StreamMethod::HCNetSDK {
            for slot in &mut self.grid_slots {
                *slot = None;
            }
            if let Some(ref mut multi) = self.hcnetsdk_multi.take() {
                multi.stop_all();
            }
            for state in &mut self.streams {
                state.frame_rx = None;
                state.texture = None;
                state.stream_stop = None;
                state.stream_handle = None;
                state.frame_width = 0;
                state.frame_height = 0;
            }
            return;
        }
        for slot in &mut self.grid_slots {
            *slot = None;
        }
        for i in 0..self.streams.len() {
            self.stop_stream(i);
        }
    }

    /// Tenta iniciar streams X11 pendentes cujas janelas já foram criadas.
    /// Chamado a cada frame após ensure_window() na renderização.
    fn try_start_pending_x11_streams(&mut self) {
        if self.hcnetsdk_x11_multi.is_none() {
            return;
        }

        // --- CANAL ZERO PENDENTE ---
        if self.zero_channel_active {
            let win_id = self.x11_manager.as_ref().and_then(|mgr| mgr.window_id(999));
            if let Some(win_id) = win_id {
                let multi = self.hcnetsdk_x11_multi.as_mut().unwrap();
                if !multi.zero_channel_active() {
                    log::info!("Starting pending zero channel on window 0x{:x}", win_id);
                    match multi.start_zero_channel(win_id) {
                        Ok(()) => {
                            log::info!("Zero channel started successfully on window 0x{:x}", win_id);
                        }
                        Err(e) => {
                            log::error!("Failed to start zero channel: {}", e);
                            self.zero_channel_active = false;
                        }
                    }
                }
            }
        }

        if self.x11_pending.is_empty() {
            return;
        }

        let is_multi = matches!(self.layout_mode, LayoutMode::Grid2x2 | LayoutMode::Grid3x3 | LayoutMode::Grid4x4);

        let mut still_pending = Vec::new();
        for &idx in &self.x11_pending {
            if idx >= self.channels.len() {
                continue;
            }
            let channel_id = &self.channels[idx].id;
            let sdk_channel = self.sdk_channel_for(channel_id);

            // Já está ativo? Remove dos pendentes.
            if self.hcnetsdk_x11_multi.as_ref().map(|m| m.is_channel_active(sdk_channel)).unwrap_or(false) {
                log::info!("X11 pending: channel {} already active, removing from pending", sdk_channel);
                continue;
            }

            // Em modo multi-view, encontrar o slot que contém este canal
            let win_id = if is_multi {
                self.grid_slots.iter().position(|s| *s == Some(idx))
                    .and_then(|slot| self.x11_manager.as_ref().and_then(|mgr| mgr.window_id(slot)))
            } else {
                self.x11_manager.as_ref().and_then(|mgr| mgr.window_id(idx))
            };

            match win_id {
                Some(win_id) => {
                    let force_sub = is_multi;
                    let multi = self.hcnetsdk_x11_multi.as_mut().unwrap();
                    match multi.start_channel(sdk_channel, !force_sub, win_id) {
                        Ok(()) => {
                            log::info!("X11 pending: channel {} started on window 0x{:x}", sdk_channel, win_id);
                        }
                        Err(e) => {
                            log::error!("X11 pending: start_channel {} failed: {}", sdk_channel, e);
                            still_pending.push(idx);
                        }
                    }
                }
                None => {
                    // Janela ainda não existe ou canal não está em slot, manter pendente
                    still_pending.push(idx);
                }
            }
        }
        self.x11_pending = still_pending;
    }

    fn channel_is_active(&self, idx: usize) -> bool {
        if self.stream_method.is_x11_overlay() {
            if idx >= self.channels.len() {
                return false;
            }
            let sdk_channel = self.sdk_channel_for(&self.channels[idx].id);
            return self.hcnetsdk_x11_multi.as_ref()
                .map(|m| m.is_channel_active(sdk_channel))
                .unwrap_or(false)
                || self.x11_pending.contains(&idx);
        }
        if idx >= self.streams.len() {
            return false;
        }
        self.streams[idx].stream_handle.is_some() || self.streams[idx].frame_rx.is_some()
    }

    fn start_zero_channel(&mut self, _ctx: &egui::Context) {
        log::info!("start_zero_channel entered");
        if !self.stream_method.is_x11_overlay() {
            return;
        }

        // Salvar grid_slots atual para restaurar ao desativar
        self.saved_grid_slots = self.grid_slots.clone();

        let to_stop: Vec<usize> = self.grid_slots.iter().filter_map(|s| *s).collect();
        for slot in &mut self.grid_slots {
            *slot = None;
        }
        for ch_idx in to_stop {
            self.stop_stream(ch_idx);
        }
        self.x11_pending.clear();

        // Esconder todas as janelas X11 restantes uma única vez.
        // Isso substitui o hide_all() que estava no render loop a cada frame.
        if let Some(ref mut mgr) = self.x11_manager {
            mgr.hide_all();
        }

        if self.x11_main_xid.is_none() {
            log::warn!("X11 main window not available, cannot start zero channel");
            self.zero_channel_active = false;
            return;
        }

        // Garantir que multi existe (login SDK)
        if self.hcnetsdk_x11_multi.is_none() {
            let vc = if self.verification_code.is_empty() {
                None
            } else {
                Some(self.verification_code.as_str())
            };
            let host = self.host.trim();
            let sdk_port: u16 = self.sdk_port.trim().parse().unwrap_or(8000);
            match hcnetsdk_x11_multi::HCNetSDKX11Multi::new(
                host, sdk_port, self.user.trim(), &self.password, vc,
            ) {
                Ok(multi) => {
                    self.hcnetsdk_x11_multi = Some(multi);
                }
                Err(e) => {
                    log::error!("Failed to create HCNetSDKX11Multi for zero channel: {}", e);
                    self.zero_channel_active = false;
                    return;
                }
            }
        }

        // Garantir que X11 manager existe
        if self.x11_manager.is_none() {
            self.x11_manager = Some(x11_embed::X11WindowManager::new());
        }

        // Sinaliza que o canal zero está ativo. O loop de renderização irá criar a janela
        // com o tamanho correto, e depois try_start_pending_x11_streams iniciará o stream.
        self.zero_channel_active = true;
    }

    fn stop_zero_channel(&mut self) {
        log::info!("Stopping zero channel");
        // Para o stream do Canal Zero no multi existente (sem fazer logout)
        if let Some(ref mut multi) = self.hcnetsdk_x11_multi {
            let _ = multi.stop_zero_channel();
        }
        // Remove a janela do Canal Zero do manager
        if let Some(ref mut mgr) = self.x11_manager {
            mgr.remove_window(999);
        }
        // Restaura os canais individuais que estavam ativos antes
        self.grid_slots = std::mem::replace(&mut self.saved_grid_slots, Vec::new());
        self.zero_channel_active = false;
    }

    fn drain_frames(&mut self, ctx: &egui::Context) {
        // No modo X11 overlay, não há frames para drenar (o SDK renderiza direto)
        if self.stream_method.is_x11_overlay() {
            return;
        }
        for (i, state) in self.streams.iter_mut().enumerate() {
            if let Some(rx) = &state.frame_rx {
                while let Ok(frame) = rx.try_recv() {
                    let w = frame.width as usize;
                    let h = frame.height as usize;
                    let color_image =
                        egui::ColorImage::from_rgba_unmultiplied([w, h], &frame.rgba);
                    if let Some(ref mut tex) = state.texture {
                        tex.set(color_image, egui::TextureOptions::LINEAR);
                    } else {
                        state.texture = Some(ctx.load_texture(
                            format!("stream_{}", i),
                            color_image,
                            egui::TextureOptions::LINEAR,
                        ));
                    }
                    state.frame_width = w;
                    state.frame_height = h;
                    state.frame_count += 1;
                }
            }

            let elapsed = state.fps_timer.elapsed();
            if elapsed >= std::time::Duration::from_secs(1) {
                state.fps = state.frame_count as f32 / elapsed.as_secs_f32();
                state.frame_count = 0;
                state.fps_timer = Instant::now();
            }
        }
    }

    fn show_login(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(100.0);
                ui.heading("Hikvision DVR Viewer");
                ui.label("Streaming via ISAPI & RTSP");
                ui.add_space(20.0);

                let field_w = 320.0;
                let field_h = 24.0;
                egui::Grid::new("login")
                    .spacing([8.0, 6.0])
                    .min_col_width(340.0)
                    .show(ui, |ui| {
                        ui.label("Host:");
                        ui.add_sized([field_w, field_h], egui::TextEdit::singleline(&mut self.host));
                        ui.end_row();
                        let port_label = if self.use_https { "HTTPS Port:" } else { "HTTP Port:" };
                        ui.label(port_label);
                        ui.add_sized([field_w, field_h], egui::TextEdit::singleline(&mut self.port));
                        ui.end_row();
                        let prev_https = self.use_https;
                        ui.label("HTTPS:");
                        ui.checkbox(&mut self.use_https, "Usar HTTPS (certificado auto-assinado)");
                        if self.use_https != prev_https {
                            let cur_port = self.port.trim().parse().unwrap_or(0);
                            if self.use_https && cur_port == 80 {
                                self.port = "443".to_string();
                            } else if !self.use_https && cur_port == 443 {
                                self.port = "80".to_string();
                            }
                        }
                        ui.end_row();
                        ui.label("RTSP Port:");
                        ui.add_sized([field_w, field_h], egui::TextEdit::singleline(&mut self.rtsp_port));
                        ui.end_row();
                        if self.stream_method.show_sdk_port() {
                            ui.label("SDK Port:");
                            ui.add_sized([field_w, field_h], egui::TextEdit::singleline(&mut self.sdk_port).hint_text("8000"));
                            ui.end_row();
                        }
                        ui.label("Username:");
                        ui.add_sized([field_w, field_h], egui::TextEdit::singleline(&mut self.user));
                        ui.end_row();
                        ui.label("Password:");
                        ui.add_sized([field_w, field_h], egui::TextEdit::singleline(&mut self.password).password(true));
                        ui.end_row();
                        ui.label("Método:");
                        egui::ComboBox::from_id_salt("stream_method")
                            .selected_text(self.stream_method.label())
                            .width(field_w)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut self.stream_method, StreamMethod::Rtsp, StreamMethod::Rtsp.label());
                                ui.selectable_value(&mut self.stream_method, StreamMethod::Snapshot, StreamMethod::Snapshot.label());
                                ui.selectable_value(&mut self.stream_method, StreamMethod::PlayCtrl, StreamMethod::PlayCtrl.label());
                                ui.selectable_value(&mut self.stream_method, StreamMethod::HCNetSDK, StreamMethod::HCNetSDK.label());
                                ui.selectable_value(&mut self.stream_method, StreamMethod::HCNetSDK_X11, StreamMethod::HCNetSDK_X11.label());
                            });
                        ui.end_row();
                        ui.label("");
                        ui.checkbox(&mut self.use_substream, "Sub-stream (menor resolução, mais leve)");
                        ui.end_row();
                        if self.stream_method == StreamMethod::Snapshot {
                            ui.label("Intervalo (ms):");
                            ui.add(egui::Slider::new(&mut self.snapshot_interval, 100u64..=2000).suffix("ms"));
                            ui.end_row();
                        }
                        if self.stream_method.needs_verification_code() {
                            ui.label("Verification Code:");
                            ui.add_sized([field_w, field_h], egui::TextEdit::singleline(&mut self.verification_code).password(true));
                            ui.end_row();
                        }
                        if self.stream_method.needs_sdk_library() {
                            let hint = if self.stream_method == StreamMethod::HCNetSDK || self.stream_method == StreamMethod::HCNetSDK_X11 {
                                "libhcnetsdk.so (vazio=auto)"
                            } else {
                                "Deixe vazio para buscar automático"
                            };
                            ui.label("Library Path:");
                            ui.add_sized([field_w, field_h], egui::TextEdit::singleline(&mut self.library_path).hint_text(hint));
                            ui.end_row();
                        }
                    });

                ui.add_space(10.0);
                if ui.button("Connect").clicked() {
                    self.connect();
                }

                ui.add_space(10.0);
                let status = match self.stream_method {
                    StreamMethod::HCNetSDK => egui::RichText::new("🔐 HCNetSDK (callback + PlayM4). Descriptografia automática via NET_DVR_SetSDKSecretKey.").small().color(egui::Color32::DARK_GREEN),
                    StreamMethod::HCNetSDK_X11 => egui::RichText::new("🖥️ HCNetSDK X11 (overlay direto). SDK renderiza via X11 — sem decodificação manual. Requer libhcnetsdk.so.").small().color(egui::Color32::DARK_GREEN),
                    StreamMethod::PlayCtrl => egui::RichText::new("🔐 PlayCtrl com descriptografia. Requer libPlayCtrl.so e Verification Code do DVR.").small().color(egui::Color32::DARK_GREEN),
                    StreamMethod::Snapshot => egui::RichText::new("ℹ️ Snapshot JPEG polling. ~2-3 FPS. Não requer desativar criptografia.").small().color(egui::Color32::DARK_GRAY),
                    StreamMethod::Rtsp => egui::RichText::new("⚠️ RTSP direto. Se a 'Criptografia de Transmissão' estiver ativada no DVR, o vídeo não carregará.").small().color(egui::Color32::DARK_GRAY),
                };
                ui.label(status);

                if let Some(ref err) = self.error {
                    ui.colored_label(egui::Color32::RED, err);
                }
            });
        });

        if let Some(ref new_fp) = self.pending_new_fingerprint.clone() {
            let is_first = self.cert_fingerprint.is_none();
            let title = if is_first { "Confirmar certificado" } else { "Fingerprint alterado" };
            egui::Window::new(title)
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    if is_first {
                        ui.label("Primeira conexão HTTPS com este servidor.");
                        ui.label("Verifique o fingerprint do certificado:");
                    } else {
                        ui.label("O certificado SSL do servidor mudou!");
                        ui.label("Isso pode significar:");
                        ui.label("  • O DVR foi reiniciado de fábrica");
                        ui.label("  • O certificado foi regenerado");
                        ui.add_space(6.0);
                        if let Some(ref old_fp) = self.cert_fingerprint {
                            ui.label(format!("Fingerprint antigo: {}", old_fp));
                        }
                    }
                    ui.add_space(4.0);
                    ui.label(format!("Fingerprint: {}", new_fp));
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.button("✓ Aceitar (salvar fingerprint)").clicked() {
                            self.cert_fingerprint = Some(new_fp.clone());
                            self.pending_new_fingerprint = None;
                            self.error = None;
                            self.connect();
                        }
                        if ui.button("✗ Rejeitar").clicked() {
                            self.pending_new_fingerprint = None;
                            self.error = Some("Conexão rejeitada".into());
                        }
                    });
                });
        }
    }

    fn show_viewer(&mut self, ctx: &egui::Context) {
        if self.layout_mode != self.prev_layout {
            self.prev_layout = self.layout_mode;

            // Reconstruir grid_slots para o novo layout
            let new_cap = self.layout_mode.capacity();
            let old_slots = std::mem::replace(&mut self.grid_slots, vec![None; new_cap]);
            let mut active_channels: Vec<usize> = old_slots.iter().filter_map(|s| *s).collect();
            // Incluir canais ativos que não estavam em slots (ex: modo Single)
            for i in 0..self.channels.len() {
                if self.channel_is_active(i) && !active_channels.contains(&i) {
                    active_channels.push(i);
                }
            }

            // Re-assign canais ativos aos novos slots na ordem
            for (i, ch_idx) in active_channels.iter().enumerate() {
                if i < new_cap {
                    self.grid_slots[i] = Some(*ch_idx);
                }
            }

            if self.stream_method.is_x11_overlay() {
                log::info!("Layout change: {} active channels to re-start", active_channels.len());

                // Destruir tudo e reconstruir limpo
                log::info!("hcnetsdk_x11_multi = None (layout change)");
                self.hcnetsdk_x11_multi = None;
                self.x11_manager = None;
                self.x11_pending.clear();

                // Limpar stream states para que start_stream() não bloqueie na guarda
                for state in &mut self.streams {
                    state.stream_handle = None;
                    state.frame_rx = None;
                    state.texture = None;
                    state.frame_width = 0;
                    state.frame_height = 0;
                }

                if self.x11_window_xid_obtained {
                    self.x11_manager = Some(x11_embed::X11WindowManager::new());
                }

                // Re-iniciar canais que estavam ativos
                for ch_idx in active_channels {
                    log::info!("Layout change: re-starting channel index {}", ch_idx);
                    self.start_stream(ch_idx, ctx);
                }
            } else {
                for ch_idx in active_channels {
                    self.stop_stream(ch_idx);
                    self.start_stream(ch_idx, ctx);
                }
            }
        }

        // Inicializar X11 overlay se necessário
        if self.stream_method.is_x11_overlay() && self.x11_manager.is_none() && self.x11_window_xid_obtained {
            log::info!("Initializing X11 overlay window manager");
            self.x11_manager = Some(x11_embed::X11WindowManager::new());
        }

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(&self.device_name);
                if self.streams.iter().any(|s| s.stream_handle.is_some() || s.frame_rx.is_some()) {
                    ui.separator();
                    let active = self.streams.iter().filter(|s| s.stream_handle.is_some() || s.frame_rx.is_some()).count();
                    let total = self.channels.len();
                    ui.label(format!("{}/{} streams", active, total));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Disconnect").clicked() {
                        self.stop_all_streams();
                        self.api = None;
                        self.channels.clear();
                        self.streams.clear();
                        self.focused_channel = None;
                    }
                });

            });
        });

        egui::SidePanel::left("channels")
            .resizable(false)
            .default_width(200.0)
            .show(ctx, |ui| {
                ui.heading("Channels");
                let method_label = match self.stream_method {
                    StreamMethod::HCNetSDK => egui::RichText::new("🔐 HCNetSDK").small().color(egui::Color32::DARK_GREEN),
                    StreamMethod::HCNetSDK_X11 => egui::RichText::new("🖥️ HCNetSDK X11").small().color(egui::Color32::DARK_GREEN),
                    StreamMethod::PlayCtrl => egui::RichText::new("🔐 PlayCtrl").small().color(egui::Color32::DARK_GREEN),
                    StreamMethod::Snapshot => egui::RichText::new("📷 Snapshot JPEG").small().color(egui::Color32::GREEN),
                    StreamMethod::Rtsp => egui::RichText::new("🎥 RTSP direto").small().color(egui::Color32::LIGHT_BLUE),
                };
                ui.label(method_label);
                ui.separator();

                egui::ComboBox::from_label("Layout")
                    .selected_text(self.layout_mode.label())
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.layout_mode, LayoutMode::Single, "1x1");
                        ui.selectable_value(&mut self.layout_mode, LayoutMode::Grid2x2, "2x2");
                        ui.selectable_value(&mut self.layout_mode, LayoutMode::Grid3x3, "3x3");
                        ui.selectable_value(&mut self.layout_mode, LayoutMode::Grid4x4, "4x4");
                    });

                if matches!(self.layout_mode, LayoutMode::Single) {
                    ui.checkbox(&mut self.use_substream, "Sub-stream");
                } else {
                    ui.colored_label(
                        egui::Color32::GRAY,
                        "Sub-stream (auto em multi-view)",
                    );
                }

                ui.separator();

                egui::ScrollArea::vertical().show(ui, |ui| {
                    let channels = self.channels.clone();
                    match self.layout_mode {
                        LayoutMode::Single => {
                            for (i, ch) in channels.iter().enumerate() {
                                let selected = self.focused_channel == Some(i);
                                let label = format!("[{}] {}", ch.id, ch.name);
                                if ui.selectable_label(selected, &label).clicked() {
                                    self.focused_channel = Some(i);
                                    if !self.channel_is_active(i) {
                                        self.start_stream(i, ctx);
                                    }
                                }
                            }
                        }
                        _ => {
                            for (i, ch) in channels.iter().enumerate() {
                                let is_assigned = self.grid_slots.iter().any(|s| *s == Some(i));
                                let mut checked = is_assigned;
                                let label = format!("[{}] {}", ch.id, ch.name);
                                if ui.checkbox(&mut checked, &label).changed() {
                                    if checked {
                                        if let Some(empty) = self.grid_slots.iter().position(|s| s.is_none()) {
                                            self.grid_slots[empty] = Some(i);
                                            self.start_stream(i, ctx);
                                        }
                                    } else if let Some(slot) = self.grid_slots.iter().position(|s| *s == Some(i)) {
                                        self.grid_slots[slot] = None;
                                        self.stop_stream(i);
                                    }
                                }
                            }
                            ui.separator();
                            if ui.button("Start All").clicked() {
                                let cap = self.layout_mode.capacity();
                                let empty_slots: Vec<usize> = (0..cap)
                                    .filter(|s| self.grid_slots.get(*s).map_or(true, |v| v.is_none()))
                                    .collect();
                                let mut next = 0;
                                for i in 0..self.channels.len().min(cap) {
                                    if !self.grid_slots.iter().any(|s| *s == Some(i)) {
                                        if next < empty_slots.len() {
                                            self.grid_slots[empty_slots[next]] = Some(i);
                                            self.start_stream(i, ctx);
                                            next += 1;
                                        }
                                    }
                                }
                            }
                            if ui.button("Stop All").clicked() {
                                for slot in 0..self.grid_slots.len() {
                                    if let Some(ch_idx) = self.grid_slots[slot].take() {
                                        self.stop_stream(ch_idx);
                                    }
                                }
                            }
                        }
                    }
                });

                if self.stream_method.is_x11_overlay() {
                    ui.separator();
                    let label = if self.zero_channel_available > 0 {
                        format!("Canal Zero ({})", self.zero_channel_available)
                    } else {
                        "Canal Zero".to_string()
                    };
                    if ui.checkbox(&mut self.zero_channel_active, &label).clicked() {
                        if self.zero_channel_active {
                            self.start_zero_channel(ctx);
                        } else {
                            self.stop_zero_channel();
                        }
                    }
                }
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            match self.layout_mode {
                LayoutMode::Single => self.show_single_view(ui),
                _ => self.show_multi_view(ui),
            }
        });
    }

    fn show_single_view(&mut self, ui: &mut egui::Ui) {
        // Modo X11 overlay: sincronizar janela filha e mostrar label
        if self.stream_method.is_x11_overlay() {
            let rect = ui.max_rect();

            if self.zero_channel_active {
                // Canal Zero ativo: janela dedicada ocupando toda área de visualização.
                // NÃO chamar hide_all() aqui — os streams individuais já foram parados
                // em start_zero_channel(). Chamar hide_all() a cada frame causa
                // unmap/map repetido que faz a tela piscar.
                if let Some(ref mut mgr) = self.x11_manager {
                    const ZC_SLOT: usize = 999;
                    mgr.ensure_window(ZC_SLOT, rect.min.x, rect.min.y, rect.width(), rect.height());
                }
                ui.vertical_centered(|ui| {
                    ui.colored_label(egui::Color32::DARK_GREEN, "● Canal Zero — Mosaico multi-câmera (X11 overlay)");
                });
                return;
            }

            let idx = match self.focused_channel {
                Some(i) if i < self.channels.len() => i,
                _ => {
                    ui.vertical_centered(|ui| {
                        ui.add_space(100.0);
                        ui.label("Select a channel to view");
                    });
                    return;
                }
            };

            if let Some(ref mut mgr) = self.x11_manager {
                // No modo Single, esconder janelas de outros canais
                mgr.hide_all_except(idx);
                mgr.ensure_window(idx, rect.min.x, rect.min.y, rect.width(), rect.height());
            }

            ui.vertical_centered(|ui| {
                let name = &self.channels[idx].name;
                if self.channel_is_active(idx) {
                    ui.colored_label(egui::Color32::DARK_GREEN, format!("● {} — Live (X11 overlay)", name));
                } else {
                    ui.add_space(100.0);
                    ui.label(name);
                    ui.colored_label(egui::Color32::DARK_GRAY, "X11 overlay — aguardando stream");
                }
            });
            return;
        }

        let idx = match self.focused_channel {
            Some(i) if i < self.streams.len() => i,
            _ => {
                ui.vertical_centered(|ui| {
                    ui.add_space(100.0);
                    ui.label("Select a channel to view");
                });
                return;
            }
        };

        let state = &self.streams[idx];
        if let Some(ref tex) = state.texture {
            let size = ui.available_size();
            if state.frame_width > 0 && state.frame_height > 0 && size.x > 0.0 && size.y > 0.0 {
                let img_aspect = state.frame_width as f32 / state.frame_height as f32;
                let area_aspect = size.x / size.y;
                let scaled = if img_aspect > area_aspect {
                    egui::Vec2::new(size.x, size.x / img_aspect)
                } else {
                    egui::Vec2::new(size.y * img_aspect, size.y)
                };
                ui.image(egui::load::SizedTexture::new(tex.id(), scaled));
            }
            let name = &self.channels[idx].name;
            let fps = self.streams[idx].fps;
            ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                ui.label(format!(
                    "{} | {}x{} | {:.1} fps",
                    name, state.frame_width, state.frame_height, fps
                ));
            });
        } else {
            ui.vertical_centered(|ui| {
                ui.add_space(100.0);
                if self.channel_is_active(idx) {
                    ui.spinner();
                    let loading_label = match self.stream_method {
                        StreamMethod::HCNetSDK => "HCNetSDK (callback mode)",
                        StreamMethod::HCNetSDK_X11 => "HCNetSDK X11 (overlay mode)",
                        StreamMethod::PlayCtrl => "PlayCtrl decrypting...",
                        StreamMethod::Snapshot => "Polling snapshot...",
                        StreamMethod::Rtsp => "Connecting to RTSP stream...",
                    };
                    ui.label(loading_label);
                } else {
                    ui.label("Select a channel to view");
                }
            });
        }
    }

    fn show_multi_view(&mut self, ui: &mut egui::Ui) {
        // Canal Zero ativo: substitui o grid multi-view pela janela do mosaico
        if self.zero_channel_active && self.stream_method.is_x11_overlay() {
            let rect = ui.max_rect();
            // NÃO chamar hide_all() aqui — os streams individuais já foram parados
            // em start_zero_channel(). Chamar hide_all() a cada frame causa
            // unmap/map repetido que faz a tela piscar.
            if let Some(ref mut mgr) = self.x11_manager {
                const ZC_SLOT: usize = 999;
                mgr.ensure_window(ZC_SLOT, rect.min.x, rect.min.y, rect.width(), rect.height());
            }
            ui.vertical_centered(|ui| {
                ui.colored_label(egui::Color32::DARK_GREEN, "● Canal Zero — Mosaico multi-câmera (X11 overlay)");
            });
            return;
        }

        let spacing = 2.0;
        let cols = self.layout_mode.cols() as f32;
        let rows = self.layout_mode.rows() as f32;
        let avail = ui.available_size();
        if avail.x <= 0.0 || avail.y <= 0.0 {
            return;
        }

        let cell_w = ((avail.x - spacing * (cols - 1.0)) / cols).max(1.0);
        let cell_h = ((avail.y - spacing * (rows - 1.0)) / rows).max(1.0);
        let cell_size = egui::vec2(cell_w, cell_h);

        // Para modo X11 overlay, sincronizar janelas filhas com as células
        let is_x11 = self.stream_method.is_x11_overlay();

        for row in 0..self.layout_mode.rows() {
            ui.horizontal(|ui| {
                for col in 0..self.layout_mode.cols() {
                    let slot = row * self.layout_mode.cols() + col;
                    let (rect, _response) = ui.allocate_exact_size(cell_size, egui::Sense::click());

                    if is_x11 {
                        // Modo X11: sincronizar janela overlay com a posição do slot
                        if let Some(ref mut mgr) = self.x11_manager {
                            mgr.ensure_window(slot, rect.min.x, rect.min.y, cell_size.x, cell_size.y);
                        }
                    }

                    let channel_idx = self.grid_slots.get(slot).copied().flatten();

                    let mut cell_ui = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(rect)
                            .layout(egui::Layout::top_down(egui::Align::Center)),
                    );
                    self.render_cell(&mut cell_ui, slot, channel_idx, cell_size);
                }
            });
        }
    }

    fn render_cell(&self, ui: &mut egui::Ui, _slot: usize, channel_idx: Option<usize>, cell_size: egui::Vec2) {
        // Dark background
        ui.painter().rect_filled(
            ui.max_rect(),
            0.0,
            egui::Color32::from_rgb(10, 10, 10),
        );

        let ch_idx = match channel_idx {
            Some(ci) if ci < self.channels.len() => ci,
            _ => {
                ui.add_space(cell_size.y * 0.4);
                ui.colored_label(egui::Color32::DARK_GRAY, "No Signal");
                return;
            }
        };

        let channel_name = format!("[{}]", self.channels[ch_idx].id);

        // Modo X11 overlay: o SDK renderiza direto na janela X11.
        // Não desenhamos textura via egui — apenas mostramos o label.
        if self.stream_method.is_x11_overlay() {
            let is_active = self.channel_is_active(ch_idx);
            ui.add_space(cell_size.y * 0.35);
            ui.colored_label(egui::Color32::WHITE, &channel_name);
            if is_active {
                ui.colored_label(egui::Color32::DARK_GREEN, "● Live");
            } else {
                ui.colored_label(egui::Color32::DARK_GRAY, "Sem Sinal");
            }
            return;
        }

        // Modos baseados em texture (RTSP, Snapshot, PlayCtrl, HCNetSDK callback)
        if ch_idx >= self.streams.len() {
            ui.add_space(cell_size.y * 0.4);
            ui.colored_label(egui::Color32::DARK_GRAY, &channel_name);
            return;
        }

        let state = &self.streams[ch_idx];

        if let Some(ref tex) = state.texture {
            let label_h = 18.0;
            let margin = 2.0;
            let img_w = (cell_size.x - 2.0 * margin).max(1.0);
            let img_h = (cell_size.y - label_h - 2.0 * margin).max(1.0);

            let (final_w, final_h) = if state.frame_width > 0 && state.frame_height > 0 {
                let img_aspect = state.frame_width as f32 / state.frame_height as f32;
                let cell_aspect = img_w / img_h;
                if img_aspect > cell_aspect {
                    (img_w, img_w / img_aspect)
                } else {
                    (img_h * img_aspect, img_h)
                }
            } else {
                (img_w, img_h)
            };

            ui.add_space((cell_size.y - label_h - final_h) * 0.5);
            ui.image(egui::load::SizedTexture::new(
                tex.id(),
                egui::vec2(final_w, final_h),
            ));
            ui.add_space(1.0);

            ui.horizontal(|ui| {
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(format!(
                        "{} {:.0}fps",
                        channel_name, state.fps
                    ))
                    .size(12.0)
                    .color(egui::Color32::WHITE),
                );
            });
        } else {
            ui.add_space(cell_size.y * 0.35);
            ui.colored_label(egui::Color32::GRAY, &channel_name);
            if ch_idx < self.streams.len() && (self.streams[ch_idx].stream_handle.is_some() || self.streams[ch_idx].frame_rx.is_some()) {
                ui.colored_label(egui::Color32::DARK_GRAY, "Aguardando...");
            } else {
                ui.colored_label(egui::Color32::DARK_GRAY, "Sem Sinal");
            }
        }
    }
}

impl eframe::App for HikvisionApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // Obter XID da janela principal no primeiro frame (para X11 overlay)
        if !self.x11_window_xid_obtained {
            if let Ok(wh) = frame.window_handle() {
                let raw = wh.as_raw();
                if let Some(xid) = x11_embed::xid_from_raw_handle(&raw) {
                    self.x11_main_xid = Some(xid);
                    x11_embed::set_main_window_xid(xid);
                    self.x11_window_xid_obtained = true;
                    log::info!("Main window XID: 0x{:x}", xid);
                }
            }
        }

        // Atualizar posição global da janela principal para overlay X11
        if self.x11_window_xid_obtained {
            x11_embed::update_main_window_pos_from_x11();
        }

        // Poll X11 events de todas as janelas overlay
        if let Some(ref mut mgr) = self.x11_manager {
            mgr.poll_all();
        }

        if self.api.is_some() && !self.channels.is_empty() {
            self.drain_frames(ctx);
            ctx.request_repaint();
            self.show_viewer(ctx);

            // Após show_viewer(), as janelas X11 já foram criadas por ensure_window().
            // Agora podemos iniciar streams pendentes.
            if self.stream_method.is_x11_overlay() {
                self.try_start_pending_x11_streams();
            }
        } else {
            self.show_login(ctx);
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        log::info!("on_exit called");
        self.stop_all_streams();
        self.x11_manager = None;
        log::info!("hcnetsdk_x11_multi = None (on_exit)");
        self.hcnetsdk_x11_multi = None;
        self.x11_pending.clear();
        self.zero_channel_active = false;
    }
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    ffmpeg_next::init().expect("Failed to initialize FFmpeg");
    ffmpeg_next::log::set_level(ffmpeg_next::log::Level::Warning);

    // Initialize X11 connection for overlay windows
    if x11_embed::init_x11().is_none() {
        log::warn!("X11 connection failed — X11 overlay mode will not work");
    }

    // Initialize HCNetSDK on the main thread (required by the Hikvision SDK
    // for X11/rendering resource initialization) before spawning any threads.
    // This is idempotent only done once at startup.
    if let Err(e) = hcnetsdk::ensure_initialized() {
        log::warn!("HCNetSDK init failed (non-fatal for RTSP/Snapshot modes): {}", e);
    }

    let config = Config::load();
    let mut app = HikvisionApp::default();
    config.apply(&mut app);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1280.0, 720.0]),
        ..Default::default()
    };
    let _ = eframe::run_native(
        "Hikvision DVR Viewer",
        options,
        Box::new(move |_cc| Ok(Box::new(app))),
    );
}
