#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(windows)]
mod recorder;

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::{Manager, State};

#[derive(Default)]
struct AppState {
    #[cfg(windows)]
    session: Mutex<Option<recorder::Session>>,
}

#[tauri::command]
fn start_recording(app: tauri::AppHandle, state: State<AppState>) -> Result<String, String> {
    let dir = app.path().video_dir().map_err(|e| e.to_string())?.join("Kakkoii");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let path = dir.join(format!("kakkoii-{stamp}.mp4")).to_string_lossy().into_owned();

    #[cfg(windows)]
    {
        let mut slot = state.session.lock().unwrap();
        if slot.is_some() {
            return Err("already recording".into());
        }
        *slot = Some(recorder::start(path.clone())?);
    }
    #[cfg(not(windows))]
    let _ = state;
    Ok(path)
}

#[tauri::command]
fn stop_recording(state: State<AppState>) -> Result<String, String> {
    #[cfg(windows)]
    {
        let session = state.session.lock().unwrap().take().ok_or("not recording")?;
        return session.stop();
    }
    #[cfg(not(windows))]
    {
        let _ = state;
        Err("recording is Windows-only for now".into())
    }
}

#[tauri::command]
fn reveal(path: String) {
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(format!("/select,{path}")).spawn();
    #[cfg(not(windows))]
    let _ = path;
}

/// Keeps our own window out of every capture, so the controls never end up in the video.
#[cfg(windows)]
fn hide_from_capture(window: &tauri::WebviewWindow) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE};
    if let Ok(hwnd) = window.hwnd() {
        unsafe { SetWindowDisplayAffinity(hwnd.0 as _, WDA_EXCLUDEFROMCAPTURE) };
    }
}

fn main() {
    // `kakkoii --selftest <secs> <out.mp4>` records the screen headless and exits.
    #[cfg(windows)]
    {
        let args: Vec<String> = std::env::args().collect();
        if args.get(1).map(String::as_str) == Some("--selftest") {
            let secs: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(3);
            let out = args.get(3).cloned().unwrap_or_else(|| "selftest.mp4".into());
            let session = recorder::start(out).expect("start");
            std::thread::sleep(std::time::Duration::from_secs(secs));
            println!("{}", session.stop().expect("stop"));
            return;
        }
    }

    tauri::Builder::default()
        .manage(AppState::default())
        .setup(|app| {
            let window = app.get_webview_window("main").unwrap();
            #[cfg(windows)]
            {
                // Mica first (Windows 11), acrylic as the fallback. The compositor draws it: zero CPU for us.
                if window_vibrancy::apply_mica(&window, Some(true)).is_err() {
                    let _ = window_vibrancy::apply_acrylic(&window, Some((18, 18, 20, 160)));
                }
                hide_from_capture(&window);
            }
            #[cfg(target_os = "macos")]
            let _ = window_vibrancy::apply_vibrancy(
                &window,
                window_vibrancy::NSVisualEffectMaterial::HudWindow,
                None,
                Some(16.0),
            );
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![start_recording, stop_recording, reveal])
        .run(tauri::generate_context!())
        .expect("error while running kakkoii");
}
