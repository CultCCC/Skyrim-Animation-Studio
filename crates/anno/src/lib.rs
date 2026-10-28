//! What an annotation means to the mods SDS runs.
//!
//! [`parse`] classifies one annotation text and pulls out its parameters;
//! [`spans`] pairs the openers with what closes them (combo windows,
//! Precision collisions, SDSCombat locks) so the timeline can draw bars.
//! Syntax sources: Precision 2.0.4, Payload Interpreter 1.1, SCAR 2.0,
//! MCO/BFCO, AMR, and `SDS DLL\SDSCombat_Guide_RU.md`.

pub mod precision;
pub mod sdscombat;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Category {
    Precision,
    Pie,
    SdsCombat,
    Scar,
    Combo,
    Attack,
    Sound,
    Foot,
    Motion,
    Other,
}

impl Category {
    pub const ALL: [Category; 10] = [
        Category::Precision,
        Category::Pie,
        Category::SdsCombat,
        Category::Scar,
        Category::Combo,
        Category::Attack,
        Category::Sound,
        Category::Foot,
        Category::Motion,
        Category::Other,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Category::Precision => "Precision",
            Category::Pie => "PIE / FX",
            Category::SdsCombat => "SDSCombat",
            Category::Scar => "SCAR",
            Category::Combo => "MCO / BFCO",
            Category::Attack => sds_i18n::t!("Атака (движок)", "Attack (engine)"),
            Category::Sound => sds_i18n::t!("Звук", "Sound"),
            Category::Foot => sds_i18n::t!("Шаги", "Footsteps"),
            Category::Motion => "Root motion",
            Category::Other => sds_i18n::t!("Прочее", "Other"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScarAction {
    pub idle: String,
    pub min_distance: f32,
    pub max_distance: f32,
    pub start_angle: f32,
    pub end_angle: f32,
    pub chance: f32,
    pub ty: String,
    /// The other keys SCAR 2.0 reads (WeaponLength, TriggerStartTime,
    /// TriggerEndTime, CoolDownAlias, CoolDownTime, VariantID,
    /// ConditionsAlias, ...), kept in file order.
    pub extra: Vec<(String, serde_json::Value)>,
}

/// SCAR 2.0's action types (SCARActionData.cpp): right / left / bash / dual,
/// each normal or power; IDLE plays `IdleAnimation` directly.
pub const SCAR_TYPES: &[(&str, sds_i18n::Tr)] = &[
    ("RA", sds_i18n::tr!("правая атака", "right attack")),
    ("RPA", sds_i18n::tr!("правая силовая", "right power attack")),
    ("LA", sds_i18n::tr!("левая атака", "left attack")),
    ("LPA", sds_i18n::tr!("левая силовая", "left power attack")),
    ("BA", sds_i18n::tr!("удар щитом", "shield bash")),
    ("BPA", sds_i18n::tr!("силовой удар щитом", "power shield bash")),
    ("DA", sds_i18n::tr!("двойная атака", "dual attack")),
    ("DPA", sds_i18n::tr!("двойная силовая", "dual power attack")),
    ("IDLE", sds_i18n::tr!("idle-анимация", "idle animation")),
];

/// Other ActionData keys SCAR reads, with what they hold.
pub const SCAR_EXTRA_KEYS: &[(&str, sds_i18n::Tr)] = &[
    ("WeaponLength", sds_i18n::tr!("учитывать длину оружия в дистанции", "add weapon length to the distance")),
    ("TriggerStartTime", sds_i18n::tr!("с какого времени клипа действие доступно, с", "clip time the action becomes available from, s")),
    ("TriggerEndTime", sds_i18n::tr!("до какого времени клипа, с", "clip time it stays available until, s")),
    ("CoolDownTime", sds_i18n::tr!("перезарядка действия, с", "action cooldown, s")),
    ("CoolDownAlias", sds_i18n::tr!("общая перезарядка по имени", "shared cooldown by name")),
    ("VariantID", sds_i18n::tr!("номер варианта (SCAR_AttackVariants)", "variant number (SCAR_AttackVariants)")),
    ("ConditionsAlias", sds_i18n::tr!("условия по имени", "conditions by name")),
];

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    /// AMR: cumulative translation from the clip start, NPC space (+Y ahead).
    AnimMotion([f32; 3]),
    /// AMR: cumulative yaw in degrees.
    AnimRotation(f32),
    /// `Collision_Add.Node(WEAPON).Duration(0.3)`: event + `Name(value)` params.
    Precision { event: String, params: Vec<(String, String)> },
    /// `PIE.@SGVI|MCO_nextattack|2` (command + args) or `PIE.$Alias` (ini alias).
    Pie { command: String, args: Vec<String> },
    ScarAction(ScarAction),
    /// `SDS_NPC_TurnSpeed.0.45`, `SDS_NPC_ReachTarget.150|40|190|170`.
    SdsNpc { event: String, values: Vec<f32> },
    Sound { descriptor: String },
    Plain,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Parsed {
    pub category: Category,
    /// Event name: the text up to its first parameter.
    pub name: String,
    pub kind: Kind,
}

pub fn parse(text: &str) -> Parsed {
    let t = text.trim();
    let lower = t.to_ascii_lowercase();

    if let Some(rest) = lower.strip_prefix("animmotion") {
        let v: Vec<f32> = rest.split_whitespace().filter_map(|x| x.parse().ok()).collect();
        if v.len() >= 3 {
            return parsed(Category::Motion, "animmotion", Kind::AnimMotion([v[0], v[1], v[2]]));
        }
    }
    if let Some(rest) = lower.strip_prefix("animrotation") {
        if let Some(d) = rest.split_whitespace().next().and_then(|x| x.parse().ok()) {
            return parsed(Category::Motion, "animrotation", Kind::AnimRotation(d));
        }
    }
    if lower.starts_with("collision_") {
        let (event, params) = dotted_params(t);
        return parsed(Category::Precision, &event, Kind::Precision { event: event.clone(), params });
    }
    if let Some(body) = t.strip_prefix("PIE.").or_else(|| t.strip_prefix("pie.")) {
        let mut parts = body.split('|');
        let command = parts.next().unwrap_or("").to_string();
        let args = parts.map(str::to_string).collect();
        return parsed(Category::Pie, &format!("PIE.{command}"), Kind::Pie { command, args });
    }
    if let Some(json) = t.strip_prefix("SCAR_ActionData") {
        if let Some(a) = scar_action(json) {
            return parsed(Category::Scar, "SCAR_ActionData", Kind::ScarAction(a));
        }
        return parsed(Category::Scar, "SCAR_ActionData", Kind::Plain);
    }
    if lower.starts_with("scar_") {
        return parsed(Category::Scar, t, Kind::Plain);
    }
    // SDSCombat also takes SDS_NPCBumpOff / SDS_NPCBumpOn for its
    // SDS_NPC_BumpOff / SDS_NPC_BumpOn (TargetLock.cpp).
    if lower.starts_with("sds_npcbump") {
        let event = format!("SDS_NPC_Bump{}", &t["SDS_NPCBump".len()..]);
        return parsed(Category::SdsCombat, &event, Kind::SdsNpc { event: event.clone(), values: Vec::new() });
    }
    if lower.starts_with("sds_npc_") {
        let (event, rest) = match t.split_once('.') {
            Some((e, r)) => (e.to_string(), r),
            None => (t.to_string(), ""),
        };
        let values = if rest.is_empty() {
            Vec::new()
        } else if rest.contains('|') {
            rest.split('|').filter_map(|x| x.trim().parse().ok()).collect()
        } else {
            rest.parse().ok().into_iter().collect()
        };
        return parsed(Category::SdsCombat, &event, Kind::SdsNpc { event: event.clone(), values });
    }
    if lower.starts_with("soundplay") {
        let descriptor = t.split_once('.').map(|(_, d)| d.to_string()).unwrap_or_default();
        return parsed(Category::Sound, "SoundPlay", Kind::Sound { descriptor });
    }
    if lower.starts_with("mco_") || lower.starts_with("bfco_") || lower.starts_with("scsi_") {
        return parsed(Category::Combo, t, Kind::Plain);
    }
    if lower.starts_with("foot") {
        return parsed(Category::Foot, t, Kind::Plain);
    }
    const ATTACK: &[&str] = &[
        "weaponswing", "weaponleftswing", "hitframe", "prehitframe", "attackstop", "attackwinstart",
        "attackwinend", "castokstart", "castokstop", "staggerstart", "bashrelease", "blockstart",
        "blockstop", "arrowattach", "arrowrelease", "bowfulldrawn", "to_draw", "weapondraw",
        "weaponsheathe", "beginweapondraw", "skipbeginend", "powerattack_start_end",
    ];
    if ATTACK.iter().any(|a| lower == *a) {
        return parsed(Category::Attack, t, Kind::Plain);
    }
    parsed(Category::Other, t, Kind::Plain)
}

fn parsed(category: Category, name: &str, kind: Kind) -> Parsed {
    Parsed { category, name: name.to_string(), kind }
}

/// `Event.Name(value).Other(value)` → ("Event", [(Name, value), ...]).
fn dotted_params(t: &str) -> (String, Vec<(String, String)>) {
    // `Event.Name(value).Flag.Other(a, b)`: values may hold dots (`0.3`),
    // flags have no parentheses (`NoRecoil`).
    let (event, rest) = match t.find(['.', '|']) {
        Some(i) => (t[..i].to_string(), &t[i + 1..]),
        None => return (t.to_string(), Vec::new()),
    };
    let mut params = Vec::new();
    let mut chars = rest.chars().peekable();
    let mut name = String::new();
    while let Some(c) = chars.next() {
        match c {
            '(' => {
                let mut value = String::new();
                for v in chars.by_ref() {
                    if v == ')' {
                        break;
                    }
                    value.push(v);
                }
                params.push((name.trim().to_string(), value.trim().to_string()));
                name.clear();
            }
            // Precision reads `|` between parameters as well (SDS clips use it).
            '.' | '|' => {
                if !name.trim().is_empty() {
                    params.push((name.trim().to_string(), String::new()));
                }
                name.clear();
            }
            _ => name.push(c),
        }
    }
    if !name.trim().is_empty() {
        params.push((name.trim().to_string(), String::new()));
    }
    (event, params)
}

fn scar_action(json: &str) -> Option<ScarAction> {
    let v: serde_json::Value = serde_json::from_str(json.trim()).ok()?;
    let num = |k: &str| -> f32 {
        match v.get(k) {
            Some(serde_json::Value::Number(n)) => n.as_f64().unwrap_or(0.0) as f32,
            Some(serde_json::Value::String(s)) => s.parse().unwrap_or(0.0),
            _ => 0.0,
        }
    };
    let text = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    const CORE: [&str; 7] = ["IdleAnimation", "MinDistance", "MaxDistance", "StartAngle", "EndAngle", "Chance", "Type"];
    let extra = v
        .as_object()
        .map(|o| o.iter().filter(|(k, _)| !CORE.contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();
    Some(ScarAction {
        idle: text("IdleAnimation"),
        min_distance: num("MinDistance"),
        max_distance: num("MaxDistance"),
        start_angle: num("StartAngle"),
        end_angle: num("EndAngle"),
        chance: num("Chance"),
        ty: text("Type"),
        extra,
    })
}

fn json_num(x: f32) -> String {
    if x.fract() == 0.0 && x.abs() < 1e9 {
        format!("{}", x as i64)
    } else {
        let s = format!("{x:.3}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

impl ScarAction {
    /// Back to annotation text, in the field order SCAR's own examples use,
    /// extra keys after.
    pub fn to_text(&self) -> String {
        let mut s = format!(
            "SCAR_ActionData{{\"IdleAnimation\":{}, \"MinDistance\":{}, \"MaxDistance\":{}, \"StartAngle\":{}, \"EndAngle\":{}, \"Chance\":{}, \"Type\":{}",
            serde_json::Value::String(self.idle.clone()),
            json_num(self.min_distance),
            json_num(self.max_distance),
            json_num(self.start_angle),
            json_num(self.end_angle),
            json_num(self.chance),
            serde_json::Value::String(self.ty.clone())
        );
        for (k, v) in &self.extra {
            s.push_str(&format!(", {}:{}", serde_json::Value::String(k.clone()), v));
        }
        s.push('}');
        s
    }

    pub fn extra_f32(&self, key: &str) -> Option<f32> {
        self.extra.iter().find(|(k, _)| k == key).and_then(|(_, v)| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))).map(|x| x as f32)
    }

    /// Target at `distance` and `angle` (degrees, + = to the NPC's right,
    /// −180..180) inside this action's ring sector. Angles wrap when
    /// StartAngle > EndAngle (a sector across the back).
    pub fn contains(&self, distance: f32, angle: f32) -> bool {
        if distance < self.min_distance || distance > self.max_distance {
            return false;
        }
        let (s, e) = (self.start_angle, self.end_angle);
        if e - s >= 360.0 {
            return true;
        }
        let norm = |a: f32| ((a % 360.0) + 360.0) % 360.0;
        let span = norm(e - s);
        norm(angle - s) <= span + 1e-4
    }

    /// Clip-time window, when TriggerStartTime / TriggerEndTime are set.
    pub fn in_window(&self, t: f32) -> bool {
        let start = self.extra_f32("TriggerStartTime");
        let end = self.extra_f32("TriggerEndTime");
        start.is_none_or(|s| t >= s) && end.is_none_or(|e| e <= 0.0 || t <= e)
    }
}

/// A bar on the timeline: an opener and what ends it.
#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub category: Category,
    pub label: String,
    /// Index of the opening annotation.
    pub open: usize,
    /// Index of the closing annotation; `None` = runs to `end`.
    pub close: Option<usize>,
    pub start: f32,
    pub end: f32,
}

/// Pair openers with closers. `items` are (time, text) sorted by time.
pub fn spans(items: &[(f32, &str)], duration: f32) -> Vec<Span> {
    let parsed: Vec<Parsed> = items.iter().map(|(_, t)| parse(t)).collect();
    let mut out = Vec::new();
    let lc = |i: usize| items[i].1.trim().to_ascii_lowercase();

    // Name-pair windows: the first matching closer after the opener.
    const PAIRS: &[(&str, &[&str], Category)] = &[
        ("mco_winopen", &["mco_winclose"], Category::Combo),
        ("mco_powerwinopen", &["mco_powerwinclose"], Category::Combo),
        ("bfco_nextwinstart", &["bfco_nextwinend", "mco_winclose"], Category::Combo),
        ("bfco_nextpowerwinstart", &["bfco_nextpowerwinend", "mco_powerwinclose"], Category::Combo),
        ("castokstart", &["castokstop"], Category::Attack),
        ("sds_npc_reachtarget", &["sds_npc_reachtargetend"], Category::SdsCombat),
        // Animation Motion Fix's movement magnetism off until BumpOn / clip end.
        ("sds_npc_bumpoff", &["sds_npc_bumpon"], Category::SdsCombat),
    ];
    for (i, p) in parsed.iter().enumerate() {
        let name = p.name.to_ascii_lowercase();
        for (open, closes, cat) in PAIRS {
            if name == *open {
                let close = (i + 1..items.len()).find(|&j| closes.contains(&parsed[j].name.to_ascii_lowercase().as_str()));
                push(&mut out, *cat, &p.name, i, close, items, duration);
            }
        }
    }

    // SDSCombat lock: from TargetLockOn/TurnSpeed until Off, StopRotation or Reset.
    let mut lock_open: Option<usize> = None;
    for (i, p) in parsed.iter().enumerate() {
        let name = p.name.to_ascii_lowercase();
        match name.as_str() {
            "sds_npc_targetlockon" | "sds_npc_turnspeed" => {
                if lock_open.is_none() {
                    lock_open = Some(i);
                }
            }
            "sds_npc_targetlockoff" | "sds_npc_stoprotation" | "sds_npc_targetlockreset" => {
                if let Some(o) = lock_open.take() {
                    push(&mut out, Category::SdsCombat, "TargetLock", o, Some(i), items, duration);
                }
            }
            _ => {}
        }
    }
    if let Some(o) = lock_open {
        push(&mut out, Category::SdsCombat, "TargetLock", o, None, items, duration);
    }

    // Precision: an Add lives for Duration(x), or until a Remove on its node,
    // or until the attack ends.
    for (i, p) in parsed.iter().enumerate() {
        let Kind::Precision { event, params } = &p.kind else { continue };
        if !event.eq_ignore_ascii_case("collision_add") {
            continue;
        }
        let param = |k: &str| params.iter().find(|(n, _)| n.eq_ignore_ascii_case(k)).map(|(_, v)| v.as_str());
        let node = param("node").unwrap_or("?").to_string();
        let label = format!("Collision {node}");
        if let Some(d) = param("duration").and_then(|d| d.parse::<f32>().ok()).filter(|d| *d > 0.0) {
            let start = items[i].0;
            out.push(Span { category: Category::Precision, label, open: i, close: None, start, end: start + d });
            continue;
        }
        let close = (i + 1..items.len()).find(|&j| match &parsed[j].kind {
            Kind::Precision { event, params } => {
                let e = event.to_ascii_lowercase();
                (e == "collision_remove" && params.iter().any(|(n, v)| n.eq_ignore_ascii_case("node") && *v == node))
                    || e == "collision_attackend"
                    || e == "collision_cancel"
            }
            _ => matches!(lc(j).as_str(), "attackstop" | "staggerstart"),
        });
        push(&mut out, Category::Precision, &label, i, close, items, duration);
    }
    out
}

fn push(out: &mut Vec<Span>, category: Category, label: &str, open: usize, close: Option<usize>, items: &[(f32, &str)], duration: f32) {
    let start = items[open].0;
    let end = close.map(|c| items[c].0).unwrap_or(duration.max(start));
    out.push(Span { category, label: label.to_string(), open, close, start, end });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds() {
        assert_eq!(parse("animmotion 0.0 146.912 0.0").kind, Kind::AnimMotion([0.0, 146.912, 0.0]));
        assert_eq!(parse("PIE.@SGVI|MCO_nextattack|2").kind, Kind::Pie { command: "@SGVI".into(), args: vec!["MCO_nextattack".into(), "2".into()] });
        assert_eq!(parse("PIE.$SDSSNEKBladeVFX").name, "PIE.$SDSSNEKBladeVFX");
        let p = parse("Collision_Add.Node(WEAPON).Duration(0.3)");
        assert_eq!(p.kind, Kind::Precision { event: "Collision_Add".into(), params: vec![("Node".into(), "WEAPON".into()), ("Duration".into(), "0.3".into())] });
        let q = parse("Collision_Add.Node(NPC R Calf [RClf])|ID(1)|Scale(2)");
        assert_eq!(
            q.kind,
            Kind::Precision { event: "Collision_Add".into(), params: vec![("Node".into(), "NPC R Calf [RClf]".into()), ("ID".into(), "1".into()), ("Scale".into(), "2".into())] }
        );
        let p = parse("Collision_Add.Node(NPC R Hand [RHnd]).NoRecoil.Translation(0, 5.5, 0)");
        assert_eq!(
            p.kind,
            Kind::Precision {
                event: "Collision_Add".into(),
                params: vec![("Node".into(), "NPC R Hand [RHnd]".into()), ("NoRecoil".into(), "".into()), ("Translation".into(), "0, 5.5, 0".into())]
            }
        );
        assert_eq!(parse("SDS_NPC_ReachTarget.150|40|190|170").kind, Kind::SdsNpc { event: "SDS_NPC_ReachTarget".into(), values: vec![150.0, 40.0, 190.0, 170.0] });
        assert_eq!(parse("SDS_NPC_TargetLockOn.0.45").kind, Kind::SdsNpc { event: "SDS_NPC_TargetLockOn".into(), values: vec![0.45] });
        let Kind::ScarAction(a) = parse(r#"SCAR_ActionData{"IdleAnimation":"ADXP_NPCPowerAttack", "MinDistance":0, "MaxDistance":1220.0, "StartAngle":-120, "EndAngle":120, "Chance":100, "Type":"RPA"}"#).kind else { panic!() };
        assert_eq!((a.max_distance, a.start_angle, a.ty.as_str()), (1220.0, -120.0, "RPA"));
        assert_eq!(parse(&a.to_text()).kind, Kind::ScarAction(a.clone()));
        assert!(a.contains(500.0, 90.0) && !a.contains(500.0, 150.0) && !a.contains(1300.0, 0.0));
        let Kind::ScarAction(b) = parse(r#"SCAR_ActionData{"IdleAnimation":"X","MinDistance":0,"MaxDistance":300,"StartAngle":150,"EndAngle":-150,"Chance":50,"Type":"IDLE","TriggerStartTime":0.4,"CoolDownTime":3}"#).kind else { panic!() };
        // Sector across the back: 150° → −150°.
        assert!(b.contains(100.0, 180.0) && b.contains(100.0, -170.0) && !b.contains(100.0, 0.0));
        assert!(!b.in_window(0.2) && b.in_window(0.5));
        assert!(b.to_text().ends_with(r#""TriggerStartTime":0.4, "CoolDownTime":3}"#), "{}", b.to_text());
    }

    #[test]
    fn pairs() {
        let items = [
            (0.1, "SDS_NPC_TargetLockOn.0.5"),
            (0.2, "Collision_Add.Node(WEAPON)"),
            (0.3, "MCO_WinOpen"),
            (0.5, "Collision_Remove.Node(WEAPON)"),
            (0.6, "SDS_NPC_StopRotation"),
            (0.9, "MCO_WinClose"),
        ];
        let s = spans(&items, 1.0);
        let find = |l: &str| s.iter().find(|x| x.label.starts_with(l)).map(|x| (x.start, x.end)).unwrap();
        assert_eq!(find("TargetLock"), (0.1, 0.6));
        assert_eq!(find("Collision"), (0.2, 0.5));
        assert_eq!(find("MCO_WinOpen"), (0.3, 0.9));
    }

    #[test]
    fn bump_pair_and_alias() {
        assert_eq!(parse("SDS_NPCBumpOff").category, Category::SdsCombat);
        assert_eq!(parse("SDS_NPCBumpOff").name, "SDS_NPC_BumpOff");
        let items = [(0.2, "SDS_NPCBumpOff"), (0.9, "SDS_NPC_BumpOn"), (1.2, "SDS_NPC_BumpOff")];
        let s = spans(&items, 2.0);
        let b: Vec<(f32, f32)> = s.iter().filter(|s| s.label == "SDS_NPC_BumpOff").map(|s| (s.start, s.end)).collect();
        assert_eq!(b, vec![(0.2, 0.9), (1.2, 2.0)]);
    }
}
