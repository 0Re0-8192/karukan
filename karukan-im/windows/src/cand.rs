//! Candidate window: a topmost, non-activating popup drawn with GDI into a
//! memory bitmap. Follows the system light/dark theme, rounds its corners
//! and adds a shadow on Windows 11 (MS-IME look). The engine pre-paginates,
//! so this only renders the current page.

use std::cell::RefCell;

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateCompatibleBitmap,
    CreateCompatibleDC, CreateFontW, CreateSolidBrush, DEFAULT_CHARSET, DEFAULT_PITCH,
    DT_END_ELLIPSIS, DT_NOPREFIX, DT_RIGHT, DT_SINGLELINE, DT_VCENTER, DeleteDC, DeleteObject,
    DrawTextW, EndPaint, FF_DONTCARE, FW_NORMAL, FillRect, GetDC, GetMonitorInfoW, GetStockObject,
    GetTextExtentPoint32W, HDC, HFONT, HGDIOBJ, InvalidateRect, MONITOR_DEFAULTTONEAREST,
    MONITORINFO, MonitorFromPoint, NULL_PEN, OUT_DEFAULT_PRECIS, PAINTSTRUCT, ReleaseDC, RoundRect,
    SRCCOPY, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetWindowRect, HWND_TOPMOST,
    IsWindowVisible, MA_NOACTIVATE, RegisterClassExW, SW_HIDE, SWP_NOACTIVATE, SWP_SHOWWINDOW,
    SetWindowPos, ShowWindow, WM_ERASEBKGND, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_PAINT,
    WNDCLASS_STYLES, WNDCLASSEXW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{PCWSTR, w};

use crate::engine::CandidateItem;

const CS_DROPSHADOW: u32 = 0x0002_0000;

struct Palette {
    bg: u32,
    sel: u32,
    text: u32,
    sub: u32,
    accent: u32,
    border: u32,
}

const DARK: Palette = Palette {
    bg: 0x002C_2C2C,
    sel: 0x003D_3D3D,
    text: 0x00FF_FFFF,
    sub: 0x009D_9D9D,
    accent: 0x00FF_CD60, // BGR of #60CDFF
    border: 0x0045_4545,
};

const LIGHT: Palette = Palette {
    bg: 0x00F9_F9F9,
    sel: 0x00E5_E5E5,
    text: 0x001A_1A1A,
    sub: 0x0070_7070,
    accent: 0x00C0_6700, // BGR of #0067C0
    border: 0x00D0_D0D0,
};

#[derive(Default)]
struct View {
    rows: Vec<String>,
    desc: Vec<String>,
    cursor: usize,
    aux: Option<String>,
    /// The keys typed so far (`kyarameru`), shown as a header row of its own
    typed: Option<String>,
    /// `typed` as drawn: the tail that fits the window, `…` in front of a cut one
    typed_shown: String,
    typed_h: i32,
    /// The window sits above the composition: the typed keys go at the bottom, next to the text
    flipped: bool,
    /// The last `show` arguments, so a change to the typed keys or the aux
    /// line can lay the window out again without a new candidate list
    last: Option<(Vec<CandidateItem>, usize, usize, usize)>,
    page: Option<(usize, usize)>, // (current 1-based, total)
    dark: bool,
    scale: f32,
    // layout, in device pixels
    pad: i32,
    row_h: i32,
    num_w: i32,
    text_w: i32,
    desc_w: i32,
    footer_h: i32,
    width: i32,
    font: Option<isize>,
    font_small: Option<isize>,
    font_scale: f32,
}

