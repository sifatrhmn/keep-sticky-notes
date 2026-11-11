//! Appearance. Themes change only how a note looks here: the Keep colour that syncs is unchanged.

use super::{app, rgb};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::WindowsAndMessaging::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Theme {
    Classic,
    Vibrant,
    Black,
    Mac,
}

pub const THEMES: [(Theme, &str, &str); 4] = [
    (Theme::Black, "black", "OLED Black"),
    (Theme::Vibrant, "vibrant", "OLED Vibrant"),
    (Theme::Classic, "classic", "Light"),
    (Theme::Mac, "mac", "macOS Style"),
];

impl Theme {
    pub fn from_key(k: Option<&str>) -> Theme {
        THEMES.iter().find(|t| Some(t.1) == k).map(|t| t.0).unwrap_or(Theme::Black)
    }
    pub fn key(self) -> &'static str {
        THEMES.iter().find(|t| t.0 == self).unwrap().1
    }
    pub fn is_dark(self) -> bool {
        matches!(self, Theme::Black | Theme::Vibrant)
    }
}

#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: u32,
    pub bar: u32,
    pub hover: u32,
    pub text: u32,
    pub muted: u32,
    pub accent: u32,
}

/// API colour, Classic pastel, Vibrant background (white text passes 4.5:1), Black accent.
const COLOURS: [(&str, u32, u32, u32); 12] = [
    ("DEFAULT", 0xFFFFFF, 0x202124, 0xFFFFFF),
    ("RED", 0xFAAFA8, 0xB0122B, 0xFF4D5E),
    ("ORANGE", 0xF39F76, 0xB04600, 0xFF9100),
    ("YELLOW", 0xFFF8B8, 0x806500, 0xFFD600),
    ("GREEN", 0xE2F6D3, 0x0B7A3B, 0x00E676),
    ("TEAL", 0xB4DDD3, 0x00736B, 0x1DE9B6),
    ("BLUE", 0xD4E4ED, 0x0B5FB5, 0x40A9FF),
    ("CERULEAN", 0xAECCDC, 0x004E92, 0x00B0FF),
    ("PURPLE", 0xD3BFDB, 0x6A1FB0, 0xB388FF),
    ("PINK", 0xF6E2DD, 0xB0146E, 0xFF4FA3),
    ("BROWN", 0xE9E3D4, 0x6E3F1C, 0xD7A36A),
    ("GRAY", 0xEFEFF1, 0x3C4043, 0xB0B0B0),
];

/// macOS Stickies-inspired paper colours for each Keep colour.
const MAC: [(&str, u32); 12] = [
    ("DEFAULT", 0xF7F7F2),
    ("RED", 0xFFC4C0),
    ("ORANGE", 0xFFD8A8),
    ("YELLOW", 0xFEF49C),
    ("GREEN", 0xCDF2B0),
    ("TEAL", 0xBDEDE2),
    ("BLUE", 0xBFE3FA),
    ("CERULEAN", 0xAFCCF5),
    ("PURPLE", 0xDDC9F2),
    ("PINK", 0xFDCBE6),
    ("BROWN", 0xE9D8BE),
    ("GRAY", 0xE4E4E6),
];

pub fn lighten(v: u32, pct: u32) -> u32 {
    let f = |c: u32| c + (255 - c) * pct / 100;
    (f((v >> 16) & 0xFF) << 16) | (f((v >> 8) & 0xFF) << 8) | f(v & 0xFF)
}

pub fn darken(v: u32, pct: u32) -> u32 {
    let f = |c: u32| c * (100 - pct) / 100;
    (f((v >> 16) & 0xFF) << 16) | (f((v >> 8) & 0xFF) << 8) | f(v & 0xFF)
}

pub fn palette(theme: Theme, api: &str) -> Palette {
    let c = COLOURS.iter().find(|c| c.0 == api).unwrap_or(&COLOURS[0]);
    match theme {
        Theme::Classic => Palette {
            bg: c.1,
            bar: darken(c.1, 7),
            hover: darken(c.1, 16),
            text: 0x202124,
            muted: 0x5F6368,
            accent: 0x202124,
        },
        Theme::Vibrant => Palette {
            bg: c.2,
            bar: darken(c.2, 22),
            hover: darken(c.2, 40),
            text: 0xFFFFFF,
            muted: 0xE0E0E0,
            accent: 0xFFFFFF,
        },
        Theme::Black => Palette { bg: 0, bar: 0, hover: 0x262626, text: 0xF0F0F0, muted: 0xA0A0A0, accent: c.3 },
        Theme::Mac => {
            let bg = MAC.iter().find(|m| m.0 == api).map(|m| m.1).unwrap_or(MAC[0].1);
            Palette { bg, bar: darken(bg, 8), hover: darken(bg, 15), text: 0x1D1D1F, muted: 0x6E6E73, accent: 0x3C3C43 }
        }
    }
}

