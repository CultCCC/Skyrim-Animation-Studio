//! The path an NPC takes through a clip under SDSCombat.dll, against a
//! standing target — what TargetLock.cpp does each frame, replayed at 60 Hz.
//!
//! Headings are Skyrim's: 0 = +Y, positive = clockwise from above; a local
//! step (x right, y ahead) goes to the world by the current heading.
//!
//! Per frame:
//! 1. Annotations up to now update the state (priority Reset > Stop > Off > On).
//! 2. Turning. StopRotation holds the heading caught at the annotation
//!    (bHoldHeading), animation rotation included. Otherwise animrotation
//!    turns the NPC, and then: a lock turns toward the target no faster than
//!    strength × fMaxEngineTurnSpeed (strength 1 or none = the engine cap,
//!    or [TargetLock] fTurnSpeed when set; strength 0 = no turning at all);
//!    without a lock (none or Off) the AI turns at `ai_turn_speed`.
//! 3. Root motion. The step is the animmotion delta seen from the current
//!    heading ([AnimMotion] — the curve minus animrotation). Clips without
//!    animmotion steering get the lock's straightening instead. ReachTarget
//!    scales forward steps by k = clamp(D − arrive, min, max) / ref, D taken
//!    once at the annotation, ref/min/max × NPC scale, k clamped by
//!    [ReachTarget] fMinScale/fMaxScale.

use crate::{parse, Kind};

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub enabled: bool,
    /// [TargetLock] fTurnSpeed (°/s), 0 = full force.
    pub turn_speed: f32,
    pub max_engine_turn_speed: f32,
    pub straight_root_motion: bool,
    pub straight_max_angle: f32,
    pub anim_motion: bool,
    pub anim_min_step: f32,
    pub reach_min_scale: f32,
    pub reach_max_scale: f32,
    pub hold_heading: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            enabled: true,
            turn_speed: 0.0,
            max_engine_turn_speed: 240.0,
            straight_root_motion: true,
            straight_max_angle: 150.0,
            anim_motion: true,
            anim_min_step: 0.05,
            reach_min_scale: 0.0,
            reach_max_scale: 2.0,
            hold_heading: true,
        }
    }
}

/// What the engine itself turns at most (~4 rad/s, measured in game).
pub const ENGINE_TURN_CAP: f32 = 229.2;

impl Settings {
    pub fn apply_ini(&mut self, text: &str) {
        let mut section = String::new();
        for line in text.lines() {
            let line = line.trim().trim_start_matches('\u{feff}');
            if line.starts_with('[') {
                section = line.trim_matches(['[', ']']).to_ascii_lowercase();
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            let v = v.split(';').next().unwrap_or("").trim();
            let f = v.parse::<f32>().ok();
            let b = f.map(|x| x != 0.0);
            match (section.as_str(), k.trim().to_ascii_lowercase().as_str()) {
                ("general", "benabled") => self.enabled = b.unwrap_or(self.enabled),
                ("targetlock", "fturnspeed") => self.turn_speed = f.unwrap_or(self.turn_speed),
                ("targetlock", "fmaxengineturnspeed") => self.max_engine_turn_speed = f.unwrap_or(self.max_engine_turn_speed),
                ("targetlock", "bstraightrootmotion") => self.straight_root_motion = b.unwrap_or(self.straight_root_motion),
                ("targetlock", "fstraightmaxangle") => self.straight_max_angle = f.unwrap_or(self.straight_max_angle),
                ("animmotion", "benabled") => self.anim_motion = b.unwrap_or(self.anim_motion),
                ("animmotion", "fminstep") => self.anim_min_step = f.unwrap_or(self.anim_min_step),
                ("reachtarget", "fminscale") => self.reach_min_scale = f.unwrap_or(self.reach_min_scale),
                ("reachtarget", "fmaxscale") => self.reach_max_scale = f.unwrap_or(self.reach_max_scale),
                ("stoprotation", "bholdheading") => self.hold_heading = b.unwrap_or(self.hold_heading),
                _ => {}
            }
        }
    }
}

/// The scene the path is played in.
#[derive(Debug, Clone, PartialEq)]
pub struct Scene {
    /// Target position (x, y), NPC starts at the origin facing +Y.
    pub target: [f32; 2],
    /// NPC scale (height × race height).
    pub npc_scale: f32,
    /// How fast the AI turns toward the target when no lock is on (°/s).
    pub ai_turn_speed: f32,
    /// Apply SDS_NPC_* annotations (SDSCombat installed and enabled).
    pub sdscombat: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub t: f32,
    pub pos: [f32; 3],
    pub heading: f32,
    /// Lock turn limit in °/s while a lock is on (None = no lock).
    pub lock: Option<f32>,
    pub stop: bool,
    pub reach: Option<f32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub t: f32,
    pub index: usize,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Path {
    pub samples: Vec<Sample>,
    pub events: Vec<Event>,
}

impl Path {
    /// Position and heading at `t`, linear between samples.
    pub fn at(&self, t: f32) -> ([f32; 3], f32) {
        let s = &self.samples;
        if s.is_empty() {
            return ([0.0; 3], 0.0);
        }
        if t <= s[0].t {
            return (s[0].pos, s[0].heading);
        }
        for w in s.windows(2) {
            if t <= w[1].t {
                let f = (t - w[0].t) / (w[1].t - w[0].t).max(1e-6);
                let lerp = |a: f32, b: f32| a + (b - a) * f;
                let dh = wrap(w[1].heading - w[0].heading);
                return (
                    [lerp(w[0].pos[0], w[1].pos[0]), lerp(w[0].pos[1], w[1].pos[1]), lerp(w[0].pos[2], w[1].pos[2])],
                    w[0].heading + dh * f,
                );
            }
        }
        let l = s.last().unwrap();
        (l.pos, l.heading)
    }

