//! Per-composition ownership between Mozc and the karukan engine.
//!
//! The IME sends every request to [`Bridge::handle`] first. `None` means "not ours, let Mozc run
//! it"; after Mozc ran, [`Bridge::observe`] tells the bridge what Mozc did (and may amend the
//! reply). A composition belongs to one engine from its first key until it is empty again, so the
//! two never mix: keys the engine cannot translate (kana from a flick keyboard, for example) start
//! no karukan composition at all.

use serde_json::{Value, json};

use crate::input::{self, Input};
use crate::output::{self, CandView, Frame, PreeditView};
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
        let line = json!({"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params})
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
}

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
        match inp.ty {
            input::SEND_KEY | input::TEST_SEND_KEY => self.on_key(raw, &inp),
            input::SEND_COMMAND => self.on_command(raw, &inp),
            input::SET_REQUEST => {
                if let Some(t) = inp.request_table {
                    self.table = t;
                }
                None
            }
            input::DELETE_SESSION => {
                self.drop_composition();
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
        if !self.active && !(self.can_claim() && input::is_romaji_start(key)) {
            return None;
        }
        let Some(sym) = input::keysym(key) else {
            return Some(self.reply(raw, inp, false, None));
        };
        if test {
            return Some(self.reply(raw, inp, true, None));
        }
        if !self.active {
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
        // Keep the request small: only the tail matters to the model.
        let tail: String = {
            let n = before.chars().count();
            before.chars().skip(n.saturating_sub(200)).collect()
        };
        let cursor = tail.chars().count();
        let text = format!("{tail}{after}");
        self.be
            .call("set_surrounding_text", json!({"text": text, "cursor_pos": cursor}));
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
                let res = self.be.call("select_candidate", json!({"page_index": idx}))?;
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
                let (key, n) = if target < at { (0xff51u32, at - target) } else { (0xff53, target - at) };
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
                        if text.is_empty() || own.contains(&text.as_str()) || extras.iter().any(|e| e.0 == text) {
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
        if let Some(o) = &out {
            self.mozc_active = proto::get(o, 5).is_some();
            if let Some(st) = proto::get(o, 13).and_then(|v| v.as_bytes()).and_then(proto::parse) {
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
        let text = self.pending.take()?;
        output::inject_result(mozc_reply, &text)
    }
}

/// The candidates of a Mozc reply (`Output.all_candidate_words`: value in field 4, annotation
/// description in field 5.3) and whether Mozc has a composition (`Output.preedit`, field 5).
fn mozc_candidates(reply: &[u8]) -> (Vec<(String, Option<String>)>, bool) {
    let Some(top) = proto::parse(reply) else {
        return (Vec::new(), false);
    };
    let Some(out) = proto::get(&top, 2).and_then(|v| v.as_bytes()).and_then(proto::parse) else {
        return (Vec::new(), false);
    };
    let composing = proto::get(&out, 5).is_some();
    let mut found = Vec::new();
    if let Some(list) = proto::get(&out, 14).and_then(|v| v.as_bytes()).and_then(proto::parse) {
        for (f, v) in &list {
            if *f != 2 {
                continue;
            }
            let Some(c) = v.as_bytes().and_then(proto::parse) else { continue };
            let Some(text) = proto::get(&c, 4).and_then(|v| v.as_str()) else { continue };
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
