//! NiControllerSequence extraction and evaluation.
//!
//! A sequence is a named clip that retargets controllers by string IDs: each
//! controlled block names an object, a controller type and (per sequence) an
//! interpolator holding the actual keys — the controller's own interpolator
//! is just the blend hub the manager mixes through. Playing a clip therefore
//! needs none of the Havok machinery; the behavior graph only decides which
//! clip plays when (docs/knowledge/behavior-hkx.md — verified against
//! SkyrimSE 1.5.97: name lookup is case-insensitive, playback duration is
//! `(stop - start) / frequency`).

use aether_nif::typed::controllers::{
    BSEffectShaderPropertyFloatController, NiBoolData, NiBoolInterpolator,
    NiBoolTimelineInterpolator, NiControllerSequence, NiFloatData, NiFloatInterpolator,
    NiTextKeyExtraData, NiTransformData, NiTransformInterpolator,
};
use aether_nif::typed::{Ctx, TypedBlock};
use aether_nif::NifFile;

/// Which of the lightning controller's slots a sequence drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LightningTrack {
    Generation,
    Mutation,
    ArcOffset,
    /// A geometry parameter re-sampled at the next strike (the game
    /// samples Subdivision/Length/Width/... only at Generation's rising
    /// edge — RE 0x140D5FFF0/0x140D600C0). `usize` indexes the streamed
    /// float fields so the player writes the right one.
    Length,
    LengthVar,
    Width,
}

/// What a track drives, resolved to plain names.
#[derive(Debug, Clone, PartialEq)]
pub enum TrackTarget {
    /// `NiPSysEmitterCtlr` with controller id `BirthRate`.
    EmitterBirthRate { system: String },
    /// `NiPSysEmitterCtlr` with controller id `EmitterActive`.
    EmitterActive { system: String },
    /// `NiPSysEmitterSpeedCtlr`.
    EmitterSpeed { system: String },
    /// The rest of the `NiPSysModifierFloatCtlr` family: `param` indexes
    /// [`crate::graph::PARAM_TARGETS`]. The modifier NAME (`Modifier:N`)
    /// rides along for provenance; the player applies to every matching
    /// slot of the system, same coarseness as `EmitterSpeed`.
    EmitterParam {
        system: String,
        modifier: String,
        param: usize,
    },
    /// `BSEffectShaderPropertyColorController` var 0 — the effect
    /// material's base colour rgb (RE 0x141322860), which for a particle
    /// system is the tint the sim already multiplies in.
    EffectShaderColor { node: String },
    /// `BSLightingShaderPropertyFloatController` on the named object's
    /// lit shader (variable per LightingShaderControlledFloat).
    LightingShaderFloat { node: String, variable: u32 },
    /// `BSLightingShaderPropertyColorController`: 0 specular, 1 emissive.
    LightingShaderColor { node: String, variable: u32 },
    /// `BSNiAlphaPropertyTestRefController` — the alpha-test threshold,
    /// raw 0-255 track.
    AlphaTestRef { node: String },
    /// `BSProceduralLightningController` — `which` indexes the
    /// controller's own interpolator-ID table: the tracks vanilla
    /// sequences actually drive are Generation (bool: the bolt exists),
    /// Mutation (bool: every FLIP re-rolls the jitter) and ArcOffset
    /// (float amplitude). `node` names the ProceduralGeometry target.
    Lightning { node: String, which: LightningTrack },
    /// `NiPSysModifierActiveCtlr` — id names the modifier.
    ModifierActive { system: String, modifier: String },
    /// A bool track on a scene object with no property (visibility).
    Visibility { node: String },
    /// `BSEffectShaderPropertyFloatController` on the named object's shader.
    EffectShaderFloat { node: String, variable: u32 },
    /// `NiTransformController` — animates the named node's local transform.
    NodeTransform { node: String },
}

impl TrackTarget {
    /// The graph parameter this track occupies, when the editor models
    /// one.
    ///
    /// The mirror of the decompiler's own routing, and the reason it
    /// can exist at all: a controller whose keys live in a clip imports
    /// with an EMPTY parameter, on purpose — a free-running copy would
    /// fight the sequence player. Measured over the corpus, that is
    /// **93% of `Birth Rate Keys` fields**, and until this map existed
    /// the node showed an empty control and said nothing about why.
    ///
    /// `None` for the tracks that drive scene objects and materials
    /// rather than a particle parameter (transforms, lighting shaders,
    /// lightning, the alpha test): those have no node parameter to
    /// annotate.
    pub fn graph_param(&self) -> Option<&'static str> {
        Some(match self {
            TrackTarget::EmitterBirthRate { .. } => "Birth Rate Keys",
            TrackTarget::EmitterActive { .. } => "Emitter Active Keys",
            // The Param Track node holds every one of the twelve
            // block spellings in one parameter.
            TrackTarget::EmitterSpeed { .. } | TrackTarget::EmitterParam { .. } => "Keys",
            TrackTarget::ModifierActive { .. } => "Keys",
            TrackTarget::Visibility { .. } => "Keys",
            TrackTarget::EffectShaderColor { .. } => "Color Keys",
            // Variables 0 and 5 are the only two the shader node
            // models -- and the only two any vanilla or shipped-mod
            // particle shader drives.
            // Through the one table beside the schema, so a modelled
            // variable cannot reach the node and miss the reader.
            TrackTarget::EffectShaderFloat { variable, .. } => crate::graph::EFFECT_SHADER_FLOAT_KEYS
                .iter()
                .find(|(_, v)| v == variable)
                .map(|(p, _)| *p)?,
            TrackTarget::LightingShaderFloat { .. }
            | TrackTarget::LightingShaderColor { .. }
            | TrackTarget::AlphaTestRef { .. }
            | TrackTarget::Lightning { .. }
            | TrackTarget::NodeTransform { .. } => return None,
        })
    }
}

