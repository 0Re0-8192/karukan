//! The input-mode button in the taskbar (GUID_LBI_INPUTMODE): あ / A, a left
//! click toggles the IME like 半角/全角, a right click opens the menu with
//! 単語の登録 — where MS-IME users go to register a word.

use std::cell::{Cell, RefCell};

use windows::Win32::Foundation::COLORREF;
use windows::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL, HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateBitmap,
    CreateCompatibleDC, CreateDIBSection, CreateFontW, DEFAULT_CHARSET, DIB_RGB_COLORS, DT_CENTER,
    DT_SINGLELINE, DT_VCENTER, DeleteDC, DeleteObject, DrawTextW, FF_DONTCARE, FW_BOLD, HBITMAP,
    HGDIOBJ, OUT_DEFAULT_PRECIS, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::Win32::UI::Input::KeyboardAndMouse::GetFocus;
use windows::Win32::UI::TextServices::{
    GUID_LBI_INPUTMODE, ITfLangBarItem_Impl, ITfLangBarItemButton, ITfLangBarItemButton_Impl,
    ITfLangBarItemSink, ITfMenu, ITfSource, ITfSource_Impl, TF_LANGBARITEMINFO, TF_LBI_CLK_LEFT,
    TF_LBI_CLK_RIGHT, TF_LBI_ICON, TF_LBI_STATUS, TF_LBI_STYLE_BTN_BUTTON,
    TF_LBI_STYLE_SHOWNINTRAY, TF_LBI_TEXT, TF_LBI_TOOLTIP, TfLBIClick,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconIndirect, CreatePopupMenu, DestroyMenu, GetSystemMetrics, HICON,
    ICONINFO, MF_STRING, SM_CXSMICON, TPM_BOTTOMALIGN, TPM_LEFTALIGN, TPM_LEFTBUTTON, TPM_NONOTIFY,
    TPM_RETURNCMD, TPM_RIGHTALIGN, TPM_TOPALIGN, TPM_VERTICAL, TPMPARAMS, TRACK_POPUP_MENU_FLAGS,
    TrackPopupMenuEx,
};
use windows::core::{BSTR, GUID, IUnknown, Interface, Ref, Result, implement, w};

use windows::Win32::UI::WindowsAndMessaging::HMENU;

use crate::guids::CLSID_KARUKAN;

/// Menu item ids.
const ID_WORD: u32 = 1;

/// What the button asks of the text service.
pub struct Hooks {
    pub is_open: Box<dyn Fn() -> bool>,
    pub toggle: Box<dyn Fn()>,
    pub register_word: Box<dyn Fn()>,
    /// A window of the app's thread the popup menu can belong to
    pub owner: Box<dyn Fn() -> Option<windows::Win32::Foundation::HWND>>,
}

#[implement(ITfLangBarItemButton, ITfSource)]
pub struct ModeButton {
    hooks: Hooks,
    sink: RefCell<Option<ITfLangBarItemSink>>,
    cookie: Cell<u32>,
}

impl ModeButton {
    pub fn new(hooks: Hooks) -> Self {
        ModeButton {
            hooks,
            sink: RefCell::new(None),
            cookie: Cell::new(0),
        }
    }
}

/// Tell the taskbar the mode changed (icon and text).
pub fn notify(button: &windows::core::ComObject<ModeButton>) {
    let sink = button.sink.borrow().clone();
    let open = (button.hooks.is_open)();
    match sink {
        Some(sink) => {
            let r = unsafe {
                sink.OnUpdate(TF_LBI_ICON | TF_LBI_STATUS | TF_LBI_TEXT | TF_LBI_TOOLTIP)
            };
            log(&format!("notify open={open} OnUpdate={r:?}"));
        }
        None => log(&format!("notify open={open}: no sink advised")),
    }
}

