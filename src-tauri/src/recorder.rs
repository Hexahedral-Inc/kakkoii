//! Screen and window recorder on Windows Graphics Capture + Media Foundation H.264.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use windows_capture::capture::{CaptureControl, Context, GraphicsCaptureApiHandler};
use windows_capture::encoder::{
    AudioSettingsBuilder, ContainerSettingsBuilder, VideoEncoder, VideoSettingsBuilder,
    VideoSettingsSubType,
};
use windows_capture::frame::Frame;
use windows_capture::graphics_capture_api::InternalCaptureControl;
use windows_capture::monitor::Monitor;
use windows_capture::settings::{
    ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
    GraphicsCaptureItemType, MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
};
use windows_capture::window::Window;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// A line in %TEMP%\kakkoii.log, for the bugs you only see on a real desktop.
pub fn log(msg: impl AsRef<str>) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(std::env::temp_dir().join("kakkoii.log")) {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        let _ = writeln!(f, "{t} {}", msg.as_ref());
    }
}

/// What to record.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Target {
    /// A whole display. `monitor` is 1-based, as Windows numbers them; none = primary.
    #[default]
    Screen,
    Monitor { index: usize },
    /// The first visible window whose title or process name contains `query`
    /// (case-insensitive), e.g. "chrome", "edge", "figma", "Visual Studio Code".
    Window { query: String },
    /// Whatever window has focus right now.
    Foreground,
    /// A box on the primary display, in physical pixels from its top-left corner.
    Area { x: u32, y: u32, w: u32, h: u32 },
    /// Just the user's face: the engine turns this into the camera window's handle.
    Face,
    /// A window by its handle (used for the face-only recording).
    Hwnd { hwnd: isize },
}

/// Which sound goes in the video.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct Sound {
    /// What the computer plays. On unless turned off.
    #[serde(default = "yes")]
    pub system: bool,
    /// The microphone.
    #[serde(default)]
    pub mic: bool,
}
fn yes() -> bool {
    true
}
impl Default for Sound {
    fn default() -> Self {
        Self { system: true, mic: false }
    }
}
impl Sound {
    pub fn any(self) -> bool {
        self.system || self.mic
    }
}

/// Size versus sharpness. Bits per pixel per frame, times fps.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Quality {
    /// 60 fps, ~25 Mbps at 4K.
    High,
    /// 60 fps, ~12 Mbps at 4K. The default.
    #[default]
    Balanced,
    /// 30 fps, ~5 Mbps at 4K. Good for sharing.
    Small,
}

impl Quality {
    fn fps(self) -> u32 {
        if self == Quality::Small { 30 } else { 60 }
    }
    fn bitrate(self, w: u32, h: u32) -> u32 {
        let bpp_x100 = match self {
            Quality::High => 5,
            Quality::Balanced => 3,
            Quality::Small => 2,
        };
        (w as u64 * h as u64 * self.fps() as u64 * bpp_x100 / 100).min(u32::MAX as u64) as u32
    }
}

/// Time spent paused, so the saved length leaves it out.
#[derive(Default)]
pub struct PauseClock {
    total: Duration,
    since: Option<Instant>,
}
impl PauseClock {
    pub fn paused_for(&self) -> Duration {
        self.total + self.since.map(|t| t.elapsed()).unwrap_or_default()
    }
}

#[derive(Clone)]
pub struct Flags {
    // Made before capture starts: it takes a couple of seconds to set up, and
    // making it on the first frame meant every video began late.
    encoder: Arc<std::sync::Mutex<Option<VideoEncoder>>>,
    quality: Quality,
    stop: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    clock: Arc<std::sync::Mutex<PauseClock>>,
    tail: Arc<std::sync::Mutex<Option<Duration>>>,
}

/// One frame at 60 fps, in 100 ns ticks: the gap left where a pause was cut out.
const ONE_FRAME: i64 = 166_667;

