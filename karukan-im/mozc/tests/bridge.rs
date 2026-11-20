use karukan_mozc::bridge::{Backend, Bridge};
use karukan_mozc::proto::{self, Val, W};
use serde_json::{Value, json};
use std::collections::VecDeque;

/// Scripted engine: `process_key` pops canned results, `status` reflects `state`.
struct Fake {
    queue: VecDeque<Value>,
    state: &'static str,
    calls: Vec<String>,
}

impl Backend for Fake {
    fn call(&mut self, method: &str, params: Value) -> Option<Value> {
        self.calls.push(format!("{method} {params}"));
        match method {
            "status" => Some(json!({"initialized": true, "model_name": "m", "state": self.state})),
            "process_key" | "commit" | "select_candidate" => {
                let r = self.queue.pop_front()?;
                self.state = if r["_state"].is_string() {
                    match r["_state"].as_str().unwrap() {
                        "composing" => "composing",
                        "conversion" => "conversion",
                        _ => "empty",
                    }
                } else {
                    self.state
                };
                Some(r)
            }
            _ => Some(json!({})),
        }
    }
}

fn key_cmd(ty: u32, f: impl FnOnce(&mut W)) -> Vec<u8> {
    let mut w = W::new();
    w.message(1, |i| {
        i.uint(1, ty as u64);
        i.uint(2, 5);
        i.message(3, f);
        i.message(6, |c| c.string(1, "前の文"));
    });
    w.0
}

fn cmd_cmd(ty: u32, id: Option<i64>) -> Vec<u8> {
    let mut w = W::new();
    w.message(1, |i| {
        i.uint(1, 5);
        i.uint(2, 5);
        i.message(4, |c| {
            c.uint(1, ty as u64);
            if let Some(id) = id {
                c.int(2, id);
            }
        });
    });
    w.0
}

fn output(cmd: &[u8]) -> Vec<(u32, Vec<u8>)> {
    let top = proto::parse(cmd).unwrap();
    let o = proto::get(&top, 2).unwrap().as_bytes().unwrap();
    proto::parse(o)
        .unwrap()
        .into_iter()
        .map(|(f, v)| {
            (
                f,
                match v {
                    Val::Varint(x) => x.to_le_bytes().to_vec(),
                    Val::Len(b) | Val::Group(b) => b.to_vec(),
                    _ => vec![],
                },
            )
        })
        .collect()
}

fn bridge() -> Bridge<Fake> {
    let f = Fake { queue: VecDeque::new(), state: "empty", calls: vec![] };
    Bridge::new(f)
}

fn composing(text: &str, cands: &[&str]) -> Value {
    json!({"consumed": true, "_state": "composing", "actions": [
        {"type": "update_preedit", "text": text, "caret": text.chars().count(),
         "attributes": [{"start": 0, "end": text.chars().count(), "style": "underline"}]},
        {"type": "show_candidates", "candidates": cands.iter().map(|c| json!({"text": c})).collect::<Vec<_>>(),
         "cursor": 0, "page": 0, "total_pages": 1}
    ]})
}

#[test]
fn claims_letters_and_builds_preedit_and_candidates() {
    let mut b = bridge();
    b.backend().queue.push_back(composing("き", &["き", "木"]));
    let out = b.handle(&key_cmd(3, |k| k.uint(1, 'k' as u64))).expect("claimed");
    let o = output(&out);
    assert!(o.iter().any(|(f, _)| *f == 5), "preedit present");
    assert!(o.iter().any(|(f, _)| *f == 14), "all_candidate_words present");
    // surrounding text was passed once, before the key
    assert!(b.backend().calls.iter().any(|c| c.starts_with("set_surrounding_text")));
}

#[test]
fn digit_start_and_non_romaji_are_routed() {
    let mut b = bridge();
    // Space/Enter/Backspace with nothing composed: Mozc's.
    assert!(b.handle(&key_cmd(3, |k| k.uint(3, 4))).is_none());
    assert!(b.handle(&key_cmd(3, |k| k.uint(3, 12))).is_none());
    // Flick kana: Mozc's.
    assert!(
        b.handle(&key_cmd(3, |k| {
            k.uint(1, '1' as u64);
            k.string(5, "あ");
        }))
        .is_none()
    );
    // Digit: karukan's.
    b.backend().queue.push_back(composing("1", &[]));
    assert!(b.handle(&key_cmd(3, |k| k.uint(1, '1' as u64))).is_some());
}

