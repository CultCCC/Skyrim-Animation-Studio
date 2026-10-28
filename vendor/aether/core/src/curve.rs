//! Float key tracks: the ONE place a keyed float curve is parsed,
//! formatted, converted to and from `NiFloatData`, and sampled.
//!
//! This existed as four half-copies — a text parser in `graph`, a
//! linear sampler in `sequence`, `(time, value)` extraction in
//! `decompile` and a hardcoded `interpolation: 1` writer in `compile` —
//! and the copies agreed only on the linear case. The corpus does not:
//! measured over every reachable NIF, `NiFloatData` ships 64,915
//! QUADRATIC blocks (cubic Hermite with per-key tangents) against 7,557
//! linear, 46 TBC and 2 CONST. Flattening quadratic keys to `(t, v)`
//! pairs lost the tangents on import, re-emitted the block as linear on
//! export, and made the field-count mismatch refuse the whole-block
//! merge — the 439 unmergeable blocks that lost authored data on any
//! edit.
//!
//! ## Text form
//!
//! Keys are edited as text, one entry per key, comma-separated:
//!
//! ```text
//! 0 = 5, 1 = 40                      linear
//! 0 = 5 ~ 0.5 -0.5, 1 = 40 ~ 0 0    quadratic: value ~ forward backward
//! 0 = 5 ~ 0 0 0                     TBC: value ~ tension bias continuity
//! ```
//!
//! The old `t = v` form parses exactly as it always did, because it is
//! a persisted graph format. CONST (type 5) has no text form on
//! purpose: the whole corpus holds two such float blocks and both are
//! the abandoned C++ editor's own output.
//!
//! ## The tangent convention, measured in the runtime
//!
//! A quadratic segment between keys `k0` and `k1` evaluates as cubic
//! Hermite with **k0's BACKWARD tangent on the h10 basis and k1's
//! FORWARD tangent on h11** -- the opposite of what the field names
//! suggest, and the opposite of what this file asserted until it was
//! measured.
//!
//! Read out of SkyrimSE 1.5.97 one dispatch at a time. A float channel
//! resolves its segment in `0x140CDCC00`, then calls
//! `qword_143013F40[key_type]`. The quadratic slot is filled with
//! `0x140CF84C0` by the type registrar at `0x140CF7C70`, which also
//! records the key size: 16 bytes, four floats. That thunk loads
//! `[k0+4]`, `[k0+0xC]`, `[k1+4]` and `[k1+8]` and hands them to the
//! Hermite polynomial at `0x140D027C0`:
//!
//! ```text
//! (((m0 + m1 - 2*(v1-v0))*s + (3*(v1-v0) - 2*m0 - m1))*s + m0)*s + v0
//! ```
//!
//! so `m0`, the h10 coefficient, is `[k0+0xC]` and `m1`, on h11, is
//! `[k1+8]`. The key loader at `0x140CF7DB0` writes the four fields in
//! file order with no permutation, and nif.xml's `Key` struct is
//! `Time, Value, Forward(+8), Backward(+0xC)`. The game therefore pairs
//! **left.Backward with right.Forward**. TBC keys (28 bytes, seven
//! floats) take the same shape at `0x140CFE780` -- `[k0+0x18]` with
//! `[k1+0x14]`, again the larger offset from the LEFT key.
//!
//! NifSkope pairs them this way too. An earlier revision of this file
//! claimed the reverse and called NifSkope wrong, on the strength of a
//! reading of the path interpolator's frame helper rather than of the
//! key dispatch itself; `path.rs` carried the same error, so every
//! vanilla spline path played with its tangents swapped. The test
//! below pins 55.0, where the swapped pairing gives 36.0 and a lerp
//! gives 30.0 -- three different numbers, so it cannot go vacuous.

use aether_nif::typed::controllers::{FloatKeyGroup, NiFloatData, Vec3KeyGroup};

/// One key. `a`/`b`/`c` are the trailing fields in FILE order, not
/// roles: quadratic stores `a` = Forward, `b` = Backward (which the
/// game reads as the segment's RIGHT and LEFT tangent respectively --
/// see the module docs); TBC stores tension, bias, continuity.
/// Linear and const keys carry zeros.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FloatKey {
    pub time: f32,
    pub value: f32,
    pub a: f32,
    pub b: f32,
    pub c: f32,
}

/// A keyed float track with its interpolation type.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FloatCurve {
    /// 1 linear, 2 quadratic, 3 TBC, 5 const. 0 = empty/unknown.
    pub interpolation: u32,
    pub keys: Vec<FloatKey>,
}

