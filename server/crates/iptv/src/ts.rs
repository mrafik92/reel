//! MPEG-TS packet analysis: PAT/PMT, keyframe (GOP start) detection, continuity + PTS health.
//!
//! Everything here works on whole 188-byte packets and never allocates per packet.

use bytes::Bytes;

pub const PKT: usize = 188;
pub const SYNC: u8 = 0x47;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    Mpeg2,
    Mpeg4,
    H264,
    Hevc,
}

impl VideoCodec {
    pub fn name(self) -> &'static str {
        match self {
            Self::Mpeg2 => "mpeg2video",
            Self::Mpeg4 => "mpeg4",
            Self::H264 => "h264",
            Self::Hevc => "hevc",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioCodec {
    Mp2,
    Mp3,
    /// ADTS AAC (stream type 0x0F): copied into fMP4 with `aac_adtstoasc`.
    Aac,
    /// LATM/LOAS AAC (stream type 0x11, common on DVB). `aac_adtstoasc` can't repackage it, and a
    /// copied track fails to parse in browsers (#93), so HLS re-encodes it.
    AacLatm,
    Ac3,
    Eac3,
    Other,
}

impl AudioCodec {
    pub fn name(self) -> &'static str {
        match self {
            Self::Mp2 => "mp2",
            Self::Mp3 => "mp3",
            Self::Aac => "aac",
            Self::AacLatm => "aac_latm",
            Self::Ac3 => "ac3",
            Self::Eac3 => "eac3",
            Self::Other => "other",
        }
    }
}

/// What the PMT says is in the stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamInfo {
    pub pmt_pid: u16,
    pub video_pid: Option<u16>,
    pub video: Option<VideoCodec>,
    pub audio: Vec<(u16, AudioCodec)>,
}

/// A parsed packet header (payload borrowed).
#[derive(Debug)]
pub struct Packet<'a> {
    pub pid: u16,
    pub pusi: bool,
    pub cc: u8,
    pub has_payload: bool,
    pub random_access: bool,
    pub discontinuity: bool,
    pub payload: &'a [u8],
}

pub fn parse_packet(p: &[u8]) -> Option<Packet<'_>> {
    if p.len() < PKT || p[0] != SYNC {
        return None;
    }
    let pid = (u16::from(p[1] & 0x1F) << 8) | u16::from(p[2]);
    let afc = (p[3] >> 4) & 3;
    let mut start = 4;
    let (mut random_access, mut discontinuity) = (false, false);
    if afc & 2 != 0 {
        let len = p[4] as usize;
        if len > 0 && len <= 183 {
            random_access = p[5] & 0x40 != 0;
            discontinuity = p[5] & 0x80 != 0;
        }
        start = 5 + len;
    }
    let has_payload = afc & 1 != 0 && start < PKT;
    Some(Packet {
        pid,
        pusi: p[1] & 0x40 != 0,
        cc: p[3] & 0x0F,
        has_payload,
        random_access,
        discontinuity,
        payload: if has_payload { &p[start..PKT] } else { &[] },
    })
}

/// PSI section body of a packet with PUSI (skips the pointer field).
fn section(payload: &[u8]) -> Option<&[u8]> {
    let ptr = *payload.first()? as usize;
    payload.get(1 + ptr..)
}

/// Program number → PMT PID of the first real program in a PAT section.
pub fn parse_pat(payload: &[u8]) -> Option<u16> {
    let s = section(payload)?;
    if s.len() < 12 || s[0] != 0x00 {
        return None;
    }
    let len = (usize::from(s[1] & 0x0F) << 8) | usize::from(s[2]);
    let end = (3 + len).min(s.len()).saturating_sub(4);
    let mut i = 8;
    while i + 4 <= end {
        let program = (u16::from(s[i]) << 8) | u16::from(s[i + 1]);
        let pid = (u16::from(s[i + 2] & 0x1F) << 8) | u16::from(s[i + 3]);
        if program != 0 {
            return Some(pid);
        }
        i += 4;
    }
    None
}

