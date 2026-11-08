//! One NewProjectilesTMP config file (`Data/HomingProjectiles/*.json`).
//!
//! Parsing is lenient: the DLL reads with jsoncpp, which ignores fields it
//! does not know, so a real-world file may carry misplaced or unknown keys.
//! Every unknown key is kept in an `extra` map and written back unchanged,
//! and a note is recorded in [`Config::notes`]. Optional fields stay `None`
//! when the file does not set them, so a round trip writes the same keys.
//!
//! Keys (`key_…`) are scoped to their file in the DLL (`filename + key`),
//! so a config must define every FormID and instance it references.

use serde_json::{Map, Value};

/// How `Triggers[].conditions` is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CondFormat {
    /// `{ "ProjBaseIsFormID": "key_x", "Hand": "Left" }` — the DLL builds of
    /// 2023–2024 (the one SDS ships). All conditions are ANDed; no invert/OR.
    Object,
    /// `[ { "type": "ProjBaseIsFormID", "formID": "key_x", "invert": false, "OR": false } ]`
    /// — GitHub master. CK-style OR chains and invert.
    Array,
}

pub type Extra = Map<String, Value>;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Config {
    /// `FormIDs`: key → `Plugin.esp|0xID`, `0xID` or another key.
    pub formids: Vec<(String, String)>,
    pub homing: Vec<(String, Homing)>,
    pub spawn_groups: Vec<(String, SpawnGroup)>,
    pub multicast: Vec<(String, Vec<McItem>)>,
    pub emitters: Vec<(String, Emitter)>,
    pub followers: Vec<(String, Follower)>,
    pub triggers: Vec<Trigger>,
    /// Unknown root keys, written back as they were.
    pub extra: Extra,
    /// The format the file used (None: it has no conditions).
    pub cond_format: Option<CondFormat>,
    /// Things noticed while reading (unknown keys, wrong types).
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Homing {
    /// `ConstSpeed` | `ConstAccel`.
    pub kind: String,
    /// `Cursor` | `Nearest` (default Nearest).
    pub target: Option<String>,
    pub check_los: Option<bool>,
    /// `Aggressive` | `Hostile` | `Any` (default Hostile).
    pub aggressive: Option<String>,
    pub rotation_time: Option<f64>,
    pub acceleration: Option<f64>,
    pub cursor_angle: Option<f64>,
    pub extra: Extra,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Figure {
    pub shape: String,
    pub count: Option<u32>,
    pub size: Option<f64>,
    pub extra: Extra,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Pattern {
    pub figure: Figure,
    /// Degrees, rotation of the figure in its plane.
    pub plane_rotate: Option<f64>,
    /// Plane normal in caster space (default [0,1,0] = forward).
    pub normal: Option<[f64; 3]>,
    /// Normal follows pitch too (default true).
    pub x_depends: Option<bool>,
    /// Node name the figure centre starts at.
    pub origin: Option<String>,
    pub pos_offset: Option<[f64; 3]>,
    pub extra: Extra,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SpawnGroup {
    pub pattern: Pattern,
    /// `Parallel` | `ToSight` | `ToCenter` | `FromCenter` | `ToTarget`.
    pub rotation: Option<String>,
    /// HomingData key for `ToTarget`.
    pub rotation_target: Option<String>,
    /// `Single` | `Every` | `None`.
    pub sound: Option<String>,
    pub rot_offset: Option<[f64; 2]>,
    pub pos_rnd: Option<[f64; 3]>,
    pub rot_rnd: Option<[f64; 2]>,
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq)]
pub enum McSource {
    /// `spellID`: a form or `Current`.
    Spell(String),
    /// `weapID` (+ optional `arrowID`), each a form or `Current`.
    Arrow { weap: String, arrow: Option<String> },
}

impl Default for McSource {
    fn default() -> Self {
        McSource::Spell("Current".into())
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct McItem {
    pub spawn_group: String,
    pub source: McSource,
    /// Run the Triggers on the spawned projectiles (default false).
    pub call_triggers: Option<bool>,
    pub functions: Option<TriggerFunctions>,
    /// `Individual` | `Evenly`.
    pub homing_detection: Option<String>,
    pub extra: Extra,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Emitter {
    pub interval: f64,
    pub limited: Option<bool>,
    pub count: Option<u32>,
    pub destroy_after: Option<bool>,
    pub functions: Vec<EmFunc>,
    pub extra: Extra,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct EmFunc {
    /// `TriggerFunctions` | `AccelerateToMaxSpeed`.
    pub kind: String,
    pub tf: TriggerFunctions,
    pub time: f64,
    /// `Linear` | `Quadratic` | `Exponential`.
    pub speed_type: String,
    pub extra: Extra,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Follower {
    pub pattern: Pattern,
    /// `None` | `Plane` | `Sphere`.
    pub rounding: Option<String>,
    pub rounding_r: Option<f64>,
    /// `Actor` | `Spell` | `None`.
    pub collision: Option<String>,
    pub speed: Option<f64>,
    pub extra: Extra,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Trigger {
    pub event: String,
    pub conditions: Vec<Cond>,
    /// The file wrote `conditions` (an empty list never fires in master).
    pub has_conditions: bool,
    pub tf: TriggerFunctions,
    pub extra: Extra,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Cond {
    pub kind: String,
    /// The FormID / key, or the hand for `Hand`.
    pub value: String,
    pub invert: bool,
    pub or: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TriggerFunctions {
    pub functions: Vec<Func>,
    pub disable_origin: Option<bool>,
    pub extra: Extra,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct NumData {
    /// `Set` | `Add` | `Mul`.
    pub op: String,
    pub value: f64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Func {
    pub kind: String,
    /// Instance key for Set*/ApplyMultiCast.
    pub id: String,
    /// `form` / `explosion` / `event` / `layer`, by kind.
    pub arg: String,
    pub num: NumData,
    pub restore_speed: Option<bool>,
    pub on_followers: Option<bool>,
    pub extra: Extra,
}

/// What a trigger function's parameter is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FuncArg {
    None,
    Homing,
    Emitter,
    Follower,
    Multicast,
    Num,
    RestoreSpeed,
    /// `form`: any form to place.
    Form,
    /// `explosion`: an EXPL.
    Explosion,
    /// `event`: an animation event name.
    AnimEvent,
    /// `layer`: Actor / Spell / None.
    Layer,
}

pub const FUNCS: &[(&str, FuncArg)] = &[
    ("SetHoming", FuncArg::Homing),
    ("SetRotationHoming", FuncArg::Homing),
    ("SetRotationToSight", FuncArg::None),
    ("SetEmitter", FuncArg::Emitter),
    ("SetFollower", FuncArg::Follower),
    ("ApplyMultiCast", FuncArg::Multicast),
    ("ChangeSpeed", FuncArg::Num),
    ("ChangeRange", FuncArg::Num),
    ("DisableHoming", FuncArg::None),
    ("DisableFollower", FuncArg::RestoreSpeed),
    ("DisableEmitter", FuncArg::None),
    ("Placeatme", FuncArg::Form),
    ("SendAnimEvent", FuncArg::AnimEvent),
    ("Explode", FuncArg::Explosion),
    ("SetColLayer", FuncArg::Layer),
];

pub fn func_arg(kind: &str) -> FuncArg {
    FUNCS.iter().find(|(n, _)| *n == kind).map(|(_, a)| *a).unwrap_or(FuncArg::None)
}

impl FuncArg {
    /// The JSON key the parameter is stored under.
    pub fn key(self) -> Option<&'static str> {
        match self {
            FuncArg::Homing | FuncArg::Emitter | FuncArg::Follower | FuncArg::Multicast => Some("id"),
            FuncArg::Num => Some("data"),
            FuncArg::RestoreSpeed => Some("restore_speed"),
            FuncArg::Form => Some("form"),
            FuncArg::Explosion => Some("explosion"),
            FuncArg::AnimEvent => Some("event"),
            FuncArg::Layer => Some("layer"),
            FuncArg::None => None,
        }
    }
}

pub const EVENTS: &[&str] = &[
    "ProjAppeared",
    "Swing",
    "HitMelee",
    "HitByMelee",
    "HitProjectile",
    "HitByProjectile",
    "Cast",
    "EffectStart",
    "EffectEnd",
    "ProjDestroyed",
    "ProjImpact",
];

/// Condition names the DLL source knows (schema lists all but EffectIsFormID/EffectsHasKwd).
pub const CONDITIONS: &[&str] = &[
    "ProjBaseIsFormID",
    "SpellIsFormID",
    "SpellHasKwd",
    "EffectsIsFormID",
    "EffectsHasKwd",
    "EffectIsFormID",
    "EffectHasKwd",
    "CasterIsFormID",
    "CasterBaseIsFormID",
    "CasterHasKwd",
    "WeaponBaseIsFormID",
    "WeaponHasKwd",
    "Hand",
];

pub const SHAPES: &[&str] = &["Single", "Line", "Circle", "HalfCircle", "FillSquare", "FillCircle", "FillHalfCircle", "Sphere", "HalfSphere", "Cylinder"];
pub const ROTATIONS: &[&str] = &["Parallel", "ToSight", "ToCenter", "FromCenter", "ToTarget"];
pub const SOUNDS: &[&str] = &["Single", "Every", "None"];
pub const HOMING_TYPES: &[&str] = &["ConstSpeed", "ConstAccel"];
pub const HOMING_TARGETS: &[&str] = &["Nearest", "Cursor"];
pub const AGGRESSIVE: &[&str] = &["Hostile", "Aggressive", "Any"];
pub const HANDS: &[&str] = &["Both", "Left", "Right"];
pub const NUM_OPS: &[&str] = &["Set", "Add", "Mul"];
pub const LAYERS: &[&str] = &["Actor", "Spell", "None"];
pub const ROUNDING: &[&str] = &["None", "Plane", "Sphere"];
pub const SPEED_TYPES: &[&str] = &["Linear", "Quadratic", "Exponential"];
pub const HOMING_DETECTION: &[&str] = &["Individual", "Evenly"];
pub const EMITTER_FUNCS: &[&str] = &["TriggerFunctions", "AccelerateToMaxSpeed"];

/// What record type a condition's FormID names (None: not a form).
pub fn cond_sig(kind: &str) -> Option<&'static [&'static [u8; 4]]> {
    Some(match kind {
        "ProjBaseIsFormID" => &[b"PROJ"],
        "SpellIsFormID" => &[b"SPEL"],
        "EffectsIsFormID" | "EffectIsFormID" => &[b"MGEF"],
        "SpellHasKwd" | "EffectsHasKwd" | "EffectHasKwd" | "CasterHasKwd" | "WeaponHasKwd" => &[b"KYWD"],
        "CasterIsFormID" => &[b"ACHR"],
        "CasterBaseIsFormID" => &[b"NPC_"],
        "WeaponBaseIsFormID" => &[b"WEAP"],
        _ => return None,
    })
}

// ---------------------------------------------------------------- parsing

/// Strip `//` and `/* */` comments and trailing commas (jsoncpp accepts
/// comments), and a UTF-8 BOM.
pub fn relax(text: &str) -> String {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let b: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let mut in_str = false;
    while i < b.len() {
        let c = b[i];
        if in_str {
            out.push(c);
            if c == '\\' && i + 1 < b.len() {
                out.push(b[i + 1]);
                i += 2;
                continue;
            }
            if c == '"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        match c {
            '"' => {
                in_str = true;
                out.push(c);
            }
            '/' if b.get(i + 1) == Some(&'/') => {
                while i < b.len() && b[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            '/' if b.get(i + 1) == Some(&'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == '*' && b[i + 1] == '/') {
                    i += 1;
                }
                i += 2;
                continue;
            }
            ',' => {
                // Trailing comma: next non-space is } or ].
                let mut j = i + 1;
                while j < b.len() && b[j].is_whitespace() {
                    j += 1;
                }
                if j < b.len() && (b[j] == '}' || b[j] == ']') {
                    i += 1;
                    continue;
                }
                out.push(c);
            }
            _ => out.push(c),
        }
        i += 1;
    }
    out
}

struct Rd<'a> {
    notes: &'a mut Vec<String>,
    path: String,
}

impl Rd<'_> {
    fn at(&mut self, p: &str) -> Rd<'_> {
        Rd { notes: self.notes, path: if self.path.is_empty() { p.to_string() } else { format!("{}.{}", self.path, p) } }
    }
    fn note(&mut self, m: String) {
        let p = if self.path.is_empty() { "root".to_string() } else { self.path.clone() };
        self.notes.push(format!("{p}: {m}"));
    }
    fn obj(&mut self, v: &Value) -> Extra {
        match v {
            Value::Object(m) => m.clone(),
            other => {
                self.note(sds_i18n::tf!("ожидался объект, а не {}", "expected an object, not {}", short(other)));
                Extra::new()
            }
        }
    }
    fn s(&mut self, m: &mut Extra, k: &str) -> Option<String> {
        match m.shift_remove(k) {
            None => None,
            Some(Value::String(s)) => Some(s),
            Some(v) => {
                self.note(sds_i18n::tf!("{k}: ожидалась строка, а не {}", "{k}: expected a string, not {}", short(&v)));
                m.insert(k.into(), v);
                None
            }
        }
    }
    fn b(&mut self, m: &mut Extra, k: &str) -> Option<bool> {
        match m.shift_remove(k) {
            None => None,
            Some(Value::Bool(b)) => Some(b),
            Some(v) => {
                self.note(sds_i18n::tf!("{k}: ожидалось true/false, а не {}", "{k}: expected true/false, not {}", short(&v)));
                m.insert(k.into(), v);
                None
            }
        }
    }
    fn f(&mut self, m: &mut Extra, k: &str) -> Option<f64> {
        match m.shift_remove(k) {
            None => None,
            Some(Value::Number(n)) => n.as_f64(),
            Some(v) => {
                self.note(sds_i18n::tf!("{k}: ожидалось число, а не {}", "{k}: expected a number, not {}", short(&v)));
                m.insert(k.into(), v);
                None
            }
        }
    }
    fn u(&mut self, m: &mut Extra, k: &str) -> Option<u32> {
        self.f(m, k).map(|f| f.max(0.0) as u32)
    }
    fn arr<const N: usize>(&mut self, m: &mut Extra, k: &str) -> Option<[f64; N]> {
        let v = m.shift_remove(k)?;
        if let Value::Array(a) = &v {
            if a.len() == N && a.iter().all(|x| x.is_number()) {
                let mut out = [0.0; N];
                for (o, x) in out.iter_mut().zip(a) {
                    *o = x.as_f64().unwrap_or(0.0);
                }
                return Some(out);
            }
        }
        self.note(sds_i18n::tf!("{k}: ожидался массив из {N} чисел", "{k}: expected an array of {N} numbers"));
        m.insert(k.into(), v);
        None
    }
    fn leftovers(&mut self, m: &Extra) {
        for k in m.keys() {
            let hint = match k.as_str() {
                "sound" if self.path.contains("Figure") => sds_i18n::t!(" — «sound» относится к группе спавна, не к Figure", " — «sound» belongs to the spawn group, not the Figure"),
                _ => "",
            };
            let msg = sds_i18n::tf!("неизвестное поле «{k}», DLL его игнорирует{hint}", "unknown field «{k}», the DLL ignores it{hint}");
            self.note(msg);
        }
    }
}

fn short(v: &Value) -> String {
    let s = v.to_string();
    if s.chars().count() > 40 {
        format!("{}…", s.chars().take(40).collect::<String>())
    } else {
        s
    }
}

impl Config {
    pub fn parse(text: &str) -> Result<Config, String> {
        let v: Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(_) => serde_json::from_str(&relax(text)).map_err(|e| e.to_string())?,
        };
        let mut c = Config::default();
        let mut notes = Vec::new();
        let mut rd = Rd { notes: &mut notes, path: String::new() };
        let mut root = rd.obj(&v);
        let mut fmt = None;

        if let Some(f) = root.shift_remove("FormIDs") {
            let mut r = rd.at("FormIDs");
            for (k, v) in r.obj(&f) {
                match v {
                    Value::String(s) => c.formids.push((k, s)),
                    other => r.note(sds_i18n::tf!("{k}: ожидалась строка FormID, а не {}", "{k}: expected a FormID string, not {}", short(&other))),
                }
            }
        }
        if let Some(h) = root.shift_remove("HomingData") {
            let mut r = rd.at("HomingData");
            for (k, v) in r.obj(&h) {
                let mut r = r.at(&k);
                let x = parse_homing(&mut r, &v);
                c.homing.push((k, x));
            }
        }
        if let Some(g) = root.shift_remove("MulticastSpawnGroups") {
            let mut r = rd.at("MulticastSpawnGroups");
            for (k, v) in r.obj(&g) {
                let mut r = r.at(&k);
                let x = parse_group(&mut r, &v);
                c.spawn_groups.push((k, x));
            }
        }
        if let Some(d) = root.shift_remove("MulticastData") {
            let mut r = rd.at("MulticastData");
            for (k, v) in r.obj(&d) {
                let mut r = r.at(&k);
                let items = match &v {
                    Value::Array(a) => a.iter().enumerate().map(|(i, it)| parse_mc_item(&mut r.at(&i.to_string()), it, &mut fmt)).collect(),
                    other => {
                        r.note(sds_i18n::tf!("ожидался массив групп, а не {}", "expected an array of groups, not {}", short(other)));
                        Vec::new()
                    }
                };
                c.multicast.push((k, items));
            }
        }
        if let Some(e) = root.shift_remove("EmittersData") {
            let mut r = rd.at("EmittersData");
            for (k, v) in r.obj(&e) {
                let mut r = r.at(&k);
                let x = parse_emitter(&mut r, &v, &mut fmt);
                c.emitters.push((k, x));
            }
        }
        if let Some(f) = root.shift_remove("FollowersData") {
            let mut r = rd.at("FollowersData");
            for (k, v) in r.obj(&f) {
                let mut r = r.at(&k);
                let x = parse_follower(&mut r, &v);
                c.followers.push((k, x));
            }
        }
        if let Some(t) = root.shift_remove("Triggers") {
            let mut r = rd.at("Triggers");
            match &t {
                Value::Array(a) => {
                    for (i, it) in a.iter().enumerate() {
                        let mut r = r.at(&i.to_string());
                        let x = parse_trigger(&mut r, it, &mut fmt);
                        c.triggers.push(x);
                    }
                }
                other => r.note(sds_i18n::tf!("ожидался массив, а не {}", "expected an array, not {}", short(other))),
            }
        } else {
            rd.note(sds_i18n::t!("нет раздела Triggers — без него ничего не сработает", "no Triggers section — nothing will fire without it").into());
        }
        for k in root.keys() {
            rd.note(sds_i18n::tf!("неизвестный раздел «{k}» в корне, DLL его игнорирует", "unknown root section «{k}», the DLL ignores it"));
        }
        c.extra = root;
        c.cond_format = fmt;
        c.notes = notes;
        Ok(c)
    }
}

fn parse_homing(r: &mut Rd, v: &Value) -> Homing {
    let mut m = r.obj(v);
    let h = Homing {
        kind: r.s(&mut m, "type").unwrap_or_else(|| "ConstSpeed".into()),
        target: r.s(&mut m, "target"),
        check_los: r.b(&mut m, "checkLOS"),
        aggressive: r.s(&mut m, "aggressive"),
        rotation_time: r.f(&mut m, "rotationTime"),
        acceleration: r.f(&mut m, "acceleration"),
        cursor_angle: r.f(&mut m, "cursorAngle"),
        extra: Extra::new(),
    };
    r.leftovers(&m);
    Homing { extra: m, ..h }
}

fn parse_pattern(r: &mut Rd, m: &mut Extra) -> Pattern {
    let Some(pv) = m.shift_remove("Pattern") else {
        r.note(sds_i18n::t!("нет Pattern", "no Pattern").into());
        return Pattern { figure: Figure { shape: "Single".into(), ..Default::default() }, ..Default::default() };
    };
    let mut r = r.at("Pattern");
    let mut pm = r.obj(&pv);
    let figure = match pm.shift_remove("Figure") {
        Some(fv) => {
            let mut r = r.at("Figure");
            let mut fm = r.obj(&fv);
            let f = Figure {
                shape: r.s(&mut fm, "shape").unwrap_or_else(|| "Single".into()),
                count: r.u(&mut fm, "count"),
                size: r.f(&mut fm, "size"),
                extra: Extra::new(),
            };
            r.leftovers(&fm);
            Figure { extra: fm, ..f }
        }
        None => {
            r.note(sds_i18n::t!("нет Figure", "no Figure").into());
            Figure { shape: "Single".into(), ..Default::default() }
        }
    };
    let p = Pattern {
        figure,
        plane_rotate: r.f(&mut pm, "planeRotate"),
        normal: r.arr::<3>(&mut pm, "normal"),
        x_depends: r.b(&mut pm, "xDepends"),
        origin: r.s(&mut pm, "origin"),
        pos_offset: r.arr::<3>(&mut pm, "posOffset"),
        extra: Extra::new(),
    };
    r.leftovers(&pm);
    Pattern { extra: pm, ..p }
}

fn parse_group(r: &mut Rd, v: &Value) -> SpawnGroup {
    let mut m = r.obj(v);
    let pattern = parse_pattern(r, &mut m);
    let g = SpawnGroup {
        pattern,
        rotation: r.s(&mut m, "rotation"),
        rotation_target: r.s(&mut m, "rotationTarget"),
        sound: r.s(&mut m, "sound"),
        rot_offset: r.arr::<2>(&mut m, "rotOffset"),
        pos_rnd: r.arr::<3>(&mut m, "posRnd"),
        rot_rnd: r.arr::<2>(&mut m, "rotRnd"),
        extra: Extra::new(),
    };
    r.leftovers(&m);
    SpawnGroup { extra: m, ..g }
}

fn parse_follower(r: &mut Rd, v: &Value) -> Follower {
    let mut m = r.obj(v);
    let pattern = parse_pattern(r, &mut m);
    let f = Follower {
        pattern,
        rounding: r.s(&mut m, "rounding"),
        rounding_r: r.f(&mut m, "roundingR"),
        collision: r.s(&mut m, "collision"),
        speed: r.f(&mut m, "speed"),
        extra: Extra::new(),
    };
    r.leftovers(&m);
    Follower { extra: m, ..f }
}

fn parse_tf(r: &mut Rd, v: &Value, _fmt: &mut Option<CondFormat>) -> TriggerFunctions {
    let mut m = r.obj(v);
    let mut functions = Vec::new();
    match m.shift_remove("functions") {
        Some(Value::Array(a)) => {
            for (i, f) in a.iter().enumerate() {
                functions.push(parse_func(&mut r.at(&format!("functions.{i}")), f));
            }
        }
        Some(other) => r.note(sds_i18n::tf!("functions: ожидался массив, а не {}", "functions: expected an array, not {}", short(&other))),
        None => r.note(sds_i18n::t!("нет functions", "no functions").into()),
    }
    let disable_origin = r.b(&mut m, "disableOrigin");
    r.leftovers(&m);
    TriggerFunctions { functions, disable_origin, extra: m }
}

fn parse_func(r: &mut Rd, v: &Value) -> Func {
    let mut m = r.obj(v);
    let kind = r.s(&mut m, "type").unwrap_or_default();
    if !FUNCS.iter().any(|(n, _)| *n == kind) {
        r.note(sds_i18n::tf!("неизвестная функция «{kind}»", "unknown function «{kind}»"));
    }
    let mut f = Func { kind: kind.clone(), on_followers: r.b(&mut m, "on_followers"), ..Default::default() };
    match func_arg(&kind) {
        FuncArg::Homing | FuncArg::Emitter | FuncArg::Follower | FuncArg::Multicast => f.id = r.s(&mut m, "id").unwrap_or_default(),
        FuncArg::Num => {
            if let Some(d) = m.shift_remove("data") {
                let mut r = r.at("data");
                let mut dm = r.obj(&d);
                f.num = NumData { op: r.s(&mut dm, "type").unwrap_or_else(|| "Set".into()), value: r.f(&mut dm, "value").unwrap_or(0.0) };
                r.leftovers(&dm);
            } else {
                r.note(sds_i18n::t!("нет data", "no data").into());
                f.num = NumData { op: "Set".into(), value: 0.0 };
            }
        }
        FuncArg::RestoreSpeed => f.restore_speed = r.b(&mut m, "restore_speed"),
        a @ (FuncArg::Form | FuncArg::Explosion | FuncArg::AnimEvent | FuncArg::Layer) => f.arg = r.s(&mut m, a.key().unwrap()).unwrap_or_default(),
        FuncArg::None => {}
    }
    r.leftovers(&m);
    f.extra = m;
    f
}

fn parse_trigger(r: &mut Rd, v: &Value, fmt: &mut Option<CondFormat>) -> Trigger {
    let mut m = r.obj(v);
    let event = r.s(&mut m, "event").unwrap_or_else(|| "ProjAppeared".into());
    let mut conditions = Vec::new();
    let has_conditions = m.contains_key("conditions");
    match m.shift_remove("conditions") {
        Some(Value::Object(o)) => {
            *fmt = Some(CondFormat::Object);
            for (k, v) in o {
                let value = match v {
                    Value::String(s) => s,
                    other => other.to_string(),
                };
                conditions.push(Cond { kind: k, value, invert: false, or: false });
            }
        }
        Some(Value::Array(a)) => {
            *fmt = Some(CondFormat::Array);
            for (i, c) in a.iter().enumerate() {
                let mut r = r.at(&format!("conditions.{i}"));
                let mut cm = r.obj(c);
                let kind = r.s(&mut cm, "type").unwrap_or_default();
                let value = if kind == "Hand" { r.s(&mut cm, "hand") } else { r.s(&mut cm, "formID") }.unwrap_or_default();
                let invert = r.b(&mut cm, "invert").unwrap_or(false);
                let or = r.b(&mut cm, "OR").unwrap_or(false);
                r.leftovers(&cm);
                conditions.push(Cond { kind, value, invert, or });
            }
        }
        Some(other) => r.note(sds_i18n::tf!("conditions: ожидался объект или массив, а не {}", "conditions: expected an object or array, not {}", short(&other))),
        None => {}
    }
    let tf = match m.shift_remove("TriggerFunctions") {
        Some(t) => parse_tf(&mut r.at("TriggerFunctions"), &t, fmt),
        None => {
            r.note(sds_i18n::t!("нет TriggerFunctions", "no TriggerFunctions").into());
            TriggerFunctions::default()
        }
    };
    r.leftovers(&m);
    Trigger { event, conditions, has_conditions, tf, extra: m }
}

fn parse_mc_item(r: &mut Rd, v: &Value, fmt: &mut Option<CondFormat>) -> McItem {
    let mut m = r.obj(v);
    let spawn_group = r.s(&mut m, "spawn_group").unwrap_or_default();
    let source = if m.contains_key("weapID") {
        McSource::Arrow { weap: r.s(&mut m, "weapID").unwrap_or_default(), arrow: r.s(&mut m, "arrowID") }
    } else {
        McSource::Spell(r.s(&mut m, "spellID").unwrap_or_else(|| {
            r.note(sds_i18n::t!("нет ни spellID, ни weapID", "neither spellID nor weapID").into());
            "Current".into()
        }))
    };
    let call_triggers = r.b(&mut m, "callTriggers");
    let functions = m.shift_remove("TriggerFunctions").map(|t| parse_tf(&mut r.at("TriggerFunctions"), &t, fmt));
    let homing_detection = r.s(&mut m, "HomingDetection");
    r.leftovers(&m);
    McItem { spawn_group, source, call_triggers, functions, homing_detection, extra: m }
}

fn parse_emitter(r: &mut Rd, v: &Value, fmt: &mut Option<CondFormat>) -> Emitter {
    let mut m = r.obj(v);
    let interval = r.f(&mut m, "interval").unwrap_or_else(|| {
        r.note(sds_i18n::t!("нет interval", "no interval").into());
        1.0
    });
    let limited = r.b(&mut m, "limited");
    let count = r.u(&mut m, "count");
    let destroy_after = r.b(&mut m, "destroyAfter");
    let mut functions = Vec::new();
    if let Some(Value::Array(a)) = m.shift_remove("functions") {
        for (i, f) in a.iter().enumerate() {
            let mut r = r.at(&format!("functions.{i}"));
            let mut fm = r.obj(f);
            let kind = r.s(&mut fm, "type").unwrap_or_else(|| "TriggerFunctions".into());
            let mut e = EmFunc { kind: kind.clone(), ..Default::default() };
            match kind.as_str() {
                "AccelerateToMaxSpeed" => {
                    e.time = r.f(&mut fm, "time").unwrap_or(1.0);
                    e.speed_type = r.s(&mut fm, "speedType").unwrap_or_else(|| "Linear".into());
                }
                _ => {
                    if let Some(t) = fm.shift_remove("TriggerFunctions") {
                        e.tf = parse_tf(&mut r.at("TriggerFunctions"), &t, fmt);
                    }
                }
            }
            r.leftovers(&fm);
            e.extra = fm;
            functions.push(e);
        }
    } else {
        r.note(sds_i18n::t!("нет массива functions", "no functions array").into());
    }
    r.leftovers(&m);
    Emitter { interval, limited, count, destroy_after, functions, extra: m }
}

// ---------------------------------------------------------------- writing

fn num(f: f64) -> Value {
    if f.fract() == 0.0 && f.abs() < 1e15 {
        Value::from(f as i64)
    } else {
        // Shortest round-trip of the f32 the DLL reads, so 0.1 stays 0.1.
        let s = format!("{}", f as f32);
        s.parse::<f64>().ok().and_then(serde_json::Number::from_f64).map(Value::Number).unwrap_or(Value::from(f))
    }
}

fn arr(a: &[f64]) -> Value {
    Value::Array(a.iter().map(|x| num(*x)).collect())
}

fn put<T>(m: &mut Extra, k: &str, v: &Option<T>, f: impl Fn(&T) -> Value) {
    if let Some(x) = v {
        m.insert(k.into(), f(x));
    }
}

fn extend(m: &mut Extra, extra: &Extra) {
    for (k, v) in extra {
        if !m.contains_key(k) {
            m.insert(k.clone(), v.clone());
        }
    }
}

fn s(v: &String) -> Value {
    Value::String(v.clone())
}

impl Config {
    pub fn to_value(&self, fmt: CondFormat) -> Value {
        let mut root = Extra::new();
        if !self.formids.is_empty() {
            root.insert("FormIDs".into(), Value::Object(self.formids.iter().map(|(k, v)| (k.clone(), s(v))).collect()));
        }
        if !self.homing.is_empty() {
            root.insert("HomingData".into(), Value::Object(self.homing.iter().map(|(k, h)| (k.clone(), homing_value(h))).collect()));
        }
        if !self.spawn_groups.is_empty() {
            root.insert("MulticastSpawnGroups".into(), Value::Object(self.spawn_groups.iter().map(|(k, g)| (k.clone(), group_value(g))).collect()));
        }
        if !self.multicast.is_empty() {
            root.insert(
                "MulticastData".into(),
                Value::Object(self.multicast.iter().map(|(k, items)| (k.clone(), Value::Array(items.iter().map(|i| mc_value(i, fmt)).collect()))).collect()),
            );
        }
        if !self.emitters.is_empty() {
            root.insert("EmittersData".into(), Value::Object(self.emitters.iter().map(|(k, e)| (k.clone(), emitter_value(e, fmt))).collect()));
        }
        if !self.followers.is_empty() {
            root.insert("FollowersData".into(), Value::Object(self.followers.iter().map(|(k, f)| (k.clone(), follower_value(f))).collect()));
        }
        root.insert("Triggers".into(), Value::Array(self.triggers.iter().map(|t| trigger_value(t, fmt)).collect()));
        extend(&mut root, &self.extra);
        Value::Object(root)
    }

    pub fn to_json(&self, fmt: CondFormat) -> String {
        let mut s = serde_json::to_string_pretty(&self.to_value(fmt)).unwrap_or_default();
        s.push('\n');
        s
    }
}

fn homing_value(h: &Homing) -> Value {
    let mut m = Extra::new();
    m.insert("type".into(), s(&h.kind));
    put(&mut m, "target", &h.target, s);
    put(&mut m, "checkLOS", &h.check_los, |b| Value::Bool(*b));
    put(&mut m, "aggressive", &h.aggressive, s);
    put(&mut m, "rotationTime", &h.rotation_time, |f| num(*f));
    put(&mut m, "acceleration", &h.acceleration, |f| num(*f));
    put(&mut m, "cursorAngle", &h.cursor_angle, |f| num(*f));
    extend(&mut m, &h.extra);
    Value::Object(m)
}

fn pattern_value(p: &Pattern) -> Value {
    let mut f = Extra::new();
    f.insert("shape".into(), s(&p.figure.shape));
    put(&mut f, "count", &p.figure.count, |c| Value::from(*c));
    put(&mut f, "size", &p.figure.size, |x| num(*x));
    extend(&mut f, &p.figure.extra);
    let mut m = Extra::new();
    m.insert("Figure".into(), Value::Object(f));
    put(&mut m, "planeRotate", &p.plane_rotate, |x| num(*x));
    put(&mut m, "normal", &p.normal, |a| arr(a));
    put(&mut m, "xDepends", &p.x_depends, |b| Value::Bool(*b));
    put(&mut m, "origin", &p.origin, s);
    put(&mut m, "posOffset", &p.pos_offset, |a| arr(a));
    extend(&mut m, &p.extra);
    Value::Object(m)
}

fn group_value(g: &SpawnGroup) -> Value {
    let mut m = Extra::new();
    m.insert("Pattern".into(), pattern_value(&g.pattern));
    put(&mut m, "rotation", &g.rotation, s);
    put(&mut m, "rotationTarget", &g.rotation_target, s);
    put(&mut m, "sound", &g.sound, s);
    put(&mut m, "rotOffset", &g.rot_offset, |a| arr(a));
    put(&mut m, "posRnd", &g.pos_rnd, |a| arr(a));
    put(&mut m, "rotRnd", &g.rot_rnd, |a| arr(a));
    extend(&mut m, &g.extra);
    Value::Object(m)
}

fn follower_value(f: &Follower) -> Value {
    let mut m = Extra::new();
    m.insert("Pattern".into(), pattern_value(&f.pattern));
    put(&mut m, "rounding", &f.rounding, s);
    put(&mut m, "roundingR", &f.rounding_r, |x| num(*x));
    put(&mut m, "collision", &f.collision, s);
    put(&mut m, "speed", &f.speed, |x| num(*x));
    extend(&mut m, &f.extra);
    Value::Object(m)
}

pub fn tf_value(t: &TriggerFunctions, fmt: CondFormat) -> Value {
    let _ = fmt;
    let mut m = Extra::new();
    m.insert("functions".into(), Value::Array(t.functions.iter().map(func_value).collect()));
    put(&mut m, "disableOrigin", &t.disable_origin, |b| Value::Bool(*b));
    extend(&mut m, &t.extra);
    Value::Object(m)
}

fn func_value(f: &Func) -> Value {
    let mut m = Extra::new();
    m.insert("type".into(), s(&f.kind));
    let a = func_arg(&f.kind);
    match a {
        FuncArg::Homing | FuncArg::Emitter | FuncArg::Follower | FuncArg::Multicast => {
            m.insert("id".into(), s(&f.id));
        }
        FuncArg::Num => {
            let mut d = Extra::new();
            d.insert("type".into(), s(&f.num.op));
            d.insert("value".into(), num(f.num.value));
            m.insert("data".into(), Value::Object(d));
        }
        FuncArg::RestoreSpeed => put(&mut m, "restore_speed", &f.restore_speed, |b| Value::Bool(*b)),
        FuncArg::Form | FuncArg::Explosion | FuncArg::AnimEvent | FuncArg::Layer => {
            m.insert(a.key().unwrap().into(), s(&f.arg));
        }
        FuncArg::None => {}
    }
    put(&mut m, "on_followers", &f.on_followers, |b| Value::Bool(*b));
    extend(&mut m, &f.extra);
    Value::Object(m)
}

fn trigger_value(t: &Trigger, fmt: CondFormat) -> Value {
    let mut m = Extra::new();
    m.insert("event".into(), s(&t.event));
    if !t.conditions.is_empty() || t.has_conditions {
        let c = match fmt {
            CondFormat::Object => Value::Object(t.conditions.iter().map(|c| (c.kind.clone(), s(&c.value))).collect()),
            CondFormat::Array => Value::Array(
                t.conditions
                    .iter()
                    .map(|c| {
                        let mut o = Extra::new();
                        o.insert("type".into(), s(&c.kind));
                        o.insert(if c.kind == "Hand" { "hand" } else { "formID" }.into(), s(&c.value));
                        if c.invert {
                            o.insert("invert".into(), Value::Bool(true));
                        }
                        if c.or {
                            o.insert("OR".into(), Value::Bool(true));
                        }
                        Value::Object(o)
                    })
                    .collect(),
            ),
        };
        m.insert("conditions".into(), c);
    }
    m.insert("TriggerFunctions".into(), tf_value(&t.tf, fmt));
    extend(&mut m, &t.extra);
    Value::Object(m)
}

fn mc_value(i: &McItem, fmt: CondFormat) -> Value {
    let mut m = Extra::new();
    m.insert("spawn_group".into(), s(&i.spawn_group));
    match &i.source {
        McSource::Spell(sp) => {
            m.insert("spellID".into(), s(sp));
        }
        McSource::Arrow { weap, arrow } => {
            m.insert("weapID".into(), s(weap));
            put(&mut m, "arrowID", arrow, s);
        }
    }
    put(&mut m, "callTriggers", &i.call_triggers, |b| Value::Bool(*b));
    if let Some(t) = &i.functions {
        m.insert("TriggerFunctions".into(), tf_value(t, fmt));
    }
    put(&mut m, "HomingDetection", &i.homing_detection, s);
    extend(&mut m, &i.extra);
    Value::Object(m)
}

fn emitter_value(e: &Emitter, fmt: CondFormat) -> Value {
    let mut m = Extra::new();
    m.insert("interval".into(), num(e.interval));
    put(&mut m, "limited", &e.limited, |b| Value::Bool(*b));
    put(&mut m, "count", &e.count, |c| Value::from(*c));
    put(&mut m, "destroyAfter", &e.destroy_after, |b| Value::Bool(*b));
    m.insert(
        "functions".into(),
        Value::Array(
            e.functions
                .iter()
                .map(|f| {
                    let mut o = Extra::new();
                    o.insert("type".into(), s(&f.kind));
                    if f.kind == "AccelerateToMaxSpeed" {
                        o.insert("time".into(), num(f.time));
                        o.insert("speedType".into(), s(&f.speed_type));
                    } else {
                        o.insert("TriggerFunctions".into(), tf_value(&f.tf, fmt));
                    }
                    extend(&mut o, &f.extra);
                    Value::Object(o)
                })
                .collect(),
        ),
    );
    extend(&mut m, &e.extra);
    Value::Object(m)
}

// ---------------------------------------------------------------- helpers

/// Which instance section a key lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    FormIds,
    Homing,
    SpawnGroups,
    Multicast,
    Emitters,
    Followers,
}

impl Section {
    pub const ALL: [Section; 6] = [Section::FormIds, Section::Homing, Section::SpawnGroups, Section::Multicast, Section::Emitters, Section::Followers];

    pub fn json_name(self) -> &'static str {
        match self {
            Section::FormIds => "FormIDs",
            Section::Homing => "HomingData",
            Section::SpawnGroups => "MulticastSpawnGroups",
            Section::Multicast => "MulticastData",
            Section::Emitters => "EmittersData",
            Section::Followers => "FollowersData",
        }
    }

    pub fn for_arg(a: FuncArg) -> Option<Section> {
        Some(match a {
            FuncArg::Homing => Section::Homing,
            FuncArg::Emitter => Section::Emitters,
            FuncArg::Follower => Section::Followers,
            FuncArg::Multicast => Section::Multicast,
            _ => return None,
        })
    }

    /// Suggested key prefix for new instances.
    pub fn prefix(self) -> &'static str {
        match self {
            Section::FormIds => "key_",
            Section::Homing => "key_H_",
            Section::SpawnGroups => "key_SG_",
            Section::Multicast => "key_MC_",
            Section::Emitters => "key_EM_",
            Section::Followers => "key_F_",
        }
    }
}

impl Config {
    pub fn keys(&self, s: Section) -> Vec<String> {
        match s {
            Section::FormIds => self.formids.iter().map(|x| x.0.clone()).collect(),
            Section::Homing => self.homing.iter().map(|x| x.0.clone()).collect(),
            Section::SpawnGroups => self.spawn_groups.iter().map(|x| x.0.clone()).collect(),
            Section::Multicast => self.multicast.iter().map(|x| x.0.clone()).collect(),
            Section::Emitters => self.emitters.iter().map(|x| x.0.clone()).collect(),
            Section::Followers => self.followers.iter().map(|x| x.0.clone()).collect(),
        }
    }

    pub fn has_key(&self, s: Section, k: &str) -> bool {
        self.keys(s).iter().any(|x| x == k)
    }

    /// A key not used in section `s`: `prefix1`, `prefix2`, …
    pub fn fresh_key(&self, s: Section, prefix: &str) -> String {
        let keys = self.keys(s);
        if !prefix.ends_with('_') && !keys.iter().any(|k| k == prefix) {
            return prefix.to_string();
        }
        (1..).map(|i| format!("{prefix}{i}")).find(|k| !keys.contains(k)).unwrap()
    }

    /// Resolve a FormID-ish string through `FormIDs` (keys may chain once).
    pub fn resolve_form<'a>(&'a self, v: &'a str) -> &'a str {
        let mut cur = v;
        for _ in 0..4 {
            if !cur.starts_with("key_") {
                break;
            }
            match self.formids.iter().find(|(k, _)| k == cur) {
                Some((_, x)) => cur = x,
                None => break,
            }
        }
        cur
    }

    /// Rename a key in section `s` and every reference to it.
    pub fn rename_key(&mut self, s: Section, old: &str, new: &str) {
        if old == new {
            return;
        }
        fn ren(list: &mut [(String, impl Sized)], old: &str, new: &str) {
            for (k, _) in list.iter_mut() {
                if k == old {
                    *k = new.to_string();
                }
            }
        }
        match s {
            Section::FormIds => ren(&mut self.formids, old, new),
            Section::Homing => ren(&mut self.homing, old, new),
            Section::SpawnGroups => ren(&mut self.spawn_groups, old, new),
            Section::Multicast => ren(&mut self.multicast, old, new),
            Section::Emitters => ren(&mut self.emitters, old, new),
            Section::Followers => ren(&mut self.followers, old, new),
        }
        let fix = |x: &mut String| {
            if x == old {
                *x = new.to_string();
            }
        };
        self.walk_refs(&mut |sec, x| {
            if sec == s {
                fix(x)
            }
        });
    }

    /// Visit every string that references a key, with the section it points into.
    pub fn walk_refs(&mut self, f: &mut dyn FnMut(Section, &mut String)) {
        fn tf(t: &mut TriggerFunctions, f: &mut dyn FnMut(Section, &mut String)) {
            for x in &mut t.functions {
                let a = func_arg(&x.kind);
                if let Some(s) = Section::for_arg(a) {
                    f(s, &mut x.id);
                }
                if matches!(a, FuncArg::Form | FuncArg::Explosion) {
                    f(Section::FormIds, &mut x.arg);
                }
            }
        }
        for (_, v) in &mut self.formids {
            f(Section::FormIds, v);
        }
        for t in &mut self.triggers {
            for c in &mut t.conditions {
                if c.kind != "Hand" {
                    f(Section::FormIds, &mut c.value);
                }
            }
            tf(&mut t.tf, f);
        }
        for (_, g) in &mut self.spawn_groups {
            if let Some(r) = &mut g.rotation_target {
                f(Section::Homing, r);
            }
        }
        for (_, items) in &mut self.multicast {
            for i in items {
                f(Section::SpawnGroups, &mut i.spawn_group);
                match &mut i.source {
                    McSource::Spell(sp) => f(Section::FormIds, sp),
                    McSource::Arrow { weap, arrow } => {
                        f(Section::FormIds, weap);
                        if let Some(a) = arrow {
                            f(Section::FormIds, a);
                        }
                    }
                }
                if let Some(t) = &mut i.functions {
                    tf(t, f);
                }
            }
        }
        for (_, e) in &mut self.emitters {
            for x in &mut e.functions {
                tf(&mut x.tf, f);
            }
        }
    }

    /// How many places reference `key` of section `s`.
    pub fn ref_count(&self, s: Section, key: &str) -> usize {
        let mut c = self.clone();
        let mut n = 0;
        c.walk_refs(&mut |sec, x| {
            if sec == s && x == key {
                n += 1
            }
        });
        n
    }
}

impl Default for CondFormat {
    fn default() -> Self {
        CondFormat::Object
    }
}
