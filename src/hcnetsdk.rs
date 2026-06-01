//! Bindings FFI para HCNetSDK (Hikvision Device Network SDK)
//!
//! Este módulo implementa bindings para as funções essenciais do SDK da Hikvision
//! necessárias para streaming com descriptografia automática.

use anyhow::{Context, Result};
use libloading::Library;
use std::ffi::{c_char, c_int, c_void, CString};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

// Type aliases matching HCNetSDK.h
type LONG = c_int;
type DWORD = u32;
type BOOL = c_int;
type HWND = u32;

// Constants from HCNetSDK.h
pub const NAME_LEN: usize = 32;
pub const SERIALNO_LEN: usize = 48;
pub const NET_DVR_DEV_ADDRESS_MAX_LEN: usize = 129;

// Stream types
pub const STREAM_MAIN: DWORD = 0;
pub const STREAM_SUB: DWORD = 1;

// Link modes
pub const LINK_TCP: DWORD = 0;
pub const LINK_UDP: DWORD = 1;
pub const LINK_RTSP: DWORD = 4;
pub const LINK_RTSP_HTTP: DWORD = 5;

// Data types for callback
pub const NET_DVR_SYSHEAD: DWORD = 1;
pub const NET_DVR_STREAMDATA: DWORD = 2;

/// Device information returned by NET_DVR_Login_V40
#[repr(C)]
#[derive(Debug, Clone)]
pub struct NET_DVR_DEVICEINFO_V30 {
    pub sSerialNumber: [c_char; SERIALNO_LEN],
    pub byAlarmInPortNum: u8,
    pub byAlarmOutPortNum: u8,
    pub byDiskNum: u8,
    pub byDVRType: u8,
    pub byChanNum: u8,
    pub byStartChan: u8,
    pub byAudioChanNum: u8,
    pub byIPChanNum: u8,
    pub byZeroChanNum: u8,
    pub byMainStream: u8,
    pub byRes2: u8,
    pub wDevType: u16,
    pub byRes1: [u8; 32],
}

/// Device info V40 (wrapper around V30 with extra fields)
#[repr(C)]
#[derive(Debug, Clone)]
pub struct NET_DVR_DEVICEINFO_V40 {
    pub struDeviceV30: NET_DVR_DEVICEINFO_V30,
    pub bySupportLock: u8,
    pub byRetryLoginTime: u8,
    pub byPasswordLevel: u8,
    pub byProxyType: u8,
    pub dwSurplusLockTime: DWORD,
    pub byCharEncodeType: u8,
    pub bySupportDev5: u8,
    pub bySupport: u8,
    pub byLoginMode: u8,
    pub dwOEMCode: DWORD,
    pub iResidualValidity: c_int,
    pub byResidualValidity: u8,
    pub bySingleStartDTalkChan: u8,
    pub bySingleDTalkChanNums: u8,
    pub byPassWordResetLevel: u8,
    pub bySupportStreamEncrypt: u8,
    pub byMarketType: u8,
    pub byRes2: [u8; 238],
}

/// Login information for NET_DVR_Login_V40
#[repr(C)]
pub struct NET_DVR_USER_LOGIN_INFO {
    pub sDeviceAddress: [c_char; NET_DVR_DEV_ADDRESS_MAX_LEN],
    pub byUseTransport: u8,
    pub wPort: u16,
    pub sUserName: [c_char; NAME_LEN],
    pub sPassword: [c_char; NAME_LEN],
    pub fLoginResultCallBack: *const c_void,
    pub pUser: *mut c_void,
    pub byLoginMode: u8,
    pub byHttps: u8,
    pub iProxyID: c_int,
    pub byUseUTCTime: u8,
    pub byRes1: [u8; 119],
}

/// Preview information for NET_DVR_RealPlay_V40
#[repr(C)]
pub struct NET_DVR_PREVIEWINFO {
    pub lChannel: LONG,
    pub dwStreamType: DWORD,
    pub dwLinkMode: DWORD,
    pub hPlayWnd: HWND,
    pub bBlocked: DWORD,
    pub bPassbackRecord: DWORD,
    pub byPreviewMode: u8,
    pub byStreamID: [u8; 32],
    pub byProtoType: u8,
    pub byRes1: u8,
    pub byVideoCodingType: u8,
    pub dwDisplayBufNum: DWORD,
    pub byNPQMode: u8,
    pub byRecvMetaData: u8,
    pub byRes: [u8; 214],
}

/// Callback type for real-time stream data
pub type REALDATACALLBACK = extern "C" fn(LONG, DWORD, *mut u8, DWORD, *mut c_void);

