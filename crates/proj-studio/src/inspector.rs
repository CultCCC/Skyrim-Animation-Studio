//! The right panel: a typed form for whatever is selected.
//!
//! Optional fields show their default when unset; changing one writes it,
//! ↺ removes it from the JSON again. Each editor works on a copy of its
//! element and hands new keys / instances back through [`Pending`].

use crate::app::{App, PickReq};
use crate::theme;
use egui::{Id, RichText};
use sds_i18n::{t, tf, Tr};
use sds_proj::chain;
use sds_proj::model::*;
use sds_proj::world::{Lookup, World};
use sds_proj::Sel;

/// Read-only view of the config while one element is edited.
struct Snap {
    formids: Vec<(String, String)>,
    keys: Vec<(Section, Vec<String>)>,
}

impl Snap {
    fn keys(&self, s: Section) -> &[String] {
        self.keys.iter().find(|(x, _)| *x == s).map(|(_, k)| k.as_slice()).unwrap_or(&[])
    }
}

#[derive(Default)]
struct Pending {
    formids: Vec<(String, String)>,
    create: Vec<(Section, String)>,
    nav: Option<Sel>,
}

struct Ix<'a> {
    world: Option<&'a World>,
    picked: &'a mut Option<(Id, String)>,
    picker: &'a mut Option<PickReq>,
    fmt: CondFormat,
    auto_keys: bool,
    snap: Snap,
    p: Pending,
}

impl Ix<'_> {
    fn fresh(&self, s: Section, prefix: &str) -> String {
        let used = |k: &str| self.snap.keys(s).iter().any(|x| x == k) || self.p.create.iter().any(|(ss, x)| *ss == s && x == k) || (s == Section::FormIds && self.p.formids.iter().any(|(x, _)| x == k));
        if !prefix.ends_with('_') && !used(prefix) {
            return prefix.to_string();
        }
        (1..).map(|i| format!("{prefix}{i}")).find(|k| !used(k)).unwrap()
    }
}

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let world = app.world.clone();
    let fmt = match app.doc() {
        Some(d) => app.export_format(d),
        None => CondFormat::Object,
    };
    let auto_keys = app.settings.auto_keys;
    let foreign = app.foreign.clone();
    let App { docs, cur, picked, picker, preview, .. } = app;
    let Some(doc) = docs.get_mut(*cur) else {
        ui.heading(t!("Инспектор", "Inspector"));
        ui.label(RichText::new(t!("Ничего не открыто", "Nothing open")).color(theme::DIM));
        return;
    };
    let cfg = &mut doc.cfg;
    let snap = Snap { formids: cfg.formids.clone(), keys: Section::ALL.iter().map(|s| (*s, cfg.keys(*s))).collect() };
    let mut ix = Ix { world: world.as_deref(), picked, picker, fmt, auto_keys, snap, p: Pending::default() };
    let sel = doc.sel.clone();

    egui::ScrollArea::vertical().id_salt("insp").auto_shrink([false, false]).show(ui, |ui| {
        match sel.owner().clone() {
            Sel::Root => root(ui, &mut ix, cfg, &doc.name, &doc.origin, doc.file_format),
            Sel::Trigger(i) => {
                if let Some(mut t) = cfg.triggers.get(i).cloned() {
                    let focus = if let Sel::Func(_, f) = &sel { Some(*f) } else { None };
                    trigger(ui, &mut ix, i, &mut t, focus);
                    cfg.triggers[i] = t;
                }
            }
            Sel::Homing(k) => {
                if let Some(pos) = cfg.homing.iter().position(|(x, _)| *x == k) {
                    header(ui, cfg, Section::Homing, &k, t!("Самонаведение", "Homing"), &mut ix);
                    let mut h = cfg.homing[pos].1.clone();
                    homing(ui, &mut h);
                    cfg.homing[pos].1 = h;
                }
            }
            Sel::Group(k) => {
                if let Some(pos) = cfg.spawn_groups.iter().position(|(x, _)| *x == k) {
                    header(ui, cfg, Section::SpawnGroups, &k, t!("Группа спавна", "Spawn group"), &mut ix);
                    let mut g = cfg.spawn_groups[pos].1.clone();
                    group(ui, &mut ix, &mut g, preview);
                    cfg.spawn_groups[pos].1 = g;
                }
            }
            Sel::Multicast(k) | Sel::McItem(k, _) => {
                if let Some(pos) = cfg.multicast.iter().position(|(x, _)| *x == k) {
                    header(ui, cfg, Section::Multicast, &k, t!("Мультикаст", "Multicast"), &mut ix);
                    let focus = if let Sel::McItem(_, i) = &sel { Some(*i) } else { None };
                    let mut items = cfg.multicast[pos].1.clone();
                    let groups: Vec<(String, SpawnGroup)> = cfg.spawn_groups.clone();
                    multicast(ui, &mut ix, &k, &mut items, focus, &groups, preview);
                    cfg.multicast[pos].1 = items;
                }
            }
            Sel::Emitter(k) | Sel::EmFunc(k, _) => {
                if let Some(pos) = cfg.emitters.iter().position(|(x, _)| *x == k) {
                    header(ui, cfg, Section::Emitters, &k, t!("Эмиттер", "Emitter"), &mut ix);
                    let mut e = cfg.emitters[pos].1.clone();
                    emitter(ui, &mut ix, &k, &mut e);
                    cfg.emitters[pos].1 = e;
                }
            }
            Sel::Follower(k) => {
                if let Some(pos) = cfg.followers.iter().position(|(x, _)| *x == k) {
                    header(ui, cfg, Section::Followers, &k, t!("Спутник", "Follower"), &mut ix);
                    let mut f = cfg.followers[pos].1.clone();
                    follower(ui, &mut f, preview);
                    cfg.followers[pos].1 = f;
                }
            }
            Sel::FormId(k) => {
                if let Some(pos) = cfg.formids.iter().position(|(x, _)| *x == k) {
                    header(ui, cfg, Section::FormIds, &k, "FormID", &mut ix);
                    let mut v = cfg.formids[pos].1.clone();
                    ui.label(t!("Значение: Plugin.esp|0xID (локальный ID записи) или 0xID для Skyrim.esm", "Value: Plugin.esp|0xID (the record's local ID) or 0xID for Skyrim.esm"));
                    form_field(ui, &mut ix, &mut v, None, false, false, "fid-val");
                    cfg.formids[pos].1 = v;
                }
            }
            Sel::Record(k) => {
                record(ui, &mut ix, cfg, k, &foreign);
            }
            Sel::Func(..) => {}
        }
        // Issues of the selection.
        let mine: Vec<_> = doc.issues.iter().filter(|i| i.at.owner() == sel.owner() || matches!((&i.at, sel.owner()), (Sel::McItem(a, _), Sel::Multicast(b)) if a == b)).collect();
        if !mine.is_empty() {
            ui.add_space(8.0);
            ui.separator();
            for i in mine {
                let c = match i.level {
                    sds_proj::validate::Level::Error => theme::ERR,
                    sds_proj::validate::Level::Warn => theme::WARN,
                    sds_proj::validate::Level::Info => theme::DIM,
                };
                ui.label(RichText::new(format!("• {}", i.msg)).color(c));
            }
        }
    });

    // Apply what the editors asked for.
    let p = std::mem::take(&mut ix.p);
    for (k, v) in p.formids {
        if !cfg.has_key(Section::FormIds, &k) {
            cfg.formids.push((k, v));
        }
    }
    for (s, k) in p.create {
        if !cfg.has_key(s, &k) {
            crate::templates::insert_instance(cfg, s, k);
        }
    }
    if let Some(n) = p.nav {
        doc.sel = n;
    }
}

