//! Sound for a recording: what the computer plays (WASAPI loopback) and the
//! microphone, mixed to 48 kHz stereo 16-bit PCM, the encoder's input.
//!
//! The mixer runs on a clock, not on the devices: every 10 ms it hands the
//! encoder exactly the samples that much time is worth, taking what the
//! devices captured and filling the rest with silence. Loopback sends nothing
//! while nothing plays, and the encoder stalls the video if audio stops
//! arriving, so the clock is what keeps both moving.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use wasapi::{DeviceEnumerator, Direction, SampleType, StreamMode, WaveFormat};

pub const RATE: u32 = 48_000;
/// Half a second: a device that runs ahead gets its oldest samples dropped, so sound can't drift late.
const MAX_QUEUE: usize = RATE as usize;

type Queue = Arc<Mutex<VecDeque<f32>>>;

pub struct Audio {
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

/// Reads one device into `q` as interleaved stereo f32 at 48 kHz (Windows converts for us).
fn capture(loopback: bool, q: Queue, stop: Arc<AtomicBool>) -> Result<(), String> {
    let _ = wasapi::initialize_mta();
    let en = DeviceEnumerator::new().map_err(|x| x.to_string())?;
    // Loopback is a capture stream opened on the speakers.
    let device = en.get_default_device(if loopback { Direction::Render } else { Direction::Capture }).map_err(|x| x.to_string())?;
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
    pub fn start(system: bool, mic: bool, mut sink: impl FnMut(&[u8]) + Send + 'static) -> Self {
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
            threads.push(std::thread::spawn(move || {
                if let Err(e) = capture(loopback, q, st) {
                    crate::recorder::log(format!("{name} not captured: {e}"));
                }
            }));
        }
        let st = stop.clone();
        threads.push(std::thread::spawn(move || {
            let start = Instant::now();
            let mut sent: u64 = 0;
            let mut out: Vec<u8> = Vec::new();
            while !st.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(10));
                let due = (start.elapsed().as_secs_f64() * RATE as f64) as u64;
                let frames = due.saturating_sub(sent) as usize;
                if frames == 0 {
                    continue;
                }
                out.clear();
                out.reserve(frames * 4);
                let mut locked: Vec<_> = queues.iter().map(|q| q.lock().unwrap()).collect();
                for _ in 0..frames * 2 {
                    let mut v = 0.0f32;
                    for q in locked.iter_mut() {
                        v += q.pop_front().unwrap_or(0.0);
                    }
                    let s = (v.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                    out.extend_from_slice(&s.to_le_bytes());
                }
                drop(locked);
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
    }
}
