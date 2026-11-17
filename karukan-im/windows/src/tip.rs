//! The text service (TIP): key handling, composition editing, engine glue.

use std::cell::RefCell;
use std::mem::ManuallyDrop;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{E_FAIL, E_INVALIDARG, LPARAM, RECT, WPARAM};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::TextServices::{
    CLSID_TF_CategoryMgr, GUID_PROP_ATTRIBUTE, IEnumTfDisplayAttributeInfo, ITfCategoryMgr,
    ITfComposition, ITfCompositionSink, ITfCompositionSink_Impl, ITfContext, ITfContextComposition,
    ITfDisplayAttributeInfo, ITfDisplayAttributeProvider, ITfDisplayAttributeProvider_Impl,
    ITfEditSession, ITfEditSession_Impl, ITfInsertAtSelection, ITfKeyEventSink,
    ITfKeyEventSink_Impl, ITfKeystrokeMgr, ITfRange, ITfTextInputProcessor,
    ITfTextInputProcessorEx, ITfTextInputProcessorEx_Impl, ITfTextInputProcessor_Impl,
    ITfThreadMgr, TF_AE_NONE, TF_ANCHOR_END, TF_ANCHOR_START, TF_ES_ASYNCDONTCARE, TF_ES_READWRITE,
    TF_ES_SYNC, TF_IAS_QUERYONLY, TF_SELECTION, TF_SELECTIONSTYLE,
};
use windows::core::{BOOL, BOOL as CoreBool, GUID, Interface, IUnknownImpl, Ref, Result, implement};

use crate::attr::{AttrInfo, EnumAttr};
use crate::cand::{self, Anchor};
use crate::engine::{Action, EngineClient, KeyResult, Mods};
use crate::guids::{GUID_ATTR_CONVERTED, GUID_ATTR_INPUT};
use crate::keymap::{self, KEYSYM_SPACE, KEYSYM_SUPER_R};

const ENGINE_RETRY: Duration = Duration::from_secs(10);
const STAMP_CHECK: Duration = Duration::from_secs(60);

pub struct State {
    tid: u32,
    thread_mgr: Option<ITfThreadMgr>,
    engine: Option<EngineClient>,
    engine_failed: Option<Instant>,
    engine_stamp: Option<std::time::SystemTime>,
    stamp_checked: Instant,
    /// Keys typed into the current composition, shown beside the candidates.
    typed: String,
    aux: Option<String>,
    composition: Option<ITfComposition>,
    has_preedit: bool,
    open: bool,
    context: Option<ITfContext>,
    atom_input: u32,
    atom_converted: u32,
}

impl State {
    fn new() -> Self {
        State {
            tid: 0,
            thread_mgr: None,
            engine: None,
            engine_failed: None,
            engine_stamp: None,
            stamp_checked: Instant::now(),
            typed: String::new(),
            aux: None,
            composition: None,
            has_preedit: false,
            open: true,
            context: None,
            atom_input: 0,
            atom_converted: 0,
        }
    }

    fn engine(&mut self) -> Option<&mut EngineClient> {
        // The server reads the dictionaries once at start-up, so a long-lived app
        // (a browser) would never see a new dict.bin or the curate job's additions.
        // Between compositions, swap in a fresh server when those files change (checked
        // once a minute; they change a few times a day); the old one saves its learning
        // cache on the way out.
        if self.engine.is_some()
            && !self.has_preedit
            && self.stamp_checked.elapsed() >= STAMP_CHECK
        {
            self.stamp_checked = Instant::now();
            if data_stamp() != self.engine_stamp {
                self.engine = None;
            }
        }
        let dead = match self.engine.as_mut() {
            Some(e) => !e.is_alive(),
            None => true,
        };
        if dead {
            self.engine = None;
            if let Some(t) = self.engine_failed {
                if t.elapsed() < ENGINE_RETRY {
                    return None;
                }
            }
            let stamp = data_stamp();
            match spawn_engine() {
                Some(e) => {
                    self.engine = Some(e);
                    self.engine_stamp = stamp;
                    self.stamp_checked = Instant::now();
                    self.engine_failed = None;
                }
                None => {
                    self.engine_failed = Some(Instant::now());
                    return None;
                }
            }
        }
        self.engine.as_mut()
    }
}