impl FloatCurve {
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// `(time, value)` pairs, for consumers that only need the shape.
    pub fn times_values(&self) -> impl Iterator<Item = (f32, f32)> + '_ {
        self.keys.iter().map(|k| (k.time, k.value))
    }

    /// Parse the editable text form. Returns an empty curve for text
    /// with no parseable keys; a malformed ENTRY is skipped rather than
    /// poisoning the rest, matching the old parser's tolerance.
    pub fn parse(text: &str) -> FloatCurve {
        let mut keys = Vec::new();
        let mut max_extra = 0usize;
        for entry in text.split(',') {
            let Some((time, rest)) = entry.split_once('=') else {
                continue;
            };
            let Ok(time) = time.trim().parse::<f32>() else {
                continue;
            };
            let (value_text, extras_text) = match rest.split_once('~') {
                Some((v, e)) => (v, Some(e)),
                None => (rest, None),
            };
            let Ok(value) = value_text.trim().parse::<f32>() else {
                continue;
            };
            let mut extras = [0.0f32; 3];
            let mut extra_count = 0usize;
            if let Some(e) = extras_text {
                for (i, part) in e.split_whitespace().take(3).enumerate() {
                    let Ok(v) = part.parse::<f32>() else { break };
                    extras[i] = v;
                    extra_count = i + 1;
                }
            }
            max_extra = max_extra.max(extra_count);
            keys.push(FloatKey {
                time,
                value,
                a: extras[0],
                b: extras[1],
                c: extras[2],
            });
        }
        let interpolation = if keys.is_empty() {
            0
        } else {
            match max_extra {
                0 => 1,
                3 => 3,
                _ => 2,
            }
        };
        FloatCurve {
            interpolation,
            keys,
        }
    }

    /// The inverse of [`Self::parse`] for the types the text can carry.
    ///
    /// Rust's float `Display` prints the shortest string that parses
    /// back to the same bits, so `parse(format(c))` reproduces every
    /// finite value exactly — which is what makes the text form safe as
    /// the storage of record.
    pub fn format(&self) -> String {
        self.keys
            .iter()
            .map(|k| match self.interpolation {
                2 => format!("{} = {} ~ {} {}", k.time, k.value, k.a, k.b),
                3 => format!("{} = {} ~ {} {} {}", k.time, k.value, k.a, k.b, k.c),
                _ => format!("{} = {}", k.time, k.value),
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Read a decoded `NiFloatData` whole, tangents included.
    pub fn from_data(data: &NiFloatData) -> FloatCurve {
        let stride = NiFloatData::key_stride(data.interpolation).unwrap_or(2);
        let keys: Vec<FloatKey> = data
            .floats
            .chunks_exact(stride)
            .map(|k| FloatKey {
                time: k[0],
                value: k[1],
                a: k.get(2).copied().unwrap_or(0.0),
                b: k.get(3).copied().unwrap_or(0.0),
                c: k.get(4).copied().unwrap_or(0.0),
            })
            .collect();
        FloatCurve {
            interpolation: if keys.is_empty() { 0 } else { data.interpolation },
            keys,
        }
    }

    /// Read a `NiTransformData` float channel (scale, or one euler
    /// axis) whole. Same wire shape as `NiFloatData`, different owner:
    /// the channels are `KeyGroup<float>` inline in the block.
    pub fn from_key_group(g: &FloatKeyGroup) -> FloatCurve {
        FloatCurve::from_data(&NiFloatData {
            interpolation: g.interpolation,
            floats: g.floats.clone(),
        })
    }

    /// The block this curve serializes as. Empty keys yield an empty
    /// block, matching what the sync writes for `n == 0`.
    pub fn to_data(&self) -> NiFloatData {
        let interpolation = match self.interpolation {
            0 => 1,
            i => i,
        };
        let stride = NiFloatData::key_stride(interpolation).unwrap_or(2);
        let mut floats = Vec::with_capacity(self.keys.len() * stride);
        for k in &self.keys {
            floats.push(k.time);
            floats.push(k.value);
            if stride >= 4 {
                floats.push(k.a);
                floats.push(k.b);
            }
            if stride >= 5 {
                floats.push(k.c);
            }
        }
        NiFloatData {
            interpolation,
            floats,
        }
    }

    /// Sample at `t`, clamped to the key range — the way the game's
    /// float controllers clamp at their track ends.
    pub fn sample(&self, t: f32) -> Option<f32> {
        let first = self.keys.first()?;
        if self.keys.len() == 1 || t <= first.time {
            return Some(first.value);
        }
        let last = self.keys.last()?;
        if t >= last.time {
            return Some(last.value);
        }
        let after = self.keys.iter().position(|k| k.time > t)?;
        let (k0, k1) = (&self.keys[after - 1], &self.keys[after]);
        let s = ((t - k0.time) / (k1.time - k0.time).max(1e-9)).clamp(0.0, 1.0);
        Some(match self.interpolation {
            // The LEFT key's Backward and the RIGHT key's Forward:
            // 1.5.97 `0x140CF84C0` loads [k0+0xC] and [k1+8].
            2 => hermite(k0.value, k1.value, k0.b, k1.a, s),
            3 => {
                let out0 = self.tbc_tangent(after - 1, true);
                let in1 = self.tbc_tangent(after, false);
                hermite(k0.value, k1.value, out0, in1, s)
            }
            // CONST holds the left key until the next one, the same
            // stepped rule the RE'd bool toggles follow. NifSkope's
            // player switches at the MIDPOINT instead; the runtime is
            // the reference, not NifSkope.
            5 => k0.value,
            _ => k0.value + (k1.value - k0.value) * s,
        })
    }

    /// Kochanek–Bartels tangent for key `i`: outgoing when `out`,
    /// incoming otherwise. With T = B = C = 0 both collapse to the
    /// Catmull–Rom tangent `(next - prev) / 2`.
    fn tbc_tangent(&self, i: usize, out: bool) -> f32 {
        let k = &self.keys[i];
        let prev = if i > 0 { &self.keys[i - 1] } else { k };
        let next = if i + 1 < self.keys.len() {
            &self.keys[i + 1]
        } else {
            k
        };
        let d_in = k.value - prev.value;
        let d_out = next.value - k.value;
        let (t, b, c) = (k.a, k.b, k.c);
        if out {
            0.5 * ((1.0 - t) * (1.0 + b) * (1.0 + c) * d_in
                + (1.0 - t) * (1.0 - b) * (1.0 - c) * d_out)
        } else {
            0.5 * ((1.0 - t) * (1.0 + b) * (1.0 - c) * d_in
                + (1.0 - t) * (1.0 - b) * (1.0 + c) * d_out)
        }
    }
}

/// What a [`Curve`] drives, which is what decides how many channels a
/// key carries, how the editor draws it and which block the compiler
/// writes for it.
///
/// `Bool` is a MODE and not a second type on purpose: the format
/// stores an on/off track as `NiBoolData`, and `NiBoolData` is
/// CONST-interpolated in **9,771 of 9,771** corpus blocks — a bool
/// track IS a stepped one-channel track. Modelling it separately is
/// what gave the editor a second key parser (`parse_bool_keys`) that
/// agreed with this one about nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Track {
    /// One channel, any interpolation: birth rate, alpha, emissive.
    #[default]
    Float,
    /// One channel, stepped, values 0 or 1: emitter active, modifier
    /// active, visibility.
    Bool,
    /// Three channels: the effect shader's emissive colour.
    Vec3,
}

impl Track {
    pub fn channels(self) -> usize {
        match self {
            Track::Vec3 => 3,
            _ => 1,
        }
    }

    /// Stable name for the persisted form.
    pub fn id(self) -> &'static str {
        match self {
            Track::Float => "float",
            Track::Bool => "bool",
            Track::Vec3 => "vec3",
        }
    }

    pub fn from_id(id: &str) -> Option<Track> {
        Some(match id {
            "float" => Track::Float,
            "bool" => Track::Bool,
            "vec3" => Track::Vec3,
            _ => return None,
        })
    }
}

/// One key of a [`Curve`]. `fwd`/`bwd` are the quadratic tangents in
/// FILE order (the game reads `bwd` as the segment's LEFT tangent and
/// `fwd` as its RIGHT one); `tbc` is the shared
/// tension/bias/continuity triple, which is per KEY and not per
/// component. A one-channel track uses index 0 and leaves the rest
/// zero.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Key {
    pub time: f32,
    pub value: [f32; 3],
    pub fwd: [f32; 3],
    pub bwd: [f32; 3],
    pub tbc: [f32; 3],
}

/// What a key carries beside `time` and `value`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyFields {
    /// Linear and const: nothing.
    None,
    /// Quadratic: `fwd` and `bwd`, one per channel each.
    Tangents,
    /// TBC: tension, bias and continuity, three per KEY whatever the
    /// channel count.
    Tbc,
}

