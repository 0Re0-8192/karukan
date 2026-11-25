//! `karukan-setup.exe --word [--reading R] [--surface S]`: the word
//! registration window (Ctrl+F7 in the text service, MS-IME's key for it).
//!
//! A small window of its own process, not one inside the app being typed
//! into: the app is never held up by it, and karukan types into its fields
//! like into any other. A registered word is appended to
//! `data\user_dicts\registered.txt` beside this exe (Mozc TSV, UTF-8), which
//! the running servers read in place within a couple of seconds.

use std::io::Write;
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::RECT;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::COLOR_BTNFACE;
use windows::Win32::Graphics::Gdi::{
    CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, DEFAULT_CHARSET, FF_DONTCARE, FW_NORMAL,
    HBRUSH, HFONT, OUT_DEFAULT_PRECIS,
};
use windows::Win32::System::Diagnostics::Debug::MessageBeep;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForSystem, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, BS_DEFPUSHBUTTON, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW,
    DestroyWindow, DispatchMessageW, ES_AUTOHSCROLL, GetMessageW, GetSystemMetrics,
    GetWindowTextLengthW, GetWindowTextW, HMENU, IDCANCEL, IDOK, IsDialogMessageW, MB_ICONWARNING,
    MSG, PostQuitMessage, RegisterClassExW, SM_CXSCREEN, SM_CYSCREEN, SW_SHOW, SendMessageW,
    SetForegroundWindow, ShowWindow, TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE,
    WM_COMMAND, WM_DESTROY, WM_SETFONT, WNDCLASSEXW, WS_CAPTION, WS_CHILD, WS_EX_CLIENTEDGE,
    WS_EX_DLGMODALFRAME, WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE,
};
use windows::core::{HSTRING, PCWSTR, w};

const ID_READING: i32 = 101;
const ID_SURFACE: i32 = 102;

thread_local! {
    static FIELDS: std::cell::Cell<(HWND, HWND)> = const { std::cell::Cell::new((HWND(std::ptr::null_mut()), HWND(std::ptr::null_mut()))) };
    static DEST: std::cell::RefCell<PathBuf> = const { std::cell::RefCell::new(PathBuf::new()) };
}

/// Where registered words go: `data\user_dicts\registered.txt` beside the exe.
pub fn registered_file(install: &Path) -> PathBuf {
    install
        .join("data")
        .join("user_dicts")
        .join("registered.txt")
}

/// The reading as the dictionary keys it: hiragana, katakana folded, trimmed.
pub fn normalize_reading(reading: &str) -> String {
    reading
        .trim()
        .chars()
        .map(|c| match c {
            '\u{30a1}'..='\u{30f6}' => char::from_u32(c as u32 - 0x60).unwrap_or(c),
            c => c,
        })
        .collect()
}

/// Append one entry to `file` (created with its directory), unless the same
/// reading and word are already there. `Err` with a reason the window beeps
/// for: an empty field, or a tab or line break that would break the format.
pub fn register(file: &Path, reading: &str, surface: &str) -> Result<bool, &'static str> {
    let reading = normalize_reading(reading);
    let surface = surface.trim();
    if reading.is_empty() || surface.is_empty() {
        return Err("empty");
    }
    if [reading.as_str(), surface]
        .iter()
        .any(|s| s.contains(['\t', '\n', '\r']))
    {
        return Err("tab or line break");
    }
    let existing = std::fs::read_to_string(file).unwrap_or_default();
    let already = existing.lines().any(|l| {
        let mut f = l.split('\t');
        f.next() == Some(reading.as_str()) && f.next() == Some(surface)
    });
    if already {
        return Ok(false);
    }
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|_| "cannot create the folder")?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
        .map_err(|_| "cannot open the file")?;
    let sep = if existing.is_empty() || existing.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    writeln!(f, "{sep}{reading}\t{surface}\t名詞\t").map_err(|_| "cannot write")?;
    Ok(true)
}