pub fn chrome_bg(theme: Theme) -> u32 {
    match theme {
        Theme::Classic => 0xF3F3F3,
        Theme::Vibrant => 0x121212,
        Theme::Black => 0x000000,
        Theme::Mac => 0xECECEC,
    }
}

/// Background for text boxes and lists.
pub fn field_bg(theme: Theme) -> u32 {
    match theme {
        Theme::Mac => 0xFFFFFF,
        t => chrome_bg(t),
    }
}

fn chrome_text(theme: Theme) -> u32 {
    if theme.is_dark() {
        0xF0F0F0
    } else {
        0x202124
    }
}

unsafe fn set_dwm(h: HWND, attr: DWMWINDOWATTRIBUTE, v: u32) {
    let _ = DwmSetWindowAttribute(h, attr, &v as *const _ as *const _, 4);
}

/// Border colour for a note, so a black note stays distinct on a black desktop.
pub unsafe fn apply_note_frame(h: HWND, theme: Theme, p: Palette) {
    const DEFAULT: u32 = 0xFFFFFFFF;
    set_dwm(h, DWMWA_BORDER_COLOR, if theme == Theme::Black { rgb(p.accent).0 } else { DEFAULT });
}

pub unsafe fn apply_control_theme(h: HWND, dark: bool, combo: bool) {
    let name: PCWSTR = match (dark, combo) {
        (true, true) => w!("DarkMode_CFD"),
        (true, false) => w!("DarkMode_Explorer"),
        _ => w!("Explorer"),
    };
    let _ = SetWindowTheme(h, name, PCWSTR::null());
}

/// Applies the current theme to a settings-style window and its controls.
pub unsafe fn apply_chrome(h: HWND) {
    if h.is_invalid() {
        return;
    }
    let theme = app().theme.get();
    set_dwm(h, DWMWA_USE_IMMERSIVE_DARK_MODE, theme.is_dark() as u32);
    let caption = if theme.is_dark() || theme == Theme::Mac { rgb(chrome_bg(theme)).0 } else { 0xFFFFFFFF };
    set_dwm(h, DWMWA_CAPTION_COLOR, caption);
    let mut child = GetWindow(h, GW_CHILD).unwrap_or_default();
    while !child.is_invalid() {
        let mut buf = [0u16; 32];
        let n = GetClassNameW(child, &mut buf) as usize;
        let class = String::from_utf16_lossy(&buf[..n]).to_uppercase();
        apply_control_theme(child, theme.is_dark(), class == "COMBOBOX");
        child = GetWindow(child, GW_HWNDNEXT).unwrap_or_default();
    }
    let _ = RedrawWindow(Some(h), None, None, RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN | RDW_FRAME);
}

unsafe fn draw_button(d: &DRAWITEMSTRUCT) {
    let a = app();
    let theme = a.theme.get();
    let dark = theme.is_dark();
    let pressed = d.itemState.0 & ODS_SELECTED.0 != 0;
    let disabled = d.itemState.0 & ODS_DISABLED.0 != 0;
    let focused = d.itemState.0 & ODS_FOCUS.0 != 0;
    let back = CreateSolidBrush(rgb(chrome_bg(theme)));
    FillRect(d.hDC, &d.rcItem, back);
    let _ = DeleteObject(back.into());
    let fill = match (dark, pressed) {
        (true, true) => 0x333333,
        (true, false) => 0x1C1C1C,
        (false, true) => 0xD8D8D8,
        (false, false) => 0xFFFFFF,
    };
    let edge = match (theme, dark, focused) {
        (Theme::Mac, _, true) => 0x007AFF,
        (Theme::Mac, _, false) => 0xC7C7CC,
        (_, true, true) => 0xFFFFFF,
        (_, true, false) => 0x5A5A5A,
        (_, false, true) => 0x202124,
        (_, false, false) => 0xB0B0B0,
    };
    let brush = CreateSolidBrush(rgb(fill));
    let pen = CreatePen(PS_SOLID, if focused { super::form::scale(d.hwndItem, 2) } else { 1 }, rgb(edge));
    let old_b = SelectObject(d.hDC, brush.into());
    let old_p = SelectObject(d.hDC, pen.into());
    let r = d.rcItem;
    let radius = if theme == Theme::Mac { super::form::scale(d.hwndItem, 14) } else { super::form::scale(d.hwndItem, 4) };
    let _ = RoundRect(d.hDC, r.left, r.top, r.right, r.bottom, radius, radius);
    SelectObject(d.hDC, old_b);
    SelectObject(d.hDC, old_p);
    let _ = DeleteObject(brush.into());
    let _ = DeleteObject(pen.into());

    let n = GetWindowTextLengthW(d.hwndItem) as usize;
    let mut buf = vec![0u16; n + 1];
    let got = GetWindowTextW(d.hwndItem, &mut buf) as usize;
    let font = HFONT(SendMessageW(d.hwndItem, WM_GETFONT, None, None).0 as *mut _);
    let old = SelectObject(d.hDC, font.into());
    SetBkMode(d.hDC, TRANSPARENT);
    SetTextColor(d.hDC, rgb(if disabled { 0x808080 } else { chrome_text(theme) }));
    let mut tr = d.rcItem;
    DrawTextW(d.hDC, &mut buf[..got], &mut tr, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS);
    SelectObject(d.hDC, old);
}

