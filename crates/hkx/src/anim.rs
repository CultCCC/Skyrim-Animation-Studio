//! Skeletons and pose sampling for Skyrim SE clips.
//!
//! `hkaSplineCompressedAnimation` layout (Havok 2010.2, little endian), per
//! block at `blockOffsets[b]`:
//! - one 4-byte mask per transform track: quantization types (position 2 bits,
//!   rotation 4 bits, scale 2 bits), then position/rotation/scale flags
//!   (low nibble = static axes, high nibble = spline axes); then one byte per
//!   float track; align 4;
//! - per track: position, rotation, scale channels. A spline channel stores
//!   `u16 numItems, u8 degree, knots[numItems + degree + 2]`, then (vectors)
//!   per-axis `min,max` for spline axes or one float for static axes and
//!   `numItems + 1` quantized control points; (rotations) the control
//!   quaternions in the rotation quantization. Every channel ends 4-aligned.
//! Knots are in frames from the block start. Evaluation is a plain B-spline.

use crate::{parse, ClassMap, HkxError};
use havok_classes::Classes;

/// Local bone transform. Rotation is (x, y, z, w).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Xform {
    pub t: [f32; 3],
    pub r: [f32; 4],
    pub s: [f32; 3],
}

impl Default for Xform {
    fn default() -> Self {
        Xform { t: [0.0; 3], r: [0.0, 0.0, 0.0, 1.0], s: [1.0; 3] }
    }
}

#[derive(Debug, Clone)]
pub struct Skeleton {
    pub name: String,
    pub bones: Vec<String>,
    pub parents: Vec<i16>,
    pub reference: Vec<Xform>,
}

impl Skeleton {
    pub fn bone(&self, name: &str) -> Option<usize> {
        self.bones.iter().position(|b| b.eq_ignore_ascii_case(name))
    }
}

fn qs(q: &havok_types::QsTransform) -> Xform {
    Xform {
        t: [q.transition.x, q.transition.y, q.transition.z],
        r: [q.quaternion.x, q.quaternion.y, q.quaternion.z, q.quaternion.scaler],
        s: [q.scale.x, q.scale.y, q.scale.z],
    }
}

/// Every hkaSkeleton in a skeleton.hkx (body first, ragdoll after).
pub fn read_skeletons(bytes: &[u8]) -> Result<Vec<Skeleton>, HkxError> {
    let map = parse(bytes)?;
    let mut out = Vec::new();
    for c in map.values() {
        if let Classes::hkaSkeleton(s) = c {
            out.push(Skeleton {
                name: s.m_name.as_str().to_string(),
                bones: s.m_bones.iter().map(|b| b.m_name.as_str().to_string()).collect(),
                parents: s.m_parentIndices.clone(),
                reference: s.m_referencePose.iter().map(qs).collect(),
            });
        }
    }
    if out.is_empty() {
        return Err(HkxError::Parse("no hkaSkeleton in the file".into()));
    }
    Ok(out)
}

// ---- channels ----------------------------------------------------------------

#[derive(Debug, Clone)]
enum Channel<const N: usize> {
    Const([f32; N]),
    Spline { degree: usize, knots: Vec<f32>, points: Vec<[f32; N]> },
}

impl<const N: usize> Channel<N> {
    fn eval(&self, u: f32) -> [f32; N] {
        match self {
            Channel::Const(v) => *v,
            Channel::Spline { degree, knots, points } => bspline(*degree, knots, points, u),
        }
    }
}

#[derive(Debug, Clone)]
struct TrackBlock {
    pos: Channel<3>,
    rot: Channel<4>,
    scale: Channel<3>,
}

#[derive(Debug, Clone)]
enum Data {
    Spline { blocks: Vec<Vec<TrackBlock>>, block_duration: f32, frame_duration: f32 },
    Interleaved { frames: usize, transforms: Vec<Xform> },
}

/// A decoded clip, ready to sample.
#[derive(Debug, Clone)]
pub struct Animation {
    pub duration: f32,
    pub tracks: usize,
    /// Track → skeleton bone. Empty in the file means identity.
    pub track_to_bone: Vec<i16>,
    data: Data,
}

