//! EFSH particles: the sprites an effect shader births over the actor's body
//! (flames off a burning NPC, sparks off a shocked one). Stateless: births
//! are counted on a fixed 30 Hz grid from the effect's start, and every
//! particle's surface point, life, speed and atlas cell come from a hash of
//! its birth slot, so a scrubbed frame looks the same every time.

use crate::paired::Skinned;
use crate::pie::ActiveShader;
use crate::render::{FxBatch, FxVertex};

const STEP: f32 = 1.0 / 30.0;
/// EFSH birth ratios read as births per second per this much surface
/// (square units): an estimate, ~10× the raw number on an armoured NPC.
const AREA_UNIT: f32 = 5000.0;
/// Upper bound per shader per frame.
const MAX_SPRITES: usize = 600;

fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^ (x >> 16)
}

/// Uniform 0..1 from a seed and a stream number.
fn rnd(seed: u32, k: u32) -> f32 {
    (hash(seed ^ k.wrapping_mul(0x9e37_79b9)) >> 8) as f32 / (1u32 << 24) as f32
}

pub fn particles(shaders: &[ActiveShader], meshes: &[&Skinned], right: [f32; 3], up: [f32; 3]) -> Vec<FxBatch> {
    let mut out = Vec::new();
    let total: usize = meshes.iter().map(|m| m.verts.len()).sum();
    if total == 0 {
        return out;
    }
    for a in shaders {
        let Some(p) = &a.shader.particles else { continue };
        let skin_only = a.shader.skin_only;
        let pool: Vec<&Skinned> = meshes.iter().copied().filter(|m| !skin_only || m.skin).collect();
        let pool_total: usize = pool.iter().map(|m| m.verts.len()).sum();
        if pool_total == 0 {
            continue;
        }
        // Body surface, posed.
        let area: f32 = pool
            .iter()
            .map(|m| {
                m.indices
                    .chunks_exact(3)
                    .map(|t| {
                        let (p0, p1, p2) = (m.verts[t[0] as usize].pos, m.verts[t[1] as usize].pos, m.verts[t[2] as usize].pos);
                        let (u, v) = ([p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]], [p2[0] - p0[0], p2[1] - p0[1], p2[2] - p0[2]]);
                        let c = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
                        0.5 * (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt()
                    })
                    .sum::<f32>()
            })
            .sum();
        let per_area = (area / AREA_UNIT).max(1.0);
        let removed_at = a.ended.map(|e| a.age - e);
        let longest = p.life + p.life_var.abs();
        let first = (((a.age - longest).max(0.0)) / STEP).floor() as u32;
        let last = (a.age / STEP).floor() as u32;
        let grey = !p.palette.is_empty() && (p.grey_color || p.grey_alpha);
        let mode = if grey { (p.grey_color as u32 | ((p.grey_alpha as u32) << 1)) as f32 } else { 0.0 };
        let mut batch = FxBatch { verts: Vec::new(), indices: Vec::new(), texture: p.texture.clone(), palette: if grey { p.palette.clone() } else { String::new() } };
        let cells = p.atlas[0] * p.atlas[1];
        'steps: for step in first..=last {
            let born = step as f32 * STEP;
            let rate = p.rate(born, removed_at.is_some_and(|r| born >= r)) * per_area;
            let slot = a.seed.wrapping_mul(0x01000193) ^ step;
            // Whole births plus a hashed chance for the fraction.
            let want = rate * STEP;
            let n = want.floor() as u32 + (rnd(slot, 0) < want.fract()) as u32;
            for j in 0..n {
                let seed = hash(slot.wrapping_add(j.wrapping_mul(0x632b_e5ab)));
                let life = (p.life + p.life_var * (rnd(seed, 1) * 2.0 - 1.0)).max(0.01);
                let age = a.age - born - rnd(seed, 2) * STEP;
                if age < 0.0 || age > life {
                    continue;
                }
                // A surface point: any vertex of the body, by count.
                let mut pick = (rnd(seed, 3) * pool_total as f32) as usize % pool_total;
                let mut vert = None;
                for m in &pool {
                    if pick < m.verts.len() {
                        vert = Some(m.verts[pick]);
                        break;
                    }
                    pick -= m.verts.len();
                }
                let Some(v) = vert else { continue };
                let sn = p.speed_normal + p.speed_normal_var * (rnd(seed, 4) * 2.0 - 1.0);
                let along = sn * age + 0.5 * p.accel_normal * age * age;
                let mut c = [0.0f32; 3];
                for k in 0..3 {
                    c[k] = v.pos[k] + v.normal[k] * along + p.velocity[k] * age + 0.5 * p.accel[k] * age * age;
                }
                let x = age / life;
                let h = p.size(x) * 0.5;
                let (rgb, alpha) = p.color(x);
                if h <= 0.0 || alpha <= 0.0 {
                    continue;
                }
                let rot = (p.rotation[0] + p.rotation[1] * (rnd(seed, 5) * 2.0 - 1.0) + (p.rotation[2] + p.rotation[3] * (rnd(seed, 6) * 2.0 - 1.0)) * age).to_radians();
                let (sa, ca) = rot.sin_cos();
                let rx = [right[0] * ca + up[0] * sa, right[1] * ca + up[1] * sa, right[2] * ca + up[2] * sa];
                let ry = [up[0] * ca - right[0] * sa, up[1] * ca - right[1] * sa, up[2] * ca - right[2] * sa];
                let cell = (rnd(seed, 7) * cells as f32) as u32 % cells.max(1);
                let (cu, cv) = (cell % p.atlas[0], cell / p.atlas[0]);
                let (du, dv) = (1.0 / p.atlas[0] as f32, 1.0 / p.atlas[1] as f32);
                let (u0, v0, u1, v1) = (cu as f32 * du, cv as f32 * dv, (cu + 1) as f32 * du, (cv + 1) as f32 * dv);
                let k = p.color_scale;
                let col = [rgb[0] * k, rgb[1] * k, rgb[2] * k, alpha];
                let base = batch.verts.len() as u32;
                for (sx, sy, u, v) in [(-1.0, -1.0, u0, v1), (1.0, -1.0, u1, v1), (1.0, 1.0, u1, v0), (-1.0, 1.0, u0, v0)] {
                    let pos = [c[0] + (rx[0] * sx + ry[0] * sy) * h, c[1] + (rx[1] * sx + ry[1] * sy) * h, c[2] + (rx[2] * sx + ry[2] * sy) * h];
                    batch.verts.push(FxVertex { pos, uv: [u, v], color: col, additive: 1.0, mode, grey: [1.0, 1.0, 1.0] });
                }
                batch.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
                if batch.verts.len() / 4 >= MAX_SPRITES {
                    break 'steps;
                }
            }
        }
        if std::env::var_os("SDS_FX_DEBUG").is_some() {
            eprintln!("FXEFSH {} age {:.2} sprites {} rate {:.1} area {:.0} tex {} pal {}", a.shader.edid, a.age, batch.verts.len() / 4, p.rate(a.age, false) * per_area, area, p.texture, p.palette);
        }
        if !batch.indices.is_empty() {
            out.push(batch);
        }
    }
    out
}
