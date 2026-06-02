use crate::hcnetsdk::{self, HCNetSDK, NET_DVR_DEVICEINFO_V40};
use crate::playctrl::{self, PlayCtrl, FrameInfo, DecCallBack, T_YV12, T_RGB32};
use crate::rtsp::RtspFrame;
use anyhow::{Context, Result};
use std::collections::VecDeque;
use std::ffi::{c_char, c_int};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex, OnceLock};

const MAX_FRAME_QUEUE: usize = 120;

static PLAYCTRL: OnceLock<PlayCtrl> = OnceLock::new();

fn get_cached_playctrl(library_path: Option<&str>) -> Result<&'static PlayCtrl> {
    if let Some(pc) = PLAYCTRL.get() {
        return Ok(pc);
    }
    log::info!("Loading libPlayCtrl.so (cached once for hcnetsdk)");
    let pc = if let Some(path) = library_path {
        PlayCtrl::load_from(std::path::Path::new(path))?
    } else {
        playctrl::search_and_load()?
    };
    let _ = PLAYCTRL.set(pc);
    Ok(PLAYCTRL.get().unwrap())
}

// ---- Stream state (shared between callback and loop) ----
struct StreamState {
    queue: VecDeque<Vec<u8>>,
    syshead: Vec<u8>,
    syshead_sent: bool,
    error: Option<String>,
}

struct DecodedFrame {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

type FrameBuffer = Arc<Mutex<Option<DecodedFrame>>>;

// ---- HCNetSDK wrapper ----
pub struct HCNetSDKStream {
    sdk: Arc<HCNetSDK>,
    user_id: i32,
    realplay_handle: Option<i32>,
    state: Arc<Mutex<StreamState>>,
    device_info: NET_DVR_DEVICEINFO_V40,
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

        let sdk = hcnetsdk::search_and_load()
            .context("Failed to load HCNetSDK")?;

        sdk.init().context("NET_DVR_Init failed")?;
        sdk.set_connect_time(10000, 1).context("NET_DVR_SetConnectTime failed")?;

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

        let state = Arc::new(Mutex::new(StreamState {
            queue: VecDeque::new(),
            syshead: Vec::new(),
            syshead_sent: false,
            error: None,
        }));

        Ok(Self { sdk: Arc::new(sdk), user_id, realplay_handle: None, state, device_info })
    }

    pub fn start(&mut self, channel: i32, main_stream: bool) -> Result<()> {
        if self.realplay_handle.is_some() {
            anyhow::bail!("Stream already started");
        }
        let stream_type = if main_stream { hcnetsdk::STREAM_MAIN } else { hcnetsdk::STREAM_SUB };
        log::info!("Starting preview: channel={}, stream_type={}", channel, stream_type);

        extern "C" fn data_callback(
            _handle: i32,
            data_type: u32,
            buffer: *mut u8,
            buf_size: u32,
            user_data: *mut std::ffi::c_void,
        ) {
            if buffer.is_null() || buf_size == 0 { return; }
            let state = unsafe { &*(user_data as *const Mutex<StreamState>) };
            let data = unsafe { std::slice::from_raw_parts(buffer, buf_size as usize) };
            let mut s = state.lock().unwrap();
            if data_type == hcnetsdk::NET_DVR_SYSHEAD {
                s.syshead = data.to_vec();
                s.syshead_sent = false;
            } else if data_type == hcnetsdk::NET_DVR_STREAMDATA {
                if s.queue.len() >= MAX_FRAME_QUEUE { s.queue.pop_front(); }
                s.queue.push_back(data.to_vec());
            }
            if data_type == hcnetsdk::NET_DVR_STREAMDATA && s.queue.len() % 500 == 0 {
                log::info!("CALLBACK: type={}, buf_size={}, qlen={}", data_type, buf_size, s.queue.len());
            }
        }

        let user_data = Arc::into_raw(Arc::clone(&self.state)) as *mut std::ffi::c_void;
        let handle = self.sdk.realplay(self.user_id, channel, stream_type, data_callback, user_data)?;
        self.realplay_handle = Some(handle);
        log::info!("Preview started, handle={}", handle);
        Ok(())
    }

    pub fn stop(&mut self) -> Result<()> {
        if let Some(handle) = self.realplay_handle.take() {
            log::info!("Stopping preview, handle={}", handle);
            self.sdk.stop_realplay(handle)?;
        }
        Ok(())
    }

