//! Studio state, the docked layout and everything that is not a panel.

use crate::doc::{Anno, ClipDoc, ClipSource};
use crate::library::{self, Library};
use crate::settings::{self, Settings};
use crate::timeline::TimelineView;
use egui_dock::{DockArea, DockState, NodeIndex, Style};
use serde::{Deserialize, Serialize};
use sds_vfs::{Progress, Source, Vfs};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tab {
    Library,
    Viewport,
    Timeline,
    Inspector,
    Annotations,
    Scene,
    Log,
    Npc,
    Oar,
    Scar,
    Combo,
}

impl Tab {
    pub const ALL: [Tab; 11] = [Tab::Library, Tab::Viewport, Tab::Timeline, Tab::Inspector, Tab::Annotations, Tab::Scene, Tab::Log, Tab::Npc, Tab::Oar, Tab::Scar, Tab::Combo];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Library => sds_i18n::t!("Библиотека", "Library"),
            Tab::Viewport => sds_i18n::t!("Рендер", "Render"),
            Tab::Timeline => sds_i18n::t!("Таймлайн", "Timeline"),
            Tab::Inspector => sds_i18n::t!("Инспектор", "Inspector"),
            Tab::Annotations => sds_i18n::t!("Аннотации", "Annotations"),
            Tab::Scene => sds_i18n::t!("Сцена", "Scene"),
            Tab::Log => sds_i18n::t!("Лог", "Log"),
            Tab::Npc => sds_i18n::t!("Персонаж", "Character"),
            Tab::Oar => "OAR",
            Tab::Scar => "SCAR",
            Tab::Combo => sds_i18n::t!("Граф комбо", "Combo graph"),
        }
    }
}

pub fn default_layout() -> DockState<Tab> {
    let mut dock = DockState::new(vec![Tab::Viewport]);
    let s = dock.main_surface_mut();
    let [main, _left] = s.split_left(NodeIndex::root(), 0.2, vec![Tab::Library, Tab::Npc]);
    let [centre, _right] = s.split_right(main, 0.76, vec![Tab::Inspector, Tab::Scene, Tab::Scar, Tab::Oar, Tab::Annotations]);
    s.split_below(centre, 0.55, vec![Tab::Timeline, Tab::Combo, Tab::Log]);
    dock
}

pub struct Loaded {
    pub vfs: Arc<Vfs>,
    pub library: Library,
    /// NPC_/RACE/ARMO/ARMA/OTFT/LVLI/LVLN/WEAP of the load order.
    pub db: Arc<sds_records::Db>,
    /// Precision settings and the ini files they came from.
    pub precision: (sds_anno::precision::Settings, Vec<String>),
    /// SDSCombat.ini of the load order.
    pub sdscombat: (sds_anno::sdscombat::Settings, Option<String>),
    /// Payload Interpreter aliases.
    pub pie: Arc<crate::pie::Aliases>,
    /// Precision trail settings ([Trails] of the MCM ini), toml trail definitions and the files they came from.
    pub trails: (crate::fx::TrailSettings, Vec<crate::fx::TrailDef>, Vec<String>),
    pub seconds: f32,
}

pub struct Loading {
    pub rx: Receiver<Result<Loaded, String>>,
    pub progress: Arc<Progress>,
    pub started: std::time::Instant,
}

/// Scene layer switches (the DSAS "Scene" panel).
pub struct SceneOptions {
    pub root_motion: bool,
    pub show_path: bool,
    pub show_grid: bool,
    /// SDSCombat annotations take part only when the DLL is installed; this
    /// can turn them off even then.
    pub sdscombat: bool,
    pub hidden: std::collections::HashSet<sds_anno::Category>,
    pub view3d: bool,
    pub show_body: bool,
    pub show_skeleton: bool,
    pub follow: bool,
    pub show_precision: bool,
    /// Size capsules as the player (Precision's player multipliers and recoil switch).
    pub precision_player: bool,
    /// Target dummy (the player): distance and angle from the NPC's start
    /// (0° = straight ahead, + = to its right).
    pub target_dist: f32,
    pub target_angle: f32,
    pub show_target: bool,
    /// AI turning toward the target when no lock is on, °/s.
    pub ai_turn_speed: f32,
    /// Also draw the path without SDSCombat, for comparison.
    pub compare_raw: bool,
    /// SCAR_ActionData sectors on the ground around the NPC.
    pub show_scar: bool,
    /// Precision weapon trails.
    pub show_trails: bool,
    /// Effects from PIE annotations.
    pub show_fx: bool,
}

/// Orbit camera around `target` (Z up). Yaw is measured from +Y, the way
/// the NPC faces at the clip start.
pub struct Camera {
    pub yaw: f32,
    pub pitch: f32,
    pub dist: f32,
    pub target: [f32; 3],
}

pub struct Playback {
    pub time: f32,
    pub playing: bool,
    pub looping: bool,
    pub speed: f32,
}

pub struct StudioApp {
    pub settings: Settings,
    pub dock: DockState<Tab>,
    pub loaded: Option<Loaded>,
    pub loading: Option<Loading>,
    pub doc: Option<ClipDoc>,
    pub selected: Option<usize>,
    pub clipboard: Vec<Anno>,
    pub play: Playback,
    pub scene: SceneOptions,
    pub timeline: TimelineView,
    pub log: Vec<String>,
    pub lib_filter: String,
    pub lib_mode_oar: bool,
    pub list_filter: String,
    /// The custom-annotation window: its text and whether it is open.
    pub custom_anno: String,
    pub custom_open: bool,
    /// The favorites window and its «Add Favorite» field.
    pub favs_open: bool,
    pub fav_new: String,
    pub profiles: Vec<String>,
    pub shot: Option<Shot>,
    pub camera: Camera,
    pub gpu: Option<crate::render::Gpu>,
    /// Decoded pose data of the open clip.
    pub anim: Option<sds_hkx::anim::Animation>,
    pub character: Option<crate::character::Character>,
    /// Bumped on every character load: GPU mesh ids include it.
    pub character_gen: u64,
    /// The NPC being shown, when one is picked.
    pub npc: Option<NpcChoice>,
    /// OAR folders loaded from disk (outside the load order).
    pub oar_external: Vec<crate::oar::OarMod>,
    /// Selected submod (its folder key) and its edit buffer.
    pub oar_edit: Option<crate::oar_panel::OarEdit>,
    pub npc_filter: String,
    /// The open project file, if the session was saved or opened as one.
    pub project_path: Option<PathBuf>,
    /// A project waiting for the load order to finish loading.
    pub pending_project: Option<crate::project::Project>,
    /// Running effect simulations and loaded effect NIFs.
    pub fx: crate::fx::FxPlayer,
    /// Sound events played while the clip plays.
    pub sound: crate::sound::SoundSim,
    /// Unsaved annotations of clips switched away from, by source: they
    /// come back when the clip is opened again.
    pub drafts: std::collections::HashMap<String, Vec<crate::doc::Anno>>,
    /// An action waiting for text fields to give up focus (frames left).
    deferred: Option<(u8, Deferred)>,
    /// SCAR combo graph and simulator.
    pub combo: crate::combo_panel::ComboState,
    /// Annotation to select once the next clip opens: (text fragment, time).
    pub select_after_open: Option<(String, f32)>,
    last_frame: Option<f64>,
}