// ------------------------------------------------------------ helpers

fn row(ui: &mut egui::Ui, label: &str, tip: Tr, set: bool) {
    let txt = if set { RichText::new(label) } else { RichText::new(label).color(theme::DIM) };
    ui.label(txt).on_hover_text(tip.get());
}

fn reset<T>(ui: &mut egui::Ui, v: &mut Option<T>) {
    if v.is_some() {
        if ui.small_button("↺").on_hover_text(t!("Сбросить к значению по умолчанию (поле уберётся из JSON)", "Reset to the default (the field leaves the JSON)")).clicked() {
            *v = None;
        }
    }
}

fn combo(ui: &mut egui::Ui, id: impl std::hash::Hash, v: &mut String, opts: &[&str], label: impl Fn(&str) -> String) -> bool {
    let mut changed = false;
    egui::ComboBox::from_id_salt(id).selected_text(label(v)).width(220.0).show_ui(ui, |ui| {
        for o in opts {
            if ui.selectable_label(v == o, label(o)).clicked() {
                *v = o.to_string();
                changed = true;
            }
        }
    });
    changed
}

fn opt_enum(ui: &mut egui::Ui, label: &str, tip: Tr, v: &mut Option<String>, opts: &[&str], default: &str) {
    row(ui, label, tip, v.is_some());
    ui.horizontal(|ui| {
        let mut cur = v.clone().unwrap_or_else(|| default.to_string());
        let shown = v.is_some();
        if combo(ui, ("oe", label), &mut cur, opts, |o| if !shown && o == default { tf!("{o} (по умолч.)", "{o} (default)") } else { o.to_string() }) {
            *v = Some(cur);
        }
        reset(ui, v);
    });
    ui.end_row();
}

fn opt_num(ui: &mut egui::Ui, label: &str, tip: Tr, v: &mut Option<f64>, default: f64, speed: f64, range: std::ops::RangeInclusive<f64>) {
    row(ui, label, tip, v.is_some());
    ui.horizontal(|ui| {
        let mut x = v.unwrap_or(default);
        if ui.add(egui::DragValue::new(&mut x).speed(speed).range(range)).changed() {
            *v = Some(x);
        }
        if v.is_none() {
            ui.label(RichText::new(t!("по умолч.", "default")).small().color(theme::DIM));
        }
        reset(ui, v);
    });
    ui.end_row();
}

fn opt_u32(ui: &mut egui::Ui, label: &str, tip: Tr, v: &mut Option<u32>, default: u32, range: std::ops::RangeInclusive<u32>) {
    row(ui, label, tip, v.is_some());
    ui.horizontal(|ui| {
        let mut x = v.unwrap_or(default);
        if ui.add(egui::DragValue::new(&mut x).speed(0.2).range(range)).changed() {
            *v = Some(x);
        }
        if v.is_none() {
            ui.label(RichText::new(t!("по умолч.", "default")).small().color(theme::DIM));
        }
        reset(ui, v);
    });
    ui.end_row();
}

fn opt_bool(ui: &mut egui::Ui, label: &str, tip: Tr, v: &mut Option<bool>, default: bool) {
    row(ui, label, tip, v.is_some());
    ui.horizontal(|ui| {
        let mut x = v.unwrap_or(default);
        if ui.checkbox(&mut x, if v.is_none() { t!("(по умолч.)", "(default)") } else { "" }).changed() {
            *v = Some(x);
        }
        reset(ui, v);
    });
    ui.end_row();
}

fn opt_vec<const N: usize>(ui: &mut egui::Ui, label: &str, tip: Tr, v: &mut Option<[f64; N]>, default: [f64; N], axes: [&str; N]) {
    row(ui, label, tip, v.is_some());
    ui.horizontal(|ui| {
        let mut x = v.unwrap_or(default);
        let mut ch = false;
        for (i, a) in axes.iter().enumerate() {
            ui.label(RichText::new(*a).small().color(theme::DIM));
            ch |= ui.add(egui::DragValue::new(&mut x[i]).speed(1.0).max_decimals(3)).changed();
        }
        if ch {
            *v = Some(x);
        }
        reset(ui, v);
    });
    ui.end_row();
}

fn opt_text(ui: &mut egui::Ui, label: &str, tip: Tr, v: &mut Option<String>, hint: &str) {
    row(ui, label, tip, v.is_some());
    ui.horizontal(|ui| {
        let mut s = v.clone().unwrap_or_default();
        if ui.add(egui::TextEdit::singleline(&mut s).hint_text(hint).desired_width(220.0)).changed() {
            *v = if s.is_empty() { None } else { Some(s) };
        }
    });
    ui.end_row();
}