impl Animation {
    pub fn bone_of_track(&self, track: usize) -> usize {
        self.track_to_bone.get(track).map(|&b| b as usize).unwrap_or(track)
    }

    /// Local transforms per track at `time` (clamped to the clip).
    pub fn sample(&self, time: f32) -> Vec<Xform> {
        let time = time.clamp(0.0, self.duration);
        match &self.data {
            Data::Interleaved { frames, transforms } => {
                if *frames == 0 {
                    return vec![Xform::default(); self.tracks];
                }
                let f = if *frames > 1 { time / self.duration * (*frames - 1) as f32 } else { 0.0 };
                let f0 = (f.floor() as usize).min(frames - 1);
                let f1 = (f0 + 1).min(frames - 1);
                let a = f - f0 as f32;
                (0..self.tracks)
                    .map(|t| blend(&transforms[f0 * self.tracks + t], &transforms[f1 * self.tracks + t], a))
                    .collect()
            }
            Data::Spline { blocks, block_duration, frame_duration } => {
                let b = ((time / block_duration) as usize).min(blocks.len() - 1);
                let u = (time - b as f32 * block_duration) / frame_duration;
                blocks[b]
                    .iter()
                    .map(|tb| Xform { t: tb.pos.eval(u), r: normalize(tb.rot.eval(u)), s: tb.scale.eval(u) })
                    .collect()
            }
        }
    }
}

fn blend(a: &Xform, b: &Xform, f: f32) -> Xform {
    let lerp3 = |x: [f32; 3], y: [f32; 3]| [x[0] + (y[0] - x[0]) * f, x[1] + (y[1] - x[1]) * f, x[2] + (y[2] - x[2]) * f];
    Xform { t: lerp3(a.t, b.t), r: nlerp(a.r, b.r, f), s: lerp3(a.s, b.s) }
}

pub fn nlerp(a: [f32; 4], b: [f32; 4], f: f32) -> [f32; 4] {
    let d = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    let s = if d < 0.0 { -1.0 } else { 1.0 };
    normalize([
        a[0] + (s * b[0] - a[0]) * f,
        a[1] + (s * b[1] - a[1]) * f,
        a[2] + (s * b[2] - a[2]) * f,
        a[3] + (s * b[3] - a[3]) * f,
    ])
}

fn normalize(q: [f32; 4]) -> [f32; 4] {
    let n = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    if n > 1e-8 {
        [q[0] / n, q[1] / n, q[2] / n, q[3] / n]
    } else {
        [0.0, 0.0, 0.0, 1.0]
    }
}

/// B-spline point at `u` (NURBS book A2.1 + A2.2, non-rational).
fn bspline<const N: usize>(p: usize, knots: &[f32], pts: &[[f32; N]], u: f32) -> [f32; N] {
    let n = pts.len() - 1;
    if n == 0 {
        return pts[0];
    }
    let span = find_span(n, p, u, knots);
    let mut basis = [0.0f32; 8];
    let mut left = [0.0f32; 8];
    let mut right = [0.0f32; 8];
    basis[0] = 1.0;
    for j in 1..=p {
        left[j] = u - knots[span + 1 - j];
        right[j] = knots[span + j] - u;
        let mut saved = 0.0;
        for r in 0..j {
            let den = right[r + 1] + left[j - r];
            let tmp = if den.abs() > 1e-12 { basis[r] / den } else { 0.0 };
            basis[r] = saved + right[r + 1] * tmp;
            saved = left[j - r] * tmp;
        }
        basis[j] = saved;
    }
    let mut out = [0.0f32; N];
    for i in 0..=p {
        let cp = &pts[(span - p + i).min(n)];
        for k in 0..N {
            out[k] += basis[i] * cp[k];
        }
    }
    out
}

