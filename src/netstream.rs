use anyhow::{Context, Result};
use libloading::Library;
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub fn default_library_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    PathBuf::from(home).join(".local/share/hikvision/weblocalserver/files/bin/libnet_stream.so.1.0.0")
}

pub fn default_lib_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    PathBuf::from(home).join(".local/share/hikvision/weblocalserver/files")
}

pub fn setup_ld_path() {
    let dir = default_lib_dir();
    let bin = dir.join("files/bin");
    let lib = dir.join("files/lib");
    unsafe {
        let current = std::env::var("LD_LIBRARY_PATH").unwrap_or_default();
        let new = format!("{}:{}:{}", lib.display(), bin.display(), current);
        std::env::set_var("LD_LIBRARY_PATH", &new);
    }
}

pub struct NetStream {
    _lib: Library,
    key_idx: Mutex<i32>,
}

impl NetStream {
    pub fn load() -> Result<Self> {
        Self::load_from(&default_library_path())
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        setup_ld_path();
        let lib: Library = unsafe { Library::new(path) }
            .with_context(|| format!("Failed to load: {}", path.display()))?;
        log::info!("Loaded libnet_stream.so from {}", path.display());

        // Initialize the library (calls HPR_Init, InitDependDsoPath, loads sub-libs)
        let init_lib = unsafe {
            lib.get::<unsafe extern "C" fn(*const libc::c_char) -> i32>(b"NS_InitLib")
                .context("NS_InitLib symbol")?
        };
        let base_dir = CString::new(
            default_lib_dir().to_str().context("invalid base dir")?
        ).context("null byte in base dir")?;
        let ret = unsafe { init_lib(base_dir.as_ptr()) };
        if ret != 0 {
            anyhow::bail!("NS_InitLib failed: ret={}", ret);
        }
        log::info!("NS_InitLib({}) = {}", base_dir.to_str().unwrap_or("?"), ret);

        Ok(Self {
            _lib: lib,
            key_idx: Mutex::new(0),
        })
    }

    pub fn set_secret_key(&self, key_idx: i32, key: &str) -> Result<()> {
        let func = unsafe {
            self._lib
                .get::<unsafe extern "C" fn(i32, *const libc::c_char) -> i32>(b"NS_SetSecretKey")
                .context("NS_SetSecretKey symbol")?
        };
        let c_key = CString::new(key).context("key contains null byte")?;
        let ret = unsafe { func(key_idx, c_key.as_ptr()) };
        if ret == 0 {
            *self.key_idx.lock().unwrap() = key_idx;
            Ok(())
        } else {
            anyhow::bail!("NS_SetSecretKey failed: ret={}", ret);
        }
    }

    pub fn decrypt(&self, input: &[u8], extra: &[u8], flags: i32) -> Result<Vec<u8>> {
        let key_idx = *self.key_idx.lock().unwrap();

        let func = unsafe {
            self._lib
                .get::<unsafe extern "C" fn(i32, *const libc::c_char, *mut libc::c_char, *mut i32, *const libc::c_char, i32) -> i32>(
                    b"NS_Decrypt",
                )
                .context("NS_Decrypt symbol")?
        };

        // NS_Decrypt expects the NAL unit bytes (body after the 2-byte NAL header)
        // Input: the encrypted data
        // Output: will be resized according to out_size
        let mut output = vec![0u8; input.len().max(1024).saturating_mul(2)];
        let mut out_size = output.len() as i32;

        let extra_c = if extra.is_empty() {
            None
        } else {
            Some(CString::new(extra).context("extra contains null byte")?)
        };
        let extra_ptr = match extra_c {
            Some(ref c) => c.as_ptr(),
            None => std::ptr::null(),
        };

        let input_ptr = input.as_ptr() as *const libc::c_char;
        let output_ptr = output.as_mut_ptr() as *mut libc::c_char;

        let ret = unsafe { func(key_idx, input_ptr, output_ptr, &mut out_size, extra_ptr, flags) };

        if ret == 0 {
            if out_size > 0 {
                output.truncate(out_size as usize);
            } else {
                output.clear();
            }
            Ok(output)
        } else {
            anyhow::bail!("NS_Decrypt failed: ret={}, out_size={}", ret, out_size);
        }
    }
}