/// Actions that must not run while a text field still holds an edit.
pub enum Deferred {
    Save,
    Open(ClipSource),
}

fn source_key(s: &ClipSource) -> String {
    match s {
        ClipSource::Vfs(p) => format!("vfs:{}", p.to_ascii_lowercase()),
        ClipSource::Disk(p) => format!("disk:{}", p.display()),
    }
}

pub struct NpcChoice {
    pub look: sds_records::npc::NpcLook,
    /// Index into `look.weapons`, None = empty hands.
    pub weapon: Option<usize>,
}

/// `--screenshot <png> [--open <vfs path>] [--time <s>] [--select <i>]`: wait
/// for the load order, open, render a few frames, save the window, quit.
pub struct Shot {
    pub path: PathBuf,
    pub open: Option<String>,
    pub time: Option<f32>,
    pub select: Option<usize>,
    pub frames: u32,
    pub opened: bool,
    /// `--save`: run Save HKX right after opening (tests the writer end to end).
    pub save: bool,
    /// `--npc <EditorID>`: dress the character as this NPC.
    pub npc: Option<String>,
    /// `--select-text <substring>`: select the first annotation containing it.
    pub select_text: Option<String>,
    /// `--save-project <path>`: write the state as a project before the shot.
    pub save_project: Option<PathBuf>,
}

