//! Janela X11 nativa para preview direto do SDK Hikvision.
//!
//! Baseado no x11wnd.rs do rustdemo. O SDK Hikvision renderiza vídeo
//! diretamente na janela X11 via overlay — sem necessidade de decodificação
//! manual via PlayM4.

use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;

pub const PREVIEW_WIDTH: u16 = 704;
pub const PREVIEW_HEIGHT: u16 = 576;

#[derive(Debug)]
pub struct PreviewWindow {
    conn: RustConnection,
    window: u32,
    wm_protocols: u32,
    wm_delete: u32,
    mapped: bool,
}

impl PreviewWindow {
    pub fn new() -> Option<Self> {
        let (conn, screen_num) = x11rb::connect(None).ok()?;
        let screen = &conn.setup().roots[screen_num];
        let root = screen.root;
        let depth = screen.root_depth;
        let visual = screen.root_visual;

        let win = conn.generate_id().ok()?;

        let x = (screen.width_in_pixels as i32 - PREVIEW_WIDTH as i32) / 2;
        let y = (screen.height_in_pixels as i32 - PREVIEW_HEIGHT as i32) / 2;
        let aux = CreateWindowAux::default()
            .background_pixel(screen.black_pixel)
            .event_mask(
                EventMask::EXPOSURE
                    | EventMask::STRUCTURE_NOTIFY
                    | EventMask::PROPERTY_CHANGE,
            );
        create_window(
            &conn, depth, win, root, x.max(0) as i16, y.max(0) as i16,
            PREVIEW_WIDTH, PREVIEW_HEIGHT, 0, WindowClass::INPUT_OUTPUT,
            visual, &aux,
        ).ok()?;

        // Window title
        for name in &[b"WM_NAME" as &[u8], b"_NET_WM_NAME" as &[u8]] {
            if let Ok(c) = conn.intern_atom(false, name) {
                if let Ok(reply) = c.reply() {
                    let title = b"Hikvision Preview";
                    let _ = change_property(
                        &conn, PropMode::REPLACE, win, reply.atom,
                        AtomEnum::STRING, 8, title.len() as u32, title,
                    );
                }
            }
        }

        // WM_CLASS
        if let Ok(c) = conn.intern_atom(false, b"WM_CLASS") {
            if let Ok(reply) = c.reply() {
                let data = b"hikvision-rs\0Preview\0";
                let _ = change_property(
                    &conn, PropMode::REPLACE, win, reply.atom,
                    AtomEnum::STRING, 8, data.len() as u32, data,
                );
            }
        }

        // WM_DELETE_WINDOW
        let wm_protocols = conn.intern_atom(false, b"WM_PROTOCOLS").ok()
            .and_then(|c| c.reply().ok())
            .map(|r| r.atom)?;
        let wm_delete = conn.intern_atom(false, b"WM_DELETE_WINDOW").ok()
            .and_then(|c| c.reply().ok())
            .map(|r| r.atom)?;
        let wm_delete_atom = wm_delete;
        let wm_delete_bytes: &[u8] = unsafe {
            std::slice::from_raw_parts(&wm_delete_atom as *const u32 as *const u8, 4)
        };
        change_property(
            &conn, PropMode::REPLACE, win, wm_protocols,
            AtomEnum::ATOM, 32, 1, wm_delete_bytes,
        ).ok();

        map_window(&conn, win).ok()?;
        let _ = conn.flush();

        log::info!("Preview window created: 0x{:x} ({}x{})", win, PREVIEW_WIDTH, PREVIEW_HEIGHT);
        Some(Self { conn, window: win, wm_protocols, wm_delete, mapped: true })
    }

    /// Cria uma janela X11 como child de outra janela (para embutir no painel egui).
    /// No Linux com X11, podemos reparentar a janela para que ela apareça
    /// dentro de um painel específico da GUI.
    pub fn new_inside(parent_xid: u32, x: i16, y: i16, width: u16, height: u16) -> Option<Self> {
        let (conn, screen_num) = x11rb::connect(None).ok()?;
        let screen = &conn.setup().roots[screen_num];
        let depth = screen.root_depth;
        let visual = screen.root_visual;

        let win = conn.generate_id().ok()?;

        let aux = CreateWindowAux::default()
            .background_pixel(screen.black_pixel)
            .event_mask(
                EventMask::EXPOSURE
                    | EventMask::STRUCTURE_NOTIFY
                    | EventMask::PROPERTY_CHANGE,
            );

        // Cria a janela com o parent especificado
        create_window(
            &conn, depth, win, parent_xid, x, y,
            width, height, 0, WindowClass::INPUT_OUTPUT,
            visual, &aux,
        ).ok()?;

        map_window(&conn, win).ok()?;
        let _ = conn.flush();

        log::info!("Embedded preview window created: 0x{:x} parent=0x{:x} ({}x{})",
            win, parent_xid, width, height);
        Some(Self { conn, window: win, wm_protocols: 0, wm_delete: 0, mapped: true })
    }

    pub fn window_id(&self) -> u32 {
        self.window
    }

    /// Processa eventos X11 pendentes sem bloquear.
    /// Retorna false se o usuário fechou a janela.
    pub fn poll_events(&mut self) -> bool {
        while let Ok(Some(event)) = self.conn.poll_for_event() {
            match event {
                Event::ClientMessage(msg) => {
                    if msg.type_ == self.wm_delete {
                        log::info!("WM_DELETE_WINDOW received");
                        return false;
                    }
                }
                Event::DestroyNotify(ev) => {
                    if ev.window == self.window {
                        log::info!("DestroyNotify received for preview window");
                        self.mapped = false;
                        return false;
                    }
                }
                Event::UnmapNotify(ev) => {
                    if ev.window == self.window {
                        self.mapped = false;
                    }
                }
                Event::MapNotify(ev) => {
                    if ev.window == self.window {
                        self.mapped = true;
                    }
                }
                _ => {}
            }
        }
        true
    }

    pub fn hide(&self) -> Result<(), Box<dyn std::error::Error>> {
        if self.mapped {
            unmap_window(&self.conn, self.window)?;
            self.conn.flush()?;
        }
        Ok(())
    }

    pub fn show(&self) -> Result<(), Box<dyn std::error::Error>> {
        if !self.mapped {
            map_window(&self.conn, self.window)?;
            self.conn.flush()?;
        }
        Ok(())
    }

    pub fn resize(&self, width: u16, height: u16) -> Result<(), Box<dyn std::error::Error>> {
        configure_window(&self.conn, self.window,
            &ConfigureWindowAux::new().width(width as u32).height(height as u32))?;
        self.conn.flush()?;
        Ok(())
    }

    pub fn reposition(&self, x: i16, y: i16, width: u16, height: u16) -> Result<(), Box<dyn std::error::Error>> {
        configure_window(&self.conn, self.window,
            &ConfigureWindowAux::new()
                .x(x as i32)
                .y(y as i32)
                .width(width as u32)
                .height(height as u32))?;
        self.conn.flush()?;
        Ok(())
    }
}

impl Drop for PreviewWindow {
    fn drop(&mut self) {
        log::info!("Destroying preview window 0x{:x}", self.window);
        let _ = destroy_window(&self.conn, self.window);
        let _ = self.conn.flush();
    }
}