macro_rules! get_fn {
    ($lib:expr, $name:literal, $sig:ty) => {
        unsafe {
            $lib.get::<$sig>($name)
                .with_context(|| format!("symbol '{}' not found in libhcnetsdk.so", std::str::from_utf8($name).unwrap_or("???")))?
        }
    };
}

pub fn default_search_paths() -> Vec<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    let cwd = std::env::current_dir().unwrap_or_default();
    let config_dir = PathBuf::from(&home).join(".config/hikvision-rs");
    let localcomponent = PathBuf::from(&home).join(".local/share/hikvision/weblocalserver/files/bin");

    vec![
        cwd.join("hikvision-libs/libhcnetsdk.so"),
        config_dir.join("libhcnetsdk.so"),
        localcomponent.join("libhcnetsdk.so"),
        PathBuf::from("/usr/local/lib/libhcnetsdk.so"),
        PathBuf::from("/usr/lib/libhcnetsdk.so"),
    ]
}

pub fn search_and_load() -> Result<HCNetSDK> {
    let paths = default_search_paths();
    let mut last_err = None;

    for path in &paths {
        if path.exists() {
            match HCNetSDK::load_from(path) {
                Ok(sdk) => {
                    log::info!("Loaded HCNetSDK from {}", path.display());
                    return Ok(sdk);
                }
                Err(e) => {
                    log::warn!("Failed to load {}: {}", path.display(), e);
                    last_err = Some(e);
                }
            }
        } else {
            log::debug!("HCNetSDK not found at {}", path.display());
        }
    }

    Err(last_err.unwrap_or_else(|| anyhow::anyhow!(
        "libhcnetsdk.so not found in any search path. Copy it to hikvision-libs/ or install Hikvision SDK."
    )))
}

/// HCNetSDK wrapper
pub struct HCNetSDK {
    _lib: Library,
    // Keep references to functions
    _init: unsafe extern "C" fn() -> BOOL,
    _cleanup: unsafe extern "C" fn() -> BOOL,
    _login_v40: unsafe extern "C" fn(*mut NET_DVR_USER_LOGIN_INFO, *mut NET_DVR_DEVICEINFO_V40) -> LONG,
    _logout: unsafe extern "C" fn(LONG) -> BOOL,
    _set_sdk_secret_key: unsafe extern "C" fn(LONG, *const c_char) -> BOOL,
    _realplay_v40: unsafe extern "C" fn(LONG, *const NET_DVR_PREVIEWINFO, Option<REALDATACALLBACK>, *mut c_void) -> LONG,
    _stop_realplay: unsafe extern "C" fn(LONG) -> BOOL,
    _get_last_error: unsafe extern "C" fn() -> DWORD,
}

impl HCNetSDK {
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

        let _lib = unsafe { Library::new(path) }
            .with_context(|| format!("Failed to load: {}", path.display()))?;

        let init = *get_fn!(_lib, b"NET_DVR_Init", unsafe extern "C" fn() -> BOOL);
        let cleanup = *get_fn!(_lib, b"NET_DVR_Cleanup", unsafe extern "C" fn() -> BOOL);
        let login_v40 = *get_fn!(_lib, b"NET_DVR_Login_V40", unsafe extern "C" fn(*mut NET_DVR_USER_LOGIN_INFO, *mut NET_DVR_DEVICEINFO_V40) -> LONG);
        let logout = *get_fn!(_lib, b"NET_DVR_Logout", unsafe extern "C" fn(LONG) -> BOOL);
        let set_sdk_secret_key = *get_fn!(_lib, b"NET_DVR_SetSDKSecretKey", unsafe extern "C" fn(LONG, *const c_char) -> BOOL);
        let realplay_v40 = *get_fn!(_lib, b"NET_DVR_RealPlay_V40", unsafe extern "C" fn(LONG, *const NET_DVR_PREVIEWINFO, Option<REALDATACALLBACK>, *mut c_void) -> LONG);
        let stop_realplay = *get_fn!(_lib, b"NET_DVR_StopRealPlay", unsafe extern "C" fn(LONG) -> BOOL);
        let get_last_error = *get_fn!(_lib, b"NET_DVR_GetLastError", unsafe extern "C" fn() -> DWORD);

        log::info!("Loaded libhcnetsdk.so from {}", path.display());

