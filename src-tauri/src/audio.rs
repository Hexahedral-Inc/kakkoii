//! Sound for a recording: what the computer plays (WASAPI loopback) and the
//! microphone, mixed to 48 kHz stereo 16-bit PCM, the encoder's input.
//!
//! The mixer runs on a clock, not on the devices: every 10 ms it hands the
//! encoder exactly the samples that much time is worth, taking what the
//! devices captured and filling the rest with silence. Loopback sends nothing
//! while nothing plays, and the encoder stalls the video if audio stops
//! arriving, so the clock is what keeps both moving.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use wasapi::{DeviceEnumerator, Direction, SampleType, StreamMode, WaveFormat};

pub const RATE: u32 = 48_000;
/// Half a second: a device that runs ahead gets its oldest samples dropped, so sound can't drift late.
const MAX_QUEUE: usize = RATE as usize;

type Queue = Arc<Mutex<VecDeque<f32>>>;

/// The island's meter: the loudest sample in five bands (low to high) of the
/// last 10 ms of the mix, as f32 bits. Cheap one-pole filters, no FFT.
const SILENT: AtomicU32 = AtomicU32::new(0);
pub static BANDS: [AtomicU32; 5] = [SILENT; 5];
const CUTS: [f32; 4] = [180.0, 500.0, 1400.0, 3500.0];

pub fn bands() -> [f32; 5] {
    std::array::from_fn(|i| f32::from_bits(BANDS[i].load(Ordering::Relaxed)))
}
fn quiet() {
    for b in &BANDS {
        b.store(0, Ordering::Relaxed);
    }
}

pub struct Audio {
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

/// The mic you picked, kept in %APPDATA%\Kakkoii\mic.txt. None: Windows' default.
fn mic_file() -> Option<std::path::PathBuf> {
    std::env::var_os("APPDATA").map(|a| std::path::Path::new(&a).join("Kakkoii").join("mic.txt"))
}
pub fn chosen_mic() -> Option<String> {
    mic_file().and_then(|f| std::fs::read_to_string(f).ok()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}
pub fn choose_mic(name: Option<&str>) -> Result<(), String> {
    let f = mic_file().ok_or("no APPDATA")?;
    let _ = std::fs::create_dir_all(f.parent().unwrap());
    std::fs::write(f, name.unwrap_or("")).map_err(|e| e.to_string())
}

/// Every microphone, which one is Windows' default, which one is picked, and
/// (with `probe`) how loud each is over the next second, in dB.
pub fn mics(probe: bool) -> Result<serde_json::Value, String> {
    let _ = wasapi::initialize_mta();
    let en = DeviceEnumerator::new().map_err(|x| x.to_string())?;
    let default = en.get_default_device(Direction::Capture).and_then(|d| d.get_friendlyname()).unwrap_or_default();
    let names: Vec<String> = en
        .get_device_collection(Direction::Capture)
        .map_err(|x| x.to_string())?
        .into_iter()
        .filter_map(|d| d.ok()?.get_friendlyname().ok())
        .collect();
    let probes: Vec<_> = names
        .iter()
        .map(|n| {
            let n = n.clone();
            probe.then(|| {
                std::thread::spawn(move || {
                    let q: Queue = Arc::new(Mutex::new(VecDeque::new()));
                    let stop = Arc::new(AtomicBool::new(false));
                    let (q2, s2) = (q.clone(), stop.clone());
                    let t = std::thread::spawn(move || capture(false, Some(n), q2, s2));
                    std::thread::sleep(Duration::from_millis(1200));
                    stop.store(true, Ordering::Relaxed);
                    let r = t.join().unwrap_or(Err("crashed".into()));
                    let peak = q.lock().unwrap().iter().fold(0.0f32, |m, v| m.max(v.abs()));
                    r.map(|_| (20.0 * peak.max(1e-6).log10() * 10.0).round() / 10.0)
                })
            })
        })
        .collect();
    let picked = chosen_mic();
    let list: Vec<_> = names
        .iter()
        .zip(probes)
        .map(|(n, p)| {
            let level = p.map(|t| match t.join().unwrap_or(Err("crashed".into())) {
                Ok(db) => serde_json::json!(db),
                Err(e) => serde_json::json!(e),
            });
            serde_json::json!({ "name": n, "default": *n == default, "picked": picked.as_deref() == Some(n.as_str()), "peak_db": level })
        })
        .collect();
    Ok(serde_json::json!({ "mics": list, "picked": picked, "default": default }))
}

/// Reads one device into `q` as interleaved stereo f32 at 48 kHz (Windows converts for us).
fn capture(loopback: bool, name: Option<String>, q: Queue, stop: Arc<AtomicBool>) -> Result<(), String> {
    let _ = wasapi::initialize_mta();
    let en = DeviceEnumerator::new().map_err(|x| x.to_string())?;
    // Loopback is a capture stream opened on the speakers. A picked mic that's
    // gone (unplugged) falls back to the default.
    let named = name.and_then(|n| en.get_device_collection(Direction::Capture).ok()?.get_device_with_name(&n).ok());
    let device = match named {
        Some(d) => d,
        None => en.get_default_device(if loopback { Direction::Render } else { Direction::Capture }).map_err(|x| x.to_string())?,
    };
    if !loopback {
        crate::recorder::log(format!("mic: {}", device.get_friendlyname().unwrap_or_default()));
    }
    let mut client = device.get_iaudioclient().map_err(|x| x.to_string())?;
    let format = WaveFormat::new(32, 32, SampleType::Float, RATE as usize, 2, None);
    let (def, _) = client.get_device_period().map_err(|x| x.to_string())?;
    let mode = StreamMode::PollingShared { autoconvert: true, buffer_duration_hns: def * 4 };
    client.initialize_client(&format, Direction::Capture, mode, None).map_err(|x| x.to_string())?;
    let cap = client.get_audiocaptureclient().map_err(|x| x.to_string())?;
    client.start_stream().map_err(|x| x.to_string())?;
    let mut bytes: VecDeque<u8> = VecDeque::new();
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(5));
        cap.read_from_device_to_deque(&mut bytes).map_err(|x| x.to_string())?;
        let n = bytes.len() / 4;
        if n == 0 {
            continue;
        }
        let mut q = q.lock().unwrap();
        for _ in 0..n {
            let b = [bytes.pop_front().unwrap(), bytes.pop_front().unwrap(), bytes.pop_front().unwrap(), bytes.pop_front().unwrap()];
            q.push_back(f32::from_le_bytes(b));
        }
        while q.len() > MAX_QUEUE * 2 {
            q.pop_front();
        }
    }
    let _ = client.stop_stream();
    Ok(())
}

