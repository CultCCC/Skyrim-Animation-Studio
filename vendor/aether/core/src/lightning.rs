//! The procedural lightning bolt generator, RE'd from SkyrimSE 1.5.97.
//!
//! `BSProceduralLightningController` stores a recipe; the game builds the
//! mesh at strike time on a tasklet. Every rule here carries the address
//! it came from (docs/knowledge/particle-runtime.md has the write-up; an
//! adversarial verify pass re-derived all of it from fresh decompiles):
//!
//! - Element tree (`0x140D67B70`): child count uniform in
//!   [max(0, nB−nBV), nB+nBV]; children halve nB and nBV, drop one
//!   subdivision level, and attach at the midpoint of a parent path
//!   point in the parent's FIRST half.
//! - Per element (`0x140D66EA0`): N = 1<<subdiv segments,
//!   L = Length·0.5^d + LengthVar·0.25^d·U with U ∈ [−1, 1); the path
//!   runs along the target node's LOCAL +Y from its base.
//! - Jitter (`0x140D67C80`): recursive dyadic hat displacement in XZ —
//!   a random direction θ ∈ [0, 2π), amplitude ArcOffset·0.5^depth
//!   halving per recursion level, weighted by a triangle hat that is 0
//!   at the interval ends. Interval ENDPOINTS are never displaced, so
//!   the bolt's start and end sit exactly on the local Y axis.
//! - Verts (`0x140D67104`): 4 per path point, an axis-aligned cross —
//!   NO camera anywhere; billboarding, if any, comes from the shader.
//!   halfWidth = 0.5·Width·ChildWidthMult^depth. u = 0|1 per ribbon
//!   side, v = i/N. Vertex colour rgb = 1; alpha fades 1−(i/N)² when
//!   the element's fade flag applies, first point 0 for the root and 1
//!   for branches, last point always 0 (soft tip).
//!
//! The game's RNG is a process-wide time-seeded MT19937 — bolts are NOT
//! reproducible from the file — so any decent generator is faithful as
//! long as the RANGES match. The one the sim already uses does.

use crate::mesh::MeshVertex;
use crate::sim::Rng;

/// The streamed recipe (the controller's non-interpolator fields).
#[derive(Debug, Clone, PartialEq)]
pub struct LightningParams {
    /// log2 of the root segment count. The runtime refuses 0 and > 12
    /// outright (`0x140D66EB8`) — no bolt at all.
    pub subdivisions: u16,
    pub num_branches: u16,
    pub num_branches_variation: u16,
    pub length: f32,
    pub length_variation: f32,
    pub width: f32,
    pub child_width_mult: f32,
    pub arc_offset: f32,
    pub fade_main_bolt: bool,
    pub fade_child_bolts: bool,
}

impl Default for LightningParams {
    fn default() -> Self {
        LightningParams {
            subdivisions: 6,
            num_branches: 1,
            num_branches_variation: 1,
            length: 512.0,
            length_variation: 30.0,
            width: 16.0,
            child_width_mult: 0.75,
            arc_offset: 20.0,
            fade_main_bolt: true,
            fade_child_bolts: true,
        }
    }
}

/// One element's bookkeeping, kept so the ArcOffset animation can
/// re-scale the jitter without re-rolling the strike (the game's
/// animate path multiplies the stored offsets by newArc/oldArc and
/// re-uploads positions only — `0x140D5FAE7`).
#[derive(Debug, Clone)]
struct Element {
    /// Index into `Bolt::points` of this element's first path point.
    first_point: usize,
    /// Path points = segments + 1.
    points: usize,
    depth: u16,
    half_width: f32,
}

/// One path point: centreline + its jitter, stored separately so the
/// arc ratio can re-scale the jitter later.
#[derive(Debug, Clone, Copy)]
struct PathPoint {
    base: [f32; 3],
    /// XZ displacement at build-time arc offset.
    jitter: [f32; 2],
    /// 0..1 along the element, for UV v and the alpha fade.
    t: f32,
    alpha: f32,
}

/// A generated strike: fixed topology, rebuildable positions.
#[derive(Debug, Clone)]
pub struct Bolt {
    elements: Vec<Element>,
    points: Vec<PathPoint>,
    /// The arc offset the jitter was rolled at, for ratio scaling.
    built_arc: f32,
    pub indices: Vec<u16>,
}

