//! karukan Windows frontend: a TSF text service DLL.
//!
//! All IME state lives in `karukan-imserver` (one server shared by every app over a
//! named pipe, or a private child process, see [`engine`]); this crate translates Windows key events and
//! applies the engine's UI actions through TSF.

pub mod engine;
pub mod keymap;

#[cfg(windows)]
mod attr;
#[cfg(windows)]
mod cand;
#[cfg(windows)]
mod factory;
#[cfg(windows)]
mod guids;
#[cfg(windows)]
mod register;
#[cfg(windows)]
mod shared;
#[cfg(windows)]
mod tip;

#[cfg(windows)]
mod dll {
    use std::ffi::c_void;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicIsize, Ordering};

    use windows::Win32::Foundation::{
        CLASS_E_CLASSNOTAVAILABLE, E_POINTER, HINSTANCE, HMODULE, S_FALSE, S_OK,
    };
    use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
    use windows::core::{BOOL, GUID, HRESULT, Interface};

    use crate::factory::Factory;
    use crate::guids::CLSID_KARUKAN;

    static MODULE: AtomicIsize = AtomicIsize::new(0);

    pub fn module() -> HMODULE {
        HMODULE(MODULE.load(Ordering::Relaxed) as *mut c_void)
    }

    /// Directory containing this DLL.
    pub fn dll_dir() -> Option<PathBuf> {
        let mut buf = [0u16; 1024];
        let n = unsafe { GetModuleFileNameW(Some(module()), &mut buf) } as usize;
        if n == 0 || n >= buf.len() {
            return None;
        }
        let p = PathBuf::from(String::from_utf16_lossy(&buf[..n]));
        p.parent().map(|d| d.to_path_buf())
    }

    /// Path to register as the COM server when the loader DLL registers on this one's behalf.
    static REGISTER_AS: std::sync::OnceLock<String> = std::sync::OnceLock::new();

    pub fn dll_path() -> Option<String> {
        if let Some(p) = REGISTER_AS.get() {
            return Some(p.clone());
        }
        let mut buf = [0u16; 1024];
        let n = unsafe { GetModuleFileNameW(Some(module()), &mut buf) } as usize;
        (n > 0 && n < buf.len()).then(|| String::from_utf16_lossy(&buf[..n]))
    }

    #[unsafe(no_mangle)]
    extern "system" fn DllMain(hinst: HINSTANCE, reason: u32, _reserved: *mut c_void) -> BOOL {
        const DLL_PROCESS_ATTACH: u32 = 1;
        if reason == DLL_PROCESS_ATTACH {
            MODULE.store(hinst.0 as isize, Ordering::Relaxed);
        }
        BOOL(1)
    }

    #[unsafe(no_mangle)]
    extern "system" fn DllGetClassObject(
        rclsid: *const GUID,
        riid: *const GUID,
        ppv: *mut *mut c_void,
    ) -> HRESULT {
        if rclsid.is_null() || riid.is_null() || ppv.is_null() {
            return E_POINTER;
        }
        unsafe {
            *ppv = std::ptr::null_mut();
            if *rclsid != CLSID_KARUKAN {
                return CLASS_E_CLASSNOTAVAILABLE;
            }
            let f: windows::Win32::System::Com::IClassFactory = Factory.into();
            f.query(riid, ppv)
        }
    }

    #[unsafe(no_mangle)]
    extern "system" fn DllCanUnloadNow() -> HRESULT {
        // Never unload: TSF can hold references we do not track.
        S_FALSE
    }

    #[unsafe(no_mangle)]
    extern "system" fn DllRegisterServer() -> HRESULT {
        match crate::register::register() {
            Ok(()) => S_OK,
            Err(e) => e.code(),
        }
    }

    /// Called by `karukan_tsf.dll` (the loader): register that file as the COM server.
    #[unsafe(no_mangle)]
    extern "system" fn KarukanTsfRegisterAs(path: *const u16) -> HRESULT {
        if path.is_null() {
            return E_POINTER;
        }
        let len = (0..).take_while(|&i| unsafe { *path.add(i) } != 0).count();
        let p = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(path, len) });
        let _ = REGISTER_AS.set(p);
        DllRegisterServer()
    }

    #[unsafe(no_mangle)]
    extern "system" fn DllUnregisterServer() -> HRESULT {
        match crate::register::unregister() {
            Ok(()) => S_OK,
            Err(e) => e.code(),
        }
    }
}

#[cfg(windows)]
pub(crate) use dll::{dll_dir, dll_path};

/// Test hook: a fresh text service object.
#[cfg(windows)]
#[doc(hidden)]
pub fn __create_text_service() -> windows::core::IUnknown {
    tip::TextService::new().into()
}

/// Test hook: the text left of the caret, as the engine is told it.
#[cfg(windows)]
#[doc(hidden)]
pub fn __read_left_context(
    ctx: &windows::Win32::UI::TextServices::ITfContext,
    tid: u32,
) -> Option<String> {
    tip::read_left_context(ctx, tid)
}

/// Test hook: render the candidate window offscreen as BGRA.
#[cfg(windows)]
#[doc(hidden)]
#[allow(clippy::too_many_arguments)]
pub fn __render_candidate_preview(
    items: &[engine::CandidateItem],
    cursor: usize,
    page: usize,
    total_pages: usize,
    aux: Option<String>,
    typed: Option<String>,
    dark: bool,
    scale: f32,
) -> (i32, i32, Vec<u8>) {
    cand::render_preview(items, cursor, page, total_pages, aux, typed, dark, scale)
}
