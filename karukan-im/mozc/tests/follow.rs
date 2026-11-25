//! Follow: a flick (12-key) composition is composed by Mozc and converted by karukan too.

use karukan_mozc::bridge::{Backend, Bridge};
use karukan_mozc::proto::{self, Val, W};
use serde_json::{Value, json};

/// The engine: `set_reading` answers with the candidates of `cands_for`.
struct Fake {
    calls: Vec<String>,
    reading: String,
}

fn cands_for(reading: &str) -> Vec<&'static str> {
    match reading {
        "き" => vec!["木", "気"],
        "きょう" => vec!["今日", "京"],
        _ => vec![],
    }
}

impl Backend for Fake {
    fn call(&mut self, method: &str, params: Value) -> Option<Value> {
        self.calls.push(format!("{method} {params}"));
        match method {
            "status" => Some(json!({"initialized": true, "model_name": "m",
                "state": if self.reading.is_empty() { "empty" } else { "composing" }})),
            "set_reading" => {
                self.reading = params["text"].as_str().unwrap().to_string();
                let c = cands_for(&self.reading);
                let mut actions = vec![json!({"type": "update_preedit", "text": self.reading})];
                if !c.is_empty() {
                    actions.push(json!({"type": "show_candidates",
                        "candidates": c.iter().map(|t| json!({"text": t})).collect::<Vec<_>>(),
                        "cursor": 0, "page": 0, "total_pages": 1}));
                }
                Some(json!({"consumed": true, "actions": actions}))
            }
            "select_candidate" => {
                let i = params["page_index"].as_u64().unwrap() as usize;
                let text = cands_for(&self.reading)[i];
                self.reading.clear();
                Some(json!({"consumed": true, "actions": [{"type": "commit", "text": text}]}))
            }
            _ => Some(json!({})),
        }
    }
}

const TOGGLE_FLICK: i64 = 16;

fn set_table(b: &mut Bridge<Fake>, table: i64) {
    let mut w = W::new();
    w.message(1, |i| {
        i.uint(1, 17);
        i.message(9, |r| r.int(4, table));
    });
    assert!(b.handle(&w.0).is_none());
}

fn bridge() -> Bridge<Fake> {
    let mut b = Bridge::new(Fake {
        calls: vec![],
        reading: String::new(),
    });
    // Gboard tells Mozc which keyboard is up.
    set_table(&mut b, TOGGLE_FLICK);
    b
}