/// One clip's claim on one graph parameter.
#[derive(Debug, Clone, PartialEq)]
pub struct DrivenBy {
    /// The clip that owns the keys.
    pub clip: String,
    /// Its position in the file's clip list, and the controlled-block
    /// slot of this track in it -- the provenance a document clip keeps.
    pub clip_index: usize,
    pub slot: usize,
    /// Those keys, as the editor's own track type, so the node can draw
    /// the shape it cannot edit.
    pub curve: crate::curve::Curve,
}

/// The clips whose tracks satisfy `want`, with the keys they carry.
///
/// The predicate matches on the track's NAMES, because that is how the
/// format binds: a `NiControllerSequence` owns its own copy of every
/// controller it drives, and those copies have a NULL target — measured
/// on ZD_MorthalSwordLeaf, whose eleven clips carry 704 controller
/// blocks, none of which is the live controller on the object. Binding
/// is by node name plus controller type and id, through the AV object
/// palette; block indices cannot bridge the two sides at all.
///
/// Which is why the caller is [`crate::decompile`]: it has already
/// resolved those names to build the graph, so matching there is a
/// lookup rather than a second implementation of the binding.
pub fn clips_driving(set: &SequenceSet, want: impl Fn(&TrackTarget) -> bool) -> Vec<DrivenBy> {
    let mut out = Vec::new();
    for (clip_index, clip) in set.clips.iter().enumerate() {
        for track in &clip.tracks {
            if !want(&track.target) {
                continue;
            }
            let Some(curve) = curve_of(&track.keys) else {
                continue;
            };
            out.push(DrivenBy {
                clip: clip.name.clone(),
                clip_index,
                slot: track.slot,
                curve,
            });
        }
    }
    out
}

/// A clip track's keys as the editor's own curve type, so a node can
/// draw the shape it cannot edit. `None` for the tracks that drive a
/// transform or a path rather than a value.
fn curve_of(keys: &TrackKeys) -> Option<crate::curve::Curve> {
    use crate::curve::{Curve, Key, Track as CurveTrack};
    Some(match keys {
        TrackKeys::Float(f) => Curve::from_float(f),
        TrackKeys::Bool(k) => Curve::from_bool_keys(
            5,
            &k.iter().map(|(t, v)| (*t, u8::from(*v))).collect::<Vec<_>>(),
        ),
        TrackKeys::Color(c) => c.clone(),
        TrackKeys::Transform(_) | TrackKeys::Path(_) => return None,
    })
}

/// The clips driving `(source block, parameter)`, or an empty slice.
pub fn driven_for<'a>(
    map: &'a [((u32, &'static str), Vec<DrivenBy>)],
    source_block: u32,
    param: &str,
) -> &'a [DrivenBy] {
    map.iter()
        .find(|((b, p), _)| *b == source_block && *p == param)
        .map(|(_, v)| v.as_slice())
        .unwrap_or(&[])
}

/// Keyed local-transform channels. Channels are independent — a track may
/// rotate without translating — and each falls back to the interpolator's
/// pose value when it has no keys.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TransformKeys {
    /// Keyed translation, sampled by its own interpolation type.
    pub translations: crate::curve::Curve,
    /// `(time, quaternion wxyz)`. Quaternion keys stay a plain list:
    /// the game slerps them, and the quadratic form the format allows
    /// is not authored anywhere in the corpus.
    pub rotations: Vec<(f32, [f32; 4])>,
    /// Per-axis euler curves (radians) when the data uses XYZ
    /// rotations. These carry their key TYPE, so a quadratic channel
    /// eases the way the game eases it instead of lerping.
    pub euler_rotations: [crate::curve::FloatCurve; 3],
    pub scales: crate::curve::FloatCurve,
    /// Pose fallback from the interpolator, per channel; `None` when the
    /// file marks the channel invalid (±FLT_MAX sentinels).
    pub pose_translation: Option<[f32; 3]>,
    pub pose_rotation: Option<[f32; 4]>,
    pub pose_scale: Option<f32>,
}