/// Key rename row shared by every instance editor.
fn header(ui: &mut egui::Ui, cfg: &mut Config, s: Section, key: &str, title: &str, ix: &mut Ix) {
    ui.heading(title);
    ui.label(RichText::new(s.json_name()).small().color(theme::DIM));
    ui.horizontal(|ui| {
        ui.label(t!("Ключ", "Key"));
        let id = Id::new(("rename", s.json_name(), key));
        let mut buf = ui.data_mut(|d| d.get_temp::<String>(id)).unwrap_or_else(|| key.to_string());
        let r = ui.add(egui::TextEdit::singleline(&mut buf).desired_width(220.0));
        let commit = r.lost_focus();
        ui.data_mut(|d| d.insert_temp(id, buf.clone()));
        if commit {
            ui.data_mut(|d| d.remove::<String>(id));
            let ok = buf.starts_with("key_") && !cfg.has_key(s, &buf);
            if buf != key && ok {
                cfg.rename_key(s, key, &buf);
                ix.p.nav = Some(sds_proj::validate::sel_of(s, &buf));
            }
        }
        let n = cfg.ref_count(s, key);
        ui.label(RichText::new(tf!("ссылок: {n}", "refs: {n}")).small().color(if n == 0 { theme::WARN } else { theme::DIM }));
    });
    ui.horizontal(|ui| {
        if ui.small_button(t!("Дублировать", "Duplicate")).clicked() {
            let nk = cfg.fresh_key(s, &format!("{key}_"));
            match s {
                Section::FormIds => dup(&mut cfg.formids, key, nk.clone()),
                Section::Homing => dup(&mut cfg.homing, key, nk.clone()),
                Section::SpawnGroups => dup(&mut cfg.spawn_groups, key, nk.clone()),
                Section::Multicast => dup(&mut cfg.multicast, key, nk.clone()),
                Section::Emitters => dup(&mut cfg.emitters, key, nk.clone()),
                Section::Followers => dup(&mut cfg.followers, key, nk.clone()),
            }
            ix.p.nav = Some(sds_proj::validate::sel_of(s, &nk));
        }
        if ui.small_button(RichText::new(t!("Удалить", "Delete")).color(theme::ERR)).on_hover_text(t!("Ссылки на ключ останутся и будут отмечены как ошибки", "References stay and show up as errors")).clicked() {
            match s {
                Section::FormIds => cfg.formids.retain(|(k, _)| k != key),
                Section::Homing => cfg.homing.retain(|(k, _)| k != key),
                Section::SpawnGroups => cfg.spawn_groups.retain(|(k, _)| k != key),
                Section::Multicast => cfg.multicast.retain(|(k, _)| k != key),
                Section::Emitters => cfg.emitters.retain(|(k, _)| k != key),
                Section::Followers => cfg.followers.retain(|(k, _)| k != key),
            }
            ix.p.nav = Some(Sel::Root);
        }
    });
    ui.separator();
}

fn dup<T: Clone>(list: &mut Vec<(String, T)>, key: &str, nk: String) {
    if let Some(p) = list.iter().position(|(k, _)| k == key) {
        let v = list[p].1.clone();
        list.insert(p + 1, (nk, v));
    }
}

/// A FormID-valued field: a key of FormIDs, a raw FormID, or `Current`.
fn form_field(ui: &mut egui::Ui, ix: &mut Ix, v: &mut String, sigs: Option<&[&[u8; 4]]>, allow_keys: bool, allow_current: bool, salt: &str) {
    let id = ui.id().with(("form", salt));
    if let Some((pid, val)) = ix.picked.take() {
        if pid == id {
            *v = if allow_keys && ix.auto_keys { key_for(ix, &val) } else { val };
        } else {
            *ix.picked = Some((pid, val));
        }
    }
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(v).desired_width(200.0).hint_text("key_… | Plugin.esp|0x…"));
        if allow_keys && !ix.snap.formids.is_empty() {
            egui::ComboBox::from_id_salt(("fk", salt)).selected_text("key").width(40.0).show_ui(ui, |ui| {
                if allow_current && ui.selectable_label(v == "Current", t!("Current — текущий", "Current")).clicked() {
                    *v = "Current".into();
                }
                for (k, val) in &ix.snap.formids {
                    let d = ix.world.map(|w| w.short(val)).unwrap_or_default();
                    if ui.selectable_label(v == k, format!("{k}  {d}")).clicked() {
                        *v = k.clone();
                    }
                }
            });
        } else if allow_current && ui.small_button("Current").clicked() {
            *v = "Current".into();
        }
        if ui.button("🔍").on_hover_text(t!("Выбрать запись из загрузки", "Pick a record from the load order")).clicked() {
            *ix.picker = Some(PickReq { target: id, sigs: sigs.map(|s| s.iter().map(|x| **x).collect()).unwrap_or_default(), query: String::new(), just_opened: true });
        }
    });
    // What it resolves to.
    if v == "Current" {
        ui.label(RichText::new(t!("тот же, что у исходного снаряда", "the same as the origin projectile")).small().color(theme::DIM));
        return;
    }
    let mut resolved = v.clone();
    let mut via = None;
    if v.starts_with("key_") {
        match ix.snap.formids.iter().chain(ix.p.formids.iter()).find(|(k, _)| k == v) {
            Some((_, x)) => {
                via = Some(x.clone());
                resolved = x.clone();
            }
            None => {
                ui.label(RichText::new(t!("ключ не объявлен в FormIDs", "key not declared in FormIDs")).small().color(theme::ERR));
                return;
            }
        }
    }
    let Some(w) = ix.world else { return };
    if resolved.is_empty() {
        return;
    }
    let (txt, col) = match w.lookup(&resolved) {
        Lookup::Found { sig, .. } => {
            let bad = sigs.is_some_and(|s| !s.iter().any(|x| **x == sig) && !(s.contains(&b"ACHR") && &sig == b"NPC_"));
            (w.describe(&resolved), if bad { theme::WARN } else { theme::OK })
        }
        Lookup::Player => (t!("Игрок (0x14)", "Player (0x14)").into(), theme::OK),
        Lookup::Unindexed => (t!("не проверяется (тип не индексируется)", "not checked (type not indexed)").into(), theme::DIM),
        _ => (w.describe(&resolved), theme::ERR),
    };
    let txt = match via {
        Some(x) => format!("{txt}   ({x})"),
        None => txt,
    };
    ui.label(RichText::new(txt).small().color(col));
}

/// The FormIDs key for a picked record: an existing one or key_<EDID>.
fn key_for(ix: &mut Ix, formid: &str) -> String {
    if let Some((k, _)) = ix.snap.formids.iter().chain(ix.p.formids.iter()).find(|(_, v)| v.eq_ignore_ascii_case(formid)) {
        return k.clone();
    }
    let edid = ix.world.and_then(|w| match w.lookup(formid) {
        Lookup::Found { edid, .. } => Some(edid),
        _ => None,
    });
    let base: String = format!("key_{}", edid.unwrap_or_else(|| "Form".into())).chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
    let k = ix.fresh(Section::FormIds, &base);
    ix.p.formids.push((k.clone(), formid.to_string()));
    k
}

/// A reference to an instance key with "new" and "go to" buttons.
fn key_ref(ui: &mut egui::Ui, ix: &mut Ix, s: Section, v: &mut String, salt: &str) {
    ui.horizontal(|ui| {
        let keys: Vec<String> = ix.snap.keys(s).to_vec();
        let txt = if v.is_empty() { t!("(не выбран)", "(none)").to_string() } else { v.clone() };
        egui::ComboBox::from_id_salt(("kr", salt)).selected_text(txt).width(180.0).show_ui(ui, |ui| {
            for k in &keys {
                if ui.selectable_label(v == k, k).clicked() {
                    *v = k.clone();
                }
            }
        });
        if ui.small_button("+").on_hover_text(tf!("Новый экземпляр в {}", "New instance in {}", s.json_name())).clicked() {
            let k = ix.fresh(s, s.prefix());
            ix.p.create.push((s, k.clone()));
            *v = k;
        }
        if !v.is_empty() && ui.small_button("→").on_hover_text(t!("Перейти", "Go to")).clicked() {
            ix.p.nav = Some(sds_proj::validate::sel_of(s, v));
        }
    });
}

