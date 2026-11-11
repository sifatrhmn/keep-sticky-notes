//! Shared layout for settings-style windows. Text and buttons wrap to the window width,
//! the window scrolls when content is taller than it, and sizes follow the monitor's
//! current DPI rather than the DPI Windows recorded at sign-in.

use super::app;
use std::cell::RefCell;
use std::collections::HashMap;
use windows::core::PCWSTR;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Controls::SetScrollInfo;
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
use windows::Win32::UI::Input::Touch::{CloseGestureInfoHandle, GetGestureInfo, GESTUREINFO, HGESTUREINFO};
use windows::Win32::UI::WindowsAndMessaging::*;

const GID_PAN: u32 = 4;
const GF_BEGIN: u32 = 1;

#[derive(Clone, Copy)]
pub enum Row {
    Title(i32),
    Text(i32),
    Field(i32),
    Buttons(&'static [i32]),
}

struct State {
    rows: &'static [Row],
    scroll: i32,
    content: i32,
    pan_y: i32,
    busy: bool,
}

thread_local! {
    static FORMS: RefCell<HashMap<isize, State>> = RefCell::new(HashMap::new());
}

pub fn scale(h: HWND, v: i32) -> i32 {
    v * unsafe { GetDpiForWindow(h) }.max(96) as i32 / 96
}

/// A size that grows in touch mode, so fingers have room to hit targets.
pub fn sized(h: HWND, normal: i32, touch: i32) -> i32 {
    scale(h, if app().touch.get() { touch } else { normal })
}

pub unsafe fn control(parent: HWND, class: PCWSTR, text: PCWSTR, style: u32, ex: u32, id: i32) -> HWND {
    let h = CreateWindowExW(
        WINDOW_EX_STYLE(ex),
        class,
        text,
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(style),
        0,
        0,
        10,
        10,
        Some(parent),
        Some(HMENU(id as isize as *mut _)),
        Some(super::hinstance()),
        None,
    )
    .unwrap_or_default();
    let f = app().fonts_for(parent).ui;
    SendMessageW(h, WM_SETFONT, Some(WPARAM(f.0 as usize)), Some(LPARAM(1)));
    h
}

pub unsafe fn children(h: HWND) -> Vec<HWND> {
    let mut out = Vec::new();
    let mut c = GetWindow(h, GW_CHILD).unwrap_or_default();
    while !c.is_invalid() {
        out.push(c);
        c = GetWindow(c, GW_HWNDNEXT).unwrap_or_default();
    }
    out
}

/// Re-applies fonts for the window's DPI and the current touch setting.
pub unsafe fn apply_fonts(h: HWND, bold_ids: &[i32]) {
    let f = app().fonts_for(h);
    for c in children(h) {
        let font = if bold_ids.contains(&GetDlgCtrlID(c)) { f.bold } else { f.ui };
        SendMessageW(c, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
    }
}

fn bold_ids(rows: &[Row]) -> Vec<i32> {
    rows.iter().filter_map(|r| if let Row::Title(id) = r { Some(*id) } else { None }).collect()
}

unsafe fn measure(ctrl: HWND, width: i32, single_line: bool) -> (i32, i32) {
    let n = GetWindowTextLengthW(ctrl) as usize;
    let mut buf = vec![0u16; n + 1];
    let got = GetWindowTextW(ctrl, &mut buf) as usize;
    let mut text: Vec<u16> = buf[..got].to_vec();
    if text.is_empty() {
        text = "Ag".encode_utf16().collect();
    }
    let dc = GetDC(Some(ctrl));
    let font = HFONT(SendMessageW(ctrl, WM_GETFONT, None, None).0 as *mut _);
    let old = SelectObject(dc, font.into());
    let mut rc = RECT { left: 0, top: 0, right: width.max(1), bottom: 0 };
    let flags = if single_line { DT_SINGLELINE } else { DT_WORDBREAK | DT_EDITCONTROL };
    DrawTextW(dc, &mut text, &mut rc, DT_CALCRECT | DT_NOPREFIX | flags);
    SelectObject(dc, old);
    let _ = ReleaseDC(Some(ctrl), dc);
    (rc.right, rc.bottom)
}

pub unsafe fn text_height(ctrl: HWND, width: i32) -> i32 {
    measure(ctrl, width, false).1
}

/// Natural width of a button: its label plus padding, never smaller than a comfortable minimum.
pub unsafe fn button_width(parent: HWND, ctrl: HWND) -> i32 {
    measure(ctrl, 2000, true).0 + sized(parent, 28, 36)
}

/// Positions buttons in rows that wrap to `width`. Returns the height used.
pub unsafe fn flow_buttons(parent: HWND, ids: &[i32], x0: i32, y0: i32, width: i32, apply: bool) -> i32 {
    let row_h = sized(parent, 32, 44);
    let gap = sized(parent, 8, 10);
    let min_w = sized(parent, 96, 112);
    let (mut x, mut y) = (x0, y0);
    for id in ids {
        let Ok(c) = GetDlgItem(Some(parent), *id) else { continue };
        let w = button_width(parent, c).max(min_w).min(width);
        if x > x0 && x + w > x0 + width {
            x = x0;
            y += row_h + gap;
        }
        if apply {
            let _ = MoveWindow(c, x, y, w, row_h, true);
        }
        x += w + gap;
    }
    y + row_h - y0
}

/// Lays out the rows from the top of the client area. Returns the content height.
pub unsafe fn layout_rows(h: HWND, rows: &[Row], scroll: i32) -> i32 {
    let mut c = RECT::default();
    let _ = GetClientRect(h, &mut c);
    let m = scale(h, 16);
    let gap = sized(h, 10, 14);
    let width = (c.right - 2 * m).max(scale(h, 120));
    let mut y = m - scroll;
    for row in rows {
        match *row {
            Row::Title(id) | Row::Text(id) => {
                let Ok(ctrl) = GetDlgItem(Some(h), id) else { continue };
                let ht = text_height(ctrl, width);
                let _ = MoveWindow(ctrl, m, y, width, ht, true);
                y += ht + gap;
            }
            Row::Field(id) => {
                let Ok(ctrl) = GetDlgItem(Some(h), id) else { continue };
                let ht = sized(h, 30, 42);
                let _ = MoveWindow(ctrl, m, y, width, ht, true);
                y += ht + gap;
            }
            Row::Buttons(ids) => {
                y += flow_buttons(h, ids, m, y, width, true) + gap;
            }
        }
    }
    y + scroll - gap + m
}

pub fn attach(h: HWND, rows: &'static [Row]) {
    FORMS.with(|f| {
        f.borrow_mut().insert(h.0 as isize, State { rows, scroll: 0, content: 0, pan_y: 0, busy: false });
    });
}

fn with_state<T>(h: HWND, f: impl FnOnce(&mut State) -> T) -> Option<T> {
    FORMS.with(|m| m.borrow_mut().get_mut(&(h.0 as isize)).map(f))
}

pub unsafe fn relayout(h: HWND) {
    let Some((rows, scroll, busy)) = with_state(h, |s| (s.rows, s.scroll, s.busy)) else { return };
    if busy {
        return;
    }
    with_state(h, |s| s.busy = true);
    let mut pos = scroll;
    for _ in 0..3 {
        let content = layout_rows(h, rows, pos);
        let mut c = RECT::default();
        let _ = GetClientRect(h, &mut c);
        let max = (content - c.bottom).max(0);
        let clamped = pos.clamp(0, max);
        let si = SCROLLINFO {
            cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
            fMask: SIF_RANGE | SIF_PAGE | SIF_POS,
            nMin: 0,
            nMax: if max > 0 { content - 1 } else { 0 },
            nPage: if max > 0 { c.bottom.max(1) as u32 } else { 0 },
            nPos: clamped,
            nTrackPos: 0,
        };
        SetScrollInfo(h, SB_VERT, &si, true);
        let mut after = RECT::default();
        let _ = GetClientRect(h, &mut after);
        with_state(h, |s| {
            s.scroll = clamped;
            s.content = content;
        });
        if after.right == c.right && clamped == pos {
            break;
        }
        pos = clamped;
    }
    with_state(h, |s| s.busy = false);
}

unsafe fn scroll_to(h: HWND, pos: i32) {
    with_state(h, |s| s.scroll = pos);
    relayout(h);
}

/// Reads a one-finger pan. Returns the vertical movement since the previous event.
pub unsafe fn pan_delta(lp: LPARAM, last: &mut i32) -> Option<i32> {
    let handle = HGESTUREINFO(lp.0 as *mut _);
    let mut gi = GESTUREINFO { cbSize: std::mem::size_of::<GESTUREINFO>() as u32, ..Default::default() };
    if GetGestureInfo(handle, &mut gi).is_err() || gi.dwID != GID_PAN {
        return None;
    }
    let y = gi.ptsLocation.y as i32;
    let delta = if gi.dwFlags & GF_BEGIN != 0 { 0 } else { *last - y };
    *last = y;
    let _ = CloseGestureInfoHandle(handle);
    Some(delta)
}

/// Sizes the window so its client area is `cw` wide and `ch` high (96-DPI units),
/// clamped to the monitor's work area, and centres it.
pub unsafe fn fit_client(h: HWND, cw: i32, ch: i32) {
    let dpi = GetDpiForWindow(h).max(96);
    let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    let _ = GetMonitorInfoW(MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST), &mut info);
    let work = info.rcWork;
    let style = WINDOW_STYLE(GetWindowLongW(h, GWL_STYLE) as u32);
    let ex = WINDOW_EX_STYLE(GetWindowLongW(h, GWL_EXSTYLE) as u32);
    let mut frame = RECT::default();
    let _ = AdjustWindowRectExForDpi(&mut frame, style, false, ex, dpi);
    let (fw, fh) = (frame.right - frame.left, frame.bottom - frame.top);
    let w = (scale(h, cw) + fw).min(work.right - work.left);
    let ht = (ch + fh).min(work.bottom - work.top);
    let x = work.left + ((work.right - work.left) - w) / 2;
    let y = work.top + ((work.bottom - work.top) - ht) / 2;
    let _ = SetWindowPos(h, None, x, y, w, ht, SWP_NOZORDER | SWP_NOACTIVATE);
}

/// Sizes a form to its content at the window's real DPI.
pub unsafe fn fit_form(h: HWND, width_96: i32) {
    fit_client(h, width_96, scale(h, 200));
    relayout(h);
    let content = with_state(h, |s| s.content).unwrap_or(0);
    fit_client(h, width_96, content);
    relayout(h);
}

pub unsafe fn refresh_fonts(h: HWND) {
    if let Some(rows) = with_state(h, |s| s.rows) {
        apply_fonts(h, &bold_ids(rows));
        relayout(h);
    }
}

/// Shared handling for resizing, scrolling, touch panning and DPI changes.
pub unsafe fn handle(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
    with_state(h, |_| ())?;
    match msg {
        WM_SIZE => {
            relayout(h);
            Some(LRESULT(0))
        }
        WM_GETMINMAXINFO => {
            let info = &mut *(lp.0 as *mut MINMAXINFO);
            info.ptMinTrackSize.x = scale(h, 320);
            info.ptMinTrackSize.y = scale(h, 220);
            Some(LRESULT(0))
        }
        WM_VSCROLL => {
            let (pos, content) = with_state(h, |s| (s.scroll, s.content)).unwrap_or((0, 0));
            let mut c = RECT::default();
            let _ = GetClientRect(h, &mut c);
            let line = scale(h, 30);
            let new = match (wp.0 & 0xFFFF) as i32 {
                0 => pos - line,
                1 => pos + line,
                2 => pos - c.bottom,
                3 => pos + c.bottom,
                4 | 5 => ((wp.0 >> 16) & 0xFFFF) as i32,
                6 => 0,
                7 => content,
                _ => pos,
            };
            scroll_to(h, new);
            Some(LRESULT(0))
        }
        WM_MOUSEWHEEL => {
            let delta = ((wp.0 >> 16) & 0xFFFF) as i16 as i32;
            let pos = with_state(h, |s| s.scroll).unwrap_or(0);
            scroll_to(h, pos - delta * scale(h, 40) / 120);
            Some(LRESULT(0))
        }
        WM_GESTURE => {
            let mut last = with_state(h, |s| s.pan_y).unwrap_or(0);
            let d = pan_delta(lp, &mut last)?;
            let pos = with_state(h, |s| {
                s.pan_y = last;
                s.scroll
            })
            .unwrap_or(0);
            scroll_to(h, pos + d);
            Some(LRESULT(0))
        }
        WM_DPICHANGED => {
            let r = &*(lp.0 as *const RECT);
            let _ = SetWindowPos(h, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
            refresh_fonts(h);
            Some(LRESULT(0))
        }
        WM_DESTROY => {
            FORMS.with(|f| f.borrow_mut().remove(&(h.0 as isize)));
            None
        }
        _ => None,
    }
}
