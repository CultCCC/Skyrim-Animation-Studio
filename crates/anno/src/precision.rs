//! Precision 2.0.4 attack collisions from a clip's annotations.
//!
//! Rules as the Precision source applies them (PrecisionHandler.cpp,
//! AttackCollision.cpp), cross-checked with SSE Animation Previewer's notes:
//! - `Collision_AttackStart` / `Collision_Start` opens the attack; `Add`,
//!   `Remove`, `ClearTargets` before it are ignored.
//! - `Collision_Add` needs `Node()`. Event and parameter names ignore case.
//! - `Collision_Remove.Node(x)` removes the FIRST collision on node `x`,
//!   compared case-sensitively. `Remove.ID(n)` erases with one iterator
//!   after `remove_if`: all matches go only when a non-matching collision
//!   follows them in the list, otherwise just one does.
//! - `Collision_AttackEnd`, `Collision_Cancel`, `attackStop`, `staggerStart`
//!   remove everything and close the attack.
//! - `Duration(x)` ends a collision x s after it starts; `Duration(0)` uses
//!   fDefaultCollisionLifetime (× the power-attack mult for power attacks).
//! - Each collision gets a recoil capsule (unless `NoRecoil`/`WeaponTip`, or
//!   recoil is off for this actor); `hitFrame` removes the first remaining
//!   recoil capsule when bRemoveRecoilOnHitframe is on.

use crate::{parse, Kind};

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub weapon_length_mult: f32,
    pub weapon_capsule_radius: f32,
    pub min_weapon_length: f32,
    pub default_lifetime: f32,
    pub lifetime_power_mult: f32,
    pub player_length_mult: f32,
    pub player_radius_mult: f32,
    pub recoil_player: bool,
    pub recoil_npc: bool,
    pub remove_recoil_on_hitframe: bool,
    pub recoil_length: f32,
}

impl Default for Settings {
    /// Precision 2.0.4's shipped settings.ini.
    fn default() -> Self {
        Settings {
            weapon_length_mult: 1.2,
            weapon_capsule_radius: 12.0,
            min_weapon_length: 40.0,
            default_lifetime: 0.3,
            lifetime_power_mult: 1.8,
            player_length_mult: 1.0,
            player_radius_mult: 1.5,
            recoil_player: true,
            recoil_npc: true,
            remove_recoil_on_hitframe: true,
            recoil_length: 25.0,
        }
    }
}