/// A keyed track as the DOCUMENT holds it: what a graph parameter
/// stores, what the node draws and what the curve editor edits.
///
/// Distinct from [`FloatCurve`], which is one float CHANNEL of a NIF
/// block — a transform's scale, one euler axis, the path percent
/// track. Those are read by playback and never by the editor, and
/// they are genuinely single-channel, so folding them into this type
/// would make every sample carry two dead components.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Curve {
    pub track: Track,
    /// 1 linear, 2 quadratic, 3 TBC, 5 const. 0 = empty/unknown.
    pub interpolation: u32,
    pub keys: Vec<Key>,
}

impl Curve {
    /// An empty track of `track`'s kind — the schema default, and what
    /// the editor starts a new curve from.
    pub fn empty(track: Track) -> Curve {
        Curve {
            track,
            interpolation: match track {
                // The format's own answer for a stepped track, measured
                // over the whole corpus.
                Track::Bool => 5,
                // Vanilla's overwhelming majority: 64,915 quadratic
                // against 7,557 linear.
                _ => 2,
            },
            keys: Vec::new(),
        }
    }

    /// A track holding one key at t = 0 — the format's way of saying
    /// "this value, always", and what a schema default that means
    /// something (`0 = on`) has to be.
    pub fn constant(track: Track, value: f32) -> Curve {
        let mut c = Curve::empty(track);
        // One key needs no interpolation between anything; keep the
        // stepped type for a bool and the plain LINEAR one otherwise,
        // so a from-scratch constant does not claim tangents it has
        // no segment to use.
        if track != Track::Bool {
            c.interpolation = 1;
        }
        c.insert(Key {
            time: 0.0,
            value: [value; 3],
            ..Key::default()
        });
        c
    }

    /// Zero every field the track's kind and key type do not use.
    ///
    /// The invariant this keeps is that two tracks are equal iff they
    /// mean the same thing: only the used channels reach the file and
    /// the project format, so a stray value in channel 2 of a float
    /// track would make a curve unequal to its own round trip and
    /// `merge_edit` see an edit nobody made.
    pub fn canonical(&self, mut key: Key) -> Key {
        for c in self.channels()..3 {
            key.value[c] = 0.0;
        }
        match self.interpolation {
            2 => {
                key.tbc = [0.0; 3];
                for c in self.channels()..3 {
                    key.fwd[c] = 0.0;
                    key.bwd[c] = 0.0;
                }
            }
            3 => {
                key.fwd = [0.0; 3];
                key.bwd = [0.0; 3];
            }
            _ => {
                key.fwd = [0.0; 3];
                key.bwd = [0.0; 3];
                key.tbc = [0.0; 3];
            }
        }
        key
    }

    pub fn channels(&self) -> usize {
        self.track.channels()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// `(first, last)` key time, or `None` for an empty track.
    pub fn time_range(&self) -> Option<(f32, f32)> {
        Some((self.keys.first()?.time, self.keys.last()?.time))
    }

    /// Value range across every channel, widened to a non-degenerate
    /// span so a flat track still draws as a line rather than a
    /// division by zero.
    pub fn value_range(&self) -> (f32, f32) {
        let mut lo = f32::INFINITY;
        let mut hi = f32::NEG_INFINITY;
        for k in &self.keys {
            for c in 0..self.channels() {
                lo = lo.min(k.value[c]);
                hi = hi.max(k.value[c]);
            }
        }
        if !lo.is_finite() || !hi.is_finite() {
            return (0.0, 1.0);
        }
        if (hi - lo).abs() < 1e-6 {
            let pad = lo.abs().max(1.0) * 0.5;
            return (lo - pad, hi + pad);
        }
        (lo, hi)
    }

    /// Whether this track is really a SWITCH rather than a curve.
    ///
    /// A bool track with one key answers every `t` with that key, so
    /// its time is dead to the simulation and the track is a boolean
    /// wearing a curve's clothes. Measured: **67% of the corpus's
    /// `Emitter Active Keys` are exactly this** — a single `0 = on`.
    /// An empty bool track counts too: the export writes an empty
    /// `NiBoolData`, which the game and the preview both read as
    /// always-on.
    ///
    /// Keyed on the VALUE and not on the parameter's name, so it holds
    /// for every bool track and cannot drift from one that was added
    /// later.
    pub fn is_switch(&self) -> bool {
        self.track == Track::Bool && self.keys.len() <= 1
    }

    /// Flip a switch, keeping the key's TIME.
    ///
    /// The time is dead to the simulation and LIVE in the file, so
    /// rewriting it would be an edit nobody made — the trap the
    /// `DragValue` clamp already sprang on `Start Time`. An empty
    /// track means ON, so turning one off has to write the key that
    /// says so rather than leave the meaning to a special case.
    pub fn with_switch(&self, on: bool) -> Curve {
        let mut out = self.clone();
        let v = if on { 1.0 } else { 0.0 };
        match out.keys.first_mut() {
            Some(k) => k.value[0] = v,
            None => {
                out.insert(Key {
                    time: 0.0,
                    value: [v; 3],
                    ..Key::default()
                });
            }
        }
        out
    }

    /// What the node prints beside the curve, and what the generated
    /// reference prints for a default. ONE place, so the two cannot
    /// describe the same track differently.
    ///
    /// Keywords stay English in both UI languages, like every other
    /// label in this editor.
    pub fn summary(&self) -> String {
        let Some((t0, t1)) = self.time_range() else {
            return "no keys".to_string();
        };
        if self.keys.len() == 1 {
            let v = self.keys[0].value;
            return match self.track {
                Track::Bool => {
                    format!("always {}", if v[0] != 0.0 { "on" } else { "off" })
                }
                Track::Vec3 => {
                    format!("constant {} {} {}", short(v[0]), short(v[1]), short(v[2]))
                }
                Track::Float => format!("constant {}", short(v[0])),
            };
        }
        format!(
            "{} keys \u{b7} {}\u{2013}{} s \u{b7} {}",
            self.keys.len(),
            short(t0),
            short(t1),
            self.interpolation_name()
        )
    }

    /// Which extra numbers a key of this track carries beside its time
    /// and value — the fields the editor offers and the file stores.
    ///
    /// One rule, next to the conversions, because the two must agree:
    /// offering a tangent the block has no room for is a control that
    /// forgets what the user typed, which is the trap the strip
    /// parameters already taught this project.
    pub fn key_fields(&self) -> KeyFields {
        match self.interpolation {
            2 => KeyFields::Tangents,
            3 => KeyFields::Tbc,
            _ => KeyFields::None,
        }
    }

    /// One word for the interpolation, for the node caption.
    pub fn interpolation_name(&self) -> &'static str {
        match self.interpolation {
            1 => "linear",
            2 => "ease",
            3 => "TBC",
            5 => "step",
            _ => "empty",
        }
    }