    /// The sample in effect at `t` (state flags).
    pub fn state(&self, t: f32) -> Option<&Sample> {
        self.samples.iter().rev().find(|s| s.t <= t + 1e-6).or(self.samples.first())
    }
}

/// Degrees into (-180, 180].
pub fn wrap(d: f32) -> f32 {
    let mut d = d % 360.0;
    if d > 180.0 {
        d -= 360.0;
    } else if d <= -180.0 {
        d += 360.0;
    }
    d
}

/// Local step (x right, y ahead) → world by a clockwise heading in degrees.
pub fn to_world(local: [f32; 2], heading: f32) -> [f32; 2] {
    let (s, c) = heading.to_radians().sin_cos();
    [local[0] * c + local[1] * s, -local[0] * s + local[1] * c]
}

fn to_local(world: [f32; 2], heading: f32) -> [f32; 2] {
    to_world(world, -heading)
}

#[derive(Default)]
struct State {
    on: bool,
    off: bool,
    stop: Option<f32>,
    strength: Option<f32>,
    reach: Option<f32>,
}

/// Play the clip. `motion(t)` = (animmotion position in the clip-start frame,
/// already × NPC scale; animrotation in degrees). `items` = time-sorted annotations.
pub fn simulate(items: &[(f32, &str)], duration: f32, motion: &dyn Fn(f32) -> ([f32; 3], f32), has_motion: bool, scene: &Scene, s: &Settings) -> Path {
    let dt = 1.0 / 60.0;
    let mut path = Path::default();
    let mut st = State::default();
    let mut pos = [0.0f32; 3];
    let mut heading = 0.0f32;
    let mut next = 0usize;
    let active = scene.sdscombat && s.enabled;
    let n = (duration / dt).ceil().max(1.0) as usize;
    let mut prev_t = 0.0f32;
    let (mut prev_m, mut prev_r) = motion(0.0);

    let lock_limit = |st: &State| -> Option<f32> {
        if !st.on || st.off || st.stop.is_some() {
            return None;
        }
        let full = s.max_engine_turn_speed.min(ENGINE_TURN_CAP);
        Some(match st.strength {
            Some(k) if k >= 0.999 => full,
            Some(k) => (k.max(0.0) * s.max_engine_turn_speed).min(ENGINE_TURN_CAP),
            None if s.turn_speed > 0.0 => s.turn_speed.min(ENGINE_TURN_CAP),
            None => full,
        })
    };

    for i in 0..=n {
        let t = (i as f32 * dt).min(duration);
        // 1. Annotations up to t (those at 0 fire before the first step).
        while next < items.len() && items[next].0 <= t + 1e-6 {
            let (at, text) = items[next];
            if active {
                if let Kind::SdsNpc { event, values } = parse(text).kind {
                    let msg = match event.to_ascii_lowercase().as_str() {
                        "sds_npc_targetlockon" => {
                            st.on = true;
                            st.strength = values.first().copied();
                            None
                        }
                        "sds_npc_turnspeed" => {
                            st.on = true;
                            st.strength = values.first().copied().or(st.strength);
                            None
                        }
                        "sds_npc_targetlockoff" => {
                            st.off = true;
                            None
                        }
                        "sds_npc_stoprotation" => {
                            if st.stop.is_none() {
                                st.stop = Some(heading);
                            }
                            None
                        }
                        "sds_npc_targetlockreset" => {
                            st = State::default();
                            None
                        }
                        "sds_npc_reachtargetend" => {
                            st.reach = None;
                            None
                        }
                        "sds_npc_reachtarget" => {
                            let k_s = if scene.npc_scale > 0.05 && scene.npc_scale < 20.0 { scene.npc_scale } else { 1.0 };
                            let f = |k: usize| values.get(k).copied();
                            let reff = f(0).unwrap_or(0.0) * k_s;
                            let min = f(1).unwrap_or(0.0).max(0.0) * k_s;
                            let max = match f(2) {
                                Some(m) if m >= 0.0 => m * k_s,
                                _ => reff,
                            };
                            let arrive = f(3).unwrap_or(0.0).max(0.0);
                            if reff <= 0.0 {
                                st.reach = None;
                                Some(sds_i18n::t!("ReachTarget пропущен: нет ref", "ReachTarget skipped: no ref").to_string())
                            } else {
                                let d = (scene.target[0] - pos[0]).hypot(scene.target[1] - pos[1]);
                                let wanted = (d - arrive).clamp(min, min.max(max));
                                let k = (wanted / reff).clamp(s.reach_min_scale, s.reach_max_scale);
                                st.reach = Some(k);
                                Some(sds_i18n::tf!("ReachTarget: дистанция {d:.0}, остановиться в {arrive:.0} → нужно {wanted:.0} из {reff:.0} (диапазон {min:.0}..{max:.0}): шаги ×{k:.2}", "ReachTarget: distance {d:.0}, stop at {arrive:.0} → need {wanted:.0} of {reff:.0} (range {min:.0}..{max:.0}): steps ×{k:.2}"))
                            }
                        }
                        _ => None,
                    };
                    if let Some(text) = msg {
                        path.events.push(Event { t: at, index: next, text });
                    }
                }
            }
            next += 1;
        }

        if i > 0 {
            let step_dt = t - prev_t;
            let (m, r) = motion(t);
            let dm = [m[0] - prev_m[0], m[1] - prev_m[1], m[2] - prev_m[2]];
            let dr = r - prev_r;

            // 2. Turning.
            if let (Some(hold), true) = (st.stop, s.hold_heading) {
                heading = hold;
            } else if st.stop.is_some() {
                heading += dr;
            } else {
                heading += dr;
                let wanted = (scene.target[0] - pos[0]).atan2(scene.target[1] - pos[1]).to_degrees();
                let diff = wrap(wanted - heading);
                let limit = match lock_limit(&st) {
                    Some(l) => l,
                    // Strength 0 under a lock is "no turning at all"; that is
                    // Some(0) above. No lock: the AI's own turning.
                    None => scene.ai_turn_speed,
                };
                let max = limit * step_dt;
                heading += diff.clamp(-max, max);
            }
            heading = wrap(heading);

            // 3. Root motion: the curve's step seen from the current heading.
            let world_clip = [dm[0], dm[1]];
            // In the clip-start frame; minus the animation's own rotation = local.
            let mut local = to_local(world_clip, r);
            let len = local[0].hypot(local[1]);
            let steered = s.anim_motion && has_motion && len > s.anim_min_step;
            if !steered && s.straight_root_motion && lock_limit(&st).is_some() && len > 1e-3 {
                let ang = local[0].atan2(local[1]).to_degrees();
                if ang.abs() > 1.0 && ang.abs() < s.straight_max_angle {
                    local = [0.0, len];
                }
            }
            if let Some(k) = st.reach {
                if local[1] > 0.0 {
                    local = [local[0] * k, local[1] * k];
                }
            }
            let w = to_world(local, heading);
            pos = [pos[0] + w[0], pos[1] + w[1], pos[2] + dm[2]];
            prev_m = m;
            prev_r = r;
        }
        path.samples.push(Sample { t, pos, heading, lock: lock_limit(&st), stop: st.stop.is_some(), reach: st.reach });
        prev_t = t;
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    fn straight(t: f32) -> ([f32; 3], f32) {
        ([0.0, 200.0 * t, 0.0], 0.0)
    }

    fn scene(target: [f32; 2]) -> Scene {
        Scene { target, npc_scale: 1.0, ai_turn_speed: 0.0, sdscombat: true }
    }

    #[test]
    fn plain_root_motion_is_the_curve() {
        let p = simulate(&[], 1.0, &straight, true, &scene([0.0, 500.0]), &Settings::default());
        let (pos, h) = p.at(1.0);
        assert!((pos[1] - 200.0).abs() < 0.5 && pos[0].abs() < 1e-3 && h == 0.0);
    }

    #[test]
    fn lock_turns_toward_target_at_its_limit() {
        // Target to the right: 90° away. Strength 0.5 = 120°/s.
        let items = [(0.0, "SDS_NPC_TargetLockOn.0.5")];
        let p = simulate(&items, 0.5, &|_| ([0.0; 3], 0.0), true, &scene([500.0, 0.0]), &Settings::default());
        let (_, h) = p.at(0.5);
        assert!((h - 60.0).abs() < 1.5, "heading {h}");
        // Strength 0: no turning at all, even with an AI turning.
        let s0 = Scene { ai_turn_speed: 90.0, ..scene([500.0, 0.0]) };
        let p = simulate(&[(0.0, "SDS_NPC_TargetLockOn.0")], 0.5, &|_| ([0.0; 3], 0.0), true, &s0, &Settings::default());
        assert!(p.at(0.5).1.abs() < 1e-3);
        // StopRotation holds; Off hands back to the AI.
        let p = simulate(&[(0.0, "SDS_NPC_TargetLockOn"), (0.1, "SDS_NPC_StopRotation")], 0.5, &|_| ([0.0; 3], 0.0), true, &scene([500.0, 0.0]), &Settings::default());
        let h1 = p.at(0.1).1;
        assert!((p.at(0.5).1 - h1).abs() < 1e-3 && h1 > 10.0);
    }

    #[test]
    fn reach_target_scales_forward_steps() {
        // Guide example 280|60|420|150: target at 300 → k = 150/280.
        let items = [(0.0, "SDS_NPC_ReachTarget.280|60|420|150")];
        let p = simulate(&items, 1.0, &straight, true, &scene([0.0, 300.0]), &Settings::default());
        let k = 150.0 / 280.0;
        assert!((p.at(1.0).0[1] - 200.0 * k).abs() < 0.5);
        assert!(p.events[0].text.contains("×0.54"));
        // Ignored without SDSCombat.
        let off = Scene { sdscombat: false, ..scene([0.0, 300.0]) };
        let p = simulate(&items, 1.0, &straight, true, &off, &Settings::default());
        assert!((p.at(1.0).0[1] - 200.0).abs() < 0.5);
    }

    #[test]
    fn motion_follows_the_lock_turn() {
        // Lock turns the NPC right; the forward animmotion follows the heading.
        let items = [(0.0, "SDS_NPC_TargetLockOn")];
        let p = simulate(&items, 1.0, &straight, true, &scene([1000.0, 0.0]), &Settings::default());
        let (pos, h) = p.at(1.0);
        assert!(h > 80.0 && pos[0] > 100.0, "{pos:?} {h}");
    }
}
