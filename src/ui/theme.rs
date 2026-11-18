//! Appearance. Themes change only how a note looks here: the Keep colour that syncs is unchanged.

use super::form::{scale, sized};
use super::{app, rgb};
use std::cell::RefCell;
use std::collections::HashMap;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::GetSystemMetricsForDpi;
use windows::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows::Win32::UI::WindowsAndMessaging::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Theme {
    Classic,
    Black,
    Mac,
    MacDark,
}

/// Each theme comes in a family: Light/Dark Mode switches within the family.
/// The key "mac" is the dark macOS style, so choosing macOS Style gives the black look.
pub const THEMES: [(Theme, &str, &str); 4] = [
    (Theme::Black, "black", "OLED Black"),
    (Theme::Classic, "classic", "Light"),
    (Theme::MacDark, "mac", "macOS Style (Dark)"),
    (Theme::Mac, "mac_light", "macOS Style (Light)"),
];

impl Theme {
    pub fn from_key(k: Option<&str>) -> Theme {
        THEMES.iter().find(|t| Some(t.1) == k).map(|t| t.0).unwrap_or(Theme::Black)
    }
    pub fn key(self) -> &'static str {
        THEMES.iter().find(|t| t.0 == self).unwrap().1
    }
    pub fn is_dark(self) -> bool {
        matches!(self, Theme::Black | Theme::MacDark)
    }
    pub fn is_mac(self) -> bool {
        matches!(self, Theme::Mac | Theme::MacDark)
    }
    /// The other member of this theme's light/dark pair.
    pub fn counterpart(self) -> Theme {
        match self {
            Theme::Black => Theme::Classic,
            Theme::Classic => Theme::Black,
            Theme::Mac => Theme::MacDark,
            Theme::MacDark => Theme::Mac,
        }
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

/// API colour, Classic pastel, (unused), Black accent.
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

/// Blends `a` towards `b` by `pct` per cent.
pub fn mix(a: u32, b: u32, pct: u32) -> u32 {
    let f = |sh: u32| {
        let (x, y) = ((a >> sh) & 0xFF, (b >> sh) & 0xFF);
        ((x * (100 - pct) + y * pct) / 100) << sh
    };
    f(16) | f(8) | f(0)
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
        Theme::Black => Palette { bg: 0, bar: 0, hover: 0x262626, text: 0xF0F0F0, muted: 0xA0A0A0, accent: c.3 },
        Theme::MacDark => {
            // macOS dark appearance: near-black graphite paper with a hint of the note's colour.
            let paper = MAC.iter().find(|m| m.0 == api).map(|m| m.1).unwrap_or(MAC[0].1);
            let bg = mix(paper, 0x1C1C1E, 88);
            Palette { bg, bar: lighten(bg, 5), hover: lighten(bg, 14), text: 0xF5F5F7, muted: 0x98989D, accent: paper }
        }
        Theme::Mac => {
            let bg = MAC.iter().find(|m| m.0 == api).map(|m| m.1).unwrap_or(MAC[0].1);
            Palette { bg, bar: darken(bg, 8), hover: darken(bg, 12), text: 0x1D1D1F, muted: 0x6E6E73, accent: 0x3C3C43 }
        }
    }
}

/// Index of a Keep colour, used as Note Manager list item data (index + 1).
pub fn colour_index(api: &str) -> usize {
    COLOURS.iter().position(|c| c.0 == api).unwrap_or(0)
}

/// Card colour and text colour for a note row in the Note Manager, like the Keep Android app.
pub fn list_tint(theme: Theme, index: usize) -> (u32, u32) {
    let c = COLOURS.get(index).unwrap_or(&COLOURS[0]);
    let paper = MAC.get(index).map(|m| m.1).unwrap_or(MAC[0].1);
    match theme {
        Theme::Classic => (c.1, 0x202124),
        Theme::Black if index == 0 => (0x161616, 0xF0F0F0),
        Theme::Black => (mix(c.3, 0x000000, 72), 0xF5F5F5),
        Theme::MacDark if index == 0 => (0x3A3A3C, 0xF5F5F7),
        Theme::MacDark => (mix(paper, 0x2C2C2E, 72), 0xF5F5F7),
        Theme::Mac => (paper, 0x1D1D1F),
    }
}

pub fn chrome_bg(theme: Theme) -> u32 {
    match theme {
        Theme::Classic => 0xF3F3F3,
        Theme::Black => 0x000000,
        Theme::Mac => 0xF5F5F7,
        Theme::MacDark => 0x1E1E1E,
    }
}

