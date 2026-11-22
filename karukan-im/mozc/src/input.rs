//! Decoding of the Mozc `Command.input` we care about.

use crate::proto::{self, Val};

// Input.CommandType
pub const CREATE_SESSION: u32 = 1;
pub const DELETE_SESSION: u32 = 2;
pub const SEND_KEY: u32 = 3;
pub const TEST_SEND_KEY: u32 = 4;
pub const SEND_COMMAND: u32 = 5;
pub const SET_REQUEST: u32 = 17;

// SessionCommand.CommandType
pub const SC_REVERT: u32 = 1;
pub const SC_SUBMIT: u32 = 2;
pub const SC_SELECT_CANDIDATE: u32 = 3;
pub const SC_HIGHLIGHT_CANDIDATE: u32 = 4;
pub const SC_SWITCH_COMPOSITION_MODE: u32 = 5;
pub const SC_SUBMIT_CANDIDATE: u32 = 7;
pub const SC_RESET_CONTEXT: u32 = 10;
pub const SC_MOVE_CURSOR: u32 = 11;
pub const SC_COMMIT_RAW_TEXT: u32 = 19;
pub const SC_TURN_ON_IME: u32 = 22;
pub const SC_TURN_OFF_IME: u32 = 23;

// KeyEvent.SpecialKey
pub const SK_SPACE: u32 = 4;
pub const SK_ENTER: u32 = 5;
pub const SK_LEFT: u32 = 6;
pub const SK_RIGHT: u32 = 7;
pub const SK_UP: u32 = 8;
pub const SK_DOWN: u32 = 9;
pub const SK_ESCAPE: u32 = 10;
pub const SK_DEL: u32 = 11;
pub const SK_BACKSPACE: u32 = 12;
pub const SK_HENKAN: u32 = 13;
pub const SK_MUHENKAN: u32 = 14;
pub const SK_HOME: u32 = 16;
pub const SK_END: u32 = 17;
pub const SK_TAB: u32 = 18;
pub const SK_F1: u32 = 19;
pub const SK_F6: u32 = 24;
pub const SK_F10: u32 = 28;
pub const SK_PAGE_UP: u32 = 31;
pub const SK_PAGE_DOWN: u32 = 32;
pub const SK_VIRTUAL_LEFT: u32 = 72;
pub const SK_VIRTUAL_RIGHT: u32 = 73;
pub const SK_VIRTUAL_ENTER: u32 = 74;
pub const SK_VIRTUAL_UP: u32 = 75;
pub const SK_VIRTUAL_DOWN: u32 = 76;

// CompositionMode
pub const MODE_HIRAGANA: u32 = 1;

// KeyEvent.ModifierKey bits
const M_CTRL: u64 = 1 | 32 | 256;
const M_ALT: u64 = 2 | 64 | 512;
const M_SHIFT: u64 = 4 | 128 | 1024;
const M_KEY_UP: u64 = 16;

#[derive(Debug, Default, Clone)]
pub struct Key {
    pub key_code: Option<u32>,
    pub special: Option<u32>,
    pub key_string: Option<String>,
    pub modifier_bits: u64,
    pub activated: Option<bool>,
}

impl Key {
    pub fn shift(&self) -> bool {
        self.modifier_bits & M_SHIFT != 0
    }
    pub fn ctrl(&self) -> bool {
        self.modifier_bits & M_CTRL != 0
    }
    pub fn alt(&self) -> bool {
        self.modifier_bits & M_ALT != 0
    }
    pub fn is_release(&self) -> bool {
        self.modifier_bits & M_KEY_UP != 0
    }
}

#[derive(Debug, Default, Clone)]
pub struct SessionCommand {
    pub ty: u32,
    pub id: Option<i32>,
    pub composition_mode: Option<u32>,
    pub text: Option<String>,
    pub cursor_position: Option<u32>,
}

