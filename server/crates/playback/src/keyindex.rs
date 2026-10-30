//! Native keyframe index readers (Matroska/WebM cues, MP4/MOV `stss`).
//!
//! They read container *metadata only* (a few KB to ~1 MB even for multi-GB files), unlike
//! `ffprobe -show_packets` which has to read the whole file (minutes over NFS). Times are
//! returned in seconds relative to the first keyframe.

use std::io::{self, Read, Seek, SeekFrom};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Largest metadata blob (Tracks/Cues/stts…) we are willing to load into memory.
const MAX_BODY: u64 = 64 * 1024 * 1024;

/// Reader wrapper enforcing a deadline and counting the bytes actually read.
pub struct Limited<R> {
    inner: R,
    deadline: Option<Instant>,
    bytes: Arc<AtomicU64>,
}

impl<R> Limited<R> {
    pub fn new(inner: R, timeout: Option<Duration>) -> Self {
        Limited {
            inner,
            deadline: timeout.map(|t| Instant::now() + t),
            bytes: Arc::new(AtomicU64::new(0)),
        }
    }
    /// Shared counter of bytes read so far.
    pub fn counter(&self) -> Arc<AtomicU64> {
        self.bytes.clone()
    }
    fn check(&self) -> io::Result<()> {
        match self.deadline {
            Some(d) if Instant::now() >= d => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "keyframe index read deadline exceeded",
            )),
            _ => Ok(()),
        }
    }
}

impl<R: Read> Read for Limited<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.check()?;
        let n = self.inner.read(buf)?;
        self.bytes.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

impl<R: Seek> Seek for Limited<R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.check()?;
        self.inner.seek(pos)
    }
}

fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

fn read_body<R: Read>(r: &mut R, size: u64) -> io::Result<Vec<u8>> {
    if size > MAX_BODY {
        return Err(bad("metadata element too large"));
    }
    let mut v = vec![0u8; size as usize];
    r.read_exact(&mut v)?;
    Ok(v)
}

fn relative(mut v: Vec<f64>) -> Vec<f64> {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    v.dedup_by(|a, b| (*a - *b).abs() < 1e-4);
    if let Some(&first) = v.first() {
        for k in v.iter_mut() {
            *k -= first;
        }
    }
    v
}

/// Detect the container by magic and read its keyframe index.
/// `Ok(None)` = no usable index (caller should fall back).
pub fn read_keyframes<R: Read + Seek>(r: &mut R) -> io::Result<Option<Vec<f64>>> {
    r.seek(SeekFrom::Start(0))?;
    let mut magic = [0u8; 8];
    r.read_exact(&mut magic)?;
    if magic[..4] == [0x1A, 0x45, 0xDF, 0xA3] {
        mkv_keyframes(r)
    } else if matches!(
        &magic[4..8],
        b"ftyp" | b"moov" | b"mdat" | b"free" | b"wide" | b"skip"
    ) {
        mp4_keyframes(r)
    } else {
        Ok(None)
    }
}

// ------------------------------------------------------------------------------------------
// Matroska / WebM
// ------------------------------------------------------------------------------------------

const ID_EBML: u32 = 0x1A45DFA3;
const ID_SEGMENT: u32 = 0x18538067;
const ID_SEEKHEAD: u32 = 0x114D9B74;
const ID_SEEK: u32 = 0x4DBB;
const ID_SEEKID: u32 = 0x53AB;
const ID_SEEKPOS: u32 = 0x53AC;
const ID_INFO: u32 = 0x1549A966;
const ID_TSCALE: u32 = 0x2AD7B1;
const ID_TRACKS: u32 = 0x1654AE6B;
const ID_TRACKENTRY: u32 = 0xAE;
const ID_TRACKNUM: u32 = 0xD7;
const ID_TRACKTYPE: u32 = 0x83;
const ID_CUES: u32 = 0x1C53BB6B;
const ID_CUEPOINT: u32 = 0xBB;
const ID_CUETIME: u32 = 0xB3;
const ID_CUETRACKPOS: u32 = 0xB7;
const ID_CUETRACK: u32 = 0xF7;
const ID_CLUSTER: u32 = 0x1F43B675;

/// Parse an element id from `buf`; returns (id, bytes used).
fn ebml_id(buf: &[u8]) -> Option<(u32, usize)> {
    let b = *buf.first()?;
    let len = b.leading_zeros() as usize + 1;
    if len > 4 || buf.len() < len {
        return None;
    }
    let mut v = 0u32;
    for &x in &buf[..len] {
        v = (v << 8) | x as u32;
    }
    Some((v, len))
}