fn find_span(n: usize, p: usize, u: f32, k: &[f32]) -> usize {
    if u >= k[n + 1] {
        return n;
    }
    if u <= k[p] {
        return p;
    }
    let (mut low, mut high) = (p, n + 1);
    let mut mid = (low + high) / 2;
    while u < k[mid] || u >= k[mid + 1] {
        if u < k[mid] {
            high = mid;
        } else {
            low = mid;
        }
        mid = (low + high) / 2;
    }
    mid
}

// ---- spline block reader -------------------------------------------------------

struct Reader<'a> {
    d: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn need(&self, n: usize) -> Result<(), HkxError> {
        if self.pos + n > self.d.len() {
            Err(HkxError::Parse(format!("spline data truncated at {}", self.pos)))
        } else {
            Ok(())
        }
    }
    fn u8(&mut self) -> Result<u8, HkxError> {
        self.need(1)?;
        self.pos += 1;
        Ok(self.d[self.pos - 1])
    }
    fn u16(&mut self) -> Result<u16, HkxError> {
        self.need(2)?;
        self.pos += 2;
        Ok(u16::from_le_bytes([self.d[self.pos - 2], self.d[self.pos - 1]]))
    }
    fn u32(&mut self) -> Result<u32, HkxError> {
        self.need(4)?;
        let v = u32::from_le_bytes(self.d[self.pos..self.pos + 4].try_into().unwrap());
        self.pos += 4;
        Ok(v)
    }
    fn f32(&mut self) -> Result<f32, HkxError> {
        Ok(f32::from_bits(self.u32()?))
    }
    fn align(&mut self, a: usize) {
        let r = self.pos % a;
        if r != 0 {
            self.pos += a - r;
        }
    }
}

const ROT_ALIGN: [usize; 6] = [4, 1, 2, 1, 2, 4];

fn read_vec(r: &mut Reader, quant: u8, flags: u8, default: f32) -> Result<Channel<3>, HkxError> {
    let spline = |i: usize| flags & (0x10 << i) != 0;
    let stat = |i: usize| flags & (0x01 << i) != 0;
    let mut base = [default; 3];
    if flags & 0xF0 != 0 {
        let num_items = r.u16()? as usize;
        let degree = r.u8()? as usize;
        let mut knots = Vec::with_capacity(num_items + degree + 2);
        for _ in 0..num_items + degree + 2 {
            knots.push(r.u8()? as f32);
        }
        r.align(4);
        let mut range = [(0.0f32, 0.0f32); 3];
        for i in 0..3 {
            if spline(i) {
                range[i] = (r.f32()?, r.f32()?);
            } else if stat(i) {
                base[i] = r.f32()?;
            }
        }
        let mut points = Vec::with_capacity(num_items + 1);
        for _ in 0..=num_items {
            let mut p = base;
            for i in 0..3 {
                if spline(i) {
                    let f = if quant == 0 { r.u8()? as f32 / 255.0 } else { r.u16()? as f32 / 65535.0 };
                    p[i] = range[i].0 + (range[i].1 - range[i].0) * f;
                }
            }
            points.push(p);
        }
        r.align(4);
        Ok(Channel::Spline { degree, knots, points })
    } else {
        for i in 0..3 {
            if stat(i) {
                base[i] = r.f32()?;
            }
        }
        r.align(4);
        Ok(Channel::Const(base))
    }
}

fn read_quat(r: &mut Reader, quant: u8) -> Result<[f32; 4], HkxError> {
    Ok(match quant {
        0 => polar32(r.u32()?),
        1 => {
            let mut v: u64 = 0;
            for k in 0..5 {
                v |= (r.u8()? as u64) << (8 * k);
            }
            three_comp40(v)
        }
        2 => {
            let x = r.u16()?;
            let y = r.u16()?;
            let z = r.u16()?;
            three_comp48(x, y, z)
        }
        3 => {
            let a = r.u8()?;
            let b = r.u8()?;
            let c = r.u8()?;
            three_comp24(a, b, c)
        }
        4 => straight16(r.u16()?),
        5 => [r.f32()?, r.f32()?, r.f32()?, r.f32()?],
        q => return Err(HkxError::Parse(format!("unknown rotation quantization {q}"))),
    })
}