/// Newest modification time among the files the server loads at start-up
/// (portable layout only; elsewhere the data directory is not ours to watch).
fn data_stamp() -> Option<std::time::SystemTime> {
    let dir = crate::dll_dir()?;
    if !dir.join("portable.flag").exists() {
        return None;
    }
    let data = dir.join("data");
    let mtime = |p: &std::path::Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let mut newest = ["dict.bin", "user_english.txt"]
        .iter()
        .filter_map(|n| mtime(&data.join(n)))
        .max();
    if let Ok(rd) = std::fs::read_dir(data.join("user_dicts")) {
        for f in rd.flatten() {
            newest = newest.max(mtime(&f.path()));
        }
    }
    newest
}

fn spawn_engine() -> Option<EngineClient> {
    let dir = crate::dll_dir()?;
    // KARUKAN_IMSERVER overrides the server path (used by the tests).
    let exe = match std::env::var_os("KARUKAN_IMSERVER") {
        Some(p) => PathBuf::from(p),
        None => dir.join("karukan-imserver.exe"),
    };
    let mut envs: Vec<(String, String)> = Vec::new();
    // Portable mode: keep every file next to the DLL.
    if dir.join("portable.flag").exists() {
        let p = |n: &str| dir.join(n).to_string_lossy().into_owned();
        envs.push(("KARUKAN_DATA_DIR".into(), p("data")));
        envs.push(("KARUKAN_CONFIG_DIR".into(), p("config")));
        envs.push(("HF_HOME".into(), p("hf")));
    }
    let mut e = EngineClient::spawn(&exe, &envs).ok()?;
    if !e.init() {
        return None;
    }
    Some(e)
}

/// A panic must never unwind into the host application.
fn guard<T>(default: T, f: impl FnOnce() -> T) -> T {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or(default)
}

fn dbg_log(msg: &str) {
    if std::env::var_os("KARUKAN_TSF_DEBUG").is_some() {
        eprintln!("karukan-tsf: {msg}");
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// UTF-16 offset of the `scalar_off`-th Unicode scalar of `text`.
fn utf16_offset(text: &str, scalar_off: usize) -> i32 {
    text.chars()
        .take(scalar_off)
        .map(|c| c.len_utf16() as i32)
        .sum()
}

#[implement(
    ITfTextInputProcessorEx,
    ITfKeyEventSink,
    ITfCompositionSink,
    ITfDisplayAttributeProvider
)]
pub struct TextService {
    state: Rc<RefCell<State>>,
}

impl TextService {
    pub fn new() -> Self {
        TextService {
            state: Rc::new(RefCell::new(State::new())),
        }
    }
}

// ---------------------------------------------------------------- edit session

#[implement(ITfEditSession)]
struct ApplySession {
    state: Rc<RefCell<State>>,
    ctx: ITfContext,
    actions: Vec<Action>,
    sink: ITfCompositionSink,
}

impl ITfEditSession_Impl for ApplySession_Impl {
    fn DoEditSession(&self, ec: u32) -> Result<()> {
        guard(Err(E_FAIL.into()), || {
            apply_actions(&self.state, ec, &self.ctx, &self.actions, &self.sink)
        })
    }
}