/// A sampled local transform; `None` channels leave the node's authored
/// value untouched.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TransformSample {
    pub translation: Option<[f32; 3]>,
    /// Quaternion, wxyz.
    pub rotation: Option<[f32; 4]>,
    pub scale: Option<f32>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TrackKeys {
    /// A keyed float track, sampled by its own interpolation type —
    /// linear, quadratic Hermite or TBC. It used to be `(time, value)`
    /// pairs sampled linearly, which silently flattened the 64,915
    /// quadratic tracks the corpus ships into lerps.
    Float(crate::curve::FloatCurve),
    /// `(time, on)`, stepped.
    Bool(Vec<(f32, bool)>),
    /// Independent translation/rotation/scale channels.
    Transform(Box<TransformKeys>),
    /// Colour controller tracks, as the WHOLE curve the `NiPosData`
    /// stores: key type, tangents and TBC triples. Kept as bare
    /// `(time, rgb)` pairs, a quadratic colour track read as linear, and
    /// the first edit to one of its values wrote the block back linear.
    Color(crate::curve::Curve),
    /// A spline path with its percent track (`NiPathInterpolator`) — the
    /// salmon leaping the falls are sequence-driven through these.
    Path(Box<crate::path::PathKeys>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub target: TrackTarget,
    pub keys: TrackKeys,
    /// The controlled-block index in its sequence: where an edit to this
    /// track is written back.
    pub slot: usize,
}

/// One playable clip.
#[derive(Debug, Clone, PartialEq)]
pub struct SequenceClip {
    pub name: String,
    pub start_time: f32,
    pub stop_time: f32,
    pub frequency: f32,
    /// CycleType 0; clamp (2) holds the final values.
    pub looping: bool,
    pub tracks: Vec<Track>,
    /// `(time, key)` — begin/loop/end markers and event names.
    pub text_keys: Vec<(f32, String)>,
    /// The `NiControllerSequence` block this clip was read from.
    pub source_block: Option<usize>,
    /// Where the clip's own manager lists it.
    ///
    /// Clips are read in BLOCK order, which is the exporter's; the
    /// manager's `Controller Sequences` array is the authored one, and
    /// it is what a file with no behavior graph has to say about which
    /// clip follows which. Measured over the corpus: of 2,127 managers
    /// with more than one sequence the two orders differ on 2, both in
    /// mods -- rare, and the reason to carry the authored one rather
    /// than assume they agree.
    pub list_index: usize,
}

impl SequenceClip {
    /// Seconds one pass takes, per the game's own formula.
    pub fn duration(&self) -> f32 {
        let span = (self.stop_time - self.start_time).max(0.0);
        if self.frequency.abs() > 1e-6 {
            span / self.frequency
        } else {
            span
        }
    }

    /// Map wall-clock playback time to sequence-local key time.
    pub fn local_time(&self, play_time: f32) -> f32 {
        let span = (self.stop_time - self.start_time).max(1e-6);
        let mut t = play_time * self.frequency;
        if self.looping {
            t %= span;
            if t < 0.0 {
                t += span;
            }
        } else {
            t = t.clamp(0.0, span);
        }
        self.start_time + t
    }
}

/// A controller whose window was never written.
///
/// nif.xml gives `NiTimeController.Start Time` the default `#FLT_MAX#`
/// and `Stop Time` the default `#FLT_MIN#` (nif.xml:3606-3607), so this
/// pair is not a corrupt file: it is the format saying the window is
/// absent. There is then nothing to be inside or outside of.
pub fn window_unset(start: f32, stop: f32) -> bool {
    start == f32::MAX || stop == f32::MIN
}

/// Map wall-clock `time` into a controller's key domain, the way the
/// game does.
///
/// `NiTimeController::ComputeScaledTime` (1.5.97 `0x140C6D5E0`). Two
/// parts of it are not what a reading of nif.xml alone would suggest:
///
/// * the cycle mapping is SKIPPED entirely unless BOTH ends are real
///   (`0x140C6D6F1`: `hiKeyTime != -FLT_MAX && loKeyTime != FLT_MAX`),
///   i.e. for an unset window; and
/// * the final clamp runs UNCONDITIONALLY after it, written
///   `if (t <= hi) { if (t < lo) t = lo; } else t = hi;`
///   (`0x140C6D7A0`). That order keeps `hi`, so an unset window pins
///   the controller at -FLT_MAX forever and it holds its FIRST key.
///
/// `f32::clamp` is NOT that function: it PANICS when `min > max`,
/// which is exactly what an unset window is. Four copies of
/// `time.clamp(start, stop)` in the simulator crashed the whole
/// application on any file carrying the default.
///
/// `looping` is the cycle type the graph models (Loop, else Clamp).
/// The runtime's third type, Reverse (a ping-pong over `2 * span`,
/// `0x140C6D706`), is not modelled anywhere in Aether and is not
/// modelled here either.
pub fn controller_time(time: f32, start: f32, stop: f32, looping: bool) -> f32 {
    let mut t = time;
    if looping && !window_unset(start, stop) {
        let span = stop - start;
        // The runtime's own guard against a zero-width window
        // (`0x140C6D769` -> LABEL_26): the answer is the low end, not a
        // division.
        t = if span == 0.0 {
            start
        } else {
            start + (t - start).rem_euclid(span)
        };
    }
    // The game's clamp, in the game's order. Written this way it is
    // total: a reversed window resolves to `stop` and a NaN to `stop`,
    // where `clamp` panics on the first and on the second.
    if t <= stop {
        if t < start {
            start
        } else {
            t
        }
    } else {
        stop
    }
}
/// The clock a FREE shader track is sampled at: the ambient clock
/// wrapped over the track's own key span.
///
/// The second of the two mappings a keyed parameter can run on, beside
/// [`controller_time`]. A `BSEffectShaderProperty` carries no
/// Start/Stop window — the node has no such parameters — so its free
/// controllers loop over the keys themselves. Named here so the
/// preview and the playhead that claims to show where the preview is
/// reading cannot disagree; it was a closure inside
/// `SystemDesc::apply_shader_tracks` until the playhead needed the
/// same answer.
///
/// A documented divergence, unchanged by the move: the game reads the
/// controller's own clamp/loop flag, which the graph does not model.
pub fn shader_track_time(time: f32, span: f32) -> f32 {
    if span <= 0.0 {
        0.0
    } else {
        time.rem_euclid(span)
    }
}

/// Linear sample of a float track at sequence-local time.
/// An rgb sample, clamped at the ends, through the curve's OWN key type
/// -- a quadratic colour eases between its keys, as the game's does. It
/// was a lerp over bare pairs, which played every colour track linear.
pub fn sample_color(keys: &crate::curve::Curve, t: f32) -> Option<[f32; 3]> {
    keys.sample(t)
}

pub fn sample_float(keys: &[(f32, f32)], t: f32) -> Option<f32> {
    let first = keys.first()?;
    if t <= first.0 {
        return Some(first.1);
    }
    let last = keys.last()?;
    if t >= last.0 {
        return Some(last.1);
    }
    let after = keys.iter().position(|k| k.0 > t)?;
    let (t0, v0) = keys[after - 1];
    let (t1, v1) = keys[after];
    let f = ((t - t0) / (t1 - t0).max(1e-9)).clamp(0.0, 1.0);
    Some(v0 + (v1 - v0) * f)
}

/// Stepped sample of a bool track: the last key at or before `t` wins.
pub fn sample_bool(keys: &[(f32, bool)], t: f32) -> Option<bool> {
    let first = keys.first()?;
    if t < first.0 {
        return Some(first.1);
    }
    keys.iter().take_while(|k| k.0 <= t).last().map(|k| k.1)
}

/// Linear sample of a vector track, clamped at the ends.
fn sample_vec3(keys: &[(f32, [f32; 3])], t: f32) -> Option<[f32; 3]> {
    let first = keys.first()?;
    if t <= first.0 {
        return Some(first.1);
    }
    let last = keys.last()?;
    if t >= last.0 {
        return Some(last.1);
    }
    let after = keys.iter().position(|k| k.0 > t)?;
    let (t0, v0) = keys[after - 1];
    let (t1, v1) = keys[after];
    let f = ((t - t0) / (t1 - t0).max(1e-9)).clamp(0.0, 1.0);
    Some([
        v0[0] + (v1[0] - v0[0]) * f,
        v0[1] + (v1[1] - v0[1]) * f,
        v0[2] + (v1[2] - v0[2]) * f,
    ])
}

/// Spherical interpolation, shortest path, wxyz. Falls back to normalized
/// lerp when the quaternions are nearly parallel (slerp divides by ~0 there).
pub fn slerp(a: [f32; 4], b: [f32; 4], f: f32) -> [f32; 4] {
    let mut b = b;
    let mut dot = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    if dot < 0.0 {
        for c in &mut b {
            *c = -*c;
        }
        dot = -dot;
    }
    let (wa, wb) = if dot > 0.9995 {
        (1.0 - f, f)
    } else {
        let theta = dot.clamp(-1.0, 1.0).acos();
        let sin = theta.sin();
        (((1.0 - f) * theta).sin() / sin, (f * theta).sin() / sin)
    };
    let out = [
        wa * a[0] + wb * b[0],
        wa * a[1] + wb * b[1],
        wa * a[2] + wb * b[2],
        wa * a[3] + wb * b[3],
    ];
    let len = (out[0] * out[0] + out[1] * out[1] + out[2] * out[2] + out[3] * out[3]).sqrt();
    if len > 1e-9 {
        [out[0] / len, out[1] / len, out[2] / len, out[3] / len]
    } else {
        a
    }
}

fn sample_quat(keys: &[(f32, [f32; 4])], t: f32) -> Option<[f32; 4]> {
    let first = keys.first()?;
    if t <= first.0 {
        return Some(first.1);
    }
    let last = keys.last()?;
    if t >= last.0 {
        return Some(last.1);
    }
    let after = keys.iter().position(|k| k.0 > t)?;
    let (t0, q0) = keys[after - 1];
    let (t1, q1) = keys[after];
    let f = ((t - t0) / (t1 - t0).max(1e-9)).clamp(0.0, 1.0);
    Some(slerp(q0, q1, f))
}

/// Quaternion for a rotation of `angle` radians about one axis (0=X,1=Y,2=Z).
fn axis_quat(axis: usize, angle: f32) -> [f32; 4] {
    let (s, c) = (angle * 0.5).sin_cos();
    let mut q = [c, 0.0, 0.0, 0.0];
    q[1 + axis] = s;
    q
}

/// Hamilton product, wxyz.
fn quat_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [
        a[0] * b[0] - a[1] * b[1] - a[2] * b[2] - a[3] * b[3],
        a[0] * b[1] + a[1] * b[0] + a[2] * b[3] - a[3] * b[2],
        a[0] * b[2] - a[1] * b[3] + a[2] * b[0] + a[3] * b[1],
        a[0] * b[3] + a[1] * b[2] - a[2] * b[1] + a[3] * b[0],
    ]
}

