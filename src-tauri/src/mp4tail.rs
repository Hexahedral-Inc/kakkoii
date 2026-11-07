//! Windows sends capture frames only when something on screen changes. If the
//! target sat still before you pressed stop, the video would end at its last
//! change. This stretches the last video frame in the saved MP4,
//! so the file lasts as long as the recording did. No re-encode, no CPU while
//! recording: it edits the sample table (stts) and the three durations.
//!
//! Only touches files laid out the way Media Foundation writes them (moov
//! after mdat, version 0 boxes); anything else is left alone.

use std::time::Duration;

fn be32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}
fn put32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_be_bytes());
}

/// Child boxes of the box body in b[start..end]: (offset, size, type).
fn boxes(b: &[u8], start: usize, end: usize) -> Vec<(usize, usize, [u8; 4])> {
    let mut out = Vec::new();
    let mut at = start;
    while at + 8 <= end {
        let size = be32(b, at) as usize;
        if size < 8 || at + size > end {
            break;
        }
        out.push((at, size, [b[at + 4], b[at + 5], b[at + 6], b[at + 7]]));
        at += size;
    }
    out
}

fn find(b: &[u8], start: usize, end: usize, t: &[u8; 4]) -> Option<(usize, usize)> {
    boxes(b, start, end).into_iter().find(|x| &x.2 == t).map(|x| (x.0, x.1))
}

/// Makes the video track last `length` (from its first frame) if it is shorter.
pub fn extend_to(path: &str, length: Duration) -> Result<(), String> {
    let mut b = std::fs::read(path).map_err(|e| e.to_string())?;
    let top = boxes(&b, 0, b.len());
    let mdat = top.iter().position(|x| &x.2 == b"mdat").ok_or("no mdat")?;
    let moov = top.iter().position(|x| &x.2 == b"moov").ok_or("no moov")?;
    if moov < mdat {
        return Err("moov before mdat: left alone".into());
    }
    let (moov_at, moov_size) = (top[moov].0, top[moov].1);
    let (mvhd_at, _) = find(&b, moov_at + 8, moov_at + moov_size, b"mvhd").ok_or("no mvhd")?;
    if b[mvhd_at + 8] != 0 {
        return Err("mvhd v1: left alone".into());
    }
    let movie_scale = be32(&b, mvhd_at + 20);

    // The video track: the one with a vmhd.
    let mut chain = None;
    for (trak_at, trak_size, t) in boxes(&b, moov_at + 8, moov_at + moov_size) {
        if &t != b"trak" {
            continue;
        }
        let Some((mdia_at, mdia_size)) = find(&b, trak_at + 8, trak_at + trak_size, b"mdia") else { continue };
        let Some((minf_at, minf_size)) = find(&b, mdia_at + 8, mdia_at + mdia_size, b"minf") else { continue };
        if find(&b, minf_at + 8, minf_at + minf_size, b"vmhd").is_none() {
            continue;
        }
        let tkhd = find(&b, trak_at + 8, trak_at + trak_size, b"tkhd").ok_or("no tkhd")?;
        let mdhd = find(&b, mdia_at + 8, mdia_at + mdia_size, b"mdhd").ok_or("no mdhd")?;
        let stbl = find(&b, minf_at + 8, minf_at + minf_size, b"stbl").ok_or("no stbl")?;
        let stts = find(&b, stbl.0 + 8, stbl.0 + stbl.1, b"stts").ok_or("no stts")?;
        chain = Some((trak_at, mdia_at, minf_at, stbl.0, stts.0, tkhd.0, mdhd.0));
        break;
    }
    let (trak_at, mdia_at, minf_at, stbl_at, stts_at, tkhd_at, mdhd_at) = chain.ok_or("no video track")?;
    if b[mdhd_at + 8] != 0 || b[tkhd_at + 8] != 0 {
        return Err("v1 headers: left alone".into());
    }
    let scale = be32(&b, mdhd_at + 20);
    let n = be32(&b, stts_at + 12) as usize;
    if n == 0 {
        return Err("empty stts".into());
    }
    let have: u64 = (0..n).map(|i| be32(&b, stts_at + 16 + i * 8) as u64 * be32(&b, stts_at + 20 + i * 8) as u64).sum();
    let want = (length.as_secs_f64() * scale as f64).round() as u64;
    // Under 40 ms short: nothing worth fixing.
    if want <= have + scale as u64 / 25 {
        return Ok(());
    }
    let add = want - have;
    let extra = Duration::from_secs_f64(add as f64 / scale as f64);
    let last = stts_at + 16 + (n - 1) * 8;
    let (count, delta) = (be32(&b, last), be32(&b, last + 4));
    let new_delta = (delta as u64 + add).min(u32::MAX as u64) as u32;

    let mut grow = 0usize;
    if count == 1 {
        put32(&mut b, last + 4, new_delta);
    } else {
        // Split the last run: (count-1, delta) then (1, delta + extra).
        put32(&mut b, last, count - 1);
        let mut entry = [0u8; 8];
        entry[..4].copy_from_slice(&1u32.to_be_bytes());
        entry[4..].copy_from_slice(&new_delta.to_be_bytes());
        b.splice(last + 8..last + 8, entry);
        put32(&mut b, stts_at + 12, n as u32 + 1);
        grow = 8;
    }
    if grow > 0 {
        // Every box that holds the stts grows by one entry. The chunk offsets
        // point into mdat, which sits before moov, so they stay right.
        for at in [stts_at, stbl_at, minf_at, mdia_at, trak_at, moov_at] {
            let s = be32(&b, at);
            put32(&mut b, at, s + grow as u32);
        }
    }
    let bump = |b: &mut Vec<u8>, at: usize, by: u64| {
        let v = be32(b, at) as u64 + by;
        put32(b, at, v.min(u32::MAX as u64) as u32);
    };
    let add_movie = (extra.as_secs_f64() * movie_scale as f64).round() as u64;
    bump(&mut b, mdhd_at + 24, add);
    bump(&mut b, tkhd_at + 28, add_movie);
    // The movie lasts as long as its longest track.
    let track_len = be32(&b, tkhd_at + 28);
    if be32(&b, mvhd_at + 24) < track_len {
        put32(&mut b, mvhd_at + 24, track_len);
    }
    std::fs::write(path, b).map_err(|e| e.to_string())
}
