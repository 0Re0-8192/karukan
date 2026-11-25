//! Per-composition ownership between Mozc and the karukan engine.
//!
//! The IME sends every request to [`Bridge::handle`] first. `None` means "not ours, let Mozc run
//! it"; after Mozc ran, [`Bridge::observe`] tells the bridge what Mozc did (and may amend the
//! reply). A composition belongs to one engine from its first key until it is empty again, so the
//! two never mix: keys the engine cannot translate (kana from a flick keyboard, for example) start
//! no karukan composition at all.
//!
//! Such a composition is still converted by karukan: Mozc composes the kana (flick, toggles,
//! dakuten), and every time its preedit changes the reading is handed to the engine, whose
//! candidates are put ahead of Mozc's in the reply ("follow"). A tap on one of them commits it
//! through the engine (which learns it) and Mozc is told to revert its composition.

use serde_json::{Value, json};

use crate::input::{self, Input};
use crate::output::{self, CandView, Frame, ListItem, PreeditView};
use crate::proto::{self, Val};

/// JSON-RPC access to the engine (`karukan-imserver` protocol).
pub trait Backend {
    fn call(&mut self, method: &str, params: Value) -> Option<Value>;
}

/// The in-process engine.
pub struct ImBackend {
    server: karukan_im::server::ImServer,
    next_id: u64,
}

impl ImBackend {
    pub fn new() -> Self {
        Self::with_server(karukan_im::server::ImServer::new())
    }
    pub fn with_server(server: karukan_im::server::ImServer) -> Self {
        Self { server, next_id: 0 }
    }
}

impl Default for ImBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl Backend for ImBackend {
    fn call(&mut self, method: &str, params: Value) -> Option<Value> {
        self.next_id += 1;
        let line =
            json!({"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params})
                .to_string();
        let resp: Value = serde_json::from_str(&self.server.handle_line(&line)?).ok()?;
        resp.get("result").cloned()
    }
}

pub struct Bridge<B: Backend> {
    be: B,
    /// A karukan composition is in progress.
    active: bool,
    /// Mozc's last output carried a preedit (Mozc owns the composition).
    mozc_active: bool,
    ime_on: bool,
    mode: u32,
    table: u32,
    ready: bool,
    started_init: bool,
    frame: Frame,
    /// Text committed by the engine that must be prepended to Mozc's next reply.
    pending: Option<String>,
    /// Hybrid mode: while karukan owns a composition, Mozc is fed the same keys in the background and
    /// the candidates it offers (symbols, emoji, ...) are appended to karukan's own.
    hybrid: bool,
    /// Mozc holds a background composition that has to be reverted when the karukan one ends.
    shadow: bool,
    plan: ShadowPlan,
    /// Candidates taken from Mozc, in the order they follow karukan's candidates.
    extras: Vec<(String, Option<String>)>,
    /// Mozc session of the last mirrored key.
    session: Option<u64>,
    /// Arguments of the last reply, to build it again with other candidates.
    last: Option<LastReply>,
    /// Mozc's composition is converted by the engine too.
    follow: Option<Follow>,
    /// The text around the caret when the last key reached Mozc with nothing composing: the
    /// context of the composition it starts. Later keys carry Mozc's own preedit in it.
    context: Option<(String, String)>,
}

struct Follow {
    reading: String,
    /// What the engine offers for `reading`, in the order shown.
    items: Vec<FollowItem>,
    /// The last amended reply.
    reply: Option<Vec<u8>>,
    /// The last whole conversion put in the preedit: (reading, text).
    settled: Option<(String, String)>,
    /// Whether the last amended reply shows a conversion in the preedit (Enter commits it then).
    inline: bool,
}

struct FollowItem {
    text: String,
    desc: Option<String>,
    pick: Pick,
}

/// How a tap on a follow item commits.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Pick {
    /// The live conversion, what a QWERTY composition would show inline: committed like Enter.
    Live,
    /// The engine's candidate at this index.
    Candidate(usize),
}

/// Ids of the engine's candidates in a follow reply, clear of Mozc's own (small, or negative).
const FOLLOW_ID: i64 = 0x3000_0000;

#[derive(Debug, Clone, Copy, PartialEq)]
enum ShadowPlan {
    None,
    Key,
    Revert,
}

struct LastReply {
    raw: Vec<u8>,
    id: Option<u64>,
    consumed: bool,
    result: Option<String>,
}

/// The most candidates taken from Mozc.
const MAX_EXTRAS: usize = 24;

impl<B: Backend> Bridge<B> {
    pub fn new(be: B) -> Self {
        Self {
            be,
            active: false,
            mozc_active: false,
            ime_on: true,
            mode: input::MODE_HIRAGANA,
            table: 0,
            ready: false,
            started_init: false,
            frame: Frame::default(),
            pending: None,
            hybrid: true,
            shadow: false,
            plan: ShadowPlan::None,
            extras: Vec::new(),
            session: None,
            last: None,
            follow: None,
            context: None,
        }
    }