    fn flush_syshead(&self) -> Vec<u8> {
        let mut s = self.state.lock().unwrap();
        if s.syshead_sent { return Vec::new(); }
        s.syshead_sent = true;
        s.syshead.clone()
    }

    fn drain_all(&self) -> Vec<Vec<u8>> {
        let mut s = self.state.lock().unwrap();
        let mut chunks = Vec::new();
        while let Some(data) = s.queue.pop_front() {
            chunks.push(data);
        }
        chunks
    }

    fn check_error(&self) -> Option<String> {
        let s = self.state.lock().unwrap();
        s.error.clone()
    }

    pub fn device_info(&self) -> &NET_DVR_DEVICEINFO_V40 { &self.device_info }
    pub fn channel_count(&self) -> u8 { self.device_info.struDeviceV30.byChanNum }
    pub fn supports_channel_zero(&self) -> bool { self.device_info.struDeviceV30.byZeroChanNum > 0 }
}

impl Drop for HCNetSDKStream {
    fn drop(&mut self) {
        let _ = self.stop();
        if let Err(e) = self.sdk.logout(self.user_id) { log::warn!("Logout failed: {}", e); }
    }
}

fn yv12_to_rgba(yv12: &[u8], w: usize, h: usize) -> Vec<u8> {
    let y_size = w * h;
    let uv_size = y_size / 4;
    let mut rgba = Vec::with_capacity(w * h * 4);
    for row in 0..h {
        for col in 0..w {
            let y_idx = row * w + col;
            let uv_idx = y_size + (row / 2) * (w / 2) + col / 2;
            let v_idx = uv_idx;
            let u_idx = y_size + uv_size + (row / 2) * (w / 2) + col / 2;
            let y = yv12.get(y_idx).copied().unwrap_or(16) as f32;
            let u = yv12.get(u_idx).copied().unwrap_or(128) as f32 - 128.0;
            let v = yv12.get(v_idx).copied().unwrap_or(128) as f32 - 128.0;
            let r = (y + 1.402 * v).clamp(0.0, 255.0) as u8;
            let g = (y - 0.344 * u - 0.714 * v).clamp(0.0, 255.0) as u8;
            let b = (y + 1.772 * u).clamp(0.0, 255.0) as u8;
            rgba.extend_from_slice(&[r, g, b, 255]);
        }
    }
    rgba
}

unsafe extern "C" fn decode_callback(
    _n_port: std::ffi::c_int,
    p_buf: *mut c_char,
    n_size: std::ffi::c_int,
    p_frame_info: *mut FrameInfo,
    n_user: *mut std::ffi::c_void,
    _n_reserved2: std::ffi::c_int,
) {
    static CALLBACK_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let count = CALLBACK_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    
    if count < 5 {
        log::info!("decode_callback called: port={}, size={}, p_buf={:?}, p_frame_info={:?}, n_user={:?}", 
            _n_port, n_size, p_buf, p_frame_info, n_user);
    }
    
    if p_buf.is_null() || p_frame_info.is_null() || n_user.is_null() || n_size <= 0 {
        if count < 5 {
            log::warn!("decode_callback: early return - p_buf={}, p_frame_info={}, n_user={}, n_size={}", 
                p_buf.is_null(), p_frame_info.is_null(), n_user.is_null(), n_size);
        }
        return;
    }
    
    let frame_buf = &*(n_user as *const FrameBuffer);
    let info = &*p_frame_info;
    let w = info.n_width as usize;
    let h = info.n_height as usize;
    
    if count < 5 {
        log::info!("decode_callback: w={}, h={}, n_type={}, n_stamp={}, n_frame_rate={}", 
            w, h, info.n_type, info.n_stamp, info.n_frame_rate);
    }
    
    if w == 0 || h == 0 { return; }

    let data = std::slice::from_raw_parts(p_buf as *const u8, n_size as usize);
    let rgba = match info.n_type {
        T_RGB32 => {
            let mut out = Vec::with_capacity(w * h * 4);
            for pixel in data.chunks(4) {
                if pixel.len() == 4 {
                    out.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
                }
            }
            out
        }
        T_YV12 => yv12_to_rgba(data, w, h),
        _ => {
            if count < 5 {
                log::warn!("decode_callback: unsupported frame type {}", info.n_type);
            }
            return;
        }
    };

    if let Ok(mut guard) = frame_buf.lock() {
        *guard = Some(DecodedFrame { width: w as u32, height: h as u32, rgba });
    }
}

