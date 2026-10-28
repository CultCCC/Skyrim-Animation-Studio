//! The open clip: its file info, the editable annotation list with undo, and
//! the AMR root-motion path derived from it.

use sds_anno::{parse, Kind};
use sds_hkx::ClipInfo;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub enum ClipSource {
    /// Normalised path inside the load order.
    Vfs(String),
    Disk(PathBuf),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Anno {
    pub time: f32,
    pub text: String,
    pub track: u16,
}

pub struct ClipDoc {
    pub source: ClipSource,
    pub name: String,
    /// Mod that provides the file (MO2 name, "(BSA)" suffix, or the disk folder).
    pub origin: String,
    pub info: ClipInfo,
    pub annos: Vec<Anno>,
    saved: Vec<Anno>,
    undo: Vec<Vec<Anno>>,
    redo: Vec<Vec<Anno>>,
    /// Key of the last merged edit: a drag on one field is one undo step.
    last_merge: Option<String>,
    pub motion: MotionPath,
}

impl ClipDoc {
    pub fn new(source: ClipSource, name: String, origin: String, info: ClipInfo) -> ClipDoc {
        let annos: Vec<Anno> = info
            .annotations
            .iter()
            .map(|a| Anno { time: a.time, text: a.text.clone(), track: a.track })
            .collect();
        let motion = MotionPath::from_annos(&annos);
        ClipDoc { source, name, origin, info, saved: annos.clone(), annos, undo: Vec::new(), redo: Vec::new(), last_merge: None, motion }
    }

    pub fn duration(&self) -> f32 {
        self.info.duration
    }

    pub fn frame_time(&self) -> f32 {
        self.info.frame_time()
    }

    /// Last frame index (frames are 0..=last).
    pub fn last_frame(&self) -> u32 {
        (self.duration() / self.frame_time()).round() as u32
    }

    pub fn dirty(&self) -> bool {
        self.annos != self.saved
    }

    pub fn mark_saved(&mut self) {
        self.saved = self.annos.clone();
    }

    /// Every change goes through here: one undo step, list kept time-sorted.
    /// Returns the new index of the annotation that was at `follow`.
    pub fn edit(&mut self, follow: Option<usize>, f: impl FnOnce(&mut Vec<Anno>)) -> Option<usize> {
        self.last_merge = None;
        self.edit_inner(follow, true, f)
    }

    /// Like `edit`, but consecutive calls with the same `key` share one undo
    /// step (dragging a number field, typing in one parameter).
    pub fn edit_merge(&mut self, follow: Option<usize>, key: &str, f: impl FnOnce(&mut Vec<Anno>)) -> Option<usize> {
        let fresh = self.last_merge.as_deref() != Some(key);
        let r = self.edit_inner(follow, fresh, f);
        self.last_merge = Some(key.to_string());
        r
    }

    fn edit_inner(&mut self, follow: Option<usize>, push_undo: bool, f: impl FnOnce(&mut Vec<Anno>)) -> Option<usize> {
        let before = self.annos.clone();
        f(&mut self.annos);
        if self.annos == before {
            return follow;
        }
        // Remember the edited item by identity so the selection follows the sort.
        let marker = follow.filter(|&i| i < self.annos.len()).map(|i| self.annos[i].clone());
        self.annos.sort_by(|a, b| a.time.total_cmp(&b.time));
        if push_undo {
            self.undo.push(before);
        }
        self.redo.clear();
        self.motion = MotionPath::from_annos(&self.annos);
        marker.and_then(|m| self.annos.iter().position(|a| *a == m))
    }

    pub fn undo(&mut self) -> bool {
        self.last_merge = None;
        let Some(prev) = self.undo.pop() else { return false };
        self.redo.push(std::mem::replace(&mut self.annos, prev));
        self.motion = MotionPath::from_annos(&self.annos);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else { return false };
        self.undo.push(std::mem::replace(&mut self.annos, next));
        self.motion = MotionPath::from_annos(&self.annos);
        true
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// hkanno64's dump format, so the file feeds `hkanno64 update` as is.
    pub fn to_hkanno_txt(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!("# numOriginalFrames: {}\n", self.last_frame()));
        s.push_str(&format!("# duration: {}\n", self.duration()));
        s.push_str(&format!("# numAnnotationTracks: {}\n", self.info.annotation_tracks));
        s.push_str(&format!("# numAnnotations: {}\n", self.annos.len()));
        for a in &self.annos {
            s.push_str(&format!("{:.6} {}\n", a.time, a.text));
        }
        s
    }
}

/// AMR root motion. `animmotion` is the cumulative step in the frame the
/// clip started in; `animrotation` is the yaw turned since the start
/// (positive = clockwise from above, as SDSCombat confirmed in game with
/// fAnimRotationSign = 1). Between keys both are linear.
#[derive(Debug, Clone, Default)]
pub struct MotionPath {
    pub moves: Vec<(f32, [f32; 3])>,
    pub turns: Vec<(f32, f32)>,
}

impl MotionPath {
    pub fn from_annos(annos: &[Anno]) -> MotionPath {
        let mut m = MotionPath::default();
        for a in annos {
            match parse(&a.text).kind {
                Kind::AnimMotion(v) => m.moves.push((a.time, v)),
                Kind::AnimRotation(d) => m.turns.push((a.time, d)),
                _ => {}
            }
        }
        m
    }

    pub fn is_empty(&self) -> bool {
        self.moves.is_empty() && self.turns.is_empty()
    }

    /// Position (clip-start frame, +Y ahead) at `t`.
    pub fn position(&self, t: f32) -> [f32; 3] {
        sample(&self.moves, t, [0.0; 3], |a, b, f| [lerp(a[0], b[0], f), lerp(a[1], b[1], f), lerp(a[2], b[2], f)])
    }

    /// Yaw turned since the clip start, degrees.
    pub fn yaw(&self, t: f32) -> f32 {
        sample(&self.turns, t, 0.0, lerp)
    }

    /// Horizontal speed (units/s) at `t`, from neighbouring keys.
    pub fn speed(&self, t: f32) -> f32 {
        let dt = 1.0 / 60.0;
        let a = self.position((t - dt).max(0.0));
        let b = self.position(t + dt);
        ((b[0] - a[0]).hypot(b[1] - a[1])) / (2.0 * dt)
    }
}

fn lerp(a: f32, b: f32, f: f32) -> f32 {
    a + (b - a) * f
}

fn sample<T: Copy>(keys: &[(f32, T)], t: f32, zero: T, mix: impl Fn(T, T, f32) -> T) -> T {
    let Some(first) = keys.first() else { return zero };
    if t <= first.0 {
        // From rest at time 0 to the first key.
        return if first.0 > 0.0 { mix(zero, first.1, (t / first.0).max(0.0)) } else { first.1 };
    }
    for w in keys.windows(2) {
        if t <= w[1].0 {
            let span = (w[1].0 - w[0].0).max(1e-6);
            return mix(w[0].1, w[1].1, (t - w[0].0) / span);
        }
    }
    keys.last().unwrap().1
}

#[cfg(test)]
mod tests {
    use super::*;
    use sds_hkx::{AnimKind, Annotation};

    fn doc(annos: &[(f32, &str)]) -> ClipDoc {
        let info = ClipInfo {
            kind: AnimKind::Spline,
            duration: 1.0,
            frames: 31,
            transform_tracks: 99,
            float_tracks: 0,
            annotation_tracks: 99,
            track_names: Vec::new(),
            skeleton_name: String::new(),
            has_extracted_motion: false,
            annotations: annos.iter().map(|(t, s)| Annotation { time: *t, text: s.to_string(), track: 0 }).collect(),
        };
        ClipDoc::new(ClipSource::Disk("x.hkx".into()), "x.hkx".into(), String::new(), info)
    }

    #[test]
    fn edit_sorts_follows_and_undoes() {
        let mut d = doc(&[(0.1, "A"), (0.5, "B")]);
        assert_eq!(d.last_frame(), 30);
        let i = d.edit(Some(0), |a| a[0].time = 0.9);
        assert_eq!(i, Some(1));
        assert_eq!(d.annos[1].text, "A");
        assert!(d.dirty());
        assert!(d.undo());
        assert_eq!(d.annos[0].text, "A");
        assert!(!d.dirty());
        assert!(d.redo());
        assert_eq!(d.annos[0].text, "B");
    }

    #[test]
    fn motion_path() {
        let d = doc(&[(0.5, "animmotion 0 50 0"), (1.0, "animmotion 10 100 0"), (1.0, "animrotation 90")]);
        assert_eq!(d.motion.position(0.25), [0.0, 25.0, 0.0]);
        assert_eq!(d.motion.position(0.75), [5.0, 75.0, 0.0]);
        assert_eq!(d.motion.yaw(0.5), 45.0);
        assert!(d.to_hkanno_txt().contains("0.500000 animmotion 0 50 0"));
    }
}
