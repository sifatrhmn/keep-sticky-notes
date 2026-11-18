//! Drawing: a canvas window for sketching in a note, plus GDI+ helpers for drawing thumbnails.
//!
//! Drawings made here are stored on this PC only. Drawings made in Google Keep are downloaded
//! and shown read-only, because the unofficial protocol offers no way to upload new ones.

use super::form;
use super::{app, hinstance, rgb, APP_NAME};
use keepsticky::store::Drawing;
use serde_json::{json, Value};
use std::cell::RefCell;
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Graphics::GdiPlus::*;
use std::result::Result::Ok;
/// GDI+ 32-bit ARGB pixel format.
const PIXEL_FORMAT_32BPP_ARGB: i32 = 0x26200A;
use windows::Win32::System::Com::{IStream, STATFLAG_NONAME, STATSTG, STREAM_SEEK_SET};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, ReleaseCapture, SetCapture, VK_CONTROL};
use windows::Win32::UI::Shell::SHCreateMemStream;
use windows::Win32::UI::WindowsAndMessaging::*;

const CLASS: PCWSTR = w!("KeepStickyNotes_Draw");
/// The canvas is stored in these logical units whatever the window size.
pub const CANVAS_W: f32 = 1000.0;
pub const CANVAS_H: f32 = 700.0;

const TOOLS: [&str; 4] = ["Pen", "Marker", "Highlighter", "Eraser"];
const INKS: [u32; 7] = [0x202124, 0xD93025, 0xF29900, 0x188038, 0x1A73E8, 0x9334E6, 0xFFFFFF];
const SIZES: [f32; 3] = [2.0, 5.0, 10.0];
const ERASER: usize = 3;

#[derive(Clone, Copy, PartialEq)]
enum Hit {
    Tool(usize),
    Ink(usize),
    Size(usize),
    Undo,
    Clear,
    Delete,
    Copy,
    Done,
}

#[derive(Clone)]
struct Stroke {
    tool: usize,
    ink: u32,
    width: f32,
    points: Vec<(f32, f32)>,
}

struct Editor {
    hwnd: HWND,
    note_id: String,
    drawing_id: Option<i64>,
    read_only: bool,
    background: Vec<u8>,
    bg: Option<Image>,
    strokes: Vec<Stroke>,
    current: Option<Stroke>,
    tool: usize,
    ink: usize,
    size: usize,
    changed: bool,
    erasing: bool,
}

thread_local! {
    static EDITOR: RefCell<Option<Editor>> = const { RefCell::new(None) };
}

// ----- GDI+ helpers -----

pub fn startup() {
    unsafe {
        let mut token = 0usize;
        let input = GdiplusStartupInput { GdiplusVersion: 1, ..Default::default() };
        let _ = GdiplusStartup(&mut token, &input, std::ptr::null_mut());
    }
}

/// A decoded image (PNG or any format GDI+ reads).
pub struct Image(*mut GpImage);

impl Image {
    pub fn from_bytes(bytes: &[u8]) -> Option<Image> {
        if bytes.is_empty() {
            return None;
        }
        unsafe {
            let stream: IStream = SHCreateMemStream(Some(bytes))?;
            let mut bmp: *mut GpBitmap = std::ptr::null_mut();
            if GdipCreateBitmapFromStream(&stream, &mut bmp) != Status(0) || bmp.is_null() {
                return None;
            }
            Some(Image(bmp as *mut GpImage))
        }
    }

    pub fn size(&self) -> (u32, u32) {
        let (mut w, mut h) = (0u32, 0u32);
        unsafe {
            GdipGetImageWidth(self.0, &mut w);
            GdipGetImageHeight(self.0, &mut h);
        }
        (w, h)
    }