// ------------------------------------------------------------ editors

fn root(ui: &mut egui::Ui, ix: &mut Ix, cfg: &mut Config, name: &str, origin: &str, file_fmt: Option<CondFormat>) {
    ui.heading(name);
    ui.label(tf!("Источник: {origin}", "Source: {origin}"));
    ui.label(tf!(
        "Формат условий в файле: {}",
        "Condition format in the file: {}",
        file_fmt.map(crate::app::fmt_name).unwrap_or(t!("нет условий", "no conditions"))
    ));
    ui.label(tf!("Экспорт запишет: {}", "Export writes: {}", crate::app::fmt_name(ix.fmt)));
    ui.separator();
    egui::Grid::new("root-counts").num_columns(2).show(ui, |ui| {
        for (n, l) in [
            (cfg.triggers.len(), t!("триггеров", "triggers")),
            (cfg.homing.len(), "HomingData"),
            (cfg.multicast.len(), "MulticastData"),
            (cfg.spawn_groups.len(), "MulticastSpawnGroups"),
            (cfg.emitters.len(), "EmittersData"),
            (cfg.followers.len(), "FollowersData"),
            (cfg.formids.len(), "FormIDs"),
        ] {
            ui.label(n.to_string());
            ui.label(l);
            ui.end_row();
        }
    });
    ui.separator();
    ui.label(RichText::new(t!("Как это работает", "How it works")).strong());
    ui.label(t!(
        "1. Триггер ловит событие (обычно «появился снаряд») по условиям.\n2. Его функции меняют снаряд: самонаведение, эмиттер, спутник, скорость…\n3. Мультикаст выпускает новые снаряды по фигуре группы спавна. С callTriggers они снова проходят через триггеры — так строятся цепочки.\n4. Ключи key_… видны только внутри этого файла.",
        "1. A trigger catches an event (usually «projectile appeared») by its conditions.\n2. Its functions change the projectile: homing, emitter, follower, speed…\n3. A multicast fires new projectiles along a spawn group figure. With callTriggers they pass through the triggers again — that is how chains form.\n4. key_… keys are visible only inside this file."
    ));
    ui.separator();
    ui.horizontal(|ui| {
        ui.label(t!("Добавить:", "Add:"));
        for s in [Section::Homing, Section::Multicast, Section::SpawnGroups, Section::Emitters, Section::Followers, Section::FormIds] {
            if ui.small_button(s.json_name()).clicked() {
                let k = crate::templates::new_instance(cfg, s);
                ix.p.nav = Some(sds_proj::validate::sel_of(s, &k));
            }
        }
    });
    if !cfg.extra.is_empty() {
        ui.separator();
        ui.label(RichText::new(t!("Неизвестные разделы (сохраняются как есть):", "Unknown sections (kept as they are):")).color(theme::WARN));
        let mut fix = None;
        for (k, v) in &cfg.extra {
            ui.horizontal(|ui| {
                ui.monospace(k);
                // A spawn group / follower that slipped out of its section.
                if v.get("Pattern").is_some() {
                    if ui.small_button(t!("→ в MulticastSpawnGroups", "→ into MulticastSpawnGroups")).clicked() {
                        fix = Some((k.clone(), "MulticastSpawnGroups"));
                    }
                    if ui.small_button(t!("→ в FollowersData", "→ into FollowersData")).clicked() {
                        fix = Some((k.clone(), "FollowersData"));
                    }
                }
            });
        }
        if let Some((k, sec)) = fix {
            if let Some(v) = cfg.extra.shift_remove(&k) {
                let mut wrap = serde_json::Map::new();
                let mut inner = serde_json::Map::new();
                inner.insert(k.clone(), v);
                wrap.insert(sec.into(), serde_json::Value::Object(inner));
                wrap.insert("Triggers".into(), serde_json::Value::Array(vec![]));
                if let Ok(mut c) = Config::parse(&serde_json::Value::Object(wrap).to_string()) {
                    cfg.spawn_groups.append(&mut c.spawn_groups);
                    cfg.followers.append(&mut c.followers);
                }
                ix.p.nav = Some(if sec == "FollowersData" { Sel::Follower(k) } else { Sel::Group(k) });
            }
        }
    }
}

fn trigger(ui: &mut egui::Ui, ix: &mut Ix, i: usize, t: &mut Trigger, focus: Option<usize>) {
    ui.heading(tf!("Триггер #{}", "Trigger #{}", i + 1));
    ui.label(RichText::new(t!("Когда срабатывает и что делает со снарядом", "When it fires and what it does to the projectile")).small().color(theme::DIM));
    ui.separator();
    ui.horizontal(|ui| {
        ui.label(t!("Событие", "Event"));
        combo(ui, ("ev", i), &mut t.event, EVENTS, |e| format!("{} — {e}", chain::event_label(e)));
    });
    ui.add_space(4.0);
    ui.label(RichText::new(t!("Условия (все должны выполниться)", "Conditions (all must hold)")).strong());
    if ix.fmt == CondFormat::Object {
        ui.label(RichText::new(t!("Формат «объект»: НЕ/ИЛИ недоступны, каждое условие — один раз.", "Object format: NOT/OR unavailable, each condition once.")).small().color(theme::DIM));
    }
    let mut remove = None;
    let mut swap = None;
    let n = t.conditions.len();
    for (ci, c) in t.conditions.iter_mut().enumerate() {
        ui.push_id(("cond", ci), |ui| {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    combo(ui, "ck", &mut c.kind, CONDITIONS, |k| format!("{} — {k}", chain::cond_label(k)));
                    if ci > 0 && ui.small_button("▲").clicked() {
                        swap = Some(ci - 1);
                    }
                    if ci + 1 < n && ui.small_button("▼").clicked() {
                        swap = Some(ci);
                    }
                    if ui.small_button("✖").clicked() {
                        remove = Some(ci);
                    }
                });
                if c.kind == "Hand" {
                    if !HANDS.contains(&c.value.as_str()) {
                        c.value = "Both".into();
                    }
                    combo(ui, "hand", &mut c.value, HANDS, |h| h.to_string());
                } else {
                    form_field(ui, ix, &mut c.value, cond_sig(&c.kind), true, false, &format!("cond{ci}"));
                }
                ui.add_enabled_ui(ix.fmt == CondFormat::Array, |ui| {
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut c.invert, t!("НЕ (invert)", "NOT (invert)"));
                        ui.checkbox(&mut c.or, t!("ИЛИ со следующим (OR)", "OR with the next")).on_hover_text(t!("Как флаг OR в CK", "Like the CK OR flag"));
                    });
                });
            });
        });
    }
    if let Some(r) = remove {
        t.conditions.remove(r);
    }
    if let Some(s) = swap {
        t.conditions.swap(s, s + 1);
    }
    if ui.button(t!("+ условие", "+ condition")).clicked() {
        t.conditions.push(Cond { kind: "ProjBaseIsFormID".into(), ..Default::default() });
        t.has_conditions = true;
    }
    ui.add_space(6.0);
    ui.separator();
    ui.label(RichText::new(t!("Функции", "Functions")).strong());
    tf_editor(ui, ix, &mut t.tf, &format!("t{i}"), focus, t.event == "ProjAppeared");
}