pub struct Rec {
    flags: Flags,
    encoder: Option<VideoEncoder>,
    done: bool,
    // When the first frame came in. The saved file is stretched to last from
    // then until stop (mp4tail.rs), however still the target sat.
    first_at: Option<Instant>,
    frames: u64,
    tail: Arc<std::sync::Mutex<Option<Duration>>>,
    // The last frame we sent, and where a pause began, in capture ticks.
    last_ts: i64,
    pause_from: Option<i64>,
}

impl GraphicsCaptureApiHandler for Rec {
    type Flags = Flags;
    type Error = BoxError;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        let encoder = ctx.flags.encoder.lock().unwrap().take();
        Ok(Self { tail: ctx.flags.tail.clone(), flags: ctx.flags, encoder, done: false, first_at: Some(Instant::now()), frames: 0, last_ts: 0, pause_from: None })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        if self.done {
            return Ok(());
        }
        if self.flags.stop.load(Ordering::Relaxed) && self.flags.paused.load(Ordering::Relaxed) {
            // Stopped while paused: save what came before the pause.
            self.finish_with_tail()?;
            control.stop();
            return Ok(());
        }
        let ts = frame.timestamp()?.Duration;
        if self.flags.paused.load(Ordering::Relaxed) {
            if self.pause_from.is_none() {
                self.pause_from = Some(self.last_ts);
            }
            return Ok(());
        }
        if let (Some(from), Some(enc)) = (self.pause_from.take(), self.encoder.as_mut()) {
            // Cut the pause out of the timeline, leaving one frame's gap.
            enc.add_time_offset((ts - from - ONE_FRAME).max(0));
        }
        self.last_ts = ts;
        if let Some(enc) = self.encoder.as_mut() {
            if let Err(e) = enc.send_frame(frame) {
                log(format!("send_frame failed after {} frames: {e}", self.frames));
                return Err(e.into());
            }
        }
        if self.frames == 0 {
            log(format!("first frame {}x{} after {:?}", frame.width(), frame.height(), self.first_at.map(|t| t.elapsed())));
        }
        self.frames += 1;
        if self.flags.stop.load(Ordering::Relaxed) {
            log("frame saw stop flag");
            self.finish_with_tail()?;
            control.stop();
        }
        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        log(format!("capture closed after {} frames", self.frames));
        // The window we were recording closed: save what we have.
        self.finish_with_tail()
    }
}

impl Rec {
    /// Saves the file and notes how long the recording really ran.
    fn finish_with_tail(&mut self) -> Result<(), BoxError> {
        log(format!("finish: done={} frames={} since first={:?}", self.done, self.frames, self.first_at.map(|t| t.elapsed())));
        if self.done {
            return Ok(());
        }
        // Measure before the encoder flushes: that takes a second and isn't recording.
        if let Some(at) = self.first_at {
            let paused = self.flags.clock.lock().unwrap().paused_for();
            *self.tail.lock().unwrap() = Some(at.elapsed().saturating_sub(paused));
        }
        if let Some(enc) = self.encoder.take() {
            enc.finish()?;
        }
        self.done = true;
        Ok(())
    }
}

pub struct Session {
    control: CaptureControl<Rec, BoxError>,
    stop: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    clock: Arc<std::sync::Mutex<PauseClock>>,
    tail: Arc<std::sync::Mutex<Option<Duration>>>,
    audio: Option<crate::audio::Audio>,
    pub path: String,
    pub label: String,
}

fn find_window(query: &str) -> Result<Window, String> {
    let q = query.to_lowercase();
    let windows = Window::enumerate().map_err(|e| e.to_string())?;
    windows
        .into_iter()
        .find(|w| {
            let title = w.title().unwrap_or_default().to_lowercase();
            let proc = w.process_name().unwrap_or_default().to_lowercase();
            !title.is_empty() && !title.contains("kakkoii") && (title.contains(&q) || proc.contains(&q))
        })
        .ok_or_else(|| format!("no open window matches \"{query}\""))
}

