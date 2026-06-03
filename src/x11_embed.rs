//! Gerenciamento de janelas X11 para overlay de vídeo Hikvision.
//!
//! Cada câmera recebe uma janela X11 **override-redirect** posicionada
//! sobre a área do egui reservada para aquela câmera.
//!
//! Por que override-redirect?
//! - O winit/eframe gerencia a janela principal e seu event loop.
//! - Janelas filhas (SubstructureRedirect) causam BadWindow no winit.
//! - Override-redirect bypassa o WM e o winit — ninguém além de nós
//!   sabe que essas janelas existem. Sem BadWindow, sem crashes.
//!
//! Fluxo:
//! 1. A cada frame, egui calcula retângulos para cada câmera
//! 2. Para cada retângulo, ensure_window() cria/sincroniza uma janela X11
//! 3. NET_DVR_RealPlay_V40 renderiza via overlay na janela X11
//! 4. poll_events() processa eventos X11 (ex: Exposure) para redraw

use std::collections::HashMap;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::rust_connection::RustConnection;

/// Conexão X11 compartilhada (única por processo).
static X11_CONN: std::sync::Mutex<Option<RustConnection>> = std::sync::Mutex::new(None);

/// Screen number da conexão X11.
static X11_SCREEN: std::sync::Mutex<Option<usize>> = std::sync::Mutex::new(None);

/// Inicializa a conexão X11 (chamar uma vez no startup).
pub fn init_x11() -> Option<()> {
    let mut guard = X11_CONN.lock().unwrap();
    if guard.is_some() {
        return Some(());
    }
    let (conn, screen_num) = x11rb::connect(None).ok()?;
    *guard = Some(conn);
    *X11_SCREEN.lock().unwrap() = Some(screen_num);
    log::info!("x11_embed: X11 connection initialized (screen={})", screen_num);
    Some(())
}

/// Posição global da janela principal na tela (atualizado a cada frame).
static MAIN_WINDOW_POS: std::sync::Mutex<(i32, i32)> = std::sync::Mutex::new((0, 0));

static MAIN_WINDOW_XID: std::sync::Mutex<Option<u32>> = std::sync::Mutex::new(None);

/// Armazena o XID da janela principal.
pub fn set_main_window_xid(xid: u32) {
    *MAIN_WINDOW_XID.lock().unwrap() = Some(xid);
    log::info!("x11_embed: main window XID = 0x{:x}", xid);
}

/// Atualiza a posição global da janela principal (top-left na tela).
/// Chamado a cada frame — consulta X11 diretamente pela posição real.
pub fn update_main_window_pos_from_x11() {
    let xid_guard = MAIN_WINDOW_XID.lock().unwrap();
    let main_xid = match *xid_guard {
        Some(x) => x,
        None => return,
    };
    drop(xid_guard);

    let pos: Option<(i32, i32)> = with_conn(|conn, _| {
        let cookie = match conn.query_tree(main_xid) {
            Ok(c) => c,
            Err(_) => return None,
        };
        let tree = match cookie.reply() {
            Ok(r) => r,
            Err(_) => return None,
        };
        let cookie = match conn.translate_coordinates(main_xid, tree.root, 0, 0) {
            Ok(c) => c,
            Err(_) => return None,
        };
        match cookie.reply() {
            Ok(coords) => Some((coords.dst_x as i32, coords.dst_y as i32)),
            Err(_) => None,
        }
    }).flatten();

    if let Some((x, y)) = pos {
        *MAIN_WINDOW_POS.lock().unwrap() = (x, y);
    }
}

/// Extrai o XID de um RawWindowHandle do winit/eframe.
pub fn xid_from_raw_handle(handle: &raw_window_handle::RawWindowHandle) -> Option<u32> {
    match handle {
        raw_window_handle::RawWindowHandle::Xlib(h) => {
            let xid = h.window;
            if xid == 0 { None } else { Some(xid as u32) }
        }
        _ => None,
    }
}

/// Helper: executa uma operação X11 com a conexão compartilhada.
fn with_conn<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&RustConnection, usize) -> R,
{
    let guard = X11_CONN.lock().unwrap();
    let screen = match *X11_SCREEN.lock().unwrap() {
        Some(s) => s,
        None => return None,
    };
    match guard.as_ref() {
        Some(conn) => Some(f(conn, screen)),
        None => None,
    }
}