    /// Draws the image scaled to fit inside `r`, keeping its proportions.
    pub unsafe fn draw_fit(&self, hdc: HDC, r: RECT) {
        let (iw, ih) = self.size();
        if iw == 0 || ih == 0 {
            return;
        }
        let (rw, rh) = ((r.right - r.left) as f32, (r.bottom - r.top) as f32);
        let k = (rw / iw as f32).min(rh / ih as f32);
        let (w, h) = (iw as f32 * k, ih as f32 * k);
        let mut g: *mut GpGraphics = std::ptr::null_mut();
        GdipCreateFromHDC(hdc, &mut g);
        GdipSetInterpolationMode(g, InterpolationModeHighQualityBicubic);
        GdipDrawImageRect(g, self.0, r.left as f32 + (rw - w) / 2.0, r.top as f32 + (rh - h) / 2.0, w, h);
        GdipDeleteGraphics(g);
    }
}

impl Drop for Image {
    fn drop(&mut self) {
        unsafe {
            GdipDisposeImage(self.0);
        }
    }
}

fn argb(colour: u32, alpha: u8) -> u32 {
    ((alpha as u32) << 24) | (colour & 0xFFFFFF)
}

/// Renders background and strokes onto `g`, mapping the canvas into the given rectangle.
unsafe fn render(g: *mut GpGraphics, ox: f32, oy: f32, k: f32, bg: Option<&Image>, strokes: &[Stroke]) {
    GdipSetSmoothingMode(g, SmoothingModeAntiAlias);
    let mut white: *mut GpSolidFill = std::ptr::null_mut();
    GdipCreateSolidFill(0xFFFFFFFF, &mut white);
    GdipFillRectangle(g, white as *mut GpBrush, ox, oy, CANVAS_W * k, CANVAS_H * k);
    GdipDeleteBrush(white as *mut GpBrush);
    if let Some(img) = bg {
        let (iw, ih) = img.size();
        if iw > 0 && ih > 0 {
            let s = (CANVAS_W / iw as f32).min(CANVAS_H / ih as f32);
            let (w, h) = (iw as f32 * s, ih as f32 * s);
            GdipSetInterpolationMode(g, InterpolationModeHighQualityBicubic);
            GdipDrawImageRect(g, img.0, ox + (CANVAS_W - w) / 2.0 * k, oy + (CANVAS_H - h) / 2.0 * k, w * k, h * k);
        }
    }
    for s in strokes {
        let (alpha, width) = match s.tool {
            1 => (255, s.width * 2.2),
            2 => (96, s.width * 4.0),
            _ => (255, s.width),
        };
        let mut pen: *mut GpPen = std::ptr::null_mut();
        GdipCreatePen1(argb(swap_rb(s.ink), alpha), width * k, UnitPixel, &mut pen);
        GdipSetPenStartCap(pen, LineCapRound);
        GdipSetPenEndCap(pen, LineCapRound);
        GdipSetPenLineJoin(pen, LineJoinRound);
        let pts: Vec<PointF> = s.points.iter().map(|(x, y)| PointF { X: ox + x * k, Y: oy + y * k }).collect();
        if pts.len() == 1 {
            let p = pts[0];
            GdipDrawLine(g, pen, p.X, p.Y, p.X + 0.1, p.Y + 0.1);
        } else if pts.len() > 1 {
            GdipDrawLines(g, pen, pts.as_ptr(), pts.len() as i32);
        }
        GdipDeletePen(pen);
    }
}

/// Colours are kept as 0xRRGGBB; GDI+ wants ARGB, which is the same layout.
fn swap_rb(c: u32) -> u32 {
    c
}