impl Bolt {
    /// Vertices at `arc_now`, in the target node's local frame. The
    /// jitter scales by `arc_now / built_arc` exactly like the game's
    /// animate path; everything else is fixed until the next strike.
    pub fn vertices(&self, arc_now: f32) -> Vec<MeshVertex> {
        let ratio = if self.built_arc.abs() > 1e-6 {
            arc_now / self.built_arc
        } else {
            // The game skips the scale when the old arc is 0
            // (`0x140D5FAEF`) — so do we.
            1.0
        };
        let mut out = Vec::with_capacity(self.points.len() * 4);
        for e in &self.elements {
            for p in &self.points[e.first_point..e.first_point + e.points] {
                let x = p.base[0] + p.jitter[0] * ratio;
                let z = p.base[2] + p.jitter[1] * ratio;
                let y = p.base[1];
                let hw = e.half_width;
                let colour = [1.0, 1.0, 1.0, p.alpha];
                // The cross: −X, +X, +Z, −Z (`0x140D6758D`). Normals are
                // not part of the game's vertex stream; a unit +Y keeps
                // the effect path's math finite.
                for (dx, dz, u) in [
                    (-hw, 0.0, 0.0f32),
                    (hw, 0.0, 1.0),
                    (0.0, hw, 0.0),
                    (0.0, -hw, 1.0),
                ] {
                    out.push(MeshVertex {
                        position: [x + dx, y, z + dz],
                        normal: [0.0, 1.0, 0.0],
                        uv: [u, p.t],
                        color: colour,
                    });
                }
            }
        }
        out
    }

    pub fn vertex_count(&self) -> usize {
        self.points.len() * 4
    }
}

/// The dyadic hat displacement (`0x140D67C80`): pick a direction, push
/// every INTERIOR point of the interval by a triangle hat peaking at
/// the middle, halve the amplitude, recurse on both halves down to
/// span 2. Written exactly to the decompile, including the tail
/// iteration on the second half.
fn displace(points: &mut [PathPoint], lo: usize, hi: usize, mut amp: f32, rng: &mut Rng) {
    let mut lo = lo;
    let mut hi = hi;
    loop {
        if hi - lo < 2 {
            return;
        }
        let theta = rng.unit() * std::f32::consts::TAU;
        let (sin, cos) = theta.sin_cos();
        let (dx, dz) = (cos * amp, sin * amp);
        let span = (hi - lo) as f32;
        for i in lo + 1..hi {
            let local = (i - lo) as f32 / span;
            let hat = 1.0 - (local - 0.5).abs() * 2.0;
            points[i].jitter[0] += dx * hat;
            points[i].jitter[1] += dz * hat;
        }
        amp *= 0.5;
        let mid = (lo + hi) / 2;
        displace(points, lo, mid, amp, rng);
        // Tail-iterate the second half, like the original.
        lo = mid;
    }
}