/// A line in debug.log (while debug.flag exists), with the thread: the
/// taskbar calls into the app's thread, and which one is the question.
fn log(msg: &str) {
    let tid = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
    crate::tip::diag(&format!("langbar tid={tid} {msg}"));
}

fn text(open: bool) -> &'static str {
    if open { "あ" } else { "A" }
}

/// The taskbar's own theme (not the apps'): light text on a dark taskbar.
fn taskbar_is_dark() -> bool {
    let mut data = 0u32;
    let mut size = 4u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("SystemUsesLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    rc.is_err() || data == 0
}

/// あ / A drawn as a small icon, in the taskbar's text colour.
fn draw_icon(label: &str) -> Result<HICON> {
    unsafe {
        let n = GetSystemMetrics(SM_CXSMICON).max(16);
        let bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: n,
                biHeight: -n,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let dc = CreateCompatibleDC(None);
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let color = CreateDIBSection(Some(dc), &bi, DIB_RGB_COLORS, &mut bits, None, 0)?;
        let old = SelectObject(dc, HGDIOBJ(color.0));
        let font = CreateFontW(
            -(n * 7 / 8),
            0,
            0,
            0,
            FW_BOLD.0 as i32,
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
        let old_font = SelectObject(dc, HGDIOBJ(font.0));
        // white on black: the luminance becomes the alpha below
        SetBkMode(dc, TRANSPARENT);
        SetTextColor(dc, COLORREF(0x00ff_ffff));
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: n,
            bottom: n,
        };
        let mut label: Vec<u16> = label.encode_utf16().collect();
        DrawTextW(
            dc,
            &mut label,
            &mut rect,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
        );
        SelectObject(dc, old_font);
        let _ = DeleteObject(HGDIOBJ(font.0));
        SelectObject(dc, old);
        let _ = DeleteDC(dc);
        let fg: u32 = if taskbar_is_dark() { 0xff } else { 0x00 };
        let px = std::slice::from_raw_parts_mut(bits as *mut u32, (n * n) as usize);
        for p in px.iter_mut() {
            let a = (*p & 0xff).max((*p >> 8) & 0xff).max((*p >> 16) & 0xff);
            // premultiplied
            let c = fg * a / 255;
            *p = (a << 24) | (c << 16) | (c << 8) | c;
        }
        let mask = CreateBitmap(n, n, 1, 1, None);
        let info = ICONINFO {
            fIcon: true.into(),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: color,
        };
        let icon = CreateIconIndirect(&info);
        let _ = DeleteObject(HGDIOBJ(mask.0));
        let _ = DeleteObject(HGDIOBJ(color.0));
        icon
    }
}

/// Where the menu goes: beside the button, on the side away from the
/// taskbar's edge (right of a taskbar on the left, above one at the bottom),
/// the button itself excluded. The click point alone does not say where the
/// button is: on a vertical taskbar it came out well below it.
fn menu_anchor(
    area: Option<RECT>,
    pt: POINT,
    work: Option<RECT>,
) -> (i32, i32, TRACK_POPUP_MENU_FLAGS) {
    let Some(r) = area else {
        let x = work.map_or(pt.x, |w| pt.x.clamp(w.left, w.right));
        return (x, pt.y, TPM_LEFTALIGN | TPM_TOPALIGN);
    };
    match work {
        Some(w) if r.right <= w.left => (r.right, r.top, TPM_LEFTALIGN | TPM_TOPALIGN),
        Some(w) if r.left >= w.right => (r.left, r.top, TPM_RIGHTALIGN | TPM_TOPALIGN),
        Some(w) if r.bottom <= w.top => (r.left, r.bottom, TPM_LEFTALIGN | TPM_TOPALIGN),
        _ => (
            r.left,
            r.top,
            TPM_LEFTALIGN | TPM_BOTTOMALIGN | TPM_VERTICAL,
        ),
    }
}

/// The right-click menu drawn dark: owner-drawn items on a dark background,
/// owned by a hidden window of ours that draws them.
mod dark_menu {
    use std::cell::Cell;

    use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, SIZE, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        CreateFontIndirectW, CreateSolidBrush, DT_LEFT, DT_SINGLELINE, DT_VCENTER, DeleteObject,
        DrawTextW, FillRect, GetDC, GetTextExtentPoint32W, HFONT, HGDIOBJ, ReleaseDC, SelectObject,
        SetBkMode, SetTextColor, TRANSPARENT,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Controls::{DRAWITEMSTRUCT, MEASUREITEMSTRUCT, ODS_SELECTED, ODT_MENU};
    use windows::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, HMENU, MENUINFO,
        MF_OWNERDRAW, MIM_BACKGROUND, NONCLIENTMETRICSW, RegisterClassExW, SPI_GETNONCLIENTMETRICS,
        SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SetMenuInfo, SystemParametersInfoW, WM_DRAWITEM,
        WM_MEASUREITEM, WNDCLASSEXW, WS_EX_TOOLWINDOW, WS_POPUP,
    };
    use windows::core::PCWSTR;

    use super::ID_WORD;

    const BG: u32 = 0x2b_2b2b;
    const HOT: u32 = 0x41_4141;
    const TEXT: u32 = 0xf0_f0f0;
    const LABEL: &str = "単語の登録";

    thread_local! {
        static OWNER: Cell<Option<HWND>> = const { Cell::new(None) };
        static FONT: Cell<Option<HFONT>> = const { Cell::new(None) };
    }

    /// The menu font of the system, made once.
    fn font() -> HFONT {
        FONT.with(|f| {
            if let Some(h) = f.get() {
                return h;
            }
            let mut m = NONCLIENTMETRICSW {
                cbSize: std::mem::size_of::<NONCLIENTMETRICSW>() as u32,
                ..Default::default()
            };
            let h = unsafe {
                let _ = SystemParametersInfoW(
                    SPI_GETNONCLIENTMETRICS,
                    m.cbSize,
                    Some(&mut m as *mut _ as *mut _),
                    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
                );
                CreateFontIndirectW(&m.lfMenuFont)
            };
            f.set(Some(h));
            h
        })
    }

    fn label() -> Vec<u16> {
        LABEL.encode_utf16().collect()
    }

    unsafe extern "system" fn proc_(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        unsafe {
            match msg {
                WM_MEASUREITEM => {
                    let mi = &mut *(lp.0 as *mut MEASUREITEMSTRUCT);
                    if mi.CtlType != ODT_MENU {
                        return DefWindowProcW(hwnd, msg, wp, lp);
                    }
                    let dc = GetDC(Some(hwnd));
                    let old = SelectObject(dc, HGDIOBJ(font().0));
                    let mut sz = SIZE::default();
                    let _ = GetTextExtentPoint32W(dc, &label(), &mut sz);
                    SelectObject(dc, old);
                    ReleaseDC(Some(hwnd), dc);
                    // the room a themed menu gives a line: padded both ways
                    mi.itemWidth = (sz.cx + sz.cy * 2) as u32;
                    mi.itemHeight = (sz.cy * 2) as u32;
                    LRESULT(1)
                }
                WM_DRAWITEM => {
                    let di = &*(lp.0 as *const DRAWITEMSTRUCT);
                    if di.CtlType != ODT_MENU {
                        return DefWindowProcW(hwnd, msg, wp, lp);
                    }
                    let hot = (di.itemState.0 & ODS_SELECTED.0) != 0;
                    let b = CreateSolidBrush(COLORREF(if hot { HOT } else { BG }));
                    FillRect(di.hDC, &di.rcItem, b);
                    let _ = DeleteObject(HGDIOBJ(b.0));
                    let old = SelectObject(di.hDC, HGDIOBJ(font().0));
                    SetBkMode(di.hDC, TRANSPARENT);
                    SetTextColor(di.hDC, COLORREF(TEXT));
                    let h = di.rcItem.bottom - di.rcItem.top;
                    let mut r = RECT {
                        left: di.rcItem.left + h / 2,
                        ..di.rcItem
                    };
                    let mut text = label();
                    DrawTextW(
                        di.hDC,
                        &mut text,
                        &mut r,
                        DT_LEFT | DT_SINGLELINE | DT_VCENTER,
                    );
                    SelectObject(di.hDC, old);
                    LRESULT(1)
                }
                _ => DefWindowProcW(hwnd, msg, wp, lp),
            }
        }
    }

    /// The hidden window the menu belongs to, made once per thread.
    fn owner() -> Option<HWND> {
        if let Some(h) = OWNER.with(|o| o.get()) {
            return Some(h);
        }
        unsafe {
            let hinst = GetModuleHandleW(None).ok()?;
            // one class per loaded copy of this code (an app can hold two)
            let name: Vec<u16> = format!("KarukanMenuOwner_{:x}", proc_ as *const () as usize)
                .encode_utf16()
                .chain(Some(0))
                .collect();
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(proc_),
                hInstance: hinst.into(),
                lpszClassName: PCWSTR(name.as_ptr()),
                ..Default::default()
            };
            RegisterClassExW(&wc);
            let h = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                PCWSTR(name.as_ptr()),
                PCWSTR::null(),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(hinst.into()),
                None,
            )
            .ok()?;
            OWNER.with(|o| o.set(Some(h)));
            Some(h)
        }
    }

    /// The menu and the window it belongs to.
    pub fn build() -> Option<(HMENU, HWND)> {
        let own = owner()?;
        unsafe {
            let menu = CreatePopupMenu().ok()?;
            let info = MENUINFO {
                cbSize: std::mem::size_of::<MENUINFO>() as u32,
                fMask: MIM_BACKGROUND,
                hbrBack: CreateSolidBrush(COLORREF(BG)),
                ..Default::default()
            };
            let _ = SetMenuInfo(menu, &info);
            let _ = AppendMenuW(menu, MF_OWNERDRAW, ID_WORD as usize, PCWSTR::null());
            Some((menu, own))
        }
    }
}