/// Renders the drawing to a PNG at canvas size.
unsafe fn encode_png(bg: Option<&Image>, strokes: &[Stroke]) -> Option<Vec<u8>> {
    let mut bmp: *mut GpBitmap = std::ptr::null_mut();
    GdipCreateBitmapFromScan0(CANVAS_W as i32, CANVAS_H as i32, 0, PIXEL_FORMAT_32BPP_ARGB, None, &mut bmp);
    if bmp.is_null() {
        return None;
    }
    let mut g: *mut GpGraphics = std::ptr::null_mut();
    GdipGetImageGraphicsContext(bmp as *mut GpImage, &mut g);
    render(g, 0.0, 0.0, 1.0, bg, strokes);
    GdipDeleteGraphics(g);
    let stream: IStream = SHCreateMemStream(None)?;
    // The PNG encoder's class id.
    let png = windows::core::GUID::from_u128(0x557cf406_1a04_11d3_9a73_0000f81ef32e);
    let ok = GdipSaveImageToStream(bmp as *mut GpImage, &stream, &png, std::ptr::null()) == Status(0);
    GdipDisposeImage(bmp as *mut GpImage);
    if !ok {
        return None;
    }
    let mut stat = STATSTG::default();
    stream.Stat(&mut stat, STATFLAG_NONAME).ok()?;
    let len = stat.cbSize as usize;
    stream.Seek(0, STREAM_SEEK_SET, None).ok()?;
    let mut out = vec![0u8; len];
    let mut read = 0u32;
    stream.Read(out.as_mut_ptr() as *mut _, len as u32, Some(&mut read)).ok().ok()?;
    out.truncate(read as usize);
    Some(out)
}

fn strokes_to_json(strokes: &[Stroke]) -> String {
    let v: Vec<Value> = strokes
        .iter()
        .map(|s| {
            let p: Vec<f32> = s.points.iter().flat_map(|(x, y)| [(*x * 10.0).round() / 10.0, (*y * 10.0).round() / 10.0]).collect();
            json!({"t": s.tool, "c": s.ink, "w": s.width, "p": p})
        })
        .collect();
    Value::Array(v).to_string()
}

fn strokes_from_json(s: &str) -> Vec<Stroke> {
    let Ok(Value::Array(a)) = serde_json::from_str::<Value>(s) else { return Vec::new() };
    a.iter()
        .filter_map(|v| {
            let p = v.get("p")?.as_array()?;
            let pts: Vec<f32> = p.iter().filter_map(|n| n.as_f64().map(|f| f as f32)).collect();
            Some(Stroke {
                tool: v.get("t")?.as_u64()? as usize,
                ink: v.get("c")?.as_u64()? as u32,
                width: v.get("w")?.as_f64()? as f32,
                points: pts.chunks(2).filter(|c| c.len() == 2).map(|c| (c[0], c[1])).collect(),
            })
        })
        .collect()
}

// ----- the editor window -----

pub fn register() {
    unsafe {
        let wc = WNDCLASSW {
            lpfnWndProc: Some(proc_),
            hInstance: hinstance(),
            lpszClassName: CLASS,
            hCursor: LoadCursorW(None, IDC_CROSS).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassW(&wc);
    }
}

/// Opens the canvas for a note: a new drawing when `drawing` is `None`.
pub fn open(note_id: &str, drawing: Option<&Drawing>) {
    let existing = EDITOR.with(|e| e.borrow().as_ref().map(|e| e.hwnd));
    if let Some(h) = existing {
        finish(true);
        unsafe {
            let _ = DestroyWindow(h);
        }
    }
    let read_only = drawing.is_some_and(|d| d.from_keep());
    let (background, strokes) = match drawing {
        Some(d) if d.from_keep() => (d.png.clone(), Vec::new()),
        Some(d) => (d.background.clone(), strokes_from_json(&d.strokes)),
        None => (Vec::new(), Vec::new()),
    };
    let title = if read_only { format!("{APP_NAME}: Drawing from Google Keep") } else { format!("{APP_NAME}: Drawing") };
    unsafe {
        let Ok(h) = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS,
            &HSTRING::from(title),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            900,
            700,
            None,
            None,
            Some(hinstance()),
            None,
        ) else {
            return;
        };
        EDITOR.with(|e| {
            *e.borrow_mut() = Some(Editor {
                hwnd: h,
                note_id: note_id.to_string(),
                drawing_id: drawing.filter(|d| !d.from_keep()).map(|d| d.id),
                read_only,
                bg: Image::from_bytes(&background),
                background,
                strokes,
                current: None,
                tool: 0,
                ink: 0,
                size: 1,
                changed: false,
                erasing: false,
            })
        });
        form::fit_client(h, 960, form::scale(h, 720));
        let _ = ShowWindow(h, SW_SHOW);
        let _ = SetForegroundWindow(h);
    }
}