    /// Insert a key at `time`, keeping the keys sorted — which is not
    /// a nicety: the runtime's segment search (1.5.97 `0x140CDCC00`)
    /// is a forward-only scan from a cached index, so an out-of-order
    /// key makes the game read the wrong segment.
    ///
    /// Returns the index it landed at.
    pub fn insert(&mut self, key: Key) -> usize {
        let key = self.canonical(key);
        let at = self
            .keys
            .iter()
            .position(|k| k.time > key.time)
            .unwrap_or(self.keys.len());
        self.keys.insert(at, key);
        at
    }

    /// Insert a key at `time` **without changing what the track
    /// plays**. Returns its index.
    ///
    /// The value alone is not enough, and assuming it was is an error
    /// this project made and a test caught: on a QUADRATIC track the
    /// shape between two keys is carried by their tangents, so a new
    /// key sitting on the curve with zero tangents flattens both
    /// halves of the segment it split — measured at 2.78 -> 5.21 on a
    /// two-key fixture. 89.5% of the corpus's `NiFloatData` blocks are
    /// quadratic, so that is the ordinary case, not a corner.
    ///
    /// The fix is the Hermite subdivision the shape actually requires.
    /// A segment is evaluated over a NORMALIZED parameter, so the
    /// split rescales the tangents by each half's share of it, and the
    /// derivative at the split becomes the new key's own pair. The
    /// NEIGHBOURS are rewritten too — their facing tangents now span a
    /// shorter segment and would otherwise overshoot.
    ///
    /// Linear, step and const need none of it: the value is the whole
    /// of what they carry between keys. TBC derives its tangents from
    /// tension/bias/continuity rather than storing them, so a split
    /// there cannot be exact and is not attempted — the new key takes
    /// the neutral 0/0/0, which is the shape TBC means by "no
    /// opinion".
    ///
    /// `fallback` answers for an EMPTY track, which samples to nothing.
    pub fn insert_on_curve(&mut self, time: f32, fallback: f32) -> usize {
        let value = self.sample(time).unwrap_or([fallback, 0.0, 0.0]);
        let mut key = Key {
            time,
            value,
            ..Key::default()
        };
        if self.track == Track::Bool {
            // The format has no value between on and off.
            key.value[0] = if key.value[0] != 0.0 { 1.0 } else { 0.0 };
            return self.insert(key);
        }
        let inside = self
            .keys
            .windows(2)
            .position(|w| w[0].time < time && time < w[1].time);
        let Some(i) = inside.filter(|_| self.interpolation == 2) else {
            return self.insert(key);
        };
        let (a, b) = (self.keys[i], self.keys[i + 1]);
        let s = (time - a.time) / (b.time - a.time);
        for c in 0..self.channels() {
            let (v0, v1) = (a.value[c], b.value[c]);
            // The game's own pairing: a segment reads its LEFT key's
            // Backward and its RIGHT key's Forward (1.5.97
            // `0x140CF84C0`). Reading them the other way round is the
            // error this project shipped for weeks.
            let (m0, m1) = (a.bwd[c], b.fwd[c]);
            let d = v0 * (6.0 * s * s - 6.0 * s)
                + m0 * (3.0 * s * s - 4.0 * s + 1.0)
                + v1 * (-6.0 * s * s + 6.0 * s)
                + m1 * (3.0 * s * s - 2.0 * s);
            key.fwd[c] = d * s;
            key.bwd[c] = d * (1.0 - s);
            self.keys[i].bwd[c] = m0 * s;
            self.keys[i + 1].fwd[c] = m1 * (1.0 - s);
        }
        self.insert(key)
    }

    /// Move key `i` to `time`, re-sorting if it crossed a neighbour.
    /// Returns its new index.
    pub fn retime(&mut self, i: usize, time: f32) -> usize {
        if i >= self.keys.len() {
            return i;
        }
        let mut key = self.keys.remove(i);
        key.time = time;
        self.insert(key)
    }

    /// Sample at `t`, clamped to the key range, per channel.
    ///
    /// The clamp at the LOW end is a deliberate divergence from the
    /// runtime, which computes `s` without one (`0x140cdcd4c`) and so
    /// extrapolates backwards along the first segment when the clock
    /// is earlier than the first key. Reachable only when the first
    /// key sits after the controller window opens; not measured, and
    /// an extrapolating preview is worse to author against than a held
    /// first value.
    pub fn sample(&self, t: f32) -> Option<[f32; 3]> {
        let first = self.keys.first()?;
        if self.keys.len() == 1 || t <= first.time {
            return Some(first.value);
        }
        let last = self.keys.last()?;
        if t >= last.time {
            return Some(last.value);
        }
        let after = self.keys.iter().position(|k| k.time > t)?;
        let (k0, k1) = (&self.keys[after - 1], &self.keys[after]);
        let s = ((t - k0.time) / (k1.time - k0.time).max(1e-9)).clamp(0.0, 1.0);
        let mut out = [0.0f32; 3];
        for a in 0..3 {
            out[a] = match self.interpolation {
                2 => hermite(k0.value[a], k1.value[a], k0.bwd[a], k1.fwd[a], s),
                3 => {
                    let m0 = self.tbc_tangent(after - 1, a, true);
                    let m1 = self.tbc_tangent(after, a, false);
                    hermite(k0.value[a], k1.value[a], m0, m1, s)
                }
                5 => k0.value[a],
                _ => k0.value[a] + (k1.value[a] - k0.value[a]) * s,
            };
        }
        Some(out)
    }

    /// Channel 0 of [`Self::sample`], for the one-channel tracks.
    pub fn sample_f32(&self, t: f32) -> Option<f32> {
        self.sample(t).map(|v| v[0])
    }

    /// Read a `NiFloatData` whole, tangents included.
    pub fn from_float_data(data: &NiFloatData) -> Curve {
        let f = FloatCurve::from_data(data);
        Curve::from_float(&f)
    }

    /// The `NiFloatData` this track serializes as.
    pub fn to_float_data(&self) -> NiFloatData {
        self.to_float().to_data()
    }

    pub fn from_float(f: &FloatCurve) -> Curve {
        Curve {
            track: Track::Float,
            interpolation: f.interpolation,
            keys: f
                .keys
                .iter()
                .map(|k| Key {
                    time: k.time,
                    value: [k.value, 0.0, 0.0],
                    // Quadratic stores Forward then Backward; TBC
                    // stores the triple. Only one applies per type.
                    fwd: [if f.interpolation == 3 { 0.0 } else { k.a }, 0.0, 0.0],
                    bwd: [if f.interpolation == 3 { 0.0 } else { k.b }, 0.0, 0.0],
                    tbc: if f.interpolation == 3 {
                        [k.a, k.b, k.c]
                    } else {
                        [0.0; 3]
                    },
                })
                .collect(),
        }
    }