impl ITfLangBarItem_Impl for ModeButton_Impl {
    fn GetInfo(&self, pinfo: *mut TF_LANGBARITEMINFO) -> Result<()> {
        if pinfo.is_null() {
            return Err(E_INVALIDARG.into());
        }
        let mut desc = [0u16; 32];
        for (d, c) in desc.iter_mut().zip("karukan".encode_utf16()) {
            *d = c;
        }
        unsafe {
            *pinfo = TF_LANGBARITEMINFO {
                clsidService: CLSID_KARUKAN,
                guidItem: GUID_LBI_INPUTMODE,
                dwStyle: TF_LBI_STYLE_BTN_BUTTON | TF_LBI_STYLE_SHOWNINTRAY,
                ulSort: 0,
                szDescription: desc,
            };
        }
        Ok(())
    }
    fn GetStatus(&self) -> Result<u32> {
        Ok(0)
    }
    fn Show(&self, _fshow: windows::core::BOOL) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn GetTooltipString(&self) -> Result<BSTR> {
        Ok(BSTR::from(format!(
            "karukan ({})",
            text((self.hooks.is_open)())
        )))
    }
}

impl ITfLangBarItemButton_Impl for ModeButton_Impl {
    fn OnClick(&self, click: TfLBIClick, pt: &POINT, prcarea: *const RECT) -> Result<()> {
        log(&format!(
            "OnClick {:?} at ({}, {}) open={}",
            click.0,
            pt.x,
            pt.y,
            (self.hooks.is_open)()
        ));
        if click == TF_LBI_CLK_LEFT {
            (self.hooks.toggle)();
        } else if click == TF_LBI_CLK_RIGHT {
            unsafe {
                // as Mozc does: the menu belongs to the app's focused window,
                // ours only when there is none, and stays inside the work area
                let app_owner = Some(GetFocus())
                    .filter(|h| !h.is_invalid())
                    .or_else(|| (self.hooks.owner)());
                let area = prcarea.as_ref().copied().filter(|r| r.right > r.left);
                let mon = MonitorFromPoint(*pt, MONITOR_DEFAULTTONEAREST);
                let mut mi = MONITORINFO {
                    cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                    ..Default::default()
                };
                let work = GetMonitorInfoW(mon, &mut mi).as_bool().then_some(mi.rcWork);
                let (x, y, align) = menu_anchor(area, *pt, work);
                let params = TPMPARAMS {
                    cbSize: std::mem::size_of::<TPMPARAMS>() as u32,
                    rcExclude: area.unwrap_or_default(),
                };
                let track = |menu: HMENU, owner: Option<HWND>| {
                    TrackPopupMenuEx(
                        menu,
                        (TPM_NONOTIFY | TPM_RETURNCMD | TPM_LEFTBUTTON | align).0,
                        x,
                        y,
                        owner.unwrap_or_default(),
                        area.map(|_| &params as *const TPMPARAMS),
                    )
                };
                // dark like the taskbar's own menus when the taskbar is dark:
                // drawn by our own window, which the menu then belongs to (the
                // app's window is left alone)
                let mut picked = windows::core::BOOL(0);
                let mut owner = app_owner;
                let mut shown = false;
                if taskbar_is_dark()
                    && let Some((menu, own)) = dark_menu::build()
                {
                    let t0 = std::time::Instant::now();
                    picked = track(menu, Some(own));
                    let _ = DestroyMenu(menu);
                    // nothing comes back at once only when it did not open
                    shown = picked.0 != 0 || t0.elapsed() > std::time::Duration::from_millis(150);
                    owner = Some(own);
                    if !shown {
                        log("dark menu did not open: the plain one");
                    }
                }
                if !shown {
                    let menu = CreatePopupMenu()?;
                    let _ = AppendMenuW(menu, MF_STRING, ID_WORD as usize, w!("単語の登録"));
                    owner = app_owner;
                    picked = track(menu, owner);
                    let _ = DestroyMenu(menu);
                }
                log(&format!(
                    "menu area={area:?} work={work:?} at ({x}, {y}) align={:#x}",
                    align.0
                ));
                let err = windows::Win32::Foundation::GetLastError();
                log(&format!(
                    "menu owner={:?} picked={} err={err:?}",
                    owner.map(|h| h.0),
                    picked.0
                ));
                if picked.0 as u32 == ID_WORD {
                    (self.hooks.register_word)();
                }
            }
        }
        Ok(())
    }
    fn InitMenu(&self, pmenu: Ref<ITfMenu>) -> Result<()> {
        log("InitMenu");
        let menu = pmenu.ok()?;
        let label: Vec<u16> = "単語の登録".encode_utf16().collect();
        unsafe {
            menu.AddMenuItem(
                ID_WORD,
                0,
                HBITMAP::default(),
                HBITMAP::default(),
                &label,
                std::ptr::null_mut(),
            )
        }
    }
    fn OnMenuSelect(&self, wid: u32) -> Result<()> {
        if wid == ID_WORD {
            (self.hooks.register_word)();
        }
        Ok(())
    }
    fn GetIcon(&self) -> Result<HICON> {
        let open = (self.hooks.is_open)();
        let icon = draw_icon(text(open));
        log(&format!("GetIcon open={open} ok={}", icon.is_ok()));
        icon
    }
    fn GetText(&self) -> Result<BSTR> {
        Ok(BSTR::from(text((self.hooks.is_open)())))
    }
}

