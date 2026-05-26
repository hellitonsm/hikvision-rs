use anyhow::{Context, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::Arc;

/// A decoded video frame ready for display.
pub struct RtspFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Run the RTSP streaming loop with automatic reconnection.
///
/// Connects to the given RTSP URL, decodes H.264/H.265 video frames,
/// converts them to RGBA, and sends them through the channel.
/// On stream errors, retries after a brief delay until `stop` is set.
pub fn stream_loop(
    url: &str,
    tx: SyncSender<RtspFrame>,
    stop: Arc<AtomicBool>,
    repaint: egui::Context,
) {
    loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        match run_stream(url, &tx, &stop, &repaint) {
            Ok(()) => return,
            Err(e) => {
                log::error!("RTSP stream error: {}, reconnecting in 2s...", e);
                // Wait before retrying, but check stop flag frequently
                for _ in 0..20 {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
        }
    }
}

fn run_stream(
    url: &str,
    tx: &SyncSender<RtspFrame>,
    stop: &Arc<AtomicBool>,
    repaint: &egui::Context,
) -> Result<()> {
    log::info!("Opening RTSP stream: {}", mask_password(url));

    let mut opts = ffmpeg_next::Dictionary::new();
    // Allow FFmpeg to auto-negotiate transport (UDP first, then TCP)
    // Forcing TCP on Hikvision H.265+ sometimes corrupts large I-frames
    // opts.set("rtsp_transport", "tcp"); 

    // Connection timeout: 5 seconds
    opts.set("stimeout", "5000000");
    
    // For H.265+ we need a massive probe size because the I-frame interval (GOP)
    // is very long, and SPS/PPS/VPS NAL units might take a long time to arrive.
    // Also, we CANNOT use "nobuffer" here, otherwise avformat_find_stream_info 
    // will fail to buffer enough frames to assemble the HEVC parameters.
    opts.set("analyzeduration", "10000000"); // 10 seconds
    opts.set("probesize", "10000000"); // 10 MB

    let mut ictx = ffmpeg_next::format::input_with_dictionary(url, opts)
        .context("Failed to open RTSP stream")?;

    let video_stream = ictx
        .streams()
        .best(ffmpeg_next::media::Type::Video)
        .ok_or_else(|| anyhow::anyhow!("no video stream found"))?;

    let video_idx = video_stream.index();
    let codec_params = video_stream.parameters();

    log::info!(
        "Video stream #{}: codec {:?}",
        video_idx,
        codec_params.id()
    );

    let codec_ctx = ffmpeg_next::codec::context::Context::from_parameters(codec_params)?;
    let mut decoder = codec_ctx.decoder().video()?;

    // Enable multithreaded decoding for H.265
    unsafe {
        (*decoder.as_mut_ptr()).thread_count = 2;
    }

    let mut scaler: Option<ffmpeg_next::software::scaling::Context> = None;
    let mut decoded_frame = ffmpeg_next::frame::Video::empty();
    let mut rgba_frame = ffmpeg_next::frame::Video::empty();

    for (stream, packet) in ictx.packets() {
        if stop.load(Ordering::Relaxed) {
            break;
        }

        if stream.index() != video_idx {
            continue;
        }

        decoder.send_packet(&packet)?;

        while decoder.receive_frame(&mut decoded_frame).is_ok() {
            // Create scaler lazily on first decoded frame
            let sws = match scaler.as_mut() {
                Some(s) => s,
                None => {
                    log::info!(
                        "First frame decoded: {}x{} {:?}",
                        decoded_frame.width(),
                        decoded_frame.height(),
                        decoded_frame.format()
                    );
                    scaler = Some(
                        ffmpeg_next::software::scaling::Context::get(
                            decoded_frame.format(),
                            decoded_frame.width(),
                            decoded_frame.height(),
                            ffmpeg_next::format::Pixel::RGBA,
                            decoded_frame.width(),
                            decoded_frame.height(),
                            ffmpeg_next::software::scaling::Flags::BILINEAR,
                        )
                        .context("failed to create pixel format converter")?,
                    );
                    scaler.as_mut().unwrap()
                }
            };

            sws.run(&decoded_frame, &mut rgba_frame)?;

            let w = rgba_frame.width();
            let h = rgba_frame.height();
            let stride = rgba_frame.stride(0);
            let data = rgba_frame.data(0);
            let row_bytes = w as usize * 4;

            // Copy pixel data, handling stride padding if present
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

            let frame = RtspFrame {
                width: w,
                height: h,
                rgba,
            };

            // try_send: drop frame if UI can't keep up (backpressure)
            let _ = tx.try_send(frame);
            repaint.request_repaint();
        }
    }

    Ok(())
}

/// Mask the password in RTSP URLs for logging.
fn mask_password(url: &str) -> String {
    // rtsp://user:PASSWORD@host:port/path
    if let Some(at_pos) = url.find('@') {
        if let Some(colon_pos) = url[..at_pos].rfind(':') {
            let prefix = &url[..colon_pos + 1];
            let suffix = &url[at_pos..];
            return format!("{}****{}", prefix, suffix);
        }
    }
    url.to_string()
}