    pub fn to_float(&self) -> FloatCurve {
        FloatCurve {
            interpolation: self.interpolation,
            keys: self
                .keys
                .iter()
                .map(|k| {
                    if self.interpolation == 3 {
                        FloatKey {
                            time: k.time,
                            value: k.value[0],
                            a: k.tbc[0],
                            b: k.tbc[1],
                            c: k.tbc[2],
                        }
                    } else {
                        FloatKey {
                            time: k.time,
                            value: k.value[0],
                            a: k.fwd[0],
                            b: k.bwd[0],
                            c: 0.0,
                        }
                    }
                })
                .collect(),
        }
    }

    /// Read a `NiBoolData` key list. The interpolation comes from the
    /// block, so a track keeps whatever the file said instead of the
    /// editor inventing one.
    pub fn from_bool_keys(interpolation: u32, keys: &[(f32, u8)]) -> Curve {
        Curve {
            track: Track::Bool,
            interpolation: if keys.is_empty() { 0 } else { interpolation },
            keys: keys
                .iter()
                .map(|(t, v)| Key {
                    time: *t,
                    value: [*v as f32, 0.0, 0.0],
                    ..Key::default()
                })
                .collect(),
        }
    }

    /// The `(time, raw byte)` list a `NiBoolData` stores. The byte is
    /// carried through rather than normalized: the block's own docs
    /// note that non-0/1 bytes occur, and rewriting one would be an
    /// edit the user never made.
    pub fn to_bool_keys(&self) -> Vec<(f32, u8)> {
        self.keys
            .iter()
            .map(|k| (k.time, k.value[0].clamp(0.0, 255.0) as u8))
            .collect()
    }

    /// Read a `NiPosData`-shaped block whole.
    pub fn from_pos_data(interpolation: u32, floats: &[f32]) -> Curve {
        let mut c = Curve::from_group(&Vec3KeyGroup {
            interpolation,
            floats: floats.to_vec(),
        });
        c.track = Track::Vec3;
        c
    }

    /// `(time, value)` pairs, for consumers that only need the shape.
    pub fn times_values(&self) -> impl Iterator<Item = (f32, [f32; 3])> + '_ {
        self.keys.iter().map(|k| (k.time, k.value))
    }

    /// Build a track from the old text form (`0 = 5 ~ 0.5 -0.5`,
    /// `0 = on`).
    ///
    /// A CONVENIENCE, not the storage form: the document holds a
    /// `Curve` now, precisely because the text left the key type to be
    /// recovered from how many numbers follow the widest `~`. It is
    /// kept because it is the terse way to write a track in a test,
    /// and because a curve pasted from somewhere has to arrive as
    /// something.
    pub fn parse(track: Track, text: &str) -> Curve {
        match track {
            Track::Float => Curve::from_float(&FloatCurve::parse(text)),
            Track::Bool => {
                let keys: Vec<(f32, u8)> = text
                    .split(',')
                    .filter_map(|entry| {
                        let (time, value) = entry.split_once('=')?;
                        let time: f32 = time.trim().parse().ok()?;
                        let value = match value.trim().to_ascii_lowercase().as_str() {
                            "on" | "1" | "true" => 1u8,
                            "off" | "0" | "false" => 0u8,
                            _ => return None,
                        };
                        Some((time, value))
                    })
                    .collect();
                Curve::from_bool_keys(5, &keys)
            }
            Track::Vec3 => {
                let mut keys = Vec::new();
                let mut max_extra = 0usize;
                for entry in text.split(',') {
                    let Some((time, rest)) = entry.split_once('=') else {
                        continue;
                    };
                    let Ok(time) = time.trim().parse::<f32>() else {
                        continue;
                    };
                    let (value_text, extras_text) = match rest.split_once('~') {
                        Some((v, e)) => (v, Some(e)),
                        None => (rest, None),
                    };
                    let vals: Vec<f32> = value_text
                        .split_whitespace()
                        .filter_map(|v| v.parse().ok())
                        .collect();
                    if vals.len() != 3 {
                        continue;
                    }
                    let mut extras = [0.0f32; 6];
                    let mut extra_count = 0usize;
                    if let Some(e) = extras_text {
                        for (i, part) in e.split_whitespace().take(6).enumerate() {
                            let Ok(v) = part.parse::<f32>() else { break };
                            extras[i] = v;
                            extra_count = i + 1;
                        }
                    }
                    max_extra = max_extra.max(extra_count);
                    keys.push(Key {
                        time,
                        value: [vals[0], vals[1], vals[2]],
                        fwd: [extras[0], extras[1], extras[2]],
                        bwd: [extras[3], extras[4], extras[5]],
                        tbc: [extras[0], extras[1], extras[2]],
                    });
                }
                let interpolation = if keys.is_empty() {
                    0
                } else {
                    match max_extra {
                        0 => 1,
                        3 => 3,
                        _ => 2,
                    }
                };
                for k in keys.iter_mut() {
                    match interpolation {
                        2 => k.tbc = [0.0; 3],
                        3 => {
                            k.fwd = [0.0; 3];
                            k.bwd = [0.0; 3];
                        }
                        _ => {
                            k.fwd = [0.0; 3];
                            k.bwd = [0.0; 3];
                            k.tbc = [0.0; 3];
                        }
                    }
                }
                Curve {
                    track: Track::Vec3,
                    interpolation,
                    keys,
                }
            }
        }
    }

    /// The persisted form. Keys are written as flat float arrays at
    /// the stride the interpolation implies, so a project file carries
    /// exactly the numbers the block does and nothing is recovered by
    /// counting punctuation.
    pub fn to_json(&self) -> crate::json::Json {
        use crate::json::Json;
        let stride = self.channels();
        let mut keys = Vec::with_capacity(self.keys.len());
        for k in &self.keys {
            let mut nums = vec![Json::Num(k.time as f64)];
            let mut push = |v: &[f32; 3]| {
                for c in v.iter().take(stride) {
                    nums.push(Json::Num(*c as f64));
                }
            };
            push(&k.value);
            match self.interpolation {
                2 => {
                    push(&k.fwd);
                    push(&k.bwd);
                }
                // The TBC triple is per KEY, not per channel, so it is
                // three numbers whatever the channel count.
                3 => {
                    for c in k.tbc {
                        nums.push(Json::Num(c as f64));
                    }
                }
                _ => {}
            }
            keys.push(Json::Arr(nums));
        }
        Json::obj()
            .with("track", Json::Str(self.track.id().to_string()))
            .with("interpolation", Json::Num(self.interpolation as f64))
            .with("keys", Json::Arr(keys))
    }

    /// Restore [`Self::to_json`]. A key whose float count disagrees
    /// with the stride is REFUSED rather than padded: a truncated key
    /// is corruption, and inventing zeros for it would silently
    /// flatten a tangent.
    pub fn from_json(json: &crate::json::Json) -> Option<Curve> {
        let track = Track::from_id(json.get("track")?.as_str()?)?;
        let interpolation = json.get("interpolation")?.as_u64()? as u32;
        let stride = track.channels();
        let per_key = 1 + stride
            + match interpolation {
                2 => stride * 2,
                3 => 3,
                _ => 0,
            };
        let mut keys = Vec::new();
        for item in json.get("keys")?.as_arr()? {
            let nums = item.as_arr()?;
            if nums.len() != per_key {
                return None;
            }
            let at = |i: usize| -> Option<f32> { nums.get(i)?.as_f32() };
            let mut key = Key {
                time: at(0)?,
                ..Key::default()
            };
            for c in 0..stride {
                key.value[c] = at(1 + c)?;
            }
            match interpolation {
                2 => {
                    for c in 0..stride {
                        key.fwd[c] = at(1 + stride + c)?;
                        key.bwd[c] = at(1 + 2 * stride + c)?;
                    }
                }
                3 => {
                    for c in 0..3 {
                        key.tbc[c] = at(1 + stride + c)?;
                    }
                }
                _ => {}
            }
            keys.push(key);
        }
        Some(Curve {
            track,
            interpolation,
            keys,
        })
    }

    /// The flat float list this curve serializes as, at its stride.
    pub fn to_floats(&self) -> (u32, Vec<f32>) {
        let interpolation = match self.interpolation {
            0 => 1,
            i => i,
        };
        let stride = Vec3KeyGroup::key_stride(interpolation).unwrap_or(4);
        let mut floats = Vec::with_capacity(self.keys.len() * stride);
        for k in &self.keys {
            floats.push(k.time);
            floats.extend_from_slice(&k.value);
            if stride == 10 {
                floats.extend_from_slice(&k.fwd);
                floats.extend_from_slice(&k.bwd);
            } else if stride == 7 {
                floats.extend_from_slice(&k.tbc);
            }
        }
        (interpolation, floats)
    }

    pub fn from_group(g: &Vec3KeyGroup) -> Curve {
        let stride = Vec3KeyGroup::key_stride(g.interpolation).unwrap_or(4);
        let keys: Vec<Key> = g
            .floats
            .chunks_exact(stride)
            .map(|k| Key {
                time: k[0],
                value: [k[1], k[2], k[3]],
                fwd: if stride >= 10 {
                    [k[4], k[5], k[6]]
                } else {
                    [0.0; 3]
                },
                bwd: if stride >= 10 {
                    [k[7], k[8], k[9]]
                } else {
                    [0.0; 3]
                },
                tbc: if stride == 7 {
                    [k[4], k[5], k[6]]
                } else {
                    [0.0; 3]
                },
            })
            .collect();
        Curve {
            track: Track::Vec3,
            interpolation: if keys.is_empty() { 0 } else { g.interpolation },
            keys,
        }
    }

    /// Kochanek-Bartels tangent of component `a` at key `i`; see
    /// [`FloatCurve::tbc_tangent`], which this mirrors per component.
    fn tbc_tangent(&self, i: usize, a: usize, out: bool) -> f32 {
        let k = &self.keys[i];
        let prev = if i > 0 { &self.keys[i - 1] } else { k };
        let next = if i + 1 < self.keys.len() {
            &self.keys[i + 1]
        } else {
            k
        };
        let d_in = k.value[a] - prev.value[a];
        let d_out = next.value[a] - k.value[a];
        let (t, b, c) = (k.tbc[0], k.tbc[1], k.tbc[2]);
        if out {
            0.5 * ((1.0 - t) * (1.0 + b) * (1.0 + c) * d_in
                + (1.0 - t) * (1.0 - b) * (1.0 - c) * d_out)
        } else {
            0.5 * ((1.0 - t) * (1.0 + b) * (1.0 - c) * d_in
                + (1.0 - t) * (1.0 - b) * (1.0 + c) * d_out)
        }
    }
}

