//! Multi-câmera split-screen via HCNetSDK callback + PlayM4 decode.
//!
//! Cada canal usa `NET_DVR_RealPlay_V40` com callback (bBlocked=0, hPlayWnd=0).
//! O callback recebe H.264 já descriptografado (via NET_DVR_SetSDKSecretKey).
//! Os dados MPEG-PS são alimentados diretamente no PlayM4, que lida com
//! data partitioning nativamente. PlayM4 → JPEG → RGBA → egui.

use crate::hcnetsdk::{self, HCNetSDK, NET_DVR_DEVICEINFO_V40, LINK_RTSP};
use crate::playctrl;
use crate::playctrl::PlayCtrl;
use crate::rtsp::RtspFrame;
use anyhow::{Context, Result};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// PlayCtrl global cache (shared across channels)
// ---------------------------------------------------------------------------

static ONCE_PLAYCTRL: OnceLock<PlayCtrl> = OnceLock::new();

fn get_cached_playctrl(library_path: Option<&str>) -> Result<&'static PlayCtrl> {
    if let Some(pc) = ONCE_PLAYCTRL.get() {
        return Ok(pc);
    }
    let pc = if let Some(path) = library_path {
        PlayCtrl::load_from(std::path::Path::new(path))?
    } else {
        playctrl::search_and_load()?
    };
    let _ = ONCE_PLAYCTRL.set(pc);
    Ok(ONCE_PLAYCTRL.get().unwrap())
}

// ---------------------------------------------------------------------------
// Global callback dispatch: extern "C" fn → per-channel data_tx
// ---------------------------------------------------------------------------

static CALLBACK_DISPATCH: OnceLock<Mutex<Vec<Option<CallbackSlot>>>> = OnceLock::new();

fn dispatch() -> &'static Mutex<Vec<Option<CallbackSlot>>> {
    CALLBACK_DISPATCH.get_or_init(|| Mutex::new(Vec::new()))
}

static CALLBACK_LOGGED: OnceLock<AtomicBool> = OnceLock::new();

struct CallbackSlot {
    data_tx: mpsc::SyncSender<(u32, Vec<u8>)>,
}

extern "C" fn data_callback(
    _handle: i32,
    data_type: u32,
    data: *mut u8,
    size: u32,
    user: *mut c_void,
) {
    let slot = user as usize;
    let guard = dispatch().lock().unwrap();
    if let Some(Some(ctx)) = guard.get(slot) {
        let buf = unsafe { std::slice::from_raw_parts(data, size as usize) };
        let logged = CALLBACK_LOGGED.get_or_init(|| AtomicBool::new(false));
        if !logged.swap(true, Ordering::Relaxed) {
            log::info!(
                "[slot{}] data_type={}, size={}, first_16={:02x?}",
                slot, data_type, size,
                &buf[..buf.len().min(16)],
            );
        }
        let _ = ctx.data_tx.try_send((data_type, buf.to_vec()));
    }
}

// ---------------------------------------------------------------------------
// PS → H.264 Annex B extractor
// ---------------------------------------------------------------------------