/// Rebuild the dropped (largest) component and put it at `shift`.
fn insert_largest(c: [f32; 3], shift: usize, negative: bool) -> [f32; 4] {
    let mut w = (1.0 - c[0] * c[0] - c[1] * c[1] - c[2] * c[2]).max(0.0).sqrt();
    if negative {
        w = -w;
    }
    let mut out = [0.0; 4];
    let mut k = 0;
    for (i, o) in out.iter_mut().enumerate() {
        if i == shift {
            *o = w;
        } else {
            *o = c[k];
            k += 1;
        }
    }
    out
}

fn three_comp40(v: u64) -> [f32; 4] {
    const MASK: u64 = (1 << 12) - 1;
    const HALF: f32 = (MASK >> 1) as f32;
    const FRACTAL: f32 = 0.000_345_436;
    let c = [
        ((v & MASK) as f32 - HALF) * FRACTAL,
        (((v >> 12) & MASK) as f32 - HALF) * FRACTAL,
        (((v >> 24) & MASK) as f32 - HALF) * FRACTAL,
    ];
    insert_largest(c, ((v >> 36) & 3) as usize, (v >> 38) & 1 != 0)
}

fn three_comp48(x: u16, y: u16, z: u16) -> [f32; 4] {
    const MASK: u16 = (1 << 15) - 1;
    const HALF: f32 = (MASK >> 1) as f32;
    const FRACTAL: f32 = 0.000_043_161;
    let shift = (((y >> 14) & 2) | ((x >> 15) & 1)) as usize;
    let negative = (z >> 15) != 0;
    let c = [
        ((x & MASK) as f32 - HALF) * FRACTAL,
        ((y & MASK) as f32 - HALF) * FRACTAL,
        ((z & MASK) as f32 - HALF) * FRACTAL,
    ];
    insert_largest(c, shift, negative)
}

fn three_comp24(a: u8, b: u8, c: u8) -> [f32; 4] {
    // 3 × 7-bit components; the top bits of a/b carry the shift, c the sign.
    const MASK: u8 = (1 << 7) - 1;
    const HALF: f32 = (MASK >> 1) as f32;
    let fractal = 1.0 / (HALF * std::f32::consts::SQRT_2);
    let shift = (((b >> 6) & 2) | ((a >> 7) & 1)) as usize;
    let negative = (c >> 7) != 0;
    let v = [((a & MASK) as f32 - HALF) * fractal, ((b & MASK) as f32 - HALF) * fractal, ((c & MASK) as f32 - HALF) * fractal];
    insert_largest(v, shift, negative)
}

fn straight16(_v: u16) -> [f32; 4] {
    // Not used by Skyrim exports; treated as identity.
    [0.0, 0.0, 0.0, 1.0]
}

fn polar32(c: u32) -> [f32; 4] {
    use std::f32::consts::{FRAC_PI_2, FRAC_PI_4};
    const R_MASK: u32 = (1 << 10) - 1;
    const R_FRAC: f32 = 1.0 / R_MASK as f32;
    let phi_frac = FRAC_PI_2 / 511.0;
    let mut r = ((c >> 18) & R_MASK) as f32 * R_FRAC;
    r = 1.0 - r * r;
    let phi_theta = (c & 0x3FFFF) as f32;
    let mut phi = phi_theta.sqrt().floor();
    let mut theta = 0.0;
    if phi > 0.0 {
        theta = FRAC_PI_4 * (phi_theta - phi * phi) / phi;
        phi *= phi_frac;
    }
    let mag = (1.0 - r * r).max(0.0).sqrt();
    let mut q = [phi.sin() * theta.cos() * mag, phi.sin() * theta.sin() * mag, phi.cos() * mag, r];
    for (i, v) in q.iter_mut().enumerate() {
        if c & (0x1000_0000 << i) != 0 {
            *v = -*v;
        }
    }
    q
}