/// Sample every channel of a transform track at sequence-local time.
pub fn sample_transform(keys: &TransformKeys, t: f32) -> TransformSample {
    let translation = keys.translations.sample(t).or(keys.pose_translation);
    let scale = keys.scales.sample(t).or(keys.pose_scale);
    let rotation = if !keys.rotations.is_empty() {
        sample_quat(&keys.rotations, t)
    } else if keys.euler_rotations.iter().any(|c| !c.is_empty()) {
        // Gamebryo XYZ rotations compose per axis: R = Rx * Ry * Rz.
        let mut q = [1.0, 0.0, 0.0, 0.0];
        for axis in 0..3 {
            let angle = keys.euler_rotations[axis].sample(t).unwrap_or(0.0);
            q = quat_mul(q, axis_quat(axis, angle));
        }
        Some(q)
    } else {
        None
    }
    .or(keys.pose_rotation);
    TransformSample {
        translation,
        rotation,
        scale,
    }
}

/// The clip a cycle plays after `current`, by the authored list order.
///
/// This is NifSkope's `AnimSwitch` rule and it is a VIEWING convenience,
/// not the game's behaviour: nothing in the engine walks a manager's
/// list. It exists for a file whose clips no behavior graph plays --
/// a script does, and a script's order is not in the file -- where the
/// alternative is that only one clip can ever be seen at a time.
///
/// Returns `None` when there is nothing to move on to (fewer than two
/// clips), so a caller cannot turn a one-clip file into a restart loop
/// it did not ask for.
pub fn next_in_cycle(clips: &[SequenceClip], current: usize) -> Option<usize> {
    if clips.len() < 2 {
        return None;
    }
    let here = clips.get(current)?;
    // The successor is the clip with the next larger `list_index`,
    // wrapping to the smallest -- computed rather than held as a sorted
    // vector, so it cannot go stale against the clips it indexes.
    let by_list = |a: &&SequenceClip, b: &&SequenceClip| a.list_index.cmp(&b.list_index);
    let next = clips
        .iter()
        .filter(|c| c.list_index > here.list_index)
        .min_by(by_list)
        .or_else(|| clips.iter().min_by(by_list))?;
    clips.iter().position(|c| std::ptr::eq(c, next))
}

#[derive(Debug, Default)]
pub struct SequenceSet {
    pub clips: Vec<SequenceClip>,
    pub warnings: Vec<String>,
}