impl Audio {
    /// Starts capturing and calls `sink` with PCM bytes, paced to the wall clock.
    pub fn start(system: bool, mic: bool, paused: Arc<AtomicBool>, mut sink: impl FnMut(&[u8]) + Send + 'static) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let mut threads = Vec::new();
        let mut queues = Vec::new();
        for (on, loopback, name) in [(system, true, "system sound"), (mic, false, "microphone")] {
            if !on {
                continue;
            }
            let q: Queue = Arc::new(Mutex::new(VecDeque::new()));
            queues.push(q.clone());
            let st = stop.clone();
            let pick = if loopback { None } else { chosen_mic() };
            threads.push(std::thread::spawn(move || {
                if let Err(e) = capture(loopback, pick, q, st) {
                    crate::recorder::log(format!("{name} not captured: {e}"));
                }
            }));
        }
        quiet();
        let st = stop.clone();
        threads.push(std::thread::spawn(move || {
            let k: [f32; 4] = CUTS.map(|f| 1.0 - (-2.0 * std::f32::consts::PI * f / RATE as f32).exp());
            let mut lp = [0.0f32; 4];
            // The clock only runs while recording: a pause stops it, and what the
            // devices capture meanwhile is thrown away.
            let mut active = Duration::ZERO;
            let mut last = Instant::now();
            let mut sent: u64 = 0;
            let mut out: Vec<u8> = Vec::new();
            while !st.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(10));
                let now = Instant::now();
                if paused.load(Ordering::Relaxed) {
                    last = now;
                    for q in &queues {
                        q.lock().unwrap().clear();
                    }
                    quiet();
                    continue;
                }
                active += now - last;
                last = now;
                let due = (active.as_secs_f64() * RATE as f64) as u64;
                let frames = due.saturating_sub(sent) as usize;
                if frames == 0 {
                    continue;
                }
                out.clear();
                out.reserve(frames * 4);
                let mut locked: Vec<_> = queues.iter().map(|q| q.lock().unwrap()).collect();
                let mut peak = [0.0f32; 5];
                for _ in 0..frames {
                    let mut mono = 0.0f32;
                    for _ in 0..2 {
                        let mut v = 0.0f32;
                        for q in locked.iter_mut() {
                            v += q.pop_front().unwrap_or(0.0);
                        }
                        let v = v.clamp(-1.0, 1.0);
                        mono += v * 0.5;
                        out.extend_from_slice(&((v * i16::MAX as f32) as i16).to_le_bytes());
                    }
                    // Each band is the gap between two low-passes.
                    let mut below = 0.0f32;
                    for i in 0..4 {
                        lp[i] += k[i] * (mono - lp[i]);
                        peak[i] = peak[i].max((lp[i] - below).abs());
                        below = lp[i];
                    }
                    peak[4] = peak[4].max((mono - below).abs());
                }
                drop(locked);
                for (b, p) in BANDS.iter().zip(peak) {
                    b.store(p.to_bits(), Ordering::Relaxed);
                }
                sink(&out);
                sent += frames as u64;
            }
        }));
        Self { stop, threads }
    }

    pub fn stop(self) {
        self.stop.store(true, Ordering::Relaxed);
        for t in self.threads {
            let _ = t.join();
        }
        quiet();
    }
}
