//! Bindings FFI para HCNetSDK (Hikvision Device Network SDK)
//!
//! Este módulo implementa bindings para as funções essenciais do SDK da Hikvision
//! necessárias para streaming com descriptografia automática.

use anyhow::{Context, Result};
use libloading::{Library, os::unix as unix_loader};
use std::ffi::{c_char, c_int, c_void, CString};
use std::os::unix::fs as unix_fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

// Safety: HCNetSDK only uses _lib (libloading::Library) during construction
// to load function pointers. After construction, _lib is never accessed --
// all SDK operations go through the loaded function pointers which are
// `unsafe extern "C" fn` (both Send + Sync). The Library is only accessed
// again in Drop to unload the shared library.
unsafe impl Sync for HCNetSDK {}

static SDK_INSTANCE: OnceLock<Arc<HCNetSDK>> = OnceLock::new();

/// Load and initialize the global HCNetSDK singleton on the current thread.
/// Must be called from the main thread before spawning any worker threads.
pub fn ensure_initialized() -> Result<()> {
    let sdk = SDK_INSTANCE.get_or_init(|| {
        search_and_load().map(Arc::new).unwrap_or_else(|e| {
            panic!("Failed to load HCNetSDK: {e}")
        })
    });
    // init() and set_connect_time() are called here on the main thread,
    // matching rustdemo's exact initialization sequence.
    sdk.init().context("NET_DVR_Init failed")?;
    sdk.set_connect_time(10_000, 1)
        .context("NET_DVR_SetConnectTime failed")?;
    log::info!("HCNetSDK initialized on main thread");
    Ok(())
}

/// Return a reference to the global HCNetSDK singleton.
/// Returns None if `ensure_initialized()` was not called yet.
pub fn try_global_sdk() -> Option<&'static Arc<HCNetSDK>> {
    SDK_INSTANCE.get()
}

/// Return a reference to the global HCNetSDK singleton.
/// Panics if `ensure_initialized()` was not called first.
pub fn global_sdk() -> &'static Arc<HCNetSDK> {
    try_global_sdk().expect("HCNetSDK not initialized. Call ensure_initialized() first.")
}

// Type aliases matching HCNetSDK.h
type LONG = c_int;
type DWORD = u32;
type BOOL = c_int;
pub type HWND = u32;

// Constants from HCNetSDK.h
pub const NAME_LEN: usize = 32;
pub const SERIALNO_LEN: usize = 48;
pub const NET_DVR_DEV_ADDRESS_MAX_LEN: usize = 129;
pub const NET_DVR_LOGIN_USERNAME_MAX_LEN: usize = 64;
pub const NET_DVR_LOGIN_PASSWD_MAX_LEN: usize = 64;

// Stream types
pub const STREAM_MAIN: DWORD = 0;
pub const STREAM_SUB: DWORD = 1;

// Link modes
pub const LINK_TCP: DWORD = 0;
pub const LINK_UDP: DWORD = 1;
pub const LINK_RTSP: DWORD = 4;
pub const LINK_RTSP_HTTP: DWORD = 5;

// Ability types for NET_DVR_GetDeviceAbility
pub const DEVICE_DYNCHAN_ABILITY: DWORD = 0x00b;  // digital channel ability (XML)
pub const DEVICE_ABILITY_INFO: DWORD = 0x011;     // common ability, XML node in pInBuf

// Data types for callback
pub const NET_DVR_SYSHEAD: DWORD = 1;
pub const NET_DVR_STREAMDATA: DWORD = 2;

/// Device information returned by NET_DVR_Login_V40
/// Layout matches HCNetSDK.h exactly (80 bytes on Linux x86_64).
#[repr(C)]
#[derive(Debug, Clone)]
pub struct NET_DVR_DEVICEINFO_V30 {
    pub sSerialNumber: [c_char; SERIALNO_LEN],  // 48
    pub byAlarmInPortNum: u8,                   // 1
    pub byAlarmOutPortNum: u8,                  // 1
    pub byDiskNum: u8,                          // 1
    pub byDVRType: u8,                          // 1
    pub byChanNum: u8,                          // 1
    pub byStartChan: u8,                        // 1
    pub byAudioChanNum: u8,                     // 1
    pub byIPChanNum: u8,                        // 1
    pub byZeroChanNum: u8,                      // 1  (offset 56)
    pub byMainProto: u8,                        // 1  (offset 57)
    pub bySubProto: u8,                         // 1  (offset 58)
    pub bySupport: u8,                          // 1
    pub bySupport1: u8,                         // 1
    pub bySupport2: u8,                         // 1
    pub wDevType: u16,                          // 2  (offset 62)
    pub bySupport3: u8,                         // 1
    pub byMultiStreamProto: u8,                 // 1
    pub byStartDChan: u8,                       // 1
    pub byStartDTalkChan: u8,                   // 1
    pub byHighDChanNum: u8,                     // 1
    pub bySupport4: u8,                         // 1
    pub byLanguageType: u8,                     // 1
    pub byVoiceInChanNum: u8,                   // 1
    pub byStartVoiceInChanNo: u8,               // 1
    pub bySupport5: u8,                         // 1
    pub bySupport6: u8,                         // 1
    pub byMirrorChanNum: u8,                    // 1
    pub wStartMirrorChanNo: u16,                // 2  (offset 76)
    pub bySupport7: u8,                         // 1
    pub byRes2: u8,                             // 1  (offset 79, total=80)
}