/// Pull every playable sequence out of a file.
pub fn sequences_from_nif(file: &NifFile) -> SequenceSet {
    let mut set = SequenceSet::default();
    let Ok(ctx) = Ctx::of(&file.header) else {
        return set;
    };
    let resolve = |idx: aether_nif::typed::StringIdx| -> String {
        idx.resolve(&file.header).unwrap_or_default().to_string()
    };

    // The game finds a clip through its manager's list
    // (`GetSequenceByName`), never by scanning blocks, so a sequence no
    // manager lists is not a clip -- which is exactly what removing one
    // leaves behind, since the splice never moves a block. Measured over
    // the corpus before this rule went in: 10,051 sequences in 2,675
    // files, every one listed, and no file with clips lacks a manager
    // (`sequence_listing_census.rs`), so no real file loses a clip here.
    //
    // Kept as a LIST, not a set: its order is the authored one (see
    // `SequenceClip::list_index`), and the position of each block in it
    // is what a cycle through the clips follows.
    let mut listed: Vec<usize> = Vec::new();
    for block in &file.blocks {
        if block.type_name != "NiControllerManager" {
            continue;
        }
        let Ok(m) = aether_nif::typed::controllers::NiControllerManager::decode(block, ctx) else {
            continue;
        };
        for slot in m.sequences.into_iter().filter_map(|r| r.get()) {
            if !listed.contains(&slot) {
                listed.push(slot);
            }
        }
    }
    for (block_index, block) in file.blocks.iter().enumerate() {
        if block.type_name != "NiControllerSequence" {
            continue;
        }
        let Some(list_index) = listed.iter().position(|b| *b == block_index) else {
            continue;
        };
        let Ok(seq) = NiControllerSequence::decode(block, ctx) else {
            set.warnings.push("a sequence failed to decode".into());
            continue;
        };
        let name = resolve(seq.name);
        let mut clip = SequenceClip {
            name: name.clone(),
            start_time: seq.start_time,
            stop_time: seq.stop_time,
            frequency: seq.frequency,
            looping: seq.cycle_type == 0,
            tracks: Vec::new(),
            text_keys: Vec::new(),
            source_block: Some(block_index),
            list_index,
        };

        if let Some(tk) = seq.text_keys.get() {
            if let Some(tk_block) = file.blocks.get(tk) {
                if let Ok(text) = NiTextKeyExtraData::decode(tk_block, ctx) {
                    clip.text_keys = text
                        .keys
                        .iter()
                        .map(|(t, s)| (*t, resolve(*s)))
                        .collect();
                }
            }
        }

        for (slot, cb) in seq.controlled_blocks.iter().enumerate() {
            let node = resolve(cb.node_name);
            let ctype = resolve(cb.controller_type);
            let cid = resolve(cb.controller_id);
            let iid = resolve(cb.interpolator_id);

            // The keys decide the emitter-controller split when the IDs are
            // ambiguous: files put "BirthRate"/"EmitterActive" in either the
            // controller id or the interpolator id, but a birth rate is
            // always a float track and the active toggle always a bool one.
            let keys_peek = keys_of(file, ctx, cb.interpolator);
            // Some exporters shift the controlled-block strings one slot
            // (the kitty pack: the controller CLASS lands in property_type
            // and the modifier name in controller_type). The game survives
            // it because string resolution falls back to the item's
            // SERIALIZED controller ref when the strings match no
            // controller (RE 0x140CC76E0: fallback read 0x140CC987E,
            // retarget 0x140CC79EC) — so dispatch string-first,
            // ref-fallback, the game's exact order.
            let ctype = if known_controller_type(&ctype) {
                ctype
            } else {
                cb.controller
                    .get()
                    .and_then(|i| file.blocks.get(i))
                    .map(|b| b.type_name.clone())
                    .filter(|t| known_controller_type(t))
                    .unwrap_or(ctype)
            };
            let target = match ctype.as_str() {
                "NiPSysEmitterCtlr"
                    if cid == "BirthRate"
                        || iid == "BirthRate"
                        || matches!(keys_peek, Some(TrackKeys::Float(_))) =>
                {
                    TrackTarget::EmitterBirthRate { system: node }
                }
                "NiPSysEmitterCtlr" => TrackTarget::EmitterActive { system: node },
                "NiPSysEmitterSpeedCtlr" => TrackTarget::EmitterSpeed { system: node },
                other if crate::graph::PARAM_CTLR_BLOCKS.contains(&other) => {
                    let param = crate::graph::PARAM_CTLR_BLOCKS
                        .iter()
                        .position(|n| *n == other)
                        .unwrap_or(0);
                    TrackTarget::EmitterParam {
                        system: node,
                        modifier: cid,
                        param,
                    }
                }
                "BSEffectShaderPropertyColorController" => {
                    TrackTarget::EffectShaderColor { node }
                }
                // The variable comes from the CONTROLLER ID first: that
                // string is what the game matches (1.5.97 `0x140CC76E0`),
                // and a clip added in the editor ships no controller copy
                // to read it from. Vanilla's copies always agree with it.
                "BSLightingShaderPropertyFloatController" => {
                    let variable = cid.parse::<u32>().ok().unwrap_or_else(|| {
                        cb.controller
                            .get()
                            .and_then(|i| file.blocks.get(i))
                            .and_then(|b| {
                                aether_nif::typed::controllers::BSLightingShaderPropertyFloatController::decode(b, ctx).ok()
                            })
                            .map(|c| c.controlled_variable)
                            .unwrap_or(0)
                    });
                    TrackTarget::LightingShaderFloat { node, variable }
                }
                "BSLightingShaderPropertyColorController" => {
                    let variable = cid.parse::<u32>().ok().unwrap_or_else(|| {
                        cb.controller
                            .get()
                            .and_then(|i| file.blocks.get(i))
                            .and_then(|b| {
                                aether_nif::typed::controllers::BSLightingShaderPropertyColorController::decode(b, ctx).ok()
                            })
                            .map(|c| c.controlled_color)
                            .unwrap_or(0)
                    });
                    TrackTarget::LightingShaderColor { node, variable }
                }
                "BSNiAlphaPropertyTestRefController" => TrackTarget::AlphaTestRef { node },
                "BSProceduralLightningController" => {
                    // The interpolator id addresses one of the nine
                    // slots by NAME (GetInterpolatorIndex is a strcmp
                    // table — RE 0x140D5FC70). Only three are driven by
                    // vanilla sequences; the rest would be new ground.
                    let which = match iid.as_str() {
                        "Generation" => Some(LightningTrack::Generation),
                        "Mutation" => Some(LightningTrack::Mutation),
                        "ArcOffset" => Some(LightningTrack::ArcOffset),
                        "Length" => Some(LightningTrack::Length),
                        "LengthVar" => Some(LightningTrack::LengthVar),
                        "Width" => Some(LightningTrack::Width),
                        // Subdivision / NumBranches / NumBranchesVar are
                        // NULL in every vanilla file (RE census) — no
                        // sequence drives them, so a warning there is
                        // real news, not noise.
                        other => {
                            set.warnings.push(format!(
                                "{name}: lightning slot {other:?} on {node:?} \
                                 is not playable yet"
                            ));
                            None
                        }
                    };
                    match which {
                        Some(which) => TrackTarget::Lightning { node, which },
                        None => continue,
                    }
                }
                "NiPSysModifierActiveCtlr" => TrackTarget::ModifierActive {
                    system: node,
                    modifier: cid,
                },
                "BSEffectShaderPropertyFloatController" => {
                    // The variable is the CONTROLLER ID string -- measured,
                    // "5" for alpha, "0" for emissive -- which is what the
                    // game binds by. The controller copy's own field is the
                    // fallback for a file that leaves the id empty; an added
                    // clip ships no copy, so reading only the copy took its
                    // alpha track for emissive.
                    let variable = cid.parse::<u32>().ok().unwrap_or_else(|| {
                        cb.controller
                            .get()
                            .and_then(|i| file.blocks.get(i))
                            .and_then(|b| {
                                BSEffectShaderPropertyFloatController::decode(b, ctx).ok()
                            })
                            .map(|c| c.controlled_variable)
                            .unwrap_or(0)
                    });
                    TrackTarget::EffectShaderFloat { node, variable }
                }
                "NiVisController" => TrackTarget::Visibility { node },
                "NiFloatExtraDataController" => {
                    // The NIF-side controller is a runtime NO-OP (Update
                    // 0x140CD7A40 writes the float; nothing reads it
                    // back). The VISIBLE effect is the same-named Havok
                    // channel: BShkVisibilityController binds the node
                    // named by the "hkVis:" SUFFIX — NOT the controller's
                    // target (the nordic door's track targets Amulet03
                    // but names Amulet01; in game only Amulet01 animates)
                    // — and hides on an EXACT == 0.0 (RE 0x140AFFDD0).
                    // We play the track as that channel's only available
                    // value source; the keys stay float, the viewport
                    // hides on == 0.
                    use aether_nif::typed::controllers::NiFloatExtraDataController;
                    let ed_name = cb
                        .controller
                        .get()
                        .and_then(|i| file.blocks.get(i))
                        .and_then(|b| NiFloatExtraDataController::decode(b, ctx).ok())
                        .map(|c| resolve(c.extra_data_name))
                        .unwrap_or_default();
                    match ed_name.strip_prefix("hkVis:") {
                        Some(suffix) => TrackTarget::Visibility {
                            node: suffix.to_string(),
                        },
                        None => {
                            // hkPhoneme/hkFade and friends: no invented
                            // semantics.
                            set.warnings.push(format!(
                                "{name}: float extra data {ed_name:?} on \
                                 {node:?} has no known consumer"
                            ));
                            continue;
                        }
                    }
                }
                "NiTransformController" => TrackTarget::NodeTransform { node },
                other => {
                    set.warnings.push(format!(
                        "{name}: {other} track on {node:?} is not playable yet"
                    ));
                    continue;
                }
            };

            let Some(keys) = keys_peek else {
                set.warnings.push(format!(
                    "{name}: no keys for {:?} (interpolator missing or unsupported)",
                    target
                ));
                continue;
            };
            clip.tracks.push(Track { target, keys, slot });
        }

        set.clips.push(clip);
    }
    // Stable order: as authored (file order), which matches how mods list
    // their sequences (mBegin before mLoop before mEnd).
    set
}

