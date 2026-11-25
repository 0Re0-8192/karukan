//! `karukan-setup.exe --word [--reading R] [--surface S]`: the word
//! registration window (Ctrl+F7 in the text service, MS-IME's key for it,
//! and 単語の登録 in the taskbar button's menu).
//!
//! A small window of its own process, not one inside the app being typed
//! into: the app is never held up by it, and karukan types into its fields
//! like into any other. The words live in `data\user_dicts\registered.txt`
//! beside this exe (Mozc TSV, UTF-8), which the running servers read in
//! place within a couple of seconds.
//!
//! One window does it all: the two fields and 登録 on top (type, Enter,
//! gone), the registered words under them, filtered by what the fields hold
//! so a word already there shows up while it is typed, Delete to remove,
//! and 取り込み for Microsoft IME's own dictionary or an exported file (a
//! file dropped on the window too). No part of speech or comment: the
//! converter uses neither; an import keeps those columns in the file.
//!
//! `--import FILE` / `--import-msime` do an import without the window; the
//! exit code is the number of words added (negative: failed).

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Globalization::{LCMAP_FULLWIDTH, LCMAP_HIRAGANA, LCMapStringEx};
use windows::Win32::Graphics::Gdi::{
    CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, COLOR_GRAYTEXT, COLOR_WINDOW, COLOR_WINDOWTEXT,
    CreateFontW, DEFAULT_CHARSET, FF_DONTCARE, FW_NORMAL, GetSysColor, GetSysColorBrush, HBRUSH,
    HDC, HFONT, InvalidateRect, OUT_DEFAULT_PRECIS, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
};
use windows::Win32::System::Diagnostics::Debug::MessageBeep;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{
    CDDS_ITEMPREPAINT, CDDS_PREPAINT, CDDS_SUBITEM, CDRF_DODEFAULT, CDRF_NEWFONT,
    CDRF_NOTIFYITEMDRAW, EM_SETCUEBANNER, ICC_LISTVIEW_CLASSES, INITCOMMONCONTROLSEX,
    InitCommonControlsEx, LVCF_TEXT, LVCF_WIDTH, LVCOLUMNW, LVIF_TEXT, LVIS_FOCUSED, LVIS_SELECTED,
    LVITEMW, LVM_ENSUREVISIBLE, LVM_GETITEMSTATE, LVM_GETNEXTITEM, LVM_INSERTCOLUMNW,
    LVM_SETEXTENDEDLISTVIEWSTYLE, LVM_SETITEMCOUNT, LVM_SETITEMSTATE, LVN_GETDISPINFOW,
    LVN_KEYDOWN, LVNI_SELECTED, LVS_EX_DOUBLEBUFFER, LVS_EX_FULLROWSELECT, LVS_NOCOLUMNHEADER,
    LVS_OWNERDATA, LVS_REPORT, LVS_SHOWSELALWAYS, NM_CUSTOMDRAW, NMHDR, NMLVCUSTOMDRAW,
    NMLVDISPINFOW, NMLVKEYDOWN, WC_LISTVIEWW,
};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForSystem, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, SetFocus, VK_CONTROL, VK_DELETE};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    DragAcceptFiles, DragFinish, DragQueryFileW, FOS_FILEMUSTEXIST, FileOpenDialog, HDROP,
    IFileOpenDialog, SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, AppendMenuW, BS_DEFPUSHBUTTON, CW_USEDEFAULT, CreatePopupMenu,
    CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow, DispatchMessageW, EN_CHANGE,
    ES_AUTOHSCROLL, GetCursorPos, GetMessageW, GetSystemMetrics, GetWindowRect,
    GetWindowTextLengthW, GetWindowTextW, HMENU, IDCANCEL, IDOK, IsDialogMessageW, MB_ICONWARNING,
    MF_GRAYED, MF_STRING, MSG, PostQuitMessage, RegisterClassExW, SM_CXSCREEN, SM_CYSCREEN,
    SW_SHOW, SendMessageW, SetForegroundWindow, SetWindowTextW, ShowWindow, TPM_RETURNCMD,
    TrackPopupMenu, TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE, WM_COMMAND,
    WM_CONTEXTMENU, WM_CTLCOLORSTATIC, WM_DESTROY, WM_DROPFILES, WM_KEYDOWN, WM_NOTIFY, WM_SETFONT,
    WNDCLASSEXW, WS_CAPTION, WS_CHILD, WS_EX_CLIENTEDGE, WS_EX_DLGMODALFRAME, WS_OVERLAPPED,
    WS_SYSMENU, WS_TABSTOP, WS_VISIBLE,
};
use windows::core::{HSTRING, PCWSTR, PWSTR, w};