/// Extract raw H.264 Annex B from MPEG-PS data.
///
/// Scans for video PES packets (stream_id 0xE0), strips PES headers, and
/// concatenates the raw H.264 NAL units (already in Annex B format with
/// `00 00 01` start codes) into a single output buffer.
fn extract_h264_from_ps(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 4 {
        return None;
    }
    let len = data.len();
    let mut out = Vec::new();
    let mut i = 0;

    while i + 3 < len {
        if data[i] != 0x00 || data[i + 1] != 0x00 || data[i + 2] != 0x01 {
            i += 1;
            continue;
        }
        let stype = data[i + 3];

        if stype == 0xE0 {
            // Video PES — find end (next PS start code: byte after 00 00 01 >= 0xBA)
            let mut end = i + 4;
            while end + 3 < len {
                if data[end] == 0x00 && data[end + 1] == 0x00 && data[end + 2] == 0x01 && data[end + 3] >= 0xBA
                {
                    break;
                }
                end += 1;
            }
            let pes = &data[i + 4..end];
            if pes.len() > 9 {
                let hdr_len = 5 + pes[4] as usize;
                if pes.len() > hdr_len {
                    out.extend_from_slice(&pes[hdr_len..]);
                }
            }
            i = end;
        } else if stype >= 0xBA {
            // Non-video PS packet — skip to next PS start code
            let mut end = i + 4;
            while end + 3 < len {
                if data[end] == 0x00 && data[end + 1] == 0x00 && data[end + 2] == 0x01 && data[end + 3] >= 0xBA
                {
                    break;
                }
                end += 1;
            }
            i = end;
        } else {
            // Inside PES payload (NAL start code) — shouldn't reach here at top level
            i += 4;
        }
    }

    if out.is_empty() { None } else { Some(out) }
}

// ---------------------------------------------------------------------------
// JPEG → RGBA helper
// ---------------------------------------------------------------------------

fn jpeg_to_rgba(jpeg_data: &[u8]) -> Result<RtspFrame> {
    let mut decoder = jpeg_decoder::Decoder::new(std::io::Cursor::new(jpeg_data));
    decoder.read_info().context("JPEG read info failed")?;
    let info = decoder.info().ok_or_else(|| anyhow::anyhow!("JPEG no info"))?;
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
    Ok(RtspFrame { width: w as u32, height: h as u32, rgba })
}

// ---------------------------------------------------------------------------
// Decoder thread: PS data → PlayM4 → JPEG → RGBA → mpsc
// ---------------------------------------------------------------------------