    /// Turn the Mozc candidates on or off. Off: karukan's candidates only.
    pub fn set_hybrid(&mut self, on: bool) {
        self.hybrid = on;
    }

    pub fn backend(&mut self) -> &mut B {
        &mut self.be
    }

    /// Start loading the model (returns quickly; loading continues in the engine).
    pub fn init(&mut self) {
        if !self.started_init {
            self.started_init = true;
            self.be.call("init", json!({}));
        }
    }

    /// The model loads on a background thread and is installed at the top of the next
    /// `process_key`; while idle, a throwaway key picks it up. Safe only when nothing is composing.
    pub fn is_ready(&mut self) -> bool {
        if self.ready {
            return true;
        }
        self.init();
        if !self.active {
            self.be.call("process_key", json!({"keysym": 'a' as u32}));
            self.be.call("reset", json!({}));
        }
        self.ready = self.be.call("status", json!({})).is_some_and(|s| {
            s["initialized"].as_bool() == Some(true)
                && s["model_name"].as_str().is_some_and(|m| m != "loading")
        });
        self.ready
    }

    fn can_claim(&mut self) -> bool {
        !self.mozc_active
            && self.ime_on
            && self.mode == input::MODE_HIRAGANA
            && input::is_qwerty_romaji_table(self.table)
            && self.is_ready()
    }

    /// Handle one `Command` (input only) from the IME. `None`: pass it to Mozc.
    pub fn handle(&mut self, bytes: &[u8]) -> Option<Vec<u8>> {
        let (raw, inp) = input::decode_command_input(bytes)?;
        if inp.ty == input::SEND_KEY && !self.active && !self.mozc_active {
            self.context = Some((
                inp.preceding.clone().unwrap_or_default(),
                inp.following.clone().unwrap_or_default(),
            ));
        }
        match inp.ty {
            input::SEND_KEY => {
                if let Some(r) = self.on_follow_enter(raw, &inp) {
                    return Some(r);
                }
                self.on_key(raw, &inp)
            }
            input::TEST_SEND_KEY => self.on_key(raw, &inp),
            input::SEND_COMMAND => {
                if let Some(r) = self.on_follow_command(raw, &inp) {
                    return Some(r);
                }
                self.on_command(raw, &inp)
            }
            input::SET_REQUEST => {
                if let Some(t) = inp.request_table {
                    self.table = t;
                }
                None
            }
            input::DELETE_SESSION => {
                self.drop_composition();
                self.end_follow();
                None
            }
            _ => None,
        }
    }

    fn drop_composition(&mut self) {
        if self.active {
            self.be.call("reset", json!({}));
        }
        self.active = false;
        self.frame = Frame::default();
    }

    fn reply(&mut self, raw: &[u8], inp: &Input, consumed: bool, result: Option<&str>) -> Vec<u8> {
        if !self.active {
            self.extras.clear();
        }
        self.last = Some(LastReply {
            raw: raw.to_vec(),
            id: inp.id,
            consumed,
            result: result.map(str::to_string),
        });
        output::encode_command(raw, inp.id, consumed, result, &self.merged_frame())
    }

    /// How many of the shown candidates are karukan's (the rest come from Mozc).
    fn own_count(&self) -> usize {
        self.frame.cands.as_ref().map_or(0, |c| c.items.len())
    }

    /// The frame as shown: karukan's candidates followed by those of Mozc.
    fn merged_frame(&self) -> Frame {
        let mut f = self.frame.clone();
        if self.extras.is_empty() || !self.active {
            return f;
        }
        match f.cands.as_mut() {
            Some(c) => c.items.extend(self.extras.iter().cloned()),
            None => {
                f.cands = Some(CandView {
                    items: self.extras.clone(),
                    cursor: 0,
                    page: 0,
                    total_pages: 1,
                })
            }
        }
        f
    }

