use hikvision_rs::api::{Channel, HikvisionAPI};
use hikvision_rs::rtsp;
use eframe::egui;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::Instant;

struct HikvisionApp {
    // Login fields
    host: String,
    port: String,
    rtsp_port: String,
    user: String,
    password: String,
    use_substream: bool,

    // Connection state
    api: Option<HikvisionAPI>,
    channels: Vec<Channel>,
    device_name: String,
    error: Option<String>,

    // Viewer state
    selected: Option<usize>,
    texture: Option<egui::TextureHandle>,
    frame_width: usize,
    frame_height: usize,
    frame_rx: Option<mpsc::Receiver<rtsp::RtspFrame>>,
    stream_stop: Option<Arc<AtomicBool>>,
    stream_handle: Option<thread::JoinHandle<()>>,

    // FPS counter
    frame_count: u64,
    fps_timer: Instant,
    fps: f32,
}

impl Default for HikvisionApp {
    fn default() -> Self {
        Self {
            host: "192.168.5.75".into(),
            port: "80".into(),
            rtsp_port: "554".into(),
            user: "admin".into(),
            password: String::new(),
            use_substream: false,
            api: None,
            channels: Vec::new(),
            device_name: String::new(),
            error: None,
            selected: None,
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

        let api = HikvisionAPI::new(&host, port, &user, &password);
        match api.device_info() {
            Ok(info) => {
                self.device_name =
                    format!("{} | {} | {}", info.name, info.model, info.firmware);
                match api.channels() {
                    Ok(chs) => {
                        self.channels = chs;
                        self.api = Some(api);
                        self.error = None;
                    }
                    Err(e) => self.error = Some(format!("Channels failed: {}", e)),
                }
            }
            Err(e) => self.error = Some(format!("Connection failed: {}", e)),
        }
    }

    fn stop_stream(&mut self) {
        if let Some(stop) = self.stream_stop.take() {
            stop.store(true, Ordering::Relaxed);
        }
        if let Some(handle) = self.stream_handle.take() {
            let _ = handle.join();
        }
        self.frame_rx = None;
        self.texture = None;
        self.frame_width = 0;
        self.frame_height = 0;
    }

    /// Build the RTSP URL for a given channel ID.
    ///
    /// Hikvision ISAPI channel IDs: 101 (ch1 main), 102 (ch1 sub), 201 (ch2 main), etc.
    /// For RTSP we use the same ID directly in the path.
    /// If `use_substream` is true, we switch the last digit from 1→2.
    fn rtsp_url(&self, channel_id: &str) -> String {
        let cid = if self.use_substream {
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

        // Safe url-encoding for credentials (handles special chars like / ? # @ :)
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

    fn start_stream(&mut self, channel_id: &str, egui_ctx: &egui::Context) {
        self.stop_stream();

        let url = self.rtsp_url(channel_id);
        log::info!("Starting RTSP stream for channel {}", channel_id);

        // Bounded channel with capacity 2: enough to pipeline, drops old frames under load
        let (tx, rx) = mpsc::sync_channel::<rtsp::RtspFrame>(2);
        self.frame_rx = Some(rx);

        let stop = Arc::new(AtomicBool::new(false));
        self.stream_stop = Some(stop.clone());

        let repaint_ctx = egui_ctx.clone();
        let handle = thread::spawn(move || {
            rtsp::stream_loop(&url, tx, stop, repaint_ctx);
        });
        self.stream_handle = Some(handle);

        // Reset FPS counter
        self.frame_count = 0;
        self.fps_timer = Instant::now();
        self.fps = 0.0;
    }

    fn show_login(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(100.0);
                ui.heading("Hikvision DVR Viewer");
                ui.label("RTSP Streaming");
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
                        ui.label("HTTP Port:");
                        ui.add_sized([field_w, field_h], egui::TextEdit::singleline(&mut self.port));
                        ui.end_row();
                        ui.label("RTSP Port:");
                        ui.add_sized([field_w, field_h], egui::TextEdit::singleline(&mut self.rtsp_port));
                        ui.end_row();
                        ui.label("Username:");
                        ui.add_sized([field_w, field_h], egui::TextEdit::singleline(&mut self.user));
                        ui.end_row();
                        ui.label("Password:");
                        ui.add_sized([field_w, field_h], egui::TextEdit::singleline(&mut self.password).password(true));
                        ui.end_row();
                        ui.label("");
                        ui.checkbox(&mut self.use_substream, "Sub-stream (menor resolução, mais leve)");
                        ui.end_row();
                    });

                ui.add_space(10.0);
                if ui.button("Connect").clicked() {
                    self.connect();
                }

                ui.add_space(10.0);
                ui.label(egui::RichText::new("⚠️ Atenção: Se a 'Criptografia de Transmissão' (Verification Code) estiver ativada no DVR, o vídeo não carregará. Desative-a no menu de Rede do DVR.").small().color(egui::Color32::DARK_GRAY));

                if let Some(ref err) = self.error {
                    ui.colored_label(egui::Color32::RED, err);
                }
            });
        });
    }

    fn show_viewer(&mut self, ctx: &egui::Context) {
        // Drain all pending frames, keep only the latest
        if let Some(rx) = &self.frame_rx {
            while let Ok(frame) = rx.try_recv() {
                let w = frame.width as usize;
                let h = frame.height as usize;

                let color_image = egui::ColorImage::from_rgba_unmultiplied([w, h], &frame.rgba);

                // Reuse existing texture if possible, otherwise create new
                if let Some(ref mut tex) = self.texture {
                    tex.set(color_image, egui::TextureOptions::LINEAR);
                } else {
                    self.texture = Some(ctx.load_texture(
                        "rtsp_frame",
                        color_image,
                        egui::TextureOptions::LINEAR,
                    ));
                }

                self.frame_width = w;
                self.frame_height = h;
                self.frame_count += 1;
            }
        }

        // FPS calculation
        let elapsed = self.fps_timer.elapsed();
        if elapsed >= std::time::Duration::from_secs(1) {
            self.fps = self.frame_count as f32 / elapsed.as_secs_f32();
            self.frame_count = 0;
            self.fps_timer = Instant::now();
        }

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(&self.device_name);
                if self.selected.is_some() {
                    ui.separator();
                    ui.label(format!(
                        "{}x{} | {:.1} fps | RTSP/H.265+",
                        self.frame_width, self.frame_height, self.fps
                    ));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Disconnect").clicked() {
                        self.stop_stream();
                        self.api = None;
                        self.channels.clear();
                        self.selected = None;
                    }
                });
            });
        });

        egui::SidePanel::left("channels")
            .resizable(false)
            .default_width(200.0)
            .show(ctx, |ui| {
                ui.heading("Channels");
                ui.separator();
                ui.checkbox(&mut self.use_substream, "Sub-stream");
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    let channels = self.channels.clone();
                    for (i, ch) in channels.iter().enumerate() {
                        let selected = self.selected == Some(i);
                        let label = format!("[{}] {}", ch.id, ch.name);
                        if ui.selectable_label(selected, &label).clicked() {
                            self.selected = Some(i);
                            self.start_stream(&ch.id, ctx);
                        }
                    }
                });
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(ref tex) = self.texture {
                let size = ui.available_size();
                if self.frame_width > 0 && self.frame_height > 0 && size.x > 0.0 && size.y > 0.0 {
                    let img_aspect = self.frame_width as f32 / self.frame_height as f32;
                    let area_aspect = size.x / size.y;
                    let scaled = if img_aspect > area_aspect {
                        egui::Vec2::new(size.x, size.x / img_aspect)
                    } else {
                        egui::Vec2::new(size.y * img_aspect, size.y)
                    };
                    ui.image(egui::load::SizedTexture::new(tex.id(), scaled));
                }
            } else {
                ui.vertical_centered(|ui| {
                    ui.add_space(100.0);
                    if self.selected.is_some() {
                        ui.spinner();
                        ui.label("Connecting to RTSP stream...");
                    } else {
                        ui.label("Select a channel to view");
                    }
                });
            }
        });
    }
}

impl eframe::App for HikvisionApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.api.is_some() && !self.channels.is_empty() {
            self.show_viewer(ctx);
        } else {
            self.show_login(ctx);
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.stop_stream();
    }
}

fn main() {
    env_logger::init();

    // Initialize FFmpeg once at startup
    ffmpeg_next::init().expect("Failed to initialize FFmpeg");
    // Enable FFmpeg logging at warning level
    ffmpeg_next::log::set_level(ffmpeg_next::log::Level::Warning);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1280.0, 720.0]),
        ..Default::default()
    };
    let _ = eframe::run_native(
        "Hikvision DVR Viewer",
        options,
        Box::new(|_cc| Ok(Box::new(HikvisionApp::default()))),
    );
}
