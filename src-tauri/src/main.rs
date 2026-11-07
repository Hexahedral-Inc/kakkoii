#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(windows)]
mod api;
#[cfg(windows)]
mod audio;
#[cfg(windows)]
mod engine;
#[cfg(windows)]
mod mp4tail;
#[cfg(windows)]
mod recorder;

#[cfg(windows)]
use std::sync::Arc;

#[cfg(windows)]
use tauri::{Manager, State};

#[cfg(windows)]
use engine::{Engine, StartOpts};

// Commands that open windows or block on the encoder run async, off the main thread:
// a sync command on Windows would deadlock building the camera window.

#[cfg(windows)]
#[tauri::command]
async fn start_recording(engine: State<'_, Arc<Engine>>, opts: serde_json::Value) -> Result<serde_json::Value, String> {
    let opts: StartOpts = serde_json::from_value(opts).map_err(|e| e.to_string())?;
    engine.start(opts)
}

#[cfg(windows)]
#[tauri::command]
async fn stop_recording(engine: State<'_, Arc<Engine>>) -> Result<serde_json::Value, String> {
    recorder::log("stop via window");
    engine.stop()
}

#[cfg(windows)]
#[tauri::command]
async fn set_camera(engine: State<'_, Arc<Engine>>, on: bool) -> Result<serde_json::Value, String> {
    engine.set_camera(on)
}

#[cfg(windows)]
#[tauri::command]
fn status(engine: State<Arc<Engine>>) -> serde_json::Value {
    engine.status()
}

#[cfg(windows)]
#[tauri::command]
async fn targets(engine: State<'_, Arc<Engine>>) -> Result<serde_json::Value, String> {
    Ok(engine.targets())
}

#[cfg(windows)]
#[tauri::command]
async fn recordings(engine: State<'_, Arc<Engine>>) -> Result<serde_json::Value, String> {
    engine.recordings()
}

#[cfg(windows)]
#[tauri::command]
fn agent_command() -> String {
    api::mcp_command()
}

#[cfg(windows)]
#[tauri::command]
fn open_folder(engine: State<Arc<Engine>>) {
    if let Ok(dir) = engine.dir() {
        let _ = std::process::Command::new("explorer").arg(dir).spawn();
    }
}

#[cfg(windows)]
#[tauri::command]
fn reveal(path: String) {
    let _ = std::process::Command::new("explorer").arg(format!("/select,{path}")).spawn();
}

/// What is behind a glass window, as a JPEG, for glass.js (Sage's glass_frame):
/// the window is left out of the capture for the instant it takes, then put back,
/// so your own screenshots still show it.
#[cfg(windows)]
#[tauri::command]
async fn glass_frame(app: tauri::AppHandle, label: String) -> Result<tauri::ipc::Response, String> {
    let win = app.get_webview_window(&label).ok_or("no such window")?;
    let pos = win.outer_position().map_err(|e| e.to_string())?;
    let size = win.outer_size().map_err(|e| e.to_string())?;
    let hwnd = win.hwnd().map_err(|e| e.to_string())?.0 as isize;
    let bytes = tauri::async_runtime::spawn_blocking(move || -> Result<Vec<u8>, String> {
        use windows_sys::Win32::UI::WindowsAndMessaging::{SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE, WDA_NONE};
        unsafe { SetWindowDisplayAffinity(hwnd as _, WDA_EXCLUDEFROMCAPTURE) };
        // Two compositor frames, so the capture is composed without us.
        std::thread::sleep(std::time::Duration::from_millis(48));
        let shot = (|| -> Result<image::RgbaImage, String> {
        let m = xcap::Monitor::from_point(pos.x + 2, pos.y + 2).map_err(|e| e.to_string())?;
        let (mx, my) = (m.x().map_err(|e| e.to_string())?, m.y().map_err(|e| e.to_string())?);
        let (mw, mh) = (m.width().map_err(|e| e.to_string())?, m.height().map_err(|e| e.to_string())?);
        let x = (pos.x - mx).clamp(0, mw as i32 - 1) as u32;
        let y = (pos.y - my).clamp(0, mh as i32 - 1) as u32;
        let w = size.width.min(mw - x).max(1);
        let h = size.height.min(mh - y).max(1);
        m.capture_region(x, y, w, h).map_err(|e| e.to_string())
        })();
        unsafe { SetWindowDisplayAffinity(hwnd as _, WDA_NONE) };
        let rgb = image::DynamicImage::ImageRgba8(shot?).to_rgb8();
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 80).encode_image(&rgb).map_err(|e| e.to_string())?;
        Ok(out)
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(tauri::ipc::Response::new(bytes))
}

/// Keeps a window out of every capture, so the controls never end up in the video.
#[cfg(windows)]
pub(crate) fn hide_from_capture(window: &tauri::WebviewWindow) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE};
    if let Ok(hwnd) = window.hwnd() {
        unsafe { SetWindowDisplayAffinity(hwnd.0 as _, WDA_EXCLUDEFROMCAPTURE) };
    }
}