use super::msime::{self, Entry, NOUN};

const ID_SURFACE: i32 = 101;
const ID_READING: i32 = 102;
const ID_LIST: i32 = 103;
const ID_IMPORT: i32 = 104;
const MENU_MSIME: usize = 1;
const MENU_FILE: usize = 2;
const MENU_DELETE: usize = 3;

/// Where registered words go: `data\user_dicts\registered.txt` beside the exe.
pub fn registered_file(install: &Path) -> PathBuf {
    install
        .join("data")
        .join("user_dicts")
        .join("registered.txt")
}

/// The widths kana input types digits and ASCII marks in (`[width]` of
/// config.toml): a reading must be keyed the way it will be typed, or the
/// word never comes up. Letters are always half-width there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Widths {
    pub half_digits: bool,
    pub half_marks: bool,
}

/// What karukan ships: digits half-width, marks full-width.
pub const SHIPPED: Widths = Widths {
    half_digits: true,
    half_marks: false,
};

static WIDTHS: std::sync::OnceLock<Widths> = std::sync::OnceLock::new();

/// The widths `config\config.toml` beside the exe sets, the shipped ones
/// for what it leaves out.
pub fn read_widths(install: &Path) -> Widths {
    let text =
        std::fs::read_to_string(install.join("config").join("config.toml")).unwrap_or_default();
    let mut w = SHIPPED;
    let mut in_width = false;
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.starts_with('[') {
            in_width = line == "[width]";
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if !in_width {
            continue;
        }
        let half = value.trim().trim_matches('"') == "half";
        match key.trim() {
            "digit" => w.half_digits = half,
            "ascii_symbol" => w.half_marks = half,
            _ => {}
        }
    }
    w
}

/// The reading as the dictionary keys it, with this install's widths.
pub fn normalize_reading(reading: &str) -> String {
    normalize_reading_with(reading, *WIDTHS.get().unwrap_or(&SHIPPED))
}

/// The reading as kana input would have typed it: hiragana (katakana folded,
/// the half-width kind joined with its marks: ﾃﾞｰﾀ → でーた), letters
/// half-width (MS-IME keeps ｎｉｃｅ so), digits and ASCII marks in the
/// widths `w` says, trimmed.
pub fn normalize_reading_with(reading: &str, w: Widths) -> String {
    let mut out = String::new();
    let mut half_kana: Vec<u16> = Vec::new();
    let flush = |half_kana: &mut Vec<u16>, out: &mut String| {
        if half_kana.is_empty() {
            return;
        }
        let mut buf = vec![0u16; half_kana.len() * 2 + 8];
        let got = unsafe {
            LCMapStringEx(
                w!("ja-JP"),
                LCMAP_HIRAGANA | LCMAP_FULLWIDTH,
                half_kana,
                Some(&mut buf),
                None,
                None,
                LPARAM(0),
            )
        };
        if got > 0 {
            out.push_str(&String::from_utf16_lossy(&buf[..got as usize]));
        } else {
            out.push_str(&String::from_utf16_lossy(half_kana));
        }
        half_kana.clear();
    };
    for c in reading.trim().chars() {
        if ('\u{ff61}'..='\u{ff9f}').contains(&c) {
            half_kana.push(c as u16);
            continue;
        }
        flush(&mut half_kana, &mut out);
        let half = |c: char| char::from_u32(c as u32 - 0xfee0).unwrap_or(c);
        let full = |c: char| char::from_u32(c as u32 + 0xfee0).unwrap_or(c);
        out.push(match c {
            '\u{30a1}'..='\u{30f6}' => char::from_u32(c as u32 - 0x60).unwrap_or(c),
            'Ａ'..='Ｚ' | 'ａ'..='ｚ' => half(c),
            '０'..='９' if w.half_digits => half(c),
            '0'..='9' if !w.half_digits => full(c),
            '\u{ff01}'..='\u{ff5e}' if w.half_marks => half(c),
            '!'..='~' if !w.half_marks && !c.is_ascii_alphanumeric() => full(c),
            c => c,
        });
    }
    flush(&mut half_kana, &mut out);
    out
}