fn tf_editor(ui: &mut egui::Ui, ix: &mut Ix, tf: &mut TriggerFunctions, salt: &str, focus: Option<usize>, origin: bool) {
    let mut d = tf.disable_origin.unwrap_or(false);
    if ui
        .add_enabled(origin || tf.disable_origin.is_some(), egui::Checkbox::new(&mut d, t!("Отменить исходный снаряд (disableOrigin)", "Cancel the origin projectile (disableOrigin)")))
        .on_hover_text(t!("Только для ProjAppeared: исходный снаряд не вылетит — удобно, когда мультикаст его заменяет", "ProjAppeared only: the origin projectile does not launch — handy when a multicast replaces it"))
        .changed()
    {
        tf.disable_origin = if d { Some(true) } else { None };
    }
    let mut remove = None;
    let mut swap = None;
    let n = tf.functions.len();
    for (fi, f) in tf.functions.iter_mut().enumerate() {
        ui.push_id((salt, fi), |ui| {
            let mut frame = egui::Frame::group(ui.style());
            if focus == Some(fi) {
                frame = frame.stroke(egui::Stroke::new(1.5_f32, egui::Color32::WHITE));
            }
            frame.show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{}.", fi + 1)).color(theme::DIM));
                    let kinds: Vec<&str> = FUNCS.iter().map(|(n, _)| *n).collect();
                    if combo(ui, "fk", &mut f.kind, &kinds, |k| format!("{} — {k}", chain::func_label(k))) {
                        let keep_follow = f.on_followers;
                        *f = crate::templates::new_func(&f.kind);
                        f.on_followers = keep_follow;
                    }
                    if fi > 0 && ui.small_button("▲").clicked() {
                        swap = Some(fi - 1);
                    }
                    if fi + 1 < n && ui.small_button("▼").clicked() {
                        swap = Some(fi);
                    }
                    if ui.small_button("✖").clicked() {
                        remove = Some(fi);
                    }
                });
                ui.label(RichText::new(func_help(&f.kind)).small().color(theme::DIM));
                func_editor(ui, ix, f, &format!("{salt}f{fi}"));
                let mut of = f.on_followers.unwrap_or(false);
                if ui.checkbox(&mut of, t!("на спутниках кастера (on_followers)", "on the caster's followers (on_followers)")).on_hover_text(t!("Выполнить функцию не на этом снаряде, а на спутниках актёра", "Run on the actor's followers instead of this projectile")).changed() {
                    f.on_followers = if of { Some(true) } else { None };
                }
            });
        });
    }
    if let Some(r) = remove {
        tf.functions.remove(r);
    }
    if let Some(s) = swap {
        tf.functions.swap(s, s + 1);
    }
    ui.menu_button(t!("+ функция", "+ function"), |ui| {
        for (name, _) in FUNCS {
            if ui.button(format!("{} — {name}", chain::func_label(name))).on_hover_text(func_help(name)).clicked() {
                tf.functions.push(crate::templates::new_func(name));
                ui.close();
            }
        }
    });
}

fn func_help(k: &str) -> &'static str {
    match k {
        "SetHoming" => t!("Снаряд становится самонаводящимся (HomingData)", "The projectile becomes homing (HomingData)"),
        "SetRotationHoming" => t!("Один раз развернуть снаряд к цели (HomingData)", "Turn the projectile to the target once (HomingData)"),
        "SetRotationToSight" => t!("Развернуть снаряд туда, куда смотрит кастер", "Turn the projectile where the caster looks"),
        "SetEmitter" => t!("Периодически вызывать функции в полёте (EmittersData)", "Call functions periodically in flight (EmittersData)"),
        "SetFollower" => t!("Снаряд следует за кастером по фигуре (FollowersData)", "The projectile follows the caster in a figure (FollowersData)"),
        "ApplyMultiCast" => t!("Выпустить новые снаряды (MulticastData)", "Fire new projectiles (MulticastData)"),
        "ChangeSpeed" => t!("Изменить скорость: Set/Add/Mul", "Change speed: Set/Add/Mul"),
        "ChangeRange" => t!("Изменить дальность: Set/Add/Mul", "Change range: Set/Add/Mul"),
        "DisableHoming" => t!("Выключить самонаведение", "Turn homing off"),
        "DisableFollower" => t!("Отпустить спутника", "Release the follower"),
        "DisableEmitter" => t!("Выключить эмиттер", "Turn the emitter off"),
        "Placeatme" => t!("Заспавнить объект в точке события", "Spawn an object at the event position"),
        "SendAnimEvent" => t!("Отправить анимационное событие кастеру", "Send an animation event to the caster"),
        "Explode" => t!("Взрыв EXPL в точке события (эффект взрыва свой, не снаряда)", "Explode an EXPL at the event position (its own effect, not the projectile's)"),
        "SetColLayer" => t!("Слой коллизии: Actor — только актёры, Spell — обычный, None — без столкновений", "Collision layer: Actor — actors only, Spell — default, None — no collision"),
        _ => "",
    }
}

fn func_editor(ui: &mut egui::Ui, ix: &mut Ix, f: &mut Func, salt: &str) {
    match func_arg(&f.kind) {
        a @ (FuncArg::Homing | FuncArg::Emitter | FuncArg::Follower | FuncArg::Multicast) => key_ref(ui, ix, Section::for_arg(a).unwrap(), &mut f.id, salt),
        FuncArg::Num => {
            ui.horizontal(|ui| {
                combo(ui, (salt, "op"), &mut f.num.op, NUM_OPS, |o| match o {
                    "Set" => t!("= задать", "= set").to_string(),
                    "Add" => t!("+ прибавить", "+ add").to_string(),
                    "Mul" => t!("× умножить", "× multiply").to_string(),
                    o => o.to_string(),
                });
                ui.add(egui::DragValue::new(&mut f.num.value).speed(0.05).max_decimals(3));
            });
        }
        FuncArg::RestoreSpeed => {
            let mut r = f.restore_speed.unwrap_or(true);
            if ui.checkbox(&mut r, t!("вернуть обычную скорость (restore_speed)", "restore the default speed (restore_speed)")).changed() {
                f.restore_speed = Some(r);
            }
        }
        FuncArg::Form => form_field(ui, ix, &mut f.arg, None, true, false, salt),
        FuncArg::Explosion => form_field(ui, ix, &mut f.arg, Some(&[b"EXPL"]), true, false, salt),
        FuncArg::AnimEvent => {
            ui.add(egui::TextEdit::singleline(&mut f.arg).hint_text(t!("имя события", "event name")).desired_width(220.0));
        }
        FuncArg::Layer => {
            combo(ui, (salt, "layer"), &mut f.arg, LAYERS, |l| l.to_string());
        }
        FuncArg::None => {}
    }
}

