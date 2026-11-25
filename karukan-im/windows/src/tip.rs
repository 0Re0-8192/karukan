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
    ITfKeyEventSink_Impl, ITfKeystrokeMgr, ITfLangBarItem, ITfLangBarItemButton, ITfLangBarItemMgr,
    ITfRange, ITfTextInputProcessor, ITfTextInputProcessor_Impl, ITfTextInputProcessorEx,
    ITfTextInputProcessorEx_Impl, ITfThreadMgr, TF_AE_NONE, TF_ANCHOR_END, TF_ANCHOR_START,
    TF_DEFAULT_SELECTION, TF_ES_ASYNCDONTCARE, TF_ES_READ, TF_ES_READWRITE, TF_ES_SYNC,
    TF_IAS_QUERYONLY, TF_SELECTION, TF_SELECTIONSTYLE,
};
use windows::core::{
    BOOL, BOOL as CoreBool, GUID, IUnknownImpl, Interface, Ref, Result, implement,
};

use crate::attr::{AttrInfo, EnumAttr};
use crate::cand::{self, Anchor};
use crate::engine::{Action, EngineClient, KeyResult, Mods};
use crate::guids::{GUID_ATTR_CONVERTED, GUID_ATTR_INPUT};
use crate::keymap::{self, KEYSYM_SPACE, KEYSYM_SUPER_R};

const ENGINE_RETRY: Duration = Duration::from_secs(10);
const STAMP_CHECK: Duration = Duration::from_secs(60);
/// How long a first key waits for a shared server it had to start.
const SHARED_START: Duration = Duration::from_secs(3);
/// After the shared server failed once, private servers for this long (a failure
/// can cost a first key up to `SHARED_START`, which must not recur every minute).
const SHARED_RETRY: Duration = Duration::from_secs(30 * 60);

