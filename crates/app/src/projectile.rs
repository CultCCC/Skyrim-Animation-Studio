//! Spell projectiles. A cast at the target (`pie::Shot`) is launched from the
//! casting hand's magic node at the target dummy's chest at the PROJ speed.
//! Missiles and arrows fly straight and fall with their gravity (×9.81 m/s²):
//! they burst on the target or wherever they reach the ground first, so a
//! projectile with gravity 10000 drops at the caster's feet. Lobbers are aimed
//! on an arc that lands on the target. Cones fly straight and widen with
//! their cone spread. Beams, flames and barriers hit at once. On impact the
//! explosion (EXPL) plays there and the effect's hit art plays on the target
//! if it was hit or inside the explosion. Sounds: the PROJ flight loop until
//! the end of the flight, the EXPL sound and the MGEF hit sound on impact.

use crate::app::StudioApp;
use crate::pie::{Cue, Shot};
use sds_anno::sdscombat::Path;
use aether_core::mesh::Transform;
use sds_records::fx::ProjKind;
use sds_records::FormKey;

/// Havok's 9.81 m/s² in game units (1 m = 69.99 units).
const GRAVITY: f32 = 9.81 * 69.99;
/// Where spells are aimed on the target dummy.
const AIM_Z: f32 = 90.0;
/// The target dummy as a collision cylinder.
const TARGET_RADIUS: f32 = 30.0;
const TARGET_HEIGHT: f32 = 140.0;
/// How long the explosion art stays (EXPL has no duration of its own).
pub const EXPLOSION_LIFE: f32 = 3.0;
/// How long an instant (beam, flame) projectile model shows.
const INSTANT_LIFE: f32 = 0.35;

#[derive(Debug, Clone)]
pub struct Flight {
    /// `cue index:shot index`, stable across frames.
    pub key: String,
    pub shot: Shot,
    pub from: [f32; 3],
    /// The aim point.
    pub to: [f32; 3],
    /// Launch velocity and downward acceleration: p(dt) = from + vel·dt − ½·g·dt²·Z.
    pub vel: [f32; 3],
    pub g: f32,
    /// Impact time; None when range or lifetime ends the flight first.
    pub impact: Option<f32>,
    /// Where it burst (target, ground, or the aim point for instant ones).
    pub hit_point: [f32; 3],
    /// The target was hit directly or is inside the explosion.
    pub hits_target: bool,
    /// When the projectile model disappears.
    pub end: f32,
}

impl Flight {
    pub fn pos(&self, t: f32) -> [f32; 3] {
        let dt = (t - self.shot.start).max(0.0);
        if self.instant() {
            return self.from;
        }
        [self.from[0] + self.vel[0] * dt, self.from[1] + self.vel[1] * dt, self.from[2] + self.vel[2] * dt - 0.5 * self.g * dt * dt]
    }

    /// Unit direction of travel at `t`.
    pub fn dir(&self, t: f32) -> [f32; 3] {
        let dt = (t - self.shot.start).max(0.0);
        let v = if self.instant() { sub(self.to, self.from) } else { [self.vel[0], self.vel[1], self.vel[2] - self.g * dt] };
        norm(v)
    }

    /// Model scale at `t`: a cone grows to CollisionRadius + d·tan(spread).
    pub fn scale(&self, t: f32) -> f32 {
        let p = &self.shot.projectile;
        if p.kind != ProjKind::Cone {
            return 1.0;
        }
        let d = p.speed * (t - self.shot.start).max(0.0);
        let r0 = p.collision_radius.max(1.0);
        (r0 + d * p.cone_spread.clamp(0.0, 89.0).to_radians().tan()) / r0
    }

    fn instant(&self) -> bool {
        matches!(self.shot.projectile.kind, ProjKind::Beam | ProjKind::Flame | ProjKind::Barrier) || self.shot.projectile.speed <= 0.0
    }

    /// Where the explosion goes.
    pub fn impact_point(&self) -> [f32; 3] {
        self.hit_point
    }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn norm(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-6);
    [v[0] / l, v[1] / l, v[2] / l]
}

