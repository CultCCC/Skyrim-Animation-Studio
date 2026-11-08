//! SDS Projectile Studio: the load order, open configs, undo, export.

use crate::settings::{ExportFormat, Settings};
use sds_i18n::{t, tf};
use sds_proj::model::{CondFormat, Config};
use sds_proj::validate::{Issue, Level};
use sds_proj::world::World;
use sds_proj::Sel;
use sds_records::FormKey;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::Arc;

pub struct Doc {
    /// File name, `SDSFoo.json`.
    pub name: String,
    /// Where it was read from: VFS path or a disk path.
    pub source: String,
    pub origin: String,
    pub cfg: Config,
    saved: Config,
    undo: Vec<Config>,
    redo: Vec<Config>,
    last_edit: Option<(Sel, f64)>,
    pub sel: Sel,
    /// The file's own condition format (None: it had none).
    pub file_format: Option<CondFormat>,
    pub issues: Vec<Issue>,
    issues_of: Option<(Config, CondFormat)>,
}

impl Doc {
    pub fn new(name: String, source: String, origin: String, cfg: Config) -> Doc {
        let file_format = cfg.cond_format;
        Doc { name, source, origin, saved: cfg.clone(), cfg, undo: Vec::new(), redo: Vec::new(), last_edit: None, sel: Sel::Root, file_format, issues: Vec::new(), issues_of: None }
    }
    pub fn dirty(&self) -> bool {
        self.cfg != self.saved
    }
    /// Record `before` as an undo step; edits to the same thing within a
    /// second coalesce (typing, dragging).
    pub fn commit(&mut self, before: Config, now: f64) {
        if before == self.cfg {
            return;
        }
        let merge = self.last_edit.as_ref().is_some_and(|(s, t)| *s == self.sel && now - t < 1.0);
        if !merge {
            self.undo.push(before);
            if self.undo.len() > 200 {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.last_edit = Some((self.sel.clone(), now));
    }
    pub fn undo(&mut self) {
        if let Some(c) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.cfg, c));
            self.last_edit = None;
        }
    }
    pub fn redo(&mut self) {
        if let Some(c) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.cfg, c));
            self.last_edit = None;
        }
    }
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    pub fn refresh_issues(&mut self, w: Option<&World>, fmt: CondFormat) {
        if self.issues_of.as_ref().is_some_and(|(c, f)| *c == self.cfg && *f == fmt) {
            return;
        }
        self.issues = sds_proj::validate::validate(&self.cfg, w, fmt);
        self.issues_of = Some((self.cfg.clone(), fmt));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Center {
    Chains,
    Json,
    Issues,
}

/// A request to pick a record for a form field.
pub struct PickReq {
    pub target: egui::Id,
    pub sigs: Vec<[u8; 4]>,
    pub query: String,
    pub just_opened: bool,
}

/// Another file's trigger that catches a projectile.
#[derive(Debug, Clone)]
pub struct ForeignCatch {
    pub file: String,
    pub trigger: usize,
}

pub struct Shot {
    pub path: PathBuf,
    pub open: Option<String>,
    pub select: Option<String>,
    pub frames: u32,
}

pub struct App {
    pub settings: Settings,
    pub world: Option<Arc<World>>,
    pub vfs: Option<Arc<sds_vfs::Vfs>>,
    loading: Option<Receiver<Result<(sds_vfs::Vfs, World), String>>>,
    load_started: f64,
    pub docs: Vec<Doc>,
    pub cur: usize,
    pub center: Center,
    pub picker: Option<PickReq>,
    pub picked: Option<(egui::Id, String)>,
    pub nav: Option<Sel>,
    pub log: Vec<String>,
    pub settings_open: bool,
    pub new_open: Option<String>,
    pub file_filter: String,
    pub chain_filter: String,
    pub collapsed: std::collections::HashSet<(String, usize)>,
    /// Projectile → triggers in OTHER configs of the load order that catch it.
    pub foreign: HashMap<FormKey, Vec<ForeignCatch>>,
    pub preview: crate::preview::Orbit,
    pub shot: Option<Shot>,
    pub confirm_close: Option<usize>,
    pub scroll_to_sel: bool,
}

impl App {
    pub fn new() -> App {
        let settings = Settings::load();
        settings.apply_language();
        let mut app = App {
            settings,
            world: None,
            vfs: None,
            loading: None,
            load_started: 0.0,
            docs: Vec::new(),
            cur: 0,
            center: Center::Chains,
            picker: None,
            picked: None,
            nav: None,
            log: Vec::new(),
            settings_open: false,
            new_open: None,
            file_filter: String::new(),
            chain_filter: String::new(),
            collapsed: Default::default(),
            foreign: HashMap::new(),
            preview: Default::default(),
            shot: None,
            confirm_close: None,
            scroll_to_sel: false,
        };
        if app.settings.load_on_start && app.settings.source().is_some() {
            app.start_load(0.0);
        } else if app.settings.source().is_none() {
            app.settings_open = true;
        }
        app
    }

    pub fn doc(&self) -> Option<&Doc> {
        self.docs.get(self.cur)
    }

    pub fn doc_mut(&mut self) -> Option<&mut Doc> {
        self.docs.get_mut(self.cur)
    }

    pub fn log(&mut self, s: String) {
        eprintln!("LOG {s}");
        self.log.push(s);
        if self.log.len() > 200 {
            self.log.remove(0);
        }
    }

    pub fn start_load(&mut self, now: f64) {
        let Some(src) = self.settings.source() else {
            self.log(t!("Не задан профиль MO2 или папка Data", "No MO2 profile or Data folder set").into());
            return;
        };
        let lang = "russian".to_string();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let r = sds_vfs::Vfs::build(&src, &Default::default()).map_err(|e| e.to_string()).map(|vfs| {
                let lang = sds_lang(&vfs).unwrap_or(lang);
                let w = World::build(&vfs, &lang);
                (vfs, w)
            });
            let _ = tx.send(r);
        });
        self.loading = Some(rx);
        self.load_started = now;
    }

    pub fn is_loading(&self) -> bool {
        self.loading.is_some()
    }

    fn poll_load(&mut self, now: f64) {
        let Some(rx) = &self.loading else { return };
        let Ok(r) = rx.try_recv() else { return };
        self.loading = None;
        match r {
            Ok((vfs, w)) => {
                self.vfs = Some(Arc::new(vfs));
                let dll = match (w.dll_format, &w.dll_origin) {
                    (Some(f), Some(o)) => tf!("DLL NewProjectilesTMP из «{o}», формат условий: {}", "NewProjectilesTMP DLL from «{o}», condition format: {}", fmt_name(f)),
                    _ => t!("DLL NewProjectilesTMP не найдена в загрузке", "NewProjectilesTMP DLL not found in the load order").into(),
                };
                self.log(tf!(
                    "Загружено за {:.1} с: {} плагинов, {} записей, {} конфигов. {dll}",
                    "Loaded in {:.1} s: {} plugins, {} records, {} configs. {dll}",
                    now - self.load_started,
                    w.db.plugins.len(),
                    w.entries.len(),
                    w.configs.len()
                ));
                self.world = Some(Arc::new(w));
                self.reindex_foreign();
                // Re-check open docs against the new load order.
                for d in &mut self.docs {
                    d.issues_of = None;
                }
            }
            Err(e) => self.log(tf!("Ошибка загрузки: {e}", "Load failed: {e}")),
        }
    }

    /// Parse every config of the load order once, for cross-file links.
    fn reindex_foreign(&mut self) {
        self.foreign.clear();
        let Some(w) = self.world.clone() else { return };
        for f in &w.configs {
            let Some(text) = self.read_config_text(&f.vfs_path).map(|(t, _)| t) else { continue };
            let Ok(c) = Config::parse(&text) else { continue };
            let cx = sds_proj::chain::Ctx { c: &c, w: Some(&w) };
            for (i, t) in c.triggers.iter().enumerate() {
                if t.event != "ProjAppeared" {
                    continue;
                }
                if let Some(p) = cx.trigger_projectile(t) {
                    self.foreign.entry(p).or_default().push(ForeignCatch { file: f.name.clone(), trigger: i });
                }
            }
        }
    }

    /// The text of a config: the exported copy in the output folder wins
    /// (the output mod is often not enabled in MO2 yet).
    fn read_config_text(&self, vfs_path: &str) -> Option<(String, String)> {
        let name = vfs_path.rsplit('/').next()?;
        let out = self.settings.out_dir().join(name);
        if let Ok(b) = std::fs::read(&out) {
            return Some((String::from_utf8_lossy(&b).into_owned(), t!("Output (экспорт)", "Output (export)").into()));
        }
        let w = self.world.as_ref()?;
        let f = w.configs.iter().find(|c| c.vfs_path == vfs_path)?;
        let b = self.vfs.as_ref()?.read(vfs_path).ok()?;
        Some((String::from_utf8_lossy(&b).into_owned(), f.origin.clone()))
    }

    pub fn open_config(&mut self, vfs_path: &str) {
        if let Some(i) = self.docs.iter().position(|d| d.source == vfs_path) {
            self.cur = i;
            return;
        }
        let Some((text, origin)) = self.read_config_text(vfs_path) else {
            self.log(tf!("Не удалось прочитать {vfs_path}", "Could not read {vfs_path}"));
            return;
        };
        let name = self.world.as_ref().and_then(|w| w.configs.iter().find(|c| c.vfs_path == vfs_path).map(|c| c.name.clone())).unwrap_or_else(|| vfs_path.rsplit('/').next().unwrap_or(vfs_path).to_string());
        match Config::parse(&text) {
            Ok(c) => {
                if !c.notes.is_empty() {
                    self.log(tf!("{name}: {} замечаний при чтении — см. «Проблемы»", "{name}: {} notes while reading — see Problems", c.notes.len()));
                }
                self.docs.push(Doc::new(name, vfs_path.to_string(), origin, c));
                self.cur = self.docs.len() - 1;
            }
            Err(e) => self.log(tf!("{name}: не JSON — {e}", "{name}: not JSON — {e}")),
        }
    }

    pub fn open_disk(&mut self, p: PathBuf) {
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        match std::fs::read(&p).map_err(|e| e.to_string()).and_then(|b| Config::parse(&String::from_utf8_lossy(&b))) {
            Ok(c) => {
                self.docs.push(Doc::new(name, p.display().to_string(), t!("файл на диске", "disk file").into(), c));
                self.cur = self.docs.len() - 1;
            }
            Err(e) => self.log(tf!("{name}: {e}", "{name}: {e}")),
        }
    }

    pub fn new_config(&mut self, name: String) {
        let name = if name.to_lowercase().ends_with(".json") { name } else { format!("{name}.json") };
        let cfg = Config { cond_format: None, ..Default::default() };
        self.docs.push(Doc::new(name.clone(), format!("new:{name}"), t!("новый", "new").into(), cfg));
        self.cur = self.docs.len() - 1;
        // A new file starts unsaved.
        if let Some(d) = self.docs.last_mut() {
            d.saved.extra.insert("__new".into(), serde_json::Value::Null);
        }
    }

    /// The format the export writes.
    pub fn export_format(&self, d: &Doc) -> CondFormat {
        match self.settings.export_format {
            ExportFormat::Object => CondFormat::Object,
            ExportFormat::Array => CondFormat::Array,
            ExportFormat::Auto => self.world.as_ref().and_then(|w| w.dll_format).or(d.file_format).unwrap_or(CondFormat::Object),
        }
    }

    pub fn export(&mut self, idx: usize, to: Option<PathBuf>) {
        let Some(d) = self.docs.get(idx) else { return };
        let fmt = self.export_format(d);
        let path = to.unwrap_or_else(|| self.settings.out_dir().join(&d.name));
        let text = d.cfg.to_json(fmt);
        let errors = d.issues.iter().filter(|i| i.level == Level::Error).count();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::write(&path, text) {
            Ok(()) => {
                let d = &mut self.docs[idx];
                d.saved = d.cfg.clone();
                d.file_format = Some(fmt);
                let warn = if errors > 0 { tf!(" — внимание: {errors} ошибок", " — warning: {errors} errors") } else { String::new() };
                self.log(tf!("Экспорт → {} ({}){warn}", "Exported → {} ({}){warn}", path.display(), fmt_name(fmt)));
                self.reindex_foreign();
            }
            Err(e) => self.log(tf!("Экспорт не удался: {} — {e}", "Export failed: {} — {e}", path.display())),
        }
    }

    pub fn close_doc(&mut self, i: usize) {
        if i < self.docs.len() {
            self.docs.remove(i);
            if self.cur >= self.docs.len() {
                self.cur = self.docs.len().saturating_sub(1);
            }
        }
    }

    pub fn select(&mut self, s: Sel) {
        if let Some(d) = self.doc_mut() {
            d.sel = s;
        }
        self.scroll_to_sel = true;
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let typing = ctx.memory(|m| m.focused().is_some());
        let (z, y, s) = ctx.input_mut(|i| {
            let z = i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z);
            let y = i.consume_key(egui::Modifiers::COMMAND, egui::Key::Y) || i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::Z);
            let s = i.consume_key(egui::Modifiers::COMMAND, egui::Key::S);
            (z, y, s)
        });
        if s {
            let c = self.cur;
            self.export(c, None);
        }
        if typing {
            return;
        }
        if let Some(d) = self.doc_mut() {
            if z {
                d.undo();
            }
            if y {
                d.redo();
            }
        }
    }

    fn take_screenshot(&mut self, ctx: &egui::Context) {
        let Some(shot) = &mut self.shot else { return };
        if self.loading.is_some() || (self.world.is_none() && self.settings.source().is_some()) {
            ctx.request_repaint();
            return;
        }
        if let Some(name) = shot.open.take() {
            let path = self.world.as_ref().and_then(|w| w.configs.iter().find(|c| c.name.eq_ignore_ascii_case(&name) || c.vfs_path == name).map(|c| c.vfs_path.clone()));
            match path {
                Some(p) => self.open_config(&p),
                None if std::path::Path::new(&name).is_file() => self.open_disk(name.into()),
                None => self.log(format!("shot: no config {name}")),
            }
            let sel = self.shot.as_mut().and_then(|s| s.select.take());
            if let Some(s) = sel.and_then(|s| parse_sel(&s)) {
                self.select(s);
            }
            ctx.request_repaint();
            return;
        }
        let shot = self.shot.as_mut().unwrap();
        if shot.frames > 0 {
            shot.frames -= 1;
            if shot.frames == 0 {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
            }
            ctx.request_repaint();
            return;
        }
        let image = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        let Some(image) = image else {
            ctx.request_repaint();
            return;
        };
        let (w, h) = (image.size[0] as u32, image.size[1] as u32);
        let mut rgba = Vec::with_capacity(image.pixels.len() * 4);
        for p in &image.pixels {
            rgba.extend_from_slice(&[p.r(), p.g(), p.b(), 255]);
        }
        if let Some(png) = aether_core::png::encode_rgba(w, h, &rgba) {
            let _ = std::fs::write(&shot.path, png);
            eprintln!("SCREENSHOT {w}x{h} -> {}", shot.path.display());
        }
        if let Some(d) = self.doc() {
            for i in &d.issues {
                eprintln!("ISSUE {:?} {:?} {}", i.level, i.at, i.msg);
            }
        }
        self.shot = None;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

/// `trigger:0`, `group:key_SG`, `homing:key`, `mc:key`, `mcitem:key:0`, `emitter:key`, `follower:key`, `formid:key`.
pub fn parse_sel(s: &str) -> Option<Sel> {
    let (k, v) = s.split_once(':')?;
    Some(match k {
        "trigger" => Sel::Trigger(v.parse().ok()?),
        "group" => Sel::Group(v.into()),
        "homing" => Sel::Homing(v.into()),
        "mc" => Sel::Multicast(v.into()),
        "mcitem" => {
            let (a, b) = v.rsplit_once(':')?;
            Sel::McItem(a.into(), b.parse().ok()?)
        }
        "emitter" => Sel::Emitter(v.into()),
        "follower" => Sel::Follower(v.into()),
        "formid" => Sel::FormId(v.into()),
        _ => return None,
    })
}

pub fn fmt_name(f: CondFormat) -> &'static str {
    match f {
        CondFormat::Object => t!("объект (DLL 2023–24)", "object (DLL 2023–24)"),
        CondFormat::Array => t!("массив (GitHub master)", "array (GitHub master)"),
    }
}

