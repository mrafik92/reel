//! Minimal fragmented-MP4 timeline handling.
//!
//! ffmpeg's HLS fMP4 muxer rebases timestamps depending on how a run starts (first run vs. restart
//! after a seek: B-frame delay, audio priming …), so the same playlist segment can come out
//! tens of milliseconds apart between runs. We normalise every GOP file we serve in copy mode so
//! that the first video sample's presentation time equals the keyframe time from the index (the
//! playlist timeline); audio is shifted by the same amount to keep A/V sync intact.

use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Track {
    pub timescale: u32,
    pub video: bool,
}

fn boxes(d: &[u8], base: usize) -> Vec<(usize, usize, [u8; 4])> {
    // (absolute body start, absolute end, type)
    let mut v = Vec::new();
    let mut p = 0usize;
    while p + 8 <= d.len() {
        let s = u32::from_be_bytes([d[p], d[p + 1], d[p + 2], d[p + 3]]) as usize;
        if s < 8 || p + s > d.len() {
            break;
        }
        v.push((
            base + p + 8,
            base + p + s,
            [d[p + 4], d[p + 5], d[p + 6], d[p + 7]],
        ));
        p += s;
    }
    v
}

fn child(d: &[u8], parent: (usize, usize), kind: &[u8; 4]) -> Option<(usize, usize)> {
    boxes(&d[parent.0..parent.1], parent.0)
        .into_iter()
        .find(|b| &b.2 == kind)
        .map(|b| (b.0, b.1))
}

fn be32(d: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(d.get(at..at + 4)?.try_into().ok()?))
}

/// track_id → timescale/kind from an init segment (`moov`).
pub fn init_tracks(init: &[u8]) -> HashMap<u32, Track> {
    let mut out = HashMap::new();
    let top = boxes(init, 0);
    let Some(moov) = top.iter().find(|b| &b.2 == b"moov").map(|b| (b.0, b.1)) else {
        return out;
    };
    for (s, e, k) in boxes(&init[moov.0..moov.1], moov.0) {
        if &k != b"trak" {
            continue;
        }
        let parse = || -> Option<(u32, Track)> {
            let tkhd = child(init, (s, e), b"tkhd")?;
            let id = be32(init, tkhd.0 + if init[tkhd.0] == 1 { 20 } else { 12 })?;
            let mdia = child(init, (s, e), b"mdia")?;
            let mdhd = child(init, mdia, b"mdhd")?;
            let timescale = be32(init, mdhd.0 + if init[mdhd.0] == 1 { 20 } else { 12 })?;
            let hdlr = child(init, mdia, b"hdlr")?;
            let video = init.get(hdlr.0 + 8..hdlr.0 + 12)? == b"vide";
            Some((id, Track { timescale, video }))
        };
        if let Some((id, t)) = parse() {
            out.insert(id, t);
        }
    }
    out
}

struct Traf {
    track: u32,
    /// absolute offset of the tfdt value and its width in bytes
    tfdt_at: usize,
    wide: bool,
    tfdt: u64,
    /// composition offset of the first sample (ticks)
    first_cts: i64,
    /// Sum of the sample durations (ticks); `None` when they come from `trex` (not parsed).
    duration: Option<u64>,
}

