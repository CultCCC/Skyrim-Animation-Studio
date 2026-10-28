//! `NiPathInterpolator` evaluation — an object following a spline path.
//!
//! Semantics reverse-engineered from SkyrimSE 1.5.97:
//! - `Update` @ 0x140CD4410: percent from the float track, percent →
//!   segment, quadratic (Hermite) position, optional follow rotation.
//! - percent → segment @ 0x140CD57E0: WITHOUT ConstantVelocity the percent
//!   is compared against the position keys' TIME values directly (clamped
//!   at both ends, no wrap); real path keys span 0..1.
//! - ConstantVelocity @ 0x140CD5590: percent wraps (`p < 0` →
//!   `fmod(p,1)+1`, then `p == 1` clamps to the end, then `p > 1` →
//!   `fmod`), target arc = percent × total length, segment found in a
//!   cumulative per-key length table, then Newton refinement on the true
//!   spline to 1e-4. We precompute a 32-sample sub-table per segment and
//!   invert by linear interpolation instead — deterministic, and within a
//!   fraction of a unit on every vanilla path.
//! - Follow frame @ 0x140CD4D60: with AllowFlip (bit 2) CLEAR — and NO
//!   vanilla file sets it — the frame is yaw-from-tangent: columns
//!   `(T, S, U)` with `T` the normalized tangent, `S = normalize(cross(Z,
//!   T))` (world Z is the constant at 0x141E062F0), `U = cross(T, S)`.
//!   Follow Axis remaps which column carries `T` (axis 1: `(-S, T, U)`,
//!   axis 2: `(-U, S, T)`); Flip (bit 6) negates `T` and `S` first — a
//!   180° yaw. Bank (bit 3) exists but the only vanilla user (the hawk)
//!   ships Max Bank Angle 0, so it is not implemented; the AllowFlip
//!   Frenet-frame branch with segment-boundary smoothing is dead code for
//!   the whole corpus and is not implemented either.
//!
//! The game zeroes the side vector when the tangent is (anti)parallel to
//! world Z, producing a degenerate matrix; we keep the node's authored
//! rotation instead (`rotation: None`), because a zeroed rotation collapses
//! the mesh — a state the game's own content never reaches.

use crate::sequence::TransformSample;

/// Sub-samples per segment for the constant-velocity arc-length table.
const ARC_SAMPLES: usize = 32;

/// One position key: time (the curve parameter), value, and the Hermite
/// out/in tangents (zero when the path is linear).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PosKey {
    pub t: f32,
    pub v: [f32; 3],
    pub fwd: [f32; 3],
    pub bwd: [f32; 3],
}

/// A ready-to-sample path track: the interpolator's flags digested into
/// booleans, both key sets, and the precomputed arc-length tables.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PathKeys {
    pub follow: bool,
    pub flip: bool,
    pub constant_velocity: bool,
    /// 0/1/2 — which local axis follows the tangent.
    pub follow_axis: u16,
    /// Hermite when true, linear otherwise (the quadratic key type — every
    /// vanilla path is quadratic).
    pub quadratic: bool,
    pub keys: Vec<PosKey>,
    /// Percent track: `(time, value, fwd, bwd)`, Hermite when
    /// `percent_quadratic`, clamped outside its time range.
    pub percent: Vec<(f32, f32, f32, f32)>,
    pub percent_quadratic: bool,
    /// Cumulative arc length at each key (index 0 = 0) and the per-segment
    /// sub-tables (each `ARC_SAMPLES + 1` cumulative values from 0), built
    /// by [`PathKeys::finish`] when constant-velocity.
    pub cum: Vec<f32>,
    pub sub: Vec<Vec<f32>>,
    pub total_len: f32,
}

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// Cubic Hermite between two keys at normalized `t`.
///
/// The tangent pairing is the LEFT key's Backward and the RIGHT key's
/// Forward, measured in 1.5.97's key dispatch -- see `curve.rs`. This
/// function paired them the other way round from the day it shipped,
/// so every vanilla spline path (salmon, hawks, moths, the Dwemer
/// astrolabe) travelled a subtly wrong curve between its keys while
/// still hitting every key exactly, which is why nothing looked
/// broken enough to investigate.
fn hermite(k0: &PosKey, k1: &PosKey, t: f32) -> [f32; 3] {
    let (t2, t3) = (t * t, t * t * t);
    let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
    let h01 = -2.0 * t3 + 3.0 * t2;
    let h10 = t3 - 2.0 * t2 + t;
    let h11 = t3 - t2;
    let mut out = [0.0; 3];
    for a in 0..3 {
        out[a] = h00 * k0.v[a] + h01 * k1.v[a] + h10 * k0.bwd[a] + h11 * k1.fwd[a];
    }
    out
}