/// Roll one strike. Returns `None` when the subdivision gate refuses
/// (0 or > 12 — the game produces NO bolt for such a file, ever).
pub fn strike(params: &LightningParams, rng: &mut Rng) -> Option<Bolt> {
    if params.subdivisions == 0 || params.subdivisions > 12 {
        return None;
    }
    let mut bolt = Bolt {
        elements: Vec::new(),
        points: Vec::new(),
        built_arc: params.arc_offset,
        indices: Vec::new(),
    };
    // (parent element index or None, subdiv, nB, nBV, depth, base)
    let mut queue: Vec<(Option<usize>, u16, u16, u16, u16, [f32; 3])> = vec![(
        None,
        params.subdivisions,
        params.num_branches,
        params.num_branches_variation,
        0,
        [0.0; 3],
    )];
    // Depth-first like the recursive original; order only affects which
    // random numbers land where, and the RNG is not file-seeded anyway.
    while let Some((parent, subdiv, nb, nbv, depth, base)) = queue.pop() {
        let n = 1usize << subdiv;
        let d = depth as i32;
        let u = rng.unit() * 2.0 - 1.0;
        let length = params.length * 0.5f32.powi(d) + params.length_variation * 0.25f32.powi(d) * u;
        let half_width = 0.5 * params.width * params.child_width_mult.powi(d);
        let first_point = bolt.points.len();
        let first_vertex = first_point * 4;
        let is_root = parent.is_none();
        let fade = (params.fade_main_bolt && is_root) || (params.fade_child_bolts && !is_root);

        for i in 0..=n {
            let t = i as f32 / n as f32;
            let alpha = if i == 0 {
                // Root starts invisible, a branch starts solid — it
                // grows out of a visible parent point (`0x140D677F0`).
                if is_root { 0.0 } else { 1.0 }
            } else if i == n {
                0.0
            } else if fade {
                1.0 - t * t
            } else {
                1.0
            };
            bolt.points.push(PathPoint {
                base: [base[0], base[1] + length * t, base[2]],
                jitter: [0.0, 0.0],
                t,
                alpha,
            });
        }
        displace(
            &mut bolt.points[first_point..first_point + n + 1],
            0,
            n,
            params.arc_offset * 0.5f32.powi(d),
            rng,
        );

        // The crossed double ribbon (`0x140D67104`): per segment
        // b = 4i — (b, b+1, b+4), (b+1, b+5, b+4) then the same for
        // the perpendicular pair at +2/+3.
        for i in 0..n {
            let b = (first_vertex + i * 4) as u16;
            for tri in [
                [b, b + 1, b + 4],
                [b + 1, b + 5, b + 4],
                [b + 2, b + 3, b + 6],
                [b + 3, b + 7, b + 6],
            ] {
                bolt.indices.extend_from_slice(&tri);
            }
        }

        let element_index = bolt.elements.len();
        bolt.elements.push(Element {
            first_point,
            points: n + 1,
            depth,
            half_width,
        });

        // Children: uniform count in [max(0, nB−nBV), nB+nBV], forking
        // from the FIRST half of this element's path (`0x140D67527`).
        // Subdiv-0 parents may not fork — the original indexes out of
        // bounds there (1 << 255 masked to 1<<31); vanilla content
        // never reaches it, and we refuse instead of reproducing UB.
        let lo = nb.saturating_sub(nbv);
        let hi = nb + nbv;
        let children = lo + (rng.unit() * (hi - lo + 1) as f32) as u16;
        let children = children.min(hi);
        if subdiv >= 1 && children > 0 && subdiv > 1 {
            let half = 1usize << (subdiv - 1);
            for _ in 0..children {
                let k = (rng.unit() * half as f32) as usize % half;
                let p = &bolt.points[first_point + k];
                let child_base = [
                    p.base[0] + p.jitter[0],
                    p.base[1],
                    p.base[2] + p.jitter[1],
                ];
                queue.push((
                    Some(element_index),
                    if subdiv <= 1 { 0 } else { subdiv - 1 },
                    nb >> 1,
                    nbv >> 1,
                    depth + 1,
                    child_base,
                ));
            }
        }
        // A u16 index buffer caps the strike the same way the game's
        // u16 counts do; stop growing rather than wrapping.
        if bolt.points.len() * 4 > u16::MAX as usize - 64 {
            break;
        }
    }
    Some(bolt)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(subdiv: u16, nb: u16, nbv: u16) -> LightningParams {
        LightningParams {
            subdivisions: subdiv,
            num_branches: nb,
            num_branches_variation: nbv,
            ..LightningParams::default()
        }
    }

    /// The subdivision gate: 0 and 13 produce NO bolt, exactly like the
    /// runtime's `(s-1) > 11` refusal — not a clamped one.
    #[test]
    fn the_subdivision_gate_refuses_instead_of_clamping() {
        let mut rng = Rng::new(7);
        assert!(strike(&params(0, 0, 0), &mut rng).is_none());
        assert!(strike(&params(13, 0, 0), &mut rng).is_none());
        assert!(strike(&params(1, 0, 0), &mut rng).is_some());
        assert!(strike(&params(12, 0, 0), &mut rng).is_some());
    }

    /// Counts are structural: 4·(N+1) vertices and 4·N triangles per
    /// element — the exact totals the game's topology pass accumulates.
    #[test]
    fn vertex_and_triangle_counts_match_the_games_arithmetic() {
        let mut rng = Rng::new(1);
        let bolt = strike(&params(6, 0, 0), &mut rng).unwrap();
        let n = 1 << 6;
        assert_eq!(bolt.vertex_count(), 4 * (n + 1));
        assert_eq!(bolt.indices.len(), 3 * 4 * n);
        let verts = bolt.vertices(20.0);
        assert_eq!(verts.len(), bolt.vertex_count());
        // Every index in range.
        assert!(bolt.indices.iter().all(|i| (*i as usize) < verts.len()));
    }

    /// **The endpoints sit exactly on the local Y axis.** The fractal
    /// displacement never touches interval ends; a bolt that drifts at
    /// its root or tip would visibly detach from what it strikes.
    #[test]
    fn the_root_bolts_endpoints_are_never_displaced() {
        for seed in 1..20u32 {
            let mut rng = Rng::new(seed);
            let bolt = strike(&params(6, 0, 0), &mut rng).unwrap();
            let verts = bolt.vertices(45.0);
            // First path point's 4 verts straddle x = 0 / z = 0.
            assert!((verts[0].position[0] + verts[1].position[0]).abs() < 1e-4);
            assert!((verts[2].position[2] + verts[3].position[2]).abs() < 1e-4);
            // Last path point too, at y = L.
            let last = verts.len() - 4;
            assert!(
                (verts[last].position[0] + verts[last + 1].position[0]).abs() < 1e-4,
                "tip drifted off axis"
            );
            // And SOME interior point is displaced, or there is no bolt
            // to speak of.
            let mid = verts.len() / 2;
            let displaced = (0..verts.len() / 4).any(|i| {
                let v = &verts[i * 4];
                (v.position[0] + bolt.elements[0].half_width).abs() > 1e-3
            });
            assert!(displaced || mid == 0, "no jitter anywhere");
        }
    }

    /// The alpha law: root starts at 0, tip always 0, interior follows
    /// 1 − t² when fading. A branch STARTS solid.
    #[test]
    fn alpha_fades_quadratically_and_the_tip_is_always_soft() {
        let mut rng = Rng::new(3);
        let bolt = strike(&params(4, 0, 0), &mut rng).unwrap();
        let verts = bolt.vertices(20.0);
        let n = 16usize;
        assert_eq!(verts[0].color[3], 0.0, "root starts visible");
        assert_eq!(verts[verts.len() - 1].color[3], 0.0, "tip is hard");
        let i = n / 2;
        let t = i as f32 / n as f32;
        assert!(
            (verts[i * 4].color[3] - (1.0 - t * t)).abs() < 1e-5,
            "interior alpha is not 1 − t²"
        );
    }

    /// ArcOffset animation scales the jitter WITHOUT re-rolling: the
    /// same strike at twice the arc doubles every displacement, and the
    /// centreline (y, endpoints) stays put — the game's ratio path.
    #[test]
    fn arc_animation_scales_jitter_without_a_new_roll() {
        let mut rng = Rng::new(11);
        let bolt = strike(&params(6, 0, 0), &mut rng).unwrap();
        let a = bolt.vertices(20.0);
        let b = bolt.vertices(40.0);
        let hw = bolt.elements[0].half_width;
        let mut any_moved = false;
        for i in 0..a.len() / 4 {
            let (av, bv) = (&a[i * 4], &b[i * 4]);
            // Centre = vertex + hw on x (v0 is the −X corner).
            let (acx, bcx) = (av.position[0] + hw, bv.position[0] + hw);
            assert!(
                (bcx - acx * 2.0).abs() < 1e-3 || acx.abs() < 1e-5,
                "jitter did not scale by the ratio"
            );
            assert_eq!(av.position[1], bv.position[1], "y moved");
            if acx.abs() > 1e-3 {
                any_moved = true;
            }
        }
        assert!(any_moved);
    }

    /// The branch-count window is the game's: uniform in
    /// [max(0, nB−nBV), nB+nBV], children halving both counts.
    #[test]
    fn branch_counts_stay_inside_the_uniform_window() {
        let mut totals = Vec::new();
        for seed in 1..40u32 {
            let mut rng = Rng::new(seed);
            let bolt = strike(&params(5, 2, 1), &mut rng).unwrap();
            // Elements beyond the root are branches; the root may have
            // 1..=3 children, each child (nB 1, nBV 0) exactly 1, etc.
            totals.push(bolt.elements.len() - 1);
        }
        assert!(totals.iter().any(|c| *c > 0), "branches never happen");
        // nB 2, nBV 1 → root children ∈ [1, 3]: never zero.
        assert!(
            totals.iter().all(|c| *c >= 1),
            "a root rolled below the window's floor"
        );
    }
}