/// The controller families the dispatch below understands — used by the
/// serialized-ref fallback to decide whether a referenced block's type
/// would actually dispatch. Adding a match arm without extending this
/// list only costs the fallback a chance, never correctness.
fn known_controller_type(t: &str) -> bool {
    matches!(
        t,
        "NiPSysEmitterCtlr"
            | "NiPSysEmitterSpeedCtlr"
            | "BSEffectShaderPropertyColorController"
            | "BSEffectShaderPropertyFloatController"
            | "BSLightingShaderPropertyFloatController"
            | "BSLightingShaderPropertyColorController"
            | "BSNiAlphaPropertyTestRefController"
            | "BSProceduralLightningController"
            | "NiPSysModifierActiveCtlr"
            | "NiVisController"
            | "NiTransformController"
            | "NiFloatExtraDataController"
    ) || crate::graph::PARAM_CTLR_BLOCKS.contains(&t)
}

/// Extract keys from a sequence's per-clip interpolator.
fn keys_of(file: &NifFile, ctx: Ctx, interp: aether_nif::typed::BlockRef) -> Option<TrackKeys> {
    let idx = interp.get()?;
    let block = file.blocks.get(idx)?;
    match block.type_name.as_str() {
        "NiFloatInterpolator" => {
            let fi = NiFloatInterpolator::decode(block, ctx).ok()?;
            match fi.data.get() {
                Some(data_idx) => {
                    let fd = NiFloatData::decode(file.blocks.get(data_idx)?, ctx).ok()?;
                    let curve = crate::curve::FloatCurve::from_data(&fd);
                    (!curve.is_empty()).then_some(TrackKeys::Float(curve))
                }
                None => Some(TrackKeys::Float(crate::curve::FloatCurve {
                    interpolation: 1,
                    keys: vec![crate::curve::FloatKey {
                        time: 0.0,
                        value: fi.value,
                        ..Default::default()
                    }],
                })),
            }
        }
        "NiPoint3Interpolator" => {
            use aether_nif::typed::controllers::{NiPoint3Interpolator, NiPosData};
            let pi = NiPoint3Interpolator::decode(block, ctx).ok()?;
            match pi.data.get() {
                Some(data_idx) => {
                    let pd = NiPosData::decode(file.blocks.get(data_idx)?, ctx).ok()?;
                    let curve = crate::curve::Curve::from_pos_data(pd.interpolation, &pd.floats);
                    (!curve.is_empty()).then_some(TrackKeys::Color(curve))
                }
                // The pose sentinel is -FLT_MAX per component; a real
                // pose colour is a single-key track.
                None if pi.value[0] > -3.0e38 => {
                    let mut curve = crate::curve::Curve::empty(crate::curve::Track::Vec3);
                    curve.interpolation = 1;
                    curve.keys = vec![crate::curve::Key { time: 0.0, value: pi.value, ..Default::default() }];
                    Some(TrackKeys::Color(curve))
                }
                None => None,
            }
        }
        "NiBoolInterpolator" | "NiBoolTimelineInterpolator" => {
            // Same layout; decode through the timeline type for both.
            let bi = if block.type_name == "NiBoolInterpolator" {
                let b = NiBoolInterpolator::decode(block, ctx).ok()?;
                (b.value, b.data)
            } else {
                let b = NiBoolTimelineInterpolator::decode(block, ctx).ok()?;
                (b.value, b.data)
            };
            match bi.1.get() {
                Some(data_idx) => {
                    let bd = NiBoolData::decode(file.blocks.get(data_idx)?, ctx).ok()?;
                    let keys: Vec<(f32, bool)> =
                        bd.keys.iter().map(|(t, v)| (*t, *v != 0)).collect();
                    (!keys.is_empty()).then_some(TrackKeys::Bool(keys))
                }
                None => Some(TrackKeys::Bool(vec![(0.0, bi.0 != 0)])),
            }
        }
        "NiTransformInterpolator" => {
            transform_keys_from(file, ctx, block).map(|k| TrackKeys::Transform(Box::new(k)))
        }
        "NiPathInterpolator" => {
            crate::path::build(file, ctx, block).map(|k| TrackKeys::Path(Box::new(k)))
        }
        _ => None,
    }
}