/// d/dt of [`hermite`] — only its direction is used.
fn hermite_deriv(k0: &PosKey, k1: &PosKey, t: f32) -> [f32; 3] {
    let t2 = t * t;
    let h00 = 6.0 * t2 - 6.0 * t;
    let h01 = -h00;
    let h10 = 3.0 * t2 - 4.0 * t + 1.0;
    let h11 = 3.0 * t2 - 2.0 * t;
    let mut out = [0.0; 3];
    for a in 0..3 {
        out[a] = h00 * k0.v[a] + h01 * k1.v[a] + h10 * k0.bwd[a] + h11 * k1.fwd[a];
    }
    out
}

impl PathKeys {
    fn eval_segment(&self, i: usize, t: f32) -> [f32; 3] {
        let (k0, k1) = (&self.keys[i], &self.keys[i + 1]);
        if self.quadratic {
            hermite(k0, k1, t)
        } else {
            lerp3(k0.v, k1.v, t)
        }
    }

    fn deriv_segment(&self, i: usize, t: f32) -> [f32; 3] {
        let (k0, k1) = (&self.keys[i], &self.keys[i + 1]);
        if self.quadratic {
            hermite_deriv(k0, k1, t)
        } else {
            [
                k1.v[0] - k0.v[0],
                k1.v[1] - k0.v[1],
                k1.v[2] - k0.v[2],
            ]
        }
    }