/// Uma janela X11 override-redirect posicionada sobre a área de vídeo.
///
/// Cada janela tem sua **própria** conexão X11 para `poll_events()`,
/// evitando conflito com o event loop do winit na conexão compartilhada.
pub struct EmbeddedX11Window {
    /// Conexão X11 dedicada para polling de eventos desta janela.
    event_conn: RustConnection,
    /// Window ID (XID).
    window: u32,
    /// WM_DELETE_WINDOW atom (para detectar close).
    wm_delete: u32,
    /// Última posição e tamanho (absoluto na tela).
    last_x: i32,
    last_y: i32,
    last_w: u32,
    last_h: u32,
    /// Janela está mapeada (visível)?
    visible: bool,
}

impl EmbeddedX11Window {
    /// Cria uma nova janela X11 override-redirect.
    ///
    /// `abs_x`, `abs_y` são coordenadas **absolutas na tela** (posição
    /// da célula egui + posição da janela principal).
    pub fn new(abs_x: i32, abs_y: i32, width: u32, height: u32) -> Option<Self> {
        let guard = X11_CONN.lock().unwrap();
        let conn = guard.as_ref()?;
        let screen_num = X11_SCREEN.lock().unwrap().unwrap_or(0);
        let screen = &conn.setup().roots[screen_num];
        let root = screen.root;
        let depth = screen.root_depth;
        let visual = screen.root_visual;

        let win = conn.generate_id().ok()?;

        // Override-redirect: o WM não intercepta esta janela.
        // Isso evita BadWindow no winit e permite que o SDK renderize
        // via overlay sem interferência.
        let aux = CreateWindowAux::default()
            .background_pixel(screen.black_pixel)
            .override_redirect(1)
            .event_mask(EventMask::EXPOSURE | EventMask::STRUCTURE_NOTIFY);

        create_window(
            conn, depth, win, root,
            abs_x as i16, abs_y as i16,
            width.max(1) as u16, height.max(1) as u16,
            0, WindowClass::INPUT_OUTPUT,
            visual, &aux,
        ).ok()?;

        map_window(conn, win).ok()?;
        let _ = conn.flush();

        log::info!(
            "x11_embed: created override-redirect window 0x{:x} at ({},{}) {}x{}",
            win, abs_x, abs_y, width, height
        );

        // Conexão dedicada para poll_events
        let (event_conn, _) = x11rb::connect(None).ok()?;
        let wm_delete = event_conn.intern_atom(false, b"WM_DELETE_WINDOW").ok()
            .and_then(|c| c.reply().ok())
            .map(|r| r.atom)
            .unwrap_or(0);

        drop(guard);

        Some(Self {
            event_conn,
            window: win,
            wm_delete,
            last_x: abs_x,
            last_y: abs_y,
            last_w: width,
            last_h: height,
            visible: true,
        })
    }

    /// Sincroniza posição e tamanho com o retângulo do egui.
    ///
    /// `abs_x`, `abs_y` são coordenadas absolutas na tela.
    pub fn sync_rect(&mut self, abs_x: i32, abs_y: i32, width: u32, height: u32) {
        if width == 0 || height == 0 {
            if self.visible {
                self.hide();
            }
            return;
        }

        let needs_move = abs_x != self.last_x || abs_y != self.last_y;
        let needs_resize = width != self.last_w || height != self.last_h;

        if !self.visible {
            with_conn(|conn, _| {
                let _ = configure_window(
                    conn, self.window,
                    &ConfigureWindowAux::new()
                        .x(abs_x)
                        .y(abs_y)
                        .width(width)
                        .height(height),
                );
                let _ = map_window(conn, self.window);
                let _ = conn.flush();
            });
            self.visible = true;
            self.last_x = abs_x;
            self.last_y = abs_y;
            self.last_w = width;
            self.last_h = height;
            return;
        }

        if needs_move || needs_resize {
            with_conn(|conn, _| {
                let aux = ConfigureWindowAux::new()
                    .x(abs_x)
                    .y(abs_y)
                    .width(width)
                    .height(height);
                let _ = configure_window(conn, self.window, &aux);
                let _ = conn.flush();
            });
            self.last_x = abs_x;
            self.last_y = abs_y;
            self.last_w = width;
            self.last_h = height;
        }
    }