/// Device info V40 (wrapper around V30 with extra fields, 344 bytes total)
#[repr(C)]
#[derive(Debug, Clone)]
pub struct NET_DVR_DEVICEINFO_V40 {
    pub struDeviceV30: NET_DVR_DEVICEINFO_V30, // 80 bytes
    pub bySupportLock: u8,                      // 1
    pub byRetryLoginTime: u8,                   // 1
    pub byPasswordLevel: u8,                    // 1
    pub byProxyType: u8,                        // 1  (offset 83)
    pub dwSurplusLockTime: DWORD,               // 4  (offset 84)
    pub byCharEncodeType: u8,                   // 1
    pub bySupportDev5: u8,                      // 1
    pub bySupport: u8,                          // 1
    pub byLoginMode: u8,                        // 1  (offset 91)
    pub dwOEMCode: DWORD,                       // 4  (offset 92)
    pub iResidualValidity: c_int,               // 4  (offset 96)
    pub byResidualValidity: u8,                 // 1  (offset 100)
    pub bySingleStartDTalkChan: u8,             // 1
    pub bySingleDTalkChanNums: u8,              // 1
    pub byPassWordResetLevel: u8,               // 1  (offset 103)
    pub bySupportStreamEncrypt: u8,             // 1
    pub byMarketType: u8,                       // 1  (offset 105)
    pub byRes2: [u8; 238],                      // 238 (offsets 106-343, total=344)
}

/// Login information for NET_DVR_Login_V40
/// Layout corresponde ao rustdemo (e ao HCNetSDK.h original)
#[repr(C)]
pub struct NET_DVR_USER_LOGIN_INFO {
    pub sDeviceAddress: [c_char; NET_DVR_DEV_ADDRESS_MAX_LEN],
    pub byUseTransport: u8,
    pub wPort: u16,
    pub sUserName: [c_char; NET_DVR_LOGIN_USERNAME_MAX_LEN],
    pub sPassword: [c_char; NET_DVR_LOGIN_PASSWD_MAX_LEN],
    pub byLoginMode: u8,      // 0=Private, 1=ISAPI
    pub byHttps: u8,          // 0=tcp, 1=tls
    pub byDeviceType: u8,
    pub byLoginClientType: u8,
    pub byProxyType: u8,
    pub sUserIP: [u8; 129],
    pub szDomain: [c_char; 256],
    pub bUseAsynLogin: BOOL,  // 0 = synchronous login
    pub byRes2: [u8; 125],
}

/// Preview information for NET_DVR_RealPlay_V40
/// Layout matches HCNetSDK.h exactly (280 bytes on Linux x86_64).
/// Note: byChanType does NOT exist in this struct — it belongs to
/// NET_DVR_DEV_CHAN_INFO_EX and other channel-config structs.
/// Canal Zero is selected via lChannel with the virtual channel index.
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
    pub byDataType: u8,
    pub byRes: [u8; 213],
}

/// Client preview info for NET_DVR_RealPlay_V30 (API antiga, mais permissiva).
/// bit 31 de lLinkMode: 0=main stream, 1=sub stream.
/// bits 0-30 de lLinkMode: 0=TCP, 1=UDP, 2=Multicast, 3=RTP, 4=RTP/RTSP, 5=RSTP/HTTP
#[repr(C)]
pub struct NET_DVR_CLIENTINFO {
    pub lChannel: LONG,
    pub lLinkMode: LONG,  // bit31=stream_type, bits0-30=link_mode
    pub hPlayWnd: HWND,
    pub sMultiCastIP: *mut c_char,  // null = no multicast
    pub byProtoType: u8,  // 0=private, 1=RTSP
    pub byRes: [u8; 3],
}

/// Callback type for real-time stream data
pub type REALDATACALLBACK = extern "C" fn(LONG, DWORD, *mut u8, DWORD, *mut c_void);

/// Zero channel configuration (command 1102/1103)
#[repr(C)]
pub struct NET_DVR_ZEROCHANCFG {
    pub dwSize: u32,
    pub byEnable: u8,        // 0=disabled, 1=enabled
    pub byRes1: [u8; 3],
    pub dwVideoBitrate: u32,
    pub dwVideoFrameRate: u32,
    pub byRes2: [u8; 32],
}

pub const NET_DVR_GET_ZEROCHANCFG: DWORD = 1102;
pub const NET_DVR_SET_ZEROCHANCFG: DWORD = 1103;
pub const NET_DVR_GET_ZERO_PREVIEWCFG_V30: DWORD = 1104;
pub const NET_DVR_SET_ZERO_PREVIEWCFG_V30: DWORD = 1105;

/// Zero channel preview config (command 1104/1105).
/// bySwitchSeq[preview_mode][window] maps window position to channel number.
/// 0xFF = window not used. MAX_PREVIEW_MODE=8, MAX_WINDOW_V30=32.
#[repr(C)]
pub struct NET_DVR_PREVIEWCFG_V30 {
    pub dwSize: u32,
    pub byPreviewNumber: u8,    // 0=1w, 1=4w, 2=9w, 3=16w, 0xff=max
    pub byEnableAudio: u8,
    pub wSwitchTime: u16,
    pub bySwitchSeq: [[u8; 32]; 8], // 256 bytes: [mode][window] = channel
    pub byRes: [u8; 24],
}

/// Preview info for NET_DVR_RealPlaySpecial (custom RTSP URL preview).
/// Allows passing an arbitrary RTSP URL directly to the SDK.
#[repr(C)]
pub struct NET_DVR_PREVIEWINFO_SPECIAL {
    pub sURL: [c_char; 1024],
    pub dwLinkMode: DWORD,
    pub hPlayWnd: HWND,
    pub bBlocked: DWORD,
    pub dwDisplayBufNum: DWORD,
    pub byRes: [u8; 64],
}

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

    // Full SDK installation (has HCNetSDKCom/ alongside)
    let sdk_home = PathBuf::from(&home)
        .join("Documentos/hikvision-linux/lib/libhcnetsdk.so");
    let sdk_qt = PathBuf::from(&home)
        .join("Documentos/hikvision-linux/QtDemo/Linux64/lib/libhcnetsdk.so");

    vec![
        cwd.join("hikvision-libs/libhcnetsdk.so"),
        sdk_home,
        sdk_qt,
        config_dir.join("libhcnetsdk.so"),
        localcomponent.join("libhcnetsdk.so"),
        PathBuf::from("/usr/local/lib/libhcnetsdk.so"),
        PathBuf::from("/usr/lib/libhcnetsdk.so"),
    ]
}