fn trafs(d: &[u8]) -> Vec<Traf> {
    let mut out = Vec::new();
    for (ms, me, mk) in boxes(d, 0) {
        if &mk != b"moof" {
            continue;
        }
        for (ts, te, tk) in boxes(&d[ms..me], ms) {
            if &tk != b"traf" {
                continue;
            }
            let parse = || -> Option<Traf> {
                let tfhd = child(d, (ts, te), b"tfhd")?;
                let tfdt = child(d, (ts, te), b"tfdt")?;
                let wide = d[tfdt.0] == 1;
                let tfdt_val = if wide {
                    u64::from_be_bytes(d.get(tfdt.0 + 4..tfdt.0 + 12)?.try_into().ok()?)
                } else {
                    be32(d, tfdt.0 + 4)? as u64
                };
                let mut first_cts = 0i64;
                // tfhd: flags, track id, [base data offset 8], [sample description 4],
                // [default sample duration 4], …
                let tfhd_flags = be32(d, tfhd.0)? & 0xFF_FFFF;
                let mut q = tfhd.0 + 8;
                if tfhd_flags & 1 != 0 {
                    q += 8;
                }
                if tfhd_flags & 2 != 0 {
                    q += 4;
                }
                let default_dur = if tfhd_flags & 8 != 0 {
                    Some(be32(d, q)? as u64)
                } else {
                    None
                };
                let mut duration = None;
                if let Some(trun) = child(d, (ts, te), b"trun") {
                    let flags = be32(d, trun.0)? & 0xFF_FFFF;
                    let count = be32(d, trun.0 + 4)? as usize;
                    let mut p = trun.0 + 8;
                    if flags & 1 != 0 {
                        p += 4;
                    }
                    if flags & 4 != 0 {
                        p += 4;
                    }
                    let fields = [0x100, 0x200, 0x400, 0x800]
                        .iter()
                        .filter(|b| flags & **b != 0)
                        .count();
                    if flags & 0x800 != 0 && count > 0 {
                        let at = p + 4 * (fields - 1);
                        let v = be32(d, at)?;
                        first_cts = if d[trun.0] == 1 {
                            v as i32 as i64
                        } else {
                            v as i64
                        };
                    }
                    duration = if flags & 0x100 != 0 {
                        let mut sum = 0u64;
                        for i in 0..count {
                            sum += be32(d, p + i * 4 * fields)? as u64;
                        }
                        Some(sum)
                    } else {
                        default_dur.map(|dd| dd * count as u64)
                    };
                }
                Some(Traf {
                    track: be32(d, tfhd.0 + 4)?,
                    tfdt_at: tfdt.0 + 4,
                    wide,
                    tfdt: tfdt_val,
                    first_cts,
                    duration,
                })
            };
            if let Some(t) = parse() {
                out.push(t);
            }
        }
    }
    out
}

/// Presentation time (seconds) of the first video sample in decode order.
pub fn first_video_pts(data: &[u8], tracks: &HashMap<u32, Track>) -> Option<f64> {
    let ts = trafs(data);
    let t = ts
        .iter()
        .find(|t| tracks.get(&t.track).is_some_and(|k| k.video))?;
    let scale = tracks[&t.track].timescale as f64;
    Some((t.tfdt as f64 + t.first_cts as f64) / scale)
}

/// Total duration (seconds) of the video samples in `data` (all fragments); `None` if a fragment
/// doesn't carry its sample durations.
pub fn video_duration(data: &[u8], tracks: &HashMap<u32, Track>) -> Option<f64> {
    let mut total = 0u64;
    let mut scale = None;
    for t in trafs(data) {
        let Some(k) = tracks.get(&t.track).filter(|k| k.video) else {
            continue;
        };
        scale = Some(k.timescale as f64);
        total += t.duration?;
    }
    Some(total as f64 / scale?)
}

/// `tfdt` (seconds) of the first video fragment.
pub fn first_video_tfdt(data: &[u8], tracks: &HashMap<u32, Track>) -> Option<f64> {
    let ts = trafs(data);
    let t = ts
        .iter()
        .find(|t| tracks.get(&t.track).is_some_and(|k| k.video))?;
    Some(t.tfdt as f64 / tracks[&t.track].timescale as f64)
}

/// Add `delta` seconds to every `tfdt` in `data` (all tracks alike, keeping A/V sync). Audio is
/// clamped at 0; a video track that would go negative leaves the buffer untouched (`false`).
pub fn shift(data: &mut [u8], tracks: &HashMap<u32, Track>, delta: f64) -> bool {
    let ts = trafs(data);
    // compute everything first so a failure leaves the buffer untouched
    let mut edits = Vec::with_capacity(ts.len());
    for t in &ts {
        let Some(k) = tracks.get(&t.track) else {
            return false;
        };
        let mut new = t.tfdt as i64 + (delta * k.timescale as f64).round() as i64;
        if new < 0 && !k.video {
            new = 0; // audio may lead video by a few ms at the very start of the file
        }
        if new < 0 || (!t.wide && new > u32::MAX as i64) {
            return false;
        }
        edits.push((t.tfdt_at, t.wide, new as u64));
    }
    for (at, wide, v) in edits {
        if wide {
            data[at..at + 8].copy_from_slice(&v.to_be_bytes());
        } else {
            data[at..at + 4].copy_from_slice(&(v as u32).to_be_bytes());
        }
    }
    true
}

