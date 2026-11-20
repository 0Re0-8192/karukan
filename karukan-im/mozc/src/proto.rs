//! Minimal protobuf wire-format reader/writer (no schema, no dependency).
//!
//! Gboard's Mozc messages are proto2 lite and contain groups (`Preedit.Segment`,
//! `CandidateWindow.Candidate`), which are wire types 3/4, so the codec handles them.

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Val<'a> {
    Varint(u64),
    Fixed64(u64),
    Len(&'a [u8]),
    Fixed32(u32),
    /// Contents of a group (between start and end tags).
    Group(&'a [u8]),
}

impl<'a> Val<'a> {
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Val::Varint(v) => Some(*v),
            _ => None,
        }
    }
    pub fn as_bytes(&self) -> Option<&'a [u8]> {
        match self {
            Val::Len(b) | Val::Group(b) => Some(b),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<String> {
        self.as_bytes().map(|b| String::from_utf8_lossy(b).into_owned())
    }
}

fn read_varint(buf: &[u8], pos: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    let mut shift = 0;
    loop {
        let b = *buf.get(*pos)?;
        *pos += 1;
        if shift >= 64 {
            return None;
        }
        v |= ((b & 0x7f) as u64) << shift;
        if b & 0x80 == 0 {
            return Some(v);
        }
        shift += 7;
    }
}

/// Parse all top-level fields. `None` on malformed input.
pub fn parse(buf: &[u8]) -> Option<Vec<(u32, Val<'_>)>> {
    let mut pos = 0;
    let out = parse_until(buf, &mut pos, None)?;
    Some(out)
}

fn parse_until<'a>(
    buf: &'a [u8],
    pos: &mut usize,
    end_group: Option<u32>,
) -> Option<Vec<(u32, Val<'a>)>> {
    let mut out = Vec::new();
    while *pos < buf.len() {
        let tag = read_varint(buf, pos)?;
        let field = (tag >> 3) as u32;
        match (tag & 7) as u8 {
            0 => out.push((field, Val::Varint(read_varint(buf, pos)?))),
            1 => {
                let b = buf.get(*pos..*pos + 8)?;
                *pos += 8;
                out.push((field, Val::Fixed64(u64::from_le_bytes(b.try_into().ok()?))));
            }
            2 => {
                let n = read_varint(buf, pos)? as usize;
                let b = buf.get(*pos..pos.checked_add(n)?)?;
                *pos += n;
                out.push((field, Val::Len(b)));
            }
            3 => {
                let start = *pos;
                // Find the matching end tag by parsing the body.
                let mut p = *pos;
                parse_until(buf, &mut p, Some(field))?;
                // `p` is just past the end tag; the body ends before the tag.
                let end_tag_len = varint_len(((field as u64) << 3) | 4);
                let body_end = p.checked_sub(end_tag_len)?;
                out.push((field, Val::Group(buf.get(start..body_end)?)));
                *pos = p;
            }
            4 => {
                return if end_group == Some(field) { Some(out) } else { None };
            }
            5 => {
                let b = buf.get(*pos..*pos + 4)?;
                *pos += 4;
                out.push((field, Val::Fixed32(u32::from_le_bytes(b.try_into().ok()?))));
            }
            _ => return None,
        }
    }
    if end_group.is_some() { None } else { Some(out) }
}

fn varint_len(mut v: u64) -> usize {
    let mut n = 1;
    while v >= 0x80 {
        v >>= 7;
        n += 1;
    }
    n
}

/// First value of `field` (last wins for scalars in protobuf, but our fields are not repeated).
pub fn get<'a>(fields: &[(u32, Val<'a>)], field: u32) -> Option<Val<'a>> {
    fields.iter().rev().find(|(f, _)| *f == field).map(|(_, v)| *v)
}

/// Repeated varint field (accepts both unpacked and packed encodings).
pub fn get_repeated_u64(fields: &[(u32, Val<'_>)], field: u32) -> Vec<u64> {
    let mut out = Vec::new();
    for (f, v) in fields {
        if *f != field {
            continue;
        }
        match v {
            Val::Varint(x) => out.push(*x),
            Val::Len(b) => {
                let mut p = 0;
                while p < b.len() {
                    match read_varint(b, &mut p) {
                        Some(x) => out.push(x),
                        None => break,
                    }
                }
            }
            _ => {}
        }
    }
    out
}

#[derive(Default, Debug, Clone)]
pub struct W(pub Vec<u8>);

impl W {
    pub fn new() -> Self {
        Self(Vec::new())
    }
    pub fn varint(&mut self, mut v: u64) {
        while v >= 0x80 {
            self.0.push((v as u8) | 0x80);
            v >>= 7;
        }
        self.0.push(v as u8);
    }
    fn tag(&mut self, field: u32, wt: u8) {
        self.varint(((field as u64) << 3) | wt as u64);
    }
    pub fn uint(&mut self, field: u32, v: u64) {
        self.tag(field, 0);
        self.varint(v);
    }
    /// int32/enum: negative values take ten bytes.
    pub fn int(&mut self, field: u32, v: i64) {
        self.tag(field, 0);
        self.varint(v as u64);
    }
    pub fn bool(&mut self, field: u32, v: bool) {
        self.uint(field, v as u64);
    }
    pub fn bytes(&mut self, field: u32, b: &[u8]) {
        self.tag(field, 2);
        self.varint(b.len() as u64);
        self.0.extend_from_slice(b);
    }
    pub fn string(&mut self, field: u32, s: &str) {
        self.bytes(field, s.as_bytes());
    }
    pub fn message(&mut self, field: u32, f: impl FnOnce(&mut W)) {
        let mut inner = W::new();
        f(&mut inner);
        self.bytes(field, &inner.0);
    }
    pub fn group(&mut self, field: u32, f: impl FnOnce(&mut W)) {
        self.tag(field, 3);
        f(self);
        self.tag(field, 4);
    }
    pub fn raw(&mut self, b: &[u8]) {
        self.0.extend_from_slice(b);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_with_group() {
        let mut w = W::new();
        w.uint(1, 300);
        w.group(2, |g| {
            g.uint(3, 1);
            g.string(4, "あ");
        });
        w.string(5, "x");
        let f = parse(&w.0).unwrap();
        assert_eq!(f.len(), 3);
        assert_eq!(get(&f, 1), Some(Val::Varint(300)));
        let Val::Group(body) = get(&f, 2).unwrap() else { panic!() };
        let g = parse(body).unwrap();
        assert_eq!(get(&g, 4).unwrap().as_str().unwrap(), "あ");
        assert_eq!(get(&f, 5).unwrap().as_str().unwrap(), "x");
    }

    #[test]
    fn malformed_is_none() {
        assert!(parse(&[0x0a, 0x05, 1]).is_none());
        assert!(parse(&[0x13]).is_none()); // unterminated group
    }
}
