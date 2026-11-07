//! Screenshots, the way Sage takes them: the screen freezes under a dither sweep
//! (the veil), you pick a box, a window or all of it, and a card with the shot
//! comes out beside the orb. An agent's screenshot shows the same sweep, so you
//! see it look.

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use image::RgbaImage;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::engine::BROWSER_ARGS;

struct Frame {
    image: RgbaImage,
    /// Where the display is, in physical pixels, and its scale.
    x: i32,
    y: i32,
    scale: f64,
}

#[derive(Default)]
pub struct Shots {
    frame: Mutex<Option<Frame>>,
    mode: Mutex<String>,
    last: Mutex<Option<(RgbaImage, String)>>,
}

fn cursor() -> (i32, i32) {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut p = POINT { x: 0, y: 0 };
    unsafe { GetCursorPos(&mut p) };
    (p.x, p.y)
}

/// The display under the cursor, or the main one.
fn grab(under_cursor: bool) -> Result<Frame, String> {
    let m = if under_cursor {
        let (x, y) = cursor();
        xcap::Monitor::from_point(x, y).ok()
    } else {
        None
    };
    let m = match m {
        Some(m) => m,
        None => {
            let all = xcap::Monitor::all().map_err(|e| e.to_string())?;
            let i = all.iter().position(|m| m.is_primary().unwrap_or(false)).unwrap_or(0);
            all.into_iter().nth(i).ok_or("no display")?
        }
    };
    let image = m.capture_image().map_err(|e| format!("Windows didn't give the screen: {e}"))?;
    Ok(Frame { image, x: m.x().unwrap_or(0), y: m.y().unwrap_or(0), scale: m.scale_factor().unwrap_or(1.0) as f64 })
}

fn save(app: &AppHandle, img: &RgbaImage) -> Result<(String, u64), String> {
    let dir = app.path().picture_dir().map_err(|e| e.to_string())?.join("Kakkoii");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let t = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let path = dir.join(format!("kakkoii-{t}.png"));
    img.save(&path).map_err(|e| e.to_string())?;
    let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    Ok((path.to_string_lossy().into_owned(), bytes))
}

fn open_veil(app: &AppHandle, f: Frame, mode: &str) -> Result<(), String> {
    let s = f.scale;
    let (x, y, w, h) = (f.x as f64 / s, f.y as f64 / s, f.image.width() as f64 / s, f.image.height() as f64 / s);
    let st = app.state::<Shots>();
    *st.frame.lock().unwrap() = Some(f);
    *st.mode.lock().unwrap() = mode.to_string();
    for label in ["veil", "shot"] {
        if let Some(old) = app.get_webview_window(label) {
            let _ = old.close();
        }
    }
    std::thread::sleep(std::time::Duration::from_millis(80));
    let look = mode == "look";
    let win = WebviewWindowBuilder::new(app, "veil", WebviewUrl::App("veil.html".into()))
        .title("Kakkoii capture")
        .inner_size(w, h)
        .position(x, y)
        .decorations(false)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .focused(!look)
        .additional_browser_args(BROWSER_ARGS)
        .build()
        .map_err(|e| e.to_string())?;
    if look {
        let _ = win.set_ignore_cursor_events(true);
    } else {
        // Esc, Enter, R, W, F need the keyboard.
        let _ = win.set_focus();
    }
    crate::hide_from_capture(&win);
    Ok(())
}

/// You asked: "full" takes the screen at once, "region" lets you pick.
pub fn capture(app: &AppHandle, mode: &str) -> Result<(), String> {
    let f = grab(true)?;
    open_veil(app, f, mode)
}

/// An agent asked: saved at once, and the sweep plays over the screen without taking the mouse.
pub fn look(app: &AppHandle) -> Result<Value, String> {
    let f = grab(false)?;
    let (file, bytes) = save(app, &f.image)?;
    let out = json!({ "file": file, "width": f.image.width(), "height": f.image.height(), "bytes": bytes });
    let app2 = app.clone();
    std::thread::spawn(move || {
        let _ = open_veil(&app2, f, "look");
    });
    Ok(out)
}

/// The frozen screen for the veil, as a JPEG no wider than 2400 px.
pub fn veil_image(app: &AppHandle) -> Result<Vec<u8>, String> {
    let st = app.state::<Shots>();
    let guard = st.frame.lock().unwrap();
    let f = guard.as_ref().ok_or("no capture")?;
    let img = image::DynamicImage::ImageRgba8(f.image.clone());
    let img = if f.image.width() > 2400 { img.resize(2400, u32::MAX, image::imageops::FilterType::Triangle) } else { img };
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 82).encode_image(&img.to_rgb8()).map_err(|e| e.to_string())?;
    Ok(out)
}

/// The mode, and the windows on that display (logical px from its corner, front first) for "click a window".
pub fn veil_info(app: &AppHandle) -> Value {
    let st = app.state::<Shots>();
    let mode = st.mode.lock().unwrap().clone();
    let guard = st.frame.lock().unwrap();
    let Some(f) = guard.as_ref() else { return json!({ "mode": mode, "windows": [] }) };
    let (mw, mh) = (f.image.width() as i32, f.image.height() as i32);
    let mut windows = Vec::new();
    for w in xcap::Window::all().unwrap_or_default() {
        let title = w.title().unwrap_or_default();
        if title.is_empty() || title.starts_with("Kakkoii") || w.is_minimized().unwrap_or(true) {
            continue;
        }
        let (x, y) = (w.x().unwrap_or(0) - f.x, w.y().unwrap_or(0) - f.y);
        let (ww, wh) = (w.width().unwrap_or(0) as i32, w.height().unwrap_or(0) as i32);
        if ww < 80 || wh < 60 || x + ww <= 0 || y + wh <= 0 || x >= mw || y >= mh {
            continue;
        }
        let (x0, y0, x1, y1) = (x.max(0), y.max(0), (x + ww).min(mw), (y + wh).min(mh));
        let s = f.scale;
        windows.push(json!({
            "x": x0 as f64 / s, "y": y0 as f64 / s, "w": (x1 - x0) as f64 / s, "h": (y1 - y0) as f64 / s,
            "title": title, "app": w.app_name().unwrap_or_default(),
        }));
    }
    json!({ "mode": mode, "windows": windows })
}

