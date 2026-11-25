//! A running text service hands over to a newer copy installed beside it,
//! without the app restarting: the built DLL is copied under two stamped
//! names, the older one is activated by hand, and gaining focus makes it
//! load and activate the newer one from the thread's message loop. A newer
//! file that cannot be loaded leaves the running copy as it was.
//!
//! Needs `KARUKAN_TSF_DEBUG` (the unregistered service cannot advise its key
//! sink); skipped otherwise.
#![cfg(windows)]

use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, IClassFactory,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows::Win32::UI::TextServices::{
    CLSID_TF_ThreadMgr, ITfKeyEventSink, ITfTextInputProcessorEx, ITfThreadMgr,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage,
};
use windows::core::{GUID, HRESULT, Interface, PCWSTR, s};

const CLSID_KARUKAN: GUID = GUID::from_u128(0x4b9f6e5a_1d3c_4c7e_9a52_8e21f0c7a311);

fn wide(p: &Path) -> Vec<u16> {
    p.as_os_str().encode_wide().chain(Some(0)).collect()
}

/// The DLL cargo built beside the test binary's folder.
fn built_dll() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    exe.parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("karukan_tsf.dll")
}

fn loaded(p: &Path) -> bool {
    unsafe { GetModuleHandleW(PCWSTR(wide(p).as_ptr())).is_ok() }
}

/// The text service of the DLL at `p`, loaded by path.
fn text_service(p: &Path) -> ITfTextInputProcessorEx {
    type GetClassObject =
        unsafe extern "system" fn(*const GUID, *const GUID, *mut *mut core::ffi::c_void) -> HRESULT;
    unsafe {
        let m = LoadLibraryW(PCWSTR(wide(p).as_ptr())).expect("load");
        let f = GetProcAddress(m, s!("DllGetClassObject")).expect("export");
        let get: GetClassObject = std::mem::transmute(f);
        let mut cf = std::ptr::null_mut();
        get(&CLSID_KARUKAN, &IClassFactory::IID, &mut cf)
            .ok()
            .expect("class object");
        IClassFactory::from_raw(cf)
            .CreateInstance(None)
            .expect("text service")
    }
}

fn pump(d: Duration) {
    let end = Instant::now() + d;
    while Instant::now() < end {
        unsafe {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None::<HWND>, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("karukan-swap-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn a_running_copy_hands_over_to_a_newer_one() {
    if std::env::var_os("KARUKAN_TSF_DEBUG").is_none() {
        eprintln!("skipped: needs KARUKAN_TSF_DEBUG");
        return;
    }
    let built = built_dll();
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let tm: ITfThreadMgr =
            CoCreateInstance(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER).unwrap();
        let tid = tm.Activate().unwrap();

        // a newer copy beside it: taken over on focus
        let d = dir("ok");
        let (old, new) = (
            d.join("karukan_tsf_impl_100.dll"),
            d.join("karukan_tsf_impl_200.dll"),
        );
        std::fs::copy(&built, &old).unwrap();
        let tip = text_service(&old);
        tip.ActivateEx(&tm, tid, 0).unwrap();
        std::fs::copy(&built, &new).unwrap();
        assert!(!loaded(&new));
        let ks: ITfKeyEventSink = tip.cast().unwrap();
        ks.OnSetFocus(true).unwrap();
        pump(Duration::from_millis(500));
        assert!(loaded(&new), "the newer copy was not loaded");
        // TSF's Deactivate reaches the copy now running
        tip.Deactivate().unwrap();

        // a newer file that is not a DLL: the running copy stays
        let d = dir("bad");
        let (old, bad) = (
            d.join("karukan_tsf_impl_300.dll"),
            d.join("karukan_tsf_impl_400.dll"),
        );
        std::fs::copy(&built, &old).unwrap();
        let tip = text_service(&old);
        tip.ActivateEx(&tm, tid, 0).unwrap();
        std::fs::write(&bad, b"not a dll").unwrap();
        let ks: ITfKeyEventSink = tip.cast().unwrap();
        ks.OnSetFocus(true).unwrap();
        pump(Duration::from_millis(500));
        assert!(!loaded(&bad));
        // still answering, and not trying again
        ks.OnSetFocus(true).unwrap();
        pump(Duration::from_millis(200));
        tip.Deactivate().unwrap();
        let _ = tm.Deactivate();
    }
}
