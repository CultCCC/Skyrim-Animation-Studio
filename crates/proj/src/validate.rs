//! Checks a config against what the DLL actually reads (master source and
//! the schema), the load order, and the known issues from the README.

use crate::model::*;
use crate::world::{Lookup, World};
use crate::Sel;
use sds_i18n::{t, tf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Error,
    Warn,
    Info,
}

#[derive(Debug, Clone)]
pub struct Issue {
    pub level: Level,
    pub at: Sel,
    pub msg: String,
}

struct V<'a> {
    c: &'a Config,
    w: Option<&'a World>,
    out: Vec<Issue>,
}

impl V<'_> {
    fn push(&mut self, level: Level, at: &Sel, msg: String) {
        self.out.push(Issue { level, at: at.clone(), msg });
    }

    fn enum_ok(&mut self, at: &Sel, what: &str, v: &Option<String>, allowed: &[&str]) {
        if let Some(v) = v {
            if !allowed.contains(&v.as_str()) {
                self.push(Level::Error, at, tf!("{what}: «{v}» — нет такого значения ({})", "{what}: «{v}» is not a valid value ({})", allowed.join(", ")));
            }
        }
    }

    /// A FormID-valued field; `want` is the record types it should name.
    fn form(&mut self, at: &Sel, what: &str, v: &str, want: Option<&[&[u8; 4]]>) {
        if v.is_empty() {
            self.push(Level::Error, at, tf!("{what}: пусто", "{what}: empty"));
            return;
        }
        if v.starts_with("key_") {
            if !self.c.has_key(Section::FormIds, v) {
                self.push(Level::Error, at, tf!("{what}: ключ {v} не объявлен в FormIDs этого файла", "{what}: key {v} is not declared in this file's FormIDs"));
                return;
            }
        }
        let r = self.c.resolve_form(v).to_string();
        if r.starts_with("key_") {
            return;
        }
        let Some(w) = self.w else { return };
        match w.lookup(&r) {
            Lookup::Found { sig, edid, .. } => {
                if let Some(want) = want {
                    if !want.iter().any(|s| **s == sig) && !(want.contains(&b"ACHR") && &sig == b"NPC_") {
                        let want_s: Vec<String> = want.iter().map(|s| String::from_utf8_lossy(*s).into_owned()).collect();
                        self.push(
                            Level::Warn,
                            at,
                            tf!("{what}: {edid} — это {}, а нужен {}", "{what}: {edid} is a {}, expected {}", String::from_utf8_lossy(&sig), want_s.join("/")),
                        );
                    }
                }
            }
            Lookup::NoPlugin(p) => self.push(Level::Error, at, tf!("{what}: плагин {p} не в порядке загрузки", "{what}: plugin {p} is not in the load order")),
            Lookup::NoRecord => self.push(Level::Error, at, tf!("{what}: в загрузке нет записи {r}", "{what}: no record {r} in the load order")),
            Lookup::Bad => self.push(Level::Error, at, tf!("{what}: «{r}» — не FormID (нужно Plugin.esp|0x123 или 0x123)", "{what}: «{r}» is not a FormID (use Plugin.esp|0x123 or 0x123)")),
            Lookup::Player | Lookup::Unindexed => {}
        }
    }

    fn key_ref(&mut self, at: &Sel, what: &str, s: Section, k: &str) {
        if k.is_empty() {
            self.push(Level::Error, at, tf!("{what}: не выбран ключ из {}", "{what}: no {} key chosen", s.json_name()));
        } else if !self.c.has_key(s, k) {
            self.push(Level::Error, at, tf!("{what}: {k} не найден в {} этого файла", "{what}: {k} not found in this file's {}", s.json_name()));
        }
    }

    fn tf(&mut self, at: &Sel, t: &TriggerFunctions, event: Option<&str>) {
        if t.functions.is_empty() && t.disable_origin != Some(true) {
            self.push(Level::Warn, at, t!("нет ни одной функции", "no functions").into());
        }
        if t.disable_origin == Some(true) && event.is_some_and(|e| e != "ProjAppeared") {
            self.push(Level::Info, at, t!("disableOrigin действует только для события ProjAppeared", "disableOrigin only works for the ProjAppeared event").into());
        }
        for f in &t.functions {
            let what = f.kind.as_str();
            if !FUNCS.iter().any(|(n, _)| *n == f.kind) {
                self.push(Level::Error, at, tf!("неизвестная функция «{what}»", "unknown function «{what}»"));
                continue;
            }
            let a = func_arg(&f.kind);
            match a {
                FuncArg::Homing | FuncArg::Emitter | FuncArg::Follower | FuncArg::Multicast => self.key_ref(at, what, Section::for_arg(a).unwrap(), &f.id),
                FuncArg::Num => {
                    if !NUM_OPS.contains(&f.num.op.as_str()) {
                        self.push(Level::Error, at, tf!("{what}: тип изменения «{}» (нужно Set/Add/Mul)", "{what}: change type «{}» (use Set/Add/Mul)", f.num.op));
                    }
                }
                FuncArg::Form => self.form(at, what, &f.arg, None),
                FuncArg::Explosion => self.form(at, what, &f.arg, Some(&[b"EXPL"])),
                FuncArg::AnimEvent => {
                    if f.arg.trim().is_empty() {
                        self.push(Level::Error, at, tf!("{what}: пустое имя события", "{what}: empty event name"));
                    }
                }
                FuncArg::Layer => {
                    if !LAYERS.contains(&f.arg.as_str()) {
                        self.push(Level::Error, at, tf!("{what}: слой «{}» (нужно Actor/Spell/None)", "{what}: layer «{}» (use Actor/Spell/None)", f.arg));
                    }
                }
                FuncArg::RestoreSpeed | FuncArg::None => {}
            }
        }
    }

    fn pattern(&mut self, at: &Sel, p: &Pattern) {
        let f = &p.figure;
        if !SHAPES.contains(&f.shape.as_str()) {
            self.push(Level::Error, at, tf!("фигура «{}» не существует", "figure «{}» does not exist", f.shape));
        }
        if f.shape != "Single" && f.size.is_none_or(|s| s < 1.0) {
            self.push(Level::Error, at, t!("для фигуры не Single нужен size ≥ 1", "a non-Single figure needs size ≥ 1").into());
        }
        if f.count == Some(0) {
            self.push(Level::Error, at, t!("count должен быть ≥ 1", "count must be ≥ 1").into());
        }
        if f.shape == "Cylinder" {
            self.push(Level::Info, at, t!("Cylinder в README отмечен как недоделанный", "the README marks Cylinder as unfinished").into());
        }
        if let Some(n) = p.normal {
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            if len < 1e-6 {
                self.push(Level::Error, at, t!("normal нулевой длины", "normal has zero length").into());
            } else if n[0].abs() < 1e-9 && n[1].abs() < 1e-9 {
                self.push(
                    Level::Warn,
                    at,
                    t!("normal строго вверх/вниз: фигура не будет поворачиваться (известная ошибка DLL), используй [0, 0.01, 1]", "normal points straight up/down: the figure will not rotate (known DLL issue), use [0, 0.01, 1]").into(),
                );
            }
        }
    }
}

