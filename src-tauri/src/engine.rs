//! One recorder state shared by the window, the HTTP API and MCP.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::recorder::{self, Quality, Target};

/// Shared by every webview. WebView2 refuses windows whose args differ, so
/// tauri.conf.json carries the same string for the main window.
pub const BROWSER_ARGS: &str = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --use-fake-ui-for-media-stream";

#[derive(Debug, Default, Deserialize)]
pub struct StartOpts {
    #[serde(default)]
    pub target: Target,
    #[serde(default)]
    pub quality: Quality,
    /// System sound on, mic off, unless asked.
    #[serde(default)]
    pub sound: recorder::Sound,
    /// Show the face bubble (it is recorded as part of the screen).
    pub camera: Option<bool>,
    /// Stop on its own after this many seconds.
    pub max_seconds: Option<u64>,
    /// File name without extension. Default: kakkoii-<unix time>.
    pub name: Option<String>,
}

struct Live {
    session: recorder::Session,
    id: u64,
    started: Instant,
    quality: Quality,
    face_only: bool,
}

const ISLAND_W: f64 = 440.0;

#[derive(Default)]
struct Inner {
    live: Option<Live>,
    next_id: u64,
    camera: bool,
    last: Option<String>,
    // What the bar asked for while the area picker is open.
    pending: Option<StartOpts>,
}

pub struct Engine {
    app: AppHandle,
    inner: Mutex<Inner>,
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn safe_name(s: &str) -> String {
    s.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect()
}

impl Engine {
    pub fn new(app: AppHandle) -> Self {
        Self { app, inner: Mutex::new(Inner::default()) }
    }

    pub fn dir(&self) -> Result<PathBuf, String> {
        let dir = self.app.path().video_dir().map_err(|e| e.to_string())?.join("Kakkoii");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        Ok(dir)
    }

    fn status_of(inner: &Inner) -> Value {
        match &inner.live {
            Some(l) => json!({
                "recording": true,
                "target": l.session.label,
                "file": l.session.path,
                "seconds": l.session.recorded(l.started),
                "paused": l.session.is_paused(),
                "quality": l.quality,
                "camera": inner.camera,
            }),
            None => json!({ "recording": false, "camera": inner.camera, "last": inner.last }),
        }
    }

    pub fn status(&self) -> Value {
        let mut inner = self.inner.lock().unwrap();
        // A window we were recording may have closed under us.
        if inner.live.as_ref().is_some_and(|l| l.session.is_finished()) {
            let l = inner.live.take().unwrap();
            inner.last = Some(l.session.path.clone());
        }
        Self::status_of(&inner)
    }

    fn emit(&self) {
        let _ = self.app.emit("kakkoii://state", self.status());
    }