/// The words of `file`, in file order (oldest first); none when there is no
/// file yet. Any other failure is an error, never an empty list: a list
/// read as empty would be saved over the words.
pub fn load(file: &Path) -> std::io::Result<Vec<Entry>> {
    match std::fs::read(file) {
        Ok(b) => Ok(msime::parse(&msime::decode(&b))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

/// Change the words of `file`: read them afresh (another window may have
/// changed them), let `f` edit them, and write them back if it says so.
/// Returns the words as they are now, with what `f` returned.
pub fn update<R>(
    file: &Path,
    f: impl FnOnce(&mut Vec<Entry>) -> (bool, R),
) -> std::io::Result<(Vec<Entry>, R)> {
    let mut entries = load(file)?;
    let (changed, r) = f(&mut entries);
    if changed {
        save(file, &entries)?;
    }
    Ok((entries, r))
}

/// Write `entries` as the whole of `file`. Written beside the `user_dicts`
/// folder and renamed over the file: every file in that folder is loaded
/// as a dictionary, and a server looking in mid-write must see either the
/// old words or the new ones, never a half or a stray temp file.
pub fn save(file: &Path, entries: &[Entry]) -> std::io::Result<()> {
    let dir = file.parent().ok_or(std::io::ErrorKind::NotFound)?;
    std::fs::create_dir_all(dir)?;
    let tmp = dir.parent().unwrap_or(dir).join(".registered.txt.tmp");
    let mut text = String::new();
    for e in entries {
        text.push_str(&format!("{}\t{}\t{}\n", e.reading, e.surface, e.rest));
    }
    std::fs::write(&tmp, text)?;
    let mut last = Ok(());
    for _ in 0..10 {
        last = std::fs::rename(&tmp, file);
        if last.is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if last.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    last
}

/// Add the words of `new` not already in `entries` (same reading and word);
/// returns how many were added, at the end.
pub fn merge(entries: &mut Vec<Entry>, new: Vec<Entry>) -> usize {
    let mut seen: HashSet<(String, String)> = entries
        .iter()
        .map(|e| (e.reading.clone(), e.surface.clone()))
        .collect();
    let before = entries.len();
    for e in new {
        if seen.insert((e.reading.clone(), e.surface.clone())) {
            entries.push(e);
        }
    }
    entries.len() - before
}

/// Add one word to `file` (created with its folder), unless the same reading
/// and word are already there. `Err` with a reason the window beeps for: an
/// empty field, or a tab or line break that would break the format.
pub fn register(file: &Path, reading: &str, surface: &str) -> Result<bool, &'static str> {
    let reading = normalize_reading(reading);
    let surface = surface.trim();
    if reading.is_empty() || surface.is_empty() {
        return Err("empty");
    }
    if !msime::usable(&reading, surface) {
        return Err("tab or line break");
    }
    let new = Entry {
        reading,
        surface: surface.to_string(),
        rest: NOUN.into(),
    };
    let (_, added) = update(file, |e| {
        let n = merge(e, vec![new]);
        (n > 0, n)
    })
    .map_err(|_| "cannot write")?;
    Ok(added > 0)
}

/// The words of `source`: an exported file, or `None` for Microsoft IME.
fn read_source(source: Option<&Path>) -> Result<Vec<Entry>, String> {
    match source {
        Some(p) => Ok(msime::parse(&msime::decode(
            &std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?,
        ))),
        None => msime::read_msime(),
    }
}

/// Import into `file`: `source` is a path, or `None` for Microsoft IME.
/// Returns how many words were added.
pub fn import(file: &Path, source: Option<&Path>) -> Result<usize, String> {
    let new = read_source(source)?;
    let (_, added) = update(file, |e| {
        let n = merge(e, new);
        (n > 0, n)
    })
    .map_err(|e| e.to_string())?;
    Ok(added)
}

/// `--import FILE` / `--import-msime`, no window: the exit code is the count.
pub fn run_import(install: &Path, source: Option<&Path>) -> i32 {
    let _ = WIDTHS.set(read_widths(install));
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    match import(&registered_file(install), source) {
        Ok(n) => n.min(i32::MAX as usize) as i32,
        Err(_) => -1,
    }
}

/// What the window holds.
struct Ui {
    file: PathBuf,
    /// every word, file order
    all: Vec<Entry>,
    /// indices into `all` the list shows: newest first, filtered by the fields
    shown: Vec<usize>,
    surface: HWND,
    reading: HWND,
    list: HWND,
    status: HWND,
    import: HWND,
    /// the words the last Delete removed, for Ctrl+Z
    removed: Vec<Entry>,
    /// added by the last import, shown until the next change
    note: Option<usize>,
}

thread_local! {
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
}

/// `None` while the window is being built, or when a control notifies us
/// from inside a call made under the borrow (setting a field's text sends
/// EN_CHANGE back): the caller refreshes after that call anyway.
fn with_ui<R>(f: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    UI.with(|u| u.try_borrow_mut().ok()?.as_mut().map(f))
}

fn text_of(h: HWND) -> String {
    unsafe {
        let n = GetWindowTextLengthW(h);
        let mut buf = vec![0u16; n as usize + 1];
        let got = GetWindowTextW(h, &mut buf);
        String::from_utf16_lossy(&buf[..got as usize])
    }
}

impl Ui {
    /// Re-filter the list by the fields and redraw it with the count.
    fn refresh(&mut self) {
        let s = text_of(self.surface).trim().to_string();
        let r = normalize_reading(&text_of(self.reading));
        self.shown = (0..self.all.len())
            .rev()
            .filter(|&i| {
                let e = &self.all[i];
                (s.is_empty() || e.surface.contains(&s)) && (r.is_empty() || e.reading.contains(&r))
            })
            .collect();
        unsafe {
            SendMessageW(
                self.list,
                LVM_SETITEMCOUNT,
                Some(WPARAM(self.shown.len())),
                Some(LPARAM(0)),
            );
        }
        let count = format!("{} 語", self.all.len());
        let text = match self.note {
            Some(n) => format!("+{n}　{count}"),
            None => count,
        };
        unsafe {
            let _ = SetWindowTextW(self.status, &HSTRING::from(text));
            // painted from the message loop: a paint made while this borrow
            // is held finds no rows to draw
            let _ = InvalidateRect(Some(self.list), None, true);
        }
    }

    fn selected(&self) -> Vec<usize> {
        let mut out = Vec::new();
        let mut i = -1isize;
        loop {
            i = unsafe {
                SendMessageW(
                    self.list,
                    LVM_GETNEXTITEM,
                    Some(WPARAM(i as usize)),
                    Some(LPARAM(LVNI_SELECTED as isize)),
                )
            }
            .0;
            if i < 0 {
                return out;
            }
            if let Some(&k) = self.shown.get(i as usize) {
                out.push(k);
            }
        }
    }

    fn select_rows(&self, rows: impl Iterator<Item = usize>) {
        let set = |row: isize, on: bool| {
            let mut item = LVITEMW {
                stateMask: windows::Win32::UI::Controls::LIST_VIEW_ITEM_STATE_FLAGS(
                    LVIS_SELECTED.0 | LVIS_FOCUSED.0,
                ),
                state: if on {
                    LVIS_SELECTED
                } else {
                    Default::default()
                },
                ..Default::default()
            };
            unsafe {
                SendMessageW(
                    self.list,
                    LVM_SETITEMSTATE,
                    Some(WPARAM(row as usize)),
                    Some(LPARAM(&mut item as *mut _ as isize)),
                );
            }
        };
        set(-1, false);
        let mut first = None;
        for r in rows {
            set(r as isize, true);
            first.get_or_insert(r);
        }
        if let Some(r) = first {
            unsafe {
                SendMessageW(
                    self.list,
                    LVM_ENSUREVISIBLE,
                    Some(WPARAM(r)),
                    Some(LPARAM(0)),
                );
            }
        }
        unsafe {
            let _ = InvalidateRect(Some(self.list), None, true);
        }
    }

    /// Apply `f` to the file's words (read afresh) and show the result; a
    /// failure beeps and changes nothing.
    fn change<R>(&mut self, f: impl FnOnce(&mut Vec<Entry>) -> (bool, R)) -> Option<R> {
        match update(&self.file, f) {
            Ok((all, r)) => {
                self.all = all;
                Some(r)
            }
            Err(_) => {
                unsafe {
                    let _ = MessageBeep(MB_ICONWARNING);
                }
                None
            }
        }
    }

    fn delete_selected(&mut self) {
        let keys: HashSet<(String, String)> = self
            .selected()
            .into_iter()
            .map(|i| (self.all[i].reading.clone(), self.all[i].surface.clone()))
            .collect();
        if keys.is_empty() {
            return;
        }
        let gone = self.change(|all| {
            let (gone, kept): (Vec<Entry>, Vec<Entry>) = std::mem::take(all)
                .into_iter()
                .partition(|e| keys.contains(&(e.reading.clone(), e.surface.clone())));
            *all = kept;
            (!gone.is_empty(), gone)
        });
        if let Some(gone) = gone {
            self.removed = gone;
        }
        self.note = None;
        self.refresh();
        self.select_rows(std::iter::empty());
    }

    /// Ctrl+Z: put back what the last Delete removed.
    fn undo(&mut self) {
        let back = std::mem::take(&mut self.removed);
        if back.is_empty() {
            return;
        }
        let keys: HashSet<(String, String)> = back
            .iter()
            .map(|e| (e.reading.clone(), e.surface.clone()))
            .collect();
        self.change(|all| {
            let n = merge(all, back);
            (n > 0, ())
        });
        self.refresh();
        let rows: Vec<usize> = (0..self.shown.len())
            .filter(|&r| {
                let e = &self.all[self.shown[r]];
                keys.contains(&(e.reading.clone(), e.surface.clone()))
            })
            .collect();
        self.select_rows(rows.into_iter());
    }

    /// Import from `source` (`None`: Microsoft IME), then show what came in:
    /// the fields cleared so nothing filters it out, the new rows selected.
    fn import(&mut self, source: Option<&Path>) {
        let Ok(new) = read_source(source) else {
            unsafe {
                let _ = MessageBeep(MB_ICONWARNING);
            }
            return;
        };
        let Some(added) = self.change(|all| {
            let n = merge(all, new);
            (n > 0, n)
        }) else {
            return;
        };
        unsafe {
            let _ = SetWindowTextW(self.surface, w!(""));
            let _ = SetWindowTextW(self.reading, w!(""));
        }
        // the fields empty, the newest `added` words are the top rows
        self.note = Some(added);
        self.refresh();
        self.select_rows(0..added);
        unsafe {
            let _ = SetFocus(Some(self.list));
        }
    }
}

/// The よみ column in grey, so the words stand out without a header (a
/// selected row keeps the highlight's own colour).
fn custom_draw(cd: &mut NMLVCUSTOMDRAW) -> LRESULT {
    let stage = cd.nmcd.dwDrawStage;
    if stage == CDDS_PREPAINT || stage == CDDS_ITEMPREPAINT {
        return LRESULT(CDRF_NOTIFYITEMDRAW as isize);
    }
    if stage.0 == CDDS_ITEMPREPAINT.0 | CDDS_SUBITEM.0 {
        let selected = unsafe {
            SendMessageW(
                cd.nmcd.hdr.hwndFrom,
                LVM_GETITEMSTATE,
                Some(WPARAM(cd.nmcd.dwItemSpec)),
                Some(LPARAM(LVIS_SELECTED.0 as isize)),
            )
        }
        .0 != 0;
        if selected {
            return LRESULT(CDRF_DODEFAULT as isize);
        }
        cd.clrText = COLORREF(unsafe {
            GetSysColor(if cd.iSubItem == 1 {
                COLOR_GRAYTEXT
            } else {
                COLOR_WINDOWTEXT
            })
        });
        return LRESULT(CDRF_NEWFONT as isize);
    }
    LRESULT(CDRF_DODEFAULT as isize)
}

/// The text of row `row`, column `col`, copied into the list's buffer.
fn display_info(info: &mut NMLVDISPINFOW) {
    with_ui(|ui| {
        let item = &mut info.item;
        if !item.mask.contains(LVIF_TEXT) || item.pszText.is_null() || item.cchTextMax <= 0 {
            return;
        }
        let Some(e) = ui
            .shown
            .get(item.iItem as usize)
            .and_then(|&k| ui.all.get(k))
        else {
            return;
        };
        let text = if item.iSubItem == 0 {
            &e.surface
        } else {
            &e.reading
        };
        let wide: Vec<u16> = text.encode_utf16().collect();
        let n = wide.len().min(item.cchTextMax as usize - 1);
        unsafe {
            std::ptr::copy_nonoverlapping(wide.as_ptr(), item.pszText.0, n);
            *item.pszText.0.add(n) = 0;
        }
    });
}

/// 取り込み: a menu under the button with Microsoft IME (its word count,
/// greyed when it has none or is not there) and a file.
fn import_menu(hwnd: HWND) {
    let Some(button) = with_ui(|ui| ui.import) else {
        return;
    };
    let msime_count = msime::read_msime().map(|w| w.len()).unwrap_or(0);
    let choice = unsafe {
        let Ok(menu) = CreatePopupMenu() else {
            return;
        };
        let label = HSTRING::from(format!("Microsoft IME（{msime_count}）"));
        let _ = AppendMenuW(
            menu,
            if msime_count == 0 {
                MF_STRING | MF_GRAYED
            } else {
                MF_STRING
            },
            MENU_MSIME,
            &label,
        );
        let _ = AppendMenuW(menu, MF_STRING, MENU_FILE, w!("ファイル…"));
        let mut r = RECT::default();
        let _ = GetWindowRect(button, &mut r);
        let c = TrackPopupMenu(menu, TPM_RETURNCMD, r.left, r.bottom, None, hwnd, None);
        let _ = DestroyMenu(menu);
        c.0 as usize
    };
    match choice {
        MENU_MSIME => {
            with_ui(|ui| ui.import(None));
        }
        MENU_FILE => {
            if let Some(path) = pick_file(hwnd) {
                with_ui(|ui| ui.import(Some(&path)));
            }
        }
        _ => {}
    }
}

/// The open-file dialog, for an exported dictionary.
fn pick_file(hwnd: HWND) -> Option<PathBuf> {
    unsafe {
        let dlg: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let types = [
            COMDLG_FILTERSPEC {
                pszName: w!("テキスト"),
                pszSpec: w!("*.txt;*.tsv;*.csv"),
            },
            COMDLG_FILTERSPEC {
                pszName: w!("すべて"),
                pszSpec: w!("*.*"),
            },
        ];
        let _ = dlg.SetFileTypes(&types);
        let _ = dlg.SetOptions(dlg.GetOptions().unwrap_or_default() | FOS_FILEMUSTEXIST);
        dlg.Show(Some(hwnd)).ok()?;
        let item = dlg.GetResult().ok()?;
        let p: PWSTR = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = PathBuf::from(p.to_string().ok()?);
        CoTaskMemFree(Some(p.0 as *const _));
        Some(path)
    }
}

/// Files dropped on the window: each imported.
fn dropped(hdrop: HDROP) {
    unsafe {
        let n = DragQueryFileW(hdrop, u32::MAX, None);
        for i in 0..n {
            let len = DragQueryFileW(hdrop, i, None) as usize;
            let mut buf = vec![0u16; len + 1];
            let got = DragQueryFileW(hdrop, i, Some(&mut buf)) as usize;
            let path = PathBuf::from(String::from_utf16_lossy(&buf[..got]));
            with_ui(|ui| ui.import(Some(&path)));
        }
        DragFinish(hdrop);
    }
}

fn register_and_close(hwnd: HWND) {
    let Some((file, r, s)) = with_ui(|ui| (ui.file.clone(), ui.reading, ui.surface)) else {
        return;
    };
    match register(&file, &text_of(r), &text_of(s)) {
        Ok(_) => unsafe {
            let _ = DestroyWindow(hwnd);
        },
        Err(_) => unsafe {
            let _ = MessageBeep(MB_ICONWARNING);
            let _ = SetFocus(Some(if text_of(s).trim().is_empty() { s } else { r }));
        },
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_COMMAND => {
            let id = (wp.0 & 0xffff) as i32;
            let code = ((wp.0 >> 16) & 0xffff) as u32;
            match id {
                _ if id == IDOK.0 => register_and_close(hwnd),
                _ if id == IDCANCEL.0 => unsafe {
                    let _ = DestroyWindow(hwnd);
                },
                ID_SURFACE | ID_READING if code == EN_CHANGE => {
                    with_ui(|ui| {
                        ui.note = None;
                        ui.refresh();
                    });
                }
                ID_IMPORT => import_menu(hwnd),
                _ => {}
            }
            LRESULT(0)
        }
        WM_NOTIFY => {
            let hdr = unsafe { &*(lp.0 as *const NMHDR) };
            if hdr.idFrom == ID_LIST as usize {
                match hdr.code {
                    NM_CUSTOMDRAW => {
                        return custom_draw(unsafe { &mut *(lp.0 as *mut NMLVCUSTOMDRAW) });
                    }
                    LVN_GETDISPINFOW => {
                        display_info(unsafe { &mut *(lp.0 as *mut NMLVDISPINFOW) });
                    }
                    LVN_KEYDOWN => {
                        let key = unsafe { &*(lp.0 as *const NMLVKEYDOWN) }.wVKey;
                        if key == VK_DELETE.0 {
                            with_ui(|ui| ui.delete_selected());
                        }
                    }
                    _ => {}
                }
            }
            LRESULT(0)
        }
        WM_CONTEXTMENU => {
            let on_list = with_ui(|ui| ui.list.0 as usize == wp.0).unwrap_or(false);
            if on_list && with_ui(|ui| !ui.selected().is_empty()).unwrap_or(false) {
                let choice = unsafe {
                    let Ok(menu) = CreatePopupMenu() else {
                        return LRESULT(0);
                    };
                    let _ = AppendMenuW(menu, MF_STRING, MENU_DELETE, w!("削除\tDelete"));
                    let mut pt = POINT::default();
                    let _ = GetCursorPos(&mut pt);
                    let c = TrackPopupMenu(menu, TPM_RETURNCMD, pt.x, pt.y, None, hwnd, None);
                    let _ = DestroyMenu(menu);
                    c.0 as usize
                };
                if choice == MENU_DELETE {
                    with_ui(|ui| ui.delete_selected());
                }
            }
            LRESULT(0)
        }
        WM_DROPFILES => {
            dropped(HDROP(wp.0 as *mut _));
            LRESULT(0)
        }
        WM_CTLCOLORSTATIC => unsafe {
            // the count, quiet grey on the window's own background
            let dc = HDC(wp.0 as *mut _);
            SetBkMode(dc, TRANSPARENT);
            SetTextColor(dc, COLORREF(GetSysColor(COLOR_GRAYTEXT)));
            LRESULT(GetSysColorBrush(COLOR_WINDOW).0 as isize)
        },
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
    let _ = WIDTHS.set(read_widths(install));
    let file = registered_file(install);
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = InitCommonControlsEx(&INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_LISTVIEW_CLASSES,
        });
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
            hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as isize as *mut _),
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
        // layout, in 96-dpi pixels
        const W: i32 = 380;
        const M: i32 = 16; // margin
        const ROW: i32 = 30;
        const GAP: i32 = 10;
        const BTN: i32 = 96;
        const LIST_H: i32 = 260;
        let y_reading = M + ROW + GAP;
        let y_list = y_reading + ROW + GAP + 6;
        let y_bottom = y_list + LIST_H + GAP;
        let h = y_bottom + ROW + M;
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU;
        let mut frame = RECT {
            left: 0,
            top: 0,
            right: px(W),
            bottom: px(h),
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
                     (x, y, w_, h): (i32, i32, i32, i32),
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
        let edit = WINDOW_STYLE(ES_AUTOHSCROLL as u32) | WS_TABSTOP;
        let cue = |h: HWND, text: PCWSTR| {
            SendMessageW(
                h,
                EM_SETCUEBANNER,
                Some(WPARAM(1)),
                Some(LPARAM(text.0 as isize)),
            );
        };
        // 単語 first, then よみ: the order of MS-IME's 単語の登録
        let s = child(
            WS_EX_CLIENTEDGE,
            w!("EDIT"),
            surface,
            edit,
            (M, M, W - 2 * M, ROW),
            ID_SURFACE,
        );
        cue(s, w!("単語"));
        let r = child(
            WS_EX_CLIENTEDGE,
            w!("EDIT"),
            reading,
            edit,
            (M, y_reading, W - 2 * M - BTN - GAP, ROW),
            ID_READING,
        );
        cue(r, w!("よみ"));
        child(
            WINDOW_EX_STYLE(0),
            w!("BUTTON"),
            "登録",
            WINDOW_STYLE(BS_DEFPUSHBUTTON as u32) | WS_TABSTOP,
            (W - M - BTN, y_reading, BTN, ROW),
            IDOK.0,
        );
        let list = child(
            WINDOW_EX_STYLE(0),
            WC_LISTVIEWW,
            "",
            WINDOW_STYLE(LVS_REPORT | LVS_OWNERDATA | LVS_SHOWSELALWAYS | LVS_NOCOLUMNHEADER)
                | WS_TABSTOP,
            (M, y_list, W - 2 * M, LIST_H),
            ID_LIST,
        );
        SendMessageW(
            list,
            LVM_SETEXTENDEDLISTVIEWSTYLE,
            Some(WPARAM(0)),
            Some(LPARAM(
                (LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER) as isize,
            )),
        );
        // no header: the columns are in the fields' order, 単語 then よみ
        let inner = W - 2 * M - 20; // the scroll bar's room
        for (i, (title, width)) in [("単語", inner * 11 / 20), ("よみ", inner * 9 / 20)]
            .into_iter()
            .enumerate()
        {
            let mut t: Vec<u16> = title.encode_utf16().chain([0]).collect();
            let col = LVCOLUMNW {
                mask: LVCF_TEXT | LVCF_WIDTH,
                cx: px(width),
                pszText: PWSTR(t.as_mut_ptr()),
                ..Default::default()
            };
            SendMessageW(
                list,
                LVM_INSERTCOLUMNW,
                Some(WPARAM(i)),
                Some(LPARAM(&col as *const _ as isize)),
            );
        }
        let import = child(
            WINDOW_EX_STYLE(0),
            w!("BUTTON"),
            "取り込み",
            WS_TABSTOP,
            (M, y_bottom, BTN + 12, ROW),
            ID_IMPORT,
        );
        let status = child(
            WINDOW_EX_STYLE(0),
            w!("STATIC"),
            "",
            WINDOW_STYLE(0x2), /* SS_RIGHT */
            (
                M + BTN + 12 + GAP,
                y_bottom + 6,
                W - 2 * M - BTN - 12 - GAP,
                ROW - 6,
            ),
            0,
        );
        DragAcceptFiles(hwnd, true);

        UI.with(|u| {
            *u.borrow_mut() = Some(Ui {
                // unreadable: shown empty, and every change refuses (it reads again)
                all: load(&file).unwrap_or_default(),
                file,
                shown: Vec::new(),
                surface: s,
                reading: r,
                list,
                status,
                import,
                removed: Vec::new(),
                note: None,
            })
        });
        with_ui(|ui| ui.refresh());

        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        // the word to type first; with the word given and no reading, the reading
        let _ = SetFocus(Some(if !surface.is_empty() && reading.is_empty() {
            r
        } else {
            s
        }));

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            // Ctrl+Z on the list puts back what Delete removed (in a field
            // it stays the field's own undo)
            if msg.message == WM_KEYDOWN
                && msg.wParam.0 == b'Z' as usize
                && GetKeyState(VK_CONTROL.0 as i32) < 0
                && with_ui(|ui| ui.list == msg.hwnd).unwrap_or(false)
            {
                with_ui(|ui| ui.undo());
                continue;
            }
            // Tab between the controls, Enter = 登録, Esc = close
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

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("karukan-word-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_word_is_appended_once_with_its_reading_in_hiragana() {
        let dir = temp("reg");
        let file = registered_file(&dir);
        assert_eq!(register(&file, " カルカン ", "karukan"), Ok(true));
        assert_eq!(register(&file, "かるかん", "karukan"), Ok(false));
        assert_eq!(normalize_reading("ｎｉｃｅ２"), "nice2");
        assert_eq!(normalize_reading(" ﾃﾞｰﾀ ヴぁ "), "でーた ゔぁ");
        assert_eq!(normalize_reading("ﾊﾟｿｺﾝ!？"), "ぱそこん！？");
        let typed_half = Widths {
            half_digits: false,
            half_marks: true,
        };
        assert_eq!(
            normalize_reading_with("ﾊﾟｿｺﾝ!？1", typed_half),
            "ぱそこん!?１"
        );
        let dir = temp("cfg");
        std::fs::create_dir_all(dir.join("config")).unwrap();
        std::fs::write(
            dir.join("config").join("config.toml"),
            "[conversion]\nascii_symbol = \"half\"\n[width]\n# x\nascii_symbol = \"half\" # c\n",
        )
        .unwrap();
        assert_eq!(
            read_widths(&dir),
            Widths {
                half_digits: true,
                half_marks: true
            }
        );
        assert_eq!(read_widths(&dir.join("none")), SHIPPED);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(register(&file, "てすと", "テスト語"), Ok(true));
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "かるかん\tkarukan\t名詞\t\nてすと\tテスト語\t名詞\t\n"
        );
        assert!(register(&file, "", "x").is_err());
        assert!(register(&file, "あ", "a\tb").is_err());
        // nothing but the dictionary in the folder the servers load
        let names: Vec<_> = std::fs::read_dir(file.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["registered.txt"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_that_cannot_be_read_is_never_written_over() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = temp("lock");
        let file = registered_file(&dir);
        register(&file, "かるかん", "karukan").unwrap();
        let before = std::fs::read(&file).unwrap();
        // held open with no sharing: reading it fails
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&file)
            .unwrap();
        assert!(register(&file, "てすと", "テスト語").is_err());
        drop(lock);
        assert_eq!(std::fs::read(&file).unwrap(), before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_import_adds_only_new_words_and_keeps_their_columns() {
        let dir = temp("imp");
        let file = registered_file(&dir);
        register(&file, "かるかん", "karukan").unwrap();
        let export = dir.join("export.txt");
        let text = "!Microsoft IME Dictionary Tool\r\nかるかん\tkarukan\t名詞\r\nかぶ\t株式会社\t短縮よみ\r\n";
        let mut bytes = vec![0xff, 0xfe];
        bytes.extend(text.encode_utf16().flat_map(|u| u.to_le_bytes()));
        std::fs::write(&export, bytes).unwrap();
        assert_eq!(import(&file, Some(&export)), Ok(1));
        assert_eq!(import(&file, Some(&export)), Ok(0));
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "かるかん\tkarukan\t名詞\t\nかぶ\t株式会社\t短縮よみ\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