#[test]
fn owner_sticks_until_empty_and_commits() {
    let mut b = bridge();
    b.backend().queue.push_back(composing("き", &[]));
    b.handle(&key_cmd(3, |k| k.uint(1, 'k' as u64))).unwrap();
    // Space is now ours.
    b.backend().queue.push_back(composing("木", &["木"]));
    assert!(b.handle(&key_cmd(3, |k| k.uint(3, 4))).is_some());
    // Enter commits
    b.backend().queue.push_back(json!({"consumed": true, "_state": "empty", "actions": [
        {"type": "hide_candidates"}, {"type": "commit", "text": "木"}]}));
    let out = b.handle(&key_cmd(3, |k| k.uint(3, 5))).unwrap();
    let o = output(&out);
    let result = o.iter().find(|(f, _)| *f == 4).expect("result");
    let r = proto::parse(&result.1).unwrap();
    assert_eq!(proto::get(&r, 2).unwrap().as_str().unwrap(), "木");
    assert!(o.iter().all(|(f, _)| *f != 5), "no preedit after commit");
    // back to Mozc
    assert!(b.handle(&key_cmd(3, |k| k.uint(3, 5))).is_none());
}

#[test]
fn flick_request_table_disables_claiming() {
    let mut b = bridge();
    let mut w = W::new();
    w.message(1, |i| {
        i.uint(1, 17);
        i.message(9, |r| r.int(4, 16));
    });
    assert!(b.handle(&w.0).is_none());
    assert!(b.handle(&key_cmd(3, |k| k.uint(1, 'k' as u64))).is_none());
}

#[test]
fn gboard_qwerty_table_is_claimed() {
    // Gboard's QWERTY romaji keyboard sends QWERTY_MOBILE_TO_HIRAGANA (20).
    let mut b = bridge();
    let mut w = W::new();
    w.message(1, |i| {
        i.uint(1, 17);
        i.message(9, |r| r.int(4, 20));
    });
    assert!(b.handle(&w.0).is_none());
    b.backend().queue.push_back(composing("き", &[]));
    assert!(b.handle(&key_cmd(3, |k| k.uint(1, 'k' as u64))).is_some());
}

#[test]
fn mozc_composition_keeps_ownership() {
    let mut b = bridge();
    // Mozc replies with a preedit
    let mut mo = W::new();
    mo.message(5, |p| p.uint(1, 1));
    let mut reply = W::new();
    reply.bytes(1, &[0x08, 0x03]);
    reply.bytes(2, &mo.0);
    assert!(b.observe(&key_cmd(3, |k| k.uint(1, 'a' as u64)), &reply.0).is_none());
    // While Mozc composes, even letters stay with Mozc.
    assert!(b.handle(&key_cmd(3, |k| k.uint(1, 'k' as u64))).is_none());
}

#[test]
fn mode_switch_commits_then_injects_into_mozc_reply() {
    let mut b = bridge();
    b.backend().queue.push_back(composing("き", &[]));
    b.handle(&key_cmd(3, |k| k.uint(1, 'k' as u64))).unwrap();
    b.backend().queue.push_back(json!({"consumed": true, "_state": "empty", "actions": [
        {"type": "commit", "text": "き"}]}));
    // SWITCH_COMPOSITION_MODE: passes to Mozc ...
    let c = cmd_cmd(5, None);
    assert!(b.handle(&c).is_none());
    // ... and the reply gets the committed text.
    let mut mo = W::new();
    mo.bool(3, true);
    let mut reply = W::new();
    reply.bytes(1, &[0x08, 0x05]);
    reply.bytes(2, &mo.0);
    let merged = b.observe(&c, &reply.0).expect("amended");
    let o = output(&merged);
    let r = proto::parse(&o.iter().find(|(f, _)| *f == 4).unwrap().1).unwrap();
    assert_eq!(proto::get(&r, 2).unwrap().as_str().unwrap(), "き");
}

#[test]
fn select_candidate_by_id() {
    let mut b = bridge();
    b.backend().queue.push_back(composing("きょう", &["今日", "京", "教"]));
    b.handle(&key_cmd(3, |k| k.uint(1, 'k' as u64))).unwrap();
    b.backend().queue.push_back(json!({"consumed": true, "_state": "empty", "actions": [
        {"type": "commit", "text": "京"}]}));
    let out = b.handle(&cmd_cmd(7, Some(1))).unwrap();
    assert!(b.backend().calls.iter().any(|c| c == r#"select_candidate {"page_index":1}"#));
    let o = output(&out);
    assert!(o.iter().any(|(f, _)| *f == 4));
}