fn homing(ui: &mut egui::Ui, h: &mut Homing) {
    egui::Grid::new("homing").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
        row(ui, t!("Тип", "Type"), Tr("ConstSpeed — разворот с постоянной скоростью; ConstAccel — постоянное ускорение к цели", "ConstSpeed — turns at a constant rate; ConstAccel — constant acceleration toward the target"), true);
        combo(ui, "ht", &mut h.kind, HOMING_TYPES, |s| s.to_string());
        ui.end_row();
        if h.kind == "ConstSpeed" {
            opt_num(ui, "rotationTime", Tr("Секунд на разворот на 180° (хорошо: 5, 3.5, 2.5)", "Seconds to turn 180° (nice: 5, 3.5, 2.5)"), &mut h.rotation_time, 2.5, 0.05, 0.5..=100.0);
        } else {
            opt_num(ui, "acceleration", Tr("Длина ускорения (хорошо: 5, 10, 50)", "Acceleration length (nice: 5, 10, 50)"), &mut h.acceleration, 10.0, 0.5, 1.0..=300.0);
        }
        opt_enum(ui, "target", Tr("Как выбирать цель: Nearest — ближайшая, Cursor — около прицела", "How to pick the target: Nearest, or Cursor — near the crosshair"), &mut h.target, HOMING_TARGETS, "Nearest");
        if h.target.as_deref() == Some("Cursor") {
            opt_num(ui, "cursorAngle", Tr("Угол конуса у прицела, 5–15 хорошо, максимум 90", "Cone angle at the crosshair, 5–15 is nice, 90 max"), &mut h.cursor_angle, 10.0, 0.5, 0.0..=90.0);
        }
        opt_enum(ui, "aggressive", Tr("Кого считать целью: Hostile — враги, Aggressive — агрессивные, Any — всех", "Who counts as a target: Hostile, Aggressive, Any"), &mut h.aggressive, AGGRESSIVE, "Hostile");
        opt_bool(ui, "checkLOS", Tr("Кастер должен видеть цель", "The caster must see the target"), &mut h.check_los, false);
    });
}

fn pattern(ui: &mut egui::Ui, p: &mut Pattern, salt: &str) {
    egui::Grid::new(("pattern", salt)).num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
        row(ui, t!("Фигура", "Figure"), Tr("Как расставить точки спавна", "How the spawn points are placed"), true);
        combo(ui, (salt, "shape"), &mut p.figure.shape, SHAPES, |s| s.to_string());
        ui.end_row();
        opt_u32(ui, "count", Tr("Число точек (снарядов)", "Number of points (projectiles)"), &mut p.figure.count, 1, 1..=200);
        if p.figure.shape != "Single" {
            row(ui, "size", Tr("Размер фигуры в игровых единицах (обычно 50–500)", "Figure size in game units (usually 50–500)"), p.figure.size.is_some());
            let mut s = p.figure.size.unwrap_or(100.0);
            if ui.add(egui::DragValue::new(&mut s).speed(1.0).range(1.0..=5000.0)).changed() || p.figure.size.is_none() {
                p.figure.size = Some(s);
            }
            ui.end_row();
        }
        opt_num(ui, "planeRotate", Tr("Поворот фигуры в её плоскости, градусы", "Rotate the figure in its plane, degrees"), &mut p.plane_rotate, 0.0, 1.0, -360.0..=360.0);
        opt_vec(ui, "normal", Tr("Нормаль плоскости фигуры относительно взгляда кастера: [0,1,0] — фигура перед собой, [0,0.01,1] — горизонтально", "Figure plane normal relative to the caster's view: [0,1,0] — facing forward, [0,0.01,1] — horizontal"), &mut p.normal, [0.0, 1.0, 0.0], ["x", "y", "z"]);
        opt_bool(ui, "xDepends", Tr("Нормаль следует и за наклоном взгляда вверх/вниз (иначе только поворот влево/вправо)", "The normal also follows looking up/down (otherwise only left/right)"), &mut p.x_depends, true);
        opt_text(ui, "origin", Tr("Узел скелета, от которого строится фигура (иначе — точка каста)", "Skeleton node the figure starts at (otherwise the cast point)"), &mut p.origin, "NPC Head [Head]");
        opt_vec(ui, "posOffset", Tr("Сдвиг центра фигуры: x вправо, y вперёд, z вверх", "Figure centre offset: x right, y forward, z up"), &mut p.pos_offset, [0.0, 0.0, 0.0], ["x", "y", "z"]);
    });
    if !p.figure.extra.is_empty() || !p.extra.is_empty() {
        let keys: Vec<&String> = p.figure.extra.keys().chain(p.extra.keys()).collect();
        ui.label(RichText::new(tf!("Лишние поля (DLL игнорирует): {}", "Extra fields (ignored by the DLL): {}", keys.iter().map(|k| k.as_str()).collect::<Vec<_>>().join(", "))).small().color(theme::WARN));
        if ui.small_button(t!("Убрать лишние поля", "Remove extra fields")).clicked() {
            p.figure.extra.clear();
            p.extra.clear();
        }
    }
}