#[cfg(windows)]
fn main() {
    use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

    // `kakkoii --selftest <secs> <out.mp4> [window query]` records headless and exits.
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--selftest") {
        let secs: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(3);
        let out = args.get(3).cloned().unwrap_or_else(|| "selftest.mp4".into());
        let target = match args.get(4) {
            Some(q) => recorder::Target::Window { query: q.clone() },
            None => recorder::Target::Screen,
        };
        let session = recorder::start(&target, recorder::Quality::Balanced, recorder::Sound::default(), out).expect("start");
        std::thread::sleep(std::time::Duration::from_secs(secs));
        println!("{}", session.stop().expect("stop"));
        return;
    }

    // Alt+Shift+R: open the bar, or stop if we're recording.
    let hotkey = Shortcut::new(Some(Modifiers::ALT | Modifiers::SHIFT), Code::KeyR);

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Some(e) = app.try_state::<Arc<Engine>>() {
                e.show_bar();
            }
        }))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |app, sc, ev| {
                    if sc == &hotkey && ev.state() == ShortcutState::Pressed {
                        if let Some(e) = app.try_state::<Arc<Engine>>() {
                            let e = e.inner().clone();
                            std::thread::spawn(move || {
                                if e.recording() {
                                    recorder::log("stop via hotkey");
                                    let _ = e.stop();
                                } else {
                                    e.show_bar();
                                }
                            });
                        }
                    }
                })
                .build(),
        )
        .setup(move |app| {
            let engine = Arc::new(Engine::new(app.handle().clone()));
            app.manage(engine.clone());
            let config = std::env::var("APPDATA").map(std::path::PathBuf::from).unwrap_or_else(|_| std::env::temp_dir()).join("Kakkoii");
            api::serve(engine, config);
            let _ = app.global_shortcut().register(hotkey);

            // The bar sits bottom centre, like Sage's.
            let bar = app.get_webview_window("main").unwrap();
            if let Ok(Some(m)) = bar.primary_monitor() {
                let s = m.scale_factor();
                let r = m.work_area();
                let (ax, ay, aw, ah) = (r.position.x as f64 / s, r.position.y as f64 / s, r.size.width as f64 / s, r.size.height as f64 / s);
                let _ = bar.set_position(tauri::LogicalPosition::new(ax + (aw - 700.0) / 2.0, ay + ah - 132.0 - 26.0));
            }
            // The bar shows in your screenshots; it hides itself whenever a recording starts.
            let _ = bar.show();
            let _ = bar.set_focus();
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            start_recording, stop_recording, set_camera, status, targets, recordings,
            agent_command, open_folder, reveal, glass_frame
        ])
        .run(tauri::generate_context!())
        .expect("error while running kakkoii");
}

#[cfg(not(windows))]
fn main() {
    eprintln!("Kakkoii records on Windows for now. macOS and Linux are on the plan.");
}