// ---- PlayM4 decoder wrapper (handles H.265+ natively) ----
struct PlayM4Decoder {
    playctrl: &'static PlayCtrl,
    port: i32,
    frame_count: u64,
    frame_buf: FrameBuffer,
    _frame_buf_raw: FrameBuffer,
    _decode_buf: Vec<i8>,
}

impl PlayM4Decoder {
    fn new(playctrl: &'static PlayCtrl, syshead: &[u8], buf_size: u32, frame_buf: FrameBuffer) -> Result<Self> {
        let port = playctrl.get_port()?;
        log::info!("PlayM4: allocated port {}", port);

        let modes_to_try: [u32; 5] = [0, 0x2, 0x401, 0x1, 0x4];
        for &mode in &modes_to_try {
            if let Err(e) = playctrl.set_stream_open_mode(port, mode) {
                log::debug!("PlayM4: SetStreamOpenMode({}) failed: {}", mode, e);
            } else {
                log::info!("PlayM4: SetStreamOpenMode({:#x}) OK", mode);
                break;
            }
        }

        playctrl.open_stream_with_header(port, syshead, buf_size)?;
        log::info!("PlayM4: stream opened ({} KB buffer, syshead={} bytes)", buf_size / 1024, syshead.len());

        let dest_size = 1920 * 1080 * 4;
        let dest_buf = vec![0i8; dest_size as usize];

        let raw_ptr = Arc::into_raw(Arc::clone(&frame_buf)) as *mut std::ffi::c_void;
        
        let mut used_ex = false;
        if let Err(e) = playctrl.set_dec_callback_ex_mend(port, decode_callback, dest_buf.as_ptr() as *mut c_char, dest_size as c_int, raw_ptr) {
            log::warn!("PlayM4: set_dec_callback_ex_mend failed: {}", e);
        } else {
            log::info!("PlayM4: decode callback (ExMend) registered");
            used_ex = true;
        }
        
        if !used_ex {
            if let Err(e) = playctrl.set_dec_callback_mend(port, decode_callback, raw_ptr) {
                let _ = unsafe { Arc::from_raw(raw_ptr as *const FrameBuffer) };
                log::warn!("PlayM4: set_dec_callback_mend failed: {}", e);
            } else {
                log::info!("PlayM4: decode callback (Mend) registered");
            }
        }

        playctrl.play(port)?;
        log::info!("PlayM4: playback started");

        Ok(Self {
            playctrl,
            port,
            frame_count: 0,
            frame_buf: frame_buf.clone(),
            _frame_buf_raw: frame_buf,
            _decode_buf: dest_buf,
        })
    }

    fn input_data(&self, data: &[u8]) -> Result<()> {
        self.playctrl.input_data(self.port, data)
    }

    fn try_get_frame(&mut self) -> Option<RtspFrame> {
        let frame = self.frame_buf.lock().ok()?.take()?;
        self.frame_count += 1;
        Some(RtspFrame {
            width: frame.width,
            height: frame.height,
            rgba: frame.rgba,
        })
    }
}

impl Drop for PlayM4Decoder {
    fn drop(&mut self) {
        let _ = self.playctrl.stop(self.port);
        let _ = self.playctrl.close_stream(self.port);
        let _ = self.playctrl.free_port(self.port);
        log::info!("PlayM4: port {} freed, {} frames decoded", self.port, self.frame_count);
    }
}