fn decoder_loop(
    data_rx: mpsc::Receiver<(u32, Vec<u8>)>,
    frame_tx: SyncSender<RtspFrame>,
    stop: Arc<AtomicBool>,
    slot: usize,
    channel_id: i32,
    playctrl: &'static PlayCtrl,
) {
    let port = match playctrl.get_port() {
        Ok(p) => p,
        Err(e) => {
            log::error!("[ch{}] PlayM4_GetPort failed: {}", channel_id, e);
            return;
        }
    };
    log::info!("[ch{}] allocated PlayM4 port {}", channel_id, port);
    log::info!("[ch{}] DISPLAY={:?}", channel_id, std::env::var("DISPLAY"));

    let mut syshead = Vec::new();
    let mut stream_opened = false;
    let mut _input_bytes = 0u64;
    let mut _h264_bytes = 0u64;

    let mut frame_count = 0u64;
    let start = Instant::now();
    let mut last_poll = Instant::now();
    let poll_interval = Duration::from_millis(100);

    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }

        loop {
            match data_rx.try_recv() {
                Ok((1, data)) => {
                    // SYSHEAD — strip IMKH prefix if present, use as codec header
                    let start = if data.len() > 4 && data[0..4] == [0x49, 0x4D, 0x4B, 0x48] {
                        4
                    } else {
                        0
                    };
                    syshead = data[start..].to_vec();
                }
                Ok((2, data)) => {
                    // STREAMDATA — open stream on first receipt, then feed
                    if !stream_opened {
                        // Set real-time streaming mode (PlayM4 auto-detects format)
                        let _ = playctrl.set_stream_open_mode(port, 1);

                        let header: &[u8] = if !syshead.is_empty() { &syshead } else { &[] };
                        if let Err(e) = playctrl.open_stream_with_header(port, header, 4 * 1024 * 1024) {
                            log::error!("[ch{}] PlayM4_OpenStream failed: {}", channel_id, e);
                            let _ = playctrl.free_port(port);
                            return;
                        }
                        log::info!("[ch{}] PlayM4 stream opened with {} bytes header", channel_id, header.len());
                        stream_opened = true;
                    }
                    // Strip PS container → raw H.264 Annex B
                    let h264 = extract_h264_from_ps(&data);
                    let Some(h264) = h264 else {
                        if frame_count < 5 {
                            log::warn!("[ch{}] PS parse returned no H.264 (size={})", channel_id, data.len());
                        }
                        continue;
                    };
                    _input_bytes += data.len() as u64;
                    _h264_bytes += h264.len() as u64;
                    if frame_count == 0 && !h264.is_empty() {
                        log::info!(
                            "[ch{}] first H.264 buffer: {} bytes, first_16={:02x?}",
                            channel_id, h264.len(), &h264[..h264.len().min(16)],
                        );
                    }
                    if let Err(e) = playctrl.input_data(port, &h264) {
                        if frame_count < 5 {
                            log::warn!("[ch{}] InputData error: {}", channel_id, e);
                        }
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => break,
                Err(mpsc::TryRecvError::Empty) => break,
                _ => {} // unknown data_type
            }
        }

        if stream_opened && last_poll.elapsed() >= poll_interval {
            last_poll = Instant::now();
            match playctrl.get_jpeg(port) {
                Ok(jpeg) if !jpeg.is_empty() => {
                    match jpeg_to_rgba(&jpeg) {
                        Ok(frame) => {
                            let (fw, fh) = (frame.width, frame.height);
                            let _ = frame_tx.try_send(frame);
                            if frame_count == 0 {
                                log::info!(
                                    "[ch{}] first decoded frame: {}x{}",
                                    channel_id, fw, fh,
                                );
                            }
                            frame_count += 1;
                            if frame_count % 60 == 0 {
                                let el = start.elapsed().as_secs_f64();
                                log::info!(
                                    "[ch{}] {:.1} fps, {} frames",
                                    channel_id,
                                    frame_count as f64 / el.max(0.001),
                                    frame_count,
                                );
                            }
                        }
                        Err(e) => {
                            if frame_count < 5 {
                                log::warn!("[ch{}] JPEG decode error: {}", channel_id, e);
                            }
                        }
                    }
                }
                Ok(_) => {
                    if frame_count == 0 {
                        log::info!("[ch{}] GetJPEG returned empty (no frame yet)", channel_id);
                    }
                }
                Err(e) => {
                    let err_code = playctrl.get_last_error(port);
                    if frame_count < 5 || frame_count % 100 == 0 {
                        log::warn!(
                            "[ch{}] GetJPEG error (err={}): {}",
                            channel_id, err_code, e,
                        );
                    }
                }
            }
        }

        std::thread::sleep(Duration::from_millis(5));
    }

    if stream_opened {
        let _ = playctrl.close_stream(port);
    }
    let _ = playctrl.free_port(port);

    {
        let mut guard = dispatch().lock().unwrap();
        if slot < guard.len() {
            guard[slot] = None;
        }
    }

    let elapsed = start.elapsed().as_secs_f64();
    log::info!(
        "[ch{}] decoder ended. {} frames, {:.1}s",
        channel_id, frame_count, elapsed,
    );
}

// ---------------------------------------------------------------------------
// HCNetSDKMultiStream — login + N preview channels
// ---------------------------------------------------------------------------

pub struct HCNetSDKMultiStream {
    sdk: Arc<HCNetSDK>,
    playctrl: &'static PlayCtrl,
    user_id: i32,
    device_info: NET_DVR_DEVICEINFO_V40,
    channels: Vec<ChannelState>,
}

struct ChannelState {
    sdk_channel: i32,
    realplay_handle: i32,
    _slot: usize,
    _data_tx: mpsc::SyncSender<(u32, Vec<u8>)>,
    decoder_stop: Arc<AtomicBool>,
    decoder_thread: Option<std::thread::JoinHandle<()>>,
}