fn request_session(
    state: &Rc<RefCell<State>>,
    ctx: &ITfContext,
    actions: Vec<Action>,
    sink: &ITfCompositionSink,
    sync: bool,
) {
    if actions.is_empty() {
        return;
    }
    let tid = state.borrow().tid;
    let session: ITfEditSession = ApplySession {
        state: state.clone(),
        ctx: ctx.clone(),
        actions: actions.clone(),
        sink: sink.clone(),
    }
    .into();
    let flags = if sync { TF_ES_SYNC } else { TF_ES_ASYNCDONTCARE } | TF_ES_READWRITE;
    let hr = unsafe { ctx.RequestEditSession(tid, &session, flags) };
    let ok = matches!(hr, Ok(h) if h.is_ok());
    if !ok && sync {
        // Some hosts refuse synchronous sessions; fall back to async.
        let session: ITfEditSession = ApplySession {
            state: state.clone(),
            ctx: ctx.clone(),
            actions,
            sink: sink.clone(),
        }
        .into();
        let _ = unsafe { ctx.RequestEditSession(tid, &session, TF_ES_ASYNCDONTCARE | TF_ES_READWRITE) };
    }
}

fn set_selection_at_end(ec: u32, ctx: &ITfContext, range: &ITfRange) -> Result<()> {
    unsafe {
        let r = range.Clone()?;
        r.Collapse(ec, TF_ANCHOR_END)?;
        set_selection(ec, ctx, r)
    }
}

unsafe fn set_selection(ec: u32, ctx: &ITfContext, r: ITfRange) -> Result<()> {
    let sel = TF_SELECTION {
        range: ManuallyDrop::new(Some(r)),
        style: TF_SELECTIONSTYLE {
            ase: TF_AE_NONE,
            fInterimChar: BOOL(0),
        },
    };
    let res = unsafe { ctx.SetSelection(ec, &[sel]) };
    res
}

fn sub_range(ec: u32, range: &ITfRange, start: i32, end: i32) -> Result<ITfRange> {
    unsafe {
        let r = range.Clone()?;
        let mut halted: i32 = 0;
        r.Collapse(ec, TF_ANCHOR_START)?;
        r.ShiftEnd(ec, end, &mut halted, std::ptr::null())?;
        r.ShiftStart(ec, start, &mut halted, std::ptr::null())?;
        Ok(r)
    }
}

fn set_attr(ec: u32, ctx: &ITfContext, range: &ITfRange, atom: u32) {
    unsafe {
        if let Ok(prop) = ctx.GetProperty(&GUID_PROP_ATTRIBUTE) {
            let _ = prop.SetValue(ec, range, &VARIANT::from(atom as i32));
        }
    }
}

fn clear_attr(ec: u32, ctx: &ITfContext, range: &ITfRange) {
    unsafe {
        if let Ok(prop) = ctx.GetProperty(&GUID_PROP_ATTRIBUTE) {
            let _ = prop.Clear(ec, range);
        }
    }
}

fn apply_actions(
    state: &Rc<RefCell<State>>,
    ec: u32,
    ctx: &ITfContext,
    actions: &[Action],
    sink: &ITfCompositionSink,
) -> Result<()> {
    // aux is folded in first so the window renders once per batch
    {
        let mut st = state.borrow_mut();
        let mut touched = false;
        for a in actions {
            match a {
                Action::UpdateAux { text } => {
                    st.aux = Some(text.clone());
                    touched = true;
                }
                Action::HideAux => {
                    st.aux = None;
                    touched = true;
                }
                _ => {}
            }
        }
        // Every batch with candidates re-folds, since the typed keys change per key.
        if touched || actions.iter().any(|a| matches!(a, Action::ShowCandidates { .. })) {
            cand::set_aux(footer_text(&st.typed, st.aux.as_deref()));
        }
    }
    for a in actions {
        match a {
            Action::UpdatePreedit {
                text,
                caret,
                attributes,
            } => update_preedit(state, ec, ctx, sink, text, *caret, attributes)?,
            Action::Commit { text } => commit(state, ec, ctx, text)?,
            Action::ShowCandidates {
                candidates,
                cursor,
                page,
                total_pages,
            } => {
                let anchor = if cand::is_visible() {
                    None
                } else {
                    text_anchor(state, ec, ctx)
                };
                cand::show(candidates, *cursor, *page, *total_pages, anchor);
            }
            Action::HideCandidates => cand::hide(),
            Action::UpdateAux { .. } | Action::HideAux => {}
        }
    }
    Ok(())
}