impl StudioApp {
    pub fn new() -> StudioApp {
        let settings = Settings::load();
        settings.apply_language();
        let dock = std::fs::read(settings::layout_path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_else(default_layout);
        let mut app = StudioApp {
            dock,
            loaded: None,
            loading: None,
            doc: None,
            selected: None,
            clipboard: Vec::new(),
            play: Playback { time: 0.0, playing: false, looping: true, speed: 1.0 },
            scene: SceneOptions {
                root_motion: true,
                show_path: true,
                show_grid: true,
                sdscombat: true,
                hidden: Default::default(),
                view3d: true,
                show_body: true,
                show_skeleton: false,
                follow: true,
                show_precision: true,
                precision_player: false,
                target_dist: 300.0,
                target_angle: 0.0,
                show_target: true,
                ai_turn_speed: 0.0,
                compare_raw: true,
                show_scar: true,
                show_trails: true,
                show_fx: true,
            },
            camera: Camera { yaw: 25f32.to_radians(), pitch: 12f32.to_radians(), dist: 320.0, target: [0.0, 0.0, 80.0] },
            gpu: None,
            anim: None,
            character: None,
            character_gen: 0,
            npc: None,
            oar_external: Vec::new(),
            oar_edit: None,
            npc_filter: String::new(),
            project_path: None,
            pending_project: None,
            drafts: Default::default(),
            deferred: None,
            combo: Default::default(),
            select_after_open: None,
            fx: crate::fx::FxPlayer::default(),
            sound: Default::default(),
            timeline: TimelineView::default(),
            log: Vec::new(),
            lib_filter: String::new(),
            lib_mode_oar: false,
            list_filter: String::new(),
            custom_anno: String::new(),
            custom_open: false,
            favs_open: false,
            fav_new: String::new(),
            profiles: Vec::new(),
            shot: None,
            last_frame: None,
            settings,
        };
        app.refresh_profiles();
        if app.settings.load_on_start {
            app.start_loading();
        }
        app
    }

    pub fn log(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        self.log.push(msg);
        if self.log.len() > 2000 {
            self.log.drain(..500);
        }
    }

    pub fn refresh_profiles(&mut self) {
        self.profiles = sds_vfs::Mo2Instance::open(std::path::Path::new(&self.settings.mo2_dir))
            .map(|m| m.profiles())
            .unwrap_or_default();
    }

    pub fn start_loading(&mut self) {
        let source = if self.settings.use_mo2 {
            Source::Mo2 { instance: PathBuf::from(&self.settings.mo2_dir), profile: self.settings.profile.clone() }
        } else {
            Source::DataDir(PathBuf::from(&self.settings.data_dir))
        };
        self.log(sds_i18n::tf!("Загрузка сборки: {source:?}", "Loading the build: {source:?}"));
        let progress = Arc::new(Progress::default());
        let (tx, rx) = channel();
        let p = progress.clone();
        std::thread::spawn(move || {
            let t0 = std::time::Instant::now();
            let r = Vfs::build(&source, &p).map_err(|e| e.to_string()).map(|vfs| {
                let library = Library::build(&vfs);
                let db = sds_records::Db::build(&vfs, &game_language(vfs.profile_dir.as_deref()));
                let precision = crate::precision_view::load_settings(&vfs);
                let sdscombat = crate::sds_path::load_settings(&vfs);
                let pie = Arc::new(crate::pie::Aliases::load(&vfs));
                let trails = load_trails(&vfs);
                Loaded { vfs: Arc::new(vfs), library, db: Arc::new(db), precision, sdscombat, pie, trails, seconds: t0.elapsed().as_secs_f32() }
            });
            let _ = tx.send(r);
        });
        self.loading = Some(Loading { rx, progress, started: std::time::Instant::now() });
    }

    fn poll_loading(&mut self) {
        let Some(l) = &self.loading else { return };
        match l.rx.try_recv() {
            Ok(Ok(loaded)) => {
                let v = &loaded.vfs;
                let msg = sds_i18n::tf!("Сборка загружена за {:.1} с: {} модов, {} BSA, {} плагинов, {} файлов, {} анимаций, {} OAR-модов, {} NPC. SDSCombat.dll: {}", "Build loaded in {:.1} s: {} mods, {} BSA, {} plugins, {} files, {} animations, {} OAR mods, {} NPCs. SDSCombat.dll: {}",
                    loaded.seconds,
                    v.roots.len(),
                    v.archives.len(),
                    v.plugins.len(),
                    v.file_count(),
                    loaded.library.anims.len(),
                    loaded.library.oar.len(),
                    loaded.db.by_sig.get(b"NPC_").map_or(0, |v| v.len()),
                    if v.has_sdscombat() { sds_i18n::t!("есть", "yes") } else { sds_i18n::t!("нет", "no") }
                );
                let mut warnings = v.warnings.clone();
                warnings.extend(loaded.db.warnings.iter().cloned());
                self.loaded = Some(loaded);
                if let Some(p) = self.pending_project.take() {
                    crate::project::apply(self, &p);
                    self.log(sds_i18n::t!("Проект восстановлен", "Project restored"));
                }
                self.loading = None;
                self.log(msg);
                for w in warnings {
                    self.log(format!("BSA: {w}"));
                }
            }
            Ok(Err(e)) => {
                self.loading = None;
                self.log(sds_i18n::tf!("Ошибка загрузки сборки: {e}", "Build failed to load: {e}"));
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.loading = None;
                self.log(sds_i18n::t!("Загрузка сборки прервалась", "Build loading was interrupted"));
            }
        }
    }

    pub fn sdscombat_active(&self) -> bool {
        self.scene.sdscombat && self.loaded.as_ref().is_some_and(|l| l.vfs.has_sdscombat())
    }

    /// Runs `d` two frames from now, after any focused text field has
    /// committed its edit (Ctrl+S while typing, a click on another clip).
    pub fn defer(&mut self, d: Deferred) {
        self.deferred = Some((2, d));
    }

    fn run_deferred(&mut self, ctx: &egui::Context) {
        let Some((n, _)) = &mut self.deferred else { return };
        ctx.memory_mut(|m| m.stop_text_input());
        ctx.request_repaint();
        if *n > 0 {
            *n -= 1;
            return;
        }
        match self.deferred.take().map(|(_, d)| d) {
            Some(Deferred::Save) => self.save_hkx(),
            Some(Deferred::Open(ClipSource::Vfs(p))) => self.open_vfs_file(&p),
            Some(Deferred::Open(ClipSource::Disk(p))) => self.open_disk_file(p),
            None => {}
        }
    }

    /// The studio's own saved copy in the output folder, if there is one:
    /// it wins over the load order, so saved edits show when reopened even
    /// when the output mod is not enabled in MO2.
    pub fn output_copy(&self, vfs_path: &str) -> Option<PathBuf> {
        if self.settings.output_dir.trim().is_empty() {
            return None;
        }
        let p = PathBuf::from(&self.settings.output_dir).join(vfs_path.replace('/', std::path::MAIN_SEPARATOR_STR));
        p.is_file().then_some(p)
    }

    pub fn open_vfs_file(&mut self, path: &str) {
        let Some(loaded) = &self.loaded else { return };
        let vfs = loaded.vfs.clone();
        let out = self.output_copy(path);
        let read = match &out {
            Some(o) => std::fs::read(o),
            None => vfs.read(path),
        };
        match read {
            Ok(bytes) => {
                let origin = match &out {
                    Some(_) => sds_i18n::tf!("папка вывода (поверх {})", "output folder (over {})", vfs.origin(path).unwrap_or_default()),
                    None => vfs.origin(path).unwrap_or_default(),
                };
                self.open_bytes(ClipSource::Vfs(path.to_string()), library::file_name(path).to_string(), origin, &bytes);
            }
            Err(e) => self.log(format!("{path}: {e}")),
        }
    }

    pub fn open_disk_file(&mut self, path: PathBuf) {
        match std::fs::read(&path) {
            Ok(bytes) => {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let origin = path.parent().map(|p| p.display().to_string()).unwrap_or_default();
                self.open_bytes(ClipSource::Disk(path), name, origin, &bytes);
            }
            Err(e) => self.log(format!("{}: {e}", path.display())),
        }
    }

    fn open_bytes(&mut self, source: ClipSource, name: String, origin: String, bytes: &[u8]) {
        if let Some(d) = self.doc.as_ref().filter(|d| d.dirty()) {
            let (key, name, annos) = (source_key(&d.source), d.name.clone(), d.annos.clone());
            self.drafts.insert(key, annos);
            self.log(sds_i18n::tf!("Несохранённые правки {name} отложены — вернутся при открытии клипа (Ctrl+S — сохранить)", "Unsaved edits of {name} are kept aside — they come back when the clip is opened (Ctrl+S — save)"));
        } else if let Some(d) = &self.doc {
            self.drafts.remove(&source_key(&d.source));
        }
        match sds_hkx::read_clip(bytes) {
            Ok(info) => {
                self.log(sds_i18n::tf!("Открыт {name}: {} {:.3} с, {} кадров, {} треков, {} аннотаций", "Opened {name}: {} {:.3} s, {} frames, {} tracks, {} annotations",
                    info.kind,
                    info.duration,
                    info.frames,
                    info.transform_tracks,
                    info.annotations.len()
                ));
                let recent = match &source {
                    ClipSource::Vfs(p) => format!("vfs:{p}"),
                    ClipSource::Disk(p) => format!("disk:{}", p.display()),
                };
                self.settings.push_recent(recent);
                self.anim = match sds_hkx::anim::read_animation(bytes) {
                    Ok(a) => Some(a),
                    Err(e) => {
                        self.log(sds_i18n::tf!("Поза не декодирована: {e}", "Pose not decoded: {e}"));
                        None
                    }
                };
                let folder = match &source {
                    ClipSource::Vfs(p) => crate::character::actor_folder(p),
                    ClipSource::Disk(p) => {
                        let s = p.to_string_lossy().replace('\\', "/").to_lowercase();
                        s.find("/meshes/actors/").and_then(|i| crate::character::actor_folder(&s[i + 1..]))
                    }
                }
                .unwrap_or_else(|| "character".into());
                self.ensure_character(&folder);
                // A clip from an OAR submod selects that submod in the OAR tab.
                if let ClipSource::Vfs(p) = &source {
                    if let Some(i) = p.find("/openanimationreplacer/") {
                        let start = i + "/openanimationreplacer/".len();
                        let parts: Vec<&str> = p[start..].splitn(3, '/').collect();
                        if parts.len() == 3 {
                            let folder = format!("{}{}/{}", &p[..start], parts[0], parts[1]);
                            if self.oar_edit.as_ref().is_none_or(|e| e.folder != folder) {
                                crate::oar_panel::select(self, &folder);
                            }
                        }
                    }
                }
                let draft = self.drafts.remove(&source_key(&source));
                let mut doc = ClipDoc::new(source, name, origin, info);
                if let Some(annos) = draft {
                    doc.edit(None, |a| *a = annos);
                    self.log(sds_i18n::tf!("{}: восстановлены несохранённые правки", "{}: unsaved edits restored", doc.name));
                }
                self.selected = None;
                if let Some((text, time)) = self.select_after_open.take() {
                    self.selected = doc
                        .annos
                        .iter()
                        .enumerate()
                        .filter(|(_, a)| a.text.replace(' ', "").contains(&text.replace(' ', "")))
                        .min_by(|a, b| (a.1.time - time).abs().total_cmp(&(b.1.time - time).abs()))
                        .map(|(i, _)| i);
                }
                self.doc = Some(doc);
                self.play.time = 0.0;
                self.timeline.fit_next = true;
            }
            Err(e) => self.log(format!("{name}: {e}")),
        }
    }

    pub fn open_project(&mut self, path: &Path) {
        match crate::project::read(path) {
            Ok(p) => {
                if path != crate::project::autosave_path() {
                    self.project_path = Some(path.to_path_buf());
                    let s = path.display().to_string();
                    self.settings.recent_projects.retain(|r| *r != s);
                    self.settings.recent_projects.insert(0, s);
                    self.settings.recent_projects.truncate(12);
                }
                self.log(sds_i18n::tf!("Проект: {}", "Project: {}", path.display()));
                if self.loaded.is_some() {
                    crate::project::apply(self, &p);
                } else {
                    if self.loading.is_none() {
                        self.start_loading();
                    }
                    self.log(sds_i18n::t!("Проект откроется, когда загрузится сборка", "The project will open once the build has loaded"));
                    self.pending_project = Some(p);
                }
            }
            Err(e) => self.log(sds_i18n::tf!("Проект не открыт: {e}", "Project not opened: {e}")),
        }
    }

    pub fn save_project(&mut self, ask: bool) {
        let path = match (&self.project_path, ask) {
            (Some(p), false) => Some(p.clone()),
            _ => {
                let name = self.doc.as_ref().map(|d| d.name.trim_end_matches(".hkx").to_string()).unwrap_or_else(|| sds_i18n::t!("проект", "project").into());
                let dir = crate::project::default_dir();
                let _ = std::fs::create_dir_all(&dir);
                rfd::FileDialog::new()
                    .add_filter(sds_i18n::t!("Проект SDS Anim Studio", "SDS Anim Studio project"), &[crate::project::EXT])
                    .set_directory(dir)
                    .set_file_name(format!("{name}.{}", crate::project::EXT))
                    .save_file()
            }
        };
        let Some(path) = path else { return };
        match crate::project::write(&crate::project::capture(self), &path) {
            Ok(()) => {
                self.log(sds_i18n::tf!("Проект сохранён: {}", "Project saved: {}", path.display()));
                let s = path.display().to_string();
                self.settings.recent_projects.retain(|r| *r != s);
                self.settings.recent_projects.insert(0, s);
                self.settings.recent_projects.truncate(12);
                self.project_path = Some(path);
            }
            Err(e) => self.log(sds_i18n::tf!("Проект не сохранён: {e}", "Project not saved: {e}")),
        }
    }

    /// Load the skeleton and default body of an actor folder, once.
    pub fn ensure_character(&mut self, folder: &str) {
        if self.npc.is_some() {
            return;
        }
        let id = format!("folder:{folder}");
        if self.character.as_ref().is_some_and(|c| c.id == id) {
            return;
        }
        let Some(loaded) = &self.loaded else { return };
        let vfs = loaded.vfs.clone();
        match crate::character::Character::for_folder(&vfs, folder) {
            Ok(c) => {
                for n in c.notes.clone() {
                    self.log(n);
                }
                self.log(sds_i18n::tf!("Персонаж «{folder}»: {} мешей", "Character «{folder}»: {} meshes", c.meshes.len()));
                self.character = Some(c);
                self.character_gen += 1;
                if let Some(g) = &mut self.gpu {
                    g.clear_meshes();
                }
            }
            Err(e) => {
                self.log(sds_i18n::tf!("Персонаж «{folder}» не загружен: {e}", "Character «{folder}» not loaded: {e}"));
                self.character = None;
            }
        }
    }

    /// Dress the character as an NPC from the load order.
    pub fn set_npc(&mut self, key: sds_records::FormKey) {
        let Some(loaded) = &self.loaded else { return };
        let Some(look) = loaded.db.npc_look(key) else {
            return self.log(sds_i18n::tf!("NPC {} не собран", "NPC {} not assembled", loaded.db.label(key)));
        };
        // Weapon in hand: the first melee one, else the first.
        let weapon = look.weapons.iter().position(|w| (1..=6).contains(&w.anim_type)).or(if look.weapons.is_empty() { None } else { Some(0) });
        self.npc = Some(NpcChoice { look, weapon });
        self.rebuild_npc();
    }

    pub fn clear_npc(&mut self) {
        self.npc = None;
        self.character = None;
        let folder = match self.doc.as_ref().map(|d| &d.source) {
            Some(ClipSource::Vfs(p)) => crate::character::actor_folder(p),
            _ => None,
        }
        .unwrap_or_else(|| "character".into());
        self.ensure_character(&folder);
    }

    pub fn rebuild_npc(&mut self) {
        let (Some(loaded), Some(choice)) = (&self.loaded, &self.npc) else { return };
        let vfs = loaded.vfs.clone();
        let db = loaded.db.clone();
        let look = choice.look.clone();
        let weapon_choice = choice.weapon;
        let look = &look;
        use crate::character::Piece;
        use sds_records::npc::{PartKind, SLOT_HAIR, SLOT_HEAD};
        let used: u32 = look.parts.iter().filter(|p| p.kind == PartKind::Armor).map(|p| p.slots).fold(0, |a, b| a | b);
        let mut pieces: Vec<Piece> = look
            .parts
            .iter()
            .map(|p| Piece { label: p.source.clone(), path: p.model.clone(), attach: None, hide_shapes: Vec::new(), visible: true })
            .collect();
        let mut notes = look.notes.clone();
        if used & SLOT_HEAD == 0 {
            let face = look.facegen.iter().find(|f| vfs.exists(f)).cloned();
            if face.is_none() {
                notes.push(sds_i18n::t!("FaceGen не найден — общая голова расы", "FaceGen not found — generic race head").into());
            }
            pieces.push(Piece {
                label: if face.is_some() { "FaceGen".into() } else { sds_i18n::t!("голова расы", "race head").into() },
                path: face.unwrap_or_else(|| look.head_fallback.clone()),
                attach: None,
                hide_shapes: if used & SLOT_HAIR != 0 { vec!["hair".into()] } else { Vec::new() },
                visible: true,
            });
        }
        if let Some(w) = weapon_choice.and_then(|i| look.weapons.get(i)) {
            pieces.push(Piece { label: format!("{} ({})", w.edid, w.type_name()), path: w.model.clone(), attach: Some(w.hand_node().into()), hide_shapes: Vec::new(), visible: true });
        }
        let id = format!("npc:{}|{:?}", db.label(look.key), weapon_choice);
        let label = format!("{} «{}»", look.edid, look.name);
        match crate::character::Character::build(&vfs, &id, &look.skeleton_hkx, Some(&look.skeleton_nif), pieces, look.scale) {
            Ok(mut c) => {
                c.notes.extend(notes);
                for n in c.notes.clone() {
                    self.log(n);
                }
                self.log(sds_i18n::tf!("NPC {label}: раса {}, масштаб {:.2}, {} мешей", "NPC {label}: race {}, scale {:.2}, {} meshes", look.race, look.scale, c.meshes.len()));
                self.character = Some(c);
                self.character_gen += 1;
                if let Some(g) = &mut self.gpu {
                    g.clear_meshes();
                }
            }
            Err(e) => self.log(sds_i18n::tf!("NPC {label} не собран: {e}", "NPC {label} not assembled: {e}")),
        }
    }

    /// Write the edited annotations into a copy of the clip in the output
    /// mod folder (same Data-relative path). The source mod is never touched.
    pub fn save_hkx(&mut self) {
        let (Some(doc), Some(rel)) = (&self.doc, self.output_rel_path()) else { return };
        let original = match &doc.source {
            ClipSource::Vfs(p) => match &self.loaded {
                Some(l) => l.vfs.read(p),
                None => Err(std::io::Error::other(sds_i18n::t!("сборка не загружена", "build not loaded"))),
            },
            ClipSource::Disk(p) => std::fs::read(p),
        };
        let original = match original {
            Ok(b) => b,
            Err(e) => return self.log(sds_i18n::tf!("Сохранение: исходный файл не прочитан: {e}", "Save: source file not read: {e}")),
        };
        let annos: Vec<sds_hkx::Annotation> =
            doc.annos.iter().map(|a| sds_hkx::Annotation { time: a.time, text: a.text.clone(), track: a.track }).collect();
        let out = PathBuf::from(&self.settings.output_dir).join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
        if let ClipSource::Disk(p) = &doc.source {
            if std::fs::canonicalize(p).ok() == std::fs::canonicalize(&out).ok() {
                return self.log(sds_i18n::t!("Сохранение отменено: папка вывода совпадает с исходным файлом", "Save cancelled: the output folder is the source file"));
            }
        }
        match sds_hkx::write::write_annotations(&original, &annos) {
            Ok(bytes) => {
                let r = out.parent().map(std::fs::create_dir_all).unwrap_or(Ok(())).and_then(|_| std::fs::write(&out, &bytes));
                match r {
                    Ok(()) => {
                        self.log(sds_i18n::tf!("Сохранено ({} аннотаций, проверено чтением): {}", "Saved ({} annotations, verified by reading back): {}", annos.len(), out.display()));
                        if let Some(d) = &mut self.doc {
                            d.mark_saved();
                        }
                    }
                    Err(e) => self.log(sds_i18n::tf!("Сохранение не удалось: {}: {e}", "Save failed: {}: {e}", out.display())),
                }
            }
            Err(e) => self.log(sds_i18n::tf!("Сохранение отменено, файл не записан: {e}", "Save cancelled, file not written: {e}")),
        }
    }

    pub fn open_recent(&mut self, item: &str) {
        if let Some(p) = item.strip_prefix("vfs:") {
            self.open_vfs_file(&p.to_string());
        } else if let Some(p) = item.strip_prefix("disk:") {
            self.open_disk_file(PathBuf::from(p));
        }
    }

    /// Data-relative path the output file gets.
    pub fn output_rel_path(&self) -> Option<String> {
        let doc = self.doc.as_ref()?;
        Some(match &doc.source {
            ClipSource::Vfs(p) => p.clone(),
            ClipSource::Disk(p) => {
                let s = p.to_string_lossy().replace('\\', "/");
                let lower = s.to_lowercase();
                match lower.find("/meshes/") {
                    Some(i) => s[i + 1..].to_string(),
                    None => format!("meshes/{}", doc.name),
                }
            }
        })
    }

    pub fn export_anno_txt(&mut self) {
        let (Some(doc), Some(rel)) = (&self.doc, self.output_rel_path()) else { return };
        let out = PathBuf::from(&self.settings.output_dir).join(rel.replace(".hkx", ".txt"));
        let text = doc.to_hkanno_txt();
        let r = out.parent().map(std::fs::create_dir_all).unwrap_or(Ok(())).and_then(|_| std::fs::write(&out, text));
        match r {
            Ok(()) => self.log(sds_i18n::tf!("Экспорт аннотаций: {}", "Annotations exported: {}", out.display())),
            Err(e) => self.log(sds_i18n::tf!("Экспорт не удался: {}: {e}", "Export failed: {}: {e}", out.display())),
        }
    }

    // ---- editing -------------------------------------------------------

    pub fn snap(&self, t: f32) -> f32 {
        match &self.doc {
            Some(d) if self.settings.snap_to_frames => {
                let ft = d.frame_time();
                ((t / ft).round() * ft).clamp(0.0, d.duration())
            }
            Some(d) => t.clamp(0.0, d.duration()),
            None => t,
        }
    }

    pub fn delete_selected(&mut self) {
        let (Some(doc), Some(i)) = (&mut self.doc, self.selected) else { return };
        doc.edit(None, |a| {
            if i < a.len() {
                a.remove(i);
            }
        });
        self.selected = None;
    }

    pub fn copy_selected(&mut self) {
        if let (Some(doc), Some(i)) = (&self.doc, self.selected) {
            if let Some(a) = doc.annos.get(i) {
                self.clipboard = vec![a.clone()];
            }
        }
    }

    /// Paste at the playhead, keeping relative times.
    pub fn paste(&mut self) {
        if self.clipboard.is_empty() {
            return;
        }
        let t = self.snap(self.play.time);
        let base = self.clipboard[0].time;
        let items: Vec<Anno> = self.clipboard.iter().map(|a| Anno { time: t + a.time - base, ..a.clone() }).collect();
        let Some(doc) = &mut self.doc else { return };
        let first = items[0].clone();
        doc.edit(None, |a| a.extend(items));
        self.selected = doc.annos.iter().position(|a| *a == first);
    }

    /// Opens what an add menu asked for.
    pub fn open_popup(&mut self, p: Option<crate::timeline::Popup>) {
        match p {
            Some(crate::timeline::Popup::Custom) => self.custom_open = true,
            Some(crate::timeline::Popup::Favorites) => self.favs_open = true,
            None => {}
        }
    }

    /// Several annotations as one undo step (template groups); selects the first.
    pub fn add_annotations(&mut self, items: Vec<(String, f32)>) {
        if items.is_empty() {
            return;
        }
        let items: Vec<Anno> = items.into_iter().map(|(text, t)| Anno { time: self.snap(t), text, track: 0 }).collect();
        let Some(doc) = &mut self.doc else { return };
        let first = items[0].clone();
        doc.edit(None, |a| a.extend(items));
        self.selected = doc.annos.iter().position(|a| *a == first);
    }

    pub fn add_annotation(&mut self, text: String) {
        let t = self.snap(self.play.time);
        let Some(doc) = &mut self.doc else { return };
        let new = Anno { time: t, text, track: 0 };
        doc.edit(None, |a| a.push(new.clone()));
        self.selected = doc.annos.iter().position(|a| *a == new);
    }

    pub fn undo(&mut self) {
        if let Some(d) = &mut self.doc {
            if d.undo() {
                self.selected = None;
            }
        }
    }

    pub fn redo(&mut self) {
        if let Some(d) = &mut self.doc {
            if d.redo() {
                self.selected = None;
            }
        }
    }

    /// Brings a tab to the front, opening it if the layout has none.
    pub fn show_tab(&mut self, tab: Tab) {
        match self.dock.find_tab(&tab) {
            Some(at) => {
                let _ = self.dock.set_active_tab(at);
            }
            None => self.dock.push_to_focused_leaf(tab),
        }
    }

    // ---- frame --------------------------------------------------------

    fn advance_time(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        let dt = self.last_frame.map(|l| (now - l) as f32).unwrap_or(0.0).min(0.1);
        self.last_frame = Some(now);
        let Some(doc) = &self.doc else {
            self.sound_pause();
            return;
        };
        if !self.play.playing {
            self.sound_pause();
            return;
        }
        let dur = doc.duration().max(1e-3);
        let prev = self.play.time;
        self.play.time += dt * self.play.speed;
        let mut wrapped = false;
        if self.play.time > dur {
            if self.play.looping {
                self.play.time %= dur;
                wrapped = true;
            } else {
                self.play.time = dur;
                self.play.playing = false;
            }
        }
        let now = self.play.time;
        self.sound_tick(prev, now, wrapped, dur);
        if !self.play.playing {
            self.sound_ended();
        }
        ctx.request_repaint();
    }

    pub fn step_frames(&mut self, n: i32) {
        let Some(doc) = &self.doc else { return };
        let ft = doc.frame_time();
        let f = (self.play.time / ft).round() as i32 + n;
        self.play.time = (f.clamp(0, doc.last_frame() as i32) as f32 * ft).min(doc.duration());
        self.play.playing = false;
    }

    fn take_screenshot(&mut self, ctx: &egui::Context) {
        let Some(shot) = &mut self.shot else { return };
        ctx.request_repaint();
        if self.loading.is_some() {
            return;
        }
        if !shot.opened {
            shot.opened = true;
            let (open, time, select, npc) = (shot.open.clone(), shot.time, shot.select, shot.npc.clone());
            if let (Some(edid), Some(l)) = (npc, &self.loaded) {
                let db = l.db.clone();
                let key = db.by_sig.get(b"NPC_").and_then(|v| v.iter().copied().find(|k| db.get(*k).is_some_and(|r| r.edid().eq_ignore_ascii_case(&edid))));
                match key {
                    Some(k) => self.set_npc(k),
                    None => self.log(sds_i18n::tf!("NPC {edid} не найден", "NPC {edid} not found")),
                }
            }
            if let Some(p) = open {
                if std::path::Path::new(&p).is_file() {
                    self.open_disk_file(PathBuf::from(p));
                } else {
                    self.open_vfs_file(&p);
                }
            }
            if let Some(time) = time {
                self.play.time = time;
            }
            if select.is_some() {
                self.selected = select;
            }
            if let (Some(s), Some(d)) = (self.shot.as_ref().and_then(|s| s.select_text.clone()), &self.doc) {
                self.selected = d.annos.iter().position(|a| a.text.contains(&s));
            }
            if self.shot.as_ref().is_some_and(|s| s.save) {
                self.add_annotation("SDS_NPC_TurnSpeed.0.5".into());
                self.save_hkx();
            }
            return;
        }
        if shot.frames > 0 {
            shot.frames -= 1;
            if shot.frames == 0 {
                let save_to = shot.save_project.take();
                if let Some(p) = save_to {
                    let proj = crate::project::capture(self);
                    let r = crate::project::write(&proj, &p);
                    eprintln!("PROJECT {} {:?}", p.display(), r);
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
            }
            return;
        }
        let image = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        let Some(image) = image else { return };
        let (w, h) = (image.size[0] as u32, image.size[1] as u32);
        let mut rgba = Vec::with_capacity(image.pixels.len() * 4);
        for p in &image.pixels {
            rgba.extend_from_slice(&[p.r(), p.g(), p.b(), 255]);
        }
        if let Some(png) = aether_core::png::encode_rgba(w, h, &rgba) {
            let _ = std::fs::write(&shot.path, png);
            eprintln!("SCREENSHOT {w}x{h} -> {}", shot.path.display());
        }
        for l in &self.log {
            eprintln!("LOG {l}");
        }
        self.shot = None;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        // Text fields own the keyboard while focused.
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        use egui::{Key, Modifiers};
        let (space, left, right, home, end, del, z, y, c, v, s) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::Space),
                i.consume_key(Modifiers::NONE, Key::ArrowLeft),
                i.consume_key(Modifiers::NONE, Key::ArrowRight),
                i.consume_key(Modifiers::NONE, Key::Home),
                i.consume_key(Modifiers::NONE, Key::End),
                i.consume_key(Modifiers::NONE, Key::Delete),
                i.consume_key(Modifiers::COMMAND, Key::Z),
                i.consume_key(Modifiers::COMMAND, Key::Y),
                i.consume_key(Modifiers::COMMAND, Key::C),
                i.consume_key(Modifiers::COMMAND, Key::V),
                i.consume_key(Modifiers::COMMAND, Key::S),
            )
        });
        if space {
            self.play.playing = !self.play.playing;
        }
        if left {
            self.step_frames(-1);
        }
        if right {
            self.step_frames(1);
        }
        if home {
            self.play.time = 0.0;
        }
        if end {
            if let Some(d) = &self.doc {
                self.play.time = d.duration();
            }
        }
        if del {
            self.delete_selected();
        }
        if z {
            self.undo();
        }
        if y {
            self.redo();
        }
        if c {
            self.copy_selected();
        }
        if v {
            self.paste();
        }
        if s {
            self.defer(Deferred::Save);
        }
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button(sds_i18n::t!("Файл", "File"), |ui| {
                if ui.button(sds_i18n::t!("Открыть проект…", "Open project…")).clicked() {
                    ui.close();
                    if let Some(p) = rfd::FileDialog::new().add_filter(sds_i18n::t!("Проект SDS Anim Studio", "SDS Anim Studio project"), &[crate::project::EXT]).set_directory(crate::project::default_dir()).pick_file() {
                        self.open_project(&p);
                    }
                }
                let save_label = match &self.project_path {
                    Some(p) => sds_i18n::tf!("Сохранить проект ({})", "Save project ({})", p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
                    None => sds_i18n::t!("Сохранить проект…", "Save project…").to_string(),
                };
                if ui.button(save_label).clicked() {
                    ui.close();
                    self.save_project(false);
                }
                if ui.button(sds_i18n::t!("Сохранить проект как…", "Save project as…")).clicked() {
                    ui.close();
                    self.save_project(true);
                }
                ui.menu_button(sds_i18n::t!("Недавние проекты", "Recent projects"), |ui| {
                    for r in self.settings.recent_projects.clone() {
                        if ui.button(&r).clicked() {
                            ui.close();
                            self.open_project(Path::new(&r));
                        }
                    }
                });
                if ui.add_enabled(crate::project::autosave_path().is_file(), egui::Button::new(sds_i18n::t!("Восстановить последнюю сессию", "Restore last session"))).on_hover_text(sds_i18n::t!("Состояние на момент последнего закрытия студии", "The state when the studio was last closed")).clicked() {
                    ui.close();
                    self.open_project(&crate::project::autosave_path());
                }
                ui.separator();
                if ui.button(sds_i18n::t!("Открыть HKX с диска…", "Open HKX from disk…")).clicked() {
                    ui.close();
                    if let Some(p) = rfd::FileDialog::new().add_filter("Havok", &["hkx"]).pick_file() {
                        self.open_disk_file(p);
                    }
                }
                ui.menu_button(sds_i18n::t!("Недавние", "Recent"), |ui| {
                    for r in self.settings.recent.clone() {
                        if ui.button(&r).clicked() {
                            ui.close();
                            self.open_recent(&r);
                        }
                    }
                });
                ui.separator();
                let has_doc = self.doc.is_some();
                if ui.add_enabled(has_doc, egui::Button::new(sds_i18n::t!("Сохранить HKX в папку вывода   Ctrl+S", "Save HKX to the output folder   Ctrl+S"))).clicked() {
                    ui.close();
                    self.defer(Deferred::Save);
                }
                if ui.add_enabled(has_doc, egui::Button::new(sds_i18n::t!("Экспорт anno.txt (hkanno) в папку вывода", "Export anno.txt (hkanno) to the output folder"))).clicked() {
                    ui.close();
                    self.export_anno_txt();
                }
            });
            ui.menu_button(sds_i18n::t!("Правка", "Edit"), |ui| {
                let can_undo = self.doc.as_ref().is_some_and(|d| d.can_undo());
                let can_redo = self.doc.as_ref().is_some_and(|d| d.can_redo());
                if ui.add_enabled(can_undo, egui::Button::new(sds_i18n::t!("Отменить   Ctrl+Z", "Undo   Ctrl+Z"))).clicked() {
                    self.undo();
                }
                if ui.add_enabled(can_redo, egui::Button::new(sds_i18n::t!("Повторить   Ctrl+Y", "Redo   Ctrl+Y"))).clicked() {
                    self.redo();
                }
                ui.separator();
                if ui.button(sds_i18n::t!("Копировать   Ctrl+C", "Copy   Ctrl+C")).clicked() {
                    self.copy_selected();
                }
                if ui.button(sds_i18n::t!("Вставить на плейхед   Ctrl+V", "Paste at playhead   Ctrl+V")).clicked() {
                    self.paste();
                }
                if ui.button(sds_i18n::t!("Удалить   Del", "Delete   Del")).clicked() {
                    self.delete_selected();
                }
                ui.separator();
                ui.checkbox(&mut self.settings.snap_to_frames, sds_i18n::t!("Привязка к кадрам", "Snap to frames"));
            });
            ui.menu_button(sds_i18n::t!("Вид", "View"), |ui| {
                for tab in Tab::ALL {
                    let open = self.dock.find_tab(&tab).is_some();
                    if ui.add_enabled(!open, egui::Button::new(tab.title())).clicked() {
                        self.dock.push_to_focused_leaf(tab);
                    }
                }
                ui.separator();
                if ui.button(sds_i18n::t!("Сбросить раскладку", "Reset layout")).clicked() {
                    self.dock = default_layout();
                }
            });
            ui.menu_button("SCAR", |ui| {
                if ui.button(sds_i18n::t!("Открыть вкладку SCAR", "Open the SCAR tab")).clicked() {
                    self.show_tab(Tab::Scar);
                    ui.close();
                }
                if ui.button(sds_i18n::t!("Граф комбо и симулятор", "Combo graph and simulator")).clicked() {
                    self.show_tab(Tab::Combo);
                    ui.close();
                }
                ui.checkbox(&mut self.scene.show_scar, sds_i18n::t!("Зоны SCAR в 3D", "SCAR zones in 3D"));
            });
            // Language switch, right-aligned in the menu bar.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                for (code, label) in [("en", "EN"), ("ru", "RU")] {
                    let on = self.settings.language.eq_ignore_ascii_case(code);
                    if ui.selectable_label(on, label).on_hover_text("Язык интерфейса / Interface language").clicked() && !on {
                        self.settings.language = code.into();
                        self.settings.apply_language();
                        self.settings.save();
                    }
                }
            });
        });
    }
}