/// sLanguage from the profile's Skyrim.ini (SDS: RUSSIAN).
fn sds_lang(vfs: &sds_vfs::Vfs) -> Option<String> {
    let dir = vfs.profile_dir.as_ref()?;
    for f in ["skyrim.ini", "Skyrim.ini"] {
        if let Ok(b) = std::fs::read(dir.join(f)) {
            for l in String::from_utf8_lossy(&b).lines() {
                if let Some((k, v)) = l.split_once('=') {
                    if k.trim().eq_ignore_ascii_case("sLanguage") {
                        return Some(v.trim().to_lowercase());
                    }
                }
            }
        }
    }
    None
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        self.poll_load(now);
        if self.loading.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        self.shortcuts(&ctx);
        self.take_screenshot(&ctx);

        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        for p in dropped.into_iter().filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("json"))) {
            self.open_disk(p);
        }

        let world = self.world.clone();
        if let Some(d) = self.docs.get_mut(self.cur) {
            let fmt = match self.settings.export_format {
                ExportFormat::Object => CondFormat::Object,
                ExportFormat::Array => CondFormat::Array,
                ExportFormat::Auto => world.as_ref().and_then(|w| w.dll_format).or(d.file_format).unwrap_or(CondFormat::Object),
            };
            d.refresh_issues(world.as_deref(), fmt);
        }

        egui::Panel::top("menu").show_inside(ui, |ui| crate::panels::menu_bar(self, ui));
        egui::Panel::bottom("status").show_inside(ui, |ui| crate::panels::status_bar(self, ui));
        egui::Panel::left("files").resizable(true).default_size(270.0).show_inside(ui, |ui| crate::panels::file_panel(self, ui));
        egui::Panel::right("inspector").resizable(true).default_size(430.0).show_inside(ui, |ui| {
            let before = self.doc().map(|d| d.cfg.clone());
            crate::inspector::show(self, ui);
            if let (Some(b), Some(d)) = (before, self.docs.get_mut(self.cur)) {
                d.commit(b, now);
            }
        });
        egui::CentralPanel::default().show_inside(ui, |ui| {
            let before = self.doc().map(|d| d.cfg.clone());
            crate::panels::center(self, ui);
            if let (Some(b), Some(d)) = (before, self.docs.get_mut(self.cur)) {
                d.commit(b, now);
            }
        });

        crate::picker::window(self, &ctx);
        crate::panels::settings_window(self, &ctx);
        crate::panels::new_window(self, &ctx);
        crate::panels::confirm_close_window(self, &ctx);
        if let Some(s) = self.nav.take() {
            self.select(s);
        }
    }

    fn on_exit(&mut self) {
        if std::env::args().any(|a| a == "--screenshot") {
            return;
        }
        self.settings.save();
    }
}
