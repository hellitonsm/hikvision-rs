use anyhow::{Context, Result};
use libloading::Library;
use std::ffi::{c_char, c_int, c_void, CString};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[repr(C)]
pub struct FrameInfo {
    pub n_width: c_int,
    pub n_height: c_int,
    pub n_stamp: c_int,
    pub n_type: c_int,
    pub n_frame_rate: c_int,
    pub dw_frame_num: u32,
}

pub type DecCallBack = unsafe extern "C" fn(
    n_port: c_int,
    p_buf: *mut c_char,
    n_size: c_int,
    p_frame_info: *mut FrameInfo,
    n_user: *mut c_void,
    n_reserved2: c_int,
);

pub const T_YV12: c_int = 3;
pub const T_RGB32: c_int = 7;

macro_rules! get_fn {
    ($lib:expr, $name:literal, $sig:ty) => {
        unsafe {
            $lib.get::<$sig>($name)
                .with_context(|| format!("symbol '{:?}' not found in libPlayCtrl.so", std::str::from_utf8($name).unwrap_or("???")))?
        }
    };
}

pub fn default_search_paths() -> Vec<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    let cwd = std::env::current_dir().unwrap_or_default();
    let config_dir = PathBuf::from(&home).join(".config/hikvision-rs");
    let localcomponent = PathBuf::from(&home).join(".local/share/hikvision/weblocalserver/files/bin");

    vec![
        cwd.join("hikvision-libs/libPlayCtrl.so"),
        config_dir.join("libPlayCtrl.so"),
        localcomponent.join("libPlayCtrl.so"),
        PathBuf::from("/usr/local/lib/libPlayCtrl.so"),
        PathBuf::from("/usr/lib/libPlayCtrl.so"),
    ]
}

pub fn default_library_path() -> PathBuf {
    default_search_paths()
        .into_iter()
        .find(|p| p.exists())
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
            PathBuf::from(&home).join(".config/hikvision-rs/libPlayCtrl.so")
        })
}

pub fn default_library_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    PathBuf::from(&home).join(".config/hikvision-rs")
}

pub fn search_and_load() -> Result<PlayCtrl> {
    let paths = default_search_paths();
    let mut last_err = None;
    for path in &paths {
        if path.exists() {
            match PlayCtrl::load_from(path) {
                Ok(p) => {
                    log::info!("Loaded PlayCtrl from {}", path.display());
                    return Ok(p);
                }
                Err(e) => {
                    log::warn!("Failed to load {}: {}", path.display(), e);
                    last_err = Some(e);
                }
            }
        } else {
            log::debug!("PlayCtrl not found at {}", path.display());
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!(
        "libPlayCtrl.so not found in any search path. Copy it to hikvision-libs/ or install Hikvision LocalComponent."
    )))
}

pub struct PlayCtrl {
    _lib: Library,
    port: Mutex<Option<c_int>>,
}

impl PlayCtrl {
    pub fn load() -> Result<Self> {
        Self::load_from(&default_library_path())
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        let lib_dir = path.parent().unwrap_or(Path::new("."));

        // Set LD_LIBRARY_PATH for bundled Qt5 deps
        unsafe {
            let current = std::env::var("LD_LIBRARY_PATH").unwrap_or_default();
            let dir_str = lib_dir.to_string_lossy().to_string();
            if !current.is_empty() {
                std::env::set_var("LD_LIBRARY_PATH", format!("{}:{}", dir_str, current));
            } else {
                std::env::set_var("LD_LIBRARY_PATH", &dir_str);
            }
        }

        let _lib: Library = unsafe { Library::new(path) }
            .with_context(|| format!("Failed to load: {}", path.display()))?;

        log::info!("Loaded libPlayCtrl.so from {}", path.display());

        Ok(Self {
            _lib,
            port: Mutex::new(None),
        })
    }

    pub fn get_port(&self) -> Result<c_int> {
        let func = get_fn!(self._lib, b"PlayM4_GetPort", unsafe extern "C" fn(*mut c_int) -> c_int);
        let mut port: c_int = -1;
        let ret = unsafe { func(&mut port) };
        if Self::success(ret) && port >= 0 {
            *self.port.lock().unwrap() = Some(port);
            Ok(port)
        } else {
            let err = self.get_last_error(0);
            anyhow::bail!("PlayM4_GetPort failed (ret={}, error={})", ret, err);
        }
    }