/// A flick key: Gboard sends the kana itself.
fn kana_key(kana: &str) -> Vec<u8> {
    let mut w = W::new();
    w.message(1, |i| {
        i.uint(1, 3);
        i.uint(2, 5);
        i.message(3, |k| {
            k.uint(1, kana.chars().next().unwrap() as u64);
            k.string(5, kana);
        });
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

/// Mozc's reply: a preedit of `segs` (text, highlighted) and candidates (id, value).
fn mozc_reply(segs: &[(&str, bool)], cands: &[(i64, &str)], focused: bool) -> Vec<u8> {
    let mut o = W::new();
    o.uint(1, 5);
    o.bool(3, true);
    if !segs.is_empty() {
        o.message(5, |p| {
            p.uint(1, 0);
            for (t, h) in segs {
                p.group(2, |g| {
                    g.uint(3, if *h { 2 } else { 1 });
                    g.string(4, t);
                    g.uint(5, t.chars().count() as u64);
                });
            }
            if segs.iter().any(|s| s.1) {
                p.uint(3, 0);
            }
        });
    }
    if !cands.is_empty() {
        o.message(14, |l| {
            if focused {
                l.uint(1, 0);
            }
            for (i, (id, t)) in cands.iter().enumerate() {
                l.message(2, |c| {
                    c.int(1, *id);
                    c.uint(2, i as u64);
                    c.string(4, t);
                });
            }
            l.uint(3, if focused { 0 } else { 1 });
        });
    }
    o.message(13, |s| {
        s.bool(1, true);
        s.uint(2, 1);
    });
    let mut w = W::new();
    w.bytes(1, &[0x08, 0x03]);
    w.bytes(2, &o.0);
    w.0
}

fn out_fields(cmd: &[u8]) -> Vec<(u32, Vec<u8>)> {
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

/// (id, value) of `all_candidate_words`.
fn candidates(cmd: &[u8]) -> Vec<(i64, String)> {
    let o = out_fields(cmd);
    let list = proto::parse(&o.iter().find(|(f, _)| *f == 14).expect("candidates").1).unwrap();
    list.iter()
        .filter(|(f, _)| *f == 2)
        .map(|(_, v)| {
            let c = proto::parse(v.as_bytes().unwrap()).unwrap();
            (
                proto::get(&c, 1).unwrap().as_u64().unwrap() as i32 as i64,
                proto::get(&c, 4).unwrap().as_str().unwrap(),
            )
        })
        .collect()
}

fn texts(cmd: &[u8]) -> Vec<String> {
    candidates(cmd).into_iter().map(|c| c.1).collect()
}

/// One flick key that Mozc composes into `preedit`, offering `cands`: the reply as amended.
fn flick(
    b: &mut Bridge<Fake>,
    kana: &str,
    preedit: &str,
    cands: &[(i64, &str)],
) -> Option<Vec<u8>> {
    let key = kana_key(kana);
    assert!(b.handle(&key).is_none(), "Mozc composes the kana");
    assert!(b.shadow_revert(&key).is_none());
    b.observe(&key, &mozc_reply(&[(preedit, false)], cands, false))
}

#[test]
fn karukan_candidates_come_first_and_mozc_keeps_its_own() {
    let mut b = bridge();
    let r = flick(&mut b, "き", "き", &[(0, "気"), (1, "黄"), (-1, "キ")]).expect("amended");
    assert_eq!(texts(&r), ["木", "気", "黄", "キ"]);
    let c = candidates(&r);
    // Mozc's ids stay, so a tap on them is still Mozc's.
    assert_eq!(c[2].0, 1);
    assert_eq!(c[3].0, -1);
    assert!(c[0].0 > 0x1000_0000 && c[1].0 > 0x1000_0000);
    // Mozc's preedit and status are kept.
    let o = out_fields(&r);
    assert!(o.iter().any(|(f, _)| *f == 5));
    assert!(o.iter().any(|(f, _)| *f == 13));
    // The context of the field reaches the engine.
    assert!(
        b.backend()
            .calls
            .iter()
            .any(|c| c.starts_with("set_surrounding_text") && c.contains("前の文"))
    );
}

#[test]
fn a_toggle_replaces_the_reading() {
    let mut b = bridge();
    flick(&mut b, "か", "か", &[]);
    let r = flick(&mut b, "か", "き", &[]).expect("amended");
    assert_eq!(texts(&r), ["木", "気"]);
    let sets: Vec<&String> = b
        .backend()
        .calls
        .iter()
        .filter(|c| c.starts_with("set_reading"))
        .collect();
    assert_eq!(sets.len(), 2);
    assert!(sets[1].contains("\"き\""));
    // The context is set once, for the first key.
    let ctx = b
        .backend()
        .calls
        .iter()
        .filter(|c| c.starts_with("set_surrounding_text"))
        .count();
    assert_eq!(ctx, 1);
}

#[test]
fn a_reading_with_no_candidates_leaves_mozc_alone() {
    let mut b = bridge();
    assert!(flick(&mut b, "ぬ", "ぬ", &[(0, "奴")]).is_none());
}

#[test]
fn tapping_a_karukan_candidate_commits_it_and_reverts_mozc() {
    let mut b = bridge();
    let r = flick(&mut b, "う", "きょう", &[(0, "強")]).unwrap();
    let id = candidates(&r)[0].0;
    let tap = cmd_cmd(7, Some(id));
    let out = b.handle(&tap).expect("ours");
    let o = out_fields(&out);
    let res = proto::parse(&o.iter().find(|(f, _)| *f == 4).expect("result").1).unwrap();
    assert_eq!(proto::get(&res, 2).unwrap().as_str().unwrap(), "今日");
    assert!(o.iter().all(|(f, _)| *f != 5 && *f != 14));
    assert!(
        b.backend()
            .calls
            .iter()
            .any(|c| c == r#"select_candidate {"page_index":0}"#)
    );
    assert!(
        b.backend()
            .calls
            .iter()
            .any(|c| c.starts_with("save_learning"))
    );
    // Mozc still holds きょう: the hybrid exchange reverts it, once.
    let revert = b.shadow_command(&tap).expect("revert");
    let top = proto::parse(&revert).unwrap();
    let i = proto::parse(proto::get(&top, 1).unwrap().as_bytes().unwrap()).unwrap();
    let c = proto::parse(proto::get(&i, 4).unwrap().as_bytes().unwrap()).unwrap();
    assert_eq!(proto::get(&c, 1), Some(Val::Varint(1)));
    assert!(b.shadow_reply(&[]).is_none());
    assert!(b.shadow_command(&tap).is_none());
    // The next key starts a new composition.
    let r = flick(&mut b, "き", "き", &[]).unwrap();
    assert_eq!(texts(&r), ["木", "気"]);
}

#[test]
fn tapping_a_mozc_candidate_is_mozcs() {
    let mut b = bridge();
    flick(&mut b, "き", "き", &[(0, "黄")]).unwrap();
    let tap = cmd_cmd(7, Some(0));
    assert!(b.handle(&tap).is_none());
    // Mozc committed: nothing composing, the engine lets go too.
    assert!(b.observe(&tap, &mozc_reply(&[], &[], false)).is_none());
    assert_eq!(
        b.backend().calls.last().map(String::as_str),
        Some(r#"set_reading {"text":""}"#)
    );
}

#[test]
fn mozc_converting_is_left_alone() {
    let mut b = bridge();
    flick(&mut b, "き", "き", &[]).unwrap();
    // The conversion key.
    let mut w = W::new();
    w.message(1, |i| {
        i.uint(1, 3);
        i.uint(2, 5);
        i.message(3, |k| k.uint(3, 4));
    });
    let space = w.0;
    assert!(b.handle(&space).is_none());
    let conv = mozc_reply(&[("木", true)], &[(0, "木"), (1, "気")], true);
    assert!(b.observe(&space, &conv).is_none());
}

#[test]
fn qwerty_romaji_is_not_followed() {
    let mut b = bridge();
    set_table(&mut b, 0);
    // A composition Mozc owns on QWERTY (one started while the model was loading, say).
    let key = kana_key("き");
    b.handle(&key);
    assert!(
        b.observe(&key, &mozc_reply(&[("き", false)], &[], false))
            .is_none()
    );
    assert!(
        !b.backend()
            .calls
            .iter()
            .any(|c| c.starts_with("set_reading"))
    );
}

#[test]
fn off_without_hybrid() {
    let mut b = bridge();
    b.set_hybrid(false);
    assert!(flick(&mut b, "き", "き", &[]).is_none());
}

#[test]
fn mozc_committing_ends_the_follow() {
    let mut b = bridge();
    flick(&mut b, "き", "き", &[]).unwrap();
    let enter = cmd_cmd(2, None);
    assert!(b.handle(&enter).is_none());
    let mut o = W::new();
    o.message(4, |m| {
        m.uint(1, 1);
        m.string(2, "き");
    });
    let mut w = W::new();
    w.bytes(1, &[0x08, 0x05]);
    w.bytes(2, &o.0);
    assert!(b.observe(&enter, &w.0).is_none());
    assert!(
        b.backend()
            .calls
            .iter()
            .any(|c| c == r#"set_reading {"text":""}"#)
    );
}

#[test]
fn a_reply_to_a_test_key_or_a_request_does_not_end_it() {
    let mut b = bridge();
    flick(&mut b, "き", "き", &[]).unwrap();
    // A test key: Mozc answers without the preedit.
    let mut w = W::new();
    w.message(1, |i| {
        i.uint(1, 4);
        i.uint(2, 5);
        i.message(3, |k| k.string(5, "く"));
    });
    assert!(b.handle(&w.0).is_none());
    assert!(b.observe(&w.0, &mozc_reply(&[], &[], false)).is_none());
    assert!(
        !b.backend()
            .calls
            .iter()
            .any(|c| c == r#"set_reading {"text":""}"#)
    );
    // The next key is still the same composition: its context is not taken.
    let r = flick(&mut b, "う", "きょう", &[]).unwrap();
    assert_eq!(texts(&r), ["今日", "京"]);
    let ctx = b
        .backend()
        .calls
        .iter()
        .filter(|c| c.starts_with("set_surrounding_text"))
        .count();
    assert_eq!(ctx, 1);
}

#[test]
fn pending_romaji_is_not_a_reading() {
    let mut b = bridge();
    assert!(flick(&mut b, "k", "k", &[]).is_none());
    assert!(flick(&mut b, "き", "きk", &[]).is_none());
}