/// Parse an element size; `Some(None)` = unknown size. Returns (size, bytes used).
fn ebml_size(buf: &[u8]) -> Option<(Option<u64>, usize)> {
    let b = *buf.first()?;
    let len = b.leading_zeros() as usize + 1;
    if len > 8 || buf.len() < len {
        return None;
    }
    let mut v = (b as u64) & (0xFFu64 >> len);
    let mut all_ones = v == (0xFFu64 >> len);
    for &x in &buf[1..len] {
        v = (v << 8) | x as u64;
        all_ones &= x == 0xFF;
    }
    Some((if all_ones { None } else { Some(v) }, len))
}

/// Read one element header at the current position: (id, size, header length).
fn read_elem_header<R: Read>(r: &mut R) -> io::Result<Option<(u32, Option<u64>, u64)>> {
    let mut head = [0u8; 12];
    // read the first byte, then exactly the remaining id+size bytes
    let mut first = [0u8; 1];
    if r.read(&mut first)? == 0 {
        return Ok(None);
    }
    head[0] = first[0];
    let id_len = first[0].leading_zeros() as usize + 1;
    if id_len > 4 {
        return Err(bad("bad EBML id"));
    }
    r.read_exact(&mut head[1..id_len + 1])?;
    let size_len = head[id_len].leading_zeros() as usize + 1;
    if size_len > 8 {
        return Err(bad("bad EBML size"));
    }
    r.read_exact(&mut head[id_len + 1..id_len + size_len])?;
    let (id, _) = ebml_id(&head[..id_len]).ok_or_else(|| bad("bad id"))?;
    let (size, _) = ebml_size(&head[id_len..id_len + size_len]).ok_or_else(|| bad("bad size"))?;
    Ok(Some((id, size, (id_len + size_len) as u64)))
}

/// Iterate the child elements in a fully loaded body.
fn children(buf: &[u8]) -> Vec<(u32, &[u8])> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < buf.len() {
        let Some((id, il)) = ebml_id(&buf[pos..]) else {
            break;
        };
        let Some((Some(size), sl)) = ebml_size(&buf[pos + il..]) else {
            break;
        };
        let start = pos + il + sl;
        let end = start.saturating_add(size as usize);
        if end > buf.len() {
            break;
        }
        out.push((id, &buf[start..end]));
        pos = end;
    }
    out
}

fn uint(b: &[u8]) -> u64 {
    b.iter().take(8).fold(0u64, |a, &x| (a << 8) | x as u64)
}

fn parse_cues(body: &[u8], video_track: Option<u64>, scale_ns: u64) -> Vec<f64> {
    let mut out = Vec::new();
    for (id, cp) in children(body) {
        if id != ID_CUEPOINT {
            continue;
        }
        let mut time = None;
        let mut tracks = Vec::new();
        for (cid, cb) in children(cp) {
            match cid {
                ID_CUETIME => time = Some(uint(cb)),
                ID_CUETRACKPOS => {
                    for (tid, tb) in children(cb) {
                        if tid == ID_CUETRACK {
                            tracks.push(uint(tb));
                        }
                    }
                }
                _ => {}
            }
        }
        let Some(t) = time else { continue };
        if video_track.is_none_or(|v| tracks.contains(&v)) {
            out.push(t as f64 * scale_ns as f64 / 1e9);
        }
    }
    out
}

fn parse_cues_at<R: Read + Seek>(
    r: &mut R,
    pos: u64,
    track: Option<u64>,
    scale: u64,
) -> io::Result<Option<Vec<f64>>> {
    r.seek(SeekFrom::Start(pos))?;
    match read_elem_header(r)? {
        Some((ID_CUES, Some(size), _)) => {
            let body = read_body(r, size)?;
            Ok(Some(parse_cues(&body, track, scale)))
        }
        _ => Ok(None),
    }
}

