use hikvision_rs::api::{Channel, HikvisionAPI};
use hikvision_rs::rtsp;
use eframe::egui;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::Instant;

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
    rtsp_port: String,
    user: String,
    password: String,
    use_substream: bool,

    api: Option<HikvisionAPI>,
    channels: Vec<Channel>,
    device_name: String,
    error: Option<String>,

    layout_mode: LayoutMode,
    prev_layout: LayoutMode,
    streams: Vec<StreamState>,
    focused_channel: Option<usize>,
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
            layout_mode: LayoutMode::Single,
            prev_layout: LayoutMode::Single,
            streams: Vec::new(),
            focused_channel: None,
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
                        self.streams = (0..self.channels.len())
                            .map(|_| StreamState::new())
                            .collect();
                        self.focused_channel = if self.channels.is_empty() {
                            None
                        } else {
                            Some(0)
                        };
                        self.api = Some(api);
                        self.error = None;
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

    fn start_stream(&mut self, channel_index: usize, ctx: &egui::Context) {
        if channel_index >= self.channels.len() {
            return;
        }
        if self.streams[channel_index].stream_handle.is_some() {
            return;
        }

        let channel_id = self.channels[channel_index].id.clone();
        let channel_name = self.channels[channel_index].name.clone();
        let force_sub = matches!(
            self.layout_mode,
            LayoutMode::Grid2x2 | LayoutMode::Grid3x3 | LayoutMode::Grid4x4
        );
        let url = self.rtsp_url(&channel_id, force_sub);
        log::info!("Starting stream for channel {}: {}", channel_id, channel_name);

        let (tx, rx) = mpsc::sync_channel::<rtsp::RtspFrame>(2);
        let stop = Arc::new(AtomicBool::new(false));

        let state = &mut self.streams[channel_index];
        state.frame_rx = Some(rx);
        state.stream_stop = Some(stop.clone());
        state.frame_count = 0;
        state.fps_timer = Instant::now();
        state.fps = 0.0;

        let repaint_ctx = ctx.clone();
        let handle = thread::spawn(move || {
            rtsp::stream_loop(&url, tx, stop, repaint_ctx);
        });
        state.stream_handle = Some(handle);
    }

    fn stop_stream(&mut self, channel_index: usize) {
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
        for i in 0..self.streams.len() {
            self.stop_stream(i);
        }
    }

    fn drain_frames(&mut self, ctx: &egui::Context) {
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
        if self.layout_mode != self.prev_layout {
            self.prev_layout = self.layout_mode;
            for i in 0..self.streams.len() {
                if self.streams[i].stream_handle.is_some() {
                    self.stop_stream(i);
                    self.start_stream(i, ctx);
                }
            }
        }

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(&self.device_name);
                if self.streams.iter().any(|s| s.stream_handle.is_some()) {
                    ui.separator();
                    let active = self.streams.iter().filter(|s| s.stream_handle.is_some()).count();
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
                                    if self.streams[i].stream_handle.is_none() {
                                        self.start_stream(i, ctx);
                                    }
                                }
                            }
                        }
                        _ => {
                            for (i, ch) in channels.iter().enumerate() {
                                let is_on = self.streams[i].stream_handle.is_some();
                                let mut checked = is_on;
                                let label = format!("[{}] {}", ch.id, ch.name);
                                if ui.checkbox(&mut checked, &label).changed() {
                                    if checked {
                                        self.start_stream(i, ctx);
                                    } else {
                                        self.stop_stream(i);
                                    }
                                }
                            }
                            ui.separator();
                            if ui.button("Start All").clicked() {
                                let cap = self.layout_mode.capacity();
                                for i in 0..self.streams.len().min(cap) {
                                    if self.streams[i].stream_handle.is_none() {
                                        self.start_stream(i, ctx);
                                    }
                                }
                            }
                            if ui.button("Stop All").clicked() {
                                self.stop_all_streams();
                            }
                        }
                    }
                });
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            match self.layout_mode {
                LayoutMode::Single => self.show_single_view(ui),
                _ => self.show_multi_view(ui),
            }
        });
    }

    fn show_single_view(&mut self, ui: &mut egui::Ui) {
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
                if self.streams[idx].stream_handle.is_some() {
                    ui.spinner();
                    ui.label("Connecting to RTSP stream...");
                } else {
                    ui.label("Select a channel to view");
                }
            });
        }
    }

    fn show_multi_view(&mut self, ui: &mut egui::Ui) {
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

        for row in 0..self.layout_mode.rows() {
            ui.horizontal(|ui| {
                for col in 0..self.layout_mode.cols() {
                    let (rect, _response) = ui.allocate_exact_size(cell_size, egui::Sense::click());
                    let mut cell_ui = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(rect)
                            .layout(egui::Layout::top_down(egui::Align::Center)),
                    );
                    self.render_cell(&mut cell_ui, row * self.layout_mode.cols() + col, cell_size);
                }
            });
        }
    }

    fn render_cell(&self, ui: &mut egui::Ui, idx: usize, cell_size: egui::Vec2) {
        // Dark background
        ui.painter().rect_filled(
            ui.max_rect(),
            0.0,
            egui::Color32::from_rgb(10, 10, 10),
        );

        if idx >= self.streams.len() {
            let label = if idx < self.channels.len() {
                format!("[{}] {}", self.channels[idx].id, self.channels[idx].name)
            } else {
                "No Signal".to_string()
            };
            ui.add_space(cell_size.y * 0.4);
            ui.colored_label(egui::Color32::DARK_GRAY, label);
            return;
        }

        let state = &self.streams[idx];
        let channel_name = if idx < self.channels.len() {
            format!("[{}]", self.channels[idx].id)
        } else {
            format!("Ch {}", idx + 1)
        };

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
            if state.stream_handle.is_some() {
                ui.colored_label(egui::Color32::DARK_GRAY, "Connecting...");
            } else {
                ui.colored_label(egui::Color32::DARK_GRAY, "No Signal");
            }
        }
    }
}

impl eframe::App for HikvisionApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.api.is_some() && !self.channels.is_empty() {
            self.drain_frames(ctx);
            ctx.request_repaint();
            self.show_viewer(ctx);
        } else {
            self.show_login(ctx);
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.stop_all_streams();
    }
}

fn main() {
    env_logger::init();

    ffmpeg_next::init().expect("Failed to initialize FFmpeg");
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