/// Shift all `tfdt` values in `data` so that the first video sample's presentation time becomes
/// `expected_video_pts` (seconds). Returns the applied shift in seconds (`None` if refused).
pub fn retime(
    data: &mut [u8],
    tracks: &HashMap<u32, Track>,
    expected_video_pts: f64,
) -> Option<f64> {
    let delta = expected_video_pts - first_video_pts(data, tracks)?;
    if delta.abs() < 0.0005 {
        return Some(0.0);
    }
    shift(data, tracks, delta).then_some(delta)
}

/// A validated media segment split into its (optional) `styp` and the `moof`/`mdat` body.
#[derive(Debug, Clone, PartialEq)]
pub struct Cleaned {
    pub styp: Option<Vec<u8>>,
    /// `moof`+`mdat` (and `emsg`/`prft`) boxes only: no `sidx` (it would disagree with retimed
    /// `tfdt` values), no further `styp`.
    pub body: Vec<u8>,
}

/// Validate a media segment produced by ffmpeg and strip boxes we must not forward.
///
/// Errors if the bytes are not a clean `[styp] [sidx]* (moof mdat)+` sequence covering the whole
/// buffer (truncated file, a stray `ftyp`/`moov`, MPEG-TS, …) or if a `moof` references a track
/// that is not in the init segment.
pub fn clean_segment(data: &[u8], tracks: &HashMap<u32, Track>) -> Result<Cleaned, String> {
    let mut pos = 0usize;
    let mut styp = None;
    let mut body = Vec::with_capacity(data.len());
    let mut pending_moof = false;
    let mut moofs = 0;
    while pos < data.len() {
        if pos + 8 > data.len() {
            return Err(format!("trailing {} bytes", data.len() - pos));
        }
        let size =
            u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
        let kind: [u8; 4] = [data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]];
        if size < 8 || pos + size > data.len() {
            return Err(format!(
                "box {:?} at {pos} has size {size} but only {} bytes remain",
                String::from_utf8_lossy(&kind),
                data.len() - pos
            ));
        }
        let b = &data[pos..pos + size];
        match &kind {
            b"styp" => {
                if styp.is_none() {
                    styp = Some(b.to_vec());
                }
            }
            b"sidx" | b"free" | b"skip" => {}
            b"moof" => {
                if pending_moof {
                    return Err("moof not followed by mdat".into());
                }
                for t in trafs(b) {
                    if !tracks.contains_key(&t.track) {
                        return Err(format!(
                            "moof references track {} missing from init",
                            t.track
                        ));
                    }
                }
                pending_moof = true;
                moofs += 1;
                body.extend_from_slice(b);
            }
            b"mdat" => {
                if !pending_moof {
                    return Err("mdat without preceding moof".into());
                }
                pending_moof = false;
                body.extend_from_slice(b);
            }
            b"emsg" | b"prft" => body.extend_from_slice(b),
            other => {
                return Err(format!(
                    "unexpected box {:?} in media segment",
                    String::from_utf8_lossy(other)
                ));
            }
        }
        pos += size;
    }
    if pending_moof {
        return Err("segment ends with a moof without mdat".into());
    }
    if moofs == 0 {
        return Err("segment contains no moof".into());
    }
    Ok(Cleaned { styp, body })
}