/// The keys typed so far (as typed, e.g. `kaigi`) ahead of the engine's own line.
fn footer_text(typed: &str, aux: Option<&str>) -> Option<String> {
    const KEEP: usize = 24;
    let n = typed.chars().count();
    let shown: String = if n > KEEP {
        format!("…{}", typed.chars().skip(n - KEEP).collect::<String>())
    } else {
        typed.to_string()
    };
    match (shown.is_empty(), aux) {
        (true, a) => a.map(str::to_string),
        (false, Some(a)) => Some(format!("{shown} │ {a}")),
        (false, None) => Some(shown),
    }
}

fn text_anchor(state: &Rc<RefCell<State>>, ec: u32, ctx: &ITfContext) -> Option<Anchor> {
    let comp = state.borrow().composition.clone()?;
    unsafe {
        let range = comp.GetRange().ok()?;
        let view = ctx.GetActiveView().ok()?;
        let mut rc = RECT::default();
        let mut clipped = BOOL(0);
        view.GetTextExt(ec, &range, &mut rc, &mut clipped).ok()?;
        Some(Anchor {
            x: rc.left,
            top: rc.top,
            bottom: rc.bottom,
        })
    }
}

fn update_preedit(
    state: &Rc<RefCell<State>>,
    ec: u32,
    ctx: &ITfContext,
    sink: &ITfCompositionSink,
    text: &str,
    caret: usize,
    attrs: &[crate::engine::PreeditAttr],
) -> Result<()> {
    if text.is_empty() {
        end_composition(state, ec, ctx);
        return Ok(());
    }
    let existing = state.borrow().composition.clone();
    let range = match existing {
        Some(c) => unsafe { c.GetRange()? },
        None => unsafe {
            let ins: ITfInsertAtSelection = ctx.cast()?;
            let at = ins.InsertTextAtSelection(ec, TF_IAS_QUERYONLY, &[])?;
            let cc: ITfContextComposition = ctx.cast()?;
            let comp = cc.StartComposition(ec, &at, sink)?;
            let r = comp.GetRange()?;
            state.borrow_mut().composition = Some(comp);
            r
        },
    };
    unsafe { range.SetText(ec, 0, &wide(text))? };
    let (atom_in, atom_cv) = {
        let s = state.borrow();
        (s.atom_input, s.atom_converted)
    };
    set_attr(ec, ctx, &range, atom_in);
    for a in attrs {
        if matches!(a.style.as_str(), "highlight" | "reverse" | "underline_double") {
            let s = utf16_offset(text, a.start);
            let e = utf16_offset(text, a.end);
            if let Ok(sr) = sub_range(ec, &range, s, e) {
                set_attr(ec, ctx, &sr, atom_cv);
            }
        }
    }
    // caret
    unsafe {
        let r = range.Clone()?;
        let mut halted: i32 = 0;
        r.Collapse(ec, TF_ANCHOR_START)?;
        r.ShiftEnd(ec, utf16_offset(text, caret), &mut halted, std::ptr::null())?;
        r.Collapse(ec, TF_ANCHOR_END)?;
        let _ = set_selection(ec, ctx, r);
    }
    state.borrow_mut().has_preedit = true;
    Ok(())
}

fn end_composition(state: &Rc<RefCell<State>>, ec: u32, ctx: &ITfContext) {
    let comp = state.borrow_mut().composition.take();
    if let Some(c) = comp {
        unsafe {
            if let Ok(r) = c.GetRange() {
                clear_attr(ec, ctx, &r);
                let _ = r.SetText(ec, 0, &[]);
            }
            let _ = c.EndComposition(ec);
        }
    }
    state.borrow_mut().has_preedit = false;
}