pub fn validate(c: &Config, w: Option<&World>, fmt: CondFormat) -> Vec<Issue> {
    let mut v = V { c, w, out: Vec::new() };
    for n in &c.notes {
        v.push(Level::Warn, &Sel::Root, n.clone());
    }

    for s in Section::ALL {
        for k in c.keys(s) {
            if !(k.starts_with("key_") || (s != Section::FormIds && k.starts_with("0x"))) {
                v.push(Level::Error, &sel_of(s, &k), tf!("ключ {k} должен начинаться с key_", "key {k} must start with key_"));
            }
            if s != Section::FormIds && c.ref_count(s, &k) == 0 {
                v.push(Level::Info, &sel_of(s, &k), tf!("{k} нигде не используется", "{k} is not used anywhere"));
            }
        }
        let keys = c.keys(s);
        for (i, k) in keys.iter().enumerate() {
            if keys[..i].contains(k) {
                v.push(Level::Error, &sel_of(s, k), tf!("ключ {k} объявлен дважды", "key {k} is declared twice"));
            }
        }
    }

    for (k, val) in &c.formids {
        let at = Sel::FormId(k.clone());
        v.form(&at, k, val, None);
    }

    for (k, h) in &c.homing {
        let at = Sel::Homing(k.clone());
        v.enum_ok(&at, "type", &Some(h.kind.clone()), HOMING_TYPES);
        v.enum_ok(&at, "target", &h.target, HOMING_TARGETS);
        v.enum_ok(&at, "aggressive", &h.aggressive, AGGRESSIVE);
        match h.kind.as_str() {
            "ConstSpeed" => match h.rotation_time {
                None => v.push(Level::Error, &at, t!("ConstSpeed: нужен rotationTime (время разворота на 180°)", "ConstSpeed needs rotationTime (time to turn 180°)").into()),
                Some(x) if !(0.5..=100.0).contains(&x) => v.push(Level::Warn, &at, t!("rotationTime вне 0.5–100", "rotationTime outside 0.5–100").into()),
                _ => {}
            },
            "ConstAccel" => match h.acceleration {
                None => v.push(Level::Error, &at, t!("ConstAccel: нужен acceleration", "ConstAccel needs acceleration").into()),
                Some(x) if !(1.0..=300.0).contains(&x) => v.push(Level::Warn, &at, t!("acceleration вне 1–300", "acceleration outside 1–300").into()),
                _ => {}
            },
            _ => {}
        }
        if h.target.as_deref() == Some("Cursor") {
            match h.cursor_angle {
                None => v.push(Level::Error, &at, t!("target Cursor без cursorAngle: DLL прочитает 0° — конус у прицела нулевой, цель почти не захватится. Обычно 5–15", "target Cursor without cursorAngle: the DLL reads 0° — a zero cone at the crosshair, a target is hardly ever captured. Usually 5–15").into()),
                Some(x) if !(0.0..=90.0).contains(&x) => v.push(Level::Warn, &at, t!("cursorAngle вне 0–90", "cursorAngle outside 0–90").into()),
                _ => {}
            }
        }
    }

    for (k, g) in &c.spawn_groups {
        let at = Sel::Group(k.clone());
        v.pattern(&at, &g.pattern);
        v.enum_ok(&at, "rotation", &g.rotation, ROTATIONS);
        v.enum_ok(&at, "sound", &g.sound, SOUNDS);
        if g.rotation.as_deref() == Some("ToTarget") {
            match &g.rotation_target {
                Some(t) => v.key_ref(&at, "rotationTarget", Section::Homing, t),
                None => v.push(Level::Error, &at, t!("ToTarget: нужен rotationTarget (ключ HomingData)", "ToTarget needs rotationTarget (a HomingData key)").into()),
            }
        }
    }

    for (k, items) in &c.multicast {
        let at = Sel::Multicast(k.clone());
        if items.is_empty() {
            v.push(Level::Warn, &at, t!("пустой мультикаст", "empty multicast").into());
        }
        for (i, it) in items.iter().enumerate() {
            let at = Sel::McItem(k.clone(), i);
            v.key_ref(&at, "spawn_group", Section::SpawnGroups, &it.spawn_group);
            match &it.source {
                McSource::Spell(s) if s != "Current" => v.form(&at, "spellID", s, Some(&[b"SPEL"])),
                McSource::Arrow { weap, arrow } => {
                    if weap != "Current" {
                        v.form(&at, "weapID", weap, Some(&[b"WEAP"]));
                    }
                    if let Some(a) = arrow.as_ref().filter(|a| *a != "Current") {
                        v.form(&at, "arrowID", a, None);
                    }
                }
                _ => {}
            }
            v.enum_ok(&at, "HomingDetection", &it.homing_detection, HOMING_DETECTION);
            if let Some(t) = &it.functions {
                v.tf(&at, t, None);
            }
        }
    }

    for (k, e) in &c.emitters {
        let at = Sel::Emitter(k.clone());
        if e.interval <= 0.0 {
            v.push(Level::Warn, &at, t!("interval 0 — функции каждый кадр", "interval 0 — functions every frame").into());
        }
        if e.count.is_some_and(|n| n > 30) {
            v.push(Level::Error, &at, t!("count больше 30 (максимум схемы)", "count above 30 (schema maximum)").into());
        }
        if e.count.is_some() && e.limited != Some(true) {
            v.push(Level::Info, &at, t!("count учитывается только с limited = true", "count only applies with limited = true").into());
        }
        if e.functions.is_empty() {
            v.push(Level::Warn, &at, t!("нет функций", "no functions").into());
        }
        for f in &e.functions {
            match f.kind.as_str() {
                "AccelerateToMaxSpeed" => {
                    if !SPEED_TYPES.contains(&f.speed_type.as_str()) {
                        v.push(Level::Error, &at, tf!("speedType «{}» (Linear/Quadratic/Exponential)", "speedType «{}» (Linear/Quadratic/Exponential)", f.speed_type));
                    }
                }
                "TriggerFunctions" => v.tf(&at, &f.tf, None),
                other => v.push(Level::Error, &at, tf!("неизвестная функция эмиттера «{other}»", "unknown emitter function «{other}»")),
            }
        }
    }

    for (k, f) in &c.followers {
        let at = Sel::Follower(k.clone());
        v.pattern(&at, &f.pattern);
        v.enum_ok(&at, "rounding", &f.rounding, ROUNDING);
        v.enum_ok(&at, "collision", &f.collision, LAYERS);
        if f.rounding.as_deref().is_some_and(|r| r != "None") && f.rounding_r.is_none() {
            v.push(Level::Error, &at, t!("rounding: нужен roundingR (радиус плавания)", "rounding needs roundingR (float radius)").into());
        }
        if f.speed.is_some_and(|s| !(0.0..=20.0).contains(&s)) {
            v.push(Level::Warn, &at, t!("speed вне 0–20", "speed outside 0–20").into());
        }
    }

    for (i, t) in c.triggers.iter().enumerate() {
        let at = Sel::Trigger(i);
        if !EVENTS.contains(&t.event.as_str()) {
            v.push(Level::Error, &at, tf!("неизвестное событие «{}»", "unknown event «{}»", t.event));
        }
        if t.conditions.is_empty() {
            v.push(Level::Warn, &at, t!("нет условий: DLL с GitHub такой триггер не запускает никогда", "no conditions: the GitHub DLL never fires such a trigger").into());
        }
        for (ci, cnd) in t.conditions.iter().enumerate() {
            if !CONDITIONS.contains(&cnd.kind.as_str()) {
                v.push(Level::Error, &at, tf!("неизвестное условие «{}»", "unknown condition «{}»", cnd.kind));
                continue;
            }
            if cnd.kind == "Hand" {
                if !HANDS.contains(&cnd.value.as_str()) {
                    v.push(Level::Error, &at, tf!("Hand: «{}» (Both/Left/Right)", "Hand: «{}» (Both/Left/Right)", cnd.value));
                }
            } else {
                v.form(&at, &cnd.kind, &cnd.value, cond_sig(&cnd.kind));
            }
            if fmt == CondFormat::Object {
                if cnd.invert || cnd.or {
                    v.push(Level::Error, &at, tf!("{}: invert/OR не записываются в формате «объект» (старая DLL) — переключи формат на «массив»", "{}: invert/OR cannot be written in the object format (old DLL) — switch the format to array", cnd.kind));
                }
                if t.conditions[..ci].iter().any(|x| x.kind == cnd.kind) {
                    v.push(Level::Error, &at, tf!("{} дважды: в формате «объект» останется только одно", "{} twice: the object format keeps only one", cnd.kind));
                }
            }
            if cnd.kind == "ProjBaseIsFormID" && matches!(t.event.as_str(), "Swing" | "HitMelee" | "HitByMelee" | "Cast") {
                v.push(Level::Info, &at, tf!("ProjBaseIsFormID при событии {} — у этого события нет снаряда", "ProjBaseIsFormID with event {} — that event has no projectile", t.event));
            }
        }
        if t.conditions.last().is_some_and(|c| c.or) {
            v.push(Level::Warn, &at, t!("последнее условие с OR — не с чем объединять", "the last condition has OR — nothing to join it with").into());
        }
        v.tf(&at, &t.tf, Some(&t.event));
    }

    v.out.sort_by_key(|i| i.level);
    v.out
}

pub fn sel_of(s: Section, k: &str) -> Sel {
    let k = k.to_string();
    match s {
        Section::FormIds => Sel::FormId(k),
        Section::Homing => Sel::Homing(k),
        Section::SpawnGroups => Sel::Group(k),
        Section::Multicast => Sel::Multicast(k),
        Section::Emitters => Sel::Emitter(k),
        Section::Followers => Sel::Follower(k),
    }
}