/// Keyed channels of one `NiTransformInterpolator` block — shared between
/// the sequence extractor and the free-running node-controller walk in
/// `mesh.rs` (the hawk flaps its wings through free controllers).
pub fn transform_keys_from(
    file: &NifFile,
    ctx: Ctx,
    block: &aether_nif::Block,
) -> Option<TransformKeys> {
    let ti = NiTransformInterpolator::decode(block, ctx).ok()?;
    let mut keys = TransformKeys {
        // Skyrim marks invalid pose channels with ±FLT_MAX floats.
        pose_translation: finite3(ti.translation),
        pose_rotation: finite4(ti.rotation),
        pose_scale: finite(ti.scale),
        ..TransformKeys::default()
    };
    if let Some(data_idx) = ti.data.get() {
        let td = NiTransformData::decode(file.blocks.get(data_idx)?, ctx).ok()?;
        keys.translations = crate::curve::Curve::from_group(&td.translations);
        keys.scales = crate::curve::FloatCurve::from_key_group(&td.scales);
        if td.rotation_type == 4 {
            for (axis, group) in td.xyz_rotations.iter().enumerate() {
                keys.euler_rotations[axis] =
                    crate::curve::FloatCurve::from_key_group(group);
            }
        } else {
            keys.rotations = td.quat_keys().collect();
        }
    }
    let has_anything = !keys.translations.is_empty()
        || !keys.rotations.is_empty()
        || keys.euler_rotations.iter().any(|c| !c.is_empty())
        || !keys.scales.is_empty()
        || keys.pose_translation.is_some()
        || keys.pose_rotation.is_some()
        || keys.pose_scale.is_some();
    has_anything.then_some(keys)
}

fn finite(v: f32) -> Option<f32> {
    (v.is_finite() && v.abs() < 1e30).then_some(v)
}

fn finite3(v: [f32; 3]) -> Option<[f32; 3]> {
    v.iter().all(|c| c.is_finite() && c.abs() < 1e30).then_some(v)
}