#[derive(Debug, Default, Clone)]
pub struct Input {
    pub ty: u32,
    pub id: Option<u64>,
    pub key: Option<Key>,
    pub command: Option<SessionCommand>,
    pub preceding: Option<String>,
    pub following: Option<String>,
    /// `Request.special_romanji_table` of a SET_REQUEST (0 = default QWERTY romaji).
    pub request_table: Option<u32>,
}

fn decode_key(b: &[u8]) -> Option<Key> {
    let f = proto::parse(b)?;
    let mut mods = 0u64;
    for m in proto::get_repeated_u64(&f, 4) {
        mods |= m;
    }
    if let Some(m) = proto::get(&f, 2).and_then(|v| v.as_u64()) {
        mods |= m; // legacy `modifiers` bitfield
    }
    Some(Key {
        key_code: proto::get(&f, 1).and_then(|v| v.as_u64()).map(|v| v as u32),
        special: proto::get(&f, 3).and_then(|v| v.as_u64()).map(|v| v as u32),
        key_string: proto::get(&f, 5).and_then(|v| v.as_str()),
        modifier_bits: mods,
        activated: proto::get(&f, 9).and_then(|v| v.as_u64()).map(|v| v != 0),
    })
}

fn decode_command(b: &[u8]) -> Option<SessionCommand> {
    let f = proto::parse(b)?;
    Some(SessionCommand {
        ty: proto::get(&f, 1).and_then(|v| v.as_u64()).unwrap_or(0) as u32,
        id: proto::get(&f, 2)
            .and_then(|v| v.as_u64())
            .map(|v| v as i64 as i32),
        composition_mode: proto::get(&f, 3).and_then(|v| v.as_u64()).map(|v| v as u32),
        text: proto::get(&f, 4).and_then(|v| v.as_str()),
        cursor_position: proto::get(&f, 5).and_then(|v| v.as_u64()).map(|v| v as u32),
    })
}

/// Returns the raw bytes of `Command.input` and its decoded form.
pub fn decode_command_input(bytes: &[u8]) -> Option<(&[u8], Input)> {
    let top = proto::parse(bytes)?;
    let raw = match proto::get(&top, 1)? {
        Val::Len(b) => b,
        _ => return None,
    };
    let f = proto::parse(raw)?;
    let mut input = Input {
        ty: proto::get(&f, 1).and_then(|v| v.as_u64())? as u32,
        id: proto::get(&f, 2).and_then(|v| v.as_u64()),
        ..Default::default()
    };
    input.key = proto::get(&f, 3)
        .and_then(|v| v.as_bytes())
        .and_then(decode_key);
    input.command = proto::get(&f, 4)
        .and_then(|v| v.as_bytes())
        .and_then(decode_command);
    if let Some(ctx) = proto::get(&f, 6)
        .and_then(|v| v.as_bytes())
        .and_then(proto::parse)
    {
        input.preceding = proto::get(&ctx, 1).and_then(|v| v.as_str());
        input.following = proto::get(&ctx, 2).and_then(|v| v.as_str());
    }
    if let Some(req) = proto::get(&f, 9)
        .and_then(|v| v.as_bytes())
        .and_then(proto::parse)
    {
        input.request_table =
            Some(proto::get(&req, 4).and_then(|v| v.as_u64()).unwrap_or(0) as u32);
    }
    Some((raw, input))
}