/// Parse a (single-packet) PMT section.
pub fn parse_pmt(pmt_pid: u16, payload: &[u8]) -> Option<StreamInfo> {
    let s = section(payload)?;
    if s.len() < 16 || s[0] != 0x02 {
        return None;
    }
    let len = (usize::from(s[1] & 0x0F) << 8) | usize::from(s[2]);
    let end = (3 + len).min(s.len()).saturating_sub(4);
    let pil = (usize::from(s[10] & 0x0F) << 8) | usize::from(s[11]);
    let mut i = 12 + pil;
    let mut info = StreamInfo {
        pmt_pid,
        video_pid: None,
        video: None,
        audio: vec![],
    };
    while i + 5 <= end {
        let stype = s[i];
        let pid = (u16::from(s[i + 1] & 0x1F) << 8) | u16::from(s[i + 2]);
        let es_len = (usize::from(s[i + 3] & 0x0F) << 8) | usize::from(s[i + 4]);
        let desc = s.get(i + 5..(i + 5 + es_len).min(s.len())).unwrap_or(&[]);
        match stype {
            0x01 | 0x02 if info.video_pid.is_none() => {
                info.video_pid = Some(pid);
                info.video = Some(VideoCodec::Mpeg2);
            }
            0x10 if info.video_pid.is_none() => {
                info.video_pid = Some(pid);
                info.video = Some(VideoCodec::Mpeg4);
            }
            0x1B if info.video_pid.is_none() => {
                info.video_pid = Some(pid);
                info.video = Some(VideoCodec::H264);
            }
            0x24 if info.video_pid.is_none() => {
                info.video_pid = Some(pid);
                info.video = Some(VideoCodec::Hevc);
            }
            0x03 => info.audio.push((pid, AudioCodec::Mp2)),
            0x04 => info.audio.push((pid, AudioCodec::Mp2)),
            0x0F => info.audio.push((pid, AudioCodec::Aac)),
            0x11 => info.audio.push((pid, AudioCodec::AacLatm)),
            0x81 => info.audio.push((pid, AudioCodec::Ac3)),
            0x87 => info.audio.push((pid, AudioCodec::Eac3)),
            0x06 => {
                // PES private data: look at descriptors (DVB AC-3 / E-AC-3).
                let mut d = 0;
                while d + 2 <= desc.len() {
                    let (tag, l) = (desc[d], desc[d + 1] as usize);
                    match tag {
                        0x6A => info.audio.push((pid, AudioCodec::Ac3)),
                        0x7A => info.audio.push((pid, AudioCodec::Eac3)),
                        _ => {}
                    }
                    d += 2 + l;
                }
            }
            _ => {}
        }
        i += 5 + es_len;
    }
    (info.video_pid.is_some() || !info.audio.is_empty()).then_some(info)
}

/// Offset just past the PES header of a payload that starts a PES packet.
fn pes_payload_offset(p: &[u8]) -> Option<usize> {
    if p.len() < 9 || p[0] != 0 || p[1] != 0 || p[2] != 1 {
        return None;
    }
    Some((9 + p[8] as usize).min(p.len()))
}

/// Parse the 33-bit PTS of a PES start payload.
pub fn parse_pts(p: &[u8]) -> Option<u64> {
    if p.len() < 14 || p[0] != 0 || p[1] != 0 || p[2] != 1 || p[7] & 0x80 == 0 {
        return None;
    }
    let b = &p[9..14];
    Some(
        (u64::from(b[0] >> 1) & 0x07) << 30
            | u64::from(b[1]) << 22
            | (u64::from(b[2] >> 1)) << 15
            | u64::from(b[3]) << 7
            | u64::from(b[4] >> 1),
    )
}