        Ok(Self {
            _lib,
            _init: init,
            _cleanup: cleanup,
            _login_v40: login_v40,
            _logout: logout,
            _set_sdk_secret_key: set_sdk_secret_key,
            _realplay_v40: realplay_v40,
            _stop_realplay: stop_realplay,
            _get_last_error: get_last_error,
        })
    }

    /// Initialize the SDK (must call before any other functions)
    pub fn init(&self) -> Result<()> {
        let ret = unsafe { (self._init)() };
        if ret != 0 {
            log::info!("NET_DVR_Init succeeded");
            Ok(())
        } else {
            let err = self.get_last_error();
            anyhow::bail!("NET_DVR_Init failed (error {})", err);
        }
    }

    /// Cleanup SDK resources
    pub fn cleanup(&self) {
        unsafe { (self._cleanup)() };
    }

    /// Login to device
    pub fn login(&self, host: &str, port: u16, username: &str, password: &str) -> Result<(LONG, NET_DVR_DEVICEINFO_V40)> {
        let mut login_info: NET_DVR_USER_LOGIN_INFO = unsafe { std::mem::zeroed() };
        let mut device_info: NET_DVR_DEVICEINFO_V40 = unsafe { std::mem::zeroed() };

        // Copy device address
        let device_addr = CString::new(host).context("host contains null byte")?;
        let bytes = device_addr.as_bytes_with_nul();
        login_info.sDeviceAddress[..bytes.len().min(NET_DVR_DEV_ADDRESS_MAX_LEN)]
            .copy_from_slice(unsafe { &*(bytes as *const [u8] as *const [i8]) });

        // Copy username
        let user = CString::new(username).context("username contains null byte")?;
        let bytes = user.as_bytes_with_nul();
        login_info.sUserName[..bytes.len().min(NAME_LEN)]
            .copy_from_slice(unsafe { &*(bytes as *const [u8] as *const [i8]) });

        // Copy password
        let pass = CString::new(password).context("password contains null byte")?;
        let bytes = pass.as_bytes_with_nul();
        login_info.sPassword[..bytes.len().min(NAME_LEN)]
            .copy_from_slice(unsafe { &*(bytes as *const [u8] as *const [i8]) });

        login_info.wPort = port;
        login_info.byLoginMode = 0; // Private protocol

        let user_id = unsafe { (self._login_v40)(&mut login_info, &mut device_info) };

        if user_id < 0 {
            let err = self.get_last_error();
            anyhow::bail!("NET_DVR_Login_V40 failed (error {})", err);
        }

        log::info!("Login succeeded, user_id={}", user_id);
        Ok((user_id, device_info))
    }

    /// Set SDK secret key for decryption
    pub fn set_sdk_secret_key(&self, user_id: LONG, key: &str) -> Result<()> {
        let key_c = CString::new(key).context("key contains null byte")?;
        let ret = unsafe { (self._set_sdk_secret_key)(user_id, key_c.as_ptr()) };

        if ret != 0 {
            log::info!("NET_DVR_SetSDKSecretKey succeeded");
            Ok(())
        } else {
            let err = self.get_last_error();
            anyhow::bail!("NET_DVR_SetSDKSecretKey failed (error {})", err);
        }
    }

    /// Start real-time preview with callback
    pub fn realplay(&self, user_id: LONG, channel: LONG, stream_type: DWORD, callback: REALDATACALLBACK, user_data: *mut c_void) -> Result<LONG> {
        let mut preview_info: NET_DVR_PREVIEWINFO = unsafe { std::mem::zeroed() };

        preview_info.lChannel = channel;
        preview_info.dwStreamType = stream_type;
        preview_info.dwLinkMode = LINK_RTSP;
        preview_info.hPlayWnd = 0; // No window, use callback
        preview_info.bBlocked = 1;

        let handle = unsafe { (self._realplay_v40)(user_id, &preview_info, Some(callback), user_data) };

        if handle < 0 {
            let err = self.get_last_error();
            anyhow::bail!("NET_DVR_RealPlay_V40 failed (error {})", err);
        }

        log::info!("RealPlay started, handle={}", handle);
        Ok(handle)
    }

    /// Stop real-time preview
    pub fn stop_realplay(&self, handle: LONG) -> Result<()> {
        let ret = unsafe { (self._stop_realplay)(handle) };
        if ret != 0 {
            log::info!("StopRealPlay succeeded");
            Ok(())
        } else {
            let err = self.get_last_error();
            anyhow::bail!("NET_DVR_StopRealPlay failed (error {})", err);
        }
    }

    /// Logout from device
    pub fn logout(&self, user_id: LONG) -> Result<()> {
        let ret = unsafe { (self._logout)(user_id) };
        if ret != 0 {
            log::info!("Logout succeeded");
            Ok(())
        } else {
            let err = self.get_last_error();
            anyhow::bail!("NET_DVR_Logout failed (error {})", err);
        }
    }

    /// Get last error code
    pub fn get_last_error(&self) -> DWORD {
        unsafe { (self._get_last_error)() }
    }
}

impl Drop for HCNetSDK {
    fn drop(&mut self) {
        self.cleanup();
    }
}