/// Rotation taking the NIF's +Y (forward) onto `dir`, Z kept up.
pub fn facing(dir: [f32; 3]) -> [f32; 9] {
    let f = norm(dir);
    let mut r = [f[1], -f[0], 0.0];
    if r[0].abs() + r[1].abs() < 1e-4 {
        r = [1.0, 0.0, 0.0];
    }
    let r = norm(r);
    let u = [r[1] * f[2] - r[2] * f[1], r[2] * f[0] - r[0] * f[2], r[0] * f[1] - r[1] * f[0]];
    // Columns are the images of local X, Y, Z.
    [r[0], f[0], u[0], r[1], f[1], u[1], r[2], f[2], u[2]]
}

/// Launch → impact for one shot from `from` at the aim point `to`.
pub fn fly(key: String, shot: &Shot, from: [f32; 3], to: [f32; 3]) -> Flight {
    let p = &shot.projectile;
    let d = sub(to, from);
    let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    let h = d[0].hypot(d[1]);
    let mut f = Flight { key, shot: shot.clone(), from, to, vel: [0.0; 3], g: 0.0, impact: Some(shot.start), hit_point: to, hits_target: true, end: shot.start + INSTANT_LIFE };
    if f.instant() {
        if p.range > 0.0 && dist > p.range {
            f.impact = None;
            f.hits_target = false;
        }
        return f;
    }
    // Range is distance flown at the launch speed; lifetime caps it too.
    let mut limit = if p.range > 0.0 { p.range / p.speed } else { f32::INFINITY };
    if p.lifetime > 0.0 {
        limit = limit.min(p.lifetime);
    }
    let n = norm(d);
    if p.kind == ProjKind::Cone {
        // A widening wave: no gravity, it passes the target and runs its range.
        f.vel = [n[0] * p.speed, n[1] * p.speed, n[2] * p.speed];
        let reach = (dist - TARGET_RADIUS).max(0.0) / p.speed;
        f.end = shot.start + if limit.is_finite() { limit } else { reach + 1.0 };
        f.impact = (reach <= limit).then_some(shot.start + reach);
        f.hits_target = f.impact.is_some();
        f.hit_point = if f.hits_target { to } else { f.pos(f.end) };
        return f;
    }
    let g = p.gravity.max(0.0) * GRAVITY;
    f.g = g;
    if p.kind == ProjKind::Lobber && g > 0.0 {
        // Lobbers are aimed: horizontal speed = PROJ speed, the vertical
        // launch speed lands the arc on the aim point.
        let time = h.max(1.0) / p.speed;
        f.vel = [d[0] / time, d[1] / time, (d[2] + 0.5 * g * time * time) / time];
        if time <= limit {
            f.impact = Some(shot.start + time);
            f.end = shot.start + time;
        } else {
            f.impact = None;
            f.hits_target = false;
            f.end = shot.start + limit;
            f.hit_point = f.pos(f.end);
        }
        return f;
    }
    // Missiles and arrows: launched straight at the aim point, gravity pulls.
    f.vel = [n[0] * p.speed, n[1] * p.speed, n[2] * p.speed];
    let z = |t: f32| from[2] + f.vel[2] * t - 0.5 * g * t * t;
    let vh = p.speed * n[0].hypot(n[1]);
    let at_target = (vh > 1e-3).then(|| (h - TARGET_RADIUS).max(0.0) / vh).filter(|&t| (0.0..=TARGET_HEIGHT).contains(&z(t)));
    let at_ground = if g > 0.0 {
        Some((f.vel[2] + (f.vel[2] * f.vel[2] + 2.0 * g * from[2].max(0.0)).sqrt()) / g)
    } else if f.vel[2] < -1e-3 {
        Some(-from[2] / f.vel[2])
    } else {
        None
    };
    let first = match (at_target, at_ground) {
        (Some(a), Some(b)) if b < a => Some((b, false)),
        (Some(a), _) => Some((a, true)),
        (None, Some(b)) => Some((b, false)),
        (None, None) => None,
    };
    match first.filter(|&(t, _)| t <= limit) {
        Some((t, direct)) => {
            f.impact = Some(shot.start + t);
            f.end = shot.start + t;
            f.hit_point = f.pos(f.end);
            if !direct {
                f.hit_point[2] = 0.0;
            }
            // Missed the target: it still takes the hit inside the explosion.
            let blast = shot.explosion.as_ref().map_or(0.0, |e| e.radius);
            f.hits_target = direct || (f.hit_point[0] - to[0]).hypot(f.hit_point[1] - to[1]) <= blast + TARGET_RADIUS;
        }
        None => {
            f.impact = None;
            f.hits_target = false;
            f.end = shot.start + if limit.is_finite() { limit } else { 3.0 };
            f.hit_point = f.pos(f.end);
        }
    }
    f
}