    /// Hikvision SDK convention: non-zero (TRUE) = success, zero (FALSE) = failure
    fn success(ret: c_int) -> bool {
        ret != 0
    }

    pub fn free_port(&self, port: c_int) -> Result<()> {
        let func = get_fn!(self._lib, b"PlayM4_FreePort", unsafe extern "C" fn(c_int) -> c_int);
        let ret = unsafe { func(port) };
        if Self::success(ret) {
            *self.port.lock().unwrap() = None;
            Ok(())
        } else {
            anyhow::bail!("PlayM4_FreePort failed (error {})", self.get_last_error(port));
        }
    }

    pub fn set_stream_open_mode(&self, port: c_int, mode: u32) -> Result<()> {
        let func = get_fn!(self._lib, b"PlayM4_SetStreamOpenMode", unsafe extern "C" fn(c_int, u32) -> c_int);
        let ret = unsafe { func(port, mode) };
        if Self::success(ret) { Ok(()) }
        else { anyhow::bail!("PlayM4_SetStreamOpenMode failed (error {})", self.get_last_error(port)) }
    }

    pub fn get_picture_size(&self, port: c_int) -> Result<(c_int, c_int)> {
        let func = get_fn!(self._lib, b"PlayM4_GetPictureSize", unsafe extern "C" fn(c_int, *mut c_int, *mut c_int) -> c_int);
        let mut w: c_int = 0;
        let mut h: c_int = 0;
        let ret = unsafe { func(port, &mut w, &mut h) };
        if Self::success(ret) && w > 0 && h > 0 {
            Ok((w, h))
        } else {
            anyhow::bail!("PlayM4_GetPictureSize failed (error {})", self.get_last_error(port));
        }
    }

    pub fn get_played_frames(&self, port: c_int) -> u32 {
        let func = match unsafe { self._lib.get::<unsafe extern "C" fn(c_int) -> u32>(b"PlayM4_GetPlayedFrames") } {
            Ok(f) => f,
            Err(_) => return 0,
        };
        unsafe { func(port) }
    }

    pub fn set_decode_frame_type(&self, port: c_int, frame_type: u32) -> Result<()> {
        let func = get_fn!(self._lib, b"PlayM4_SetDecodeFrameType", unsafe extern "C" fn(c_int, u32) -> c_int);
        let ret = unsafe { func(port, frame_type) };
        if Self::success(ret) { Ok(()) }
        else { anyhow::bail!("PlayM4_SetDecodeFrameType failed (error {})", self.get_last_error(port)) }
    }

    pub fn set_dec_callback_mend(&self, port: c_int, callback: DecCallBack, user_data: *mut c_void) -> Result<()> {
        let func = get_fn!(self._lib, b"PlayM4_SetDecCallBackMend", unsafe extern "C" fn(c_int, DecCallBack, *mut c_void) -> c_int);
        let ret = unsafe { func(port, callback, user_data) };
        if Self::success(ret) { Ok(()) }
        else { anyhow::bail!("PlayM4_SetDecCallBackMend failed (error {})", self.get_last_error(port)) }
    }

    pub fn set_dec_callback_ex_mend(
        &self,
        port: c_int,
        callback: DecCallBack,
        dest_buf: *mut c_char,
        dest_size: c_int,
        user_data: *mut c_void,
    ) -> Result<()> {
        let func = get_fn!(self._lib, b"PlayM4_SetDecCallBackExMend", unsafe extern "C" fn(c_int, DecCallBack, *mut c_char, c_int, *mut c_void) -> c_int);
        let ret = unsafe { func(port, callback, dest_buf, dest_size, user_data) };
        if Self::success(ret) { Ok(()) }
        else { anyhow::bail!("PlayM4_SetDecCallBackExMend failed (error {})", self.get_last_error(port)) }
    }

    pub fn refresh_play(&self, port: c_int) -> Result<()> {
        let func = get_fn!(self._lib, b"PlayM4_RefreshPlay", unsafe extern "C" fn(c_int) -> c_int);
        let ret = unsafe { func(port) };
        if Self::success(ret) { Ok(()) }
        else { anyhow::bail!("PlayM4_RefreshPlay failed (error {})", self.get_last_error(port)) }
    }