impl Settings {
    /// Apply one ini file over the current values (`key = value` lines).
    pub fn apply_ini(&mut self, text: &str) {
        for line in text.lines() {
            let line = line.trim().trim_start_matches('\u{feff}');
            let Some((k, v)) = line.split_once('=') else { continue };
            let k = k.trim().to_ascii_lowercase();
            let v = v.trim();
            let f = v.parse::<f32>().ok();
            let b = v.parse::<f32>().ok().map(|x| x != 0.0);
            match k.as_str() {
                "fweaponlengthmult" => self.weapon_length_mult = f.unwrap_or(self.weapon_length_mult),
                "fweaponcapsuleradius" => self.weapon_capsule_radius = f.unwrap_or(self.weapon_capsule_radius),
                "fminweaponlength" => self.min_weapon_length = f.unwrap_or(self.min_weapon_length),
                "fdefaultcollisionlifetime" => self.default_lifetime = f.unwrap_or(self.default_lifetime),
                "fdefaultcollisionlifetimepowerattackmult" => self.lifetime_power_mult = f.unwrap_or(self.lifetime_power_mult),
                "fplayerattacklengthmult" => self.player_length_mult = f.unwrap_or(self.player_length_mult),
                "fplayerattackradiusmult" => self.player_radius_mult = f.unwrap_or(self.player_radius_mult),
                "brecoilplayer" => self.recoil_player = b.unwrap_or(self.recoil_player),
                "brecoilnpc" => self.recoil_npc = b.unwrap_or(self.recoil_npc),
                "bremoverecoilonhitframe" => self.remove_recoil_on_hitframe = b.unwrap_or(self.remove_recoil_on_hitframe),
                "frecoilcollisionlength" => self.recoil_length = f.unwrap_or(self.recoil_length),
                _ => {}
            }
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Params {
    pub node: String,
    pub id: Option<i64>,
    pub duration: Option<f32>,
    pub length: Option<f32>,
    pub radius: Option<f32>,
    pub length_mult: f32,
    pub radius_mult: f32,
    pub scale: f32,
    pub translation: [f32; 3],
    /// Degrees, applied X then Y then Z.
    pub rotation: [f32; 3],
    pub no_recoil: bool,
    pub weapon_tip: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Collision {
    /// Index of the `Collision_Add` annotation.
    pub add: usize,
    pub params: Params,
    pub start: f32,
    pub end: f32,
    /// Why it ended (annotation text, "Duration", "конец клипа").
    pub end_reason: String,
    /// When its recoil capsule goes (None = no recoil capsule).
    pub recoil_end: Option<f32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub index: usize,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Result {
    pub collisions: Vec<Collision>,
    pub notes: Vec<Note>,
}

fn floats(v: &str) -> Vec<f32> {
    v.split(|c: char| c == ',' || c.is_whitespace() || c == '|').filter_map(|x| x.trim().parse().ok()).collect()
}

fn params_of(list: &[(String, String)]) -> Params {
    let mut p = Params { length_mult: 1.0, radius_mult: 1.0, scale: 1.0, ..Default::default() };
    for (k, v) in list {
        let f = v.trim().parse::<f32>().ok();
        match k.to_ascii_lowercase().as_str() {
            "node" => p.node = v.clone(),
            "id" => p.id = v.trim().parse().ok(),
            "duration" => p.duration = f,
            "length" => p.length = f,
            "radius" => p.radius = f,
            "lengthmult" => p.length_mult = f.unwrap_or(1.0),
            "radiusmult" => p.radius_mult = f.unwrap_or(1.0),
            "scale" => p.scale = f.unwrap_or(1.0),
            "translation" => {
                let t = floats(v);
                if t.len() >= 3 {
                    p.translation = [t[0], t[1], t[2]];
                }
            }
            "rotation" => {
                let t = floats(v);
                if t.len() >= 3 {
                    p.rotation = [t[0], t[1], t[2]];
                }
            }
            "norecoil" => p.no_recoil = true,
            "weapontip" => p.weapon_tip = true,
            _ => {}
        }
    }
    p
}

struct Active {
    c: Collision,
    expires: Option<f32>,
    recoil: bool,
}

/// Run the clip's annotations (time-sorted, as `(time, text)`).
pub fn simulate(items: &[(f32, &str)], duration: f32, settings: &Settings, power_attack: bool, player: bool) -> Result {
    let mut out = Result::default();
    let mut open = false;
    let mut active: Vec<Active> = Vec::new();
    let recoil_on = if player { settings.recoil_player } else { settings.recoil_npc };

    let finish = |a: Active, t: f32, reason: &str, out: &mut Result| {
        let mut c = a.c;
        c.end = t;
        c.end_reason = reason.to_string();
        if let Some(r) = c.recoil_end {
            c.recoil_end = Some(r.min(t));
        } else if a.recoil {
            c.recoil_end = Some(t);
        }
        out.collisions.push(c);
    };

    for (i, (t, text)) in items.iter().enumerate() {
        let t = *t;
        // Expire Duration collisions up to now.
        let mut k = 0;
        while k < active.len() {
            if active[k].expires.is_some_and(|e| e <= t) {
                let a = active.remove(k);
                let e = a.expires.unwrap();
                finish(a, e, "Duration", &mut out);
            } else {
                k += 1;
            }
        }
        let lower = text.trim().to_ascii_lowercase();
        if lower == "attackstop" || lower == "staggerstart" {
            for a in active.drain(..) {
                finish(a, t, text, &mut out);
            }
            open = false;
            continue;
        }
        if lower == "hitframe" {
            if settings.remove_recoil_on_hitframe {
                if let Some(a) = active.iter_mut().find(|a| a.recoil && a.c.recoil_end.is_none()) {
                    a.c.recoil_end = Some(t);
                }
            }
            continue;
        }
        let Kind::Precision { event, params } = parse(text).kind else { continue };
        match event.to_ascii_lowercase().as_str() {
            "collision_attackstart" | "collision_start" => open = true,
            "collision_attackend" | "collision_cancel" => {
                for a in active.drain(..) {
                    finish(a, t, text, &mut out);
                }
                open = false;
            }
            "collision_add" => {
                if !open {
                    out.notes.push(Note { index: i, text: sds_i18n::t!("игнорируется: до Collision_AttackStart (или после конца атаки)", "ignored: before Collision_AttackStart (or after the attack ended)").into() });
                    continue;
                }
                let p = params_of(&params);
                if p.node.is_empty() {
                    out.notes.push(Note { index: i, text: sds_i18n::t!("отклонено: нет Node()", "rejected: no Node()").into() });
                    continue;
                }
                let expires = match p.duration {
                    Some(d) if d > 0.0 => Some(t + d),
                    Some(_) => Some(t + settings.default_lifetime * if power_attack { settings.lifetime_power_mult } else { 1.0 }),
                    None => None,
                };
                let recoil = recoil_on && !p.no_recoil && !p.weapon_tip;
                active.push(Active {
                    c: Collision { add: i, params: p, start: t, end: duration, end_reason: String::new(), recoil_end: None },
                    expires,
                    recoil,
                });
            }
            "collision_remove" => {
                if !open {
                    out.notes.push(Note { index: i, text: sds_i18n::t!("игнорируется: атака не открыта", "ignored: attack is not open").into() });
                    continue;
                }
                let p = params_of(&params);
                if !p.node.is_empty() {
                    match active.iter().position(|a| a.c.params.node == p.node) {
                        Some(k) => {
                            let a = active.remove(k);
                            finish(a, t, text, &mut out);
                        }
                        None => out.notes.push(Note { index: i, text: sds_i18n::tf!("нет активной коллизии на узле «{}» (сравнение с учётом регистра)", "no active collision on node «{}» (case-sensitive match)", p.node) }),
                    }
                } else if let Some(id) = p.id {
                    let matches: Vec<usize> = (0..active.len()).filter(|&k| active[k].c.params.id == Some(id)).collect();
                    let Some(&last) = matches.last() else {
                        out.notes.push(Note { index: i, text: sds_i18n::tf!("нет коллизий с ID({id})", "no collisions with ID({id})") });
                        continue;
                    };
                    let tail_after = active.len() > last + 1;
                    let remove: Vec<usize> = if tail_after || matches.len() == 1 { matches.clone() } else { vec![matches[0]] };
                    if remove.len() < matches.len() {
                        out.notes.push(Note { index: i, text: sds_i18n::tf!("особенность Precision: ID({id}) в конце списка — снята только одна из {}", "Precision quirk: ID({id}) at the end of the list — only one of {} removed", matches.len()) });
                    }
                    for k in remove.into_iter().rev() {
                        let a = active.remove(k);
                        finish(a, t, text, &mut out);
                    }
                }
            }
            "collision_cleartargets" => {}
            _ => {}
        }
    }
    // Whatever is left: Duration expiry within the clip, else the clip end.
    for a in active.drain(..) {
        match a.expires {
            Some(e) if e <= duration => finish(a, e, "Duration", &mut out),
            _ => finish(a, duration, sds_i18n::t!("конец клипа", "clip end"), &mut out),
        }
    }
    out.collisions.sort_by(|a, b| a.start.total_cmp(&b.start));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sim(items: &[(f32, &str)]) -> Result {
        simulate(items, 2.0, &Settings::default(), false, false)
    }

    #[test]
    fn start_gate_and_remove_by_node() {
        let r = sim(&[
            (0.1, "Collision_Add.Node(WEAPON)"),
            (0.2, "Collision_AttackStart"),
            (0.3, "Collision_Add.Node(WEAPON)"),
            (0.6, "Collision_Remove.Node(weapon)"),
            (0.7, "Collision_Remove.Node(WEAPON)"),
        ]);
        assert_eq!(r.collisions.len(), 1);
        assert_eq!((r.collisions[0].start, r.collisions[0].end), (0.3, 0.7));
        assert_eq!(r.notes.len(), 2); // early Add, case-mismatched Remove
    }

    #[test]
    fn durations_and_attack_end() {
        let r = sim(&[
            (0.0, "Collision_AttackStart"),
            (0.1, "Collision_Add.Node(WEAPON).Duration(0)"),
            (0.2, "Collision_Add.Node(SHIELD).Duration(0.25)"),
            (0.3, "Collision_Add.Node(NPC R Hand [RHnd])"),
            (1.0, "Collision_AttackEnd"),
            (1.1, "Collision_Add.Node(WEAPON)"),
        ]);
        let ends: Vec<(f32, f32)> = r.collisions.iter().map(|c| (c.start, (c.end * 100.0).round() / 100.0)).collect();
        assert_eq!(ends, vec![(0.1, 0.4), (0.2, 0.45), (0.3, 1.0)]);
        assert_eq!(r.notes.len(), 1);
        let p = simulate(&[(0.0, "Collision_AttackStart"), (0.1, "Collision_Add.Node(WEAPON).Duration(0)")], 2.0, &Settings::default(), true, false);
        assert!((p.collisions[0].end - (0.1 + 0.3 * 1.8)).abs() < 1e-5);
    }

    #[test]
    fn id_removal_quirk_and_recoil() {
        let r = sim(&[
            (0.0, "Collision_AttackStart"),
            (0.1, "Collision_Add.Node(WEAPON).ID(1)"),
            (0.2, "Collision_Add.Node(SHIELD).ID(1)"),
            (0.4, "HitFrame"),
            (0.5, "Collision_Remove.ID(1)"),
        ]);
        // Two ID(1) at the end of the list: only one goes.
        assert_eq!(r.collisions.iter().filter(|c| c.end == 0.5).count(), 1);
        assert_eq!(r.collisions.iter().filter(|c| c.end == 2.0).count(), 1);
        // The first recoil capsule went at the hit frame.
        assert_eq!(r.collisions[0].recoil_end, Some(0.4));
        let s = Settings { recoil_npc: false, ..Settings::default() };
        let n = simulate(&[(0.0, "Collision_AttackStart"), (0.1, "Collision_Add.Node(WEAPON)")], 1.0, &s, false, false);
        assert_eq!(n.collisions[0].recoil_end, None);
    }

    #[test]
    fn ini_layers() {
        let mut s = Settings::default();
        s.apply_ini("[AttackCollisions]\nfWeaponCapsuleRadius = 12\nfMinWeaponLength = 40\n");
        s.apply_ini("\u{feff}[Recoil]\nbRecoilNPC = 0\n[AttackCollisions]\nfWeaponCapsuleRadius = 14.000000\nfMinWeaponLength = 85.000000\n");
        assert_eq!((s.weapon_capsule_radius, s.min_weapon_length, s.recoil_npc), (14.0, 85.0, false));
    }
}

// ---- editing schema ------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamKind {
    /// Bone / node name.
    Node,
    Int,
    /// Present or not, no value.
    Flag,
    Float,
    /// `x, y, z`.
    Vec3,
    /// `r, g, b, a`.
    Color4,
    /// `strength, length, frequency`.
    Shake,
    Path,
}

pub struct ParamSpec {
    pub name: &'static str,
    pub kind: ParamKind,
    /// Only meaningful on `Collision_Add` (Node and ID also go on Remove).
    pub add_only: bool,
    pub default: &'static str,
    pub help: sds_i18n::Tr,
}

/// Precision's payload parameters, in the order its documentation lists them.
pub const PARAMS: &[ParamSpec] = &[
    ParamSpec { name: "Node", kind: ParamKind::Node, add_only: false, default: "WEAPON", help: sds_i18n::tr!("Кость-родитель коллизии. Обязателен в Collision_Add. Пример: Node(WEAPON), Node(NPC R Hand [RHnd])", "Parent bone of the collision. Required in Collision_Add. Example: Node(WEAPON), Node(NPC R Hand [RHnd])") },
    ParamSpec { name: "ID", kind: ParamKind::Int, add_only: false, default: "1", help: sds_i18n::tr!("Общий ID: коллизии с одним ID делят список уже поражённых целей — атака бьёт цель один раз, какой бы капсулой ни попала (например, взмах хвостом дракона)", "Shared ID: collisions with the same ID share the list of already hit targets — the attack hits a target once, whichever capsule touched it (e.g. a dragon tail swipe)") },
    ParamSpec { name: "NoRecoil", kind: ParamKind::Flag, add_only: true, default: "", help: sds_i18n::tr!("Коллизия не вызывает отдачу при ударе о статику, даже если отдача включена", "The collision causes no recoil when hitting statics, even if recoil is enabled") },
    ParamSpec { name: "NoTrail", kind: ParamKind::Flag, add_only: true, default: "", help: sds_i18n::tr!("Без следа (trail), даже если следы включены", "No trail, even if trails are enabled") },
    ParamSpec { name: "TrailUseTrueLength", kind: ParamKind::Flag, add_only: true, default: "", help: sds_i18n::tr!("След берёт длину коллизии, а не длину меша оружия (для магически удлинённых атак)", "The trail takes the collision length instead of the weapon mesh length (for magically extended attacks)") },
    ParamSpec { name: "WeaponTip", kind: ParamKind::Flag, add_only: true, default: "", help: sds_i18n::tr!("Только для WEAPON/SHIELD: корень капсулы на кончике оружия, а не в руке. С тем же ID и DamageMult — «сладкая точка» рапиры/копья/косы", "WEAPON/SHIELD only: the capsule root is at the weapon tip instead of the hand. With the same ID and DamageMult — the «sweet spot» of a rapier/spear/scythe") },
    ParamSpec { name: "DamageMult", kind: ParamKind::Float, add_only: true, default: "1.5", help: sds_i18n::tr!("Множитель урона ударов этой коллизией (добивания ×1.5, быстрые двойные ×0.5)", "Damage multiplier for hits by this collision (finishers ×1.5, quick dual ×0.5)") },
    ParamSpec { name: "Length", kind: ParamKind::Float, add_only: true, default: "50", help: sds_i18n::tr!("Заменяет длину капсулы узла", "Replaces the node capsule length") },
    ParamSpec { name: "LengthMult", kind: ParamKind::Float, add_only: true, default: "1.5", help: sds_i18n::tr!("Умножает длину капсулы узла", "Multiplies the node capsule length") },
    ParamSpec { name: "Radius", kind: ParamKind::Float, add_only: true, default: "25.5", help: sds_i18n::tr!("Заменяет радиус капсулы узла", "Replaces the node capsule radius") },
    ParamSpec { name: "RadiusMult", kind: ParamKind::Float, add_only: true, default: "1.5", help: sds_i18n::tr!("Умножает радиус капсулы узла", "Multiplies the node capsule radius") },
    ParamSpec { name: "Duration", kind: ParamKind::Float, add_only: true, default: "0.5", help: sds_i18n::tr!("Капсула снимается через это время. Без него нужен Collision_Remove (автор мода рекомендует Remove)", "The capsule is removed after this time. Without it a Collision_Remove is needed (the mod author recommends Remove)") },
    ParamSpec { name: "Rotation", kind: ParamKind::Vec3, add_only: true, default: "90, 0, 90", help: sds_i18n::tr!("Локальный поворот капсулы, углы Эйлера (градусы)", "Local capsule rotation, Euler angles (degrees)") },
    ParamSpec { name: "Translation", kind: ParamKind::Vec3, add_only: true, default: "0, -25, -25", help: sds_i18n::tr!("Локальное смещение капсулы", "Local capsule offset") },
    ParamSpec { name: "Scale", kind: ParamKind::Float, add_only: true, default: "2", help: sds_i18n::tr!("Равномерный локальный масштаб капсулы", "Uniform local capsule scale") },
    ParamSpec { name: "GroundShake", kind: ParamKind::Shake, add_only: true, default: "30, 0.3, 40", help: sds_i18n::tr!("Тряска камеры при ударе о землю: сила, длительность, частота", "Camera shake when hitting the ground: strength, duration, frequency") },
    ParamSpec { name: "TrailLifetimeMult", kind: ParamKind::Float, add_only: true, default: "1.5", help: sds_i18n::tr!("Множитель жизни следа; отключает поиск переопределения следа в .toml", "Trail lifetime multiplier; disables the trail override lookup in .toml") },
    ParamSpec { name: "TrailBaseColorOverride", kind: ParamKind::Color4, add_only: true, default: "1, 0.35, 0.1, 1", help: sds_i18n::tr!("Цвет следа (r, g, b, a)", "Trail colour (r, g, b, a)") },
    ParamSpec { name: "TrailBaseColorScaleMult", kind: ParamKind::Float, add_only: true, default: "2.5", help: sds_i18n::tr!("Множитель яркости цвета следа", "Trail colour brightness multiplier") },
    ParamSpec { name: "TrailMeshOverride", kind: ParamKind::Path, add_only: true, default: "Effects/WeaponTrails/AttackTrailMagic.nif", help: sds_i18n::tr!("Свой меш следа", "Custom trail mesh") },
];

/// Precision's events with what they do.
pub const EVENTS: &[(&str, sds_i18n::Tr)] = &[
    ("Collision_AttackStart", sds_i18n::tr!("Открывает атаку: до него Add/Remove игнорируются", "Opens the attack: Add/Remove before it are ignored")),
    ("Collision_Add", sds_i18n::tr!("Добавляет капсулу (нужен Node)", "Adds a capsule (Node required)")),
    ("Collision_Remove", sds_i18n::tr!("Снимает капсулу по Node (первую на узле, с учётом регистра) или по ID", "Removes a capsule by Node (the first on the node, case-sensitive) or by ID")),
    ("Collision_ClearTargets", sds_i18n::tr!("Очищает список поражённых целей: та же атака может ударить снова", "Clears the list of hit targets: the same attack can hit again")),
    ("Collision_AttackEnd", sds_i18n::tr!("Снимает все капсулы и закрывает атаку", "Removes all capsules and closes the attack")),
    ("Collision_Cancel", sds_i18n::tr!("То же, что AttackEnd", "Same as AttackEnd")),
];

pub fn spec(name: &str) -> Option<&'static ParamSpec> {
    PARAMS.iter().find(|s| s.name.eq_ignore_ascii_case(name))
}

/// The separator a text already uses between parameters (`.` or `|`).
pub fn separator_of(text: &str) -> char {
    if text.contains(")|") || text.contains("|ID(") {
        '|'
    } else {
        '.'
    }
}

/// `Event.P1(v)<sep>Flag<sep>P2(v)`, parameters in schema order, unknown ones kept last.
pub fn build(event: &str, params: &[(String, String)], sep: char) -> String {
    let mut ordered: Vec<&(String, String)> = Vec::new();
    for s in PARAMS {
        ordered.extend(params.iter().filter(|(k, _)| k.eq_ignore_ascii_case(s.name)));
    }
    ordered.extend(params.iter().filter(|(k, _)| spec(k).is_none()));
    let mut out = event.to_string();
    for (i, (k, v)) in ordered.iter().enumerate() {
        out.push(if i == 0 { '.' } else { sep });
        out.push_str(k);
        let flag = spec(k).is_some_and(|s| s.kind == ParamKind::Flag);
        if !flag || !v.is_empty() {
            out.push('(');
            out.push_str(v);
            out.push(')');
        }
    }
    out
}

#[cfg(test)]
mod schema_tests {
    use super::*;
    use crate::{parse, Kind};

    #[test]
    fn rebuild_keeps_style() {
        let t = "Collision_Add.Node(NPC R Calf [RClf])|ID(1)|Scale(2)";
        let Kind::Precision { event, params } = parse(t).kind else { panic!() };
        assert_eq!(build(&event, &params, separator_of(t)), t);
        let t2 = "Collision_Add.Node(WEAPON).WeaponTip.DamageMult(1.5).ID(3)";
        let Kind::Precision { event, params } = parse(t2).kind else { panic!() };
        // Schema order: Node, ID, flags, numbers.
        assert_eq!(build(&event, &params, '.'), "Collision_Add.Node(WEAPON).ID(3).WeaponTip.DamageMult(1.5)");
        assert_eq!(build("Collision_AttackStart", &[], '.'), "Collision_AttackStart");
    }
}