fn commit(state: &Rc<RefCell<State>>, ec: u32, ctx: &ITfContext, text: &str) -> Result<()> {
    let comp = state.borrow_mut().composition.take();
    unsafe {
        if let Some(c) = comp {
            let r = c.GetRange()?;
            r.SetText(ec, 0, &wide(text))?;
            clear_attr(ec, ctx, &r);
            let _ = set_selection_at_end(ec, ctx, &r);
            let _ = c.EndComposition(ec);
        } else {
            let ins: ITfInsertAtSelection = ctx.cast()?;
            let _ = ins.InsertTextAtSelection(ec, Default::default(), &wide(text))?;
        }
    }
    state.borrow_mut().has_preedit = false;
    Ok(())
}

// ------------------------------------------------------------------ TSF impls

impl TextService_Impl {
    fn activate(&self, ptim: Option<&ITfThreadMgr>, tid: u32) -> Result<()> {
        let tm = ptim.ok_or(E_INVALIDARG)?.clone();
        unsafe {
            let ksm: ITfKeystrokeMgr = tm.cast().inspect_err(|e| dbg_log(&format!("cast ksm {e:?}")))?;
            let ks: ITfKeyEventSink = self.to_interface();
            if let Err(e) = ksm.AdviseKeyEventSink(tid, &ks, true) {
                // An unregistered text service (the e2e test) cannot advise.
                if std::env::var_os("KARUKAN_TSF_DEBUG").is_none() {
                    return Err(e);
                }
                dbg_log(&format!("advise failed (ignored in debug): {e:?}"));
            }
            let cat: ITfCategoryMgr =
                CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER).inspect_err(|e| dbg_log(&format!("catmgr {e:?}")))?;
            let a_in = cat.RegisterGUID(&GUID_ATTR_INPUT).inspect_err(|e| dbg_log(&format!("regguid1 {e:?}")))?;
            let a_cv = cat.RegisterGUID(&GUID_ATTR_CONVERTED).inspect_err(|e| dbg_log(&format!("regguid2 {e:?}")))?;
            let mut st = self.state.borrow_mut();
            st.tid = tid;
            st.thread_mgr = Some(tm);
            st.atom_input = a_in;
            st.atom_converted = a_cv;
            st.open = true;
        }
        Ok(())
    }

    fn sink(&self) -> ITfCompositionSink {
        self.to_interface()
    }

    /// Commit whatever is pending in the engine into the last used context.
    fn flush(&self) {
        let (res, ctx) = {
            let mut st = self.state.borrow_mut();
            let ctx = st.context.clone();
            st.typed.clear();
            st.aux = None;
            // Never start a server just to commit nothing: an app that merely
            // gained and lost focus has none, and starting one costs ~650 MB and a
            // second on the UI thread.
            let res = st
                .engine
                .as_mut()
                .and_then(|e| e.is_alive().then_some(e))
                .and_then(|e| e.commit());
            (res, ctx)
        };
        if let (Some(res), Some(ctx)) = (res, ctx) {
            request_session(&self.state, &ctx, res.actions, &self.sink(), false);
        }
        cand::hide();
    }

    fn handle_key(&self, pic: Option<&ITfContext>, wparam: WPARAM, lparam: LPARAM) -> bool {
        let vk = wparam.0 as u32;
        let Some(ctx) = pic else { return false };
        self.state.borrow_mut().context = Some(ctx.clone());

        // --- IME on/off keys
        if keymap::is_open_close_vk(vk) {
            let was_open = self.state.borrow().open;
            if was_open {
                self.flush();
            }
            self.state.borrow_mut().open = !was_open;
            return true;
        }
        let open = self.state.borrow().open;
        if !open {
            if vk == keymap::VK_CONVERT {
                self.state.borrow_mut().open = true;
                return true;
            }
            return false;
        }
        if keymap::is_modifier_vk(vk) {
            return false;
        }
        let has_preedit = self.state.borrow().has_preedit;

        // --- keys with MS-IME meaning that are not plain keysyms
        let (keysym, mods): (u32, Mods) = match vk {
            keymap::VK_CONVERT => {
                if has_preedit {
                    (KEYSYM_SPACE, Mods::default())
                } else {
                    (KEYSYM_SUPER_R, Mods::default())
                }
            }
            keymap::VK_OEM_COPY => (KEYSYM_SUPER_R, Mods::default()),
            keymap::VK_NONCONVERT => {
                if has_preedit {
                    return true; // swallowed until the MS-IME conversion cycle exists
                }
                self.state.borrow_mut().open = false;
                return true;
            }
            _ => {
                let Some((key, _)) = keymap::translate(vk, lparam.0) else {
                    return has_preedit; // unknown key mid-composition: swallow
                };
                if key.mods.alt {
                    return false;
                }
                let special = keymap::special_keysym(vk).is_some();
                let plain_space = vk == keymap::VK_SPACE && !key.mods.shift;
                if !has_preedit && ((key.mods.control) || (special && plain_space) || (special && vk != keymap::VK_SPACE)) {
                    return false;
                }
                match (has_preedit, keymap::msime_chord(key.keysym, key.mods)) {
                    (true, Some(f)) => (f, Mods::default()),
                    _ => (key.keysym, key.mods),
                }
            }
        };

        let result: Option<KeyResult> = {
            let mut st = self.state.borrow_mut();
            st.engine().and_then(|e| e.process_key(keysym, mods))
        };
        let Some(result) = result else {
            return false;
        };
        let consumed = result.consumed;
        // a commit or an emptied preedit ends the composition: the next word starts afresh
        let ends = result.actions.iter().any(|a| {
            matches!(a, Action::Commit { .. }) || matches!(a, Action::UpdatePreedit { text, .. } if text.is_empty())
        });
        {
            let mut st = self.state.borrow_mut();
            if ends {
                st.typed.clear();
                st.aux = None;
            } else {
                // the engine knows what the keys were (it sees Backspace out of a
                // conversion, cursor moves, mid-word inserts); a count kept here would not
                st.typed = result.typed.clone();
            }
        }
        request_session(&self.state, ctx, result.actions, &self.sink(), true);
        consumed
    }

    /// Cheap prediction used by OnTestKeyDown (no engine round trip).
    fn would_eat(&self, vk: u32, lparam: LPARAM) -> bool {
        if keymap::is_open_close_vk(vk) {
            return true;
        }
        let (open, has_preedit) = {
            let s = self.state.borrow();
            (s.open, s.has_preedit)
        };
        if !open {
            return vk == keymap::VK_CONVERT;
        }
        if keymap::is_modifier_vk(vk) {
            return false;
        }
        match vk {
            keymap::VK_CONVERT | keymap::VK_OEM_COPY | keymap::VK_NONCONVERT => return true,
            _ => {}
        }
        let Some((key, _)) = keymap::translate(vk, lparam.0) else {
            return has_preedit;
        };
        if key.mods.alt {
            return false;
        }
        if has_preedit {
            return true;
        }
        if key.mods.control {
            return false;
        }
        match keymap::special_keysym(vk) {
            Some(_) => vk == keymap::VK_SPACE && key.mods.shift,
            None => true,
        }
    }
}