impl eframe::App for StudioApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_loading();
        if self.loading.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        self.advance_time(&ctx);
        self.run_deferred(&ctx);
        self.shortcuts(&ctx);
        self.take_screenshot(&ctx);

        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        if let Some(p) = dropped.into_iter().find(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("hkx"))) {
            self.open_disk_file(p);
        }

        egui::Panel::top("menu").show_inside(ui, |ui| self.menu_bar(ui));
        egui::Panel::bottom("status").show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                match &self.doc {
                    Some(d) => {
                        ui.label(format!("{}{}", d.name, if d.dirty() { " •" } else { "" }));
                        ui.separator();
                        ui.label(&d.origin);
                    }
                    None => {
                        ui.label(sds_i18n::t!("Клип не открыт", "No clip open"));
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let sds = if self.sdscombat_active() { sds_i18n::t!("SDSCombat: вкл", "SDSCombat: on") } else { sds_i18n::t!("SDSCombat: выкл", "SDSCombat: off") };
                    ui.label(sds);
                    if let Some(l) = &self.loaded {
                        ui.separator();
                        ui.label(sds_i18n::tf!("{} анимаций", "{} animations", l.library.anims.len()));
                    }
                });
            });
        });

        let mut dock = std::mem::replace(&mut self.dock, DockState::new(vec![]));
        let style = Style::from_egui(ui.style().as_ref());
        DockArea::new(&mut dock)
            .id(egui::Id::new("sds-dock"))
            .show_leaf_close_all_buttons(false)
            .style(style)
            .show_inside(ui, &mut TabHost { app: self });
        self.dock = dock;
        crate::timeline::custom_window(self, &ctx);
        crate::timeline::favorites_window(self, &ctx);
    }

    fn on_exit(&mut self) {
        if self.shot.is_some() || std::env::args().any(|a| a == "--screenshot") {
            return;
        }
        // The session itself, for "Восстановить последнюю сессию".
        if self.doc.is_some() {
            let _ = crate::project::write(&crate::project::capture(self), &crate::project::autosave_path());
        }
        self.settings.save();
        if let Ok(s) = serde_json::to_string(&self.dock) {
            let _ = std::fs::write(settings::layout_path(), s);
        }
    }
}