fn text_of(h: HWND) -> String {
    unsafe {
        let n = GetWindowTextLengthW(h);
        let mut buf = vec![0u16; n as usize + 1];
        let got = GetWindowTextW(h, &mut buf);
        String::from_utf16_lossy(&buf[..got as usize])
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_COMMAND => {
            let id = (wp.0 & 0xffff) as i32;
            if id == IDOK.0 {
                let (r, s) = FIELDS.with(|f| f.get());
                let file = DEST.with(|d| d.borrow().clone());
                match register(&file, &text_of(r), &text_of(s)) {
                    Ok(_) => unsafe {
                        let _ = DestroyWindow(hwnd);
                    },
                    Err(_) => unsafe {
                        let _ = MessageBeep(MB_ICONWARNING);
                        let _ = SetFocus(Some(if text_of(r).trim().is_empty() { r } else { s }));
                    },
                }
            } else if id == IDCANCEL.0 {
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

/// Show the window; returns when it is closed. `reading`/`surface` prefill
/// the fields (the composition Ctrl+F7 was pressed in).
pub fn run(install: &Path, reading: &str, surface: &str) -> i32 {
    DEST.with(|d| *d.borrow_mut() = registered_file(install));
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let dpi = GetDpiForSystem().max(96) as i32;
        let px = |v: i32| v * dpi / 96;
        let Ok(module) = GetModuleHandleW(None) else {
            return 2;
        };
        let class = w!("KarukanWordWindow");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wnd_proc),
            hInstance: module.into(),
            lpszClassName: class,
            hbrBackground: HBRUSH((COLOR_BTNFACE.0 + 1) as isize as *mut _),
            ..Default::default()
        };
        RegisterClassExW(&wc);
        let font = CreateFontW(
            -px(15),
            0,
            0,
            0,
            FW_NORMAL.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            FF_DONTCARE.0 as u32,
            w!("Yu Gothic UI"),
        );
        // the client area the controls need, the frame around it added
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU;
        let mut frame = RECT {
            left: 0,
            top: 0,
            right: px(330),
            bottom: px(130),
        };
        let _ = AdjustWindowRectEx(&mut frame, style, false, WS_EX_DLGMODALFRAME);
        let (cw, ch) = (frame.right - frame.left, frame.bottom - frame.top);
        let x = (GetSystemMetrics(SM_CXSCREEN) - cw) / 2;
        let y = (GetSystemMetrics(SM_CYSCREEN) - ch) / 3;
        let Ok(hwnd) = CreateWindowExW(
            WS_EX_DLGMODALFRAME,
            class,
            w!("単語の登録"),
            style,
            if x > 0 { x } else { CW_USEDEFAULT },
            if y > 0 { y } else { CW_USEDEFAULT },
            cw,
            ch,
            None,
            None,
            Some(module.into()),
            None,
        ) else {
            return 2;
        };
        let child = |ex: WINDOW_EX_STYLE,
                     cls: PCWSTR,
                     text: &str,
                     style: WINDOW_STYLE,
                     x,
                     y,
                     w_,
                     h,
                     id: i32| {
            let h = CreateWindowExW(
                ex,
                cls,
                &HSTRING::from(text),
                WS_CHILD | WS_VISIBLE | style,
                px(x),
                px(y),
                px(w_),
                px(h),
                Some(hwnd),
                Some(HMENU(id as isize as *mut _)),
                Some(module.into()),
                None,
            )
            .unwrap_or_default();
            SendMessageW(
                h,
                WM_SETFONT,
                Some(WPARAM(font.0 as usize)),
                Some(LPARAM(1)),
            );
            h
        };
        let none = WINDOW_STYLE(0);
        let edit = WINDOW_STYLE(ES_AUTOHSCROLL as u32) | WS_TABSTOP;
        child(
            WINDOW_EX_STYLE(0),
            w!("STATIC"),
            "よみ",
            none,
            14,
            18,
            50,
            22,
            0,
        );
        let r = child(
            WS_EX_CLIENTEDGE,
            w!("EDIT"),
            reading,
            edit,
            66,
            14,
            250,
            26,
            ID_READING,
        );
        child(
            WINDOW_EX_STYLE(0),
            w!("STATIC"),
            "単語",
            none,
            14,
            54,
            50,
            22,
            0,
        );
        let s = child(
            WS_EX_CLIENTEDGE,
            w!("EDIT"),
            surface,
            edit,
            66,
            50,
            250,
            26,
            ID_SURFACE,
        );
        child(
            WINDOW_EX_STYLE(0),
            w!("BUTTON"),
            "登録",
            WINDOW_STYLE(BS_DEFPUSHBUTTON as u32) | WS_TABSTOP,
            216,
            88,
            100,
            28,
            IDOK.0,
        );
        FIELDS.with(|f| f.set((r, s)));
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        // the empty field first: a reading from the composition leaves the word to type
        let _ = SetFocus(Some(if reading.is_empty() || !surface.is_empty() {
            r
        } else {
            s
        }));

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            // Tab between the fields, Enter = 登録, Esc = close
            if IsDialogMessageW(hwnd, &msg).as_bool() {
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let _: HFONT = font;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_word_is_appended_once_with_its_reading_in_hiragana() {
        let dir = std::env::temp_dir().join(format!("karukan-word-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = registered_file(&dir);
        assert_eq!(register(&file, " カルカン ", "karukan"), Ok(true));
        assert_eq!(register(&file, "かるかん", "karukan"), Ok(false));
        assert_eq!(register(&file, "てすと", "テスト語"), Ok(true));
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "かるかん\tkarukan\t名詞\t\nてすと\tテスト語\t名詞\t\n"
        );
        assert!(register(&file, "", "x").is_err());
        assert!(register(&file, "あ", "a\tb").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
