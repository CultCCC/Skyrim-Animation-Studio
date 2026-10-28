//! Writing annotations back into a clip.
//!
//! The file is parsed with serde-hkx, only the animation's annotation tracks
//! are replaced, and it is serialized again as a Skyrim SE packfile. The
//! result is then read back and checked: same classes, same animation data
//! and binding, and exactly the annotations asked for. A file that does not
//! pass is never returned.

use crate::{clip_from_map, parse, Annotation, ClassMap, HkxError};
use havok_classes::{hkaAnimation, hkaAnnotationTrackAnnotation, Classes};
use serde_hkx::bytes::serde::hkx_header::HkxHeader;
use std::borrow::Cow;

fn animation_mut<'a, 'b>(map: &'b mut ClassMap<'a>) -> Option<&'b mut hkaAnimation<'a>> {
    for c in map.values_mut() {
        match c {
            Classes::hkaSplineCompressedAnimation(a) => return Some(&mut a.parent),
            Classes::hkaInterleavedUncompressedAnimation(a) => return Some(&mut a.parent),
            _ => {}
        }
    }
    None
}

fn set_annotations<'a>(map: &mut ClassMap<'a>, annos: &[Annotation]) -> Result<(), HkxError> {
    let anim = animation_mut(map).ok_or(HkxError::NoAnimation)?;
    if anim.m_annotationTracks.is_empty() {
        return Err(HkxError::Parse("clip has no annotation tracks to write into".into()));
    }
    for t in anim.m_annotationTracks.iter_mut() {
        t.m_annotations.clear();
    }
    let last = anim.m_annotationTracks.len() - 1;
    for a in annos {
        let track = (a.track as usize).min(last);
        let mut item = hkaAnnotationTrackAnnotation::default();
        item.m_time = a.time;
        item.m_text = Cow::<'a, str>::Owned(a.text.clone()).into();
        anim.m_annotationTracks[track].m_annotations.push(item);
    }
    Ok(())
}

/// Debug text of every class with the annotation tracks emptied: what must
/// not change when only annotations are edited.
fn fingerprint(map: &ClassMap<'_>) -> String {
    let mut map = map.clone();
    if let Some(anim) = animation_mut(&mut map) {
        for t in anim.m_annotationTracks.iter_mut() {
            t.m_annotations.clear();
        }
    }
    format!("{map:?}")
}

/// New file bytes: `original` with its annotations replaced by `annos`.
pub fn write_annotations(original: &[u8], annos: &[Annotation]) -> Result<Vec<u8>, HkxError> {
    // Header byte 16 = pointer size, 17 = little endian flag.
    if original.len() < 64 || original[16] != 8 || original[17] != 1 {
        return Err(HkxError::Parse("only 64-bit little-endian (Skyrim SE) packfiles can be written".into()));
    }
    let src = parse(original)?;
    let mut map = src.clone();
    set_annotations(&mut map, annos)?;
    let bytes = serde_hkx::to_bytes(&map, &HkxHeader::new_skyrim_se()).map_err(|e| HkxError::Parse(format!("serialize: {e}")))?;
    verify(&src, &bytes, annos)?;
    Ok(bytes)
}

fn verify(src: &ClassMap<'_>, written: &[u8], annos: &[Annotation]) -> Result<(), HkxError> {
    let back = parse(written).map_err(|e| HkxError::Parse(format!("written file does not read back: {e}")))?;
    if fingerprint(&back) != fingerprint(src) {
        return Err(HkxError::Parse("written file differs outside the annotations".into()));
    }
    let info = clip_from_map(&back)?;
    let mut want: Vec<Annotation> = annos.to_vec();
    want.sort_by(|a, b| a.time.total_cmp(&b.time));
    let mut got = info.annotations;
    got.sort_by(|a, b| a.time.total_cmp(&b.time));
    let clamp = |v: &mut Vec<Annotation>| {
        for a in v.iter_mut() {
            a.track = a.track.min(info.annotation_tracks.saturating_sub(1) as u16);
        }
    };
    clamp(&mut want);
    let key = |a: &Annotation| (a.time.to_bits(), a.text.clone(), a.track);
    let mut w: Vec<_> = want.iter().map(key).collect();
    let mut g: Vec<_> = got.iter().map(key).collect();
    w.sort();
    g.sort();
    if w != g {
        return Err(HkxError::Parse("annotations read back differ from the ones written".into()));
    }
    Ok(())
}