    fn on_key(&mut self, raw: &[u8], inp: &Input) -> Option<Vec<u8>> {
        let key = inp.key.as_ref()?;
        if key.is_release() {
            return None;
        }
        let test = inp.ty == input::TEST_SEND_KEY;
        if !(self.active || (self.can_claim() && input::is_romaji_start(key))) {
            return None;
        }
        let Some(sym) = input::keysym(key) else {
            return Some(self.reply(raw, inp, false, None));
        };
        if test {
            return Some(self.reply(raw, inp, true, None));
        }
        if !self.active {
            self.end_follow();
            self.set_context(inp);
        }
        let params = json!({
            "keysym": sym,
            "modifiers": {"shift": key.shift(), "control": key.ctrl(), "alt": key.alt()},
        });
        let res = self.be.call("process_key", params)?;
        let (consumed, committed) = self.apply(&res);
        self.refresh_active();
        if !committed.is_empty() {
            self.be.call("save_learning", json!({}));
        }
        Some(self.reply(raw, inp, consumed, Some(&committed)))
    }

    fn set_context(&mut self, inp: &Input) {
        let before = inp.preceding.clone().unwrap_or_default();
        let after = inp.following.clone().unwrap_or_default();
        self.send_context(&before, &after);
    }

    fn send_context(&mut self, before: &str, after: &str) {
        // Keep the request small: only the tail matters to the model.
        let tail: String = {
            let n = before.chars().count();
            before.chars().skip(n.saturating_sub(200)).collect()
        };
        let cursor = tail.chars().count();
        let text = format!("{tail}{after}");
        self.be.call(
            "set_surrounding_text",
            json!({"text": text, "cursor_pos": cursor}),
        );
    }