/// Read Matroska/WebM CuePoints for the first video track.
pub fn mkv_keyframes<R: Read + Seek>(r: &mut R) -> io::Result<Option<Vec<f64>>> {
    r.seek(SeekFrom::Start(0))?;
    let Some((ID_EBML, Some(size), hl)) = read_elem_header(r)? else {
        return Ok(None);
    };
    r.seek(SeekFrom::Start(hl + size))?;
    let Some((ID_SEGMENT, seg_size, hl2)) = read_elem_header(r)? else {
        return Ok(None);
    };
    let seg_data = r.stream_position()?;
    let _ = hl2;
    let seg_end = seg_size.map(|s| seg_data + s);

    let mut scale = 1_000_000u64;
    let mut video_track: Option<u64> = None;
    let mut cues_pos: Option<u64> = None;
    let mut pos = seg_data;
    loop {
        if seg_end.is_some_and(|e| pos >= e) {
            return Ok(None);
        }
        r.seek(SeekFrom::Start(pos))?;
        let Some((id, size, hl)) = read_elem_header(r)? else {
            return Ok(None);
        };
        let body = pos + hl;
        match id {
            ID_CLUSTER => {
                if let Some(cp) = cues_pos {
                    return Ok(parse_cues_at(r, cp, video_track, scale)?
                        .map(relative)
                        .filter(|v| !v.is_empty()));
                }
                // no SeekHead pointer: keep scanning top-level elements, skipping clusters
            }
            ID_CUES => {
                let Some(size) = size else { return Ok(None) };
                let b = read_body(r, size)?;
                let v = parse_cues(&b, video_track, scale);
                return Ok(Some(relative(v)).filter(|v| !v.is_empty()));
            }
            ID_SEEKHEAD | ID_INFO | ID_TRACKS => {
                let Some(size) = size else { return Ok(None) };
                let b = read_body(r, size)?;
                match id {
                    ID_SEEKHEAD => {
                        for (sid, sb) in children(&b) {
                            if sid != ID_SEEK {
                                continue;
                            }
                            let (mut what, mut at) = (None, None);
                            for (k, v) in children(sb) {
                                match k {
                                    ID_SEEKID => what = Some(uint(v)),
                                    ID_SEEKPOS => at = Some(uint(v)),
                                    _ => {}
                                }
                            }
                            if what == Some(ID_CUES as u64) {
                                cues_pos = at.map(|a| seg_data + a);
                            }
                        }
                    }
                    ID_INFO => {
                        for (k, v) in children(&b) {
                            if k == ID_TSCALE {
                                scale = uint(v).max(1);
                            }
                        }
                    }
                    _ => {
                        for (k, te) in children(&b) {
                            if k != ID_TRACKENTRY || video_track.is_some() {
                                continue;
                            }
                            let (mut num, mut ty) = (None, None);
                            for (f, v) in children(te) {
                                match f {
                                    ID_TRACKNUM => num = Some(uint(v)),
                                    ID_TRACKTYPE => ty = Some(uint(v)),
                                    _ => {}
                                }
                            }
                            if ty == Some(1) {
                                video_track = num;
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        // unknown-size elements cannot be skipped
        let Some(size) = size else { return Ok(None) };
        pos = body + size;
    }
}

// ------------------------------------------------------------------------------------------
// MP4 / MOV
// ------------------------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct BoxRef {
    kind: [u8; 4],
    body: u64,
    end: u64,
}

fn mp4_children<R: Read + Seek>(r: &mut R, start: u64, end: u64) -> io::Result<Vec<BoxRef>> {
    let mut out = Vec::new();
    let mut pos = start;
    while pos + 8 <= end {
        r.seek(SeekFrom::Start(pos))?;
        let mut h = [0u8; 8];
        if r.read(&mut h[..1])? == 0 {
            break;
        }
        r.read_exact(&mut h[1..])?;
        let size32 = u32::from_be_bytes([h[0], h[1], h[2], h[3]]) as u64;
        let kind = [h[4], h[5], h[6], h[7]];
        let (hl, size) = match size32 {
            1 => {
                let mut b = [0u8; 8];
                r.read_exact(&mut b)?;
                (16, u64::from_be_bytes(b))
            }
            0 => (8, end - pos),
            n => (8, n),
        };
        if size < hl || pos + size > end {
            break;
        }
        out.push(BoxRef {
            kind,
            body: pos + hl,
            end: pos + size,
        });
        pos += size;
    }
    Ok(out)
}

fn find(boxes: &[BoxRef], kind: &[u8; 4]) -> Option<BoxRef> {
    boxes.iter().find(|b| &b.kind == kind).copied()
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn payload<R: Read + Seek>(r: &mut R, b: BoxRef) -> io::Result<Vec<u8>> {
    r.seek(SeekFrom::Start(b.body))?;
    read_body(r, b.end - b.body)
}

/// Read MP4/MOV sync-sample (`stss`) presentation times of the first video track.
pub fn mp4_keyframes<R: Read + Seek>(r: &mut R) -> io::Result<Option<Vec<f64>>> {
    let len = r.seek(SeekFrom::End(0))?;
    let top = mp4_children(r, 0, len)?;
    let Some(moov) = find(&top, b"moov") else {
        return Ok(None);
    };
    for trak in mp4_children(r, moov.body, moov.end)?
        .into_iter()
        .filter(|b| &b.kind == b"trak")
    {
        let t = mp4_children(r, trak.body, trak.end)?;
        let Some(mdia) = find(&t, b"mdia") else {
            continue;
        };
        let m = mp4_children(r, mdia.body, mdia.end)?;
        let (Some(hdlr), Some(mdhd), Some(minf)) =
            (find(&m, b"hdlr"), find(&m, b"mdhd"), find(&m, b"minf"))
        else {
            continue;
        };
        let h = payload(r, hdlr)?;
        if h.get(8..12) != Some(b"vide") {
            continue;
        }
        let md = payload(r, mdhd)?;
        let timescale = if md.first() == Some(&1) {
            be32(&md, 20)
        } else {
            be32(&md, 12)
        }
        .filter(|t| *t > 0);
        let Some(timescale) = timescale else {
            return Ok(None);
        };
        let Some(stbl) = find(&mp4_children(r, minf.body, minf.end)?, b"stbl") else {
            return Ok(None);
        };
        let s = mp4_children(r, stbl.body, stbl.end)?;
        let (Some(stss), Some(stts)) = (find(&s, b"stss"), find(&s, b"stts")) else {
            return Ok(None);
        };
        let stss = payload(r, stss)?;
        let stts = payload(r, stts)?;
        let ctts = match find(&s, b"ctts") {
            Some(b) => Some(payload(r, b)?),
            None => None,
        };
        return Ok(sync_times(&stss, &stts, ctts.as_deref(), timescale as f64)
            .map(relative)
            .filter(|v| !v.is_empty()));
    }
    Ok(None)
}

/// Compute presentation time of each sync sample from stss/stts/ctts payloads.
fn sync_times(stss: &[u8], stts: &[u8], ctts: Option<&[u8]>, timescale: f64) -> Option<Vec<f64>> {
    let n_sync = be32(stss, 4)? as usize;
    let n_stts = be32(stts, 4)? as usize;
    let n_ctts = ctts.map(|c| be32(c, 4).unwrap_or(0) as usize).unwrap_or(0);
    // cursors: (run index, first sample index of the run, dts at run start)
    let (mut ri, mut run_first, mut run_dts) = (0usize, 0u64, 0i64);
    let (mut ci, mut c_first) = (0usize, 0u64);
    let mut out = Vec::with_capacity(n_sync);
    for k in 0..n_sync {
        let sample = be32(stss, 8 + 4 * k)? as u64 - 1;
        // dts
        loop {
            let count = be32(stts, 8 + 8 * ri)? as u64;
            let delta = be32(stts, 12 + 8 * ri)? as i64;
            if sample < run_first + count {
                let dts = run_dts + (sample - run_first) as i64 * delta;
                let cts = match ctts {
                    Some(c) if n_ctts > 0 => loop {
                        let cc = be32(c, 8 + 8 * ci)? as u64;
                        if sample < c_first + cc {
                            break be32(c, 12 + 8 * ci)? as i32 as i64;
                        }
                        c_first += cc;
                        ci += 1;
                        if ci >= n_ctts {
                            break 0;
                        }
                    },
                    _ => 0,
                };
                out.push((dts + cts) as f64 / timescale);
                break;
            }
            run_first += count;
            run_dts += count as i64 * delta;
            ri += 1;
            if ri >= n_stts {
                return None;
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ebml_varints() {
        assert_eq!(ebml_id(&[0x1A, 0x45, 0xDF, 0xA3]), Some((0x1A45DFA3, 4)));
        assert_eq!(ebml_id(&[0xAE]), Some((0xAE, 1)));
        assert_eq!(ebml_size(&[0x81]), Some((Some(1), 1)));
        assert_eq!(ebml_size(&[0x40, 0x02]), Some((Some(2), 2)));
        assert_eq!(ebml_size(&[0xFF]), Some((None, 1)));
        assert_eq!(
            ebml_size(&[0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]),
            Some((None, 8))
        );
    }

    #[test]
    fn stts_ctts_math() {
        // 5 samples, delta 100, sync at samples 1 and 4; ctts: 2 samples +200, rest 0
        let mut stss = vec![0, 0, 0, 0, 0, 0, 0, 2];
        stss.extend(1u32.to_be_bytes());
        stss.extend(4u32.to_be_bytes());
        let mut stts = vec![0, 0, 0, 0, 0, 0, 0, 1];
        stts.extend(5u32.to_be_bytes());
        stts.extend(100u32.to_be_bytes());
        let mut ctts = vec![0, 0, 0, 0, 0, 0, 0, 2];
        for (c, o) in [(2u32, 200u32), (3, 0)] {
            ctts.extend(c.to_be_bytes());
            ctts.extend(o.to_be_bytes());
        }
        let v = sync_times(&stss, &stts, Some(&ctts), 100.0).unwrap();
        assert_eq!(v, vec![2.0, 3.0]);
    }
}