impl ITfTextInputProcessor_Impl for TextService_Impl {
    fn Activate(&self, ptim: Ref<ITfThreadMgr>, tid: u32) -> Result<()> {
        self.activate(ptim.as_ref(), tid)
    }
    fn Deactivate(&self) -> Result<()> {
        guard((), || self.flush());
        let tm = self.state.borrow().thread_mgr.clone();
        let tid = self.state.borrow().tid;
        if let Some(tm) = tm {
            if let Ok(ksm) = tm.cast::<ITfKeystrokeMgr>() {
                let _ = unsafe { ksm.UnadviseKeyEventSink(tid) };
            }
        }
        let mut st = self.state.borrow_mut();
        st.composition = None;
        st.context = None;
        st.has_preedit = false;
        st.thread_mgr = None;
        st.engine = None; // closes stdin: the server saves its learning cache
        cand::destroy();
        Ok(())
    }
}

impl ITfTextInputProcessorEx_Impl for TextService_Impl {
    fn ActivateEx(&self, ptim: Ref<ITfThreadMgr>, tid: u32, _flags: u32) -> Result<()> {
        self.activate(ptim.as_ref(), tid)
    }
}

impl ITfKeyEventSink_Impl for TextService_Impl {
    fn OnSetFocus(&self, foreground: BOOL) -> Result<()> {
        if !foreground.as_bool() {
            guard((), || self.flush());
        }
        Ok(())
    }
    fn OnTestKeyDown(&self, _pic: Ref<ITfContext>, wparam: WPARAM, lparam: LPARAM) -> Result<BOOL> {
        Ok(BOOL::from(guard(false, || self.would_eat(wparam.0 as u32, lparam))))
    }
    fn OnTestKeyUp(&self, _pic: Ref<ITfContext>, _w: WPARAM, _l: LPARAM) -> Result<BOOL> {
        Ok(BOOL(0))
    }
    fn OnKeyDown(&self, pic: Ref<ITfContext>, wparam: WPARAM, lparam: LPARAM) -> Result<BOOL> {
        Ok(BOOL::from(guard(false, || self.handle_key(pic.as_ref(), wparam, lparam))))
    }
    fn OnKeyUp(&self, _pic: Ref<ITfContext>, _w: WPARAM, _l: LPARAM) -> Result<BOOL> {
        Ok(BOOL(0))
    }
    fn OnPreservedKey(&self, _pic: Ref<ITfContext>, _rguid: *const GUID) -> Result<BOOL> {
        Ok(BOOL(0))
    }
}