/// Does the first packet of a video PES contain a keyframe marker?
///
/// H.264: SPS (7) or IDR slice (5); HEVC: VPS/SPS (32/33) or IRAP slice (16..=21);
/// MPEG-2: sequence header (0xB3) or GOP header (0xB8).
pub fn has_keyframe_marker(codec: VideoCodec, pes: &[u8]) -> bool {
    let Some(off) = pes_payload_offset(pes) else {
        return false;
    };
    let es = &pes[off..];
    let mut i = 0;
    while i + 4 <= es.len() {
        if es[i] == 0 && es[i + 1] == 0 && es[i + 2] == 1 {
            let b = es[i + 3];
            let hit = match codec {
                VideoCodec::H264 | VideoCodec::Mpeg4 => matches!(b & 0x1F, 5 | 7),
                VideoCodec::Hevc => matches!((b >> 1) & 0x3F, 16..=21 | 32 | 33),
                VideoCodec::Mpeg2 => matches!(b, 0xB3 | 0xB8),
            };
            if hit {
                return true;
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    false
}

/// Result of analysing one packet.
#[derive(Debug, Default, Clone, Copy)]
pub struct PacketEvent {
    /// First packet of a video access unit that starts a GOP.
    pub keyframe: bool,
}

/// Incremental analyser: keeps the latest PAT/PMT packets, the stream layout and health counters.
pub struct TsAnalyzer {
    pmt_pid: Option<u16>,
    pub info: Option<StreamInfo>,
    pat_pkt: Option<Bytes>,
    pmt_pkt: Option<Bytes>,
    psi_changed: bool,
    info_changed: bool,
    cc_last: Vec<u8>,
    last_pts: Option<u64>,
    last_audio_sync: Option<u64>,
    pub cc_errors: u64,
    pub pts_discontinuities: u64,
    pub packets: u64,
    pub keyframes: u64,
}

impl Default for TsAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

/// PTS jump (90 kHz ticks) treated as a discontinuity: backwards, or forwards by over 10 s.
const PTS_JUMP: i64 = 90_000 * 10;

impl TsAnalyzer {
    pub fn new() -> Self {
        Self {
            pmt_pid: None,
            info: None,
            pat_pkt: None,
            pmt_pkt: None,
            psi_changed: false,
            info_changed: false,
            cc_last: vec![0xFF; 8192],
            last_pts: None,
            last_audio_sync: None,
            cc_errors: 0,
            pts_discontinuities: 0,
            packets: 0,
            keyframes: 0,
        }
    }

    /// Forget stream state (after a reconnect), keeping lifetime counters.
    pub fn reset_stream(&mut self) {
        self.cc_last.iter_mut().for_each(|c| *c = 0xFF);
        self.last_pts = None;
        self.last_audio_sync = None;
    }

    /// Latest PAT and PMT packets, if both were seen.
    pub fn psi(&self) -> Option<(Bytes, Bytes)> {
        Some((self.pat_pkt.clone()?, self.pmt_pkt.clone()?))
    }

    /// True once after PAT/PMT packets changed.
    pub fn take_psi_changed(&mut self) -> bool {
        std::mem::take(&mut self.psi_changed)
    }

    /// `Some` once after the stream layout was (re)discovered.
    pub fn take_info_changed(&mut self) -> Option<StreamInfo> {
        if std::mem::take(&mut self.info_changed) {
            self.info.clone()
        } else {
            None
        }
    }

    /// Analyse one packet. `pkt` must be exactly 188 bytes starting with the sync byte.
    pub fn analyze(&mut self, pkt: &Bytes) -> PacketEvent {
        self.packets += 1;
        let Some(p) = parse_packet(pkt) else {
            return PacketEvent::default();
        };
        if p.pid == 0x1FFF {
            return PacketEvent::default();
        }
        let mut ev = PacketEvent::default();

        // Continuity counter (only for the elementary streams we know).
        let tracked = self.info.as_ref().is_some_and(|i| {
            Some(p.pid) == i.video_pid || i.audio.iter().any(|(a, _)| *a == p.pid)
        });
        if p.has_payload {
            let slot = &mut self.cc_last[p.pid as usize];
            if tracked && *slot != 0xFF && !p.discontinuity {
                let expected = (*slot + 1) & 0x0F;
                // A repeated counter is a legal duplicate packet.
                if p.cc != expected && p.cc != *slot {
                    self.cc_errors += 1;
                }
            }
            *slot = p.cc;
        }

        if p.pid == 0 && p.pusi {
            if let Some(pmt) = parse_pat(p.payload) {
                if self.pmt_pid != Some(pmt) {
                    self.pmt_pid = Some(pmt);
                }
                if self.pat_pkt.as_deref() != Some(&pkt[..]) {
                    self.pat_pkt = Some(pkt.clone());
                    self.psi_changed = true;
                }
            }
            return ev;
        }
        if Some(p.pid) == self.pmt_pid && p.pusi {
            if let Some(info) = parse_pmt(p.pid, p.payload) {
                if self.info.as_ref() != Some(&info) {
                    self.info = Some(info);
                    self.info_changed = true;
                }
                if self.pmt_pkt.as_deref() != Some(&pkt[..]) {
                    self.pmt_pkt = Some(pkt.clone());
                    self.psi_changed = true;
                }
            }
            return ev;
        }

        let Some(info) = &self.info else { return ev };
        if p.pusi && p.has_payload {
            if Some(p.pid) == info.video_pid {
                let codec = info.video.unwrap_or(VideoCodec::H264);
                ev.keyframe = p.random_access || has_keyframe_marker(codec, p.payload);
                if ev.keyframe {
                    self.keyframes += 1;
                }
                if let Some(pts) = parse_pts(p.payload) {
                    if let Some(prev) = self.last_pts {
                        let d = pts as i64 - prev as i64;
                        // B-frames reorder PTS slightly backwards; only large jumps count.
                        if !(-(90_000 / 2)..=PTS_JUMP).contains(&d) {
                            self.pts_discontinuities += 1;
                        }
                    }
                    self.last_pts = Some(pts);
                }
            } else if info.video_pid.is_none() && info.audio.first().map(|a| a.0) == Some(p.pid) {
                // Audio-only channel: use PES starts at most once per ~second as sync points.
                if let Some(pts) = parse_pts(p.payload) {
                    let due = self
                        .last_audio_sync
                        .is_none_or(|l| pts.wrapping_sub(l) >= 90_000);
                    if due {
                        self.last_audio_sync = Some(pts);
                        ev.keyframe = true;
                        self.keyframes += 1;
                    }
                }
            }
        }
        ev
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pes(nals: &[&[u8]]) -> Vec<u8> {
        let mut v = vec![0, 0, 1, 0xE0, 0, 0, 0x80, 0x80, 5, 0x21, 0, 1, 0, 1];
        for n in nals {
            v.extend_from_slice(&[0, 0, 0, 1]);
            v.extend_from_slice(n);
        }
        v
    }

    #[test]
    fn h264_marker_detection() {
        let idr = pes(&[&[0x09, 0xF0], &[0x65, 0x88]]);
        let sps = pes(&[&[0x09, 0xF0], &[0x67, 0x42], &[0x68, 0xCE]]);
        let p_frame = pes(&[&[0x09, 0x30], &[0x41, 0x9A]]);
        assert!(has_keyframe_marker(VideoCodec::H264, &idr));
        assert!(has_keyframe_marker(VideoCodec::H264, &sps));
        assert!(!has_keyframe_marker(VideoCodec::H264, &p_frame));
    }

    #[test]
    fn hevc_and_mpeg2_markers() {
        // IDR_W_RADL = 19 → header byte 19<<1; VPS = 32<<1; TRAIL_R = 1<<1.
        assert!(has_keyframe_marker(
            VideoCodec::Hevc,
            &pes(&[&[19 << 1, 1]])
        ));
        assert!(has_keyframe_marker(
            VideoCodec::Hevc,
            &pes(&[&[32 << 1, 1]])
        ));
        assert!(!has_keyframe_marker(
            VideoCodec::Hevc,
            &pes(&[&[1 << 1, 1]])
        ));
        let mut m2 = pes(&[]);
        m2.extend_from_slice(&[0, 0, 1, 0xB3, 0x16, 0]);
        assert!(has_keyframe_marker(VideoCodec::Mpeg2, &m2));
        let mut pic = pes(&[]);
        pic.extend_from_slice(&[0, 0, 1, 0x00, 0, 0]);
        assert!(!has_keyframe_marker(VideoCodec::Mpeg2, &pic));
    }

    #[test]
    fn pts_roundtrip() {
        // PTS = 90_000 (1 s): marker bits '0010' + pts[32..30] ...
        let pts: u64 = 90_000;
        let b = [
            0x21 | (((pts >> 30) as u8 & 7) << 1),
            (pts >> 22) as u8,
            ((pts >> 15) as u8 & 0x7F) << 1 | 1,
            (pts >> 7) as u8,
            ((pts as u8) & 0x7F) << 1 | 1,
        ];
        let mut p = vec![0, 0, 1, 0xE0, 0, 0, 0x80, 0x80, 5];
        p.extend_from_slice(&b);
        assert_eq!(parse_pts(&p), Some(90_000));
    }

    fn psi_packet(pid: u16, body: &[u8]) -> Bytes {
        let mut p = vec![0xFFu8; PKT];
        p[0] = SYNC;
        p[1] = 0x40 | (pid >> 8) as u8;
        p[2] = pid as u8;
        p[3] = 0x10;
        p[4] = 0; // pointer field
        p[5..5 + body.len()].copy_from_slice(body);
        Bytes::from(p)
    }

    #[test]
    fn pat_pmt_parse() {
        // PAT: program 1 → PMT PID 0x100
        let pat = [
            0x00, 0xB0, 0x0D, 0, 1, 0xC1, 0, 0, 0, 1, 0xE1, 0x00, 0, 0, 0, 0,
        ];
        let pkt = psi_packet(0, &pat);
        assert_eq!(parse_pat(parse_packet(&pkt).unwrap().payload), Some(0x100));
        // PMT: H264 on 0x101, AAC on 0x102, AC-3 (private + 0x6A) on 0x103
        let pmt = [
            0x02, 0xB0, 0x1F, 0, 1, 0xC1, 0, 0, 0xE1, 0x01, 0xF0, 0, 0x1B, 0xE1, 0x01, 0xF0, 0,
            0x0F, 0xE1, 0x02, 0xF0, 0, 0x06, 0xE1, 0x03, 0xF0, 0x03, 0x6A, 0x01, 0x00, 0, 0, 0, 0,
        ];
        let pkt = psi_packet(0x100, &pmt);
        let info = parse_pmt(0x100, parse_packet(&pkt).unwrap().payload).unwrap();
        assert_eq!(info.video_pid, Some(0x101));
        assert_eq!(info.video, Some(VideoCodec::H264));
        assert_eq!(
            info.audio,
            vec![(0x102, AudioCodec::Aac), (0x103, AudioCodec::Ac3)]
        );
    }
}
