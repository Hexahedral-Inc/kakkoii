//! Full-screen recorder on Windows Graphics Capture + Media Foundation H.264.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

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
    MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Clone)]
pub struct Flags {
    path: String,
    width: u32,
    height: u32,
    stop: Arc<AtomicBool>,
}

pub struct Rec {
    encoder: Option<VideoEncoder>,
    stop: Arc<AtomicBool>,
}

impl GraphicsCaptureApiHandler for Rec {
    type Flags = Flags;
    type Error = BoxError;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        let f = ctx.flags;
        let encoder = VideoEncoder::new(
            // H.264, not the HEVC default: every player and browser opens it.
            // About 0.05 bits per pixel at 60 fps: ~25 Mbps at 4K, ~6 Mbps at 1080p.
            VideoSettingsBuilder::new(f.width, f.height)
                .sub_type(VideoSettingsSubType::H264)
                .bitrate(f.width * f.height * 3),
            AudioSettingsBuilder::default().disabled(true),
            ContainerSettingsBuilder::default(),
            &f.path,
        )?;
        Ok(Self { encoder: Some(encoder), stop: f.stop })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        if let Some(enc) = self.encoder.as_mut() {
            enc.send_frame(frame)?;
        }
        if self.stop.load(Ordering::Relaxed) {
            if let Some(enc) = self.encoder.take() {
                enc.finish()?;
            }
            control.stop();
        }
        Ok(())
    }
}

pub struct Session {
    control: CaptureControl<Rec, BoxError>,
    stop: Arc<AtomicBool>,
    pub path: String,
}

pub fn start(path: String) -> Result<Session, String> {
    let monitor = Monitor::primary().map_err(|e| e.to_string())?;
    let width = monitor.width().map_err(|e| e.to_string())?;
    let height = monitor.height().map_err(|e| e.to_string())?;
    let stop = Arc::new(AtomicBool::new(false));

    let settings = Settings::new(
        monitor,
        CursorCaptureSettings::WithCursor,
        DrawBorderSettings::WithoutBorder,
        SecondaryWindowSettings::Default,
        MinimumUpdateIntervalSettings::Default,
        DirtyRegionSettings::Default,
        ColorFormat::Bgra8,
        Flags { path: path.clone(), width, height, stop: stop.clone() },
    );

    let control = Rec::start_free_threaded(settings).map_err(|e| e.to_string())?;
    Ok(Session { control, stop, path })
}

impl Session {
    /// Asks the capture thread to finish the file on its next frame. A still
    /// screen sends no frames, so after 3 s we stop it outright.
    pub fn stop(self) -> Result<String, String> {
        self.stop.store(true, Ordering::Relaxed);
        let deadline = Instant::now() + Duration::from_secs(3);
        while !self.control.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(30));
        }
        if self.control.is_finished() {
            self.control.wait().map_err(|e| e.to_string())?;
        } else {
            self.control.stop().map_err(|e| e.to_string())?;
        }
        Ok(self.path)
    }
}