pub struct State {
    tid: u32,
    thread_mgr: Option<ITfThreadMgr>,
    engine: Option<EngineClient>,
    engine_failed: Option<Instant>,
    /// What the running server was started for (the pipe name, or for a private
    /// server the data stamp): when it changes, the next word gets a new server.
    engine_key: Option<String>,
    stamp_checked: Instant,
    /// The shared server could not be reached: private servers until then.
    shared_failed: Option<Instant>,
    /// Keys typed into the current composition, shown beside the candidates.
    typed: String,
    aux: Option<String>,
    composition: Option<ITfComposition>,
    has_preedit: bool,
    /// The taskbar's あ / A button, while the service is active
    mode_button: Option<(
        windows::core::ComObject<crate::langbar::ModeButton>,
        ITfLangBarItemMgr,
    )>,
    /// The composition text last written, to tell a text change from a
    /// caret move (`caret_after_text_change`).
    shown_text: String,
    /// A caret to set again in a session of its own (see `update_preedit`).
    caret_retry: Option<usize>,
    /// The candidate window stands at the caret or the window, not at the text.
    anchor_provisional: bool,
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
            engine_key: None,
            stamp_checked: Instant::now(),
            shared_failed: None,
            typed: String::new(),
            aux: None,
            composition: None,
            has_preedit: false,
            mode_button: None,
            shown_text: String::new(),
            caret_retry: None,
            anchor_provisional: false,
            open: true,
            context: None,
            atom_input: 0,
            atom_converted: 0,
        }
    }

    /// Let go of a server started for files that have changed since (checked at
    /// most once a minute, never mid-composition). Also run on focus loss, so an
    /// app left idle does not keep an outdated shared server alive.
    fn drop_if_stale(&mut self) {
        if self.engine.is_some() && !self.has_preedit && self.stamp_checked.elapsed() >= STAMP_CHECK
        {
            self.stamp_checked = Instant::now();
            if engine_key(self.engine.as_ref().is_some_and(|e| e.pipe_name.is_some()))
                != self.engine_key
            {
                self.engine = None;
            }
        }
    }

    fn engine(&mut self) -> Option<&mut EngineClient> {
        // The server reads the dictionaries once at start-up, so a long-lived app
        // (a browser) would never see a new dict.bin or the curate job's additions.
        // Between compositions, move to a fresh server when those files (or the
        // server itself) change, checked once a minute. The English word list is
        // not one of them: the server reloads it in place.
        self.drop_if_stale();
        let dead = match self.engine.as_mut() {
            Some(e) => !e.is_alive(),
            None => true,
        };
        if dead {
            self.engine = None;
            if let Some(t) = self.engine_failed
                && t.elapsed() < ENGINE_RETRY
            {
                return None;
            }
            let t0 = Instant::now();
            let try_shared = use_shared()
                && self
                    .shared_failed
                    .is_none_or(|t| t.elapsed() >= SHARED_RETRY);
            let shared = if try_shared {
                let e = connect_shared();
                if e.is_none() {
                    self.shared_failed = Some(Instant::now());
                }
                e
            } else {
                None
            };
            let got = shared.or_else(spawn_engine);
            match got {
                Some(e) => {
                    self.engine_key = engine_key(e.pipe_name.is_some());
                    diag(&format!(
                        "server ready in {} ms ({})",
                        t0.elapsed().as_millis(),
                        if e.pipe_name.is_some() {
                            "shared"
                        } else {
                            "private"
                        }
                    ));
                    self.engine = Some(e);
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

/// What a server is started for: the shared server's pipe name, or for a
/// private one the data stamp.
fn engine_key(shared: bool) -> Option<String> {
    if shared {
        crate::dll_dir().and_then(|d| pipe_name(&d))
    } else {
        Some(format!("{:?}", data_stamp()))
    }
}

/// Newest modification time among the files the server loads at start-up
/// (portable layout only; elsewhere the data directory is not ours to watch).
/// `user_english.txt` and the user dictionaries are left out: the server
/// picks up new words in either by itself, and counting them restarted every
/// app's server each time a word was learned or registered (Ctrl+F7).
fn data_stamp() -> Option<std::time::SystemTime> {
    let dir = crate::dll_dir()?;
    if !dir.join("portable.flag").exists() {
        return None;
    }
    std::fs::metadata(dir.join("data").join("dict.bin"))
        .and_then(|m| m.modified())
        .ok()
}

fn server_exe(dir: &std::path::Path) -> PathBuf {
    // KARUKAN_IMSERVER overrides the server path (used by the tests).
    match std::env::var_os("KARUKAN_IMSERVER") {
        Some(p) => PathBuf::from(p),
        None => dir.join("karukan-imserver.exe"),
    }
}

fn server_envs(dir: &std::path::Path) -> Vec<(String, String)> {
    let mut envs: Vec<(String, String)> = Vec::new();
    // Portable mode: keep every file next to the DLL.
    if dir.join("portable.flag").exists() {
        let p = |n: &str| dir.join(n).to_string_lossy().into_owned();
        envs.push(("KARUKAN_DATA_DIR".into(), p("data")));
        envs.push(("KARUKAN_CONFIG_DIR".into(), p("config")));
        envs.push(("HF_HOME".into(), p("hf")));
    }
    envs
}

/// The shared server is for ordinary desktop apps; a sandboxed or low-integrity
/// one cannot open it and keeps a private server. Tests that point `KARUKAN_IMSERVER` at a server
/// get private ones unless they ask (`KARUKAN_SHARED=1`, or `auto` to decide by
/// this process's integrity level like an installed one).
fn use_shared() -> bool {
    if std::env::var_os("KARUKAN_IMSERVER").is_some() {
        // "auto": decide as an installed text service would (the medium-integrity test)
        return match std::env::var("KARUKAN_SHARED").as_deref() {
            Ok("1") => true,
            Ok("auto") => crate::shared::eligible(),
            _ => false,
        };
    }
    std::env::var_os("KARUKAN_SHARED").is_none_or(|v| v != "0") && crate::shared::eligible()
}

/// `karukan-<user SID>-<hash of the install, the server build and the dictionaries>`:
/// one name for every app of the user, elevated or not. The installer marks the
/// server exe medium integrity, so it runs with an ordinary user's rights whoever
/// starts it, and an elevated app (writing down) reaches it like any other.
fn pipe_name(dir: &std::path::Path) -> Option<String> {
    let sid = crate::shared::user_sid()?;
    let exe = server_exe(dir);
    let mtime = std::fs::metadata(&exe).and_then(|m| m.modified()).ok();
    let key = format!("{}|{:?}|{:?}", dir.display(), mtime, data_stamp());
    // FNV-1a: stable across processes and builds (std's hasher is not)
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in key.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    Some(format!("karukan-{sid}-{h:016x}"))
}

/// The shared server this process started last, and when.
static PREWARMED: std::sync::Mutex<Option<(String, Instant)>> = std::sync::Mutex::new(None);

/// Start the shared server if it is not running yet; never waits for it.
/// Called when an app activates the text service, so the dictionaries and models
/// are loaded by the time the user types.
pub(crate) fn prewarm_shared() {
    if !use_shared() {
        return;
    }
    let Some(dir) = crate::dll_dir() else { return };
    let Some(name) = pipe_name(&dir) else { return };
    if !crate::shared::pipe_exists(&name) {
        let r = crate::shared::spawn_detached(&server_exe(&dir), &name, &server_envs(&dir));
        diag(&format!("started {name} ahead of the first key: {r:?}"));
        if r.is_ok() {
            *PREWARMED.lock().unwrap_or_else(|e| e.into_inner()) = Some((name, Instant::now()));
        }
    }
}

/// What an app does on activation and on its first key, without TSF: decide,
/// start the shared server, connect, check the server, init, one key. For tests
/// in sessions with no desktop (over SSH), where a thread manager cannot start.
pub(crate) fn shared_probe() -> std::result::Result<(String, Duration), String> {
    if !use_shared() {
        return Err(format!(
            "not using the shared server (integrity {:?})",
            crate::shared::integrity()
        ));
    }
    prewarm_shared();
    let t = Instant::now();
    let mut e = connect_shared().ok_or("could not connect (see debug.log)")?;
    let name = e.pipe_name.clone().unwrap_or_default();
    e.process_key(0x6b, Mods::default())
        .ok_or("no reply to a key")?;
    Ok((name, t.elapsed()))
}

/// Connect to the shared server, starting it when nobody has yet.
fn connect_shared() -> Option<EngineClient> {
    let dir = crate::dll_dir()?;
    let name = pipe_name(&dir)?;
    let deadline = Instant::now() + SHARED_START;
    let mut spawned = false;
    let mut e = loop {
        match EngineClient::connect(&name) {
            Ok(e) => break e,
            Err(err) => {
                // 2: no such pipe yet; 231: all instances busy (another app connecting)
                let code = err.raw_os_error().unwrap_or(0);
                // a server started ahead of the first key may not have its pipe yet
                let prewarmed = PREWARMED
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .as_ref()
                    .is_some_and(|(n, t)| *n == name && t.elapsed() < SHARED_START);
                if code == 2 && !spawned && !prewarmed {
                    spawned = true;
                    diag(&format!("{name} not running: starting it"));
                    if let Err(err) =
                        crate::shared::spawn_detached(&server_exe(&dir), &name, &server_envs(&dir))
                    {
                        diag(&format!("cannot start the shared server: {err}"));
                        return None;
                    }
                } else if code != 2 && code != 231 {
                    diag(&format!("cannot open {name}: {err}"));
                    return None;
                }
                if Instant::now() >= deadline {
                    diag(&format!("{name} did not appear"));
                    return None;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    };
    // The name is predictable, so make sure it is our server on the other end, not
    // something another account set up first to read the keys.
    let ours = |p: &std::path::Path| p.canonicalize().ok();
    let server = e.server_pid().and_then(crate::shared::process_image);
    if server.as_deref().and_then(ours).is_none()
        || server.as_deref().and_then(ours) != ours(&server_exe(&dir))
    {
        diag(&format!(
            "{name} is served by {server:?}, not karukan-imserver: not used"
        ));
        return None;
    }
    if !e.init() {
        diag(&format!("{name} did not answer init"));
        return None;
    }
    Some(e)
}

fn spawn_engine() -> Option<EngineClient> {
    let dir = crate::dll_dir()?;
    let exe = server_exe(&dir);
    let envs = server_envs(&dir);
    let mut e = match EngineClient::spawn(&exe, &envs) {
        Ok(e) => e,
        Err(err) => {
            diag(&format!("cannot start {}: {err}", exe.display()));
            return None;
        }
    };
    if !e.init() {
        diag(&format!(
            "{} started but did not answer init",
            exe.display()
        ));
        return None;
    }
    Some(e)
}

/// A panic must never unwind into the host application.
fn guard<T>(default: T, f: impl FnOnce() -> T) -> T {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(v) => v,
        Err(_) => {
            diag("panic caught");
            default
        }
    }
}

/// Why a key was lost, written to `debug.log` beside the DLL while a
/// `debug.flag` file exists there.
pub(crate) fn diag(msg: &str) {
    use std::io::Write;
    let Some(dir) = crate::dll_dir() else { return };
    if !dir.join("debug.flag").exists() {
        return;
    }
    let log = crate::rotated_log(&dir);
    let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
    else {
        return;
    };
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let _ = writeln!(f, "t={secs} pid={} diag: {msg}", std::process::id());
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
        let r = guard(Err(E_FAIL.into()), || {
            apply_actions(&self.state, ec, &self.ctx, &self.actions, &self.sink)
        });
        if let Err(e) = &r {
            diag(&format!("edit session failed: {e:?}"));
        }
        r
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
    let flags = if sync {
        TF_ES_SYNC
    } else {
        TF_ES_ASYNCDONTCARE
    } | TF_ES_READWRITE;
    let hr = unsafe { ctx.RequestEditSession(tid, &session, flags) };
    let ok = matches!(hr, Ok(h) if h.is_ok());
    if !ok {
        diag(&format!("RequestEditSession sync={sync} failed: {hr:?}"));
    }
    if !ok && sync {
        // Some hosts refuse synchronous sessions; fall back to async.
        let session: ITfEditSession = ApplySession {
            state: state.clone(),
            ctx: ctx.clone(),
            actions,
            sink: sink.clone(),
        }
        .into();
        let hr2 =
            unsafe { ctx.RequestEditSession(tid, &session, TF_ES_ASYNCDONTCARE | TF_ES_READWRITE) };
        if !matches!(hr2, Ok(h) if h.is_ok()) {
            diag(&format!(
                "RequestEditSession async fallback failed: {hr2:?}"
            ));
        }
    }
}

/// How far back (UTF-16 units) the text left of the caret is read; the engine
/// keeps its own `context_chars` of it.
const LEFT_CONTEXT_UNITS: i32 = 40;

#[implement(ITfEditSession)]
struct ReadLeftSession {
    ctx: ITfContext,
    out: Rc<RefCell<Option<String>>>,
}

impl ITfEditSession_Impl for ReadLeftSession_Impl {
    fn DoEditSession(&self, ec: u32) -> Result<()> {
        guard(Ok(()), || {
            let text = read_left(ec, &self.ctx).unwrap_or_default();
            *self.out.borrow_mut() = Some(text);
            Ok(())
        })
    }
}

fn read_left(ec: u32, ctx: &ITfContext) -> Result<String> {
    unsafe {
        let mut sel = [TF_SELECTION::default()];
        let mut fetched = 0u32;
        ctx.GetSelection(ec, TF_DEFAULT_SELECTION, &mut sel, &mut fetched)?;
        let range = ManuallyDrop::into_inner(std::mem::take(&mut sel[0].range)).ok_or(E_FAIL)?;
        let r = range.Clone()?;
        r.Collapse(ec, TF_ANCHOR_START)?;
        let mut halted = 0i32;
        r.ShiftStart(ec, -LEFT_CONTEXT_UNITS, &mut halted, std::ptr::null())?;
        let mut buf = [0u16; LEFT_CONTEXT_UNITS as usize + 8];
        let mut n = 0u32;
        r.GetText(ec, 0, &mut buf, &mut n)?;
        dbg_log(&format!("left context: shifted {halted}, got {n} units"));
        Ok(String::from_utf16_lossy(&buf[..n as usize]))
    }
}

/// The text just left of the caret, or None when the app will not say
/// (no text store, a game, a password field).
pub(crate) fn read_left_context(ctx: &ITfContext, tid: u32) -> Option<String> {
    let out = Rc::new(RefCell::new(None));
    let session: ITfEditSession = ReadLeftSession {
        ctx: ctx.clone(),
        out: out.clone(),
    }
    .into();
    let hr = unsafe { ctx.RequestEditSession(tid, &session, TF_ES_SYNC | TF_ES_READ) };
    if !matches!(hr, Ok(h) if h.is_ok()) {
        return None;
    }
    out.borrow_mut().take()
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

    unsafe { ctx.SetSelection(ec, &[sel]) }
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
        let shows = actions
            .iter()
            .any(|a| matches!(a, Action::ShowCandidates { .. }));
        if touched || shows {
            cand::set_aux(st.aux.clone());
            cand::set_typed(Some(st.typed.clone()).filter(|t| !t.is_empty()));
            // no new list in this batch (Backspace keeps the window as it is):
            // the open window still has to show the new keys
            if !shows && !actions.iter().any(|a| matches!(a, Action::HideCandidates)) {
                cand::refresh();
            }
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
                // Placed once per composition, unless it went to a stand-in spot (the
                // caret, the window) because the text's own position was not known
                // yet: then ask again each time, and move once it is.
                let provisional = state.borrow().anchor_provisional;
                let anchor = if cand::is_visible() && !provisional {
                    None
                } else {
                    match text_anchor(state, ec, ctx) {
                        Some((a, exact)) => {
                            state.borrow_mut().anchor_provisional = !exact;
                            (exact || !cand::is_visible()).then_some(a)
                        }
                        None => None,
                    }
                };
                cand::show(candidates, *cursor, *page, *total_pages, anchor);
            }
            Action::HideCandidates => cand::hide(),
            Action::UpdateAux { .. } | Action::HideAux => {}
        }
    }
    Ok(())
}

/// Where the composition is on screen, and whether that is its own position
/// (`true`) or a stand-in (the caret, the window).
fn text_anchor(state: &Rc<RefCell<State>>, ec: u32, ctx: &ITfContext) -> Option<(Anchor, bool)> {
    let comp = state.borrow().composition.clone()?;
    unsafe {
        let range = comp.GetRange().ok()?;
        let view = ctx.GetActiveView().ok()?;
        let mut rc = RECT::default();
        let mut clipped = BOOL(0);
        let got = view.GetTextExt(ec, &range, &mut rc, &mut clipped);
        // An app whose layout is not ready yet (TS_E_NOLAYOUT, common for the
        // first character in a browser) fails here, and some report an empty
        // rectangle. Either way the window must not fall back on wherever it last
        // was, which can be another field or another app.
        if got.is_ok() && rc.bottom > rc.top && !(rc.left == 0 && rc.top == 0) {
            return Some((
                Anchor {
                    x: rc.left,
                    top: rc.top,
                    bottom: rc.bottom,
                },
                true,
            ));
        }
        let hwnd = view.GetWnd().ok();
        let fallback = caret_anchor(hwnd);
        diag(&format!(
            "GetTextExt {got:?} rect=({},{},{},{}); caret fallback {:?}",
            rc.left,
            rc.top,
            rc.right,
            rc.bottom,
            fallback.map(|a| (a.x, a.top, a.bottom))
        ));
        fallback.map(|a| (a, false))
    }
}

/// Where the app's own caret is (the system caret most edit controls keep), or
/// failing that the top-left of the app window's client area.
fn caret_anchor(hwnd: Option<windows::Win32::Foundation::HWND>) -> Option<Anchor> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::ClientToScreen;
    use windows::Win32::UI::WindowsAndMessaging::{
        GUITHREADINFO, GetClientRect, GetGUIThreadInfo, GetWindowThreadProcessId,
    };
    unsafe {
        let thread = hwnd.map(|h| GetWindowThreadProcessId(h, None)).unwrap_or(0);
        let mut gi = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        if GetGUIThreadInfo(thread, &mut gi).is_ok()
            && !gi.hwndCaret.is_invalid()
            && gi.rcCaret.bottom > gi.rcCaret.top
        {
            let mut top = POINT {
                x: gi.rcCaret.left,
                y: gi.rcCaret.top,
            };
            let mut bottom = POINT {
                x: gi.rcCaret.left,
                y: gi.rcCaret.bottom,
            };
            if ClientToScreen(gi.hwndCaret, &mut top).as_bool()
                && ClientToScreen(gi.hwndCaret, &mut bottom).as_bool()
            {
                return Some(Anchor {
                    x: top.x,
                    top: top.y,
                    bottom: bottom.y,
                });
            }
        }
        let h = hwnd
            .filter(|h| !h.is_invalid())
            .or(Some(gi.hwndFocus).filter(|h| !h.is_invalid()))?;
        let mut rc = RECT::default();
        GetClientRect(h, &mut rc).ok()?;
        let mut p = POINT { x: 0, y: 0 };
        if !ClientToScreen(h, &mut p).as_bool() {
            return None;
        }
        Some(Anchor {
            x: p.x + 16,
            top: p.y,
            bottom: p.y + 24,
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
        if matches!(
            a.style.as_str(),
            "highlight" | "reverse" | "underline_double"
        ) {
            let s = utf16_offset(text, a.start);
            let e = utf16_offset(text, a.end);
            if let Ok(sr) = sub_range(ec, &range, s, e) {
                set_attr(ec, ctx, &sr, atom_cv);
            }
        }
    }
    // caret
    let caret_units = utf16_offset(text, caret);
    unsafe {
        let _ = set_caret(ec, ctx, &range, caret_units);
    }
    let mut st = state.borrow_mut();
    // A Chromium host (Chrome, Electron apps) ignores a caret set in the same
    // session that changed the text, and draws it where its own bookkeeping
    // left it, at an edge of the change: 今日は|海 → → showed は|うみ for
    // はう|み. A caret move alone is honored, so the caret is set again in a
    // session of its own when the text changed and the caret is not at its
    // end (at the end the host lands on it anyway).
    if st.shown_text != text && caret < text.chars().count() {
        st.caret_retry = Some(caret_units as usize);
    }
    st.shown_text = text.to_string();
    st.has_preedit = true;
    Ok(())
}

/// Put the caret `units` UTF-16 units into the composition `range`.
unsafe fn set_caret(ec: u32, ctx: &ITfContext, range: &ITfRange, units: i32) -> Result<()> {
    unsafe {
        let r = range.Clone()?;
        let mut halted: i32 = 0;
        r.Collapse(ec, TF_ANCHOR_START)?;
        r.ShiftEnd(ec, units, &mut halted, std::ptr::null())?;
        r.Collapse(ec, TF_ANCHOR_END)?;
        set_selection(ec, ctx, r)
    }
}

/// Tell the taskbar button the IME went on or off.
fn notify_mode(state: &Rc<RefCell<State>>) {
    let button = state.borrow().mode_button.as_ref().map(|(b, _)| b.clone());
    if let Some(b) = button {
        crate::langbar::notify(&b);
    }
}

/// Commit what is being composed (the IME turned off from the taskbar), in
/// an asynchronous session: a click is not a key event.
fn commit_pending(state: &Rc<RefCell<State>>, sink: &ITfCompositionSink) {
    let (res, ctx) = {
        let mut st = state.borrow_mut();
        let ctx = st.context.clone();
        let has_preedit = st.has_preedit;
        let res = st
            .engine
            .as_mut()
            .filter(|_| has_preedit)
            .and_then(|e| e.is_alive().then(|| e.commit()).flatten());
        st.typed.clear();
        st.aux = None;
        (res, ctx)
    };
    if let (Some(res), Some(ctx)) = (res, ctx) {
        request_session(state, &ctx, res.actions, sink, false);
    }
    cand::hide();
}

/// Ctrl+F7 (no Alt or Shift): MS-IME's word registration key.
fn is_word_key(vk: u32) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState;
    let down = |k: u32| unsafe { GetKeyState(k as i32) } < 0;
    vk == 0x76 && down(keymap::VK_CONTROL) && !down(keymap::VK_MENU) && !down(keymap::VK_SHIFT)
}

/// The word registration window (`karukan-setup.exe --word`, a process of
/// its own), its reading filled in from the composition: the typed keys as
/// kana, and the word with what the composition shows when that differs.
fn open_word_window(state: &Rc<RefCell<State>>) {
    let Some(dir) = crate::dll_dir() else { return };
    let exe = dir.join("karukan-setup.exe");
    let (reading, surface) = {
        let st = state.borrow();
        if st.has_preedit {
            let shown = st.shown_text.clone();
            let surface = if shown != st.typed {
                shown
            } else {
                String::new()
            };
            (st.typed.clone(), surface)
        } else {
            (String::new(), String::new())
        }
    };
    let args = format!(
        "--word --reading {} --surface {}",
        crate::shared::quote_arg(&reading),
        crate::shared::quote_arg(&surface)
    );
    if let Err(e) = crate::shared::spawn_with_args(&exe, &args, &[]) {
        diag(&format!("word window: {e}"));
    }
}

/// A click on the candidate at `index` on the page: committed like Ctrl+digit,
/// in an asynchronous session (a click is not a key event, so the host may
/// refuse a synchronous one).
fn pick_candidate(state: &Rc<RefCell<State>>, sink: &ITfCompositionSink, index: usize) {
    let (res, ctx) = {
        let mut st = state.borrow_mut();
        let ctx = st.context.clone();
        let res = st
            .engine
            .as_mut()
            .and_then(|e| e.is_alive().then(|| e.select_candidate(index)).flatten());
        if res.is_some() {
            st.typed.clear();
            st.aux = None;
        }
        (res, ctx)
    };
    if let (Some(res), Some(ctx)) = (res, ctx) {
        request_session(state, &ctx, res.actions, sink, false);
    }
}

/// Sets the caret again, alone (see `update_preedit`).
#[implement(ITfEditSession)]
struct CaretSession {
    state: Rc<RefCell<State>>,
    ctx: ITfContext,
    units: usize,
}

impl ITfEditSession_Impl for CaretSession_Impl {
    fn DoEditSession(&self, ec: u32) -> Result<()> {
        guard(Ok(()), || {
            let comp = self.state.borrow().composition.clone();
            if let Some(c) = comp {
                unsafe {
                    let range = c.GetRange()?;
                    set_caret(ec, &self.ctx, &range, self.units as i32)?;
                }
            }
            Ok(())
        })
    }
}

/// The caret `update_preedit` asked to set again, in a session of its own.
fn retry_caret(state: &Rc<RefCell<State>>, ctx: &ITfContext) {
    let Some(units) = state.borrow_mut().caret_retry.take() else {
        return;
    };
    let tid = state.borrow().tid;
    let session: ITfEditSession = CaretSession {
        state: state.clone(),
        ctx: ctx.clone(),
        units,
    }
    .into();
    let hr =
        unsafe { ctx.RequestEditSession(tid, &session, TF_ES_ASYNCDONTCARE | TF_ES_READWRITE) };
    if !matches!(hr, Ok(h) if h.is_ok()) {
        diag(&format!("caret session failed: {hr:?}"));
    }
}

fn end_composition(state: &Rc<RefCell<State>>, ec: u32, ctx: &ITfContext) {
    {
        let mut st = state.borrow_mut();
        st.shown_text.clear();
        st.caret_retry = None;
    }
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
    let mut st = state.borrow_mut();
    st.has_preedit = false;
    st.shown_text.clear();
    st.caret_retry = None;
    Ok(())
}

// ------------------------------------------------------------------ TSF impls

impl TextService_Impl {
    fn activate(&self, ptim: Option<&ITfThreadMgr>, tid: u32) -> Result<()> {
        let tm = ptim.ok_or(E_INVALIDARG)?.clone();
        unsafe {
            let ksm: ITfKeystrokeMgr = tm
                .cast()
                .inspect_err(|e| dbg_log(&format!("cast ksm {e:?}")))?;
            let ks: ITfKeyEventSink = self.to_interface();
            if let Err(e) = ksm.AdviseKeyEventSink(tid, &ks, true) {
                // An unregistered text service (the e2e test) cannot advise.
                if std::env::var_os("KARUKAN_TSF_DEBUG").is_none() {
                    return Err(e);
                }
                dbg_log(&format!("advise failed (ignored in debug): {e:?}"));
            }
            let cat: ITfCategoryMgr =
                CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)
                    .inspect_err(|e| dbg_log(&format!("catmgr {e:?}")))?;
            let a_in = cat
                .RegisterGUID(&GUID_ATTR_INPUT)
                .inspect_err(|e| dbg_log(&format!("regguid1 {e:?}")))?;
            let a_cv = cat
                .RegisterGUID(&GUID_ATTR_CONVERTED)
                .inspect_err(|e| dbg_log(&format!("regguid2 {e:?}")))?;
            let mut st = self.state.borrow_mut();
            st.tid = tid;
            st.thread_mgr = Some(tm);
            st.atom_input = a_in;
            st.atom_converted = a_cv;
            st.open = true;
        }
        prewarm_shared();
        let state = self.state.clone();
        let sink = self.sink();
        cand::set_on_pick(Some(Box::new(move |index| {
            pick_candidate(&state, &sink, index)
        })));
        let tm = self.state.borrow().thread_mgr.clone();
        if let Some(tm) = tm {
            self.add_mode_button(&tm);
        }
        Ok(())
    }

    /// The taskbar's あ / A button: left click toggles the IME, right click
    /// opens the menu with 単語の登録 (where MS-IME users register words).
    fn add_mode_button(&self, tm: &ITfThreadMgr) {
        let Ok(mgr) = tm.cast::<ITfLangBarItemMgr>() else {
            return;
        };
        let (s1, s2, s3) = (self.state.clone(), self.state.clone(), self.state.clone());
        let sink = self.sink();
        let hooks = crate::langbar::Hooks {
            is_open: Box::new(move || s1.borrow().open),
            toggle: Box::new(move || {
                let was_open = s2.borrow().open;
                if was_open {
                    commit_pending(&s2, &sink);
                }
                s2.borrow_mut().open = !was_open;
                notify_mode(&s2);
            }),
            register_word: Box::new(move || open_word_window(&s3)),
            owner: Box::new(cand::owner_window),
        };
        let button = windows::core::ComObject::new(crate::langbar::ModeButton::new(hooks));
        let button_if: ITfLangBarItemButton = button.to_interface();
        let Ok(item) = button_if.cast::<ITfLangBarItem>() else {
            return;
        };
        let added = unsafe { mgr.AddItem(&item) };
        diag(&format!("langbar AddItem {added:?}"));
        if added.is_ok() {
            self.state.borrow_mut().mode_button = Some((button, mgr));
        } else {
            dbg_log("langbar AddItem failed");
        }
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
            // Never start a server just to commit nothing, and never wait for one
            // either: with nothing typed, focus moves on at once and the engine is
            // only told to reset (the shared server may be busy with another app).
            let has_preedit = st.has_preedit;
            let res = match st.engine.as_mut().filter(|_| has_preedit) {
                Some(e) => e.is_alive().then(|| e.commit()).flatten(),
                None => {
                    if let Some(e) = st.engine.as_mut() {
                        e.reset_nowait();
                    }
                    None
                }
            };
            st.anchor_provisional = false;
            st.drop_if_stale();
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
            notify_mode(&self.state);
            return true;
        }
        // Ctrl+F7: register a word (MS-IME's key), the composition's reading
        // filled in
        if is_word_key(vk) {
            open_word_window(&self.state);
            return true;
        }
        let open = self.state.borrow().open;
        if !open {
            if vk == keymap::VK_CONVERT {
                self.state.borrow_mut().open = true;
                notify_mode(&self.state);
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
                notify_mode(&self.state);
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
                if !has_preedit
                    && ((key.mods.control)
                        || (special && plain_space)
                        || (special && vk != keymap::VK_SPACE))
                {
                    return false;
                }
                match (has_preedit, keymap::msime_chord(key.keysym, key.mods)) {
                    (true, Some(f)) => (f, Mods::default()),
                    _ => (key.keysym, key.mods),
                }
            }
        };

        // The first key of a new word: hand the engine what stands left of the
        // caret, so the model reads the sentence it is continuing.
        if !has_preedit && !mods.control && (0x21..=0x7e).contains(&keysym) {
            let tid = self.state.borrow().tid;
            if let Some(left) = read_left_context(ctx, tid)
                && let Some(e) = self.state.borrow_mut().engine()
            {
                e.set_surrounding_text(&left, left.chars().count());
            }
        }

        let result: Option<KeyResult> = {
            let mut st = self.state.borrow_mut();
            match st.engine() {
                None => {
                    diag(&format!("no engine (vk={vk:#x}, composing={has_preedit})"));
                    None
                }
                Some(e) => {
                    let r = e.process_key(keysym, mods);
                    if r.is_none() {
                        diag(&format!(
                            "process_key got no reply (vk={vk:#x}, composing={has_preedit})"
                        ));
                    }
                    r
                }
            }
        };
        let Some(result) = result else {
            return false;
        };
        // 変換 and ひらがな are the IME's own keys (OnTestKeyDown said so): the
        // engine leaves the hiragana-mode key unconsumed when it is already in
        // hiragana mode, which must not hand the key to the application mid-word.
        let consumed = result.consumed || matches!(vk, keymap::VK_CONVERT | keymap::VK_OEM_COPY);
        // a commit or an emptied preedit ends the composition: the next word starts afresh
        let ends = result.actions.iter().any(|a| {
            matches!(a, Action::Commit { .. })
                || matches!(a, Action::UpdatePreedit { text, .. } if text.is_empty())
        });
        {
            let mut st = self.state.borrow_mut();
            if ends {
                st.typed.clear();
                st.aux = None;
            } else {
                // the engine knows what the keys were (it sees Backspace out of a
                // conversion, cursor moves, mid-word inserts); a count kept here would not
                st.typed = if result.typed_kana.is_empty() {
                    result.typed.clone()
                } else {
                    result.typed_kana.clone()
                };
            }
        }
        request_session(&self.state, ctx, result.actions, &self.sink(), true);
        retry_caret(&self.state, ctx);
        consumed
    }

    /// Cheap prediction used by OnTestKeyDown (no engine round trip).
    fn would_eat(&self, vk: u32, lparam: LPARAM) -> bool {
        if keymap::is_open_close_vk(vk) || is_word_key(vk) {
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
        if let Some(tm) = tm
            && let Ok(ksm) = tm.cast::<ITfKeystrokeMgr>()
        {
            let _ = unsafe { ksm.UnadviseKeyEventSink(tid) };
        }
        let mut st = self.state.borrow_mut();
        st.composition = None;
        st.context = None;
        st.has_preedit = false;
        st.thread_mgr = None;
        st.engine = None; // closes stdin: the server saves its learning cache
        if let Some((button, mgr)) = st.mode_button.take() {
            let button_if: ITfLangBarItemButton = button.to_interface();
            if let Ok(item) = button_if.cast::<ITfLangBarItem>() {
                let _ = unsafe { mgr.RemoveItem(&item) };
            }
        }
        cand::set_on_pick(None);
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
        Ok(BOOL::from(guard(false, || {
            self.would_eat(wparam.0 as u32, lparam)
        })))
    }
    fn OnTestKeyUp(&self, _pic: Ref<ITfContext>, _w: WPARAM, _l: LPARAM) -> Result<BOOL> {
        Ok(BOOL(0))
    }
    fn OnKeyDown(&self, pic: Ref<ITfContext>, wparam: WPARAM, lparam: LPARAM) -> Result<BOOL> {
        Ok(BOOL::from(guard(false, || {
            self.handle_key(pic.as_ref(), wparam, lparam)
        })))
    }
    fn OnKeyUp(&self, _pic: Ref<ITfContext>, _w: WPARAM, _l: LPARAM) -> Result<BOOL> {
        Ok(BOOL(0))
    }
    fn OnPreservedKey(&self, _pic: Ref<ITfContext>, _rguid: *const GUID) -> Result<BOOL> {
        Ok(BOOL(0))
    }
}

impl ITfCompositionSink_Impl for TextService_Impl {
    fn OnCompositionTerminated(
        &self,
        _ecwrite: u32,
        _pcomposition: Ref<ITfComposition>,
    ) -> Result<()> {
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
