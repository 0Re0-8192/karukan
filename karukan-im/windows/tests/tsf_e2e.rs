//! End-to-end check against the real TSF runtime, without registering the
//! DLL: a thread manager and a store-less context are created in-process,
//! the text service is activated by hand and driven through its key sink.
//!
//! Needs `KARUKAN_IMSERVER` (path to karukan-imserver.exe; skipped otherwise)
//! and `KARUKAN_TSF_DEBUG` (the unregistered service cannot advise its key sink).
#![cfg(windows)]

use std::cell::RefCell;
use std::rc::Rc;

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::Win32::UI::TextServices::{
    CLSID_TF_ThreadMgr, ITfContext, ITfEditSession, ITfEditSession_Impl, ITfKeyEventSink,
    ITfTextInputProcessorEx, ITfThreadMgr, TF_ANCHOR_END, TF_ES_READWRITE, TF_ES_SYNC,
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
            let start = self.ctx.GetStart(ec)?;
            let end = self.ctx.GetEnd(ec)?;
            let _ = end;
            let r = start.Clone()?;
            r.Collapse(ec, windows::Win32::UI::TextServices::TF_ANCHOR_START)?;
            let mut halted = 0i32;
            let mut total = String::new();
            loop {
                let mut buf = [0u16; 64];
                let mut got = 0u32;
                // grow the range one chunk at a time
                let chunk = r.Clone()?;
                chunk.Collapse(ec, windows::Win32::UI::TextServices::TF_ANCHOR_START)?;
                chunk.ShiftEnd(ec, 4096, &mut halted, std::ptr::null())?;
                chunk.GetText(ec, 0, &mut buf, &mut got)?;
                total.push_str(&String::from_utf16_lossy(&buf[..got as usize]));
                break;
            }
            let _ = TF_ANCHOR_END;
            *self.out.borrow_mut() = total;
        }
        Ok(())
    }
}

struct Harness {
    ctx: ITfContext,
    tid: u32,
    sink: ITfKeyEventSink,
    _tm: ITfThreadMgr,
}

fn setup() -> Option<Harness> {
    std::env::var_os("KARUKAN_IMSERVER")?;
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let tm: ITfThreadMgr =
            CoCreateInstance(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER).expect("ThreadMgr");
        let tid = tm.Activate().expect("Activate");
        let tip: ITfTextInputProcessorEx = karukan_tsf::__create_text_service()
            .cast()
            .expect("cast TIP");
        tip.ActivateEx(&tm, tid, 0).expect("ActivateEx");
        let doc = tm.CreateDocumentMgr().expect("CreateDocumentMgr");
        let mut ctx = None;
        let mut ec = 0u32;
        doc.CreateContext(tid, 0, None, &mut ctx, &mut ec)
            .expect("CreateContext");
        let ctx = ctx.expect("ctx");
        doc.Push(&ctx).expect("Push");
        let _ = tm.SetFocus(&doc);
        let sink: ITfKeyEventSink = tip.cast().expect("cast sink");
        Some(Harness {
            ctx,
            tid,
            sink,
            _tm: tm,
        })
    }
}

impl Harness {
    fn key(&self, vk: u32) -> bool {
        unsafe {
            self.sink
                .OnKeyDown(&self.ctx, WPARAM(vk as usize), LPARAM(0))
                .map(|b| b.as_bool())
                .unwrap_or(false)
        }
    }

    fn text(&self) -> String {
        let out = Rc::new(RefCell::new(String::new()));
        let session: ITfEditSession = ReadAll {
            ctx: self.ctx.clone(),
            out: out.clone(),
        }
        .into();
        unsafe {
            let hr = self
                .ctx
                .RequestEditSession(self.tid, &session, TF_ES_SYNC | TF_ES_READWRITE);
            assert!(hr.is_ok(), "read session failed: {hr:?}");
        }
        let s = out.borrow().clone();
        s
    }
}

#[test]
fn typing_updates_composition_and_commits() {
    let Some(h) = setup() else {
        eprintln!("KARUKAN_IMSERVER not set; skipped");
        return;
    };
    // k y o u
    for vk in [0x4B, 0x59, 0x4F, 0x55] {
        assert!(h.key(vk), "letter should be consumed");
    }
    let text = h.text();
    assert!(text.contains("きょう"), "composition text was {text:?}");

    // F7 -> katakana, Enter commits
    assert!(h.key(0x76));
    assert!(h.text().contains("キョウ"), "after F7: {:?}", h.text());
    assert!(h.key(0x0D));
    let text = h.text();
    assert_eq!(text, "キョウ", "after commit: {text:?}");
}

#[test]
fn per_key_latency() {
    let Some(h) = setup() else { return };
    // warm-up: first key spawns the server and installs the models
    let t = std::time::Instant::now();
    h.key(0x57); // w
    eprintln!("first key: {} ms", t.elapsed().as_millis());
    std::thread::sleep(std::time::Duration::from_secs(3));
    let mut ms = vec![];
    for vk in "ATASHINONAMAEHANAKADADESUKYOUHATENKIGAIIDESUNEASITAMOHARERUTOIIDESUGAKOREWANAGAIBUNNDESU".bytes() {
        let t = std::time::Instant::now();
        h.key(vk as u32);
        ms.push(t.elapsed().as_millis());
    }
    eprintln!("per key ms: {ms:?}");
    eprintln!("text: {}", h.text());
}

#[implement(ITfEditSession)]
struct Put {
    ctx: ITfContext,
    text: String,
}

impl ITfEditSession_Impl for Put_Impl {
    fn DoEditSession(&self, ec: u32) -> Result<()> {
        unsafe {
            let ins: windows::Win32::UI::TextServices::ITfInsertAtSelection = self.ctx.cast()?;
            let w: Vec<u16> = self.text.encode_utf16().collect();
            let r = ins.InsertTextAtSelection(ec, Default::default(), &w)?;
            // an app leaves the caret after what was typed; this store does not
            r.Collapse(ec, windows::Win32::UI::TextServices::TF_ANCHOR_END)?;
            let sel = windows::Win32::UI::TextServices::TF_SELECTION {
                range: std::mem::ManuallyDrop::new(Some(r)),
                style: windows::Win32::UI::TextServices::TF_SELECTIONSTYLE {
                    ase: windows::Win32::UI::TextServices::TF_AE_NONE,
                    fInterimChar: windows::core::BOOL(0),
                },
            };
            self.ctx.SetSelection(ec, &[sel])?;
        }
        Ok(())
    }
}

#[test]
fn the_text_left_of_the_caret_is_readable() {
    let Some(h) = setup() else { return };
    let text = "今日は天気が";
    let s: ITfEditSession = Put { ctx: h.ctx.clone(), text: text.into() }.into();
    unsafe {
        let hr = h.ctx.RequestEditSession(h.tid, &s, TF_ES_SYNC | TF_ES_READWRITE);
        assert!(hr.is_ok(), "{hr:?}");
    }
    let left = karukan_tsf::__read_left_context(&h.ctx, h.tid);
    assert_eq!(left.as_deref(), Some(text));
    // typing after it still works
    for vk in [0x4B, 0x59, 0x4F, 0x55] {
        assert!(h.key(vk));
    }
    assert!(h.text().ends_with("きょう"), "{:?}", h.text());
}