thread_local! {
    static VIEW: RefCell<View> = RefCell::new(View::default());
    static HWND_CAND: RefCell<Option<HWND>> = const { RefCell::new(None) };
    /// Where the composition started; the window is re-placed against it on
    /// every update so a size change (flipped above the text) stays attached.
    static LAST_ANCHOR: std::cell::Cell<Option<Anchor>> = const { std::cell::Cell::new(None) };
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

fn px(base: i32, scale: f32) -> i32 {
    (base as f32 * scale).round() as i32
}

fn system_is_dark() -> bool {
    let mut data = 1u32;
    let mut size = 4u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    rc.is_ok() && data == 0
}

fn make_font(height: i32) -> HFONT {
    unsafe {
        CreateFontW(
            -height,
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
            (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
            w!("Yu Gothic UI"),
        )
    }
}

/// (Re)create the fonts when the DPI scale changed.
fn ensure_fonts(scale: f32) -> (HFONT, HFONT) {
    VIEW.with(|v| {
        let mut v = v.borrow_mut();
        if v.font.is_none() || (v.font_scale - scale).abs() > 0.01 {
            unsafe {
                if let Some(f) = v.font.take() {
                    let _ = DeleteObject(HGDIOBJ(f as *mut _));
                }
                if let Some(f) = v.font_small.take() {
                    let _ = DeleteObject(HGDIOBJ(f as *mut _));
                }
            }
            v.font = Some(make_font(px(17, scale)).0 as isize);
            v.font_small = Some(make_font(px(13, scale)).0 as isize);
            v.font_scale = scale;
        }
        (
            HFONT(v.font.unwrap() as *mut _),
            HFONT(v.font_small.unwrap() as *mut _),
        )
    })
}

/// What a click on the candidate row at an index on the page does.
pub type OnPick = Box<dyn Fn(usize)>;

thread_local! {
    /// Set by the text service.
    static ON_PICK: RefCell<Option<OnPick>> = const { RefCell::new(None) };
}

/// Have a click on the row at `index` on the page call `f`; `None` stops it.
pub fn set_on_pick(f: Option<OnPick>) {
    ON_PICK.with(|p| *p.borrow_mut() = f);
}

/// The candidate row at client `y`, if any (the same layout `paint` draws).
fn row_at(y: i32) -> Option<usize> {
    VIEW.with(|v| {
        let v = v.borrow();
        if v.row_h <= 0 {
            return None;
        }
        let rows_top = px(4, v.scale) + if v.flipped { 0 } else { v.typed_h };
        let i = (y - rows_top).div_euclid(v.row_h);
        (y >= rows_top && (i as usize) < v.rows.len()).then_some(i as usize)
    })
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_LBUTTONUP => {
            // the window never takes focus, so the app keeps typing into the field
            let y = ((lp.0 >> 16) & 0xffff) as i16 as i32;
            if let Some(i) = row_at(y) {
                // released before calling out: the pick re-renders this window
                let pick = ON_PICK.with(|p| p.borrow_mut().take());
                if let Some(f) = pick {
                    f(i);
                    ON_PICK.with(|p| {
                        let mut slot = p.borrow_mut();
                        if slot.is_none() {
                            *slot = Some(f);
                        }
                    });
                }
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            unsafe { paint(hwnd) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

unsafe fn fill(hdc: HDC, rect: &RECT, color: u32) {
    unsafe {
        let b = CreateSolidBrush(COLORREF(color));
        FillRect(hdc, rect, b);
        let _ = DeleteObject(HGDIOBJ(b.0));
    }
}

unsafe fn draw_text(hdc: HDC, text: &str, rect: &mut RECT, flags: u32, color: u32) {
    unsafe {
        SetTextColor(hdc, COLORREF(color));
        let mut ws = wide(text);
        DrawTextW(
            hdc,
            &mut ws,
            rect,
            windows::Win32::Graphics::Gdi::DRAW_TEXT_FORMAT(flags),
        );
    }
}

unsafe fn paint(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc_win = unsafe { BeginPaint(hwnd, &mut ps) };
    let mut client = RECT::default();
    let _ = unsafe { GetClientRect(hwnd, &mut client) };
    let (cw, ch) = (client.right - client.left, client.bottom - client.top);

    let mem = unsafe { CreateCompatibleDC(Some(hdc_win)) };
    let bmp = unsafe { CreateCompatibleBitmap(hdc_win, cw, ch) };
    let old_bmp = unsafe { SelectObject(mem, HGDIOBJ(bmp.0)) };
    unsafe { SetBkMode(mem, TRANSPARENT) };

    draw(mem, cw, ch);
    let _ = unsafe { BitBlt(hdc_win, 0, 0, cw, ch, Some(mem), 0, 0, SRCCOPY) };

    unsafe {
        SelectObject(mem, old_bmp);
        let _ = DeleteObject(HGDIOBJ(bmp.0));
        let _ = DeleteDC(mem);
        let _ = EndPaint(hwnd, &ps);
    }
}

fn draw(mem: HDC, cw: i32, ch: i32) {
    VIEW.with(|v| {
        let v = v.borrow();
        let pal = if v.dark { &DARK } else { &LIGHT };
        let font = HFONT(v.font.unwrap_or(0) as *mut _);
        let small = HFONT(v.font_small.unwrap_or(0) as *mut _);
        unsafe {
            SetBkMode(mem, TRANSPARENT);
            fill(
                mem,
                &RECT {
                    left: 0,
                    top: 0,
                    right: cw,
                    bottom: ch,
                },
                pal.bg,
            );
            let old_font = SelectObject(mem, HGDIOBJ(font.0));
            let top0 = px(4, v.scale);
            let rows_top = top0 + if v.flipped { 0 } else { v.typed_h };
            if v.typed_h > 0 {
                // the typed keys, a row of their own on the side nearest the text:
                // above the candidates, or below them when the window is flipped above the text
                let ty = if v.flipped {
                    rows_top + v.rows.len() as i32 * v.row_h + v.footer_h
                } else {
                    top0
                };
                let mut r = RECT {
                    left: v.pad,
                    top: ty,
                    right: cw - v.pad,
                    bottom: ty + v.typed_h,
                };
                let flags = (DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX).0;
                draw_text(mem, &v.typed_shown, &mut r, flags, pal.accent);
                let ly = if v.flipped { ty } else { ty + v.typed_h - 1 };
                fill(
                    mem,
                    &RECT {
                        left: px(4, v.scale),
                        top: ly,
                        right: cw - px(4, v.scale),
                        bottom: ly + 1,
                    },
                    pal.border,
                );
            }
            let top0 = rows_top;
            for (i, row) in v.rows.iter().enumerate() {
                let top = top0 + i as i32 * v.row_h;
                let band = RECT {
                    left: px(4, v.scale),
                    top: top + px(1, v.scale),
                    right: cw - px(4, v.scale),
                    bottom: top + v.row_h - px(1, v.scale),
                };
                if i == v.cursor {
                    let brush = CreateSolidBrush(COLORREF(pal.sel));
                    let old_b = SelectObject(mem, HGDIOBJ(brush.0));
                    let old_p = SelectObject(mem, GetStockObject(NULL_PEN));
                    let r = px(8, v.scale);
                    let _ = RoundRect(mem, band.left, band.top, band.right, band.bottom, r, r);
                    SelectObject(mem, old_p);
                    SelectObject(mem, old_b);
                    let _ = DeleteObject(HGDIOBJ(brush.0));
                    // accent bar
                    let bar_h = (band.bottom - band.top) * 6 / 10;
                    let bar_top = band.top + ((band.bottom - band.top) - bar_h) / 2;
                    let bar = CreateSolidBrush(COLORREF(pal.accent));
                    let old_b = SelectObject(mem, HGDIOBJ(bar.0));
                    let old_p = SelectObject(mem, GetStockObject(NULL_PEN));
                    let bw = px(3, v.scale);
                    let _ = RoundRect(
                        mem,
                        band.left + px(3, v.scale),
                        bar_top,
                        band.left + px(3, v.scale) + bw + 1,
                        bar_top + bar_h,
                        bw,
                        bw,
                    );
                    SelectObject(mem, old_p);
                    SelectObject(mem, old_b);
                    let _ = DeleteObject(HGDIOBJ(bar.0));
                }
                let flags = (DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX).0;
                let mut num_r = RECT {
                    left: v.pad,
                    top,
                    right: v.pad + v.num_w,
                    bottom: top + v.row_h,
                };
                draw_text(mem, &format!("{}", i + 1), &mut num_r, flags, pal.sub);
                let mut text_r = RECT {
                    left: v.pad + v.num_w + px(10, v.scale),
                    top,
                    right: cw - v.pad,
                    bottom: top + v.row_h,
                };
                draw_text(mem, row, &mut text_r, flags | DT_END_ELLIPSIS.0, pal.text);
                if !v.desc[i].is_empty() {
                    let mut desc_r = RECT {
                        left: v.pad,
                        top,
                        right: cw - v.pad,
                        bottom: top + v.row_h,
                    };
                    draw_text(mem, &v.desc[i], &mut desc_r, flags | DT_RIGHT.0, pal.sub);
                }
            }
            // footer: aux text on the left, page on the right
            if v.footer_h > 0 {
                SelectObject(mem, HGDIOBJ(small.0));
                let top = top0 + v.rows.len() as i32 * v.row_h;
                let flags = (DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX).0;
                let mut page_w = 0;
                if let Some((cur, total)) = v.page {
                    let label = format!("{cur}/{total}");
                    let ws = wide(&label);
                    let mut sz = SIZE::default();
                    let _ = GetTextExtentPoint32W(mem, &ws, &mut sz);
                    page_w = sz.cx;
                    let mut r = RECT {
                        left: v.pad,
                        top,
                        right: cw - v.pad,
                        bottom: top + v.footer_h,
                    };
                    draw_text(mem, &label, &mut r, flags | DT_RIGHT.0, pal.sub);
                }
                if let Some(aux) = &v.aux {
                    let mut r = RECT {
                        left: v.pad,
                        top,
                        right: cw
                            - v.pad
                            - if page_w > 0 {
                                page_w + px(12, v.scale)
                            } else {
                                0
                            },
                        bottom: top + v.footer_h,
                    };
                    draw_text(mem, aux, &mut r, flags | DT_END_ELLIPSIS.0, pal.sub);
                }
            }
            SelectObject(mem, old_font);
        }
    });
}

fn ensure_window() -> Option<HWND> {
    if let Some(h) = HWND_CAND.with(|h| *h.borrow()) {
        return Some(h);
    }
    unsafe {
        let hinst = GetModuleHandleW(None).ok()?;
        // One class per loaded copy of this code: after an update an app can hold two copies, and
        // a shared name would give the new window the old copy's window procedure.
        let class_name: Vec<u16> = format!(
            "KarukanCandidateWindow_{:x}",
            wnd_proc as *const () as usize
        )
        .encode_utf16()
        .chain(Some(0))
        .collect();
        let class = PCWSTR(class_name.as_ptr());
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: WNDCLASS_STYLES(CS_DROPSHADOW),
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinst.into(),
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassExW(&wc); // already registered is fine
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class,
            PCWSTR::null(),
            WS_POPUP,
            0,
            0,
            10,
            10,
            None,
            None,
            Some(hinst.into()),
            None,
        )
        .ok()?;
        // Windows 11: rounded corners; harmless errors elsewhere.
        let corner = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &corner as *const _ as *const _,
            std::mem::size_of_val(&corner) as u32,
        );
        HWND_CAND.with(|h| *h.borrow_mut() = Some(hwnd));
        Some(hwnd)
    }
}

/// (max width, max height) of the given strings in `font`.
fn measure(font: HFONT, texts: &[&str]) -> (i32, i32) {
    unsafe {
        let hdc = GetDC(None);
        let old = SelectObject(hdc, HGDIOBJ(font.0));
        let mut maxw = 0;
        let mut h = 0;
        for t in texts {
            let ws = wide(t);
            let mut sz = SIZE::default();
            let _ = GetTextExtentPoint32W(hdc, &ws, &mut sz);
            maxw = maxw.max(sz.cx);
            h = h.max(sz.cy);
        }
        SelectObject(hdc, old);
        ReleaseDC(None, hdc);
        (maxw, h)
    }
}

#[derive(Clone, Copy)]
pub struct Anchor {
    /// Top of the composition (screen coordinates).
    pub top: i32,
    /// Bottom-left of the composition.
    pub x: i32,
    pub bottom: i32,
}

/// The tail of `text` that fits `max_w`, with `…` in front when the head was cut:
/// the keys typed last are the ones being looked at.
fn fit_tail(font: HFONT, text: &str, max_w: i32) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut skip = 0;
    loop {
        let s: String = if skip == 0 {
            chars.iter().collect()
        } else {
            std::iter::once('…')
                .chain(chars[skip..].iter().copied())
                .collect()
        };
        if skip + 1 >= chars.len() || measure(font, &[s.as_str()]).0 <= max_w {
            return s;
        }
        skip += 1;
    }
}

/// Lay the candidates out for `scale` and store them in the view; returns
/// the window size in device pixels.
fn compute(
    items: &[CandidateItem],
    cursor: usize,
    page: usize,
    total_pages: usize,
    scale: f32,
    dark: bool,
) -> (i32, i32) {
    let (font, small) = ensure_fonts(scale);
    let rows: Vec<String> = items.iter().map(|c| c.text.clone()).collect();
    let desc: Vec<String> = items
        .iter()
        .map(|c| c.description.clone().unwrap_or_default())
        .collect();
    let row_strs: Vec<&str> = rows.iter().map(String::as_str).collect();
    let desc_strs: Vec<&str> = desc.iter().map(String::as_str).collect();
    let (text_w, text_h) = measure(font, &row_strs);
    let (desc_w, _) = measure(font, &desc_strs);
    let (num_w, _) = measure(font, &["9"]);
    let text_h = text_h.max(px(18, scale));

    let aux = VIEW.with(|v| v.borrow().aux.clone());
    let page_label = (total_pages > 1).then(|| format!("{}/{}", page + 1, total_pages));
    let (aux_w, small_h) = measure(small, &[aux.as_deref().unwrap_or("")]);
    let (page_w, _) = measure(small, &[page_label.as_deref().unwrap_or("")]);
    let has_footer = aux.is_some() || page_label.is_some();

    let pad = px(12, scale);
    let typed = VIEW
        .with(|v| v.borrow().typed.clone())
        .filter(|t| !t.is_empty());
    let typed_shown = typed
        .as_deref()
        .map(|t| fit_tail(font, t, px(520, scale) - pad * 2))
        .unwrap_or_default();
    let (typed_w, _) = measure(font, &[typed_shown.as_str()]);
    let typed_h = if typed.is_some() {
        text_h + px(10, scale)
    } else {
        0
    };
    let row_h = text_h + px(10, scale);
    let footer_h = if has_footer {
        small_h + px(10, scale)
    } else {
        0
    };
    let comment_gap = if desc_w > 0 { px(28, scale) } else { 0 };
    let content_w = pad * 2 + num_w + px(10, scale) + text_w + comment_gap + desc_w;
    let footer_w = pad * 2
        + aux_w
        + if page_w > 0 {
            page_w + px(12, scale)
        } else {
            0
        };
    let width = content_w
        .max(footer_w)
        .max(pad * 2 + typed_w)
        .clamp(px(180, scale), px(520, scale));
    let height = typed_h + rows.len() as i32 * row_h + footer_h + px(8, scale);

    VIEW.with(|v| {
        let mut v = v.borrow_mut();
        v.rows = rows;
        v.desc = desc;
        v.cursor = cursor;
        v.page = page_label.as_ref().map(|_| (page + 1, total_pages));
        v.dark = dark;
        v.scale = scale;
        v.pad = pad;
        v.row_h = row_h;
        v.num_w = num_w;
        v.text_w = text_w;
        v.desc_w = desc_w;
        v.footer_h = footer_h;
        v.typed_shown = typed_shown;
        v.typed_h = typed_h;
        v.width = width;
    });
    (width, height)
}

/// Rows on a full candidate page (the engine pages by nine).
const PAGE_ROWS: usize = 9;

/// Where the window's top goes, and whether it sits above the text: below
/// (`below` is just under the composition) unless a full page would run past
/// the work area's bottom, then right above the composition (`top`).
fn place_y(
    top: i32,
    below: i32,
    height: i32,
    page_height: i32,
    work: (i32, i32),
    gap: i32,
) -> (i32, bool) {
    if below + page_height > work.1 {
        ((top - height - gap).max(work.0), true)
    } else {
        (below, false)
    }
}

/// Show or update the window. `anchor` places it under (or, near the screen
/// bottom, above) the composition; `None` keeps the current position.
pub fn show(
    items: &[CandidateItem],
    cursor: usize,
    page: usize,
    total_pages: usize,
    anchor: Option<Anchor>,
) {
    let Some(hwnd) = ensure_window() else { return };
    VIEW.with(|v| v.borrow_mut().last = Some((items.to_vec(), cursor, page, total_pages)));
    if anchor.is_some() {
        LAST_ANCHOR.with(|a| a.set(anchor));
    }
    let anchor = anchor.or_else(|| LAST_ANCHOR.with(|a| a.get()));
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
    let scale = dpi as f32 / 96.0;

    let (width, height) = compute(items, cursor, page, total_pages, scale, system_is_dark());
    let dark = VIEW.with(|v| v.borrow().dark);
    unsafe {
        // border colour (Windows 11)
        let border = COLORREF(if dark { DARK.border } else { LIGHT.border });
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            &border as *const _ as *const _,
            std::mem::size_of_val(&border) as u32,
        );

        let (mut x, mut y) = match &anchor {
            Some(a) => (a.x, a.bottom + px(2, scale)),
            None => {
                let mut r = RECT::default();
                let _ = GetWindowRect(hwnd, &mut r);
                (r.left, r.top)
            }
        };
        let mut flipped = VIEW.with(|v| v.borrow().flipped);
        if let Some(a) = &anchor {
            flipped = false;
            // keep it on screen: flip above the composition near the bottom.
            // Decided for a full page, not for the rows shown now: with the
            // window's own height a short list (an edit inside the
            // composition) went below the text and a long one above, so the
            // window jumped sides within one composition.
            let row_h = VIEW.with(|v| v.borrow().row_h);
            let shown_rows = items.len().min(PAGE_ROWS) as i32;
            let page_height = height + (PAGE_ROWS as i32 - shown_rows) * row_h;
            let mon = MonitorFromPoint(
                POINT {
                    x: a.x,
                    y: a.bottom,
                },
                MONITOR_DEFAULTTONEAREST,
            );
            let mut mi = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if GetMonitorInfoW(mon, &mut mi).as_bool() {
                let work = mi.rcWork;
                (y, flipped) = place_y(
                    a.top,
                    y,
                    height,
                    page_height,
                    (work.top, work.bottom),
                    px(2, scale),
                );
                if x + width > work.right {
                    x = (work.right - width).max(work.left);
                }
            }
        }
        VIEW.with(|v| v.borrow_mut().flipped = flipped);
        place_log(&anchor, x, y, width, height, dpi);
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            x,
            y,
            width,
            height,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
}

/// Placement trace, written to `debug.log` beside the DLL when a
/// `debug.flag` file exists there.
fn place_log(anchor: &Option<Anchor>, x: i32, y: i32, w: i32, h: i32, dpi: u32) {
    use std::io::Write;
    let Some(dir) = crate::dll_dir() else { return };
    if !dir.join("debug.flag").exists() {
        return;
    }
    let log = crate::rotated_log(&dir);
    let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
    else {
        return;
    };
    let mut work = RECT::default();
    let (mut tx, mut ty) = (0, 0);
    if let Some(a) = anchor {
        let mon = unsafe {
            MonitorFromPoint(
                POINT {
                    x: a.x,
                    y: a.bottom,
                },
                MONITOR_DEFAULTTONEAREST,
            )
        };
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if unsafe { GetMonitorInfoW(mon, &mut mi) }.as_bool() {
            work = mi.rcWork;
        }
        tx = a.x;
        ty = a.top;
    }
    let ctx = unsafe { windows::Win32::UI::HiDpi::GetThreadDpiAwarenessContext() };
    let aw = unsafe { windows::Win32::UI::HiDpi::GetAwarenessFromDpiAwarenessContext(ctx) };
    let _ = writeln!(
        f,
        "pid={} anchor=({tx},{ty},bottom={:?}) work=({},{},{},{}) place=({x},{y},{w}x{h}) dpi={dpi} awareness={}",
        std::process::id(),
        anchor.as_ref().map(|a| a.bottom),
        work.left,
        work.top,
        work.right,
        work.bottom,
        aw.0
    );
}

pub fn set_aux(text: Option<String>) {
    VIEW.with(|v| v.borrow_mut().aux = text);
}

/// The keys typed so far, drawn as a header row above the candidates.
pub fn set_typed(text: Option<String>) {
    VIEW.with(|v| v.borrow_mut().typed = text);
}

/// Lay the open window out again with the current aux line and typed keys
/// (the candidates themselves unchanged), so a key that only changed those
/// is on screen at once.
pub fn refresh() {
    if !is_visible() {
        return;
    }
    if let Some((items, cursor, page, total)) = VIEW.with(|v| v.borrow().last.clone()) {
        show(&items, cursor, page, total, None);
    }
}

pub fn hide() {
    LAST_ANCHOR.with(|a| a.set(None));
    if let Some(h) = HWND_CAND.with(|h| *h.borrow()) {
        unsafe {
            let _ = ShowWindow(h, SW_HIDE);
        }
    }
}

pub fn is_visible() -> bool {
    HWND_CAND.with(|h| {
        h.borrow()
            .map(|h| unsafe { IsWindowVisible(h).as_bool() })
            .unwrap_or(false)
    })
}

pub fn destroy() {
    if let Some(h) = HWND_CAND.with(|h| h.borrow_mut().take()) {
        unsafe {
            let _ = DestroyWindow(h);
        }
    }
}

/// Test hook: render the window offscreen. Returns (width, height, BGRA).
#[doc(hidden)]
#[allow(clippy::too_many_arguments)]
pub fn render_preview(
    items: &[CandidateItem],
    cursor: usize,
    page: usize,
    total_pages: usize,
    aux: Option<String>,
    typed: Option<String>,
    dark: bool,
    scale: f32,
) -> (i32, i32, Vec<u8>) {
    use windows::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, GetDIBits,
    };
    set_aux(aux);
    set_typed(typed);
    let (w, h) = compute(items, cursor, page, total_pages, scale, dark);
    unsafe {
        let hdc = GetDC(None);
        let mem = CreateCompatibleDC(Some(hdc));
        let bmp = CreateCompatibleBitmap(hdc, w, h);
        let old = SelectObject(mem, HGDIOBJ(bmp.0));
        draw(mem, w, h);
        let mut bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut buf = vec![0u8; (w * h * 4) as usize];
        GetDIBits(
            mem,
            bmp,
            0,
            h as u32,
            Some(buf.as_mut_ptr() as *mut _),
            &mut bi,
            DIB_RGB_COLORS,
        );
        SelectObject(mem, old);
        let _ = DeleteObject(HGDIOBJ(bmp.0));
        let _ = DeleteDC(mem);
        ReleaseDC(None, hdc);
        (w, h, buf)
    }
}

#[cfg(test)]
mod placement_tests {
    use super::place_y;

    #[test]
    fn a_short_list_goes_where_a_full_page_would() {
        // text at 1340..1361 on a 1440-high work area; a full page is 340 high
        let full = place_y(1340, 1363, 340, 340, (0, 1440), 2);
        let short = place_y(1340, 1363, 68, 340, (0, 1440), 2);
        assert!(full.1 && short.1, "{full:?} {short:?}");
        // and stays attached right above the text
        assert_eq!(short.0, 1340 - 68 - 2);
        // with room below, both go below
        assert_eq!(place_y(400, 423, 68, 340, (0, 1440), 2), (423, false));
    }
}

#[cfg(test)]
mod click_tests {
    use super::*;

    fn item(t: &str) -> CandidateItem {
        CandidateItem {
            text: t.into(),
            description: None,
        }
    }

    #[test]
    fn a_click_finds_the_row_drawn_there() {
        let items = [item("ただ"), item("例えば"), item("他")];
        // laid out the way the window draws it, typed-keys row on top
        render_preview(&items, 0, 0, 1, None, Some("た".into()), false, 1.0);
        let (top, row_h) = VIEW.with(|v| {
            let v = v.borrow();
            (px(4, v.scale) + v.typed_h, v.row_h)
        });
        assert!(row_h > 0);
        assert_eq!(row_at(top + 1), Some(0));
        assert_eq!(row_at(top + row_h + row_h / 2), Some(1));
        assert_eq!(row_at(top + 2 * row_h + 1), Some(2));
        // the typed-keys row and below the last row are not candidates
        assert_eq!(row_at(top - 1), None);
        assert_eq!(row_at(top + 3 * row_h + 1), None);
    }
}
