//! Words to take over from another IME: Microsoft IME's user dictionary,
//! read in place through its own COM interface (`IFEDictionary`, the one
//! Mozc's importer uses), and the text files IMEs export (MS-IME's
//! UTF-16 export, Mozc / Google 日本語入力 TSV, Shift_JIS from older tools).

use windows::Win32::Globalization::{MULTI_BYTE_TO_WIDE_CHAR_FLAGS, MultiByteToWideChar};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CLSIDFromProgID, COINIT_APARTMENTTHREADED, CoCreateInstance,
    CoInitializeEx,
};
use windows::Win32::UI::Input::Ime::{
    IFED_E_NO_ENTRY, IFED_POS_ALL, IFED_REG_USER, IFED_S_EMPTY_DICTIONARY, IFED_S_MORE_ENTRIES,
    IFED_SELECT_ALL, IFEDictionary, IMEWRD,
};
use windows::core::{Interface, PCWSTR, w};

/// One word: reading (hiragana), the word, and the columns after them as
/// the source wrote them (part of speech, comment: kept in the file so
/// nothing is lost, unused by the converter).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub reading: String,
    pub surface: String,
    pub rest: String,
}

/// The columns after the word when the source gave none.
pub const NOUN: &str = "名詞\t";

/// The words of Microsoft IME's user dictionary. `Err` when MS-IME is not
/// there to ask (an edition without it, or its COM server unregistered).
pub fn read_msime() -> Result<Vec<Entry>, String> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let clsid = CLSIDFromProgID(w!("MSIME.Japan")).map_err(|e| format!("MSIME.Japan: {e}"))?;
        let dic: IFEDictionary = CoCreateInstance(&clsid, None, CLSCTX_INPROC_SERVER)
            .map_err(|e| format!("IFEDictionary: {e}"))?;
        // the user dictionary MS-IME itself uses
        dic.Open(None, std::ptr::null_mut())
            .map_err(|e| format!("Open: {e}"))?;
        let out = read_all(&dic);
        let _ = dic.Close();
        out
    }
}

unsafe fn read_all(dic: &IFEDictionary) -> Result<Vec<Entry>, String> {
    // IMEWRD records first, the strings they point at after them
    let mut buf = vec![0u8; 512 * 1024];
    let mut out = Vec::new();
    let mut n = 0u32;
    let vt = dic.vtable();
    // called through the vtable: the wrapper folds IFED_S_MORE_ENTRIES into Ok
    let mut hr = unsafe {
        (vt.GetWords)(
            dic.as_raw(),
            PCWSTR::null(),
            PCWSTR::null(),
            PCWSTR::null(),
            IFED_POS_ALL,
            IFED_SELECT_ALL,
            IFED_REG_USER,
            buf.as_mut_ptr(),
            buf.len() as u32,
            &mut n,
        )
    };
    loop {
        if hr == IFED_E_NO_ENTRY || hr == IFED_S_EMPTY_DICTIONARY {
            return Ok(out);
        }
        if hr.is_err() {
            return Err(format!("GetWords: {hr:?}"));
        }
        let words = buf.as_ptr() as *const IMEWRD;
        for i in 0..n as usize {
            let w = unsafe { std::ptr::read_unaligned(words.add(i)) };
            let reading = super::word::normalize_reading(&unsafe { wide(w.pwchReading.0) });
            let surface = unsafe { wide(w.pwchDisplay.0) }.trim().to_string();
            if usable(&reading, &surface) {
                out.push(Entry {
                    reading,
                    surface,
                    rest: NOUN.into(),
                });
            }
        }
        if hr != IFED_S_MORE_ENTRIES {
            return Ok(out);
        }
        n = 0;
        hr = unsafe { (vt.NextWords)(dic.as_raw(), buf.as_mut_ptr(), buf.len() as u32, &mut n) };
    }
}

unsafe fn wide(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut len = 0;
    while unsafe { *p.add(len) } != 0 {
        len += 1;
    }
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, len) })
}