/// Toolbar buttons and their rectangles. Tools wrap onto more rows in a narrow window.
unsafe fn buttons(h: HWND, ed: &Editor) -> Vec<(Hit, RECT, String)> {
    let mut c = RECT::default();
    let _ = GetClientRect(h, &mut c);
    let pad = form::scale(h, 6);
    let gap = form::scale(h, 4);
    let item_h = form::sized(h, 36, 48);
    let done_w = form::sized(h, 80, 96);
    let right_n = if ed.drawing_id.is_some() { 2 } else { 1 };
    let limit = c.right - pad - right_n * (done_w + gap);
    let (mut x, mut y) = (pad, pad);
    let mut out = Vec::new();
    // `need` is the width of the whole group at its first button, so groups wrap together.
    let mut push = |hit: Hit, w: i32, label: &str, need: i32, x: &mut i32, y: &mut i32| {
        if need > 0 && *x > pad {
            *x += gap * 2;
        }
        if *x > pad && *x + need.max(w) > limit {
            *x = pad;
            *y += item_h + gap;
        }
        out.push((hit, RECT { left: *x, top: *y, right: *x + w, bottom: *y + item_h }, label.to_string()));
        *x += w + gap;
    };
    let wide = form::sized(h, 92, 110);
    if ed.read_only {
        if ed.bg.is_some() {
            push(Hit::Copy, form::sized(h, 150, 180), "Edit a copy here", 0, &mut x, &mut y);
        }
    } else {
        let group = |n: i32, w: i32| n * w + (n - 1) * gap;
        for (i, t) in TOOLS.iter().enumerate() {
            push(Hit::Tool(i), wide, t, 0, &mut x, &mut y);
        }
        for i in 0..INKS.len() {
            push(Hit::Ink(i), item_h, "", if i == 0 { group(INKS.len() as i32, item_h) } else { 0 }, &mut x, &mut y);
        }
        for (i, l) in ["S", "M", "L"].iter().enumerate() {
            push(Hit::Size(i), item_h, l, if i == 0 { group(3, item_h) } else { 0 }, &mut x, &mut y);
        }
        let uw = form::sized(h, 70, 84);
        push(Hit::Undo, uw, "Undo", group(2, uw), &mut x, &mut y);
        push(Hit::Clear, uw, "Clear", 0, &mut x, &mut y);
    }
    // Right-aligned actions on the first row.
    let mut rx = c.right - pad - done_w;
    out.push((Hit::Done, RECT { left: rx, top: pad, right: rx + done_w, bottom: pad + item_h }, if ed.read_only { "Close" } else { "Done" }.into()));
    if ed.drawing_id.is_some() {
        rx -= gap + done_w;
        out.push((Hit::Delete, RECT { left: rx, top: pad, right: rx + done_w, bottom: pad + item_h }, "Delete".into()));
    }
    out
}

/// Height of the toolbar, including any wrapped rows.
unsafe fn bar_h(h: HWND) -> i32 {
    let bottom = EDITOR.with(|e| e.borrow().as_ref().map(|ed| buttons(h, ed).iter().map(|b| b.1.bottom).max().unwrap_or(0)));
    bottom.unwrap_or(form::sized(h, 42, 54)) + form::scale(h, 6)
}