/// Procura pelo diretório HCNetSDKCom/ (componentes do SDK como libHCPreview.so)
fn find_hcnetsdkcom_dir(lib_dir: &Path) -> Option<PathBuf> {
    // 1. Ao lado da biblioteca carregada
    let direct = lib_dir.join("HCNetSDKCom");
    if direct.is_dir() {
        return Some(direct);
    }

    // 2. Diretório pai (algumas instalações colocam um nível acima)
    if let Some(parent) = lib_dir.parent() {
        let parent_com = parent.join("HCNetSDKCom");
        if parent_com.is_dir() {
            return Some(parent_com);
        }
    }

    // 3. Caminhos conhecidos da instalação do SDK
    let home = std::env::var("HOME").ok()?;
    let known = [
        format!("{home}/Documentos/hikvision-linux/lib/HCNetSDKCom"),
        format!("{home}/Documentos/hikvision-linux/QtDemo/Linux64/lib/HCNetSDKCom"),
        format!("{home}/Documentos/hikvision-linux/consoleDemo/linux64/lib/HCNetSDKCom"),
    ];
    for p in &known {
        let path = PathBuf::from(p);
        if path.is_dir() {
            return Some(path);
        }
    }

    None
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

/// Pre-load libPlayCtrl.so so the SDK's internal dlopen() reuses the handle.
/// We must also pre-load its dependencies (libAudioRender.so, libSuperRender.so)
/// via absolute paths, because setenv("LD_LIBRARY_PATH") after startup has no
/// effect on dlopen in glibc — the loader caches the search path at process start.
///
/// Matching rustdemo's approach where these are linked at compile time via
/// `cargo:rustc-link-lib=dylib=AudioRender` etc., so the dynamic linker already
/// has them loaded before any dlopen happens.
fn load_playctrl() -> Result<Option<Library>> {
    // Search for libPlayCtrl.so using the same paths as playctrl.rs
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    let cwd = std::env::current_dir().unwrap_or_default();
    let config_dir = PathBuf::from(&home).join(".config/hikvision-rs");
    let localcomponent = PathBuf::from(&home).join(".local/share/hikvision/weblocalserver/files/bin");

    let search_paths = vec![
        cwd.join("hikvision-libs/libPlayCtrl.so"),
        config_dir.join("libPlayCtrl.so"),
        localcomponent.join("libPlayCtrl.so"),
        PathBuf::from("/usr/local/lib/libPlayCtrl.so"),
        PathBuf::from("/usr/lib/libPlayCtrl.so"),
    ];

    let mut lib_dir: Option<PathBuf> = None;
    let mut path_opt: Option<PathBuf> = None;

    for p in &search_paths {
        if p.exists() {
            path_opt = Some(p.clone());
            lib_dir = p.parent().map(|d| d.to_path_buf());
            break;
        }
    }

    let Some(path) = path_opt else {
        log::warn!("libPlayCtrl.so not found in any search path.");
        return Ok(None);
    };

    let lib_dir = lib_dir.unwrap_or_else(|| path.parent().unwrap().to_path_buf());

    // Pre-load transitive deps via absolute paths so the dynamic linker doesn't
    // need LD_LIBRARY_PATH to resolve DT_NEEDED entries in libPlayCtrl.so.
    for dep in &["libAudioRender.so", "libSuperRender.so"] {
        let dep_path = lib_dir.join(dep);
        if dep_path.exists() {
            match unsafe { unix_loader::Library::open(Some(&dep_path), unix_loader::RTLD_LAZY) } {
                Ok(_) => log::info!("Pre-loaded dep {dep}"),
                Err(e) => log::warn!("Failed to pre-load dep {dep}: {e}"),
            }
        }
    }

    let lib = unsafe { unix_loader::Library::open(Some(&path), unix_loader::RTLD_LAZY) }
        .with_context(|| format!("Failed to load {}", path.display()))?;
    log::info!("Loaded libPlayCtrl.so from {}", path.display());
    Ok(Some(lib.into()))
}

/// HCNetSDK wrapper
pub struct HCNetSDK {
    _lib: Library,
    /// Pre-loaded libPlayCtrl.so — HCNetSDK internally dlopen()s this during
    /// NET_DVR_RealPlay_V40 (window mode), but fails on missing deps
    /// (libAudioRender.so, libSuperRender.so) with RTLD_NOW.  Loading it
    /// ourselves with RTLD_LAZY first makes the subsequent dlopen() reuse
    /// the already-loaded handle, skipping the symbol-resolution check.
    _lib_playctrl: Option<Library>,
    // Keep references to functions
    _init: unsafe extern "C" fn() -> BOOL,
    _cleanup: unsafe extern "C" fn() -> BOOL,
    _login_v40: unsafe extern "C" fn(*mut NET_DVR_USER_LOGIN_INFO, *mut NET_DVR_DEVICEINFO_V40) -> LONG,
    _logout: unsafe extern "C" fn(LONG) -> BOOL,
    _set_connect_time: unsafe extern "C" fn(DWORD, DWORD) -> BOOL,
    _set_sdk_secret_key: unsafe extern "C" fn(LONG, *const c_char) -> BOOL,
    _realplay_v40: unsafe extern "C" fn(LONG, *const NET_DVR_PREVIEWINFO, Option<REALDATACALLBACK>, *mut c_void) -> LONG,
    _realplay_v30: unsafe extern "C" fn(LONG, *const NET_DVR_CLIENTINFO, Option<REALDATACALLBACK>, *mut c_void, BOOL) -> LONG,
    _stop_realplay: unsafe extern "C" fn(LONG) -> BOOL,
    _get_last_error: unsafe extern "C" fn() -> DWORD,
    _set_player_buf_number: unsafe extern "C" fn(LONG) -> BOOL,
    _get_dvr_config: unsafe extern "C" fn(LONG, DWORD, LONG, *mut c_void, DWORD, *mut DWORD) -> BOOL,
    _set_dvr_config: unsafe extern "C" fn(LONG, DWORD, LONG, *const c_void, DWORD) -> BOOL,
    _get_device_ability: unsafe extern "C" fn(LONG, DWORD, *const c_char, DWORD, *mut c_char, DWORD) -> BOOL,
    _realplay_special: unsafe extern "C" fn(LONG, *const NET_DVR_PREVIEWINFO_SPECIAL, Option<REALDATACALLBACK>, *mut c_void) -> LONG,
    _zero_make_key_frame: unsafe extern "C" fn(LONG, LONG) -> BOOL,
}

impl HCNetSDK {
    pub fn load_from(path: &Path) -> Result<Self> {
        let lib_dir = path.parent().unwrap_or(Path::new("."));

        // Encontra HCNetSDKCom/ (componentes como libHCPreview.so que o SDK carrega)
        let com_dir = find_hcnetsdkcom_dir(lib_dir);
        let local_com = lib_dir.join("HCNetSDKCom");

        // Se encontrar HCNetSDKCom em outro local, faz symlink para junto da lib
        if let Some(ref com) = com_dir {
            if !local_com.exists() {
                log::info!("Symlinking HCNetSDKCom from {} to {}", com.display(), local_com.display());
                if let Err(e) = unix_fs::symlink(com, &local_com) {
                    log::warn!("Failed to symlink HCNetSDKCom: {}", e);
                }
            }
        }

        // Set LD_LIBRARY_PATH for bundled Qt5 deps and HCNetSDKCom components
        if local_com.exists() {
            log::info!("HCNetSDKCom available at {}", local_com.display());
        } else {
            log::warn!("HCNetSDKCom/ not found — SDK may fail with error 107 (NET_DVR_LOAD_HCPREVIEW_SDK_ERROR). \
                        Copy HCNetSDKCom/ next to libhcnetsdk.so or use the SDK installation directly.");
        }

        unsafe {
            let current = std::env::var("LD_LIBRARY_PATH").unwrap_or_default();
            let dir_str = lib_dir.to_string_lossy().to_string();
            let mut paths = vec![dir_str];
            if local_com.exists() {
                paths.push(local_com.to_string_lossy().to_string());
            }
            let new_path = paths.join(":");
            if !current.is_empty() {
                std::env::set_var("LD_LIBRARY_PATH", format!("{}:{}", new_path, current));
            } else {
                std::env::set_var("LD_LIBRARY_PATH", &new_path);
            }
        }

        let _lib = unsafe { Library::new(path) }
            .with_context(|| format!("Failed to load: {}", path.display()))?;

        let init = *get_fn!(_lib, b"NET_DVR_Init", unsafe extern "C" fn() -> BOOL);
        let cleanup = *get_fn!(_lib, b"NET_DVR_Cleanup", unsafe extern "C" fn() -> BOOL);
        let login_v40 = *get_fn!(_lib, b"NET_DVR_Login_V40", unsafe extern "C" fn(*mut NET_DVR_USER_LOGIN_INFO, *mut NET_DVR_DEVICEINFO_V40) -> LONG);
        let logout = *get_fn!(_lib, b"NET_DVR_Logout", unsafe extern "C" fn(LONG) -> BOOL);
        let set_connect_time = *get_fn!(_lib, b"NET_DVR_SetConnectTime", unsafe extern "C" fn(DWORD, DWORD) -> BOOL);
        let set_sdk_secret_key = *get_fn!(_lib, b"NET_DVR_SetSDKSecretKey", unsafe extern "C" fn(LONG, *const c_char) -> BOOL);
        let realplay_v40 = *get_fn!(_lib, b"NET_DVR_RealPlay_V40", unsafe extern "C" fn(LONG, *const NET_DVR_PREVIEWINFO, Option<REALDATACALLBACK>, *mut c_void) -> LONG);
        let realplay_v30 = *get_fn!(_lib, b"NET_DVR_RealPlay_V30", unsafe extern "C" fn(LONG, *const NET_DVR_CLIENTINFO, Option<REALDATACALLBACK>, *mut c_void, BOOL) -> LONG);
        let stop_realplay = *get_fn!(_lib, b"NET_DVR_StopRealPlay", unsafe extern "C" fn(LONG) -> BOOL);
        let get_last_error = *get_fn!(_lib, b"NET_DVR_GetLastError", unsafe extern "C" fn() -> DWORD);
        let set_player_buf_number = *get_fn!(_lib, b"NET_DVR_SetPlayerBufNumber", unsafe extern "C" fn(LONG) -> BOOL);
        let get_dvr_config = *get_fn!(_lib, b"NET_DVR_GetDVRConfig", unsafe extern "C" fn(LONG, DWORD, LONG, *mut c_void, DWORD, *mut DWORD) -> BOOL);
        let set_dvr_config = *get_fn!(_lib, b"NET_DVR_SetDVRConfig", unsafe extern "C" fn(LONG, DWORD, LONG, *const c_void, DWORD) -> BOOL);
        let get_device_ability = *get_fn!(_lib, b"NET_DVR_GetDeviceAbility", unsafe extern "C" fn(LONG, DWORD, *const c_char, DWORD, *mut c_char, DWORD) -> BOOL);
        let zero_make_key_frame = *get_fn!(_lib, b"NET_DVR_ZeroMakeKeyFrame", unsafe extern "C" fn(LONG, LONG) -> BOOL);
        let realplay_special = *get_fn!(_lib, b"NET_DVR_RealPlaySpecial", unsafe extern "C" fn(LONG, *const NET_DVR_PREVIEWINFO_SPECIAL, Option<REALDATACALLBACK>, *mut c_void) -> LONG);

        log::info!("Loaded libhcnetsdk.so from {}", path.display());

        // Pre-load libPlayCtrl.so from the same directory so the SDK's
        // internal dlopen() during NET_DVR_RealPlay_V40 (window mode)
        // reuses our already-loaded handle and skips RTLD_NOW symbol
        // resolution (which fails on missing libAudioRender.so /
        // libSuperRender.so).
        let _lib_playctrl = match load_playctrl() {
            Ok(lib) => {
                log::info!("Pre-loaded libPlayCtrl.so — RealPlay window mode should work");
                lib
            }
            Err(e) => {
                log::warn!("Failed to pre-load libPlayCtrl.so: {e} — RealPlay window mode may fail");
                None
            }
        };

        log::info!("Loaded libhcnetsdk.so from {}", path.display());

        Ok(Self {
            _lib,
            _lib_playctrl,
            _init: init,
            _cleanup: cleanup,
            _login_v40: login_v40,
            _logout: logout,
            _set_connect_time: set_connect_time,
            _set_sdk_secret_key: set_sdk_secret_key,
            _realplay_v40: realplay_v40,
            _realplay_v30: realplay_v30,
            _realplay_special: realplay_special,
            _stop_realplay: stop_realplay,
            _get_last_error: get_last_error,
            _set_player_buf_number: set_player_buf_number,
            _get_dvr_config: get_dvr_config,
            _set_dvr_config: set_dvr_config,
            _get_device_ability: get_device_ability,
            _zero_make_key_frame: zero_make_key_frame,
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

    /// Set connection timeout and retry count
    ///
    /// Qt demo reference: NET_DVR_SetConnectTime(10000, 1) — 10s timeout, 1 retry
    pub fn set_connect_time(&self, wait_ms: u32, retries: u32) -> Result<()> {
        let ret = unsafe { (self._set_connect_time)(wait_ms, retries) };
        if ret != 0 {
            log::info!("NET_DVR_SetConnectTime({}, {}) succeeded", wait_ms, retries);
            Ok(())
        } else {
            let err = self.get_last_error();
            anyhow::bail!("NET_DVR_SetConnectTime failed (error {})", err);
        }
    }

    /// Set player buffer number for real-time preview.
    /// Must be called before NET_DVR_RealPlay_V40 when using window rendering.
    pub fn set_player_buf_number(&self, buf_num: LONG) -> Result<()> {
        let ret = unsafe { (self._set_player_buf_number)(buf_num) };
        if ret != 0 {
            log::info!("NET_DVR_SetPlayerBufNumber({}) succeeded", buf_num);
            Ok(())
        } else {
            let err = self.get_last_error();
            log::warn!("NET_DVR_SetPlayerBufNumber failed (error {}), continuing anyway", err);
            Ok(())
        }
    }

    /// Cleanup SDK resources
    pub fn cleanup(&self) {
        unsafe { (self._cleanup)() };
    }

    /// Login to device
    /// Segue a mesma abordagem do rustdemo: bytes copiados diretamente nos arrays C.
    pub fn login(&self, host: &str, port: u16, username: &str, password: &str) -> Result<(LONG, NET_DVR_DEVICEINFO_V40)> {
        let mut login_info: NET_DVR_USER_LOGIN_INFO = unsafe { std::mem::zeroed() };
        let mut device_info: NET_DVR_DEVICEINFO_V40 = unsafe { std::mem::zeroed() };

        // Copy device address (byte a byte, como o rustdemo faz)
        let ip_bytes = host.as_bytes();
        let n = ip_bytes.len().min(NET_DVR_DEV_ADDRESS_MAX_LEN - 1);
        login_info.sDeviceAddress[..n].copy_from_slice(unsafe { &*(&ip_bytes[..n] as *const [u8] as *const [i8]) });

        // Copy username
        let user_bytes = username.as_bytes();
        let n = user_bytes.len().min(NET_DVR_LOGIN_USERNAME_MAX_LEN - 1);
        login_info.sUserName[..n].copy_from_slice(unsafe { &*(&user_bytes[..n] as *const [u8] as *const [i8]) });

        // Copy password
        let pw_bytes = password.as_bytes();
        let n = pw_bytes.len().min(NET_DVR_LOGIN_PASSWD_MAX_LEN - 1);
        login_info.sPassword[..n].copy_from_slice(unsafe { &*(&pw_bytes[..n] as *const [u8] as *const [i8]) });

        login_info.wPort = port;
        login_info.bUseAsynLogin = 0;     // FALSE = synchronous
        login_info.byProxyType = 0;
        login_info.byLoginMode = 0;       // 0=Private
        login_info.byHttps = 0;           // 0=tcp

        log::info!("Calling NET_DVR_Login_V40: host={}, port={}, user={}, pass_len={}",
            host, port, username, password.len());

        let user_id = unsafe { (self._login_v40)(&mut login_info, &mut device_info) };

        if user_id < 0 {
            let err = self.get_last_error();
            log::error!("NET_DVR_Login_V40 failed with error code: {}", err);
            anyhow::bail!("NET_DVR_Login_V40 failed (error {}). Check credentials and SDK version.", err);
        }

        log::info!("Login succeeded, user_id={}, channels={}, zero_chan={}",
            user_id, device_info.struDeviceV30.byChanNum, device_info.struDeviceV30.byZeroChanNum);
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

    /// Start real-time preview with callback (modo original, com PlayM4)
    pub fn realplay(&self, user_id: LONG, channel: LONG, stream_type: DWORD, callback: REALDATACALLBACK, user_data: *mut c_void) -> Result<LONG> {
        let mut preview_info: NET_DVR_PREVIEWINFO = unsafe { std::mem::zeroed() };

        preview_info.lChannel = channel;
        preview_info.dwStreamType = stream_type;
        preview_info.dwLinkMode = LINK_TCP;
        preview_info.hPlayWnd = 0;
        preview_info.bBlocked = 1;

        let handle = unsafe { (self._realplay_v40)(user_id, &preview_info, Some(callback), user_data) };

        if handle < 0 {
            let err = self.get_last_error();
            anyhow::bail!("NET_DVR_RealPlay_V40 failed (error {})", err);
        }

        log::info!("RealPlay started, handle={}", handle);
        Ok(handle)
    }

    /// Start real-time preview com callback (non-blocking, para multi-câmera).
    ///
    /// hPlayWnd=0, bBlocked=0 — o SDK retorna imediatamente e chama o callback
    /// com dados de áudio/vídeo em tempo real. Útil para split-screen multi-câmera.
    pub fn realplay_with_callback(
        &self,
        user_id: LONG,
        channel: LONG,
        stream_type: DWORD,
        link_mode: DWORD,
        callback: REALDATACALLBACK,
        user_data: *mut c_void,
    ) -> Result<LONG> {
        let mut preview_info: NET_DVR_PREVIEWINFO = unsafe { std::mem::zeroed() };

        preview_info.lChannel = channel;
        preview_info.dwStreamType = stream_type;
        preview_info.dwLinkMode = link_mode;
        preview_info.hPlayWnd = 0;
        preview_info.bBlocked = 0;
        preview_info.dwDisplayBufNum = 1;

        let handle = unsafe { (self._realplay_v40)(user_id, &preview_info, Some(callback), user_data) };

        if handle < 0 {
            let err = self.get_last_error();
            anyhow::bail!("NET_DVR_RealPlay_V40 (callback) failed (error {})", err);
        }

        log::info!("RealPlay (callback) started, channel={}, handle={}", channel, handle);
        Ok(handle)
    }

    /// Start real-time preview com janela X11 direta (conforme ARCHITECTURE.md).
    ///
    /// O SDK renderiza vídeo diretamente na janela via overlay — sem callback.
    /// A janela X11 DEVE estar mapeada (visível) antes desta chamada.
    pub fn realplay_with_window(&self, user_id: LONG, channel: LONG, stream_type: DWORD, hwnd: HWND) -> Result<LONG> {
        self.realplay_with_window_ex(user_id, channel, stream_type, LINK_TCP, hwnd)
    }

    /// Como `realplay_with_window` mas permite especificar `link_mode`.
    pub fn realplay_with_window_ex(&self, user_id: LONG, channel: LONG, stream_type: DWORD, link_mode: DWORD, hwnd: HWND) -> Result<LONG> {
        self.realplay_with_window_ex2(user_id, channel, stream_type, link_mode, 0, hwnd)
    }

    /// Como `realplay_with_window_ex` mas permite especificar `link_mode` e `preview_mode`.
    pub fn realplay_with_window_ex2(&self, user_id: LONG, channel: LONG, stream_type: DWORD, link_mode: DWORD, preview_mode: u8, hwnd: HWND) -> Result<LONG> {
        self.realplay_with_window_ex3(user_id, channel, stream_type, link_mode, preview_mode, 0, 0, hwnd)
    }

    /// Como `realplay_with_window_ex2` mas permite especificar `data_type` e `proto_type`.
    /// - `data_type`: 0=private (default), 1=standard (PS stream)
    /// - `proto_type`: 0=private (default), 1=RTSP
    pub fn realplay_with_window_ex3(&self, user_id: LONG, channel: LONG, stream_type: DWORD, link_mode: DWORD, preview_mode: u8, data_type: u8, proto_type: u8, hwnd: HWND) -> Result<LONG> {
        let mut preview_info: NET_DVR_PREVIEWINFO = unsafe { std::mem::zeroed() };

        preview_info.lChannel = channel;
        preview_info.dwStreamType = stream_type;
        preview_info.dwLinkMode = link_mode;
        preview_info.hPlayWnd = hwnd;
        preview_info.bBlocked = 1;
        preview_info.dwDisplayBufNum = 1;
        preview_info.byPreviewMode = preview_mode;
        preview_info.byDataType = data_type;
        preview_info.byProtoType = proto_type;

        let handle = unsafe { (self._realplay_v40)(user_id, &preview_info, None, std::ptr::null_mut()) };

        if handle < 0 {
            let err = self.get_last_error();
            anyhow::bail!("NET_DVR_RealPlay_V40 (window) failed (error {})", err);
        }

        log::info!("RealPlay (window) started, handle={}, channel={}, stream={}, link={}, pm={}, dt={}, pt={}, hwnd=0x{:x}",
            handle, channel, stream_type, link_mode, preview_mode, data_type, proto_type, hwnd);
        Ok(handle)
    }

    /// API V30 antiga — mais permissiva que a V40, pode funcionar quando V40 falha.
    /// lLinkMode: bit31=0 main, bit31=1 sub; bits0-30=link_mode (0=TCP).
    pub fn realplay_v30_with_window(&self, user_id: LONG, channel: LONG, is_sub_stream: bool, hwnd: HWND) -> Result<LONG> {
        let mut ci: NET_DVR_CLIENTINFO = unsafe { std::mem::zeroed() };

        ci.lChannel = channel;
        ci.lLinkMode = if is_sub_stream { i32::MIN } else { 0i32 };  // bit31 = stream_type
        ci.hPlayWnd = hwnd;
        ci.sMultiCastIP = std::ptr::null_mut();
        ci.byProtoType = 0;

        let handle = unsafe { (self._realplay_v30)(user_id, &ci, None, std::ptr::null_mut(), 1) };

        if handle < 0 {
            let err = self.get_last_error();
            anyhow::bail!("NET_DVR_RealPlay_V30 (window) failed (error {})", err);
        }

        log::info!("RealPlay V30 started, handle={}, channel={}, sub={}, hwnd=0x{:x}",
            handle, channel, is_sub_stream, hwnd);
        Ok(handle)
    }

    /// API V30 com byProtoType=1 (RTSP) — variação que pode funcionar quando
    /// byProtoType=0 (private) falha, especialmente para canais virtuais/IP.
    pub fn realplay_v30_with_window_rtsp(&self, user_id: LONG, channel: LONG, is_sub_stream: bool, hwnd: HWND) -> Result<LONG> {
        let mut ci: NET_DVR_CLIENTINFO = unsafe { std::mem::zeroed() };

        ci.lChannel = channel;
        ci.lLinkMode = if is_sub_stream { i32::MIN } else { 0i32 };
        ci.hPlayWnd = hwnd;
        ci.sMultiCastIP = std::ptr::null_mut();
        ci.byProtoType = 1;  // 1=RTSP

        let handle = unsafe { (self._realplay_v30)(user_id, &ci, None, std::ptr::null_mut(), 1) };

        if handle < 0 {
            let err = self.get_last_error();
            anyhow::bail!("NET_DVR_RealPlay_V30_RTSP (window) failed (error {})", err);
        }

        log::info!("RealPlay V30 RTSP started, handle={}, channel={}, sub={}, hwnd=0x{:x}",
            handle, channel, is_sub_stream, hwnd);
        Ok(handle)
    }

    /// NET_DVR_RealPlaySpecial: preview com URL RTSP customizada.
    /// Permite passar uma URL RTSP arbitrária (ex: com ?zeroChannel=1).
    pub fn realplay_special(&self, user_id: LONG, url: &str, link_mode: DWORD, hwnd: HWND) -> Result<LONG> {
        let mut info: NET_DVR_PREVIEWINFO_SPECIAL = unsafe { std::mem::zeroed() };

        let url_bytes = url.as_bytes();
        let n = url_bytes.len().min(1023);
        info.sURL[..n].copy_from_slice(unsafe { &*(&url_bytes[..n] as *const [u8] as *const [i8]) });
        info.dwLinkMode = link_mode;
        info.hPlayWnd = hwnd;
        info.bBlocked = 1;
        info.dwDisplayBufNum = 1;

        let handle = unsafe { (self._realplay_special)(user_id, &info, None, std::ptr::null_mut()) };

        if handle < 0 {
            let err = self.get_last_error();
            anyhow::bail!("NET_DVR_RealPlaySpecial failed (error {})", err);
        }

        log::info!("RealPlaySpecial started, handle={}, url={}, link={}, hwnd=0x{:x}",
            handle, url, link_mode, hwnd);
        Ok(handle)
    }

    /// NET_DVR_RealPlaySpecial com data callback em vez de janela.
    /// Útil quando a renderização em janela falha, permite processar
    /// o stream manualmente (ex: via PlayM4).
    pub fn realplay_special_with_callback(
        &self,
        user_id: LONG,
        url: &str,
        link_mode: DWORD,
        callback: REALDATACALLBACK,
        user_data: *mut c_void,
    ) -> Result<LONG> {
        let mut info: NET_DVR_PREVIEWINFO_SPECIAL = unsafe { std::mem::zeroed() };

        let url_bytes = url.as_bytes();
        let n = url_bytes.len().min(1023);
        info.sURL[..n].copy_from_slice(unsafe { &*(&url_bytes[..n] as *const [u8] as *const [i8]) });
        info.dwLinkMode = link_mode;
        info.hPlayWnd = 0;
        info.bBlocked = 0;
        info.dwDisplayBufNum = 1;

        let handle = unsafe { (self._realplay_special)(user_id, &info, Some(callback), user_data) };

        if handle < 0 {
            let err = self.get_last_error();
            anyhow::bail!("NET_DVR_RealPlaySpecial (callback) failed (error {})", err);
        }

        log::info!("RealPlaySpecial (callback) started, handle={}, url={}, link={}",
            handle, url, link_mode);
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

    /// Query device ability via XML.
    /// - `ability_type`: e.g. `DEVICE_ABILITY_INFO` or `DEVICE_DYNCHAN_ABILITY`
    /// - `input_xml`: XML string for the specific ability node (e.g. `<PreviewSwitchAbility>`)
    ///   or empty for ability types that don't need input
    /// Returns the XML response from the device.
    pub fn get_device_ability(&self, user_id: LONG, ability_type: DWORD, input_xml: &str) -> Result<String> {
        let in_c = CString::new(input_xml).context("input_xml contains null byte")?;
        let mut out_buf = vec![0u8; 2048];

        let ret = unsafe {
            (self._get_device_ability)(
                user_id,
                ability_type,
                in_c.as_ptr(),
                input_xml.len() as DWORD,
                out_buf.as_mut_ptr() as *mut c_char,
                out_buf.len() as DWORD,
            )
        };

        if ret == 0 {
            let err = self.get_last_error();
            anyhow::bail!("NET_DVR_GetDeviceAbility failed (error {})", err);
        }

        // Find the null terminator
        let len = out_buf.iter().position(|&b| b == 0).unwrap_or(out_buf.len());
        let xml = String::from_utf8_lossy(&out_buf[..len]).to_string();
        log::info!("GetDeviceAbility type=0x{:x} returned {} bytes XML", ability_type, len);
        Ok(xml)
    }

    /// Query device ability with raw binary input (e.g. DWORD channel number for DYNCHAN_ABILITY).
    pub fn get_device_ability_raw(&self, user_id: LONG, ability_type: DWORD, input: &[u8]) -> Result<String> {
        let mut out_buf = vec![0u8; 2048];

        let ret = unsafe {
            (self._get_device_ability)(
                user_id,
                ability_type,
                input.as_ptr() as *const c_char,
                input.len() as DWORD,
                out_buf.as_mut_ptr() as *mut c_char,
                out_buf.len() as DWORD,
            )
        };

        if ret == 0 {
            let err = self.get_last_error();
            anyhow::bail!("NET_DVR_GetDeviceAbility failed (error {})", err);
        }

        let len = out_buf.iter().position(|&b| b == 0).unwrap_or(out_buf.len());
        let xml = String::from_utf8_lossy(&out_buf[..len]).to_string();
        log::info!("GetDeviceAbility raw type=0x{:x} returned {} bytes XML", ability_type, len);
        Ok(xml)
    }
    /// Generic NET_DVR_GetDVRConfig: reads device configuration struct.
    /// `cfg` should have `dwSize` set (first field) before calling.
    /// `command`: SDK command code (e.g. NET_DVR_GET_ZEROCHANCFG).
    /// `channel`: channel number (0 for global configs).
    pub fn get_dvr_config<T: Sized>(&self, user_id: LONG, command: DWORD, channel: LONG, cfg: &mut T) -> Result<()> {
        let size = std::mem::size_of::<T>() as u32;
        let mut bytes_returned: DWORD = 0;

        let ret = unsafe {
            (self._get_dvr_config)(
                user_id,
                command,
                channel,
                cfg as *mut _ as *mut c_void,
                size,
                &mut bytes_returned,
            )
        };

        if ret == 0 {
            let err = self.get_last_error();
            anyhow::bail!("NET_DVR_GetDVRConfig(command=0x{:x}) failed (error {})", command, err);
        }

        log::info!("NET_DVR_GetDVRConfig(command=0x{:x}) returned {} bytes", command, bytes_returned);
        Ok(())
    }

    /// Generic NET_DVR_SetDVRConfig: writes device configuration struct.
    pub fn set_dvr_config<T: Sized>(&self, user_id: LONG, command: DWORD, channel: LONG, cfg: &T) -> Result<()> {
        let size = std::mem::size_of::<T>() as u32;

        let ret = unsafe {
            (self._set_dvr_config)(
                user_id,
                command,
                channel,
                cfg as *const _ as *const c_void,
                size,
            )
        };

        if ret == 0 {
            let err = self.get_last_error();
            anyhow::bail!("NET_DVR_SetDVRConfig(command=0x{:x}) failed (error {})", command, err);
        }

        log::info!("NET_DVR_SetDVRConfig(command=0x{:x}) succeeded", command);
        Ok(())
    }

    /// Probe and trigger a key frame on the zero channel.
    /// `zero_chan`: the zero channel number to probe.
    /// Returns true if the device accepted it (likely the correct zero channel number).
    pub fn zero_make_key_frame(&self, user_id: LONG, zero_chan: LONG) -> Result<bool> {
        let ret = unsafe { (self._zero_make_key_frame)(user_id, zero_chan) };
        if ret != 0 {
            log::info!("NET_DVR_ZeroMakeKeyFrame({}) succeeded", zero_chan);
            Ok(true)
        } else {
            let err = self.get_last_error();
            log::warn!("NET_DVR_ZeroMakeKeyFrame({}) failed (error {})", zero_chan, err);
            Ok(false)
        }
    }

    pub fn ensure_zero_channel_enabled(&self, user_id: LONG, force_enable: bool) -> Result<(bool, bool)> {
        let mut cfg: NET_DVR_ZEROCHANCFG = unsafe { std::mem::zeroed() };
        cfg.dwSize = std::mem::size_of::<NET_DVR_ZEROCHANCFG>() as u32;
        let mut bytes_returned: DWORD = 0;

        let ret = unsafe {
            (self._get_dvr_config)(
                user_id,
                NET_DVR_GET_ZEROCHANCFG,
                0,
                &mut cfg as *mut _ as *mut c_void,
                cfg.dwSize,
                &mut bytes_returned,
            )
        };

        if ret == 0 {
            let err = self.get_last_error();
            anyhow::bail!("NET_DVR_GetDVRConfig(ZEROCHANCFG) failed (error {})", err);
        }

        if cfg.byEnable == 1 {
            log::info!("Canal Zero already enabled (byEnable=1)");
            return Ok((true, false));
        }

        if !force_enable {
            log::warn!("Canal Zero is disabled (byEnable=0) and force_enable=false");
            return Ok((false, false));
        }

        log::info!("Canal Zero disabled (byEnable=0), enabling via SDK...");
        cfg.byEnable = 1;

        let ret = unsafe {
            (self._set_dvr_config)(
                user_id,
                NET_DVR_SET_ZEROCHANCFG,
                0,
                &cfg as *const _ as *const c_void,
                cfg.dwSize,
            )
        };

        if ret == 0 {
            let err = self.get_last_error();
            anyhow::bail!("NET_DVR_SetDVRConfig(ZEROCHANCFG) failed (error {})", err);
        }

        log::info!("Canal Zero enabled via SDK successfully");
        Ok((true, true))
    }
}

impl Drop for HCNetSDK {
    fn drop(&mut self) {
        self.cleanup();
    }
}
