//! Spawn points of a Pattern, ported from the DLL's Positioning.cpp.
//! Caster space: +X right, +Y forward (the cast direction), +Z up; the
//! caster looks straight ahead (no pitch), the centre is at the origin.

use crate::model::Pattern;

pub type V3 = [f32; 3];

fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn mul(a: V3, s: f32) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn len(a: V3) -> f32 {
    dot(a, a).sqrt()
}
fn unit(a: V3) -> V3 {
    let l = len(a);
    if l > 0.0 {
        mul(a, 1.0 / l)
    } else {
        a
    }
}

/// The figure plane: right and up from the cast direction, as `Plane` does.
pub struct Plane {
    pub start: V3,
    pub right: V3,
    pub up: V3,
    pub forward: V3,
}

impl Plane {
    pub fn new(start: V3, cast_dir: V3) -> Plane {
        let mut right = unit(cross([0.0, 0.0, -1.0], cast_dir));
        if dot(right, right) == 0.0 {
            right = [1.0, 0.0, 0.0];
        }
        let up = cross(right, cast_dir);
        let forward = unit(cross(up, right));
        Plane { start, right, up, forward }
    }
}

pub struct Points {
    pub points: Vec<V3>,
    /// Figure centre (origin + posOffset).
    pub center: V3,
    /// The (normal) cast direction of the plane.
    pub normal: V3,
}

/// Spawn positions. `origin_z` lifts the centre to the origin node height
/// (the DLL takes the node's world position; the preview approximates it).
pub fn points(p: &Pattern, origin_z: f32) -> Points {
    let f = &p.figure;
    let count = f.count.unwrap_or(1).max(1) as usize;
    let size = if f.shape == "Single" { 0.0 } else { f.size.unwrap_or(0.0) as f32 };
    let n = p.normal.map(|n| [n[0] as f32, n[1] as f32, n[2] as f32]).unwrap_or([0.0, 1.0, 0.0]);
    let off = p.pos_offset.map(|o| [o[0] as f32, o[1] as f32, o[2] as f32]).unwrap_or([0.0; 3]);
    let center = add([0.0, 0.0, origin_z], off);
    let plane = Plane::new(center, n);
    let alpha = p.plane_rotate.unwrap_or(0.0) as f32 * std::f32::consts::PI / 180.0;
    let axis = unit(n);
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let mut pt = position(&f.shape, &plane, count, size, i);
        if alpha != 0.0 {
            pt = rotate_about(pt, alpha, center, axis);
        }
        out.push(pt);
    }
    Points { points: out, center, normal: axis }
}

fn rotate_about(p: V3, a: f32, o: V3, axis: V3) -> V3 {
    // Rodrigues around `axis` through `o`.
    let v = sub(p, o);
    let (s, c) = a.sin_cos();
    let r = add(add(mul(v, c), mul(cross(axis, v), s)), mul(axis, dot(axis, v) * (1.0 - c)));
    add(o, r)
}

fn position(shape: &str, pl: &Plane, count: usize, size: f32, ind: usize) -> V3 {
    let cf = count as f32;
    let fi = ind as f32;
    let at = |r: f32, a: f32| add(pl.start, mul(add(mul(pl.right, a.cos()), mul(pl.up, a.sin())), r));
    match shape {
        "Line" => {
            if count == 1 {
                return pl.start;
            }
            let from = sub(pl.start, mul(pl.right, size * 0.5));
            add(from, mul(pl.right, size / (cf - 1.0) * fi))
        }
        "Circle" => at(size, 2.0 * std::f32::consts::PI / cf * fi),
        "HalfCircle" => {
            if count == 1 {
                return pl.start;
            }
            at(size, std::f32::consts::PI / (cf - 1.0) * fi)
        }
        "FillSquare" => {
            if count == 1 {
                return pl.start;
            }
            let m = (cf.sqrt()) as usize;
            let rest = count - m * m;
            let has_right = rest >= m;
            let has_up = rest != 0 && rest != m;
            let w = if has_right { m + 1 } else { m };
            let h = if has_up { m + 1 } else { m };
            let dx = size / (w as f32 - 1.0).max(1e-6);
            let dy = if h == 1 { 0.0 } else { size / (h as f32 - 1.0) };
            let ind = ind % count;
            if ind < w * m {
                let x = (ind % w) as f32;
                let y = (ind / w) as f32;
                let from = sub(pl.start, mul(add(pl.right, pl.up), size * 0.5));
                add(add(from, mul(pl.right, dx * x)), mul(pl.up, dy * y))
            } else {
                let ind = ind - w * m;
                let up_size = if rest >= m { rest - m } else { rest } as f32;
                let from = sub(sub(pl.start, mul(pl.right, (up_size - 1.0) * 0.5 * dx)), mul(pl.up, size * 0.5 - dy * m as f32));
                add(from, mul(pl.right, dx * ind as f32))
            }
        }
        "FillCircle" => {
            let c = size / cf.sqrt();
            at(c * fi.sqrt(), 2.399_963_2 * fi)
        }
        "FillHalfCircle" => {
            let c = size / cf.sqrt();
            let pi = std::f32::consts::PI;
            let mut a = 0.5 * 2.399_963_2 * fi;
            while a >= 2.0 * pi {
                a -= 2.0 * pi;
            }
            if a >= pi {
                a -= pi;
            }
            at(c * fi.sqrt(), a)
        }
        "Sphere" | "HalfSphere" | "Cylinder" => {
            if count == 1 {
                return pl.start;
            }
            let phi = 3.883_222_f32;
            let theta = phi * fi;
            let (x, y, z) = match shape {
                "Sphere" => {
                    let y = 1.0 - (fi / (cf - 1.0)) * 2.0;
                    let r = (1.0 - y * y).max(0.0).sqrt();
                    (theta.cos() * r, y, theta.sin() * r)
                }
                "HalfSphere" => {
                    let z = 1.0 - fi / (cf - 1.0);
                    let r = (1.0 - z * z).max(0.0).sqrt();
                    (theta.cos() * r, theta.sin() * r, z)
                }
                _ => {
                    let y = 1.0 - (fi / (cf - 1.0)) * 2.0;
                    (theta.cos(), y, theta.sin())
                }
            };
            // forward_dir = up × right (unit).
            add(pl.start, mul(add(add(mul(pl.right, x), mul(pl.up, z)), mul(pl.forward, y)), size))
        }
        _ => pl.start,
    }
}

/// Launch direction of each point for a spawn group `rotation`.
pub fn directions(pts: &Points, rotation: &str, sight: V3) -> Vec<V3> {
    pts.points
        .iter()
        .map(|p| match rotation {
            "ToCenter" => unit(sub(pts.center, *p)),
            "FromCenter" => unit(sub(*p, pts.center)),
            "ToSight" => unit(sub(sight, *p)),
            // ToTarget: aims at the homing target — shown toward the sight point.
            "ToTarget" => unit(sub(sight, *p)),
            _ => [0.0, 1.0, 0.0],
        })
        .map(|d| if len(d) < 1e-6 { [0.0, 1.0, 0.0] } else { d })
        .collect()
}
