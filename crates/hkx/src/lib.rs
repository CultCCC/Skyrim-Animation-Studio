//! Skyrim SE animation files (hk_2010.2.0-r1, 64-bit packfiles).
//!
//! Stage 0 reads what the timeline needs: the clip's kind, duration, frame
//! count, track count and every annotation with its track. Pose decoding and
//! writing come with stage 1.

pub mod anim;
pub mod write;

use havok_classes::Classes;
use std::fmt;

pub type ClassMap<'a> = indexmap::IndexMap<usize, Classes<'a>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimKind {
    Spline,
    Interleaved,
}

impl fmt::Display for AnimKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            AnimKind::Spline => "spline",
            AnimKind::Interleaved => "interleaved",
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Annotation {
    pub time: f32,
    pub text: String,
    /// Annotation track (= bone track) it is stored on. Skyrim keeps nearly
    /// all of them on track 0.
    pub track: u16,
}

#[derive(Debug, Clone)]
pub struct ClipInfo {
    pub kind: AnimKind,
    pub duration: f32,
    /// Frames as authored (`numOriginalFrames` in hkanno's dump).
    pub frames: u32,
    pub transform_tracks: u32,
    pub float_tracks: u32,
    pub annotation_tracks: u32,
    pub track_names: Vec<String>,
    pub skeleton_name: String,
    pub has_extracted_motion: bool,
    /// Sorted by time, ties kept in file order.
    pub annotations: Vec<Annotation>,
}

impl ClipInfo {
    /// Frame step of the source: duration / (frames - 1).
    pub fn frame_time(&self) -> f32 {
        if self.frames > 1 {
            self.duration / (self.frames - 1) as f32
        } else {
            1.0 / 30.0
        }
    }
}

#[derive(Debug)]
pub enum HkxError {
    Tagfile,
    Parse(String),
    NoAnimation,
}

impl fmt::Display for HkxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HkxError::Tagfile => f.write_str("Havok tagfile (not a packfile) — not supported yet"),
            HkxError::Parse(e) => write!(f, "HKX parse error: {e}"),
            HkxError::NoAnimation => f.write_str("no hkaAnimation in the file (behavior or skeleton?)"),
        }
    }
}

impl std::error::Error for HkxError {}

const PACKFILE_MAGIC: [u8; 8] = [0x57, 0xE0, 0xE0, 0x57, 0x10, 0xC0, 0xC0, 0x10];
const TAGFILE_MAGIC: [u8; 4] = [0xCA, 0xB0, 0x0D, 0x1E];

pub fn parse(bytes: &[u8]) -> Result<ClassMap<'_>, HkxError> {
    if bytes.len() >= 4 && bytes[..4] == TAGFILE_MAGIC {
        return Err(HkxError::Tagfile);
    }
    if bytes.len() < 8 || bytes[..8] != PACKFILE_MAGIC {
        return Err(HkxError::Parse("not a Havok packfile".into()));
    }
    serde_hkx::from_bytes(bytes).map_err(|e| HkxError::Parse(e.to_string()))
}

fn s(p: &havok_types::StringPtr<'_>) -> String {
    p.as_str().to_string()
}

pub fn read_clip(bytes: &[u8]) -> Result<ClipInfo, HkxError> {
    let map = parse(bytes)?;
    clip_from_map(&map)
}

pub fn clip_from_map(map: &ClassMap<'_>) -> Result<ClipInfo, HkxError> {
    let mut skeleton_name = String::new();
    for c in map.values() {
        if let Classes::hkaAnimationBinding(b) = c {
            skeleton_name = s(&b.m_originalSkeletonName);
            break;
        }
    }
    for c in map.values() {
        let (kind, anim, frames) = match c {
            Classes::hkaSplineCompressedAnimation(a) => (AnimKind::Spline, &a.parent, a.m_numFrames.max(0) as u32),
            Classes::hkaInterleavedUncompressedAnimation(a) => {
                let tracks = a.parent.m_numberOfTransformTracks.max(1) as usize;
                (AnimKind::Interleaved, &a.parent, (a.m_transforms.len() / tracks) as u32)
            }
            _ => continue,
        };
        let mut annotations = Vec::new();
        let mut track_names = Vec::new();
        for (ti, t) in anim.m_annotationTracks.iter().enumerate() {
            track_names.push(s(&t.m_trackName));
            for a in &t.m_annotations {
                annotations.push(Annotation { time: a.m_time, text: s(&a.m_text), track: ti as u16 });
            }
        }
        annotations.sort_by(|a, b| a.time.total_cmp(&b.time));
        let pointer: usize = anim.m_extractedMotion.clone().into();
        return Ok(ClipInfo {
            kind,
            duration: anim.m_duration,
            frames,
            transform_tracks: anim.m_numberOfTransformTracks.max(0) as u32,
            float_tracks: anim.m_numberOfFloatTracks.max(0) as u32,
            annotation_tracks: anim.m_annotationTracks.len() as u32,
            track_names,
            skeleton_name,
            has_extracted_motion: pointer != 0,
            annotations,
        });
    }
    // Other compressions exist in Havok but not in Skyrim SE content.
    for c in map.values() {
        if let Classes::hkaAnimationContainer(_) = c {
            return Err(HkxError::Parse("unsupported animation compression".into()));
        }
    }
    Err(HkxError::NoAnimation)
}