/// XKB keysym for a Mozc key event, or `None` when the engine has no equivalent.
pub fn keysym(k: &Key) -> Option<u32> {
    if let Some(sp) = k.special.filter(|s| *s != 0) {
        return Some(match sp {
            SK_SPACE => 0x20,
            SK_ENTER | SK_VIRTUAL_ENTER => 0xff0d,
            SK_LEFT | SK_VIRTUAL_LEFT => 0xff51,
            SK_UP | SK_VIRTUAL_UP => 0xff52,
            SK_RIGHT | SK_VIRTUAL_RIGHT => 0xff53,
            SK_DOWN | SK_VIRTUAL_DOWN => 0xff54,
            SK_ESCAPE => 0xff1b,
            SK_DEL => 0xffff,
            SK_BACKSPACE => 0xff08,
            SK_HENKAN => 0xff23,
            SK_MUHENKAN => 0xff22,
            SK_HOME => 0xff50,
            SK_END => 0xff57,
            SK_TAB => 0xff09,
            SK_PAGE_UP => 0xff55,
            SK_PAGE_DOWN => 0xff56,
            n if (SK_F6..=SK_F10).contains(&n) => 0xffbe + (n - SK_F1), // F1 = 0xffbe
            _ => return None,
        });
    }
    let c = k.key_code?;
    if c == 0 {
        return None;
    }
    Some(if c < 0x100 { c } else { 0x0100_0000 + c })
}

/// `SpecialRomanjiTable` values that are plain QWERTY romaji: DEFAULT_TABLE (0) and Gboard's
/// QWERTY_MOBILE_TO_HIRAGANA (20). Flick / 12-key tables stay with Mozc.
pub fn is_qwerty_romaji_table(t: u32) -> bool {
    t == 0 || t == 20
}

/// A plain printable ASCII key that starts a romaji composition.
pub fn is_romaji_start(k: &Key) -> bool {
    if k.special.is_some_and(|s| s != 0) {
        return false;
    }
    if k.ctrl() || k.alt() {
        return false;
    }
    if k.key_string
        .as_deref()
        .is_some_and(|s| !s.is_empty() && !s.is_ascii())
    {
        return false; // kana from a toggle/flick keyboard
    }
    matches!(k.key_code, Some(c) if (0x21..0x7f).contains(&c))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::W;

    fn command(key: impl FnOnce(&mut W)) -> Vec<u8> {
        let mut w = W::new();
        w.message(1, |i| {
            i.uint(1, SEND_KEY as u64);
            i.uint(2, 77);
            i.message(3, key);
        });
        w.0
    }

    #[test]
    fn decodes_letter_key() {
        let b = command(|k| k.uint(1, 'k' as u64));
        let (_, i) = decode_command_input(&b).unwrap();
        assert_eq!(i.ty, SEND_KEY);
        assert_eq!(i.id, Some(77));
        let k = i.key.unwrap();
        assert_eq!(keysym(&k), Some('k' as u32));
        assert!(is_romaji_start(&k));
    }

    #[test]
    fn special_and_modifiers() {
        let b = command(|k| {
            k.uint(3, SK_BACKSPACE as u64);
            k.uint(4, 4);
        });
        let (_, i) = decode_command_input(&b).unwrap();
        let k = i.key.unwrap();
        assert_eq!(keysym(&k), Some(0xff08));
        assert!(k.shift());
        assert!(!is_romaji_start(&k));
    }

    #[test]
    fn kana_key_string_is_not_romaji() {
        let b = command(|k| {
            k.uint(1, '1' as u64);
            k.string(5, "あ");
        });
        let (_, i) = decode_command_input(&b).unwrap();
        assert!(!is_romaji_start(&i.key.unwrap()));
    }

    #[test]
    fn request_table() {
        let mut w = W::new();
        w.message(1, |i| {
            i.uint(1, SET_REQUEST as u64);
            i.message(9, |r| r.int(4, 16));
        });
        let (_, i) = decode_command_input(&w.0).unwrap();
        assert_eq!(i.request_table, Some(16));
    }

    #[test]
    fn context() {
        let mut w = W::new();
        w.message(1, |i| {
            i.uint(1, SEND_KEY as u64);
            i.message(6, |c| c.string(1, "今日は"));
        });
        let (_, i) = decode_command_input(&w.0).unwrap();
        assert_eq!(i.preceding.as_deref(), Some("今日は"));
    }
}