fn finite4(v: [f32; 4]) -> Option<[f32; 4]> {
    v.iter().all(|c| c.is_finite() && c.abs() < 1e30).then_some(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn float_sampling_interpolates_and_clamps() {
        let keys = vec![(1.0, 10.0), (3.0, 30.0)];
        assert_eq!(sample_float(&keys, 0.0), Some(10.0), "clamps before");
        assert_eq!(sample_float(&keys, 2.0), Some(20.0), "midpoint");
        assert_eq!(sample_float(&keys, 9.0), Some(30.0), "clamps after");
    }

    #[test]
    fn bool_sampling_is_stepped() {
        let keys = vec![(0.0, false), (1.0, true), (2.0, false)];
        assert_eq!(sample_bool(&keys, 0.5), Some(false));
        assert_eq!(sample_bool(&keys, 1.0), Some(true));
        assert_eq!(sample_bool(&keys, 1.99), Some(true));
        assert_eq!(sample_bool(&keys, 2.0), Some(false));
    }

    #[test]
    fn slerp_walks_the_shortest_arc() {
        let id = [1.0, 0.0, 0.0, 0.0];
        let half = std::f32::consts::FRAC_PI_4; // 90° about Z, half-angle 45°
        let z90 = [half.cos(), 0.0, 0.0, half.sin()];
        let mid = slerp(id, z90, 0.5);
        let expect = std::f32::consts::FRAC_PI_8; // 45° rotation, half-angle 22.5°
        assert!((mid[0] - expect.cos()).abs() < 1e-5, "{mid:?}");
        assert!((mid[3] - expect.sin()).abs() < 1e-5, "{mid:?}");

        // The negated quaternion is the same rotation; interpolation must
        // not take the long way around.
        let neg = [-z90[0], -z90[1], -z90[2], -z90[3]];
        let mid2 = slerp(id, neg, 0.5);
        let dot = mid[0] * mid2[0] + mid[1] * mid2[1] + mid[2] * mid2[2] + mid[3] * mid2[3];
        assert!(dot.abs() > 0.9999, "long-way slerp: {mid2:?}");
    }

    #[test]
    fn transform_channels_sample_and_fall_back_independently() {
        let half = std::f32::consts::FRAC_PI_4;
        let keys = TransformKeys {
            rotations: vec![
                (0.0, [1.0, 0.0, 0.0, 0.0]),
                (1.0, [half.cos(), 0.0, 0.0, half.sin()]),
            ],
            pose_translation: Some([5.0, 0.0, 0.0]),
            ..TransformKeys::default()
        };
        let s = sample_transform(&keys, 1.0);
        let rot = s.rotation.unwrap();
        assert!((rot[3] - half.sin()).abs() < 1e-5, "keyed rotation sampled");
        assert_eq!(
            s.translation,
            Some([5.0, 0.0, 0.0]),
            "translation falls back to the interpolator pose"
        );
        assert_eq!(s.scale, None, "absent channel stays untouched");
    }

    #[test]
    fn euler_keys_compose_into_a_quaternion() {
        let angle = 1.0f32;
        let keys = TransformKeys {
            euler_rotations: [
                crate::curve::FloatCurve {
                    interpolation: 1,
                    keys: vec![crate::curve::FloatKey {
                        time: 0.0,
                        value: angle,
                        ..Default::default()
                    }],
                },
                crate::curve::FloatCurve::default(),
                crate::curve::FloatCurve::default(),
            ],
            ..TransformKeys::default()
        };
        let s = sample_transform(&keys, 0.0);
        let q = s.rotation.unwrap();
        assert!((q[0] - (angle / 2.0).cos()).abs() < 1e-5, "{q:?}");
        assert!((q[1] - (angle / 2.0).sin()).abs() < 1e-5, "{q:?}");
        assert!(q[2].abs() < 1e-6 && q[3].abs() < 1e-6, "{q:?}");
    }

    #[test]
    fn local_time_respects_frequency_and_cycle() {
        let clip = SequenceClip {
            name: "t".into(),
            start_time: 1.0,
            stop_time: 3.0,
            frequency: 2.0,
            looping: true,
            tracks: Vec::new(),
            text_keys: Vec::new(),
            source_block: None,
            list_index: 0,
        };
        // duration = (3-1)/2 = 1s of wall clock per pass.
        assert!((clip.duration() - 1.0).abs() < 1e-6);
        assert!((clip.local_time(0.0) - 1.0).abs() < 1e-6);
        assert!((clip.local_time(0.5) - 2.0).abs() < 1e-6);
        // Loops back rather than clamping.
        assert!((clip.local_time(1.25) - 1.5).abs() < 1e-6);

        let clamped = SequenceClip {
            looping: false,
            ..clip
        };
        assert!((clamped.local_time(5.0) - 3.0).abs() < 1e-6);
    }
}

/// Which transform channels are driven at runtime, per scene node.
///
/// **Per CHANNEL, not per node**, and that is a measured decision rather
/// than a nicety: 9,764 of 125,133 corpus nodes are driven, but 3,048 of
/// those keep at least one AUTHORED channel — 1,186 keep their authored
/// translation, which is exactly what a move gizmo writes. A per-node
/// "this is animated, do not touch" would cry wolf on a third of the
/// cases it fires on.
///
/// A channel counts as driven when it has keys OR a pose value. `None`
/// for a pose means the file wrote the +-FLT_MAX "no pose" sentinel,
/// which is precisely where the game falls back to the authored value —
/// so a sentinel channel is NOT driven.
///
/// Name resolution is case-insensitive and TOLERANT of misses: all
/// eleven of ZD_MorthalSwordLeaf's sequences drive a `PBomb05` that does
/// not exist in its scene, and `norsecrmsmdoorsm02.nif` genuinely has
/// two nodes called `Amulet01`.
pub fn driven_channels(
    scene: &crate::mesh::MeshScene,
    clips: &[SequenceClip],
) -> std::collections::BTreeMap<usize, [bool; 3]> {
    let mut out: std::collections::BTreeMap<usize, [bool; 3]> = Default::default();
    let mut mark = |node: usize, ch: [bool; 3]| {
        let e = out.entry(node).or_default();
        for i in 0..3 {
            e[i] |= ch[i];
        }
    };

    for clip in clips {
        for track in &clip.tracks {
            let TrackTarget::NodeTransform { node } = &track.target else {
                continue;
            };
            let Some(n) = scene.node_index(node) else { continue };
            match &track.keys {
                TrackKeys::Transform(k) => mark(n, transform_channels(k)),
                // A path drives the position always; it turns the object
                // only when Follow Axis is on, and it never scales.
                TrackKeys::Path(p) => mark(n, [true, p.follow, false]),
                _ => {}
            }
        }
    }
    // Free-running controllers drive the same channels, and the viewport
    // plays them without any sequence at all.
    for anim in &scene.node_anims {
        match &anim.keys {
            crate::mesh::NodeAnimKeys::Transform(k) => mark(anim.node, transform_channels(k)),
            crate::mesh::NodeAnimKeys::Path(p) => mark(anim.node, [true, p.follow, false]),
        }
    }
    out
}

fn transform_channels(k: &TransformKeys) -> [bool; 3] {
    [
        !k.translations.is_empty() || k.pose_translation.is_some(),
        !k.rotations.is_empty()
            || k.euler_rotations.iter().any(|c| !c.is_empty())
            || k.pose_rotation.is_some(),
        !k.scales.is_empty() || k.pose_scale.is_some(),
    ]
}