impl HCNetSDKMultiStream {
    pub fn new(
        host: &str,
        sdk_port: u16,
        username: &str,
        password: &str,
        verification_code: Option<&str>,
        library_path: Option<&str>,
    ) -> Result<Self> {
        let playctrl = get_cached_playctrl(library_path)
            .context("PlayCtrl not available for HCNetSDK multi-stream")?;

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
            "HCNetSDKMultiStream logged in. channels={}, zero={}",
            device_info.struDeviceV30.byChanNum,
            device_info.struDeviceV30.byZeroChanNum,
        );

        Ok(Self {
            sdk: sdk.clone(),
            playctrl,
            user_id,
            device_info,
            channels: Vec::new(),
        })
    }

    pub fn start_channel(
        &mut self,
        sdk_channel: i32,
        main_stream: bool,
    ) -> Result<mpsc::Receiver<RtspFrame>> {
        let stream_type = if main_stream {
            hcnetsdk::STREAM_MAIN
        } else {
            hcnetsdk::STREAM_SUB
        };

        let (data_tx, data_rx) = mpsc::sync_channel::<(u32, Vec<u8>)>(64);
        let slot = {
            let mut guard = dispatch().lock().unwrap();
            let slot = guard.len();
            guard.push(Some(CallbackSlot {
                data_tx: data_tx.clone(),
            }));
            slot
        };

        let (frame_tx, frame_rx) = mpsc::sync_channel::<RtspFrame>(2);

        let stop = Arc::new(AtomicBool::new(false));
        let thread = std::thread::Builder::new()
            .name(format!("hcsdk-{}", sdk_channel))
            .spawn({
                let stop = stop.clone();
                let pc = self.playctrl;
                move || decoder_loop(data_rx, frame_tx, stop, slot, sdk_channel, pc)
            })
            .context("failed to spawn decoder thread")?;

        let handle = self.sdk.realplay_with_callback(
            self.user_id,
            sdk_channel,
            stream_type,
            LINK_RTSP,
            data_callback,
            slot as *mut c_void,
        )?;

        self.channels.push(ChannelState {
            sdk_channel,
            realplay_handle: handle,
            _slot: slot,
            _data_tx: data_tx,
            decoder_stop: stop,
            decoder_thread: Some(thread),
        });

        log::info!(
            "Channel {} started (handle={}, slot={}, active={})",
            sdk_channel,
            handle,
            slot,
            self.channels.len(),
        );

        Ok(frame_rx)
    }

    pub fn stop_channel(&mut self, sdk_channel: i32) -> bool {
        let pos = self.channels.iter().position(|c| c.sdk_channel == sdk_channel);
        let Some(idx) = pos else {
            log::warn!("stop_channel {} not found", sdk_channel);
            return false;
        };
        let ch = &self.channels[idx];
        ch.decoder_stop.store(true, Ordering::Relaxed);
        let _ = self.sdk.stop_realplay(ch.realplay_handle);
        if let Some(ch_state) = self.channels.get_mut(idx) {
            if let Some(thread) = ch_state.decoder_thread.take() {
                let _ = thread.join();
            }
        }
        self.channels.remove(idx);
        log::info!("Channel {} stopped", sdk_channel);
        true
    }

    pub fn stop_all(&mut self) {
        let count = self.channels.len();
        let handles: Vec<(i32, Arc<AtomicBool>)> = self
            .channels
            .iter()
            .map(|c| (c.realplay_handle, c.decoder_stop.clone()))
            .collect();
        for (handle, stop) in &handles {
            stop.store(true, Ordering::Relaxed);
            let _ = self.sdk.stop_realplay(*handle);
        }
        for ch in &mut self.channels {
            if let Some(thread) = ch.decoder_thread.take() {
                let _ = thread.join();
            }
        }
        self.channels.clear();
        log::info!("All {} channels stopped", count);
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

    pub fn active_channel_count(&self) -> usize {
        self.channels.len()
    }
}

impl Drop for HCNetSDKMultiStream {
    fn drop(&mut self) {
        self.stop_all();
        if let Err(e) = self.sdk.logout(self.user_id) {
            log::warn!("Logout failed: {}", e);
        }
    }
}
