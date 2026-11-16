//! Windows virtual-key → XKB keysym translation (the representation
//! `karukan-imserver` speaks), plus the MS-IME style key classification.

use crate::engine::Mods;

pub const KEYSYM_SPACE: u32 = 0x0020;
/// Super_R: the engine's "return to hiragana mode" key.
pub const KEYSYM_SUPER_R: u32 = 0xffec;

pub const VK_BACK: u32 = 0x08;
pub const VK_TAB: u32 = 0x09;
pub const VK_RETURN: u32 = 0x0D;
pub const VK_SHIFT: u32 = 0x10;
pub const VK_CONTROL: u32 = 0x11;
pub const VK_MENU: u32 = 0x12;
pub const VK_CAPITAL: u32 = 0x14;
pub const VK_KANA: u32 = 0x15;
pub const VK_KANJI: u32 = 0x19;
pub const VK_ESCAPE: u32 = 0x1B;
pub const VK_CONVERT: u32 = 0x1C;
pub const VK_NONCONVERT: u32 = 0x1D;
pub const VK_SPACE: u32 = 0x20;
pub const VK_LWIN: u32 = 0x5B;
pub const VK_RWIN: u32 = 0x5C;
pub const VK_OEM_COPY: u32 = 0xF2; // ひらがな/カタカナ
pub const VK_OEM_AUTO: u32 = 0xF3; // 半角/全角
pub const VK_OEM_ENLW: u32 = 0xF4; // 半角/全角

/// Keys that have a fixed keysym independent of layout and modifiers.
pub fn special_keysym(vk: u32) -> Option<u32> {
    Some(match vk {
        VK_BACK => 0xff08,
        VK_TAB => 0xff09,
        VK_RETURN => 0xff0d,
        VK_ESCAPE => 0xff1b,
        VK_SPACE => KEYSYM_SPACE,
        0x21 => 0xff55, // PageUp
        0x22 => 0xff56, // PageDown
        0x23 => 0xff57, // End
        0x24 => 0xff50, // Home
        0x25 => 0xff51, // Left
        0x26 => 0xff52, // Up
        0x27 => 0xff53, // Right
        0x28 => 0xff54, // Down
        0x2E => 0xffff, // Delete
        0x75 => 0xffc3, // F6
        0x76 => 0xffc4, // F7
        0x77 => 0xffc5, // F8
        0x78 => 0xffc6, // F9
        0x79 => 0xffc7, // F10
        _ => return None,
    })
}

/// Modifier-only keys never go to the engine.
pub fn is_modifier_vk(vk: u32) -> bool {
    matches!(
        vk,
        VK_SHIFT | VK_CONTROL | VK_MENU | VK_CAPITAL | VK_LWIN | VK_RWIN | 0xA0..=0xA5
    )
}

/// 半角/全角 family: toggles the IME on/off.
pub fn is_open_close_vk(vk: u32) -> bool {
    matches!(vk, VK_KANJI | VK_OEM_AUTO | VK_OEM_ENLW)
}

/// Letter/digit for Ctrl chords, where `ToUnicode` would give a control char.
pub fn ctrl_chord_keysym(vk: u32) -> Option<u32> {
    match vk {
        0x41..=0x5A => Some(vk + 0x20), // 'A'..'Z' -> 'a'..'z'
        0x30..=0x39 => Some(vk),
        _ => None,
    }
}

/// MS-IME's Ctrl chords for the script conversions: Ctrl+U/I/O/P/T act as
/// F6/F7/F8/F9/F10. Only meaningful while composing.
pub fn msime_chord(keysym: u32, mods: Mods) -> Option<u32> {
    if !mods.control || mods.alt {
        return None;
    }
    match keysym {
        0x75 => Some(0xffc3), // u -> F6
        0x69 => Some(0xffc4), // i -> F7
        0x6f => Some(0xffc5), // o -> F8
        0x70 => Some(0xffc6), // p -> F9
        0x74 => Some(0xffc7), // t -> F10
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Key {
    pub keysym: u32,
    pub mods: Mods,
}

/// Modifier state from a 256-byte `GetKeyboardState` array.
pub fn mods_from_state(state: &[u8; 256]) -> Mods {
    let down = |vk: u32| state[vk as usize] & 0x80 != 0;
    Mods {
        shift: down(VK_SHIFT),
        control: down(VK_CONTROL),
        // AltGr reports as Ctrl+Alt; treat a lone right Alt as a layout shift.
        alt: down(VK_MENU) && !down(VK_CONTROL),
    }
}

#[cfg(windows)]
pub fn translate(vk: u32, lparam: isize) -> Option<(Key, [u8; 256])> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetKeyboardLayout, GetKeyboardState, ToUnicodeEx,
    };
    let mut state = [0u8; 256];
    unsafe { GetKeyboardState(&mut state).ok()? };
    let mods = mods_from_state(&state);

    if let Some(keysym) = special_keysym(vk) {
        return Some((Key { keysym, mods }, state));
    }
    if mods.control {
        let keysym = ctrl_chord_keysym(vk)?;
        return Some((Key { keysym, mods }, state));
    }
    let scan = ((lparam >> 16) & 0xff) as u32;
    let mut buf = [0u16; 8];
    let n = unsafe {
        // flag 0x4: do not change the keyboard's dead-key state
        ToUnicodeEx(vk, scan, &state, &mut buf, 0x4, Some(GetKeyboardLayout(0)))
    };
    if n == 1 && (0x20..=0x7e).contains(&buf[0]) {
        return Some((
            Key {
                keysym: buf[0] as u32,
                mods,
            },
            state,
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specials() {
        assert_eq!(special_keysym(VK_RETURN), Some(0xff0d));
        assert_eq!(special_keysym(0x41), None);
    }

    #[test]
    fn ctrl() {
        assert_eq!(ctrl_chord_keysym(0x4B), Some('k' as u32));
        assert_eq!(ctrl_chord_keysym(0x31), Some('1' as u32));
        assert_eq!(ctrl_chord_keysym(0xBA), None);
    }

    #[test]
    fn chords() {
        let ctrl = Mods {
            control: true,
            ..Mods::default()
        };
        assert_eq!(msime_chord('i' as u32, ctrl), Some(0xffc4));
        assert_eq!(msime_chord('k' as u32, ctrl), None);
        assert_eq!(msime_chord('i' as u32, Mods::default()), None);
    }

    #[test]
    fn mods() {
        let mut s = [0u8; 256];
        s[VK_SHIFT as usize] = 0x80;
        let m = mods_from_state(&s);
        assert!(m.shift && !m.control);
    }
}