/// A file's text whatever it was saved as: UTF-16 (MS-IME's export), UTF-8,
/// or Shift_JIS.
pub fn decode(bytes: &[u8]) -> String {
    let utf16 = |b: &[u8], le: bool| {
        let units: Vec<u16> = b
            .chunks_exact(2)
            .map(|c| {
                if le {
                    u16::from_le_bytes([c[0], c[1]])
                } else {
                    u16::from_be_bytes([c[0], c[1]])
                }
            })
            .collect();
        String::from_utf16_lossy(&units)
    };
    match bytes {
        [0xff, 0xfe, rest @ ..] => utf16(rest, true),
        [0xfe, 0xff, rest @ ..] => utf16(rest, false),
        [0xef, 0xbb, 0xbf, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        // UTF-16 without a mark: every other byte of ASCII text is zero
        b if b.len() >= 4 && b[1] == 0 && b[3] == 0 => utf16(b, true),
        b => match std::str::from_utf8(b) {
            Ok(s) => s.to_string(),
            Err(_) => sjis(b),
        },
    }
}

fn sjis(b: &[u8]) -> String {
    unsafe {
        let n = MultiByteToWideChar(932, MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0), b, None);
        let mut out = vec![0u16; n.max(0) as usize];
        let got = MultiByteToWideChar(932, MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0), b, Some(&mut out));
        String::from_utf16_lossy(&out[..got.max(0) as usize])
    }
}

/// Both fields present, and neither carrying what would break a TSV line.
pub fn usable(reading: &str, surface: &str) -> bool {
    !reading.is_empty()
        && !surface.is_empty()
        && !reading.contains(['\t', '\n', '\r'])
        && !surface.contains(['\t', '\n', '\r'])
}

/// The words of an exported dictionary: `よみ<TAB>単語[<TAB>品詞[<TAB>…]]`
/// per line, MS-IME's `!` header lines and Mozc's `#` comments skipped, and
/// so are Mozc's 抑制単語 rows: they hide a word, they are not one.
pub fn parse(text: &str) -> Vec<Entry> {
    text.lines()
        .filter(|l| !l.starts_with(['!', '#']))
        .filter_map(|l| {
            let mut f = l.splitn(3, '\t');
            let reading = super::word::normalize_reading(f.next()?);
            let surface = f.next()?.trim().to_string();
            let rest = f.next().unwrap_or("");
            if rest.split('\t').next().map(str::trim) == Some("抑制単語") {
                return None;
            }
            usable(&reading, &surface).then(|| Entry {
                reading,
                surface,
                rest: if rest.trim().is_empty() {
                    NOUN.into()
                } else {
                    rest.to_string()
                },
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_msime_export_is_read_from_utf16() {
        let text = "!Microsoft IME Dictionary Tool\r\n!Version:\r\n\r\nかぶ\t株式会社\t短縮よみ\r\nカルカン\tkarukan\t名詞\r\n";
        let mut bytes = vec![0xff, 0xfe];
        bytes.extend(text.encode_utf16().flat_map(|u| u.to_le_bytes()));
        let got = parse(&decode(&bytes));
        assert_eq!(
            got,
            vec![
                Entry {
                    reading: "かぶ".into(),
                    surface: "株式会社".into(),
                    rest: "短縮よみ".into()
                },
                Entry {
                    reading: "かるかん".into(),
                    surface: "karukan".into(),
                    rest: "名詞".into()
                },
            ]
        );
    }

    #[test]
    fn a_mozc_export_and_shift_jis_are_read() {
        let got = parse(&decode(
            "# comment\nてすと\tテスト語\t名詞\tメモ\nだめ\nかくす\t隠す\t抑制単語\t\nなし\tナシ\n"
                .as_bytes(),
        ));
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].surface, "テスト語");
        assert_eq!(got[0].rest, "名詞\tメモ");
        assert_eq!(got[1].rest, NOUN);
        // 「あ\t亜」 in Shift_JIS
        let got = parse(&decode(&[0x82, 0xa0, b'\t', 0x88, 0x9f]));
        assert_eq!(got[0].reading, "あ");
        assert_eq!(got[0].surface, "亜");
    }

    /// The words of this machine's MS-IME, counted and checked for shape
    /// (kana readings, words of a sane length), never printed.
    #[test]
    #[ignore]
    fn msime_count() {
        let words = read_msime().expect("MS-IME");
        println!("msime words: {}", words.len());
        for w in &words {
            assert!(
                w.reading
                    .chars()
                    .all(|c| matches!(c, '\u{3041}'..='\u{309f}' | 'ー')
                        || c.is_ascii_alphanumeric()),
                "a reading that is not kana"
            );
            assert!((1..=200).contains(&w.surface.chars().count()));
        }
    }
}