fn group(ui: &mut egui::Ui, ix: &mut Ix, g: &mut SpawnGroup, orbit: &mut crate::preview::Orbit) {
    // Group fields written inside Figure/Pattern by mistake (seen in real configs).
    let misplaced: Vec<String> = ["sound", "rotation", "rotationTarget", "rotOffset", "posRnd", "rotRnd"]
        .iter()
        .filter(|k| g.pattern.figure.extra.contains_key(**k) || g.pattern.extra.contains_key(**k))
        .map(|k| k.to_string())
        .collect();
    if !misplaced.is_empty() {
        ui.label(RichText::new(tf!("{} записаны внутри Pattern/Figure — DLL их там не читает.", "{} are written inside Pattern/Figure — the DLL does not read them there.", misplaced.join(", "))).color(theme::WARN));
        if ui.button(t!("Перенести на уровень группы", "Move them to the group level")).clicked() {
            for k in &misplaced {
                let v = g.pattern.figure.extra.shift_remove(k).or_else(|| g.pattern.extra.shift_remove(k));
                let Some(v) = v else { continue };
                let s = v.as_str().map(String::from);
                let a = v.as_array().map(|a| a.iter().filter_map(|x| x.as_f64()).collect::<Vec<f64>>());
                match k.as_str() {
                    "sound" if g.sound.is_none() => g.sound = s,
                    "rotation" if g.rotation.is_none() => g.rotation = s,
                    "rotationTarget" if g.rotation_target.is_none() => g.rotation_target = s,
                    "rotOffset" if g.rot_offset.is_none() => g.rot_offset = a.filter(|a| a.len() == 2).map(|a| [a[0], a[1]]),
                    "rotRnd" if g.rot_rnd.is_none() => g.rot_rnd = a.filter(|a| a.len() == 2).map(|a| [a[0], a[1]]),
                    "posRnd" if g.pos_rnd.is_none() => g.pos_rnd = a.filter(|a| a.len() == 3).map(|a| [a[0], a[1], a[2]]),
                    _ => {}
                }
            }
        }
        ui.separator();
    }
    pattern(ui, &mut g.pattern, "g");
    egui::Grid::new("group").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
        opt_enum(
            ui,
            "rotation",
            Tr("Куда летят снаряды: Parallel — параллельно взгляду, ToSight — в точку прицела, ToCenter/FromCenter — к/от центра фигуры, ToTarget — на цель HomingData", "Where projectiles fly: Parallel — along the view, ToSight — to the aim point, ToCenter/FromCenter — to/from the figure centre, ToTarget — at a HomingData target"),
            &mut g.rotation,
            ROTATIONS,
            "Parallel",
        );
        if g.rotation.as_deref() == Some("ToTarget") {
            row(ui, "rotationTarget", Tr("Ключ HomingData: как искать цель", "HomingData key: how to find the target"), true);
            let mut k = g.rotation_target.clone().unwrap_or_default();
            key_ref(ui, ix, Section::Homing, &mut k, "rt");
            g.rotation_target = Some(k);
            ui.end_row();
        }
        opt_enum(ui, "sound", Tr("Звук выстрела: Single — один на группу, Every — каждому, None — без", "Launch sound: Single — once per group, Every — each, None"), &mut g.sound, SOUNDS, "Single");
        opt_vec(ui, "rotOffset", Tr("Смещение направления группы от направления каста [наклон, поворот], градусы", "Group direction offset from the cast [pitch, yaw], degrees"), &mut g.rot_offset, [0.0, 0.0], ["x", "z"]);
        opt_vec(ui, "posRnd", Tr("Случайный разброс позиции каждого снаряда", "Random position spread per projectile"), &mut g.pos_rnd, [0.0; 3], ["x", "y", "z"]);
        opt_vec(ui, "rotRnd", Tr("Случайный разброс направления каждого снаряда, градусы", "Random direction spread per projectile, degrees"), &mut g.rot_rnd, [0.0, 0.0], ["x", "z"]);
    });
    ui.separator();
    crate::preview::show(ui, orbit, &g.pattern, g.rotation.as_deref().unwrap_or("Parallel"), false);
}

fn follower(ui: &mut egui::Ui, f: &mut Follower, orbit: &mut crate::preview::Orbit) {
    pattern(ui, &mut f.pattern, "f");
    egui::Grid::new("follower").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
        opt_enum(ui, "rounding", Tr("Плавание вокруг своей точки: None, Plane — в плоскости, Sphere — в сфере", "Floating around its point: None, Plane, Sphere"), &mut f.rounding, ROUNDING, "None");
        if f.rounding.as_deref().is_some_and(|r| r != "None") {
            opt_num(ui, "roundingR", Tr("Радиус плавания", "Float radius"), &mut f.rounding_r, 10.0, 0.5, 0.0..=1000.0);
        }
        opt_enum(ui, "collision", Tr("Столкновения: Actor — только с актёрами, Spell — обычные, None — без", "Collision: Actor — actors only, Spell — default, None"), &mut f.collision, LAYERS, "Actor");
        opt_num(ui, "speed", Tr("Множитель скорости движения к точке; 0 — мгновенно", "Speed multiplier toward the point; 0 — instant"), &mut f.speed, 1.0, 0.05, 0.0..=20.0);
    });
    ui.label(RichText::new(t!("Известная особенность: спутники сверх числа точек фигуры встают в точку №0.", "Known issue: followers beyond the figure's point count all share point 0.")).small().color(theme::DIM));
    ui.separator();
    crate::preview::show(ui, orbit, &f.pattern, "Parallel", true);
}

#[allow(clippy::too_many_arguments)]
fn multicast(ui: &mut egui::Ui, ix: &mut Ix, key: &str, items: &mut Vec<McItem>, focus: Option<usize>, groups: &[(String, SpawnGroup)], orbit: &mut crate::preview::Orbit) {
    ui.label(RichText::new(t!("Каждая группа выпускает снаряды по своей фигуре.", "Each group fires projectiles along its figure.")).small().color(theme::DIM));
    let mut remove = None;
    for (ii, it) in items.iter_mut().enumerate() {
        ui.push_id(("mci", ii), |ui| {
            let mut frame = egui::Frame::group(ui.style());
            if focus == Some(ii) {
                frame = frame.stroke(egui::Stroke::new(1.5_f32, egui::Color32::WHITE));
            }
            frame.show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(tf!("Группа {}", "Group {}", ii + 1)).strong());
                    if ui.small_button("✖").clicked() {
                        remove = Some(ii);
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("spawn_group");
                    key_ref(ui, ix, Section::SpawnGroups, &mut it.spawn_group, &format!("sg{ii}"));
                });
                let mut is_arrow = matches!(it.source, McSource::Arrow { .. });
                ui.horizontal(|ui| {
                    if ui.radio_value(&mut is_arrow, false, t!("заклинание", "spell")).changed() || ui.radio_value(&mut is_arrow, true, t!("стрела", "arrow")).changed() {
                        it.source = if is_arrow { McSource::Arrow { weap: "Current".into(), arrow: None } } else { McSource::Spell("Current".into()) };
                    }
                });
                match &mut it.source {
                    McSource::Spell(s) => {
                        ui.label("spellID");
                        form_field(ui, ix, s, Some(&[b"SPEL"]), true, true, &format!("sp{ii}"));
                        if let (Some(w), false) = (ix.world, s == "Current") {
                            let r = ix.snap.formids.iter().find(|(k, _)| k == s).map(|(_, v)| v.clone()).unwrap_or_else(|| s.clone());
                            if let Some(p) = w.key_of(&r).and_then(|k| w.spell_projectile(k)) {
                                ui.label(RichText::new(tf!("→ снаряд {}", "→ projectile {}", w.entry(p).map(|e| e.edid.clone()).unwrap_or_default())).small().color(theme::DIM));
                            }
                        }
                    }
                    McSource::Arrow { weap, arrow } => {
                        ui.label("weapID");
                        form_field(ui, ix, weap, Some(&[b"WEAP"]), true, true, &format!("wp{ii}"));
                        let mut a = arrow.clone().unwrap_or_else(|| "Current".into());
                        ui.label("arrowID");
                        form_field(ui, ix, &mut a, None, true, true, &format!("ar{ii}"));
                        *arrow = Some(a);
                    }
                }
                egui::Grid::new(("mcg", ii)).num_columns(2).show(ui, |ui| {
                    opt_bool(ui, "callTriggers", Tr("Новые снаряды снова проходят через Triggers — продолжение цепочки", "New projectiles pass through the Triggers again — the chain continues"), &mut it.call_triggers, false);
                    opt_enum(ui, "HomingDetection", Tr("Как раздавать цели: Individual — каждому свою, Evenly — поровну между целями", "How targets are shared: Individual, or Evenly between targets"), &mut it.homing_detection, HOMING_DETECTION, "Individual");
                });
                let mut has_tf = it.functions.is_some();
                if ui.checkbox(&mut has_tf, t!("Функции для новых снарядов", "Functions for the new projectiles")).changed() {
                    it.functions = if has_tf { Some(TriggerFunctions::default()) } else { None };
                }
                if let Some(t) = &mut it.functions {
                    tf_editor(ui, ix, t, &format!("{key}i{ii}"), None, false);
                }
                if let Some((_, g)) = groups.iter().find(|(k, _)| *k == it.spawn_group) {
                    if focus == Some(ii) || items_len_one(focus) {
                        crate::preview::show(ui, orbit, &g.pattern, g.rotation.as_deref().unwrap_or("Parallel"), false);
                    }
                }
            });
        });
    }
    if let Some(r) = remove {
        items.remove(r);
    }
    if ui.button(t!("+ группа", "+ group")).clicked() {
        let g = ix.snap.keys(Section::SpawnGroups).first().cloned().unwrap_or_default();
        items.push(McItem { spawn_group: g, ..Default::default() });
    }
}

