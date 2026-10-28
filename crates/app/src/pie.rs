//! Payload Interpreter annotations, resolved to what they show.
//!
//! Syntax (Payload Interpreter README, github.com/D7ry/PayloadInterpreter):
//! commands start with `@`, parameters are separated by `|`, booleans are
//! 0/1. `$Name` aliases live in `SKSE/PayloadInterpreter/Config/*.ini` as
//! `$Name = @COMMAND|...`; repeating a name on several lines maps it to
//! several instructions. `![seconds]@COMMAND|...` delays an instruction.
//! Visual commands handled here:
//! - `@CAST` / `@CASTSPELL` (and SDS's `@CASTRIGHT` / `@CASTLEFT`): casting
//!   art on the hand, hit art on the caster when self-targeting, else on the
//!   target, enchant art on the weapon;
//! - `@APPLYSPELL`: hit / enchant art on the actor until `@UNAPPLYSPELL`;
//! - `@PLAYPARTICLE|nif|body part|scale|play time|flags|x|y|z`.

use sds_records::fx::{ArtSlot, SpellVisuals};
use sds_records::Db;
use sds_vfs::Vfs;
use std::collections::HashMap;

/// Parameter types of a command, as the README states them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ArgKind {
    Text,
    FormId,
    Plugin,
    Float,
    Int,
    Bool,
    /// uint32 written with a `U` suffix (`4U`).
    Flags,
    BodyPart,
}

/// (name, kind) per parameter after the command.
pub fn schema(op: &str) -> Option<&'static [(sds_i18n::Tr, ArgKind)]> {
    use ArgKind::*;
    Some(match op.to_ascii_uppercase().as_str() {
        "@SGVB" => &[(sds_i18n::tr!("переменная графа", "graph variable"), Text), (sds_i18n::tr!("значение (bool)", "value (bool)"), Bool)],
        "@SGVF" => &[(sds_i18n::tr!("переменная графа", "graph variable"), Text), (sds_i18n::tr!("значение (float)", "value (float)"), Float)],
        "@SGVI" => &[(sds_i18n::tr!("переменная графа", "graph variable"), Text), (sds_i18n::tr!("значение (int)", "value (int)"), Int)],
        "@CAST" | "@CASTSPELL" | "@CASTRIGHT" | "@CASTLEFT" => &[
            (sds_i18n::tr!("FormID заклинания", "spell FormID"), FormId),
            (sds_i18n::tr!("плагин (.esp/.esm/.esl)", "plugin (.esp/.esm/.esl)"), Plugin),
            (sds_i18n::tr!("эффективность", "effectiveness"), Float),
            (sds_i18n::tr!("магнитуда", "magnitude"), Float),
            (sds_i18n::tr!("на себя", "on self"), Bool),
            (sds_i18n::tr!("требование здоровья", "health requirement"), Float),
            (sds_i18n::tr!("стоимость здоровья", "health cost"), Float),
            (sds_i18n::tr!("требование выносливости", "stamina requirement"), Float),
            (sds_i18n::tr!("стоимость выносливости", "stamina cost"), Float),
            (sds_i18n::tr!("требование магии", "magicka requirement"), Float),
            (sds_i18n::tr!("стоимость магии", "magicka cost"), Float),
        ],
        "@APPLYSPELL" | "@UNAPPLYSPELL" => &[(sds_i18n::tr!("FormID заклинания", "spell FormID"), FormId), (sds_i18n::tr!("плагин", "plugin"), Plugin)],
        "@SETGHOST" => &[(sds_i18n::tr!("неуязвим (призрак)", "invulnerable (ghost)"), Bool)],
        "@PLAYPARTICLE" => &[
            (sds_i18n::tr!("путь NIF от meshes/", "NIF path from meshes/"), Text),
            (sds_i18n::tr!("часть тела (индекс)", "body part (index)"), BodyPart),
            (sds_i18n::tr!("масштаб NIF", "NIF scale"), Float),
            (sds_i18n::tr!("время проигрывания, с", "play time, s"), Float),
            (sds_i18n::tr!("флаги (uint32, напр. 4U)", "flags (uint32, e.g. 4U)"), Flags),
            (sds_i18n::tr!("смещение X", "offset X"), Float),
            (sds_i18n::tr!("смещение Y", "offset Y"), Float),
            (sds_i18n::tr!("смещение Z", "offset Z"), Float),
        ],
        "@CAMSHAKE" => &[(sds_i18n::tr!("сила", "strength"), Float), (sds_i18n::tr!("длительность, с", "duration, s"), Float)],
        _ => return None,
    })
}