fn launch<I>(item: I, flags: Flags) -> Result<CaptureControl<Rec, BoxError>, String>
where
    I: TryInto<GraphicsCaptureItemType> + Send + 'static,
{
    let interval = if flags.quality == Quality::Small {
        MinimumUpdateIntervalSettings::Custom(Duration::from_millis(33))
    } else {
        MinimumUpdateIntervalSettings::Default
    };
    let settings = Settings::new(
        item,
        CursorCaptureSettings::WithCursor,
        DrawBorderSettings::WithoutBorder,
        SecondaryWindowSettings::Default,
        interval,
        DirtyRegionSettings::Default,
        ColorFormat::Bgra8,
        flags,
    );
    Rec::start_free_threaded(settings).map_err(|e| e.to_string())
}

fn encoder(w: u32, h: u32, quality: Quality, sound: bool, path: &str) -> Result<VideoEncoder, String> {
    // H.264 wants even sizes. Frames that differ later get padded by the encoder.
    let (w, h) = (w.max(2) & !1, h.max(2) & !1);
    VideoEncoder::new(
        VideoSettingsBuilder::new(w, h)
            .sub_type(VideoSettingsSubType::H264)
            .frame_rate(quality.fps())
            .bitrate(quality.bitrate(w, h)),
        // 48 kHz stereo 16-bit in, AAC out: what audio.rs hands over.
        AudioSettingsBuilder::default().disabled(!sound),
        ContainerSettingsBuilder::default(),
        path,
    )
    .map_err(|e| e.to_string())
}

fn window_size(w: &Window) -> (u32, u32) {
    (w.width().unwrap_or(1280).max(2) as u32, w.height().unwrap_or(720).max(2) as u32)
}

pub fn start(target: &Target, quality: Quality, sound: Sound, path: String) -> Result<Session, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let tail = Arc::new(std::sync::Mutex::new(None));
    let slot = Arc::new(std::sync::Mutex::new(None));
    let paused = Arc::new(AtomicBool::new(false));
    let clock = Arc::new(std::sync::Mutex::new(PauseClock::default()));
    let flags = Flags { encoder: slot.clone(), quality, stop: stop.clone(), paused: paused.clone(), clock: clock.clone(), tail: tail.clone() };
    let ready = |w: u32, h: u32| -> Result<(), String> {
        *slot.lock().unwrap() = Some(encoder(w, h, quality, sound.any(), &path)?);
        Ok(())
    };
    let (control, label) = match target {
        Target::Screen => {
            let m = Monitor::primary().map_err(|e| e.to_string())?;
            ready(m.width().map_err(|e| e.to_string())?, m.height().map_err(|e| e.to_string())?)?;
            (launch(m, flags)?, "Screen".to_string())
        }
        Target::Monitor { index } => {
            let m = Monitor::from_index(*index).map_err(|e| e.to_string())?;
            ready(m.width().map_err(|e| e.to_string())?, m.height().map_err(|e| e.to_string())?)?;
            (launch(m, flags)?, format!("Display {index}"))
        }
        Target::Window { query } => {
            let w = find_window(query)?;
            let (ww, wh) = window_size(&w);
            ready(ww, wh)?;
            let label = w.title().unwrap_or_else(|_| query.clone());
            (launch(w, flags)?, label)
        }
        Target::Area { x, y, w, h } => {
            let m = Monitor::primary().map_err(|e| e.to_string())?;
            let (mw, mh) = (m.width().map_err(|e| e.to_string())?, m.height().map_err(|e| e.to_string())?);
            let (x, y) = ((*x).min(mw.saturating_sub(16)), (*y).min(mh.saturating_sub(16)));
            let (w, h) = ((*w).clamp(16, mw - x), (*h).clamp(16, mh - y));
            ready(w, h)?;
            if let Some(enc) = slot.lock().unwrap().as_mut() {
                enc.set_crop(x, y);
            }
            (launch(m, flags)?, format!("Area {}x{}", w & !1, h & !1))
        }
        Target::Face => return Err("face recording goes through the engine".into()),
        Target::Hwnd { hwnd } => {
            let w = Window::from_raw_hwnd(*hwnd as *mut std::ffi::c_void);
            let (ww, wh) = window_size(&w);
            ready(ww, wh)?;
            (launch(w, flags)?, "Window".to_string())
        }
        Target::Foreground => {
            let w = Window::foreground().map_err(|e| e.to_string())?;
            let (ww, wh) = window_size(&w);
            ready(ww, wh)?;
            let label = w.title().unwrap_or_else(|_| "Window".into());
            (launch(w, flags)?, label)
        }
    };
    // Sound goes straight into the encoder the capture thread owns.
    let audio = sound.any().then(|| {
        let rec = control.callback();
        crate::audio::Audio::start(sound.system, sound.mic, paused.clone(), move |pcm| {
            if let Some(enc) = rec.lock().encoder.as_mut() {
                let _ = enc.send_audio_buffer(pcm, 0);
            }
        })
    });
    Ok(Session { control, stop, paused, clock, tail, audio, path, label })
}

