//! One server for every app: two text services in two "apps" (threads with a
//! thread manager each) reach the same server, and the second one's first key
//! does not wait for a server to start.
//!
//! Needs `KARUKAN_IMSERVER` (a copy of karukan-imserver.exe under a name no
//! other server has: running servers are counted by it) and `KARUKAN_TSF_DEBUG`.
//! `KARUKAN_SHARED=1` is set here, so this test lives in its own binary.
#![cfg(windows)]

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::Win32::UI::TextServices::{
    CLSID_TF_ThreadMgr, ITfContext, ITfEditSession, ITfEditSession_Impl, ITfKeyEventSink,
    ITfTextInputProcessorEx, ITfThreadMgr, TF_ANCHOR_START, TF_ES_READWRITE, TF_ES_SYNC,
};
use windows::core::{Interface, Result, implement};

#[implement(ITfEditSession)]
struct ReadAll {
    ctx: ITfContext,
    out: Rc<RefCell<String>>,
}

impl ITfEditSession_Impl for ReadAll_Impl {
    fn DoEditSession(&self, ec: u32) -> Result<()> {
        unsafe {
            let r = self.ctx.GetStart(ec)?;
            r.Collapse(ec, TF_ANCHOR_START)?;
            let mut halted = 0i32;
            r.ShiftEnd(ec, 4096, &mut halted, std::ptr::null())?;
            let mut buf = [0u16; 64];
            let mut got = 0u32;
            r.GetText(ec, 0, &mut buf, &mut got)?;
            *self.out.borrow_mut() = String::from_utf16_lossy(&buf[..got as usize]);
        }
        Ok(())
    }
}

fn running(name: &str) -> usize {
    let out = std::process::Command::new("tasklist")
        .args(["/FI", &format!("IMAGENAME eq {name}"), "/NH"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).matches(name).count()
}

/// One "app": activate the text service, type `kyou`, commit with Enter.
/// Returns the first key's time and the committed text.
fn app() -> (Duration, String) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let tm: ITfThreadMgr =
            CoCreateInstance(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER).unwrap();
        let tid = tm.Activate().unwrap();
        let tip: ITfTextInputProcessorEx = karukan_tsf::__create_text_service().cast().unwrap();
        tip.ActivateEx(&tm, tid, 0).unwrap();
        let doc = tm.CreateDocumentMgr().unwrap();
        let mut ctx = None;
        let mut ec = 0u32;
        doc.CreateContext(tid, 0, None, &mut ctx, &mut ec).unwrap();
        let ctx = ctx.unwrap();
        doc.Push(&ctx).unwrap();
        let _ = tm.SetFocus(&doc);
        let sink: ITfKeyEventSink = tip.cast().unwrap();
        let key = |vk: u32| {
            sink.OnKeyDown(&ctx, WPARAM(vk as usize), LPARAM(0))
                .map(|b| b.as_bool())
                .unwrap_or(false)
        };
        let t = Instant::now();
        assert!(key(0x4B), "k not consumed");
        let first = t.elapsed();
        for vk in [0x59, 0x4F, 0x55] {
            assert!(key(vk));
        }
        assert!(key(0x76)); // F7: katakana, so the text does not depend on the model
        assert!(key(0x0D));
        let out = Rc::new(RefCell::new(String::new()));
        let session: ITfEditSession = ReadAll {
            ctx: ctx.clone(),
            out: out.clone(),
        }
        .into();
        let _ = ctx.RequestEditSession(tid, &session, TF_ES_SYNC | TF_ES_READWRITE);
        tip.Deactivate().unwrap();
        let _ = tm.Deactivate();
        (first, out.borrow().clone())
    }
}

#[test]
fn two_apps_share_one_server() {
    let Some(exe) = std::env::var_os("KARUKAN_IMSERVER") else {
        eprintln!("KARUKAN_IMSERVER not set; skipped");
        return;
    };
    // `KARUKAN_SHARED=auto` from outside runs the decision an installed text
    // service makes (integrity level); otherwise the shared server is forced.
    if std::env::var_os("KARUKAN_SHARED").is_none() {
        // SAFETY: set before any thread of this test binary reads the environment
        unsafe { std::env::set_var("KARUKAN_SHARED", "1") };
    }
    let name = std::path::Path::new(&exe)
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert_eq!(running(&name), 0, "another {name} is already running");

    let (first_a, text_a) = std::thread::spawn(app).join().unwrap();
    assert_eq!(text_a, "キョウ");
    // (the karukan registered on this PC may activate in this process too and
    // start a server of its own, so count what app B adds, not the total)
    let after_a = running(&name);
    assert!(after_a >= 1, "no server after the first app");

    let (first_b, text_b) = std::thread::spawn(app).join().unwrap();
    assert_eq!(text_b, "キョウ");
    assert_eq!(
        running(&name),
        after_a,
        "the second app started a server of its own"
    );
    eprintln!("first key: app A {first_a:?}, app B {first_b:?}");
    let servers = std::process::Command::new("wmic")
        .args([
            "process",
            "where",
            &format!("name='{name}'"),
            "get",
            "CommandLine",
        ])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    eprintln!(
        "servers: {}",
        servers
            .split_whitespace()
            .filter(|w| w.starts_with("karukan-S-"))
            .collect::<Vec<_>>()
            .join(" ")
    );
    assert!(
        first_b < Duration::from_millis(150),
        "app B waited {first_b:?}"
    );

    // the server outlives its apps (until it has been idle a while): stop it here
    let _ = std::process::Command::new("taskkill")
        .args(["/F", "/IM", &name])
        .output();
}