/// Background for text boxes and lists.
pub fn field_bg(theme: Theme) -> u32 {
    match theme {
        Theme::Mac => 0xFFFFFF,
        Theme::MacDark => 0x2C2C2E,
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
    let caption = if theme.is_dark() || theme.is_mac() { rgb(chrome_bg(theme)).0 } else { 0xFFFFFFFF };
    set_dwm(h, DWMWA_CAPTION_COLOR, caption);
    let mut child = GetWindow(h, GW_CHILD).unwrap_or_default();
    while !child.is_invalid() {
        let mut buf = [0u16; 32];
        let n = GetClassNameW(child, &mut buf) as usize;
        let class = String::from_utf16_lossy(&buf[..n]).to_uppercase();
        apply_control_theme(child, theme.is_dark(), class == "COMBOBOX");
        child = GetWindow(child, GW_HWNDNEXT).unwrap_or_default();
    }
    let _ = SetWindowPos(h, None, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED);
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
        _ if theme == Theme::MacDark => if pressed { 0x48484A } else { 0x3A3A3C },
        _ if theme == Theme::Mac && pressed => 0xE5E5EA,
        (true, true) => 0x333333,
        (true, false) => 0x1C1C1C,
        (false, true) => 0xD8D8D8,
        (false, false) => 0xFFFFFF,
    };
    let edge = match (theme, dark, focused) {
        (t, _, true) if t.is_mac() => 0x0A84FF,
        (Theme::MacDark, _, false) => 0x505053,
        (Theme::Mac, _, false) => 0xD2D2D7,
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
    let radius = if theme.is_mac() { r.bottom - r.top } else { scale(d.hwndItem, 4) };
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

/// A note row drawn as a rounded card in the note's own colour; selection is an outline.
unsafe fn draw_tinted_item(d: &DRAWITEMSTRUCT, index: usize) {
    let theme = app().theme.get();
    let selected = d.itemState.0 & ODS_SELECTED.0 != 0;
    let back = CreateSolidBrush(rgb(field_bg(theme)));
    FillRect(d.hDC, &d.rcItem, back);
    let _ = DeleteObject(back.into());
    let (card, fg) = list_tint(theme, index);
    let s = |v| scale(d.hwndItem, v);
    let r = RECT { left: d.rcItem.left + s(4), top: d.rcItem.top + s(2), right: d.rcItem.right - s(4), bottom: d.rcItem.bottom - s(2) };
    let brush = CreateSolidBrush(rgb(card));
    let accent = if theme.is_mac() { 0x0A84FF } else if theme.is_dark() { 0x8AB4F8 } else { 0x1A73E8 };
    let outline = if selected { accent } else if theme.is_dark() { lighten(card, 12) } else { darken(card, 10) };
    let pen = CreatePen(PS_SOLID, if selected { s(2) } else { 1 }, rgb(outline));
    let ob = SelectObject(d.hDC, brush.into());
    let op = SelectObject(d.hDC, pen.into());
    let rad = s(if theme.is_mac() { 12 } else { 8 });
    let _ = RoundRect(d.hDC, r.left, r.top, r.right, r.bottom, rad, rad);
    SelectObject(d.hDC, ob);
    SelectObject(d.hDC, op);
    let _ = DeleteObject(brush.into());
    let _ = DeleteObject(pen.into());
    let len = SendMessageW(d.hwndItem, LB_GETTEXTLEN, Some(WPARAM(d.itemID as usize)), None).0;
    if len <= 0 {
        return;
    }
    let mut buf = vec![0u16; len as usize + 1];
    SendMessageW(d.hwndItem, LB_GETTEXT, Some(WPARAM(d.itemID as usize)), Some(LPARAM(buf.as_mut_ptr() as isize)));
    let font = HFONT(SendMessageW(d.hwndItem, WM_GETFONT, None, None).0 as *mut _);
    let bold = bold_of(font);
    SetBkMode(d.hDC, TRANSPARENT);
    SetTextColor(d.hDC, rgb(fg));
    // The title (before the first separator) is drawn bold, the preview in the normal weight.
    let text = &buf[..len as usize];
    let sep: Vec<u16> = "  |  ".encode_utf16().collect();
    let split = text.windows(sep.len()).position(|w| w == sep.as_slice()).unwrap_or(text.len());
    let flags = DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS;
    let mut tr = RECT { left: r.left + s(10), right: r.right - s(10), ..r };
    let old = SelectObject(d.hDC, bold.into());
    let mut title = text[..split].to_vec();
    let mut size = SIZE::default();
    let _ = GetTextExtentPoint32W(d.hDC, &title, &mut size);
    DrawTextW(d.hDC, &mut title, &mut tr, flags);
    if split < text.len() {
        SelectObject(d.hDC, font.into());
        let mut rest = text[split..].to_vec();
        tr.left += size.cx;
        if tr.left < tr.right {
            DrawTextW(d.hDC, &mut rest, &mut tr, flags);
        }
    }
    SelectObject(d.hDC, old);
    let _ = DeleteObject(bold.into());
}

/// A bold copy of `font`; the caller deletes it.
unsafe fn bold_of(font: HFONT) -> HFONT {
    let mut lf = LOGFONTW::default();
    GetObjectW(font.into(), std::mem::size_of::<LOGFONTW>() as i32, Some(&mut lf as *mut _ as *mut _));
    lf.lfWeight = 700;
    CreateFontIndirectW(&lf)
}

/// A "PINNED" / "OTHERS" section heading in the Note Manager.
unsafe fn draw_section_header(d: &DRAWITEMSTRUCT) {
    let theme = app().theme.get();
    let back = CreateSolidBrush(rgb(field_bg(theme)));
    FillRect(d.hDC, &d.rcItem, back);
    let _ = DeleteObject(back.into());
    let len = SendMessageW(d.hwndItem, LB_GETTEXTLEN, Some(WPARAM(d.itemID as usize)), None).0;
    if len <= 0 {
        return;
    }
    let mut buf = vec![0u16; len as usize + 1];
    SendMessageW(d.hwndItem, LB_GETTEXT, Some(WPARAM(d.itemID as usize)), Some(LPARAM(buf.as_mut_ptr() as isize)));
    let font = HFONT(SendMessageW(d.hwndItem, WM_GETFONT, None, None).0 as *mut _);
    let mut lf = LOGFONTW::default();
    GetObjectW(font.into(), std::mem::size_of::<LOGFONTW>() as i32, Some(&mut lf as *mut _ as *mut _));
    lf.lfWeight = 700;
    lf.lfHeight = lf.lfHeight * 4 / 5;
    let small = CreateFontIndirectW(&lf);
    let old = SelectObject(d.hDC, small.into());
    SetBkMode(d.hDC, TRANSPARENT);
    SetTextColor(d.hDC, rgb(if theme.is_dark() { 0x98989D } else { 0x5F6368 }));
    let mut r = RECT { left: d.rcItem.left + scale(d.hwndItem, 12), top: d.rcItem.top + scale(d.hwndItem, 6), ..d.rcItem };
    DrawTextW(d.hDC, &mut buf[..len as usize], &mut r, DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
    SelectObject(d.hDC, old);
    let _ = DeleteObject(small.into());
}

/// Item data that marks a Note Manager section heading.
pub const SECTION_HEADER: usize = 0x100;

unsafe fn draw_list_item(d: &DRAWITEMSTRUCT) {
    if d.itemID == u32::MAX {
        return;
    }
    if d.itemData == SECTION_HEADER {
        return draw_section_header(d);
    }
    if (1..=COLOURS.len()).contains(&d.itemData) {
        return draw_tinted_item(d, d.itemData - 1);
    }
    let theme = app().theme.get();
    let selected = d.itemState.0 & ODS_SELECTED.0 != 0;
    if theme.is_mac() {
        let back = CreateSolidBrush(rgb(field_bg(theme)));
        FillRect(d.hDC, &d.rcItem, back);
        let _ = DeleteObject(back.into());
        if selected {
            let brush = CreateSolidBrush(rgb(0x0A84FF));
            let ob = SelectObject(d.hDC, brush.into());
            let op = SelectObject(d.hDC, GetStockObject(NULL_PEN));
            let r = d.rcItem;
            let rad = scale(d.hwndItem, 12);
            let _ = RoundRect(d.hDC, r.left + 2, r.top + 1, r.right - 1, r.bottom, rad, rad);
            SelectObject(d.hDC, ob);
            SelectObject(d.hDC, op);
            let _ = DeleteObject(brush.into());
        }
    }
    let (bg, fg) = match (theme, selected) {
        (t, true) if t.is_mac() => (0x0A84FF, 0xFFFFFF),
        (t, true) if t.is_dark() => (0x333333, 0xFFFFFF),
        (_, true) => (0xCCE4F7, 0x202124),
        (t, false) => (field_bg(t), chrome_text(t)),
    };
    if !theme.is_mac() {
        let brush = CreateSolidBrush(rgb(bg));
        FillRect(d.hDC, &d.rcItem, brush);
        let _ = DeleteObject(brush.into());
    }
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
            if is_mac() {
                paint_strip(h, HDC(wp.0 as *mut _));
            }
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

// ----- macOS-style window frame for the Note Manager, Account and About windows -----

thread_local! {
    /// Per window: (traffic lights hovered, mouse-leave tracking active).
    static FRAME: RefCell<HashMap<isize, (bool, bool)>> = RefCell::new(HashMap::new());
}

pub fn is_mac() -> bool {
    app().theme.get().is_mac()
}

/// Height of the custom title strip, or zero when the Windows title bar is in use.
pub fn top_inset(h: HWND) -> i32 {
    if is_mac() {
        sized(h, 38, 50)
    } else {
        0
    }
}

/// One traffic light: a coloured disc, with a glyph when the group is hovered.
pub unsafe fn draw_light(dc: HDC, cx: i32, cy: i32, d: i32, colour: u32, glyph: Option<u16>, tiny: HFONT) {
    let brush = CreateSolidBrush(rgb(colour));
    let pen = CreatePen(PS_SOLID, 1, rgb(darken(colour, 16)));
    let ob = SelectObject(dc, brush.into());
    let op = SelectObject(dc, pen.into());
    let _ = Ellipse(dc, cx - d / 2, cy - d / 2, cx - d / 2 + d, cy - d / 2 + d);
    SelectObject(dc, ob);
    SelectObject(dc, op);
    let _ = DeleteObject(brush.into());
    let _ = DeleteObject(pen.into());
    if let Some(g) = glyph {
        let old = SelectObject(dc, tiny.into());
        SetBkMode(dc, TRANSPARENT);
        SetTextColor(dc, rgb(0x4A1F1C));
        let mut buf = [g];
        let mut r = RECT { left: cx - d / 2, top: cy - d / 2, right: cx - d / 2 + d, bottom: cy - d / 2 + d };
        DrawTextW(dc, &mut buf, &mut r, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
        SelectObject(dc, old);
    }
}

fn light_slot(h: HWND, i: i32) -> RECT {
    let w = sized(h, 20, 34);
    let x0 = scale(h, 10);
    RECT { left: x0 + i * w, top: 0, right: x0 + (i + 1) * w, bottom: top_inset(h) }
}

fn light_at(h: HWND, x: i32, y: i32) -> Option<i32> {
    (0..3).find(|&i| {
        let r = light_slot(h, i);
        x >= r.left && x < r.right && y >= r.top && y < r.bottom
    })
}

unsafe fn can_zoom(h: HWND) -> bool {
    GetWindowLongW(h, GWL_STYLE) as u32 & WS_MAXIMIZEBOX.0 != 0
}

unsafe fn strip_rect(h: HWND) -> RECT {
    let mut c = RECT::default();
    let _ = GetClientRect(h, &mut c);
    RECT { bottom: top_inset(h), ..c }
}

unsafe fn paint_strip(h: HWND, dc: HDC) {
    let r = strip_rect(h);
    let th = r.bottom.max(1);
    let dark = app().theme.get().is_dark();
    let (top, bottom) = if dark { (0x3A3A3Cu32, 0x2A2A2C) } else { (0xFFFFFFu32, chrome_bg(Theme::Mac)) };
    for line in 0..th {
        let mix = |sh: u32| -> u32 {
            let (a, b) = (((top >> sh) & 0xFF) as i32, ((bottom >> sh) & 0xFF) as i32);
            ((a + (b - a) * line / th) as u32) << sh
        };
        let br = CreateSolidBrush(rgb(mix(16) | mix(8) | mix(0)));
        FillRect(dc, &RECT { left: 0, top: line, right: r.right, bottom: line + 1 }, br);
        let _ = DeleteObject(br.into());
    }
    let hair = CreateSolidBrush(rgb(if dark { 0x0A0A0A } else { 0xE1E1E6 }));
    FillRect(dc, &RECT { left: 0, top: th - 1, right: r.right, bottom: th }, hair);
    let _ = DeleteObject(hair.into());

    let fonts = app().fonts_for(h);
    let hovered = FRAME.with(|f| f.borrow().get(&(h.0 as isize)).map(|s| s.0).unwrap_or(false));
    let colours = [0xFF5F57, 0xFEBC2E, if can_zoom(h) { 0x28C840 } else if dark { 0x5A5A5E } else { 0xD0D0D5 }];
    let glyphs = [0xE711u16, 0xE738, 0xE710];
    let d = sized(h, 12, 18);
    for i in 0..3 {
        let s = light_slot(h, i as i32);
        let glyph = if hovered && (i < 2 || can_zoom(h)) { Some(glyphs[i]) } else { None };
        draw_light(dc, (s.left + s.right) / 2, th / 2, d, colours[i], glyph, fonts.tiny);
    }
    let n = GetWindowTextLengthW(h) as usize;
    let mut buf = vec![0u16; n + 1];
    let got = GetWindowTextW(h, &mut buf) as usize;
    let side = light_slot(h, 2).right + scale(h, 8);
    let mut tr = RECT { left: side, top: 0, right: (r.right - side).max(side + 1), bottom: th };
    let old = SelectObject(dc, fonts.bold.into());
    SetBkMode(dc, TRANSPARENT);
    SetTextColor(dc, rgb(if dark { 0xE5E5EA } else { 0x3C3C43 }));
    DrawTextW(dc, &mut buf[..got], &mut tr, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS);
    SelectObject(dc, old);
}

/// Replaces the Windows title bar with a macOS-style strip while the macOS theme is active.
pub unsafe fn frame_message(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
    if msg == WM_DESTROY {
        FRAME.with(|f| f.borrow_mut().remove(&(h.0 as isize)));
        return None;
    }
    if !is_mac() {
        return None;
    }
    let key = h.0 as isize;
    match msg {
        WM_NCCALCSIZE if wp.0 != 0 => {
            // Keep the side and bottom resize borders but give the caption area to the client.
            let p = lp.0 as *mut NCCALCSIZE_PARAMS;
            let top = (*p).rgrc[0].top;
            DefWindowProcW(h, msg, wp, lp);
            let dpi = windows::Win32::UI::HiDpi::GetDpiForWindow(h);
            let frame = GetSystemMetricsForDpi(SM_CYSIZEFRAME, dpi) + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi);
            (*p).rgrc[0].top = if IsZoomed(h).as_bool() { top + frame } else { top };
            Some(LRESULT(0))
        }
        WM_NCHITTEST => {
            let r = DefWindowProcW(h, msg, wp, lp);
            if r.0 as u32 != HTCLIENT {
                return Some(r);
            }
            let mut pt = POINT { x: (lp.0 & 0xFFFF) as i16 as i32, y: ((lp.0 >> 16) & 0xFFFF) as i16 as i32 };
            let _ = windows::Win32::Graphics::Gdi::ScreenToClient(h, &mut pt);
            if pt.y >= top_inset(h) || light_at(h, pt.x, pt.y).is_some() {
                return Some(r);
            }
            let top_edge = !IsZoomed(h).as_bool() && pt.y < sized(h, 6, 10);
            Some(LRESULT(if top_edge { HTTOP } else { HTCAPTION } as isize))
        }
        WM_MOUSEMOVE => {
            let (x, y) = ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32);
            let over = light_at(h, x, y).is_some();
            let (was, tracking) = FRAME.with(|f| f.borrow().get(&key).copied().unwrap_or((false, false)));
            if !tracking {
                let mut t = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: h, dwHoverTime: 0 };
                let _ = TrackMouseEvent(&mut t);
            }
            FRAME.with(|f| f.borrow_mut().insert(key, (over, true)));
            if was != over {
                let _ = InvalidateRect(Some(h), Some(&strip_rect(h)), true);
            }
            None
        }
        WM_MOUSELEAVE => {
            let was = FRAME.with(|f| f.borrow_mut().insert(key, (false, false))).map(|s| s.0).unwrap_or(false);
            if was {
                let _ = InvalidateRect(Some(h), Some(&strip_rect(h)), true);
            }
            None
        }
        WM_LBUTTONUP => {
            let (x, y) = ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32);
            match light_at(h, x, y)? {
                0 => {
                    let _ = PostMessageW(Some(h), WM_CLOSE, WPARAM(0), LPARAM(0));
                }
                1 => {
                    let _ = ShowWindow(h, SW_MINIMIZE);
                }
                _ if can_zoom(h) => {
                    let _ = ShowWindow(h, if IsZoomed(h).as_bool() { SW_RESTORE } else { SW_MAXIMIZE });
                }
                _ => {}
            }
            Some(LRESULT(0))
        }
        WM_SETTEXT => {
            let r = DefWindowProcW(h, msg, wp, lp);
            let _ = InvalidateRect(Some(h), Some(&strip_rect(h)), true);
            Some(r)
        }
        _ => None,
    }
}
