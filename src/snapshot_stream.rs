use anyhow::{Context, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::api::HikvisionAPI;
use crate::rtsp::RtspFrame;

pub fn snapshot_stream_loop(
    channel: &str,
    host: &str,
    port: u16,
    user: &str,
    password: &str,
    tx: SyncSender<RtspFrame>,
    stop: Arc<AtomicBool>,
    repaint: egui::Context,
    interval_ms: u64,
) {
    let api = HikvisionAPI::new(host, port, user, password);
    let mut frame_count = 0u64;
    let mut fps_timer = Instant::now();

    loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        match poll_snapshot(&api, channel) {
            Ok(frame) => {
                frame_count += 1;
                let elapsed = fps_timer.elapsed();
                if elapsed >= Duration::from_secs(1) {
                    let fps = frame_count as f64 / elapsed.as_secs_f64();
                    log::debug!("Snapshot stream: {}x{}, {:.1} fps", frame.width, frame.height, fps);
                    frame_count = 0;
                    fps_timer = Instant::now();
                }
                let _ = tx.try_send(frame);
                repaint.request_repaint();
            }
            Err(e) => {
                log::warn!("Snapshot poll error: {}", e);
            }
        }
        if interval_ms > 0 {
            std::thread::sleep(Duration::from_millis(interval_ms));
        }
    }
}

fn poll_snapshot(api: &HikvisionAPI, channel: &str) -> Result<RtspFrame> {
    let jpeg_data = api.snapshot(channel).context("snapshot request failed")?;

    let mut decoder = jpeg_decoder::Decoder::new(std::io::Cursor::new(&jpeg_data));
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