/// Every flight of the open clip's casts.
pub fn flights(app: &StudioApp, cues: &[Cue], path: Option<&Path>) -> Vec<Flight> {
    let tgt = crate::sds_path::target_pos(app);
    let to = [tgt[0], tgt[1], AIM_Z];
    let (k, rm) = (crate::sds_path::npc_scale(app), app.scene.root_motion);
    let mut out = Vec::new();
    for cue in cues {
        for (i, shot) in cue.shots.iter().enumerate() {
            let root = crate::sds_path::root(k, rm, path, shot.start);
            let hand = match &app.character {
                Some(c) => {
                    let local = match &app.anim {
                        Some(a) => sds_hkx::anim::local_pose(&c.skeleton, a, shot.start),
                        None => c.skeleton.reference.clone(),
                    };
                    let model: Vec<Transform> = sds_hkx::anim::model_pose(&c.skeleton, &local).iter().map(crate::character::to_transform).collect();
                    c.node_world(&model, &root, &shot.hand).map(|w| w.t)
                }
                None => None,
            };
            let from = hand.unwrap_or([root.t[0], root.t[1], root.t[2] + 110.0 * root.s]);
            let f = fly(format!("{}:{i}", cue.index), shot, from, to);
            if std::env::var_os("SDS_FX_DEBUG").is_some() {
                let p = &f.shot.projectile;
                eprintln!(
                    "FXPROJ {} {} {:?} speed {} grav {} model {:?} start {:.3} impact {:?} end {:.3} expl {:?}",
                    f.key, p.edid, p.kind, p.speed, p.gravity, p.model, f.shot.start, f.impact, f.end,
                    f.shot.explosion.as_ref().map(|e| (&e.edid, &e.model))
                );
            }
            out.push(f);
        }
    }
    out
}

/// A sound a flight plays: (time, SNDR, label, stop time for loops).
pub struct FlightSound {
    pub time: f32,
    pub sound: FormKey,
    pub label: String,
    pub until: Option<f32>,
}

pub fn sounds(flights: &[Flight], db: &sds_records::Db) -> Vec<FlightSound> {
    let edid = |k: FormKey| db.get(k).map(|r| r.edid()).unwrap_or_default();
    let mut out = Vec::new();
    for f in flights {
        let p = &f.shot.projectile;
        if let Some(s) = p.sound {
            out.push(FlightSound { time: f.shot.start, sound: s, label: sds_i18n::tf!("полёт: {} ({})", "flight: {} ({})", edid(s), f.shot.spell), until: Some(f.end) });
        }
        let Some(ti) = f.impact else { continue };
        if let Some(e) = &f.shot.explosion {
            if let Some(s) = e.sound {
                out.push(FlightSound { time: ti, sound: s, label: sds_i18n::tf!("взрыв: {} ({})", "explosion: {} ({})", edid(s), e.edid), until: None });
            }
        }
        for (s, label) in f.shot.hit_sounds.iter().filter(|_| f.hits_target) {
            out.push(FlightSound { time: ti, sound: *s, label: label.clone(), until: None });
        }
    }
    out
}

