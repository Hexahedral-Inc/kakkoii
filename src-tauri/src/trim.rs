//! Lossless trim: copies the frames between two times into a new MP4, no re-encode.
//! The cut starts on the keyframe at or before `start`, so it can begin a little early.

use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::Path;

use mp4::{AacConfig, AvcConfig, MediaConfig, MediaType, Mp4Config, Mp4Reader, Mp4Writer, TrackConfig, TrackType};

pub struct Trimmed {
    /// Where the cut really starts and ends, in seconds of the source.
    pub start: f64,
    pub end: f64,
}

struct Track {
    id: u32,
    out: u32,
    timescale: u32,
    count: u32,
    video: bool,
}

pub fn trim(src: &Path, dst: &Path, start: f64, end: f64) -> Result<Trimmed, String> {
    if end <= start {
        return Err("the end is before the start".into());
    }
    let f = File::open(src).map_err(|e| e.to_string())?;
    let size = f.metadata().map_err(|e| e.to_string())?.len();
    let mut r = Mp4Reader::read_header(BufReader::new(f), size).map_err(|e| e.to_string())?;

    let mut tracks = Vec::new();
    let mut configs = Vec::new();
    let mut ids: Vec<u32> = r.tracks().keys().copied().collect();
    ids.sort();
    for id in ids {
        let t = &r.tracks()[&id];
        let media_conf = match t.media_type().map_err(|e| e.to_string())? {
            MediaType::H264 => MediaConfig::AvcConfig(AvcConfig {
                width: t.width(),
                height: t.height(),
                seq_param_set: t.sequence_parameter_set().map_err(|e| e.to_string())?.to_vec(),
                pic_param_set: t.picture_parameter_set().map_err(|e| e.to_string())?.to_vec(),
            }),
            MediaType::AAC => MediaConfig::AacConfig(AacConfig {
                bitrate: t.bitrate(),
                profile: t.audio_profile().map_err(|e| e.to_string())?,
                freq_index: t.sample_freq_index().map_err(|e| e.to_string())?,
                chan_conf: t.channel_config().map_err(|e| e.to_string())?,
            }),
            _ => continue,
        };
        let video = matches!(media_conf, MediaConfig::AvcConfig(_));
        configs.push(TrackConfig {
            track_type: if video { TrackType::Video } else { TrackType::Audio },
            timescale: t.timescale(),
            language: t.language().to_string(),
            media_conf,
        });
        tracks.push(Track { id, out: tracks.len() as u32 + 1, timescale: t.timescale(), count: t.sample_count(), video });
    }
    let v = tracks.iter().find(|t| t.video).ok_or("no video track")?;
    let (vid, vts, vcount) = (v.id, v.timescale as f64, v.count);

    // Start on the last keyframe at or before `start`; end at the first frame at or after `end`.
    let (mut first, mut cut0) = (1u32, 0f64);
    let mut cut1 = 0f64;
    for i in 1..=vcount {
        let s = r.read_sample(vid, i).map_err(|e| e.to_string())?.ok_or("missing frame")?;
        let t = s.start_time as f64 / vts;
        if t >= end {
            break;
        }
        if s.is_sync && t <= start {
            first = i;
            cut0 = t;
        }
        cut1 = t + s.duration as f64 / vts;
    }
    if cut1 <= cut0 {
        return Err("nothing between those times".into());
    }

    let mut w = Mp4Writer::write_start(
        BufWriter::new(File::create(dst).map_err(|e| e.to_string())?),
        &Mp4Config {
            major_brand: r.major_brand().clone(),
            minor_version: r.minor_version(),
            compatible_brands: r.compatible_brands().to_vec(),
            timescale: r.timescale(),
        },
    )
    .map_err(|e| e.to_string())?;
    for c in &configs {
        w.add_track(c).map_err(|e| e.to_string())?;
    }

    // Copy each track's samples in [cut0, cut1), interleaved by time.
    // Video from the keyframe (the edit list below hides the frames before `start`), sound from `start`.
    let start = start.max(cut0);
    let mut next: Vec<u32> = tracks
        .iter()
        .map(|t| if t.video { first } else { seek(&mut r, t, start).unwrap_or(t.count + 1) })
        .collect();
    let mut peek: Vec<Option<(f64, mp4::Mp4Sample)>> = tracks.iter().map(|_| None).collect();
    loop {
        for (k, t) in tracks.iter().enumerate() {
            if peek[k].is_none() && next[k] <= t.count {
                let s = r.read_sample(t.id, next[k]).map_err(|e| e.to_string())?.ok_or("missing sample")?;
                let at = s.start_time as f64 / t.timescale as f64;
                if at < cut1 {
                    peek[k] = Some((at, s));
                    next[k] += 1;
                } else {
                    next[k] = t.count + 1;
                }
            }
        }
        let k = (0..tracks.len())
            .filter(|&k| peek[k].is_some())
            .min_by(|&a, &b| peek[a].as_ref().unwrap().0.total_cmp(&peek[b].as_ref().unwrap().0));
        let Some(k) = k else { break };
        let (_, s) = peek[k].take().unwrap();
        w.write_sample(tracks[k].out, &s).map_err(|e| e.to_string())?;
    }
    w.write_end().map_err(|e| e.to_string())?;
    drop(w);
    let vout = tracks.iter().find(|t| t.video).map(|t| t.out).unwrap_or(1);
    let movie_ts = r.timescale() as f64;
    edit_list(dst, vout, ((start - cut0) * vts).round() as u64, ((cut1 - start) * movie_ts).round() as u64)?;
    Ok(Trimmed { start, end: cut1 })
}

