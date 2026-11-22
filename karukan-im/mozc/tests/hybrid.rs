//! Hybrid mode: the candidates of Mozc are added to karukan's.

use karukan_mozc::bridge::{Backend, Bridge};
use karukan_mozc::proto::{self, Val, W};
use serde_json::{Value, json};
use std::collections::VecDeque;

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
            // The throwaway key that makes the engine pick up its model (Bridge::is_ready).
            "process_key" if params["keysym"] == 97 => Some(json!({})),
            "process_key" | "commit" | "select_candidate" => {
                let r = self.queue.pop_front()?;
                if let Some(s) = r["_state"].as_str() {
                    self.state = match s {
                        "composing" => "composing",
                        "conversion" => "conversion",
                        _ => "empty",
                    };
                }
                Some(r)
            }
            _ => Some(json!({})),
        }
    }
}

fn bridge() -> Bridge<Fake> {
    Bridge::new(Fake {
        queue: VecDeque::new(),
        state: "empty",
        calls: vec![],
    })
}

fn key_cmd(ty: u32, f: impl FnOnce(&mut W)) -> Vec<u8> {
    let mut w = W::new();
    w.message(1, |i| {
        i.uint(1, ty as u64);
        i.uint(2, 5);
        i.message(3, f);
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

fn composing(text: &str, cands: &[&str]) -> Value {
    let n = text.chars().count();
    let mut actions = vec![json!({"type": "update_preedit", "text": text, "caret": n,
        "attributes": [{"start": 0, "end": n, "style": "underline"}]})];
    if !cands.is_empty() {
        actions.push(json!({"type": "show_candidates",
            "candidates": cands.iter().map(|c| json!({"text": c})).collect::<Vec<_>>(),
            "cursor": 0, "page": 0, "total_pages": 1}));
    }
    json!({"consumed": true, "_state": "composing", "actions": actions})
}

/// A Mozc reply with the given candidates (value, description) and, optionally, a preedit.
fn mozc_reply(cands: &[(&str, Option<&str>)], preedit: bool) -> Vec<u8> {
    let mut o = W::new();
    if preedit {
        o.message(5, |p| p.uint(1, 1));
    }
    o.message(14, |l| {
        for (i, (text, desc)) in cands.iter().enumerate() {
            l.message(2, |c| {
                c.int(1, i as i64);
                c.string(4, text);
                if let Some(d) = desc {
                    c.message(5, |a| a.string(3, d));
                }
            });
        }
    });
    let mut w = W::new();
    w.bytes(1, &[0x08, 0x03]);
    w.bytes(2, &o.0);
    w.0
}

fn candidate_values(cmd: &[u8]) -> Vec<String> {
    let o = output(cmd);
    let list = proto::parse(&o.iter().find(|(f, _)| *f == 14).expect("candidates").1).unwrap();
    list.iter()
        .filter(|(f, _)| *f == 2)
        .map(|(_, v)| {
            let c = proto::parse(v.as_bytes().unwrap()).unwrap();
            proto::get(&c, 4).unwrap().as_str().unwrap()
        })
        .collect()
}

/// Input{ type = SEND_COMMAND, command{ type = REVERT } }
fn is_revert(cmd: &[u8]) -> bool {
    let top = proto::parse(cmd).unwrap();
    let i = proto::parse(proto::get(&top, 1).unwrap().as_bytes().unwrap()).unwrap();
    let c = proto::parse(proto::get(&i, 4).unwrap().as_bytes().unwrap()).unwrap();
    proto::get(&i, 1) == Some(Val::Varint(5)) && proto::get(&c, 1) == Some(Val::Varint(1))
}

/// One karukan key that starts a composition, mirrored to Mozc, which answers with `cands`.
fn typed(b: &mut Bridge<Fake>, own: &[&str], cands: &[(&str, Option<&str>)]) -> Vec<u8> {
    b.backend().queue.push_back(composing("まる", own));
    let key = key_cmd(3, |k| k.uint(1, 'u' as u64));
    b.handle(&key).unwrap();
    assert_eq!(
        b.shadow_command(&key).as_deref(),
        Some(&key[..]),
        "the key is mirrored"
    );
    b.shadow_reply(&mozc_reply(cands, true)).expect("amended")
}

#[test]
fn appends_mozc_candidates_after_karukan_ones() {
    let mut b = bridge();
    let merged = typed(
        &mut b,
        &["丸", "まる"],
        &[
            ("丸", None),
            ("○", Some("記号")),
            ("●", None),
            ("まる", None),
        ],
    );
    assert_eq!(candidate_values(&merged), ["丸", "まる", "○", "●"]);
    // The description of Mozc's candidate is kept.
    assert!(merged.windows("記号".len()).any(|w| w == "記号".as_bytes()));
}

#[test]
fn shows_mozc_candidates_while_karukan_has_none() {
    let mut b = bridge();
    let merged = typed(&mut b, &[], &[("○", None), ("●", None)]);
    assert_eq!(candidate_values(&merged), ["○", "●"]);
}

#[test]
fn selecting_a_mozc_candidate_commits_it_and_reverts_mozc() {
    let mut b = bridge();
    typed(&mut b, &["三角", "さんかく"], &[("△", None), ("▲", None)]);
    // Index 2 is the first candidate of Mozc.
    let select = cmd_cmd(7, Some(2));
    let out = b.handle(&select).expect("ours");
    let o = output(&out);
    let r = proto::parse(&o.iter().find(|(f, _)| *f == 4).expect("result").1).unwrap();
    assert_eq!(proto::get(&r, 2).unwrap().as_str().unwrap(), "△");
    assert!(
        o.iter().all(|(f, _)| *f != 5 && *f != 14),
        "nothing is left to show"
    );
    assert!(
        !b.backend()
            .calls
            .iter()
            .any(|c| c.starts_with("select_candidate")),
        "karukan is not asked"
    );
    // The composition is over: Mozc is told to revert, once.
    let revert = b.shadow_command(&select).expect("revert");
    assert!(is_revert(&revert));
    assert!(b.shadow_reply(&[]).is_none());
    assert!(b.shadow_command(&select).is_none());
}

#[test]
fn selecting_a_karukan_candidate_still_goes_to_karukan() {
    let mut b = bridge();
    typed(&mut b, &["三角", "さんかく"], &[("△", None)]);
    b.backend()
        .queue
        .push_back(json!({"consumed": true, "_state": "empty", "actions": [
        {"type": "commit", "text": "さんかく"}]}));
    let out = b.handle(&cmd_cmd(7, Some(1))).unwrap();
    assert!(
        b.backend()
            .calls
            .iter()
            .any(|c| c == r#"select_candidate {"page_index":1}"#)
    );
    assert!(output(&out).iter().any(|(f, _)| *f == 4));
}

#[test]
fn commit_by_enter_reverts_mozc() {
    let mut b = bridge();
    typed(&mut b, &["木"], &[("気", None)]);
    b.backend()
        .queue
        .push_back(json!({"consumed": true, "_state": "empty", "actions": [
        {"type": "hide_candidates"}, {"type": "commit", "text": "木"}]}));
    let enter = key_cmd(3, |k| k.uint(3, 5));
    b.handle(&enter).unwrap();
    assert!(is_revert(&b.shadow_command(&enter).expect("revert")));
}

#[test]
fn mozc_without_a_composition_adds_nothing_and_needs_no_revert() {
    let mut b = bridge();
    b.backend().queue.push_back(composing("き", &["木"]));
    let key = key_cmd(3, |k| k.uint(1, 'k' as u64));
    b.handle(&key).unwrap();
    b.shadow_command(&key).unwrap();
    let merged = b.shadow_reply(&mozc_reply(&[("気", None)], false)).unwrap();
    assert_eq!(candidate_values(&merged), ["木"]);
    b.backend()
        .queue
        .push_back(json!({"consumed": true, "_state": "empty", "actions": [
        {"type": "commit", "text": "木"}]}));
    let enter = key_cmd(3, |k| k.uint(3, 5));
    b.handle(&enter).unwrap();
    assert!(b.shadow_command(&enter).is_none());
}

#[test]
fn a_command_for_mozc_clears_the_background_composition_first() {
    let mut b = bridge();
    typed(&mut b, &["木"], &[("気", None)]);
    // A mode switch goes to Mozc: revert first, so that Mozc does not commit its own composition too.
    b.backend()
        .queue
        .push_back(json!({"consumed": true, "_state": "empty", "actions": [
        {"type": "commit", "text": "き"}]}));
    let switch = cmd_cmd(5, None);
    assert!(b.handle(&switch).is_none());
    assert!(is_revert(&b.shadow_revert(&switch).expect("revert")));
    assert!(b.shadow_revert(&switch).is_none());
}

#[test]
fn key_release_and_test_keys_leave_it_alone() {
    let mut b = bridge();
    typed(&mut b, &["木"], &[("気", None)]);
    let release = key_cmd(3, |k| {
        k.uint(1, 'k' as u64);
        k.uint(4, 16);
    });
    assert!(b.shadow_revert(&release).is_none());
    assert!(
        b.shadow_revert(&key_cmd(4, |k| k.uint(1, 'k' as u64)))
            .is_none()
    );
    assert!(
        b.shadow_command(&release).is_none(),
        "a release is not mirrored"
    );
}

#[test]
fn off_adds_nothing() {
    let mut b = bridge();
    b.set_hybrid(false);
    b.backend().queue.push_back(composing("き", &["木"]));
    let key = key_cmd(3, |k| k.uint(1, 'k' as u64));
    b.handle(&key).unwrap();
    assert!(b.shadow_command(&key).is_none());
    assert!(b.shadow_reply(&mozc_reply(&[("気", None)], true)).is_none());
}