    pub fn get_total_frames(&self, port: c_int) -> u32 {
        let func = match unsafe { self._lib.get::<unsafe extern "C" fn(c_int) -> u32>(b"PlayM4_GetFileTotalFrames") } {
            Ok(f) => f,
            Err(_) => return 0,
        };
        unsafe { func(port) }
    }

    pub fn get_source_buffer_remain(&self, port: c_int) -> u32 {
        let func = match unsafe { self._lib.get::<unsafe extern "C" fn(c_int) -> u32>(b"PlayM4_GetSourceBufferRemain") } {
            Ok(f) => f,
            Err(_) => return 0,
        };
        unsafe { func(port) }
    }

    pub fn set_secret_key(&self, port: c_int, key: &str) -> Result<()> {
        let func = get_fn!(self._lib, b"PlayM4_SetSecretKey", unsafe extern "C" fn(c_int, c_int, *const c_char, c_int) -> c_int);
        let c_key = CString::new(key).context("key contains null byte")?;
        let ret = unsafe { func(port, 0, c_key.as_ptr(), key.len() as c_int) };
        if Self::success(ret) {
            Ok(())
        } else {
            anyhow::bail!("PlayM4_SetSecretKey failed (error {})", self.get_last_error(port));
        }
    }

    pub fn open_stream(&self, port: c_int, buf_size: u32) -> Result<()> {
        self.open_stream_with_header(port, &[], buf_size)
    }

    pub fn open_stream_with_header(&self, port: c_int, header: &[u8], buf_size: u32) -> Result<()> {
        let func = get_fn!(self._lib, b"PlayM4_OpenStream", unsafe extern "C" fn(c_int, *const u8, u32, u32) -> c_int);
        let (ptr, len) = if header.is_empty() {
            (std::ptr::null(), 0u32)
        } else {
            (header.as_ptr(), header.len() as u32)
        };
        let ret = unsafe { func(port, ptr, len, buf_size) };
        if Self::success(ret) {
            Ok(())
        } else {
            anyhow::bail!("PlayM4_OpenStream failed (error {})", self.get_last_error(port));
        }
    }

    pub fn close_stream(&self, port: c_int) -> Result<()> {
        let func = get_fn!(self._lib, b"PlayM4_CloseStream", unsafe extern "C" fn(c_int) -> c_int);
        let ret = unsafe { func(port) };
        if Self::success(ret) {
            Ok(())
        } else {
            anyhow::bail!("PlayM4_CloseStream failed (error {})", self.get_last_error(port));
        }
    }

    pub fn input_data(&self, port: c_int, data: &[u8]) -> Result<()> {
        let func = get_fn!(self._lib, b"PlayM4_InputData", unsafe extern "C" fn(c_int, *const u8, u32) -> c_int);
        let ret = unsafe { func(port, data.as_ptr(), data.len() as u32) };
        if Self::success(ret) {
            Ok(())
        } else {
            Err(anyhow::anyhow!("PlayM4_InputData failed (error {})", self.get_last_error(port)))
        }
    }

    pub fn get_last_error(&self, port: c_int) -> c_int {
        let func = match unsafe { self._lib.get::<unsafe extern "C" fn(c_int) -> c_int>(b"PlayM4_GetLastError") } {
            Ok(f) => f,
            Err(_) => return -1,
        };
        unsafe { func(port) }
    }

    pub fn get_jpeg(&self, port: c_int) -> Result<Vec<u8>> {
        let func = get_fn!(self._lib, b"PlayM4_GetJPEG", unsafe extern "C" fn(c_int, *mut u8, u32, *mut u32) -> c_int);
        let buf_size: u32 = 1024 * 1024;
        let mut buf: Vec<u8> = vec![0u8; buf_size as usize];
        let mut jpeg_size: u32 = 0;
        let ret = unsafe { func(port, buf.as_mut_ptr(), buf_size, &mut jpeg_size) };
        if Self::success(ret) {
            buf.truncate(jpeg_size as usize);
            log::debug!("PlayM4_GetJPEG: {} bytes", jpeg_size);
            Ok(buf)
        } else {
            anyhow::bail!("PlayM4_GetJPEG failed (error {})", self.get_last_error(port));
        }
    }