pub fn command_help(op: &str) -> &'static str {
    match op.to_ascii_uppercase().as_str() {
        "@SGVB" | "@SGVF" | "@SGVI" => sds_i18n::t!("Ставит переменную графа поведения (bool / float / int).", "Sets a behavior graph variable (bool / float / int)."),
        "@CAST" | "@CASTSPELL" => sds_i18n::t!("Кастует заклинание с актёра.", "Casts a spell from the actor."),
        "@CASTRIGHT" | "@CASTLEFT" => sds_i18n::t!("Кастует заклинание с правой / левой руки (вариант сборки).", "Casts a spell from the right / left hand (build variant)."),
        "@APPLYSPELL" => sds_i18n::t!("Мгновенно накладывает эффект заклинания на актёра; может остаться на нём.", "Instantly applies the spell's effect to the actor; it may stay on."),
        "@UNAPPLYSPELL" => sds_i18n::t!("Снимает эффект заклинания с актёра.", "Removes the spell's effect from the actor."),
        "@SETGHOST" => sds_i18n::t!("1 — актёр неуязвим (не получает удары и заклинания, но сам бьёт), 0 — снять.", "1 — the actor is invulnerable (takes no hits or spells but still attacks), 0 — clear."),
        "@PLAYPARTICLE" => sds_i18n::t!("Проигрывает NIF на части тела (только двуногие). Индексы 33+ — оружие (README советует их).", "Plays a NIF on a body part (bipeds only). Indices 33+ are weapons (the README recommends them)."),
        "@CAMSHAKE" => sds_i18n::t!("Тряска камеры (параметры в README не описаны; по примерам — сила и длительность).", "Camera shake (parameters are not documented in the README; by the examples — strength and duration)."),
        _ => sds_i18n::t!("Команда не из README — без визуала.", "Not a README command — no visuals."),
    }
}

/// Biped body part index (BIPED_OBJECT) → skeleton node, for @PLAYPARTICLE.
pub fn body_part(index: i64) -> (&'static str, &'static str) {
    match index {
        0 => (sds_i18n::t!("голова", "head"), "NPC Head [Head]"),
        1 => (sds_i18n::t!("волосы", "hair"), "NPC Head [Head]"),
        2 => (sds_i18n::t!("тело", "body"), "NPC Spine2 [Spn2]"),
        // README: kHands actually points to the head.
        3 => (sds_i18n::t!("руки (в игре — голова)", "hands (in game — the head)"), "NPC Head [Head]"),
        4 => (sds_i18n::t!("предплечья", "forearms"), "NPC R Forearm [RLar]"),
        5 => (sds_i18n::t!("амулет", "amulet"), "NPC Neck [Neck]"),
        6 => (sds_i18n::t!("кольцо", "ring"), "NPC R Hand [RHnd]"),
        7 => (sds_i18n::t!("ступни", "feet"), "NPC R Foot [Rft ]"),
        8 => (sds_i18n::t!("икры", "calves"), "NPC R Calf [RClf]"),
        9 => (sds_i18n::t!("щит", "shield"), "SHIELD"),
        10 => (sds_i18n::t!("хвост", "tail"), "NPC Root [Root]"),
        11 => (sds_i18n::t!("длинные волосы", "long hair"), "NPC Head [Head]"),
        12 => (sds_i18n::t!("венец", "circlet"), "NPC Head [Head]"),
        13 => (sds_i18n::t!("уши", "ears"), "NPC Head [Head]"),
        20 | 21 => (sds_i18n::t!("обезглавливание", "decapitation"), "NPC Neck [Neck]"),
        31 => ("FX01", "NPC Root [Root]"),
        33 => (sds_i18n::t!("одноручный меч", "one-handed sword"), "WEAPON"),
        34 => (sds_i18n::t!("кинжал", "dagger"), "WEAPON"),
        35 => (sds_i18n::t!("одноручный топор", "one-handed axe"), "WEAPON"),
        36 => (sds_i18n::t!("булава", "mace"), "WEAPON"),
        37 => (sds_i18n::t!("двуручное", "two-handed"), "WEAPON"),
        38 => (sds_i18n::t!("лук", "bow"), "SHIELD"),
        39 => (sds_i18n::t!("посох", "staff"), "WEAPON"),
        40 => (sds_i18n::t!("арбалет", "crossbow"), "WEAPON"),
        41 => (sds_i18n::t!("колчан", "quiver"), "QUIVER"),
        _ => ("?", "NPC Root [Root]"),
    }
}