/// The veil is done: crop (a box in its logical px, or None for all of it), save, show the card.
pub fn veil_done(app: &AppHandle, rect: Option<[f64; 4]>) -> Result<Value, String> {
    let st = app.state::<Shots>();
    let f = st.frame.lock().unwrap().take().ok_or("no capture")?;
    if let Some(w) = app.get_webview_window("veil") {
        let _ = w.close();
    }
    let img = match rect {
        Some([x, y, w, h]) => {
            let s = f.scale;
            let (iw, ih) = f.image.dimensions();
            let px = ((x * s).max(0.0) as u32).min(iw.saturating_sub(1));
            let py = ((y * s).max(0.0) as u32).min(ih.saturating_sub(1));
            let pw = ((w * s) as u32).clamp(1, iw - px);
            let ph = ((h * s) as u32).clamp(1, ih - py);
            image::imageops::crop_imm(&f.image, px, py, pw, ph).to_image()
        }
        None => f.image,
    };
    let (file, bytes) = save(app, &img)?;
    let out = json!({ "file": file, "width": img.width(), "height": img.height(), "bytes": bytes });
    *st.last.lock().unwrap() = Some((img, file));
    let _ = app.emit("kakkoii://shot", &out);
    open_card(app)?;
    Ok(out)
}

pub fn veil_cancel(app: &AppHandle) {
    app.state::<Shots>().frame.lock().unwrap().take();
    if let Some(w) = app.get_webview_window("veil") {
        let _ = w.close();
    }
}

/// The card: beside the orb if there's room, else on the other side.
fn open_card(app: &AppHandle) -> Result<(), String> {
    let (w, h) = (392.0, 356.0);
    let m = app.primary_monitor().ok().flatten().ok_or("no display")?;
    let s = m.scale_factor();
    let r = m.work_area();
    let (ax, ay, aw, ah) = (r.position.x as f64 / s, r.position.y as f64 / s, r.size.width as f64 / s, r.size.height as f64 / s);
    let orb = app.get_webview_window("orb").and_then(|o| {
        let p = o.outer_position().ok()?.to_logical::<f64>(s);
        let z = o.outer_size().ok()?.to_logical::<f64>(s);
        o.is_visible().ok()?.then_some((p.x + z.width / 2.0, p.y + z.height / 2.0))
    });
    let (x, y) = match orb {
        Some((ox, oy)) => {
            let x = if ox + 70.0 + w < ax + aw { ox + 70.0 } else { ox - 70.0 - w };
            (x, (oy - h / 2.0).clamp(ay + 10.0, ay + ah - h - 10.0))
        }
        None => (ax + aw - w - 24.0, ay + ah - h - 24.0),
    };
    WebviewWindowBuilder::new(app, "shot", WebviewUrl::App("shot.html".into()))
        .title("Kakkoii shot")
        .inner_size(w, h)
        .position(x, y)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .focused(true)
        .additional_browser_args(BROWSER_ARGS)
        .build()
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn shot_info(app: &AppHandle) -> Value {
    match app.state::<Shots>().last.lock().unwrap().as_ref() {
        Some((img, file)) => json!({ "file": file, "width": img.width(), "height": img.height() }),
        None => Value::Null,
    }
}

pub fn shot_copy(app: &AppHandle) -> Result<(), String> {
    let st = app.state::<Shots>();
    let last = st.last.lock().unwrap();
    let (img, _) = last.as_ref().ok_or("no shot")?;
    let mut cb = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    cb.set_image(arboard::ImageData { width: img.width() as usize, height: img.height() as usize, bytes: std::borrow::Cow::Borrowed(img.as_raw()) })
        .map_err(|e| e.to_string())
}

/// Small pictures of what each open window shows, for the bar's window grid.
/// Minimised windows have nothing to show, so they come back without one.
pub fn thumbs() -> Vec<Value> {
    use base64::Engine as _;
    let mut out = Vec::new();
    for w in xcap::Window::all().unwrap_or_default() {
        let title = w.title().unwrap_or_default();
        let (ww, wh) = (w.width().unwrap_or(0), w.height().unwrap_or(0));
        if title.is_empty() || title.starts_with("Kakkoii") || ww < 160 || wh < 100 {
            continue;
        }
        let process = w.app_name().unwrap_or_default();
        let thumb = if w.is_minimized().unwrap_or(false) {
            None
        } else {
            w.capture_image().ok().and_then(|img| {
                let img = image::DynamicImage::ImageRgba8(img).thumbnail(400, 250).to_rgb8();
                let mut jpg = Vec::new();
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpg, 72).encode_image(&img).ok()?;
                Some(format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(jpg)))
            })
        };
        out.push(json!({ "title": title, "process": process, "thumb": thumb }));
        if out.len() >= 12 {
            break;
        }
    }
    out
}