impl ITfSource_Impl for ModeButton_Impl {
    fn AdviseSink(&self, riid: *const GUID, punk: Ref<IUnknown>) -> Result<u32> {
        if riid.is_null() || unsafe { *riid } != ITfLangBarItemSink::IID {
            return Err(E_INVALIDARG.into());
        }
        let sink: ITfLangBarItemSink = punk.ok()?.cast()?;
        log("AdviseSink");
        *self.sink.borrow_mut() = Some(sink);
        self.cookie.set(self.cookie.get().wrapping_add(1).max(1));
        Ok(self.cookie.get())
    }
    fn UnadviseSink(&self, dwcookie: u32) -> Result<()> {
        log("UnadviseSink");
        if dwcookie == self.cookie.get() {
            *self.sink.borrow_mut() = None;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Graphics::Gdi::{GetDC, GetDIBits, ReleaseDC};
    use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo};

    /// The icon's pixels (BGRA, top-down) and side.
    fn pixels(icon: HICON) -> (i32, Vec<u8>) {
        unsafe {
            let mut info = ICONINFO::default();
            GetIconInfo(icon, &mut info).unwrap();
            let n = GetSystemMetrics(SM_CXSMICON).max(16);
            let mut bi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: n,
                    biHeight: -n,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut buf = vec![0u8; (n * n * 4) as usize];
            let dc = GetDC(None);
            GetDIBits(
                dc,
                info.hbmColor,
                0,
                n as u32,
                Some(buf.as_mut_ptr() as *mut _),
                &mut bi,
                DIB_RGB_COLORS,
            );
            ReleaseDC(None, dc);
            let _ = DeleteObject(HGDIOBJ(info.hbmColor.0));
            let _ = DeleteObject(HGDIOBJ(info.hbmMask.0));
            (n, buf)
        }
    }

    #[test]
    fn the_menu_sits_beside_the_button_away_from_the_edge() {
        let rect = |l, t, r, b| RECT {
            left: l,
            top: t,
            right: r,
            bottom: b,
        };
        let pt = POINT { x: 18, y: 1284 };
        // taskbar on the left: right of the button, top-aligned with it
        let (x, y, a) = menu_anchor(
            Some(rect(0, 1230, 72, 1262)),
            pt,
            Some(rect(72, 0, 5120, 1440)),
        );
        assert_eq!((x, y, a), (72, 1230, TPM_LEFTALIGN | TPM_TOPALIGN));
        // at the bottom: above the button
        let (x, y, a) = menu_anchor(
            Some(rect(4800, 1392, 4840, 1440)),
            pt,
            Some(rect(0, 0, 5120, 1392)),
        );
        assert_eq!(
            (x, y, a),
            (4800, 1392, TPM_LEFTALIGN | TPM_BOTTOMALIGN | TPM_VERTICAL)
        );
        // no rectangle: the click point, kept in the work area
        assert_eq!(menu_anchor(None, pt, Some(rect(72, 0, 5120, 1440))).0, 72);
    }

    #[test]
    fn the_icon_draws_its_letter() {
        for label in ["あ", "A"] {
            let icon = draw_icon(label).unwrap();
            let (n, buf) = pixels(icon);
            let opaque = buf.chunks(4).filter(|p| p[3] > 128).count();
            // a letter: some pixels drawn, most of the square left clear
            assert!(
                opaque > 5 && opaque < (n * n / 2) as usize,
                "{label}: {opaque}"
            );
            if let Some(dir) = std::env::var_os("KARUKAN_PREVIEW_DIR") {
                let mut out = (n as u32).to_le_bytes().to_vec();
                out.extend((n as u32).to_le_bytes());
                out.extend(&buf);
                let name = if label == "A" {
                    "mode_a.raw"
                } else {
                    "mode_hira.raw"
                };
                std::fs::write(std::path::Path::new(&dir).join(name), out).unwrap();
            }
            unsafe {
                let _ = DestroyIcon(icon);
            }
        }
    }
}