// ---- Stream loop ----
pub fn hcnetsdk_stream_loop(
    host: &str,
    port: u16,
    username: &str,
    password: &str,
    channel_id: &str,
    sdk_channel: i32,
    verification_code: &str,
    library_path: Option<&str>,
    tx: mpsc::SyncSender<RtspFrame>,
    stop: Arc<AtomicBool>,
    ctx: egui::Context,
) {
    log::info!("HCNetSDK stream loop starting for channel {} (sdk_channel={})", channel_id, sdk_channel);

    // Load PlayCtrl
    let playctrl = match get_cached_playctrl(library_path) {
        Ok(p) => p,
        Err(e) => {
            log::error!("Failed to load PlayCtrl: {}", e);
            return;
        }
    };

    let mut stream = match HCNetSDKStream::new(host, port, username, password, Some(verification_code)) {
        Ok(s) => s,
        Err(e) => { log::error!("Failed to create HCNetSDK stream: {}", e); return; }
    };

    if let Err(e) = stream.start(sdk_channel, true) {
        log::error!("Failed to start preview: {}", e); return;
    }

    log::info!("HCNetSDK preview started, waiting for SYSHEAD...");

    let frame_buf: FrameBuffer = Arc::new(Mutex::new(None));
    let mut playm4: Option<PlayM4Decoder> = None;
    let mut total_bytes = 0usize;
    let mut frame_count = 0u64;
    let mut last_diag = std::time::Instant::now();
    let mut diag_count = 0usize;

    while !stop.load(Ordering::Relaxed) {
        // Initialize PlayM4 decoder once we have SYSHEAD
        if playm4.is_none() {
            let syshead = stream.flush_syshead();
            if !syshead.is_empty() {
                log::info!("Got SYSHEAD ({} bytes), first 16: {:02x?}", syshead.len(), &syshead[..syshead.len().min(16)]);
                
                if syshead.len() >= 40 {
                    let fourcc = String::from_utf8_lossy(&syshead[0..4]);
                    let version = u16::from_le_bytes([syshead[4], syshead[5]]);
                    let device_id = u16::from_le_bytes([syshead[6], syshead[7]]);
                    let system_fmt = u16::from_le_bytes([syshead[8], syshead[9]]);
                    let video_fmt = u16::from_le_bytes([syshead[10], syshead[11]]);
                    log::info!("HIK_MEDIAINFO: fourcc='{}' version=0x{:04x} device_id={} system_format={} video_format={}",
                        fourcc, version, device_id, system_fmt, video_fmt);
                }
                
                match PlayM4Decoder::new(playctrl, &syshead, 2u32 * 1024 * 1024, frame_buf.clone()) {
                    Ok(d) => { playm4 = Some(d); }
                    Err(e) => { log::error!("Failed to init PlayM4 decoder: {}", e); break; }
                }
            }
        }

        if let Some(ref p) = playm4 {
            let chunks = stream.drain_all();
            let n_chunks = chunks.len();
            let n_bytes: usize = chunks.iter().map(|c| c.len()).sum();
            if n_chunks > 0 && diag_count < 5 {
                log::info!("Feeding {} chunks ({} bytes) to PlayM4", n_chunks, n_bytes);
                if let Some(first) = chunks.first() {
                    let first16 = &first[..first.len().min(16)];
                    log::info!("First chunk first 16 bytes: {:02x?}", first16);
                }
                diag_count += 1;
            }
            for chunk in &chunks {
                if let Err(e) = p.input_data(chunk) {
                    log::debug!("PlayM4: InputData error: {}", e);
                }
            }
            total_bytes += n_bytes;
        }

        // Extract frames from callback buffer
        if let Some(ref mut p) = playm4 {
            if let Some(frame) = p.try_get_frame() {
                frame_count += 1;
                let _ = tx.try_send(frame);
                ctx.request_repaint();
                if frame_count % 30 == 0 {
                    log::info!("PlayM4: {} frames decoded, {} bytes received", frame_count, total_bytes);
                }
            }
        }

        if last_diag.elapsed() > std::time::Duration::from_secs(1) {
            last_diag = std::time::Instant::now();
            if let Some(ref p) = playm4 {
                let played = p.playctrl.get_played_frames(p.port);
                let size = p.playctrl.get_picture_size(p.port);
                let buf_remain = p.playctrl.get_source_buffer_remain(p.port);
                let total_f = p.playctrl.get_total_frames(p.port);
                log::info!("PlayM4 diag: port={}, played_frames={}, picture_size={:?}, src_buf_rem={}, total_f={}, bytes={}, frames={}",
                    p.port, played, size, buf_remain, total_f, total_bytes, frame_count);
                if let Err(e) = p.playctrl.refresh_play(p.port) {
                    log::debug!("PlayM4: refresh_play: {}", e);
                }
            }
        }

        if let Some(err) = stream.check_error() {
            log::error!("Stream error: {}", err); break;
        }

        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    log::info!("HCNetSDK stream loop ending, decoded frames: {}, total bytes: {}", frame_count, total_bytes);
}