/// A number for a caption: two decimals, trailing zeros gone. The
/// full precision belongs in the editor's own fields, not in a label
/// that has to fit on a node.
fn short(v: f32) -> String {
    let s = format!("{v:.2}");
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

/// Cubic Hermite on a normalized segment. `m0` rides the h10 basis
/// and `m1` rides h11; WHICH stored field is which is the caller's
/// business, and for a quadratic NIF key it is the left key's
/// Backward and the right key's Forward (module docs, `0x140D027C0`).
fn hermite(v0: f32, v1: f32, m0: f32, m1: f32, s: f32) -> f32 {
    let (s2, s3) = (s * s, s * s * s);
    let h00 = 2.0 * s3 - 3.0 * s2 + 1.0;
    let h01 = -2.0 * s3 + 3.0 * s2;
    let h10 = s3 - 2.0 * s2 + s;
    let h11 = s3 - s2;
    h00 * v0 + h01 * v1 + h10 * m0 + h11 * m1
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A shaped quadratic track: asymmetric tangents, so a curve that
    /// merely hits both keys is not enough to pass anything below.
    fn shaped() -> Curve {
        let mut c = Curve::empty(Track::Float);
        c.interpolation = 2;
        c.insert(Key { time: 0.0, value: [0.0; 3], bwd: [60.0, 0.0, 0.0], ..Key::default() });
        c.insert(Key { time: 1.0, value: [10.0, 0.0, 0.0], fwd: [-20.0, 0.0, 0.0], ..Key::default() });
        c
    }

    /// Adding a key must not change what the track PLAYS -- the whole
    /// promise of "a new key lands on the curve", and the one a value
    /// alone does not keep: with zero tangents the split segment
    /// flattens, measured at 2.78 -> 5.21 before the subdivision went
    /// in.
    #[test]
    fn adding_a_key_to_a_shaped_track_does_not_move_the_curve() {
        let mut c = shaped();
        // The fixture must really be shaped, or a straight line would
        // satisfy every assertion below and the test would be vacuous.
        let mid = c.sample(0.5).unwrap()[0];
        assert!((mid - 5.0).abs() > 1.0, "fixture is not a lerp: {mid}");

        let before: Vec<f32> = (0..=40).map(|i| c.sample(i as f32 / 40.0).unwrap()[0]).collect();
        let i = c.insert_on_curve(0.5, 999.0);
        assert_eq!(c.keys.len(), 3);
        assert!((c.keys[i].value[0] - mid).abs() < 1e-4, "took the curve's value, not the fallback");
        for (n, want) in before.iter().enumerate() {
            let t = n as f32 / 40.0;
            let got = c.sample(t).unwrap()[0];
            assert!((got - want).abs() < 1e-3, "the curve moved at t={t}: {want} -> {got}");
        }
        // Preserving it REQUIRES rewriting the neighbours: their
        // facing tangents now span a shorter segment.
        assert!((c.keys[0].bwd[0] - 30.0).abs() < 1e-4, "left tangent halved: {:?}", c.keys[0].bwd);
        assert!((c.keys[2].fwd[0] + 10.0).abs() < 1e-4, "right tangent halved: {:?}", c.keys[2].fwd);
    }

    /// A linear track carries nothing between its keys but the value,
    /// so the same call must NOT invent tangents there -- they would
    /// be fields the key type has no room for.
    #[test]
    fn adding_a_key_to_a_linear_track_stays_flat() {
        let mut c = Curve::empty(Track::Float);
        c.interpolation = 1;
        c.insert(Key { time: 0.0, value: [0.0; 3], ..Key::default() });
        c.insert(Key { time: 2.0, value: [10.0, 0.0, 0.0], ..Key::default() });
        let i = c.insert_on_curve(0.5, 0.0);
        assert!((c.keys[i].value[0] - 2.5).abs() < 1e-5);
        assert!(c.keys.iter().all(|k| k.fwd == [0.0; 3] && k.bwd == [0.0; 3]));
        assert!((c.sample(1.5).unwrap()[0] - 7.5).abs() < 1e-5);
    }

    /// A bool key has no value between on and off.
    #[test]
    fn adding_a_key_to_a_bool_track_snaps() {
        let mut c = Curve::empty(Track::Bool);
        c.insert(Key { time: 0.0, value: [1.0, 0.0, 0.0], ..Key::default() });
        c.insert_on_curve(0.5, 0.0);
        assert!(c.keys.iter().all(|k| k.value[0] == 0.0 || k.value[0] == 1.0));
    }

    /// The text form still round-trips every representable curve. It
    /// is no longer the document's storage — [`Curve`] is — but
    /// [`Curve::parse`] is built on it, so a break here is a break in
    /// every track written in a test or pasted from elsewhere.
    #[test]
    fn text_round_trips_every_key_type() {
        for (text, interp, n) in [
            ("0 = 5, 1 = 40", 1u32, 2usize),
            ("0 = 5 ~ 0.5 -0.5, 1 = 40 ~ 0 0", 2, 2),
            ("0 = 5 ~ 0.25 0.5 -0.75, 2 = 1 ~ 0 0 0", 3, 2),
        ] {
            let c = FloatCurve::parse(text);
            assert_eq!(c.interpolation, interp, "{text}");
            assert_eq!(c.keys.len(), n, "{text}");
            let again = FloatCurve::parse(&c.format());
            assert_eq!(again, c, "{text} did not survive format+parse");
        }
        // Junk entries are skipped, not poisonous.
        let c = FloatCurve::parse("garbage, 1 = 2, also garbage");
        assert_eq!(c.keys.len(), 1);
    }

    /// The PERSISTED form carries every key type, both channel counts
    /// and the tangents, and it carries the key TYPE as a field —
    /// which is the whole reason the document stopped storing a
    /// sentence. The text form could not spell CONST at all (the two
    /// corpus blocks that use it came back linear); this one can, and
    /// the case is asserted first so the test cannot pass by the type
    /// being ignored everywhere.
    #[test]
    fn the_persisted_form_carries_the_key_type_and_the_tangents() {
        let const_track = Curve {
            track: Track::Float,
            interpolation: 5,
            keys: vec![
                Key { time: 0.0, value: [5.0, 0.0, 0.0], ..Key::default() },
                Key { time: 1.0, value: [40.0, 0.0, 0.0], ..Key::default() },
            ],
        };
        let back = reparse(&const_track);
        assert_eq!(back.interpolation, 5, "CONST did not survive the project format");
        assert_eq!(back, const_track);
        // ...and CONST really is a different curve from linear, or the
        // assertion above would be about a label rather than a shape.
        let mut linear = const_track.clone();
        linear.interpolation = 1;
        assert_eq!(const_track.sample_f32(0.5), Some(5.0));
        assert_eq!(linear.sample_f32(0.5), Some(22.5));

        for track in [Track::Float, Track::Bool, Track::Vec3] {
            for interpolation in [1u32, 2, 3, 5] {
                if track == Track::Bool && !matches!(interpolation, 1 | 5) {
                    continue;
                }
                let c = Curve {
                    track,
                    interpolation,
                    keys: vec![
                        Key {
                            time: 0.25,
                            value: [1.0, 2.0, 3.0],
                            fwd: [0.5, -0.5, 0.25],
                            bwd: [-1.0, 1.0, 0.0],
                            tbc: [0.1, 0.2, 0.3],
                        },
                        Key {
                            time: 1.5,
                            value: [4.0, 5.0, 6.0],
                            fwd: [0.0, 0.0, 0.0],
                            bwd: [2.0, 0.0, -2.0],
                            tbc: [0.0, -0.5, 0.5],
                        },
                    ],
                };
                // Only the fields the TYPE uses are written, so compare
                // against what the reader can legitimately produce
                // rather than against the raw literal above.
                let once = reparse(&c);
                let twice = reparse(&once);
                assert_eq!(once, twice, "{track:?}/{interpolation} is not stable");
                if interpolation == 2 && track != Track::Bool {
                    assert_eq!(
                        once.keys[0].fwd[0], 0.5,
                        "{track:?} lost a quadratic tangent"
                    );
                    assert_eq!(once.keys[1].bwd[0], 2.0);
                }
                if interpolation == 3 && track != Track::Bool {
                    assert_eq!(once.keys[0].tbc, [0.1, 0.2, 0.3], "{track:?} lost TBC");
                }
                if track == Track::Vec3 {
                    assert_eq!(once.keys[1].value, [4.0, 5.0, 6.0]);
                }
            }
        }
    }

    /// A truncated key is REFUSED, not padded with zeros — padding
    /// would turn a corrupt file into a silently flattened tangent.
    #[test]
    fn a_short_key_is_refused() {
        let good = Curve {
            track: Track::Float,
            interpolation: 2,
            keys: vec![Key { time: 0.0, value: [1.0, 0.0, 0.0], ..Key::default() }],
        };
        let json = good.to_json();
        assert!(Curve::from_json(&json).is_some());
        // The same document with one number dropped from the key: a
        // quadratic float key is time + value + fwd + bwd, so three
        // numbers is one short.
        let short = crate::json::Json::obj()
            .with("track", crate::json::Json::Str("float".into()))
            .with("interpolation", crate::json::Json::Num(2.0))
            .with(
                "keys",
                crate::json::Json::Arr(vec![crate::json::Json::Arr(vec![
                    crate::json::Json::Num(0.0),
                    crate::json::Json::Num(1.0),
                    crate::json::Json::Num(0.0),
                ])]),
            );
        assert!(
            Curve::from_json(&short).is_none(),
            "a key short of its stride was accepted"
        );
    }

    /// Keys stay sorted through an insert and through a drag that
    /// crosses a neighbour. The runtime scans segments FORWARD from a
    /// cached index (1.5.97 `0x140CDCC00`), so an out-of-order key is
    /// not a cosmetic problem — the game reads the wrong segment.
    #[test]
    fn keys_stay_sorted() {
        let mut c = Curve::parse(Track::Float, "0 = 0, 1 = 10, 2 = 20");
        let at = c.insert(Key { time: 0.5, value: [5.0; 3], ..Key::default() });
        assert_eq!(at, 1);
        assert!(c.keys.windows(2).all(|w| w[0].time <= w[1].time));
        // Drag the first key past the last one.
        let moved = c.retime(0, 9.0);
        assert_eq!(moved, c.keys.len() - 1, "the dragged key did not re-sort");
        assert!(c.keys.windows(2).all(|w| w[0].time <= w[1].time));
        assert_eq!(c.keys.len(), 4, "re-sorting lost or duplicated a key");
    }

    fn reparse(c: &Curve) -> Curve {
        let text = c.to_json().pretty();
        Curve::from_json(&crate::json::Json::parse(&text).expect("valid JSON"))
            .expect("a curve did not survive its own JSON")
    }

    /// `NiFloatData` round-trips through the curve BYTE-EXACTLY for
    /// every stride, which is the whole point: this is what lets the
    /// compiler re-emit what the importer read.
    #[test]
    fn nifloatdata_round_trips_through_the_curve() {
        for (interp, floats) in [
            (1u32, vec![0.0f32, 5.0, 1.0, 40.0]),
            (2, vec![0.0, 5.0, 0.5, -0.5, 1.0, 40.0, 0.125, 0.25]),
            (3, vec![0.0, 5.0, 0.1, 0.2, 0.3, 2.0, 1.0, 0.0, 0.0, 0.0]),
            (5, vec![0.0, 5.0, 1.0, 40.0]),
        ] {
            let data = NiFloatData {
                interpolation: interp,
                floats: floats.clone(),
            };
            let back = FloatCurve::from_data(&data).to_data();
            assert_eq!(back.interpolation, interp);
            assert_eq!(back.floats, floats, "stride {interp} lost floats");
        }
        // ...and the TEXT round trip preserves the types it can carry
        // (const deliberately cannot: two corpus blocks, both the dead
        // editor's own output).
        for interp in [1u32, 2, 3] {
            let data = NiFloatData {
                interpolation: interp,
                floats: match interp {
                    1 => vec![0.0, 5.0, 1.0, 40.0],
                    2 => vec![0.0, 5.0, 0.5, -0.5, 1.0, 40.0, 0.125, 0.25],
                    _ => vec![0.0, 5.0, 0.1, 0.2, 0.3, 2.0, 1.0, 0.0, 0.0, 0.0],
                },
            };
            let text = FloatCurve::from_data(&data).format();
            let back = FloatCurve::parse(&text).to_data();
            assert_eq!(back.interpolation, interp, "type lost through text");
            assert_eq!(back.floats, data.floats, "floats lost through text");
        }
    }

    /// Quadratic sampling is cubic Hermite with the pairing MEASURED in
    /// the runtime: the left key's BACKWARD on h10, the right key's
    /// FORWARD on h11 (1.5.97 `0x140CF84C0` -> `0x140D027C0`).
    ///
    /// The decoy tangents are chosen so the three candidate behaviours
    /// give three DIFFERENT answers at s = 0.5:
    ///   - lerp, or Hermite with the tangents dropped:  30.0
    ///   - the swapped pairing this file used to ship:   36.0
    ///   - the runtime's pairing:                        55.0
    #[test]
    fn quadratic_sampling_uses_the_measured_tangent_pairing() {
        let c = FloatCurve::parse("0 = 10 ~ 24 100, 1 = 50 ~ -100 -24");
        assert_eq!(c.interpolation, 2);
        // At s=0.5: h00=h01=0.5, h10=0.125, h11=-0.125.
        // m0 = k0.backward = 100, m1 = k1.forward = -100:
        //   0.5*10 + 0.5*50 + 0.125*100 + (-0.125)*(-100) = 55.
        let v = c.sample(0.5).unwrap();
        assert!((v - 55.0).abs() < 1e-4, "sampled {v}, expected 55");
        // Endpoints are exact regardless of tangents.
        assert_eq!(c.sample(0.0), Some(10.0));
        assert_eq!(c.sample(1.0), Some(50.0));
        // ...and clamped outside the range.
        assert_eq!(c.sample(-5.0), Some(10.0));
        assert_eq!(c.sample(5.0), Some(50.0));
    }

    /// TBC with all-zero parameters is Catmull–Rom; a middle key's
    /// tangent is (next - prev) / 2.
    #[test]
    fn tbc_zero_parameters_are_catmull_rom() {
        let c = FloatCurve::parse("0 = 0 ~ 0 0 0, 1 = 10 ~ 0 0 0, 2 = 0 ~ 0 0 0");
        assert_eq!(c.interpolation, 3);
        // Symmetric tent: the peak's Catmull-Rom tangent is 0, so the
        // curve is symmetric and s=0.5 of the first segment must equal
        // s=0.5 of the second.
        let left = c.sample(0.5).unwrap();
        let right = c.sample(1.5).unwrap();
        assert!((left - right).abs() < 1e-4, "{left} vs {right}");
        // And it overshoots a lerp on the way up (the ease-in from the
        // flat start), which pins that tangents participate at all.
        assert!(
            (left - 5.0).abs() > 0.1,
            "TBC sampled exactly like a lerp: {left}"
        );
    }

    /// Linear is a lerp, single-key curves are constants, and an empty
    /// curve says None rather than inventing a value.
    #[test]
    fn linear_and_degenerate_sampling() {
        let c = FloatCurve::parse("0 = 10, 1 = 50");
        assert_eq!(c.sample(0.25), Some(20.0));
        let single = FloatCurve::parse("3 = 7");
        assert_eq!(single.sample(0.0), Some(7.0));
        assert_eq!(single.sample(99.0), Some(7.0));
        assert_eq!(FloatCurve::parse("").sample(0.0), None);
    }
}