/// The first sample of `t` that starts at or after `at` seconds.
fn seek<R: std::io::Read + std::io::Seek>(r: &mut Mp4Reader<R>, t: &Track, at: f64) -> Option<u32> {
    (1..=t.count).find(|&i| {
        r.read_sample(t.id, i).ok().flatten().is_some_and(|s| s.start_time as f64 / t.timescale as f64 >= at)
    })
}

fn be32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes(b[at..at + 4].try_into().unwrap())
}

/// Child boxes of `b[from..to]` as (start, size, type).
fn boxes(b: &[u8], mut from: usize, to: usize) -> Vec<(usize, usize, [u8; 4])> {
    let mut out = Vec::new();
    while from + 8 <= to {
        let size = be32(b, from) as usize;
        if size < 8 || from + size > to {
            break;
        }
        out.push((from, size, b[from + 4..from + 8].try_into().unwrap()));
        from += size;
    }
    out
}

/// Gives video track `track` an edit list that starts `skip` media units in and lasts
/// `length` movie units, and sets the track and movie durations to match.
/// The writer puts moov after mdat, so growing it moves no sample offsets.
fn edit_list(path: &Path, track: u32, skip: u64, length: u64) -> Result<(), String> {
    let mut b = std::fs::read(path).map_err(|e| e.to_string())?;
    let top = boxes(&b, 0, b.len());
    let &(moov, moov_size, _) = top.iter().rev().find(|x| &x.2 == b"moov").ok_or("no moov")?;
    if moov + moov_size != b.len() {
        return Err("moov is not at the end".into());
    }
    let kids = boxes(&b, moov + 8, moov + moov_size);
    let set_dur = |b: &mut Vec<u8>, at: usize, v1_off: usize, v0_off: usize| {
        if b[at + 8] == 1 {
            b[at + v1_off..at + v1_off + 8].copy_from_slice(&length.to_be_bytes());
        } else {
            b[at + v0_off..at + v0_off + 4].copy_from_slice(&(length.min(u32::MAX as u64) as u32).to_be_bytes());
        }
    };
    let mut target = None;
    for &(at, size, kind) in &kids {
        if &kind == b"mvhd" {
            set_dur(&mut b, at, 32, 24);
        }
        if &kind == b"trak" {
            let inner = boxes(&b, at + 8, at + size);
            if let Some(&(tk, _, _)) = inner.iter().find(|x| &x.2 == b"tkhd") {
                let id = if b[tk + 8] == 1 { be32(&b, tk + 28) } else { be32(&b, tk + 20) };
                if id == track {
                    set_dur(&mut b, tk, 36, 28);
                    target = Some((at, inner.iter().find(|x| &x.2 == b"tkhd").map(|x| x.0 + x.1).unwrap()));
                }
            }
        }
    }
    let (trak, after_tkhd) = target.ok_or("no video trak")?;
    // edts > elst, version 1: one entry (segment duration, media time, rate 1.0).
    let mut elst = Vec::with_capacity(36);
    elst.extend_from_slice(&36u32.to_be_bytes());
    elst.extend_from_slice(b"elst");
    elst.extend_from_slice(&[1, 0, 0, 0]);
    elst.extend_from_slice(&1u32.to_be_bytes());
    elst.extend_from_slice(&length.to_be_bytes());
    elst.extend_from_slice(&(skip as i64).to_be_bytes());
    elst.extend_from_slice(&[0, 1, 0, 0]);
    let mut edts = Vec::with_capacity(44);
    edts.extend_from_slice(&44u32.to_be_bytes());
    edts.extend_from_slice(b"edts");
    edts.extend_from_slice(&elst);
    let grow = |b: &mut Vec<u8>, at: usize| {
        let n = be32(b, at) + edts.len() as u32;
        b[at..at + 4].copy_from_slice(&n.to_be_bytes());
    };
    grow(&mut b, moov);
    grow(&mut b, trak);
    b.splice(after_tkhd..after_tkhd, edts.iter().copied());
    std::fs::write(path, b).map_err(|e| e.to_string())
}