impl ITfCompositionSink_Impl for TextService_Impl {
    fn OnCompositionTerminated(&self, _ecwrite: u32, _pcomposition: Ref<ITfComposition>) -> Result<()> {
        let mut st = self.state.borrow_mut();
        st.composition = None;
        st.has_preedit = false;
        st.typed.clear();
        st.aux = None;
        if let Some(e) = st.engine.as_mut() {
            e.reset();
        }
        cand::hide();
        Ok(())
    }
}

impl ITfDisplayAttributeProvider_Impl for TextService_Impl {
    fn EnumDisplayAttributeInfo(&self) -> Result<IEnumTfDisplayAttributeInfo> {
        Ok(EnumAttr::new())
    }
    fn GetDisplayAttributeInfo(&self, guid: *const GUID) -> Result<ITfDisplayAttributeInfo> {
        let g = unsafe { guid.as_ref() }.ok_or(E_INVALIDARG)?;
        AttrInfo::lookup(g).ok_or_else(|| E_FAIL.into())
    }
}

#[allow(dead_code)]
fn _types(_: PathBuf, _: CoreBool, _: Option<ITfTextInputProcessor>) {}

#[cfg(test)]
mod typed_tests {
    use super::*;

    #[test]
    fn footer_joins_typed_and_aux() {
        assert_eq!(footer_text("", Some("a")), Some("a".into()));
        assert_eq!(footer_text("kaigi", Some("a")), Some("kaigi │ a".into()));
        assert_eq!(footer_text("kaigi", None), Some("kaigi".into()));
        assert_eq!(footer_text("", None), None);
        let long = "a".repeat(40);
        assert_eq!(footer_text(&long, None).unwrap().chars().count(), 25);
    }
}