    pub fn play(&self, port: c_int) -> Result<()> {
        let func = get_fn!(self._lib, b"PlayM4_Play", unsafe extern "C" fn(c_int, u32) -> c_int);
        let ret = unsafe { func(port, 0) };
        if Self::success(ret) {
            Ok(())
        } else {
            anyhow::bail!("PlayM4_Play failed (error {})", self.get_last_error(port));
        }
    }

    pub fn stop(&self, port: c_int) -> Result<()> {
        let func = get_fn!(self._lib, b"PlayM4_Stop", unsafe extern "C" fn(c_int) -> c_int);
        let ret = unsafe { func(port) };
        if Self::success(ret) {
            Ok(())
        } else {
            anyhow::bail!("PlayM4_Stop failed (error {})", self.get_last_error(port));
        }
    }

    pub fn get_bmp(&self, port: c_int) -> Result<Vec<u8>> {
        let func = get_fn!(self._lib, b"PlayM4_GetBMP", unsafe extern "C" fn(c_int, *mut u8, u32, *mut u32) -> c_int);
        let buf_size: u32 = 1024 * 1024 * 4;
        let mut buf: Vec<u8> = vec![0u8; buf_size as usize];
        let mut bmp_size: u32 = 0;
        let ret = unsafe { func(port, buf.as_mut_ptr(), buf_size, &mut bmp_size) };
        if Self::success(ret) {
            buf.truncate(bmp_size as usize);
            log::debug!("PlayM4_GetBMP: {} bytes", bmp_size);
            Ok(buf)
        } else {
            anyhow::bail!("PlayM4_GetBMP failed (error {})", self.get_last_error(port));
        }
    }
}

pub fn last_error_name(code: c_int) -> &'static str {
    match code {
        0 => "PLAYM4_NOERROR",
        1 => "PLAYM4_PARA_OVER",
        2 => "PLAYM4_ENGINE_FAIL",
        3 => "PLAYM4_CREATE_WND_FAIL",
        4 => "PLAYM4_DXDEVICE_ERR",
        5 => "PLAYM4_GET_DISPLAY_FAIL",
        6 => "PLAYM4_DDRAW_FAIL",
        7 => "PLAYM4_DRAW_OVER",
        8 => "PLAYM4_NOT_SUPPORT",
        9 => "PLAYM4_FILE_OPEN_FAIL",
        10 => "PLAYM4_FILE_OVER_SIZE",
        11 => "PLAYM4_SET_OVER_TYPE_FIAL",
        12 => "PLAYM4_GET_OVER_TYPE_FAIL",
        13 => "PLAYM4_AUDIO_OPEN_FAIL",
        14 => "PLAYM4_AUDIO_GET_AUTHORITY_FAIL",
        15 => "PLAYM4_DECODE_FRAME_ERROR",
        16 => "PLAYM4_OPEN_STREAM_FAIL",
        17 => "PLAYM4_OPEN_FILE_CODEC_FAIL",
        18 => "PLAYM4_NOT_STREAM",
        19 => "PLAYM4_AUDIO_DECODE_FAIL",
        20 => "PLAYM4_GET_AUDIO_INFO_FAIL",
        21 => "PLAYM4_AUDIO_TYPE_NOT_SUPPORT",
        22 => "PLAYM4_SET_AUDIO_TYPE_FAIL",
        23 => "PLAYM4_BUFFER_OVER",
        24 => "PLAYM4_AUDIO_DECODER_NOT_INIT",
        25 => "PLAYM4_GET_INIT_FAIL",
        26 => "PLAYM4_NOT_INIT",
        27 => "PLAYM4_OPEN_FILE_TIMEOUT",
        28 => "PLAYM4_NEED_MORE_DATA",
        29 => "PLAYM4_SECRET_KEY_ERROR",
        30 => "PLAYM4_VERIFY_KEY_FAIL",
        31 => "PLAYM4_VERIFY_KEY_TIMEOUT",
        32 => "PLAYM4_NOT_SUPPORT_DECODE",
        _ => "UNKNOWN",
    }
}
