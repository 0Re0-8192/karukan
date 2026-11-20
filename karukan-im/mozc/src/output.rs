//! Encoding of the Mozc `Command` (input echo + `Output`) built from engine state.

use crate::proto::{self, Val, W};

pub const ANN_UNDERLINE: u64 = 1;
pub const ANN_HIGHLIGHT: u64 = 2;

pub const CAT_CONVERSION: u64 = 0;
pub const CAT_SUGGESTION: u64 = 2;

#[derive(Debug, Clone, PartialEq)]
pub struct PreeditView {
    pub text: String,
    /// Caret in Unicode scalar values.
    pub caret: usize,
    /// `(start, end, highlighted)` in Unicode scalar values.
    pub attrs: Vec<(usize, usize, bool)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CandView {
    pub items: Vec<(String, Option<String>)>,
    pub cursor: usize,
    pub page: usize,
    pub total_pages: usize,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Frame {
    pub preedit: Option<PreeditView>,
    pub cands: Option<CandView>,
}

impl Frame {
    pub fn converting(&self) -> bool {
        self.preedit
            .as_ref()
            .is_some_and(|p| p.attrs.iter().any(|a| a.2))
    }
}

fn chars_slice(s: &str, a: usize, b: usize) -> String {
    s.chars().skip(a).take(b.saturating_sub(a)).collect()
}

fn write_preedit(w: &mut W, p: &PreeditView) {
    let n = p.text.chars().count();
    // Cut the text into segments along the attribute ranges; gaps are plain underline.
    let mut attrs: Vec<(usize, usize, bool)> = p
        .attrs
        .iter()
        .map(|&(s, e, h)| (s.min(n), e.min(n), h))
        .filter(|a| a.0 < a.1)
        .collect();
    attrs.sort();
    let mut segs: Vec<(usize, usize, bool)> = Vec::new();
    let mut at = 0;
    for (s, e, h) in attrs {
        if s < at {
            continue;
        }
        if s > at {
            segs.push((at, s, false));
        }
        segs.push((s, e, h));
        at = e;
    }
    if at < n {
        segs.push((at, n, false));
    }
    w.uint(1, p.caret.min(n) as u64);
    let mut highlighted = None;
    for (s, e, h) in &segs {
        if *h && highlighted.is_none() {
            highlighted = Some(*s);
        }
        w.group(2, |g| {
            g.uint(3, if *h { ANN_HIGHLIGHT } else { ANN_UNDERLINE });
            let v = chars_slice(&p.text, *s, *e);
            g.string(4, &v);
            g.uint(5, (*e - *s) as u64);
        });
    }
    if let Some(h) = highlighted {
        w.uint(3, h as u64);
    }
}

fn write_annotation(w: &mut W, field: u32, desc: &Option<String>) {
    if let Some(d) = desc.as_deref().filter(|d| !d.is_empty()) {
        w.message(field, |a| a.string(3, d));
    }
}

fn write_candidates(out: &mut W, c: &CandView, converting: bool) {
    let cat = if converting { CAT_CONVERSION } else { CAT_SUGGESTION };
    // all_candidate_words (CandidateList)
    out.message(14, |l| {
        if converting {
            l.uint(1, c.cursor as u64);
        }
        for (i, (text, desc)) in c.items.iter().enumerate() {
            l.message(2, |w| {
                w.int(1, i as i64); // id
                w.uint(2, i as u64); // index
                w.string(4, text);
                write_annotation(w, 5, desc);
            });
        }
        l.uint(3, cat);
    });
    // candidate_window
    out.message(6, |cw| {
        if converting {
            cw.uint(1, c.cursor as u64);
        }
        cw.uint(2, c.items.len() as u64);
        for (i, (text, desc)) in c.items.iter().enumerate() {
            cw.group(3, |g| {
                g.uint(4, i as u64);
                g.string(5, text);
                g.int(9, i as i64);
                write_annotation(g, 7, desc);
            });
        }
        cw.uint(6, 0); // position
        cw.uint(11, cat);
        cw.uint(18, 9); // page_size
    });
}

/// Serialize `Command{ input = <echo>, output = Output{...} }`.
pub fn encode_command(
    input_raw: &[u8],
    session_id: Option<u64>,
    consumed: bool,
    result: Option<&str>,
    frame: &Frame,
) -> Vec<u8> {
    let mut o = W::new();
    if let Some(id) = session_id {
        o.uint(1, id);
    }
    o.bool(3, consumed);
    if let Some(r) = result.filter(|r| !r.is_empty()) {
        o.message(4, |m| {
            m.uint(1, 1); // STRING
            m.string(2, r);
        });
    }
    if let Some(p) = frame.preedit.as_ref().filter(|p| !p.text.is_empty()) {
        o.message(5, |m| write_preedit(m, p));
    }
    if let Some(c) = frame.cands.as_ref().filter(|c| !c.items.is_empty()) {
        write_candidates(&mut o, c, frame.converting());
    }
    let mut w = W::new();
    w.bytes(1, input_raw);
    w.bytes(2, &o.0);
    w.0
}

fn write_fields(w: &mut W, fields: &[(u32, Val<'_>)]) {
    for (f, v) in fields {
        match v {
            Val::Varint(x) => w.uint(*f, *x),
            Val::Len(b) => w.bytes(*f, b),
            Val::Group(b) => {
                w.group(*f, |g| g.raw(b));
            }
            Val::Fixed64(x) => {
                w.varint(((*f as u64) << 3) | 1);
                w.raw(&x.to_le_bytes());
            }
            Val::Fixed32(x) => {
                w.varint(((*f as u64) << 3) | 5);
                w.raw(&x.to_le_bytes());
            }
        }
    }
}

/// Prepend `text` to the `Output.result` of a Mozc-produced `Command` (committing a composition
/// that this engine owned just before Mozc handled a mode switch).
pub fn inject_result(cmd: &[u8], text: &str) -> Option<Vec<u8>> {
    let top = proto::parse(cmd)?;
    let out_raw = match proto::get(&top, 2) {
        Some(Val::Len(b)) => b,
        _ => &[][..],
    };
    let mut of = proto::parse(out_raw)?;
    let old = of
        .iter()
        .position(|(f, _)| *f == 4)
        .and_then(|i| match of.remove(i).1 {
            Val::Len(b) => proto::parse(b),
            _ => None,
        });
    let old_value = old
        .as_ref()
        .and_then(|f| proto::get(f, 2))
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let mut o = W::new();
    write_fields(&mut o, &of);
    o.message(4, |m| {
        m.uint(1, 1);
        m.string(2, &format!("{text}{old_value}"));
    });
    let mut w = W::new();
    for (f, v) in &top {
        if *f == 2 {
            continue;
        }
        write_fields(&mut w, &[(*f, *v)]);
    }
    w.bytes(2, &o.0);
    Some(w.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out_of(cmd: &[u8]) -> Vec<u8> {
        let top = proto::parse(cmd).unwrap();
        proto::get(&top, 2).unwrap().as_bytes().unwrap().to_vec()
    }

    #[test]
    fn preedit_and_candidates_roundtrip() {
        let frame = Frame {
            preedit: Some(PreeditView {
                text: "今日は".into(),
                caret: 3,
                attrs: vec![(0, 2, true)],
            }),
            cands: Some(CandView {
                items: vec![("今日".into(), None), ("きょう".into(), Some("ひらがな".into()))],
                cursor: 0,
                page: 0,
                total_pages: 1,
            }),
        };
        let cmd = encode_command(&[0x08, 0x03], Some(9), true, None, &frame);
        let ob = out_of(&cmd);
        let o = proto::parse(&ob).unwrap();
        assert_eq!(proto::get(&o, 1), Some(Val::Varint(9)));
        assert_eq!(proto::get(&o, 3), Some(Val::Varint(1)));
        let pre = proto::parse(proto::get(&o, 5).unwrap().as_bytes().unwrap()).unwrap();
        assert_eq!(proto::get(&pre, 1), Some(Val::Varint(3)));
        let segs: Vec<_> = pre.iter().filter(|(f, _)| *f == 2).collect();
        assert_eq!(segs.len(), 2);
        let Val::Group(g0) = segs[0].1 else { panic!() };
        let g0 = proto::parse(g0).unwrap();
        assert_eq!(proto::get(&g0, 3), Some(Val::Varint(ANN_HIGHLIGHT)));
        assert_eq!(proto::get(&g0, 4).unwrap().as_str().unwrap(), "今日");
        assert_eq!(proto::get(&g0, 5), Some(Val::Varint(2)));
        assert_eq!(proto::get(&pre, 3), Some(Val::Varint(0)));
        let all = proto::parse(proto::get(&o, 14).unwrap().as_bytes().unwrap()).unwrap();
        assert_eq!(all.iter().filter(|(f, _)| *f == 2).count(), 2);
        assert!(proto::get(&o, 6).is_some());
    }

    #[test]
    fn commit_result_only() {
        let cmd = encode_command(&[], Some(1), true, Some("今日"), &Frame::default());
        let ob = out_of(&cmd);
        let o = proto::parse(&ob).unwrap();
        let r = proto::parse(proto::get(&o, 4).unwrap().as_bytes().unwrap()).unwrap();
        assert_eq!(proto::get(&r, 2).unwrap().as_str().unwrap(), "今日");
        assert!(proto::get(&o, 5).is_none());
    }

    #[test]
    fn inject_keeps_other_fields_and_prepends() {
        let mut mo = W::new();
        mo.uint(1, 5);
        mo.bool(3, true);
        mo.message(13, |s| {
            s.bool(1, true);
            s.uint(2, 3);
        });
        let mut w = W::new();
        w.bytes(1, &[0x08, 0x05]);
        w.bytes(2, &mo.0);
        let merged = inject_result(&w.0, "あ").unwrap();
        let ob = out_of(&merged);
        let o = proto::parse(&ob).unwrap();
        assert_eq!(proto::get(&o, 1), Some(Val::Varint(5)));
        assert!(proto::get(&o, 13).is_some());
        let r = proto::parse(proto::get(&o, 4).unwrap().as_bytes().unwrap()).unwrap();
        assert_eq!(proto::get(&r, 2).unwrap().as_str().unwrap(), "あ");
    }
}