/// Projectile, explosion and hit art for time `t` into `out`.
#[allow(clippy::too_many_arguments)]
pub fn draw(
    flights: &[Flight],
    t: f32,
    player: &mut crate::fx::FxPlayer,
    vfs: &sds_vfs::Vfs,
    cam: &crate::fx::Camera,
    target: &Transform,
    out: &mut Vec<crate::render::FxBatch>,
) {
    for f in flights {
        let p = &f.shot.projectile;
        if t >= f.shot.start && t < f.end.max(f.shot.start + 1e-3) && !p.model.is_empty() {
            let asset = player.asset(vfs, &p.model);
            let world = Transform::from_av(f.pos(t), facing(f.dir(t)), f.scale(t));
            player.draw(&format!("proj:{}", f.key), &asset, &world, t - f.shot.start, cam, None, out);
        }
        let Some(ti) = f.impact else { continue };
        if t < ti {
            continue;
        }
        if let Some(e) = f.shot.explosion.as_ref().filter(|e| !e.model.is_empty()) {
            if t < ti + EXPLOSION_LIFE {
                let asset = player.asset(vfs, &e.model);
                let world = Transform::from_av(f.impact_point(), Transform::IDENTITY.rot, 1.0);
                player.draw(&format!("expl:{}", f.key), &asset, &world, t - ti, cam, None, out);
            }
        }
        for (k, piece) in f.shot.hits.iter().enumerate().filter(|_| f.hits_target) {
            if t >= ti + piece.start + piece.life {
                continue;
            }
            let asset = player.asset(vfs, &piece.model);
            let world = target.compose(&Transform::from_av(piece.offset, Transform::IDENTITY.rot, piece.scale));
            player.draw(&format!("hit:{}:{k}", f.key), &asset, &world, t - ti - piece.start, cam, None, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sds_records::fx::Projectile;

    fn shot(kind: ProjKind, speed: f32, gravity: f32, range: f32) -> Shot {
        Shot {
            start: 1.0,
            hand: String::new(),
            spell: String::new(),
            projectile: Projectile { key: FormKey { module: 0, id: 1 }, edid: "P".into(), model: String::new(), kind, gravity, speed, range, lifetime: 0.0, sound: None, explosion: None, cone_spread: 15.0, collision_radius: 16.0 },
            explosion: None,
            hits: Vec::new(),
            hit_sounds: Vec::new(),
        }
    }

    #[test]
    fn missile_flies_straight() {
        let f = fly("a".into(), &shot(ProjKind::Missile, 1000.0, 0.0, 0.0), [0.0, 0.0, 100.0], [0.0, 500.0, 100.0]);
        // Hits the dummy's surface, 30 units before its centre.
        assert!((f.impact.unwrap() - 1.47).abs() < 1e-4);
        assert!(f.hits_target);
        assert_eq!(f.pos(1.25), [0.0, 250.0, 100.0]);
    }

    #[test]
    fn lobber_lands_on_target() {
        let f = fly("a".into(), &shot(ProjKind::Lobber, 800.0, 1.0, 0.0), [0.0, 0.0, 120.0], [0.0, 800.0, 90.0]);
        let ti = f.impact.unwrap();
        assert!((ti - 2.0).abs() < 1e-4);
        let p = f.pos(ti);
        assert!((p[1] - 800.0).abs() < 0.01 && (p[2] - 90.0).abs() < 0.01, "{p:?}");
        assert!(f.pos(1.5)[2] > 120.0, "arcs up first");
    }

    #[test]
    fn out_of_range_never_hits() {
        let f = fly("a".into(), &shot(ProjKind::Missile, 1000.0, 0.0, 300.0), [0.0; 3], [0.0, 500.0, 0.0]);
        assert!(f.impact.is_none());
        assert!((f.end - 1.3).abs() < 1e-4);
    }

    #[test]
    fn heavy_missile_drops_at_the_feet() {
        let f = fly("a".into(), &shot(ProjKind::Missile, 10000.0, 10000.0, 10000.0), [0.0, 0.0, 120.0], [0.0, 700.0, 90.0]);
        let ti = f.impact.unwrap() - 1.0;
        assert!(ti < 0.01, "{ti}");
        // ~58 units ahead: 10000 u/s for the 6 ms it takes to fall 120.
        assert!(f.hit_point[1] < 100.0 && f.hit_point[2] == 0.0, "{:?}", f.hit_point);
        assert!(!f.hits_target);
    }

    #[test]
    fn cone_widens_and_passes_the_target() {
        let f = fly("a".into(), &shot(ProjKind::Cone, 1000.0, 0.0, 2000.0), [0.0, 0.0, 100.0], [0.0, 530.0, 100.0]);
        assert!((f.impact.unwrap() - 1.5).abs() < 1e-4);
        assert!((f.end - 3.0).abs() < 1e-4);
        assert!((f.pos(1.5)[1] - 500.0).abs() < 1e-3);
        // 500 units out: (16 + 500·tan 15°) / 16 ≈ 9.37.
        assert!((f.scale(1.5) - 9.37).abs() < 0.01, "{}", f.scale(1.5));
    }

    #[test]
    fn facing_maps_forward() {
        let r = facing([1.0, 0.0, 0.0]);
        // Local +Y → world +X.
        assert!((r[1] - 1.0).abs() < 1e-5 && r[4].abs() < 1e-5 && r[7].abs() < 1e-5);
    }
}