fn read_rot(r: &mut Reader, quant: u8, flags: u8) -> Result<Channel<4>, HkxError> {
    let align = *ROT_ALIGN.get(quant as usize).unwrap_or(&4);
    let ch = if flags & 0xF0 != 0 {
        let num_items = r.u16()? as usize;
        let degree = r.u8()? as usize;
        let mut knots = Vec::with_capacity(num_items + degree + 2);
        for _ in 0..num_items + degree + 2 {
            knots.push(r.u8()? as f32);
        }
        r.align(align);
        let mut points = Vec::with_capacity(num_items + 1);
        for _ in 0..=num_items {
            points.push(read_quat(r, quant)?);
        }
        // Havok evaluates the raw control points and normalizes; no
        // hemisphere fix-up (checked against an hkxcmd KF export).
        Channel::Spline { degree, knots, points }
    } else if flags & 0x0F != 0 {
        r.align(align);
        Channel::Const(read_quat(r, quant)?)
    } else {
        Channel::Const([0.0, 0.0, 0.0, 1.0])
    };
    r.align(4);
    Ok(ch)
}

fn read_spline_block(data: &[u8], offset: usize, tracks: usize, floats: usize) -> Result<Vec<TrackBlock>, HkxError> {
    let mut r = Reader { d: data, pos: offset };
    let mut masks = Vec::with_capacity(tracks);
    for _ in 0..tracks {
        masks.push([r.u8()?, r.u8()?, r.u8()?, r.u8()?]);
    }
    r.pos += floats;
    r.align(4);
    let mut out = Vec::with_capacity(tracks);
    for m in masks {
        let pos = read_vec(&mut r, m[0] & 3, m[1], 0.0)?;
        let rot = read_rot(&mut r, (m[0] >> 2) & 0xF, m[2])?;
        let scale = read_vec(&mut r, (m[0] >> 6) & 3, m[3], 1.0)?;
        out.push(TrackBlock { pos, rot, scale });
    }
    Ok(out)
}

pub fn read_animation(bytes: &[u8]) -> Result<Animation, HkxError> {
    let map = parse(bytes)?;
    animation_from_map(&map)
}

pub fn animation_from_map(map: &ClassMap<'_>) -> Result<Animation, HkxError> {
    let mut track_to_bone = Vec::new();
    for c in map.values() {
        if let Classes::hkaAnimationBinding(b) = c {
            track_to_bone = b.m_transformTrackToBoneIndices.clone();
            break;
        }
    }
    for c in map.values() {
        match c {
            Classes::hkaSplineCompressedAnimation(a) => {
                if a.m_endian != 0 {
                    return Err(HkxError::Parse("big-endian spline data".into()));
                }
                let tracks = a.parent.m_numberOfTransformTracks.max(0) as usize;
                let floats = a.parent.m_numberOfFloatTracks.max(0) as usize;
                let mut blocks = Vec::new();
                for &off in &a.m_blockOffsets {
                    blocks.push(read_spline_block(&a.m_data, off as usize, tracks, floats)?);
                }
                if blocks.is_empty() {
                    return Err(HkxError::Parse("spline animation without blocks".into()));
                }
                return Ok(Animation {
                    duration: a.parent.m_duration,
                    tracks,
                    track_to_bone,
                    data: Data::Spline { blocks, block_duration: a.m_blockDuration, frame_duration: a.m_frameDuration },
                });
            }
            Classes::hkaInterleavedUncompressedAnimation(a) => {
                let tracks = a.parent.m_numberOfTransformTracks.max(0) as usize;
                let transforms: Vec<Xform> = a.m_transforms.iter().map(qs).collect();
                return Ok(Animation {
                    duration: a.parent.m_duration,
                    tracks,
                    track_to_bone,
                    data: Data::Interleaved { frames: transforms.len() / tracks.max(1), transforms },
                });
            }
            _ => {}
        }
    }
    Err(HkxError::NoAnimation)
}

// ---- pose ----------------------------------------------------------------------

pub fn quat_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}