/// Where the canvas sits in the window: origin and scale.
unsafe fn canvas_rect(h: HWND) -> (f32, f32, f32) {
    let mut c = RECT::default();
    let _ = GetClientRect(h, &mut c);
    let m = form::scale(h, 12) as f32;
    let top = bar_h(h) as f32 + m;
    let (aw, ah) = ((c.right as f32 - 2.0 * m).max(10.0), (c.bottom as f32 - top - m).max(10.0));
    let k = (aw / CANVAS_W).min(ah / CANVAS_H);
    (m + (aw - CANVAS_W * k) / 2.0, top + (ah - CANVAS_H * k) / 2.0, k)
}

unsafe fn to_canvas(h: HWND, lp: LPARAM) -> (f32, f32) {
    let (x, y) = ((lp.0 & 0xFFFF) as i16 as f32, ((lp.0 >> 16) & 0xFFFF) as i16 as f32);
    let (ox, oy, k) = canvas_rect(h);
    (((x - ox) / k).clamp(0.0, CANVAS_W), ((y - oy) / k).clamp(0.0, CANVAS_H))
}

unsafe fn paint(h: HWND, target: HDC, ed: &Editor) {
    let mut c = RECT::default();
    let _ = GetClientRect(h, &mut c);
    let dc = CreateCompatibleDC(Some(target));
    let bmp = CreateCompatibleBitmap(target, c.right.max(1), c.bottom.max(1));
    let old_bmp = SelectObject(dc, bmp.into());
    let theme = app().theme.get();
    let dark = theme.is_dark();
    let bg = CreateSolidBrush(rgb(super::theme::chrome_bg(theme)));
    FillRect(dc, &c, bg);
    let _ = DeleteObject(bg.into());

    let (ox, oy, k) = canvas_rect(h);
    let mut g: *mut GpGraphics = std::ptr::null_mut();
    GdipCreateFromHDC(dc, &mut g);
    let mut all = ed.strokes.clone();
    if let Some(s) = &ed.current {
        all.push(s.clone());
    }
    render(g, ox, oy, k, ed.bg.as_ref(), &all);
    GdipDeleteGraphics(g);
    if ed.read_only && ed.bg.is_none() {
        let f = app().fonts_for(h);
        let old = SelectObject(dc, f.ui.into());
        SetBkMode(dc, TRANSPARENT);
        SetTextColor(dc, rgb(0x5F6368));
        let mut w: Vec<u16> = "This drawing has not been downloaded from Google Keep yet. It appears after the next sync.".encode_utf16().collect();
        let mut tr = RECT { left: ox as i32, top: oy as i32, right: (ox + CANVAS_W * k) as i32, bottom: (oy + CANVAS_H * k) as i32 };
        DrawTextW(dc, &mut w, &mut tr, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS);
        SelectObject(dc, old);
    }

    let f = app().fonts_for(h);
    let old_font = SelectObject(dc, f.ui.into());
    SetBkMode(dc, TRANSPARENT);
    let text = if dark { 0xF0F0F0 } else { 0x202124 };
    let face = if dark { 0x2C2C2E } else { 0xFFFFFF };
    let edge = if dark { 0x48484A } else { 0xD0D0D5 };
    let accent = 0x1A73E8;
    let rad = form::scale(h, 10);
    for (hit, r, label) in buttons(h, ed) {
        let on = match hit {
            Hit::Tool(i) => ed.tool == i,
            Hit::Ink(i) => ed.ink == i,
            Hit::Size(i) => ed.size == i,
            _ => false,
        };
        let fill = match hit {
            Hit::Ink(i) => INKS[i],
            Hit::Done if !ed.read_only => accent,
            _ if on => if dark { 0x0A3D7A } else { 0xD2E3FC },
            _ => face,
        };
        let brush = CreateSolidBrush(rgb(fill));
        let pen = CreatePen(PS_SOLID, if on { form::scale(h, 3) } else { 1 }, rgb(if on { accent } else { edge }));
        let ob = SelectObject(dc, brush.into());
        let op = SelectObject(dc, pen.into());
        if matches!(hit, Hit::Ink(_)) {
            let _ = Ellipse(dc, r.left, r.top, r.right, r.bottom);
        } else {
            let _ = RoundRect(dc, r.left, r.top, r.right, r.bottom, rad, rad);
        }
        SelectObject(dc, ob);
        SelectObject(dc, op);
        let _ = DeleteObject(brush.into());
        let _ = DeleteObject(pen.into());
        if !label.is_empty() {
            SetTextColor(dc, rgb(if matches!(hit, Hit::Done) && !ed.read_only { 0xFFFFFF } else { text }));
            let mut w: Vec<u16> = label.encode_utf16().collect();
            let mut tr = r;
            DrawTextW(dc, &mut w, &mut tr, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
        }
    }
    if ed.read_only {
        SetTextColor(dc, rgb(if dark { 0x98989D } else { 0x5F6368 }));
        let mut w: Vec<u16> = "Drawn in Google Keep. It is read-only here; edit it in Google Keep, or edit a copy on this PC.".encode_utf16().collect();
        let first = buttons(h, ed).into_iter().next().expect("Done button");
        let left = if first.0 == Hit::Copy { first.1.right + form::scale(h, 12) } else { form::scale(h, 12) };
        let mut tr = RECT { left, top: first.1.top, right: c.right - form::sized(h, 100, 120), bottom: first.1.bottom };
        DrawTextW(dc, &mut w, &mut tr, DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS);
    }
    SelectObject(dc, old_font);
    let _ = BitBlt(target, 0, 0, c.right, c.bottom, Some(dc), 0, 0, SRCCOPY);
    SelectObject(dc, old_bmp);
    let _ = DeleteObject(bmp.into());
    let _ = DeleteDC(dc);
}

fn erase_at(ed: &mut Editor, p: (f32, f32)) {
    let r = 12.0;
    let before = ed.strokes.len();
    ed.strokes.retain(|s| !s.points.iter().any(|q| (q.0 - p.0).powi(2) + (q.1 - p.1).powi(2) < r * r + s.width * s.width));
    if ed.strokes.len() != before {
        ed.changed = true;
    }
}

/// Saves the drawing (if it changed) and tells the note to show the new thumbnail.
fn finish(save: bool) {
    let Some(ed) = EDITOR.with(|e| e.borrow_mut().take()) else { return };
    if save && ed.changed && !ed.read_only {
        let store = &app().store;
        let empty = ed.strokes.is_empty() && ed.background.is_empty();
        match ed.drawing_id {
            Some(id) if empty => {
                let _ = store.delete_drawing(id);
            }
            Some(id) => {
                if let Some(png) = unsafe { encode_png(ed.bg.as_ref(), &ed.strokes) } {
                    let _ = store.save_drawing(id, &strokes_to_json(&ed.strokes), &png);
                }
            }
            None if empty => {}
            None => {
                if let Some(png) = unsafe { encode_png(ed.bg.as_ref(), &ed.strokes) } {
                    let _ = store.add_drawing(&ed.note_id, &strokes_to_json(&ed.strokes), &png, &ed.background);
                }
            }
        }
    }
    super::note::reload_drawings(&ed.note_id);
}

fn with_editor<R>(f: impl FnOnce(&mut Editor) -> R) -> Option<R> {
    EDITOR.with(|e| e.borrow_mut().as_mut().map(f))
}

unsafe fn click(h: HWND, hit: Hit) {
    match hit {
        Hit::Done => {
            let _ = DestroyWindow(h);
        }
        Hit::Delete => {
            let r = MessageBoxW(Some(h), w!("Delete this drawing?"), w!("Delete drawing"), MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2);
            if r == IDYES {
                if let Some(Some(id)) = with_editor(|e| {
                    e.changed = false;
                    e.drawing_id
                }) {
                    let _ = app().store.delete_drawing(id);
                }
                let _ = DestroyWindow(h);
            }
        }
        Hit::Copy => {
            with_editor(|e| {
                e.read_only = false;
                e.changed = true;
            });
            let _ = SetWindowTextW(h, &HSTRING::from(format!("{APP_NAME}: Drawing (copy)")));
        }
        _ => {
            with_editor(|e| match hit {
                Hit::Tool(i) => e.tool = i,
                Hit::Ink(i) => {
                    e.ink = i;
                    if e.tool == ERASER {
                        e.tool = 0;
                    }
                }
                Hit::Size(i) => e.size = i,
                Hit::Undo => {
                    if e.strokes.pop().is_some() {
                        e.changed = true;
                    }
                }
                Hit::Clear => {
                    if !e.strokes.is_empty() {
                        e.strokes.clear();
                        e.changed = true;
                    }
                }
                _ => {}
            });
        }
    }
    let _ = InvalidateRect(Some(h), None, false);
}

extern "system" fn proc_(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let dc = BeginPaint(h, &mut ps);
                EDITOR.with(|e| {
                    if let Some(ed) = e.borrow().as_ref() {
                        paint(h, dc, ed);
                    }
                });
                let _ = EndPaint(h, &ps);
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_SIZE => {
                let _ = InvalidateRect(Some(h), None, false);
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                let (x, y) = ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32);
                let hit = EDITOR.with(|e| {
                    e.borrow().as_ref().and_then(|ed| buttons(h, ed).into_iter().find(|(_, r, _)| x >= r.left && x < r.right && y >= r.top && y < r.bottom).map(|b| b.0))
                });
                if let Some(hit) = hit {
                    click(h, hit);
                    return LRESULT(0);
                }
                let p = to_canvas(h, lp);
                with_editor(|e| {
                    if e.read_only {
                        return;
                    }
                    if e.tool == ERASER {
                        e.erasing = true;
                        erase_at(e, p);
                    } else {
                        e.current = Some(Stroke { tool: e.tool, ink: INKS[e.ink], width: SIZES[e.size], points: vec![p] });
                    }
                    SetCapture(h);
                });
                let _ = InvalidateRect(Some(h), None, false);
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                if wp.0 & 0x0001 != 0 {
                    let p = to_canvas(h, lp);
                    let moved = with_editor(|e| {
                        if e.erasing {
                            erase_at(e, p);
                            true
                        } else if let Some(s) = e.current.as_mut() {
                            s.points.push(p);
                            true
                        } else {
                            false
                        }
                    });
                    if moved == Some(true) {
                        let _ = InvalidateRect(Some(h), None, false);
                    }
                }
                LRESULT(0)
            }
            WM_LBUTTONUP | WM_CAPTURECHANGED => {
                with_editor(|e| {
                    e.erasing = false;
                    if let Some(s) = e.current.take() {
                        e.strokes.push(s);
                        e.changed = true;
                    }
                });
                if msg == WM_LBUTTONUP {
                    let _ = ReleaseCapture();
                }
                let _ = InvalidateRect(Some(h), None, false);
                LRESULT(0)
            }
            WM_KEYDOWN => {
                if wp.0 == 'Z' as usize && GetKeyState(VK_CONTROL.0 as i32) < 0 {
                    click(h, Hit::Undo);
                } else if wp.0 == 0x1B {
                    let _ = DestroyWindow(h);
                }
                LRESULT(0)
            }
            WM_GETMINMAXINFO => {
                let info = &mut *(lp.0 as *mut MINMAXINFO);
                info.ptMinTrackSize.x = form::scale(h, 560);
                info.ptMinTrackSize.y = form::scale(h, 400);
                LRESULT(0)
            }
            WM_DPICHANGED => {
                let r = &*(lp.0 as *const RECT);
                let _ = SetWindowPos(h, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
                LRESULT(0)
            }
            WM_DESTROY => {
                let mine = EDITOR.with(|e| e.borrow().as_ref().is_some_and(|ed| ed.hwnd == h));
                if mine {
                    finish(true);
                }
                LRESULT(0)
            }
            _ => DefWindowProcW(h, msg, wp, lp),
        }
    }
}