#[derive(Default)]
pub struct Aliases {
    /// Alias without `$`, exact case → its instructions, and the ini file.
    pub map: HashMap<String, (Vec<String>, String)>,
    pub files: usize,
}

impl Aliases {
    pub fn load(vfs: &Vfs) -> Aliases {
        let mut a = Aliases::default();
        let mut files = vfs.paths(|p| p.starts_with("skse/payloadinterpreter/config/") && p.ends_with(".ini"));
        files.sort();
        for f in files {
            let Ok(bytes) = vfs.read(&f) else { continue };
            a.files += 1;
            for line in String::from_utf8_lossy(&bytes).lines() {
                let line = line.trim().trim_start_matches('\u{feff}');
                let Some(rest) = line.strip_prefix('$') else { continue };
                let Some((name, value)) = rest.split_once('=') else { continue };
                let e = a.map.entry(name.trim().to_string()).or_insert_with(|| (Vec::new(), f.clone()));
                e.0.push(value.trim().to_string());
            }
        }
        a
    }

    pub fn get(&self, name: &str) -> Option<&(Vec<String>, String)> {
        self.map.get(name).or_else(|| self.map.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v))
    }
}

/// Where an art piece is shown.
#[derive(Debug, Clone, PartialEq)]
pub enum Anchor {
    /// A node of the actor's skeleton (case-insensitive).
    Node(String),
    /// The target dummy.
    Target,
}

#[derive(Debug, Clone)]
pub struct Piece {
    pub slot: ArtSlot,
    pub model: String,
    pub anchor: Anchor,
    pub effect: String,
    /// Absolute clip time it appears at (annotation time + delay).
    pub start: f32,
    /// Seconds it stays visible.
    pub life: f32,
    pub scale: f32,
    pub offset: [f32; 3],
}

/// One instruction of a cue (an alias can expand to several).
#[derive(Debug, Clone)]
pub struct Instruction {
    pub delay: f32,
    pub command: String,
}