    pub fn start(self: &std::sync::Arc<Self>, mut opts: StartOpts) -> Result<Value, String> {
        let dir = self.dir()?;
        if self.inner.lock().unwrap().live.is_some() {
            return Err("already recording; stop first".into());
        }
        // Just my face: a big camera window, and that window is what we record.
        let face_only = matches!(opts.target, Target::Face);
        if face_only {
            let w = self.camera_window(true)?;
            std::thread::sleep(std::time::Duration::from_millis(900)); // the camera warms up
            opts.target = Target::Hwnd { hwnd: w.hwnd().map_err(|e| e.to_string())?.0 as isize };
        } else if let Some(on) = opts.camera {
            let _ = self.set_camera(on);
        }
        let (id, status) = {
            let mut inner = self.inner.lock().unwrap();
            let name = opts.name.as_deref().map(safe_name).unwrap_or_else(|| format!("kakkoii-{}", now_secs()));
            let path = dir.join(format!("{name}.mp4")).to_string_lossy().into_owned();
            let mut session = recorder::start(&opts.target, opts.quality, opts.sound, path)?;
            if face_only {
                session.label = "Your face".into();
            }
            inner.next_id += 1;
            let id = inner.next_id;
            inner.live = Some(Live { session, id, started: Instant::now(), quality: opts.quality, face_only });
            (id, Self::status_of(&inner))
        };
        self.island(true);
        if let Some(secs) = opts.max_seconds {
            let me = self.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(secs));
                let same = me.inner.lock().unwrap().live.as_ref().is_some_and(|l| l.id == id);
                if same {
                    recorder::log("auto-stop (max_seconds)");
                    let _ = me.stop();
                }
            });
        }
        self.emit();
        Ok(status)
    }

    pub fn stop(&self) -> Result<Value, String> {
        let live = self.inner.lock().unwrap().live.take().ok_or("not recording")?;
        let seconds = live.session.recorded(live.started);
        let path = live.session.stop()?;
        let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        self.inner.lock().unwrap().last = Some(path.clone());
        if live.face_only {
            if let Some(w) = self.app.get_webview_window("camera") {
                let _ = w.close();
            }
            self.inner.lock().unwrap().camera = false;
        }
        let saved = json!({ "file": path, "seconds": seconds, "bytes": bytes });
        self.island(false);
        let _ = self.app.emit("kakkoii://saved", &saved);
        self.emit();
        Ok(saved)
    }

    /// The camera window. Bubble: 200 px, bottom left, recorded as part of the screen.
    /// Full: 960x540 for the face-only recording.
    fn camera_window(&self, full: bool) -> Result<tauri::WebviewWindow, String> {
        if let Some(w) = self.app.get_webview_window("camera") {
            if !full {
                return Ok(w);
            }
            let _ = w.close();
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
        let area = self.app.primary_monitor().ok().flatten().map(|m| {
            let s = m.scale_factor();
            let r = m.work_area();
            (r.position.x as f64 / s, r.position.y as f64 / s, r.size.width as f64 / s, r.size.height as f64 / s)
        });
        let (ax, ay, aw, ah) = area.unwrap_or((0.0, 0.0, 1920.0, 1040.0));
        let (w, h) = if full { (960.0, 540.0) } else { (200.0, 200.0) };
        let (x, y) = if full { (ax + (aw - w) / 2.0, ay + (ah - h) / 2.0) } else { (ax + 24.0, ay + ah - h - 24.0) };
        let url = if full { "camera.html?full" } else { "camera.html" };
        WebviewWindowBuilder::new(&self.app, "camera", WebviewUrl::App(url.into()))
            .title("Kakkoii camera")
            .inner_size(w, h)
            .position(x, y)
            .decorations(false)
            .transparent(!full)
            .shadow(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .resizable(false)
            .focused(false)
            .additional_browser_args(BROWSER_ARGS)
            .build()
            .map_err(|e| e.to_string())
    }

    pub fn set_camera(&self, on: bool) -> Result<Value, String> {
        if on {
            self.camera_window(false)?;
        } else if let Some(w) = self.app.get_webview_window("camera") {
            let _ = w.close();
        }
        self.inner.lock().unwrap().camera = on;
        self.emit();
        Ok(json!({ "camera": on }))
    }

    /// Recording: the bar goes and Sage's island capsule drops in at the top centre.
    /// Done: the island folds away and the bar comes back with the saved file.
    fn island(&self, recording: bool) {
        let island = match self.app.get_webview_window("island") {
            Some(w) => Some(w),
            None if recording => {
                let (x, y) = self
                    .app
                    .primary_monitor()
                    .ok()
                    .flatten()
                    .map(|m| {
                        let s = m.scale_factor();
                        let size = m.size().to_logical::<f64>(s);
                        let pos = m.position().to_logical::<f64>(s);
                        (pos.x + (size.width - ISLAND_W) / 2.0, pos.y)
                    })
                    .unwrap_or((800.0, 0.0));
                WebviewWindowBuilder::new(&self.app, "island", WebviewUrl::App("island.html".into()))
                    .title("Kakkoii island")
                    .inner_size(ISLAND_W, 60.0)
                    .position(x, y)
                    .decorations(false)
                    .transparent(true)
                    .shadow(false)
                    .always_on_top(true)
                    .skip_taskbar(true)
                    .resizable(false)
                    .focused(false)
                    .additional_browser_args(BROWSER_ARGS)
                    .build()
                    .ok()
            }
            None => None,
        };
        if let Some(w) = &island {
            crate::hide_from_capture(w);
            if recording {
                let _ = w.show();
            } else {
                let w = w.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(340)); // let it fold
                    let _ = w.hide();
                });
            }
        }
        if let Some(bar) = self.app.get_webview_window("main") {
            if recording {
                let _ = bar.hide();
            } else {
                self.show_bar();
            }
        }
    }

    pub fn show_bar(&self) {
        if let Some(bar) = self.app.get_webview_window("main") {
            let _ = bar.show();
            let _ = bar.set_focus();
            let _ = self.app.emit_to("main", "kakkoii://open", ());
        }
    }

    /// Pause or carry on with the recording in progress.
    pub fn pause(&self, on: bool) -> Result<Value, String> {
        {
            let inner = self.inner.lock().unwrap();
            let live = inner.live.as_ref().ok_or("not recording")?;
            live.session.set_paused(on);
        }
        self.emit();
        Ok(self.status())
    }

    /// Opens the full-screen picker: the user drags a box, then `area_chosen` starts it.
    pub fn open_area(&self, opts: StartOpts) -> Result<(), String> {
        if self.recording() {
            return Err("already recording; stop first".into());
        }
        self.inner.lock().unwrap().pending = Some(opts);
        if let Some(bar) = self.app.get_webview_window("main") {
            let _ = bar.hide();
        }
        let m = self.app.primary_monitor().ok().flatten().ok_or("no display")?;
        let s = m.scale_factor();
        let size = m.size().to_logical::<f64>(s);
        let pos = m.position().to_logical::<f64>(s);
        if let Some(w) = self.app.get_webview_window("area") {
            let _ = w.close();
        }
        let w = WebviewWindowBuilder::new(&self.app, "area", WebviewUrl::App("area.html".into()))
            .title("Kakkoii area")
            .inner_size(size.width, size.height)
            .position(pos.x, pos.y)
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
        crate::hide_from_capture(&w);
        Ok(())
    }

    /// The picker's answer, in physical pixels from the display's corner. None: cancelled.
    pub fn area_chosen(self: &std::sync::Arc<Self>, rect: Option<(u32, u32, u32, u32)>) -> Result<Value, String> {
        if let Some(w) = self.app.get_webview_window("area") {
            let _ = w.close();
        }
        let pending = self.inner.lock().unwrap().pending.take();
        let Some((x, y, w, h)) = rect else {
            self.show_bar();
            return Ok(json!({ "cancelled": true }));
        };
        let mut opts = pending.unwrap_or_default();
        opts.target = Target::Area { x, y, w, h };
        std::thread::sleep(std::time::Duration::from_millis(120)); // the picker fades before the first frame
        self.start(opts)
    }

    pub fn recording(&self) -> bool {
        self.inner.lock().unwrap().live.is_some()
    }

    pub fn targets(&self) -> Value {
        json!(recorder::targets())
    }

    pub fn recordings(&self) -> Result<Value, String> {
        let dir = self.dir()?;
        let mut files: Vec<Value> = std::fs::read_dir(&dir)
            .map_err(|e| e.to_string())?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "mp4" || x == "gif"))
            .filter_map(|e| {
                let m = e.metadata().ok()?;
                let modified = m.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_secs();
                Some(json!({ "file": e.path().to_string_lossy(), "bytes": m.len(), "modified": modified }))
            })
            .collect();
        files.sort_by_key(|f| std::cmp::Reverse(f["modified"].as_u64().unwrap_or(0)));
        Ok(json!({ "dir": dir.to_string_lossy(), "files": files }))
    }

    /// A lossless copy of `start..end` seconds as <name>-trim.mp4 (then -trim2, ...).
    pub fn trim(&self, file: Option<String>, start: f64, end: f64) -> Result<Value, String> {
        let src = file
            .or_else(|| self.inner.lock().unwrap().last.clone())
            .ok_or("no file given and nothing recorded yet")?;
        let src_path = Path::new(&src);
        if !src_path.exists() {
            return Err(format!("{src} does not exist"));
        }
        let stem = src_path.file_stem().unwrap_or_default().to_string_lossy();
        let out = (1..)
            .map(|n| src_path.with_file_name(if n == 1 { format!("{stem}-trim.mp4") } else { format!("{stem}-trim{n}.mp4") }))
            .find(|p| !p.exists())
            .unwrap();
        let cut = crate::trim::trim(src_path, &out, start.max(0.0), end).inspect_err(|_| {
            let _ = std::fs::remove_file(&out);
        })?;
        let bytes = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
        recorder::log(&format!("trim {src} {:.2}..{:.2}", cut.start, cut.end));
        Ok(json!({ "file": out.to_string_lossy(), "start": cut.start, "end": cut.end, "seconds": cut.end - cut.start, "bytes": bytes }))
    }

    /// The trim window: the recording, a timeline with two handles, Save.
    pub fn open_trim(&self, file: Option<String>) -> Result<(), String> {
        let file = file.or_else(|| self.inner.lock().unwrap().last.clone()).ok_or("nothing recorded yet")?;
        if let Some(bar) = self.app.get_webview_window("main") {
            let _ = bar.hide();
        }
        if let Some(w) = self.app.get_webview_window("trim") {
            let _ = w.close();
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
        let enc: String = file.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect();
        let (ax, ay, aw, ah) = self
            .app
            .primary_monitor()
            .ok()
            .flatten()
            .map(|m| {
                let s = m.scale_factor();
                let r = m.work_area();
                (r.position.x as f64 / s, r.position.y as f64 / s, r.size.width as f64 / s, r.size.height as f64 / s)
            })
            .unwrap_or((0.0, 0.0, 1920.0, 1040.0));
        let (w, h) = (aw.min(980.0), ah.min(680.0));
        WebviewWindowBuilder::new(&self.app, "trim", WebviewUrl::App(format!("trim.html?f={enc}").into()))
            .title("Kakkoii trim")
            .inner_size(w, h)
            .position(ax + (aw - w) / 2.0, ay + (ah - h) / 2.0)
            .decorations(false)
            .transparent(true)
            .shadow(false)
            .always_on_top(true)
            .resizable(false)
            .focused(true)
            .additional_browser_args(BROWSER_ARGS)
            .build()
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Re-encodes a recording smaller with ffmpeg (if installed). `quality` small | balanced.
    pub fn compress(&self, file: Option<String>, quality: Quality, gif: bool) -> Result<Value, String> {
        let src = file
            .or_else(|| self.inner.lock().unwrap().last.clone())
            .ok_or("no file given and nothing recorded yet")?;
        let src_path = Path::new(&src);
        if !src_path.exists() {
            return Err(format!("{src} does not exist"));
        }
        let stem = src_path.file_stem().unwrap_or_default().to_string_lossy();
        let out = src_path.with_file_name(if gif { format!("{stem}.gif") } else { format!("{stem}-small.mp4") });
        let mut cmd = std::process::Command::new("ffmpeg");
        cmd.args(["-y", "-v", "error", "-i", &src]);
        if gif {
            cmd.args(["-vf", "fps=15,scale=960:-1:flags=lanczos,split[a][b];[a]palettegen[p];[b][p]paletteuse"]);
        } else {
            let (crf, scale) = if quality == Quality::Small { ("30", "scale='min(1920,iw)':-2") } else { ("26", "scale='min(2560,iw)':-2") };
            cmd.args(["-vf", scale, "-c:v", "libx264", "-preset", "veryfast", "-crf", crf, "-movflags", "+faststart", "-an"]);
        }
        cmd.arg(&out);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // no console window
        }
        let st = cmd.output().map_err(|_| "ffmpeg not found. Install it: winget install Gyan.FFmpeg".to_string())?;
        if !st.status.success() {
            return Err(String::from_utf8_lossy(&st.stderr).into_owned());
        }
        let before = std::fs::metadata(&src).map(|m| m.len()).unwrap_or(0);
        let after = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
        Ok(json!({ "file": out.to_string_lossy(), "bytes": after, "was": before }))
    }
}