    /// Move a janela para frente (XRaiseWindow) para ficar sobre o egui.
    pub fn raise(&self) {
        with_conn(|conn, _| {
            let _ = configure_window(
                conn, self.window,
                &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
            );
            let _ = conn.flush();
        });
    }

    pub fn hide(&mut self) {
        if !self.visible {
            return;
        }
        with_conn(|conn, _| {
            let _ = unmap_window(conn, self.window);
            let _ = conn.flush();
        });
        self.visible = false;
    }

    pub fn show(&mut self) {
        if self.visible {
            return;
        }
        with_conn(|conn, _| {
            let _ = map_window(conn, self.window);
            let _ = conn.flush();
        });
        self.visible = true;
    }

    /// Processa eventos X11 pendentes (Exposure para redraw).
    /// Retorna false se a janela foi destruída externamente.
    pub fn poll_events(&mut self) -> bool {
        while let Ok(Some(event)) = self.event_conn.poll_for_event() {
            match event {
                x11rb::protocol::Event::DestroyNotify(ev) => {
                    if ev.window == self.window {
                        self.visible = false;
                        return false;
                    }
                }
                x11rb::protocol::Event::UnmapNotify(ev) => {
                    if ev.window == self.window {
                        self.visible = false;
                    }
                }
                _ => {}
            }
        }
        true
    }

    pub fn window_id(&self) -> u32 {
        self.window
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }
}

impl Drop for EmbeddedX11Window {
    fn drop(&mut self) {
        with_conn(|conn, _| {
            let _ = destroy_window(conn, self.window);
            let _ = conn.flush();
        });
        log::info!("x11_embed: destroyed window 0x{:x}", self.window);
    }
}

/// Gerenciador de múltiplas janelas X11 para multi-câmera.
pub struct X11WindowManager {
    windows: HashMap<usize, EmbeddedX11Window>,
}

impl X11WindowManager {
    pub fn new() -> Self {
        Self {
            windows: HashMap::new(),
        }
    }

    /// Garante que existe uma janela para o índice dado, na posição absoluta.
    ///
    /// `cell_x`, `cell_y` são coordenadas **locais** dentro da janela egui.
    /// Convertemos para absolutas somando a posição global da janela principal.
    pub fn ensure_window(&mut self, idx: usize, cell_x: f32, cell_y: f32, width: f32, height: f32) {
        let w = width.round().max(1.0) as u32;
        let h = height.round().max(1.0) as u32;
        if w == 0 || h == 0 {
            return;
        }

        // Converter coordenadas locais para absolutas na tela
        let (main_x, main_y) = *MAIN_WINDOW_POS.lock().unwrap();
        let abs_x = main_x + cell_x.round() as i32;
        let abs_y = main_y + cell_y.round() as i32;

        if let Some(wnd) = self.windows.get_mut(&idx) {
            wnd.sync_rect(abs_x, abs_y, w, h);
            wnd.raise();
        } else if let Some(wnd) = EmbeddedX11Window::new(abs_x, abs_y, w, h) {
            self.windows.insert(idx, wnd);
        }
    }

    pub fn remove_window(&mut self, idx: usize) {
        self.windows.remove(&idx);
    }

    pub fn hide_all(&mut self) {
        for wnd in self.windows.values_mut() {
            wnd.hide();
        }
    }

    /// Esconde todas as janelas exceto a do índice dado.
    pub fn hide_all_except(&mut self, keep_idx: usize) {
        for (idx, wnd) in self.windows.iter_mut() {
            if *idx != keep_idx {
                wnd.hide();
            }
        }
    }

    pub fn clear(&mut self) {
        self.windows.clear();
    }

    pub fn window_id(&self, idx: usize) -> Option<u32> {
        self.windows.get(&idx).map(|w| w.window_id())
    }

    pub fn window_count(&self) -> usize {
        self.windows.len()
    }

    /// Processa eventos X11 de todas as janelas. Remove as que foram destruídas.
    pub fn poll_all(&mut self) {
        let mut dead = Vec::new();
        for (idx, wnd) in self.windows.iter_mut() {
            if !wnd.poll_events() {
                dead.push(*idx);
            }
        }
        for idx in dead {
            self.windows.remove(&idx);
        }
    }
}