/// Renumber the `mfhd` sequence numbers of all fragments in `data` as `base, base+1, …` so
/// fragments stitched together from different ffmpeg runs stay strictly increasing.
pub fn renumber(data: &mut [u8], base: u32) {
    let mut seq = base;
    for (ms, me, mk) in boxes(data, 0) {
        if &mk != b"moof" {
            continue;
        }
        if let Some(mfhd) = child(data, (ms, me), b"mfhd") {
            if mfhd.0 + 8 <= mfhd.1 {
                data[mfhd.0 + 4..mfhd.0 + 8].copy_from_slice(&seq.to_be_bytes());
                seq += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bx(k: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut v = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(k);
        v.extend_from_slice(payload);
        v
    }

    #[test]
    fn clean_segment_validates_and_strips() {
        let tracks = HashMap::from([(
            1,
            Track {
                timescale: 1000,
                video: true,
            },
        )]);
        let traf = |id: u32| {
            let mut tfhd = vec![0, 0, 0, 0];
            tfhd.extend(id.to_be_bytes());
            let mut tf = vec![1, 0, 0, 0];
            tf.extend(0u64.to_be_bytes());
            let mut trun = vec![0, 0, 0, 0];
            trun.extend(0u32.to_be_bytes());
            bx(
                b"traf",
                &[bx(b"tfhd", &tfhd), bx(b"tfdt", &tf), bx(b"trun", &trun)].concat(),
            )
        };
        let good = [
            bx(b"styp", b"msdh"),
            bx(b"sidx", &[0; 8]),
            bx(b"moof", &traf(1)),
            bx(b"mdat", b"xx"),
            bx(b"styp", b"msdh"),
            bx(b"moof", &traf(1)),
            bx(b"mdat", b"yy"),
        ]
        .concat();
        let c = clean_segment(&good, &tracks).unwrap();
        assert_eq!(c.styp, Some(bx(b"styp", b"msdh")));
        assert_eq!(
            c.body,
            [
                bx(b"moof", &traf(1)),
                bx(b"mdat", b"xx"),
                bx(b"moof", &traf(1)),
                bx(b"mdat", b"yy")
            ]
            .concat()
        );
        // truncated mdat
        assert!(clean_segment(&good[..good.len() - 3], &tracks).is_err());
        // stray init boxes / ts / unknown track / missing mdat
        assert!(
            clean_segment(
                &[
                    bx(b"ftyp", b"isom"),
                    bx(b"moof", &traf(1)),
                    bx(b"mdat", b"x")
                ]
                .concat(),
                &tracks
            )
            .is_err()
        );
        assert!(clean_segment(&[0x47u8; 188], &tracks).is_err());
        assert!(
            clean_segment(
                &[bx(b"moof", &traf(9)), bx(b"mdat", b"x")].concat(),
                &tracks
            )
            .is_err()
        );
        assert!(clean_segment(&bx(b"moof", &traf(1)), &tracks).is_err());
        assert!(clean_segment(&[], &tracks).is_err());
    }

    #[test]
    fn retimes_video_and_audio_by_same_delta() {
        // trun flags 0x000900 = duration(0x100) + cts(0x800), no data offset
        let mk = |track: u32, tfdt: u64, cts: u32| {
            let mut tfhd = vec![0, 0, 0, 0];
            tfhd.extend(track.to_be_bytes());
            let mut tf = vec![1, 0, 0, 0];
            tf.extend(u64::to_be_bytes(tfdt));
            let mut trun = vec![1, 0, 0x09, 0x00];
            trun.extend(1u32.to_be_bytes());
            trun.extend(10u32.to_be_bytes()); // duration
            trun.extend(cts.to_be_bytes());
            [bx(b"tfhd", &tfhd), bx(b"tfdt", &tf), bx(b"trun", &trun)].concat()
        };
        let moof = bx(
            b"moof",
            &[bx(b"traf", &mk(1, 1000, 100)), bx(b"traf", &mk(2, 2000, 0))].concat(),
        );
        let mut data = moof;
        let tracks = HashMap::from([
            (
                1,
                Track {
                    timescale: 1000,
                    video: true,
                },
            ),
            (
                2,
                Track {
                    timescale: 2000,
                    video: false,
                },
            ),
        ]);
        // video first pts = (1000+100)/1000 = 1.1s; want 2.0 => +0.9s
        let d = retime(&mut data, &tracks, 2.0).unwrap();
        assert!((d - 0.9).abs() < 1e-9);
        let t = trafs(&data);
        assert_eq!(t[0].tfdt, 1900);
        assert_eq!(t[1].tfdt, 2000 + 1800);
        // already aligned → no change
        assert_eq!(retime(&mut data, &tracks, 2.0), Some(0.0));
        // video would go negative → refused, untouched
        let before = data.clone();
        assert_eq!(retime(&mut data, &tracks, -5.0), None);
        assert_eq!(data, before);
        // audio is clamped at 0 instead of failing
        let mut d2 = bx(
            b"moof",
            &[bx(b"traf", &mk(1, 1000, 0)), bx(b"traf", &mk(2, 100, 0))].concat(),
        );
        assert!(retime(&mut d2, &tracks, 0.5).is_some());
        assert_eq!(trafs(&d2)[1].tfdt, 0);
    }
}