    /// Build the arc-length tables. Call once after filling the keys;
    /// a no-op for non-constant-velocity tracks (they never consult them).
    pub fn finish(&mut self) {
        self.cum.clear();
        self.sub.clear();
        self.total_len = 0.0;
        if !self.constant_velocity || self.keys.len() < 2 {
            return;
        }
        self.cum.push(0.0);
        for i in 0..self.keys.len() - 1 {
            let mut table = Vec::with_capacity(ARC_SAMPLES + 1);
            table.push(0.0);
            let mut prev = self.eval_segment(i, 0.0);
            let mut acc = 0.0f32;
            for s in 1..=ARC_SAMPLES {
                let p = self.eval_segment(i, s as f32 / ARC_SAMPLES as f32);
                let d = [p[0] - prev[0], p[1] - prev[1], p[2] - prev[2]];
                acc += (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                table.push(acc);
                prev = p;
            }
            self.total_len += acc;
            self.cum.push(self.total_len);
            self.sub.push(table);
        }
    }

    /// Percent value at controller-local time `t` — the float track,
    /// clamped outside its range, Hermite between quadratic keys.
    fn percent_at(&self, t: f32) -> f32 {
        let keys = &self.percent;
        match keys.len() {
            0 => return 0.0,
            1 => return keys[0].1,
            _ => {}
        }
        if t <= keys[0].0 {
            return keys[0].1;
        }
        if t >= keys[keys.len() - 1].0 {
            return keys[keys.len() - 1].1;
        }
        let i = keys.partition_point(|k| k.0 <= t).saturating_sub(1);
        let (t0, v0, _, bwd0) = keys[i];
        let (t1, v1, fwd1, _) = keys[i + 1];
        let span = (t1 - t0).max(1e-6);
        let u = (t - t0) / span;
        if self.percent_quadratic {
            let (u2, u3) = (u * u, u * u * u);
            (2.0 * u3 - 3.0 * u2 + 1.0) * v0
                + (-2.0 * u3 + 3.0 * u2) * v1
                + (u3 - 2.0 * u2 + u) * bwd0
                + (u3 - u2) * fwd1
        } else {
            v0 + (v1 - v0) * u
        }
    }

    /// Percent → (segment, fraction), the game's two mappings.
    fn segment_at(&self, percent: f32) -> (usize, f32) {
        let n = self.keys.len();
        if self.constant_velocity {
            // The wrap order is the game's exactly: 0 short-circuits,
            // negatives wrap first, an exact 1.0 clamps to the end, then
            // anything above 1 wraps.
            let mut p = percent;
            if p == 0.0 {
                return (0, 0.0);
            }
            if p < 0.0 {
                p = p % 1.0 + 1.0;
            }
            if p == 1.0 {
                return (n - 2, 1.0);
            }
            if p > 1.0 {
                p %= 1.0;
            }
            let target = p * self.total_len;
            let mut i = 0;
            while i + 1 < self.cum.len() - 1 && target > self.cum[i + 1] {
                i += 1;
            }
            let within = target - self.cum[i];
            // Invert the sub-table by linear interpolation (the game
            // Newton-iterates on the true spline to 1e-4 instead).
            let table = &self.sub[i];
            let j = table.partition_point(|&l| l <= within).saturating_sub(1);
            if j + 1 >= table.len() {
                return (i, 1.0);
            }
            let seg = (table[j + 1] - table[j]).max(1e-12);
            let frac = (within - table[j]) / seg;
            (i, (j as f32 + frac) / ARC_SAMPLES as f32)
        } else {
            // Percent against the pos keys' own time values, clamped, no
            // wrap (RE: the else branch of 0x140CD57E0).
            if percent <= self.keys[0].t {
                return (0, 0.0);
            }
            if percent >= self.keys[n - 1].t {
                return (n - 2, 1.0);
            }
            let i = self
                .keys
                .partition_point(|k| k.t <= percent)
                .saturating_sub(1);
            let span = (self.keys[i + 1].t - self.keys[i].t).max(1e-6);
            (i, (percent - self.keys[i].t) / span)
        }
    }
}

/// Build a [`PathKeys`] from a decoded `NiPathInterpolator` block and its
/// data blocks. `None` when either data ref is missing or empty — the game
/// updates nothing in that case, and so do we.
pub fn build(
    file: &aether_nif::NifFile,
    ctx: aether_nif::typed::Ctx,
    block: &aether_nif::Block,
) -> Option<PathKeys> {
    use aether_nif::typed::controllers::{NiFloatData, NiPathInterpolator, NiPosData};
    use aether_nif::typed::TypedBlock;

    let pi = NiPathInterpolator::decode(block, ctx).ok()?;
    let pos = NiPosData::decode(file.blocks.get(pi.path_data.get()?)?, ctx).ok()?;
    let pct = NiFloatData::decode(file.blocks.get(pi.percent_data.get()?)?, ctx).ok()?;
    let keys: Vec<PosKey> = pos
        .hermite_keys()
        .into_iter()
        .map(|(t, v, fwd, bwd)| PosKey { t, v, fwd, bwd })
        .collect();
    if keys.len() < 2 {
        return None;
    }
    let mut out = PathKeys {
        follow: pi.flags & 0x20 != 0,
        flip: pi.flags & 0x40 != 0,
        constant_velocity: pi.flags & 0x10 != 0,
        follow_axis: pi.follow_axis,
        quadratic: pos.is_quadratic(),
        keys,
        percent: pct.hermite_keys(),
        percent_quadratic: pct.is_quadratic(),
        ..PathKeys::default()
    };
    out.finish();
    Some(out)
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize(v: [f32; 3]) -> Option<[f32; 3]> {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    (len > 1e-6).then(|| [v[0] / len, v[1] / len, v[2] / len])
}

/// Quaternion (wxyz) from a row-major rotation matrix (Shepperd's method).
fn quat_from_rows(m: &[f32; 9]) -> [f32; 4] {
    let trace = m[0] + m[4] + m[8];
    if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        [
            0.25 * s,
            (m[7] - m[5]) / s,
            (m[2] - m[6]) / s,
            (m[3] - m[1]) / s,
        ]
    } else if m[0] > m[4] && m[0] > m[8] {
        let s = (1.0 + m[0] - m[4] - m[8]).sqrt() * 2.0;
        [
            (m[7] - m[5]) / s,
            0.25 * s,
            (m[1] + m[3]) / s,
            (m[2] + m[6]) / s,
        ]
    } else if m[4] > m[8] {
        let s = (1.0 + m[4] - m[0] - m[8]).sqrt() * 2.0;
        [
            (m[2] - m[6]) / s,
            (m[1] + m[3]) / s,
            0.25 * s,
            (m[5] + m[7]) / s,
        ]
    } else {
        let s = (1.0 + m[8] - m[0] - m[4]).sqrt() * 2.0;
        [
            (m[3] - m[1]) / s,
            (m[2] + m[6]) / s,
            (m[5] + m[7]) / s,
            0.25 * s,
        ]
    }
}

/// Sample the path at controller-local time `t`: translation always (when
/// there are at least 2 keys), rotation only when Follow is set and the
/// frame is non-degenerate.
pub fn sample_path(keys: &PathKeys, t: f32) -> TransformSample {
    if keys.keys.len() < 2 {
        return TransformSample {
            translation: keys.keys.first().map(|k| k.v),
            ..TransformSample::default()
        };
    }
    let percent = keys.percent_at(t);
    let (i, frac) = keys.segment_at(percent);
    let translation = keys.eval_segment(i, frac);

    let mut rotation = None;
    if keys.follow {
        if let Some(tangent) = normalize(keys.deriv_segment(i, frac)) {
            // side = normalize(cross(world Z, tangent)) — zero when the
            // tangent is vertical, in which case the game emits a
            // degenerate frame and we keep the authored rotation.
            if let Some(side) = normalize(cross([0.0, 0.0, 1.0], tangent)) {
                let up = cross(tangent, side);
                let (tv, sv) = if keys.flip {
                    (
                        [-tangent[0], -tangent[1], -tangent[2]],
                        [-side[0], -side[1], -side[2]],
                    )
                } else {
                    (tangent, side)
                };
                // Columns by follow axis (RE 0x140CD4D60): which local
                // axis carries the tangent.
                let (c0, c1, c2) = match keys.follow_axis {
                    1 => ([-sv[0], -sv[1], -sv[2]], tv, up),
                    2 => ([-up[0], -up[1], -up[2]], sv, tv),
                    _ => (tv, sv, up),
                };
                let rows = [
                    c0[0], c1[0], c2[0], //
                    c0[1], c1[1], c2[1], //
                    c0[2], c1[2], c2[2],
                ];
                rotation = Some(quat_from_rows(&rows));
            }
        }
    }

    TransformSample {
        translation: Some(translation),
        rotation,
        scale: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line_keys(cv: bool) -> PathKeys {
        // Deliberately uneven segments: 1 unit then 9 units. Constant
        // velocity and time-based mapping disagree hard on this path,
        // which is what lets the tests tell them apart.
        let mut k = PathKeys {
            constant_velocity: cv,
            quadratic: false,
            keys: vec![
                PosKey {
                    t: 0.0,
                    v: [0.0, 0.0, 0.0],
                    ..PosKey::default()
                },
                PosKey {
                    t: 0.5,
                    v: [1.0, 0.0, 0.0],
                    ..PosKey::default()
                },
                PosKey {
                    t: 1.0,
                    v: [10.0, 0.0, 0.0],
                    ..PosKey::default()
                },
            ],
            percent: vec![(0.0, 0.0, 0.0, 0.0), (1.0, 1.0, 0.0, 0.0)],
            ..PathKeys::default()
        };
        k.finish();
        k
    }

    #[test]
    fn hermite_hits_its_keys_exactly() {
        // Tangents live in the fields the RUNTIME reads: the left
        // key's Backward and the right key's Forward. Filling the
        // other two (as this fixture used to) makes the segment a
        // plain smoothstep and the midpoint assertion vacuous.
        let k0 = PosKey {
            t: 0.0,
            v: [1.0, 2.0, 3.0],
            fwd: [0.0; 3],
            bwd: [5.0, -4.0, 2.0],
        };
        let k1 = PosKey {
            t: 1.0,
            v: [-7.0, 6.0, 0.5],
            fwd: [1.0, 1.0, 1.0],
            bwd: [0.0; 3],
        };
        assert_eq!(hermite(&k0, &k1, 0.0), k0.v, "t=0 must be key 0");
        assert_eq!(hermite(&k0, &k1, 1.0), k1.v, "t=1 must be key 1");
        // The tangents must actually matter: the midpoint of a straight
        // lerp would be the average; the Hermite midpoint is not.
        let mid = hermite(&k0, &k1, 0.5);
        assert!((mid[0] - (k0.v[0] + k1.v[0]) * 0.5).abs() > 0.1);
    }

    #[test]
    fn constant_velocity_paces_by_arc_length_not_key_time() {
        let cv = line_keys(true);
        let plain = line_keys(false);
        // Halfway percent. The path is 10 units; CV must stand at 5.0
        // (halfway by DISTANCE, inside the long segment); the time-based
        // mapping stands at key 1 (1.0), because 0.5 is that key's time.
        let cv_mid = sample_path(&cv, 0.5).translation.unwrap();
        let plain_mid = sample_path(&plain, 0.5).translation.unwrap();
        assert!((cv_mid[0] - 5.0).abs() < 0.05, "cv {cv_mid:?}");
        assert!((plain_mid[0] - 1.0).abs() < 1e-5, "plain {plain_mid:?}");
    }

    #[test]
    fn constant_velocity_wraps_like_the_game() {
        let cv = line_keys(true);
        let a = sample_path(&cv, 0.25).translation.unwrap();
        // percent 1.25 wraps to 0.25; -0.75 wraps to 0.25 as well.
        let mut wide = line_keys(true);
        wide.percent = vec![(0.0, 1.25, 0.0, 0.0)];
        let b = sample_path(&wide, 0.0).translation.unwrap();
        wide.percent = vec![(0.0, -0.75, 0.0, 0.0)];
        let c = sample_path(&wide, 0.0).translation.unwrap();
        assert!((a[0] - b[0]).abs() < 1e-4, "{a:?} vs {b:?}");
        assert!((a[0] - c[0]).abs() < 1e-4, "{a:?} vs {c:?}");
    }

    #[test]
    fn follow_frame_is_a_proper_rotation_facing_the_tangent() {
        let mut k = line_keys(true);
        k.follow = true;
        k.follow_axis = 1; // the salmon's: local Y along the tangent
        let s = sample_path(&k, 0.3);
        let q = s.rotation.expect("follow must produce a rotation");
        // Unit quaternion.
        let n: f32 = q.iter().map(|c| c * c).sum();
        assert!((n - 1.0).abs() < 1e-5, "{q:?}");
        // Rotate local +Y by q: must land on the tangent (+X here).
        let (w, x, y, z) = (q[0], q[1], q[2], q[3]);
        let rot_y = [
            2.0 * (x * y - w * z),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z + w * x),
        ];
        assert!(
            (rot_y[0] - 1.0).abs() < 1e-5 && rot_y[1].abs() < 1e-5 && rot_y[2].abs() < 1e-5,
            "local +Y should map onto the +X tangent, got {rot_y:?}"
        );
        // Flip turns it around.
        k.flip = true;
        let q = sample_path(&k, 0.3).rotation.unwrap();
        let (w, x, y, z) = (q[0], q[1], q[2], q[3]);
        let rot_y = [
            2.0 * (x * y - w * z),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z + w * x),
        ];
        assert!((rot_y[0] + 1.0).abs() < 1e-5, "flip must face -X: {rot_y:?}");
    }

    #[test]
    fn vertical_tangent_keeps_the_authored_rotation() {
        let mut k = PathKeys {
            constant_velocity: false,
            quadratic: false,
            follow: true,
            keys: vec![
                PosKey {
                    t: 0.0,
                    v: [0.0, 0.0, 0.0],
                    ..PosKey::default()
                },
                PosKey {
                    t: 1.0,
                    v: [0.0, 0.0, 10.0],
                    ..PosKey::default()
                },
            ],
            percent: vec![(0.0, 0.5, 0.0, 0.0)],
            ..PathKeys::default()
        };
        k.finish();
        let s = sample_path(&k, 0.0);
        assert!(s.translation.is_some());
        assert!(
            s.rotation.is_none(),
            "a vertical tangent has no yaw frame; the node keeps its own"
        );
    }
}
