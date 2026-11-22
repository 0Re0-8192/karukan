//! The DLL that is registered with COM (`karukan_tsf.dll`). It holds no IME code: every
//! `DllGetClassObject` loads the newest `karukan_tsf_impl*.dll` beside it and forwards the call.
//!
//! A DLL that an app has loaded cannot be replaced, which is why updating the IME needed an app
//! restart. This shim never changes; an update adds a new `karukan_tsf_impl_<stamp>.dll`, and the
//! next time TSF activates the text service in an app (switching the input method away and back,
//! or opening a new window thread) that app gets the new code.
#![cfg(windows)]

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicIsize, Ordering};

use windows::Win32::Foundation::{
    CLASS_E_CLASSNOTAVAILABLE, E_FAIL, E_POINTER, HINSTANCE, HMODULE, S_FALSE,
};
use windows::Win32::System::LibraryLoader::{
    GetModuleFileNameW, GetProcAddress, LOAD_WITH_ALTERED_SEARCH_PATH, LoadLibraryExW,
};
use windows::core::{BOOL, GUID, HRESULT, PCSTR, PCWSTR, s};

static MODULE: AtomicIsize = AtomicIsize::new(0);

type GetClassObject =
    unsafe extern "system" fn(*const GUID, *const GUID, *mut *mut c_void) -> HRESULT;
type Plain = unsafe extern "system" fn() -> HRESULT;
type RegisterAs = unsafe extern "system" fn(*const u16) -> HRESULT;

fn own_path() -> Option<PathBuf> {
    let mut buf = [0u16; 1024];
    let n = unsafe {
        GetModuleFileNameW(
            Some(HMODULE(MODULE.load(Ordering::Relaxed) as *mut c_void)),
            &mut buf,
        )
    } as usize;
    (n > 0 && n < buf.len()).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..n])))
}

/// The newest `karukan_tsf_impl*.dll` next to this file (names sort by their date stamp).
fn newest_impl() -> Option<PathBuf> {
    let dir = own_path()?.parent()?.to_path_buf();
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let n = p
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            n.starts_with("karukan_tsf_impl") && n.ends_with(".dll")
        })
        .max_by_key(|p| p.file_name().map(|n| n.to_ascii_lowercase()))
}

/// Loads the newest implementation (loading the same file twice just adds a reference).
fn load() -> Option<HMODULE> {
    let path = newest_impl()?;
    let w: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe { LoadLibraryExW(PCWSTR(w.as_ptr()), None, LOAD_WITH_ALTERED_SEARCH_PATH).ok() }
}

unsafe fn proc<T: Copy>(m: HMODULE, name: PCSTR) -> Option<T> {
    let p = unsafe { GetProcAddress(m, name) }?;
    Some(unsafe { std::mem::transmute_copy::<_, T>(&p) })
}

#[unsafe(no_mangle)]
extern "system" fn DllMain(hinst: HINSTANCE, reason: u32, _reserved: *mut c_void) -> BOOL {
    if reason == 1 {
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
    unsafe { *ppv = std::ptr::null_mut() };
    let Some(m) = load() else {
        return CLASS_E_CLASSNOTAVAILABLE;
    };
    match unsafe { proc::<GetClassObject>(m, s!("DllGetClassObject")) } {
        Some(f) => unsafe { f(rclsid, riid, ppv) },
        None => CLASS_E_CLASSNOTAVAILABLE,
    }
}

#[unsafe(no_mangle)]
extern "system" fn DllCanUnloadNow() -> HRESULT {
    S_FALSE
}

fn forward_register(export: PCSTR) -> HRESULT {
    let (Some(m), Some(me)) = (load(), own_path()) else {
        return E_FAIL;
    };
    // the registry must point at this file, not at the implementation
    let w: Vec<u16> = me.as_os_str().encode_wide().chain(Some(0)).collect();
    match unsafe { proc::<RegisterAs>(m, export) } {
        Some(f) => unsafe { f(w.as_ptr()) },
        None => E_FAIL,
    }
}

#[unsafe(no_mangle)]
extern "system" fn DllRegisterServer() -> HRESULT {
    forward_register(s!("KarukanTsfRegisterAs"))
}

#[unsafe(no_mangle)]
extern "system" fn DllUnregisterServer() -> HRESULT {
    let Some(m) = load() else { return E_FAIL };
    match unsafe { proc::<Plain>(m, s!("DllUnregisterServer")) } {
        Some(f) => unsafe { f() },
        None => E_FAIL,
    }
}