#[derive(Debug, Clone)]
pub struct Cue {
    pub index: usize,
    pub time: f32,
    pub instructions: Vec<Instruction>,
    pub alias_file: Option<String>,
    pub spells: Vec<SpellVisuals>,
    pub pieces: Vec<Piece>,
    /// Effect sounds (MGEF SNDD) the instructions play.
    pub sounds: Vec<CueSound>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CueSound {
    pub time: f32,
    pub sound: sds_records::FormKey,
    /// "выпуск: SNDR_EDID (эффект)".
    pub label: String,
}

const RIGHT_MAGIC: &str = "NPC R MagicNode [RMag]";
const LEFT_MAGIC: &str = "NPC L MagicNode [LMag]";
const ROOT: &str = "NPC Root [Root]";

pub fn parse_hex(s: &str) -> Option<u32> {
    let s = s.trim();
    let s = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")).unwrap_or(s);
    u32::from_str_radix(s, 16).ok()
}

/// `![1.5]@CMD|...` → (1.5, "@CMD|...").
pub fn split_delay(s: &str) -> (f32, &str) {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix("![") {
        if let Some((n, cmd)) = rest.split_once(']') {
            return (n.trim().parse().unwrap_or(0.0), cmd.trim());
        }
    }
    (0.0, s)
}

fn meshes_path(p: &str) -> String {
    let p = p.trim().replace('\\', "/");
    if p.to_ascii_lowercase().starts_with("meshes/") {
        p
    } else {
        format!("meshes/{p}")
    }
}

/// Every PIE annotation of a clip, resolved. `duration` = clip length.
pub fn resolve(items: &[(f32, &str)], duration: f32, aliases: &Aliases, db: Option<&Db>) -> Vec<Cue> {
    let mut cues: Vec<Cue> = Vec::new();
    for (index, (time, text)) in items.iter().enumerate() {
        let Some(body) = text.trim().strip_prefix("PIE.").or_else(|| text.trim().strip_prefix("pie.")) else { continue };
        let mut cue = Cue { index, time: *time, instructions: Vec::new(), alias_file: None, spells: Vec::new(), pieces: Vec::new(), sounds: Vec::new(), notes: Vec::new() };
        // Expand aliases (one level of nesting is enough for real configs; deeper is guarded).
        let mut queue: Vec<(f32, String, usize)> = vec![(0.0, body.to_string(), 0)];
        while let Some((delay, raw, depth)) = queue.pop() {
            let (d, cmd) = split_delay(&raw);
            let delay = delay + d;
            if let Some(name) = cmd.strip_prefix('$') {
                match aliases.get(name) {
                    Some((list, f)) if depth < 4 => {
                        cue.alias_file.get_or_insert_with(|| f.clone());
                        for c in list.iter().rev() {
                            queue.push((delay, c.clone(), depth + 1));
                        }
                    }
                    Some(_) => cue.notes.push(sds_i18n::tf!("${name}: слишком глубокая вложенность алиасов", "${name}: alias nesting too deep")),
                    None => cue.notes.push(sds_i18n::tf!("алиас ${name} не найден в SKSE/PayloadInterpreter/Config", "alias ${name} not found in SKSE/PayloadInterpreter/Config")),
                }
            } else {
                cue.instructions.push(Instruction { delay, command: cmd.to_string() });
            }
        }
        let instructions = cue.instructions.clone();
        for ins in &instructions {
            let parts: Vec<&str> = ins.command.split('|').map(str::trim).collect();
            let op = parts.first().map(|s| s.to_ascii_uppercase()).unwrap_or_default();
            let at = *time + ins.delay;
            let spell_of = |cue: &mut Cue| -> Option<SpellVisuals> {
                let (id, plugin) = (parts.get(1).and_then(|s| parse_hex(s)), parts.get(2));
                let (Some(id), Some(plugin)) = (id, plugin) else {
                    cue.notes.push(sds_i18n::tf!("{op}: нет формы заклинания", "{op}: no spell form"));
                    return None;
                };
                let db = db?;
                match db.find(plugin, id).and_then(|k| db.spell_visuals(k)) {
                    Some(s) => Some(s),
                    None => {
                        cue.notes.push(sds_i18n::tf!("заклинание 0x{id:06X} из {plugin} не найдено (или это не SPEL/ENCH)", "spell 0x{id:06X} from {plugin} not found (or not a SPEL/ENCH)"));
                        None
                    }
                }
            };
            match op.as_str() {
                "@CAST" | "@CASTSPELL" | "@CASTRIGHT" | "@CASTLEFT" => {
                    if let Some(s) = spell_of(&mut cue) {
                        let self_target = parts.get(5).is_some_and(|v| v.trim() == "1");
                        let hand = if op == "@CASTLEFT" { LEFT_MAGIC } else { RIGHT_MAGIC };
                        // Release on the cast; the hit sound too when it lands on
                        // the caster (a projectile's flight is not modelled).
                        for (effect, kind, sk) in &s.sounds {
                            if *kind == 3 || (*kind == 5 && self_target) {
                                let edid = db.and_then(|d| d.get(*sk)).map(|r| r.edid()).unwrap_or_default();
                                cue.sounds.push(CueSound { time: at, sound: *sk, label: format!("{}: {edid} ({effect})", sds_records::sound::mgef_sound_slot(*kind)) });
                            }
                        }
                        for v in &s.visuals {
                            let (anchor, life) = match v.slot {
                                ArtSlot::Casting => (Anchor::Node(hand.into()), 1.0),
                                ArtSlot::Hit if self_target => (Anchor::Node(ROOT.into()), if v.duration > 0.0 { v.duration } else { 3.0 }),
                                ArtSlot::Hit => (Anchor::Target, if v.duration > 0.0 { v.duration } else { 2.0 }),
                                ArtSlot::Enchant => (Anchor::Node("WEAPON".into()), if v.duration > 0.0 { v.duration } else { 3.0 }),
                            };
                            cue.pieces.push(Piece { slot: v.slot, model: v.model.clone(), anchor, effect: v.effect.clone(), start: at, life: life.min(duration.max(1.0) + 5.0), scale: 1.0, offset: [0.0; 3] });
                        }
                        if !self_target {
                            cue.notes.push(sds_i18n::t!("не на себя: арт эффекта показан на цели (снаряд не моделируется)", "not on self: the effect art is shown on the target (projectile not modelled)").into());
                        }
                        cue.spells.push(s);
                    }
                }
                "@APPLYSPELL" => {
                    if let Some(s) = spell_of(&mut cue) {
                        for v in &s.visuals {
                            if v.slot == ArtSlot::Casting {
                                continue;
                            }
                            let anchor = if v.slot == ArtSlot::Enchant { Anchor::Node("WEAPON".into()) } else { Anchor::Node(ROOT.into()) };
                            cue.pieces.push(Piece { slot: v.slot, model: v.model.clone(), anchor, effect: v.effect.clone(), start: at, life: f32::INFINITY, scale: 1.0, offset: [0.0; 3] });
                        }
                        cue.spells.push(s);
                    }
                }
                "@UNAPPLYSPELL" => {
                    let key = (parts.get(1).and_then(|s| parse_hex(s)), parts.get(2).map(|s| s.to_ascii_lowercase()));
                    let mut ended = 0;
                    for c in cues.iter_mut().chain(std::iter::once(&mut cue)) {
                        let applies: Vec<f32> = c
                            .instructions
                            .iter()
                            .filter(|i| {
                                let cp: Vec<&str> = i.command.split('|').map(str::trim).collect();
                                cp.first().is_some_and(|o| o.eq_ignore_ascii_case("@APPLYSPELL"))
                                    && (cp.get(1).and_then(|s| parse_hex(s)), cp.get(2).map(|s| s.to_ascii_lowercase())) == key
                            })
                            .map(|i| c.time + i.delay)
                            .collect();
                        for p in c.pieces.iter_mut().filter(|p| p.life.is_infinite() && applies.contains(&p.start) && p.start <= at) {
                            p.life = (at - p.start).max(0.0);
                            ended += 1;
                        }
                    }
                    cue.notes.push(sds_i18n::tf!("@UNAPPLYSPELL снимает {ended} арт", "@UNAPPLYSPELL removes {ended} art"));
                }
                "@PLAYPARTICLE" => {
                    let path = parts.get(1).map(|p| meshes_path(p)).unwrap_or_default();
                    let part: i64 = parts.get(2).and_then(|v| v.trim().parse().ok()).unwrap_or(0);
                    let num = |i: usize, d: f32| parts.get(i).and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(d);
                    let (part_name, node) = body_part(part);
                    let life = num(4, 0.0);
                    cue.pieces.push(Piece {
                        slot: ArtSlot::Hit,
                        model: path,
                        anchor: Anchor::Node(node.into()),
                        effect: sds_i18n::tf!("@PLAYPARTICLE, часть тела {part} ({part_name})", "@PLAYPARTICLE, body part {part} ({part_name})"),
                        start: at,
                        life: if life > 0.0 { life } else { 2.0 },
                        scale: num(3, 1.0),
                        offset: [num(6, 0.0), num(7, 0.0), num(8, 0.0)],
                    });
                    if part < 33 {
                        cue.notes.push(sds_i18n::t!("README: индексы не-оружия частично неверны (kHands указывает на голову) — советуют 33+", "README: non-weapon indices are partly wrong (kHands points to the head) — 33+ recommended").into());
                    }
                }
                "" => {}
                other => cue.notes.push(sds_i18n::tf!("{other}: без визуала", "{other}: no visuals")),
            }
        }
        cues.push(cue);
    }
    // APPLYSPELL art not removed lasts to the clip end.
    for c in &mut cues {
        for p in &mut c.pieces {
            if p.life.is_infinite() {
                p.life = (duration - p.start).max(0.0);
            }
        }
    }
    cues
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_delays_and_unapply() {
        let mut a = Aliases::default();
        a.map.insert("Blade".into(), (vec!["@APPLYSPELL|0x1|X.esp".into(), "![0.5]@SETGHOST|1".into()], "x.ini".into()));
        let items = [(0.1, "PIE.$Blade"), (0.2, "PIE.@SGVI|MCO_nextattack|1"), (0.9, "PIE.@UNAPPLYSPELL|0x000001|x.esp"), (1.0, "PIE.$Missing")];
        let cues = resolve(&items, 2.0, &a, None);
        assert_eq!(cues[0].instructions.len(), 2);
        assert_eq!(cues[0].instructions[1].delay, 0.5);
        assert!(cues[1].notes[0].contains(sds_i18n::t!("без визуала", "no visuals")));
        assert!(cues[3].notes[0].contains(sds_i18n::t!("не найден", "not found")));
        assert_eq!(split_delay("![2.5]@SETGHOST|0"), (2.5, "@SETGHOST|0"));
        assert_eq!(schema("@CAST").unwrap()[4].0 .0, sds_i18n::t!("на себя", "on self"));
    }

    #[test]
    fn playparticle_on_weapon() {
        let items = [(0.3, "PIE.@PLAYPARTICLE|ValhallaCombat/block_Counter.nif|33|1.5|0.8|4U|0|10|0")];
        let cues = resolve(&items, 2.0, &Aliases::default(), None);
        let p = &cues[0].pieces[0];
        assert_eq!((p.model.as_str(), p.anchor.clone(), p.scale, p.life), ("meshes/ValhallaCombat/block_Counter.nif", Anchor::Node("WEAPON".into()), 1.5, 0.8));
    }
}