unsafe fn draw_list_item(d: &DRAWITEMSTRUCT) {
    if d.itemID == u32::MAX {
        return;
    }
    let theme = app().theme.get();
    let selected = d.itemState.0 & ODS_SELECTED.0 != 0;
    let (bg, fg) = match (theme, selected) {
        (Theme::Mac, true) => (0x0A64D2, 0xFFFFFF),
        (t, true) if t.is_dark() => (0x333333, 0xFFFFFF),
        (_, true) => (0xCCE4F7, 0x202124),
        (t, false) => (field_bg(t), chrome_text(t)),
    };
    let brush = CreateSolidBrush(rgb(bg));
    FillRect(d.hDC, &d.rcItem, brush);
    let _ = DeleteObject(brush.into());
    let len = SendMessageW(d.hwndItem, LB_GETTEXTLEN, Some(WPARAM(d.itemID as usize)), None).0;
    if len <= 0 {
        return;
    }
    let mut buf = vec![0u16; len as usize + 1];
    SendMessageW(d.hwndItem, LB_GETTEXT, Some(WPARAM(d.itemID as usize)), Some(LPARAM(buf.as_mut_ptr() as isize)));
    let font = HFONT(SendMessageW(d.hwndItem, WM_GETFONT, None, None).0 as *mut _);
    let old = SelectObject(d.hDC, font.into());
    SetBkMode(d.hDC, TRANSPARENT);
    SetTextColor(d.hDC, rgb(fg));
    let pad = super::form::scale(d.hwndItem, 8);
    let mut r = RECT { left: d.rcItem.left + pad, right: d.rcItem.right - pad, ..d.rcItem };
    DrawTextW(d.hDC, &mut buf[..len as usize], &mut r, DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS);
    SelectObject(d.hDC, old);
}

/// Shared message handling that themes the Note Manager and Account windows.
pub unsafe fn chrome_message(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
    let a = app();
    match msg {
        WM_ERASEBKGND => {
            let mut c = RECT::default();
            let _ = GetClientRect(h, &mut c);
            FillRect(HDC(wp.0 as *mut _), &c, a.chrome_brush.get());
            Some(LRESULT(1))
        }
        WM_CTLCOLORSTATIC | WM_CTLCOLORBTN => {
            let dc = HDC(wp.0 as *mut _);
            let t = a.theme.get();
            SetBkColor(dc, rgb(chrome_bg(t)));
            SetTextColor(dc, rgb(chrome_text(t)));
            Some(LRESULT(a.chrome_brush.get().0 as isize))
        }
        WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX => {
            let dc = HDC(wp.0 as *mut _);
            let t = a.theme.get();
            SetBkColor(dc, rgb(field_bg(t)));
            SetTextColor(dc, rgb(chrome_text(t)));
            Some(LRESULT(a.field_brush.get().0 as isize))
        }
        WM_DRAWITEM => {
            let d = &*(lp.0 as *const DRAWITEMSTRUCT);
            if d.CtlType == ODT_BUTTON {
                draw_button(d);
                return Some(LRESULT(1));
            }
            if d.CtlType == ODT_LISTBOX {
                draw_list_item(d);
                return Some(LRESULT(1));
            }
            None
        }
        _ => None,
    }
}