fn items_len_one(focus: Option<usize>) -> bool {
    focus.is_none()
}

fn emitter(ui: &mut egui::Ui, ix: &mut Ix, key: &str, e: &mut Emitter) {
    egui::Grid::new("emitter").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
        row(ui, "interval", Tr("Как часто вызывать функции, секунды", "How often to call the functions, seconds"), true);
        ui.add(egui::DragValue::new(&mut e.interval).speed(0.05).range(0.0..=600.0).suffix(" s"));
        ui.end_row();
        opt_bool(ui, "limited", Tr("Ограничить число вызовов", "Limit the number of calls"), &mut e.limited, false);
        if e.limited == Some(true) {
            opt_u32(ui, "count", Tr("Сколько раз (максимум 30)", "How many times (30 max)"), &mut e.count, 1, 1..=30);
        }
        opt_bool(ui, "destroyAfter", Tr("Уничтожить снаряд после вызова", "Destroy the projectile after the call"), &mut e.destroy_after, false);
    });
    ui.separator();
    let mut remove = None;
    for (fi, f) in e.functions.iter_mut().enumerate() {
        ui.push_id(("emf", fi), |ui| {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    combo(ui, "efk", &mut f.kind, EMITTER_FUNCS, |k| match k {
                        "TriggerFunctions" => t!("Функции триггера", "Trigger functions").to_string(),
                        "AccelerateToMaxSpeed" => t!("Разгон до макс. скорости", "Accelerate to max speed").to_string(),
                        o => o.to_string(),
                    });
                    if ui.small_button("✖").clicked() {
                        remove = Some(fi);
                    }
                });
                if f.kind == "AccelerateToMaxSpeed" {
                    ui.horizontal(|ui| {
                        ui.label("time");
                        ui.add(egui::DragValue::new(&mut f.time).speed(0.05).range(0.0..=100.0).suffix(" s"));
                        if !SPEED_TYPES.contains(&f.speed_type.as_str()) {
                            f.speed_type = "Linear".into();
                        }
                        combo(ui, "st", &mut f.speed_type, SPEED_TYPES, |s| s.to_string());
                    });
                } else {
                    tf_editor(ui, ix, &mut f.tf, &format!("{key}e{fi}"), None, false);
                }
            });
        });
    }
    if let Some(r) = remove {
        e.functions.remove(r);
    }
    if ui.button(t!("+ функция эмиттера", "+ emitter function")).clicked() {
        e.functions.push(EmFunc { kind: "TriggerFunctions".into(), ..Default::default() });
    }
}

fn record(ui: &mut egui::Ui, ix: &mut Ix, cfg: &mut Config, k: sds_records::FormKey, foreign: &std::collections::HashMap<sds_records::FormKey, Vec<crate::app::ForeignCatch>>) {
    let Some(w) = ix.world else { return };
    let Some(e) = w.entry(k) else {
        ui.label(w.formid(k));
        return;
    };
    ui.heading(&e.edid);
    if !e.name.is_empty() {
        ui.label(format!("«{}»", e.name));
    }
    ui.label(format!("{} · {}", String::from_utf8_lossy(&e.sig), e.formid));
    if let Some(pi) = w.proj_info(k) {
        ui.label(tf!("{} · скорость {:.0} · дальность {:.0} · гравитация {:.2}", "{} · speed {:.0} · range {:.0} · gravity {:.2}", pi.kind, pi.speed, pi.range, pi.gravity));
        if let Some(x) = pi.explosion.and_then(|x| w.entry(x)) {
            ui.label(tf!("взрыв: {}", "explosion: {}", x.edid));
        }
        let spells = w.spells_firing(k);
        if !spells.is_empty() {
            ui.label(RichText::new(t!("Кастуют заклинания:", "Cast by spells:")).strong());
            for s in spells.iter().take(12) {
                if let Some(se) = w.entry(*s) {
                    ui.label(format!("• {} {}", se.edid, if se.name.is_empty() { String::new() } else { format!("«{}»", se.name) }));
                }
            }
        }
        if let Some(f) = foreign.get(&k) {
            ui.label(RichText::new(t!("Ловят триггеры конфигов загрузки:", "Caught by load-order config triggers:")).strong());
            for c in f {
                ui.label(format!("• {} #{}", c.file, c.trigger + 1));
            }
        }
        ui.separator();
        ui.label(t!("Новая цепочка для этого снаряда:", "New chain for this projectile:"));
        ui.horizontal_wrapped(|ui| {
            for (tp, name, hint) in crate::templates::ALL {
                if ui.button(name.get()).on_hover_text(hint.get()).clicked() {
                    let key = key_for(ix, &e.formid);
                    for (fk, fv) in std::mem::take(&mut ix.p.formids) {
                        if !cfg.has_key(Section::FormIds, &fk) {
                            cfg.formids.push((fk, fv));
                        }
                    }
                    let i = crate::templates::add(cfg, *tp, Some(key));
                    ix.p.nav = Some(Sel::Trigger(i));
                }
            }
        });
    }
}