    /// Fold engine actions into `self.frame`; returns `(consumed, committed text)`.
    fn apply(&mut self, res: &Value) -> (bool, String) {
        let consumed = res["consumed"].as_bool().unwrap_or(false);
        let mut committed = String::new();
        for a in res["actions"].as_array().into_iter().flatten() {
            match a["type"].as_str() {
                Some("update_preedit") => {
                    let text = a["text"].as_str().unwrap_or_default().to_string();
                    if text.is_empty() {
                        self.frame.preedit = None;
                    } else {
                        let attrs = a["attributes"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(|x| {
                                (
                                    x["start"].as_u64().unwrap_or(0) as usize,
                                    x["end"].as_u64().unwrap_or(0) as usize,
                                    x["style"].as_str() == Some("highlight"),
                                )
                            })
                            .collect();
                        self.frame.preedit = Some(PreeditView {
                            caret: a["caret"].as_u64().unwrap_or(0) as usize,
                            text,
                            attrs,
                        });
                    }
                }
                Some("show_candidates") => {
                    let items = a["candidates"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|c| {
                            (
                                c["text"].as_str().unwrap_or_default().to_string(),
                                c["description"].as_str().map(str::to_string),
                            )
                        })
                        .collect();
                    self.frame.cands = Some(CandView {
                        items,
                        cursor: a["cursor"].as_u64().unwrap_or(0) as usize,
                        page: a["page"].as_u64().unwrap_or(0) as usize,
                        total_pages: a["total_pages"].as_u64().unwrap_or(1) as usize,
                    });
                }
                Some("hide_candidates") => self.frame.cands = None,
                Some("commit") => {
                    committed.push_str(a["text"].as_str().unwrap_or_default());
                    self.frame.preedit = None;
                    self.frame.cands = None;
                }
                _ => {}
            }
        }
        (consumed, committed)
    }

    fn refresh_active(&mut self) {
        let empty = self
            .be
            .call("status", json!({}))
            .is_none_or(|s| s["state"].as_str() == Some("empty"));
        self.active = !empty;
        if empty {
            self.frame = Frame::default();
        }
    }

    fn on_command(&mut self, raw: &[u8], inp: &Input) -> Option<Vec<u8>> {
        let cmd = inp.command.as_ref()?;
        if !self.active {
            return None;
        }
        use input::*;
        match cmd.ty {
            SC_SUBMIT => {
                let res = self.be.call("commit", json!({}))?;
                let (_, text) = self.apply(&res);
                self.refresh_active();
                self.be.call("save_learning", json!({}));
                Some(self.reply(raw, inp, true, Some(&text)))
            }
            SC_REVERT | SC_RESET_CONTEXT => {
                self.drop_composition();
                Some(self.reply(raw, inp, true, None))
            }
            SC_SELECT_CANDIDATE | SC_SUBMIT_CANDIDATE => {
                let idx = cmd.id.filter(|i| *i >= 0).unwrap_or(0) as usize;
                let n = self.own_count();
                if idx >= n {
                    // A candidate of Mozc: commit it as it is and end the composition.
                    let Some((text, _)) = self.extras.get(idx - n).cloned() else {
                        return Some(self.reply(raw, inp, true, None));
                    };
                    self.drop_composition();
                    self.extras.clear();
                    return Some(self.reply(raw, inp, true, Some(&text)));
                }
                let res = self
                    .be
                    .call("select_candidate", json!({"page_index": idx}))?;
                let (consumed, text) = self.apply(&res);
                self.refresh_active();
                if !text.is_empty() {
                    self.be.call("save_learning", json!({}));
                }
                Some(self.reply(raw, inp, consumed, Some(&text)))
            }
            SC_MOVE_CURSOR => {
                let target = cmd.cursor_position.unwrap_or(0) as i64;
                let at = self.frame.preedit.as_ref().map_or(0, |p| p.caret) as i64;
                let (key, n) = if target < at {
                    (0xff51u32, at - target)
                } else {
                    (0xff53, target - at)
                };
                for _ in 0..n.min(64) {
                    if let Some(res) = self.be.call("process_key", json!({"keysym": key})) {
                        self.apply(&res);
                    }
                }
                Some(self.reply(raw, inp, true, None))
            }
            SC_HIGHLIGHT_CANDIDATE => Some(self.reply(raw, inp, true, None)),
            SC_TURN_OFF_IME | SC_TURN_ON_IME | SC_SWITCH_COMPOSITION_MODE | SC_COMMIT_RAW_TEXT => {
                // Commit what is being typed, then let Mozc perform the switch.
                if let Some(res) = self.be.call("commit", json!({})) {
                    let (_, text) = self.apply(&res);
                    if !text.is_empty() {
                        self.pending = Some(text);
                        self.be.call("save_learning", json!({}));
                    }
                }
                self.drop_composition();
                None
            }
            _ => Some(self.reply(raw, inp, true, None)),
        }
    }

    /// After [`Bridge::handle`] answered `bytes`: the command to give Mozc in the background, if any.
    /// A key of a karukan composition is mirrored; when the composition has ended, Mozc is told to revert.
    /// Hand Mozc's answer to [`Bridge::shadow_reply`].
    pub fn shadow_command(&mut self, bytes: &[u8]) -> Option<Vec<u8>> {
        self.plan = ShadowPlan::None;
        if !self.hybrid {
            return None;
        }
        let (_, inp) = input::decode_command_input(bytes)?;
        let press = inp.ty == input::SEND_KEY && inp.key.as_ref().is_some_and(|k| !k.is_release());
        if self.active && press {
            self.session = inp.id.or(self.session);
            self.plan = ShadowPlan::Key;
            return Some(bytes.to_vec());
        }
        if !self.active && self.shadow {
            self.plan = ShadowPlan::Revert;
            return Some(self.revert_command(inp.id));
        }
        None
    }

    /// Mozc's answer to the command of [`Bridge::shadow_command`]. Returns the karukan reply with the
    /// candidates of Mozc added, or `None` to keep it as it is.
    pub fn shadow_reply(&mut self, mozc_reply: &[u8]) -> Option<Vec<u8>> {
        let plan = std::mem::replace(&mut self.plan, ShadowPlan::None);
        match plan {
            ShadowPlan::Revert => {
                self.shadow = false;
                self.extras.clear();
                None
            }
            ShadowPlan::Key => {
                let (found, composing) = mozc_candidates(mozc_reply);
                self.shadow = composing;
                let mut extras: Vec<(String, Option<String>)> = Vec::new();
                if composing {
                    let own: Vec<&str> = self
                        .frame
                        .cands
                        .iter()
                        .flat_map(|c| c.items.iter().map(|i| i.0.as_str()))
                        .collect();
                    for (text, desc) in found {
                        if text.is_empty()
                            || own.contains(&text.as_str())
                            || extras.iter().any(|e| e.0 == text)
                        {
                            continue;
                        }
                        extras.push((text, desc));
                        if extras.len() >= MAX_EXTRAS {
                            break;
                        }
                    }
                }
                self.extras = extras;
                let last = self.last.as_ref()?;
                Some(output::encode_command(
                    &last.raw,
                    last.id,
                    last.consumed,
                    last.result.as_deref(),
                    &self.merged_frame(),
                ))
            }
            ShadowPlan::None => None,
        }
    }

    /// Before `bytes` goes to Mozc for real (karukan did not take it): the command that clears the
    /// background composition first, if there is one and `bytes` could touch it.
    pub fn shadow_revert(&mut self, bytes: &[u8]) -> Option<Vec<u8>> {
        if !self.shadow {
            return None;
        }
        let (_, inp) = input::decode_command_input(bytes)?;
        let harmless = inp.ty == input::TEST_SEND_KEY
            || (inp.ty == input::SEND_KEY && inp.key.as_ref().is_some_and(|k| k.is_release()));
        if harmless {
            return None;
        }
        self.shadow = false;
        self.extras.clear();
        Some(self.revert_command(inp.id))
    }

    fn revert_command(&self, id: Option<u64>) -> Vec<u8> {
        let mut w = proto::W::new();
        w.message(1, |i| {
            i.uint(1, input::SEND_COMMAND as u64);
            if let Some(id) = id.or(self.session) {
                i.uint(2, id);
            }
            i.message(4, |c| c.uint(1, input::SC_REVERT as u64));
        });
        w.0
    }

    /// Called after Mozc handled `bytes`; returns a replacement reply when the bridge has to
    /// amend it.
    pub fn observe(&mut self, bytes: &[u8], mozc_reply: &[u8]) -> Option<Vec<u8>> {
        let inp = input::decode_command_input(bytes).map(|(_, i)| i);
        let top = proto::parse(mozc_reply);
        let out = top
            .as_ref()
            .and_then(|t| proto::get(t, 2))
            .and_then(|v| v.as_bytes())
            .and_then(proto::parse);
        // Only a key or a command answers with the composition as it stands; the reply to anything
        // else (a test key, a request) can lack a preedit while Mozc is still composing.
        let composition_reply = inp
            .as_ref()
            .is_some_and(|i| i.ty == input::SEND_KEY || i.ty == input::SEND_COMMAND);
        if let Some(o) = &out {
            if composition_reply {
                self.mozc_active = proto::get(o, 5).is_some();
            }
            if let Some(st) = proto::get(o, 13)
                .and_then(|v| v.as_bytes())
                .and_then(proto::parse)
            {
                if let Some(a) = proto::get(&st, 1).and_then(|v| v.as_u64()) {
                    self.ime_on = a != 0;
                }
                if let Some(m) = proto::get(&st, 2).and_then(|v| v.as_u64()) {
                    self.mode = m as u32;
                }
            } else if let Some(Val::Varint(m)) = proto::get(o, 2) {
                self.mode = m as u32;
            }
        }
        if let Some(c) = inp.as_ref().and_then(|i| i.command.as_ref()) {
            match c.ty {
                input::SC_TURN_OFF_IME => self.ime_on = false,
                input::SC_TURN_ON_IME => {
                    self.ime_on = true;
                    if let Some(m) = c.composition_mode {
                        self.mode = m;
                    }
                }
                input::SC_SWITCH_COMPOSITION_MODE => {
                    if let Some(m) = c.composition_mode {
                        self.mode = m;
                    }
                }
                _ => {}
            }
        }
        let amended = if composition_reply {
            // The last kana is settled once Gboard stops the toggling — a second after a tap — or
            // at once when the key cannot toggle (a flick, a deletion).
            let settled = inp.as_ref().is_some_and(|i| match i.ty {
                input::SEND_COMMAND => i
                    .command
                    .as_ref()
                    .is_some_and(|c| c.ty == input::SC_STOP_KEY_TOGGLING),
                input::SEND_KEY => !i.key.as_ref().is_some_and(input::may_toggle),
                _ => false,
            });
            self.follow_reply(mozc_reply, settled)
        } else {
            None
        };
        match self.pending.take() {
            Some(text) => output::inject_result(amended.as_deref().unwrap_or(mozc_reply), &text),
            None => amended,
        }
    }

    /// Follow Mozc's composition: hand its reading to the engine, show the engine's conversion in
    /// the preedit, and put the engine's candidates ahead of Mozc's. `settled`: the reply to Gboard's
    /// stop-toggling, after which the last kana no longer changes. `None` keeps Mozc's reply as it is.
    fn follow_reply(&mut self, mozc_reply: &[u8], settled: bool) -> Option<Vec<u8>> {
        let reply = self.follow_amend(mozc_reply, settled);
        if let Some(f) = self.follow.as_mut() {
            if reply.is_none() {
                f.inline = false;
            }
            f.reply = reply.clone();
        }
        reply
    }

    fn follow_amend(&mut self, mozc_reply: &[u8], settled: bool) -> Option<Vec<u8>> {
        // The revert after a commit rides on the hybrid exchange, so no hybrid, no follow.
        let can = self.hybrid
            && !self.active
            && self.ime_on
            && self.mode == input::MODE_HIRAGANA
            && !input::is_qwerty_romaji_table(self.table);
        let Some(view) = can.then(|| mozc_view(mozc_reply)).flatten() else {
            self.end_follow();
            return None;
        };
        if view.reading.chars().any(|c| c.is_ascii_alphabetic()) {
            // Romaji still pending (a Godan keyboard): not a reading yet.
            self.end_follow();
            return None;
        }
        if view.converting {
            // Mozc's candidates are for one of its segments now: leave them to it.
            return None;
        }
        if !self.is_ready() {
            return None;
        }
        if self.follow.is_none() {
            let (before, after) = self.context.take().unwrap_or_default();
            self.send_context(&before, &after);
        }
        if self
            .follow
            .as_ref()
            .is_none_or(|f| f.reading != view.reading)
        {
            let res = self
                .be
                .call("set_reading", json!({"text": view.reading}))
                .unwrap_or_default();
            let settled = self.follow.take().and_then(|f| f.settled);
            self.follow = Some(Follow {
                items: follow_items(&res, &view.reading),
                reading: view.reading.clone(),
                reply: None,
                settled,
                inline: false,
            });
        }
        let f = self.follow.as_mut()?;
        if f.items.is_empty() {
            return None;
        }
        // The preedit: the whole conversion once the last kana is settled; while it may still
        // toggle, the conversion shown so far plus the kana typed since, so the kana being chosen
        // stays visible. A caret moved into the composition keeps Mozc's kana.
        let live = f
            .items
            .first()
            .filter(|it| it.pick == Pick::Live)
            .map(|it| it.text.clone());
        let whole = live.unwrap_or_else(|| view.reading.clone());
        let shown = if !view.caret_at_end {
            None
        } else if settled {
            f.settled = Some((view.reading.clone(), whole.clone()));
            Some(whole)
        } else {
            match &f.settled {
                Some((r, t)) if view.reading.starts_with(r.as_str()) => {
                    Some(format!("{t}{}", &view.reading[r.len()..]))
                }
                // Typing began here, so the kana may still toggle.
                None => None,
                // An edit (a deletion): nothing is toggling.
                Some(_) => {
                    f.settled = Some((view.reading.clone(), whole.clone()));
                    Some(whole)
                }
            }
        };
        let shown = shown.filter(|s| *s != view.reading);
        f.inline = shown.is_some();
        let mut list: Vec<ListItem> = f
            .items
            .iter()
            .enumerate()
            .map(|(i, it)| ListItem {
                id: FOLLOW_ID + i as i64,
                text: it.text.clone(),
                desc: it.desc.clone(),
            })
            .collect();
        for it in view.items {
            if !it.text.is_empty() && !list.iter().any(|l| l.text == it.text) {
                list.push(it);
            }
        }
        let reply = output::replace_candidates(mozc_reply, &list, view.category)?;
        match shown {
            Some(s) => output::replace_preedit(&reply, &s, &view.reading),
            None => Some(reply),
        }
    }

    /// Enter while the preedit shows the engine's conversion: commit that, as a QWERTY
    /// composition does.
    fn on_follow_enter(&mut self, raw: &[u8], inp: &Input) -> Option<Vec<u8>> {
        let key = inp.key.as_ref()?;
        let enter = matches!(key.special, Some(input::SK_ENTER | input::SK_VIRTUAL_ENTER))
            && !key.shift()
            && !key.ctrl()
            && !key.alt()
            && !key.is_release();
        if !enter || !self.follow.as_ref().is_some_and(|f| f.inline) {
            return None;
        }
        let res = self
            .be
            .call("process_key", json!({"keysym": KEY_RETURN}))
            .unwrap_or_default();
        let fallback = self
            .follow
            .as_ref()
            .and_then(|f| f.settled.as_ref().map(|s| s.1.clone()))
            .unwrap_or_default();
        Some(self.commit_follow(raw, inp, &res, fallback))
    }

    /// The engine committed (a tap or Enter): hand its text to Gboard and revert Mozc's kana.
    fn commit_follow(&mut self, raw: &[u8], inp: &Input, res: &Value, fallback: String) -> Vec<u8> {
        let mut text: String = res["actions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|a| a["type"].as_str() == Some("commit"))
            .filter_map(|a| a["text"].as_str())
            .collect();
        if text.is_empty() {
            text = fallback;
        } else {
            self.be.call("save_learning", json!({}));
        }
        self.end_follow();
        // Mozc still holds the kana: the hybrid exchange reverts it (shadow_command).
        self.mozc_active = false;
        self.shadow = true;
        self.session = inp.id.or(self.session);
        self.extras.clear();
        output::encode_command(raw, inp.id, true, Some(&text), &Frame::default())
    }

    /// A tap on one of the engine's candidates in a follow reply: commit it through the engine.
    fn on_follow_command(&mut self, raw: &[u8], inp: &Input) -> Option<Vec<u8>> {
        let cmd = inp.command.as_ref()?;
        let f = self.follow.as_ref()?;
        let idx = cmd.id? as i64 - FOLLOW_ID;
        let idx = usize::try_from(idx).ok().filter(|i| *i < f.items.len())?;
        use input::*;
        match cmd.ty {
            SC_SELECT_CANDIDATE | SC_SUBMIT_CANDIDATE => {
                let fallback = f.items[idx].text.clone();
                let res = match f.items[idx].pick {
                    Pick::Live => self.be.call("process_key", json!({"keysym": KEY_RETURN})),
                    Pick::Candidate(i) => {
                        self.be.call("select_candidate", json!({"page_index": i}))
                    }
                }
                .unwrap_or_default();
                Some(self.commit_follow(raw, inp, &res, fallback))
            }
            // Mozc does not know the id: show the same thing again.
            SC_HIGHLIGHT_CANDIDATE => f.reply.clone(),
            _ => None,
        }
    }

    fn end_follow(&mut self) {
        if self.follow.take().is_some() {
            self.be.call("set_reading", json!({"text": ""}));
        }
    }
}

/// XKB Return: commits the composition as the engine shows it.
const KEY_RETURN: u32 = 0xff0d;

/// What a follow offers for `reading`, from the engine's `KeyResult`: first its live conversion —
/// a QWERTY composition shows it inline, and here the candidate row is the only place to show
/// it — then its candidates (the last `show_candidates`), without the live text again.
fn follow_items(res: &Value, reading: &str) -> Vec<FollowItem> {
    let mut live = None;
    let mut cands: Vec<(String, Option<String>)> = Vec::new();
    for a in res["actions"].as_array().into_iter().flatten() {
        match a["type"].as_str() {
            Some("update_preedit") => {
                live = a["text"].as_str().map(str::to_string);
            }
            Some("show_candidates") => {
                cands = a["candidates"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|c| {
                        let text = c["text"].as_str().filter(|t| !t.is_empty())?;
                        Some((
                            text.to_string(),
                            c["description"].as_str().map(str::to_string),
                        ))
                    })
                    .collect();
            }
            Some("hide_candidates") => cands.clear(),
            _ => {}
        }
    }
    // The reading itself is no conversion: Mozc's own kana candidate has it.
    let live = live.filter(|l| !l.is_empty() && l != reading);
    let mut items: Vec<FollowItem> = live
        .iter()
        .map(|l| FollowItem {
            text: l.clone(),
            desc: None,
            pick: Pick::Live,
        })
        .collect();
    for (i, (text, desc)) in cands.into_iter().enumerate() {
        if live.as_deref() != Some(text.as_str()) {
            items.push(FollowItem {
                text,
                desc,
                pick: Pick::Candidate(i),
            });
        }
    }
    items
}

/// What a Mozc reply shows: the reading of its preedit, whether a segment is being converted,
/// and its candidates (ids as Mozc gave them).
struct MozcView {
    reading: String,
    /// The preedit's caret is after the last character.
    caret_at_end: bool,
    converting: bool,
    items: Vec<ListItem>,
    category: u64,
}

fn mozc_view(reply: &[u8]) -> Option<MozcView> {
    let top = proto::parse(reply)?;
    let out = proto::parse(proto::get(&top, 2)?.as_bytes()?)?;
    let pre = proto::parse(proto::get(&out, 5)?.as_bytes()?)?;
    let mut reading = String::new();
    let mut converting = proto::get(&pre, 3).is_some();
    for (f, v) in &pre {
        let (2, Val::Group(g) | Val::Len(g)) = (*f, v) else {
            continue;
        };
        let Some(seg) = proto::parse(g) else { continue };
        if proto::get(&seg, 3).and_then(|v| v.as_u64()) == Some(output::ANN_HIGHLIGHT) {
            converting = true;
        }
        if let Some(t) = proto::get(&seg, 4).and_then(|v| v.as_str()) {
            reading.push_str(&t);
        }
    }
    if reading.is_empty() {
        return None;
    }
    let caret_at_end = proto::get(&pre, 1)
        .and_then(|v| v.as_u64())
        .is_none_or(|c| c as usize == reading.chars().count());
    let mut items = Vec::new();
    let mut category = output::CAT_SUGGESTION;
    if let Some(list) = proto::get(&out, 14)
        .and_then(|v| v.as_bytes())
        .and_then(proto::parse)
    {
        if proto::get(&list, 1).is_some() {
            converting = true; // a focused candidate: a conversion
        }
        if let Some(c) = proto::get(&list, 3).and_then(|v| v.as_u64()) {
            if c == output::CAT_CONVERSION {
                converting = true;
            }
            category = c;
        }
        for (f, v) in &list {
            if *f != 2 {
                continue;
            }
            let Some(c) = v.as_bytes().and_then(proto::parse) else {
                continue;
            };
            let Some(text) = proto::get(&c, 4).and_then(|v| v.as_str()) else {
                continue;
            };
            let id = proto::get(&c, 1).and_then(|v| v.as_u64()).unwrap_or(0) as i32 as i64;
            let desc = proto::get(&c, 5)
                .and_then(|v| v.as_bytes())
                .and_then(proto::parse)
                .and_then(|a| proto::get(&a, 3).and_then(|v| v.as_str()))
                .filter(|d| !d.is_empty());
            items.push(ListItem { id, text, desc });
        }
    }
    Some(MozcView {
        reading,
        caret_at_end,
        converting,
        items,
        category,
    })
}

/// The candidates of a Mozc reply (`Output.all_candidate_words`: value in field 4, annotation
/// description in field 5.3) and whether Mozc has a composition (`Output.preedit`, field 5).
fn mozc_candidates(reply: &[u8]) -> (Vec<(String, Option<String>)>, bool) {
    let Some(top) = proto::parse(reply) else {
        return (Vec::new(), false);
    };
    let Some(out) = proto::get(&top, 2)
        .and_then(|v| v.as_bytes())
        .and_then(proto::parse)
    else {
        return (Vec::new(), false);
    };
    let composing = proto::get(&out, 5).is_some();
    let mut found = Vec::new();
    if let Some(list) = proto::get(&out, 14)
        .and_then(|v| v.as_bytes())
        .and_then(proto::parse)
    {
        for (f, v) in &list {
            if *f != 2 {
                continue;
            }
            let Some(c) = v.as_bytes().and_then(proto::parse) else {
                continue;
            };
            let Some(text) = proto::get(&c, 4).and_then(|v| v.as_str()) else {
                continue;
            };
            let desc = proto::get(&c, 5)
                .and_then(|v| v.as_bytes())
                .and_then(proto::parse)
                .and_then(|a| proto::get(&a, 3).and_then(|v| v.as_str()))
                .filter(|d| !d.is_empty());
            found.push((text, desc));
        }
    }
    (found, composing)
}
