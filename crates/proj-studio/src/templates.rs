//! Ready-made chains: a trigger plus the instances it needs, with fresh keys.

use sds_i18n::Tr;
use sds_proj::model::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tpl {
    Empty,
    Homing,
    Recast,
    Shotgun,
    Emitter,
    Followers,
}

pub const ALL: &[(Tpl, Tr, Tr)] = &[
    (Tpl::Empty, Tr("Пустой триггер", "Empty trigger"), Tr("Снаряд появился → (добавь функции)", "Projectile appeared → (add functions)")),
    (Tpl::Homing, Tr("Самонаведение", "Homing"), Tr("Снаряд появился → наводится на ближайшего врага", "Projectile appeared → homes in on the nearest enemy")),
    (
        Tpl::Recast,
        Tr("Подмена снаряда", "Replace projectile"),
        Tr("Исходный снаряд отменяется, вместо него кастуется другое заклинание (как SDS BloodArc)", "The origin projectile is cancelled and another spell is cast instead (like SDS BloodArc)"),
    ),
    (Tpl::Shotgun, Tr("Веер (мультикаст)", "Fan (multicast)"), Tr("Снаряд появился → 5 копий веером по линии", "Projectile appeared → 5 copies in a fanned line")),
    (Tpl::Emitter, Tr("Эмиттер", "Emitter"), Tr("Снаряд в полёте раз в секунду выпускает заклинание (3 раза)", "The projectile casts a spell once a second in flight (3 times)")),
    (Tpl::Followers, Tr("Спутники", "Followers"), Tr("Снаряды кружат вокруг кастера по окружности", "Projectiles orbit the caster in a circle")),
];

fn group(shape: &str, count: u32, size: f64) -> SpawnGroup {
    SpawnGroup {
        pattern: Pattern {
            figure: Figure { shape: shape.into(), count: Some(count), size: (shape != "Single").then_some(size), extra: Extra::new() },
            ..Default::default()
        },
        ..Default::default()
    }
}

fn func(kind: &str, id: &str) -> Func {
    Func { kind: kind.into(), id: id.into(), ..Default::default() }
}

/// Add the chain; returns the new trigger index. `proj` fills the
/// ProjBaseIsFormID condition when given.
pub fn add(c: &mut Config, t: Tpl, proj: Option<String>) -> usize {
    let cond = vec![Cond { kind: "ProjBaseIsFormID".into(), value: proj.unwrap_or_default(), invert: false, or: false }];
    let mut trig = Trigger { event: "ProjAppeared".into(), conditions: cond, has_conditions: true, ..Default::default() };
    match t {
        Tpl::Empty => {}
        Tpl::Homing => {
            let k = c.fresh_key(Section::Homing, "key_H_");
            c.homing.push((k.clone(), Homing { kind: "ConstSpeed".into(), rotation_time: Some(2.5), target: Some("Nearest".into()), ..Default::default() }));
            trig.tf.functions.push(func("SetHoming", &k));
        }
        Tpl::Recast | Tpl::Shotgun => {
            let g = c.fresh_key(Section::SpawnGroups, "key_SG_");
            let sg = if t == Tpl::Recast { group("Single", 1, 0.0) } else { SpawnGroup { rotation: Some("ToSight".into()), ..group("Line", 5, 250.0) } };
            c.spawn_groups.push((g.clone(), sg));
            let m = c.fresh_key(Section::Multicast, "key_MC_");
            let spell = if t == Tpl::Recast { String::new() } else { "Current".into() };
            c.multicast.push((m.clone(), vec![McItem { spawn_group: g, source: McSource::Spell(spell), ..Default::default() }]));
            trig.tf.functions.push(func("ApplyMultiCast", &m));
            trig.tf.disable_origin = Some(true);
        }
        Tpl::Emitter => {
            let g = c.fresh_key(Section::SpawnGroups, "key_SG_");
            c.spawn_groups.push((g.clone(), group("Single", 1, 0.0)));
            let m = c.fresh_key(Section::Multicast, "key_MC_");
            c.multicast.push((m.clone(), vec![McItem { spawn_group: g, source: McSource::Spell(String::new()), ..Default::default() }]));
            let e = c.fresh_key(Section::Emitters, "key_EM_");
            c.emitters.push((
                e.clone(),
                Emitter {
                    interval: 1.0,
                    limited: Some(true),
                    count: Some(3),
                    functions: vec![EmFunc { kind: "TriggerFunctions".into(), tf: TriggerFunctions { functions: vec![func("ApplyMultiCast", &m)], ..Default::default() }, ..Default::default() }],
                    ..Default::default()
                },
            ));
            trig.tf.functions.push(func("SetEmitter", &e));
        }
        Tpl::Followers => {
            let f = c.fresh_key(Section::Followers, "key_F_");
            let mut fo = Follower { pattern: group("Circle", 6, 120.0).pattern, collision: Some("None".into()), ..Default::default() };
            fo.pattern.normal = Some([0.0, 0.01, 1.0]);
            fo.pattern.pos_offset = Some([0.0, 0.0, 100.0]);
            fo.pattern.x_depends = Some(false);
            c.followers.push((f.clone(), fo));
            trig.tf.functions.push(func("SetFollower", &f));
        }
    }
    c.triggers.push(trig);
    c.triggers.len() - 1
}

/// A new default instance in section `s`; returns its key.
pub fn new_instance(c: &mut Config, s: Section) -> String {
    let k = c.fresh_key(s, s.prefix());
    insert_instance(c, s, k)
}

/// A default instance under key `k` (taken as is).
pub fn insert_instance(c: &mut Config, s: Section, k: String) -> String {
    match s {
        Section::FormIds => c.formids.push((k.clone(), String::new())),
        Section::Homing => c.homing.push((k.clone(), Homing { kind: "ConstSpeed".into(), rotation_time: Some(2.5), ..Default::default() })),
        Section::SpawnGroups => c.spawn_groups.push((k.clone(), group("Single", 1, 0.0))),
        Section::Multicast => {
            let g = c.keys(Section::SpawnGroups).into_iter().next().unwrap_or_else(|| new_instance(c, Section::SpawnGroups));
            c.multicast.push((k.clone(), vec![McItem { spawn_group: g, ..Default::default() }]));
        }
        Section::Emitters => c.emitters.push((k.clone(), Emitter { interval: 1.0, functions: vec![EmFunc { kind: "TriggerFunctions".into(), ..Default::default() }], ..Default::default() })),
        Section::Followers => c.followers.push((k.clone(), Follower { pattern: group("Circle", 4, 100.0).pattern, ..Default::default() })),
    }
    k
}

/// A new function with sensible defaults (instance keys left empty).
pub fn new_func(kind: &str) -> Func {
    let mut f = func(kind, "");
    match func_arg(kind) {
        FuncArg::Num => f.num = NumData { op: "Mul".into(), value: 1.0 },
        FuncArg::Layer => f.arg = "Spell".into(),
        _ => {}
    }
    f
}