pub fn quat_rotate(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let u = [q[0], q[1], q[2]];
    let s = q[3];
    let dot = u[0] * v[0] + u[1] * v[1] + u[2] * v[2];
    let uu = u[0] * u[0] + u[1] * u[1] + u[2] * u[2];
    let cross = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
    let mut out = [0.0; 3];
    for i in 0..3 {
        out[i] = 2.0 * dot * u[i] + (s * s - uu) * v[i] + 2.0 * s * cross[i];
    }
    out
}

/// Parent ∘ child.
pub fn compose(parent: &Xform, child: &Xform) -> Xform {
    let scaled = [child.t[0] * parent.s[0], child.t[1] * parent.s[1], child.t[2] * parent.s[2]];
    let rt = quat_rotate(parent.r, scaled);
    Xform {
        t: [parent.t[0] + rt[0], parent.t[1] + rt[1], parent.t[2] + rt[2]],
        r: normalize(quat_mul(parent.r, child.r)),
        s: [parent.s[0] * child.s[0], parent.s[1] * child.s[1], parent.s[2] * child.s[2]],
    }
}

/// Local pose of every skeleton bone at `time`: the clip's tracks over the
/// reference pose.
pub fn local_pose(skel: &Skeleton, anim: &Animation, time: f32) -> Vec<Xform> {
    let mut pose = skel.reference.clone();
    for (track, x) in anim.sample(time).into_iter().enumerate() {
        let b = anim.bone_of_track(track);
        if b < pose.len() {
            pose[b] = x;
        }
    }
    pose
}

/// Model-space transforms from a local pose (parents precede children in
/// Havok skeletons, but do not rely on it).
pub fn model_pose(skel: &Skeleton, local: &[Xform]) -> Vec<Xform> {
    let n = local.len();
    let mut out: Vec<Option<Xform>> = vec![None; n];
    fn solve(i: usize, skel: &Skeleton, local: &[Xform], out: &mut Vec<Option<Xform>>, depth: usize) -> Xform {
        if let Some(x) = out[i] {
            return x;
        }
        let p = skel.parents.get(i).copied().unwrap_or(-1);
        let x = if p >= 0 && (p as usize) < local.len() && depth < 512 {
            let px = solve(p as usize, skel, local, out, depth + 1);
            compose(&px, &local[i])
        } else {
            local[i]
        };
        out[i] = Some(x);
        x
    }
    (0..n).map(|i| solve(i, skel, local, &mut out, 0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_comp40_identity_and_axes() {
        // Middle values (2047) and the largest component, w, at shift 3.
        let mid = 2047u64 | (2047 << 12) | (2047 << 24) | (3 << 36);
        let q = three_comp40(mid);
        assert!((q[3] - 1.0).abs() < 1e-5 && q[0].abs() < 1e-3);
        // Shift 0 puts the rebuilt component first.
        let q = three_comp40(2047u64 | (2047 << 12) | (2047 << 24));
        assert!((q[0] - 1.0).abs() < 1e-5);
    }

    #[test]
    fn bspline_degree1_is_linear() {
        let knots = vec![0.0, 0.0, 10.0, 10.0];
        let pts = vec![[0.0f32], [10.0]];
        assert!((bspline(1, &knots, &pts, 2.5)[0] - 2.5).abs() < 1e-5);
        assert!((bspline(1, &knots, &pts, 10.0)[0] - 10.0).abs() < 1e-5);
    }

    #[test]
    fn bspline_degree3_clamped_ends() {
        let knots = vec![0.0, 0.0, 0.0, 0.0, 30.0, 30.0, 30.0, 30.0];
        let pts = vec![[1.0f32], [2.0], [3.0], [4.0]];
        assert!((bspline(3, &knots, &pts, 0.0)[0] - 1.0).abs() < 1e-5);
        assert!((bspline(3, &knots, &pts, 30.0)[0] - 4.0).abs() < 1e-5);
        assert!((bspline(3, &knots, &pts, 15.0)[0] - 2.5).abs() < 1e-5);
    }
}
