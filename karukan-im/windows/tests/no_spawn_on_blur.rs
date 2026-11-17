//! Losing focus in an app where nothing was typed must not start a server:
//! each one holds the whole dictionary (~650 MB) and the start-up blocks the
//! host's UI thread.
//!
//! Needs `KARUKAN_IMSERVER`, whose file name is used to count running servers
//! (give it a name no other server has, e.g. a copy), and `KARUKAN_TSF_DEBUG`.
#![cfg(windows)]

use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::Win32::UI::TextServices::{
    CLSID_TF_ThreadMgr, ITfKeyEventSink, ITfTextInputProcessorEx, ITfThreadMgr,
};
use windows::core::Interface;

fn running(name: &str) -> usize {
    let out = std::process::Command::new("tasklist")
        .args(["/FI", &format!("IMAGENAME eq {name}"), "/NH"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).matches(name).count()
}

#[test]
fn blur_without_typing_starts_no_server() {
    let Some(exe) = std::env::var_os("KARUKAN_IMSERVER") else {
        eprintln!("KARUKAN_IMSERVER not set; skipped");
        return;
    };
    let name = std::path::Path::new(&exe)
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let tm: ITfThreadMgr =
            CoCreateInstance(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER).unwrap();
        let tid = tm.Activate().unwrap();
        let tip: ITfTextInputProcessorEx = karukan_tsf::__create_text_service().cast().unwrap();
        tip.ActivateEx(&tm, tid, 0).unwrap();
        let sink: ITfKeyEventSink = tip.cast().unwrap();
        assert_eq!(running(&name), 0, "another {name} is already running");

        let t = std::time::Instant::now();
        sink.OnSetFocus(false).unwrap();
        let blur_ms = t.elapsed().as_millis();
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert_eq!(running(&name), 0, "focus loss started a server");
        assert!(blur_ms < 200, "focus loss took {blur_ms} ms");

        tip.Deactivate().unwrap();
        assert_eq!(running(&name), 0);
    }
}