impl Session {
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    /// Pause or carry on. Frames and sound while paused are dropped, and the
    /// gap is cut out of the video's timeline.
    pub fn set_paused(&self, on: bool) {
        let mut c = self.clock.lock().unwrap();
        if on && c.since.is_none() {
            c.since = Some(Instant::now());
        } else if !on {
            if let Some(t) = c.since.take() {
                c.total += t.elapsed();
            }
        }
        self.paused.store(on, Ordering::Relaxed);
    }

    /// Seconds actually recorded, pauses left out.
    pub fn recorded(&self, started: Instant) -> f64 {
        started.elapsed().saturating_sub(self.clock.lock().unwrap().paused_for()).as_secs_f64()
    }
    pub fn is_finished(&self) -> bool {
        self.control.is_finished()
    }

    /// Asks the capture thread to finish the file on its next frame. A still
    /// screen sends no frames, so after 3 s we stop it outright.
    pub fn stop(mut self) -> Result<String, String> {
        log(format!("session stop called ({})", self.path));
        if let Some(a) = self.audio.take() {
            a.stop();
        }
        self.stop.store(true, Ordering::Relaxed);
        // A frame that arrives now saves the file itself. If the target is still,
        // none will, so we finish it from here with the held frame.
        let deadline = Instant::now() + Duration::from_millis(150);
        while !self.control.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(15));
        }
        if !self.control.is_finished() {
            let rec = self.control.callback();
            let mut rec = rec.lock();
            rec.finish_with_tail().map_err(|e| e.to_string())?;
        }
        log(format!("stop: finished={}", self.control.is_finished()));
        if self.control.is_finished() {
            if let Err(e) = self.control.wait() {
                log(format!("capture thread error: {e}"));
                return Err(e.to_string());
            }
        } else {
            self.control.stop().map_err(|e| e.to_string())?;
        }
        if let Some(length) = *self.tail.lock().unwrap() {
            let _ = crate::mp4tail::extend_to(&self.path, length);
        }
        Ok(self.path)
    }
}

#[derive(Serialize)]
pub struct TargetInfo {
    pub kind: &'static str,
    pub index: Option<usize>,
    pub title: String,
    pub process: Option<String>,
    pub size: Option<(u32, u32)>,
}

/// Every display and every titled window, for agents and the picker.
pub fn targets() -> Vec<TargetInfo> {
    let mut out = Vec::new();
    if let Ok(ms) = Monitor::enumerate() {
        for m in ms {
            out.push(TargetInfo {
                kind: "monitor",
                index: m.index().ok(),
                title: m.name().unwrap_or_default(),
                process: None,
                size: m.width().ok().zip(m.height().ok()),
            });
        }
    }
    if let Ok(ws) = Window::enumerate() {
        for w in ws {
            let title = w.title().unwrap_or_default();
            if title.is_empty() || title.to_lowercase().contains("kakkoii") {
                continue;
            }
            out.push(TargetInfo {
                kind: "window",
                index: None,
                process: w.process_name().ok(),
                size: w.width().ok().zip(w.height().ok()).map(|(a, b)| (a.max(0) as u32, b.max(0) as u32)),
                title,
            });
        }
    }
    out
}