struct TabHost<'a> {
    app: &'a mut StudioApp,
}

impl egui_dock::TabViewer for TabHost<'_> {
    type Tab = Tab;

    fn title(&mut self, tab: &mut Tab) -> egui::WidgetText {
        tab.title().into()
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Tab) {
        let app = &mut *self.app;
        match tab {
            Tab::Library => crate::panels::library(app, ui),
            Tab::Viewport => crate::panels::viewport(app, ui),
            Tab::Timeline => crate::timeline::show(app, ui),
            Tab::Inspector => crate::panels::inspector(app, ui),
            Tab::Annotations => crate::panels::annotation_list(app, ui),
            Tab::Scene => crate::panels::scene(app, ui),
            Tab::Log => crate::panels::log(app, ui),
            Tab::Npc => crate::npc_panel::show(app, ui),
            Tab::Oar => crate::oar_panel::show(app, ui),
            Tab::Scar => crate::scar_panel::show(app, ui),
            Tab::Combo => crate::combo_panel::show(app, ui),
        }
    }
}

/// `sLanguage` of the profile's Skyrim.ini (strings files are named after it).
fn game_language(profile: Option<&std::path::Path>) -> String {
    for name in ["Skyrim.ini", "SkyrimCustom.ini"] {
        if let Some(text) = profile.and_then(|p| std::fs::read(p.join(name)).ok()) {
            for l in String::from_utf8_lossy(&text).lines() {
                if let Some(v) = l.trim().strip_prefix("sLanguage=") {
                    return v.trim().to_string();
                }
            }
        }
    }
    "english".into()
}

/// Precision trail settings and every `[[TrailDefinition]]` of the load order's tomls.
fn load_trails(vfs: &Vfs) -> (crate::fx::TrailSettings, Vec<crate::fx::TrailDef>, Vec<String>) {
    let mut ts = crate::fx::TrailSettings::default();
    let mut used = Vec::new();
    for p in ["mcm/config/precision/settings.ini", "mcm/settings/precision.ini"] {
        if let Ok(b) = vfs.read(p) {
            ts.apply_ini(&String::from_utf8_lossy(&b));
        }
    }
    let mut defs = Vec::new();
    // Only the folder Precision reads (not its subfolders, e.g. backups).
    let mut files = vfs.paths(|p| p.starts_with("skse/plugins/precision/") && p.ends_with(".toml") && p["skse/plugins/precision/".len()..].find('/').is_none());
    files.sort();
    for f in files {
        if let Ok(b) = vfs.read(&f) {
            let d = crate::fx::parse_trail_defs(&String::from_utf8_lossy(&b), &f);
            if !d.is_empty() {
                used.push(format!("{f} ({})", d.len()));
            }
            defs.extend(d);
        }
    }
    (ts, defs, used)
}
