//! Projects: the studio's working state in a `.sdsproj` (JSON) file, so a
//! session can be closed and picked up where it stopped — the clip with its
//! unsaved annotation edits, the playhead, the NPC and weapon, the scene,
//! camera and timeline, external OAR folders.
//!
//! The load order itself is not stored, only which MO2 profile it came from;
//! a project opened while the build is still loading waits for it.

use crate::app::StudioApp;
use crate::doc::{Anno, ClipSource};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const EXT: &str = "sdsproj";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Source {
    Vfs(String),
    Disk(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SceneState {
    pub root_motion: bool,
    pub show_path: bool,
    pub show_grid: bool,
    pub sdscombat: bool,
    pub hidden: Vec<String>,
    pub view3d: bool,
    pub show_body: bool,
    pub show_skeleton: bool,
    pub follow: bool,
    pub show_precision: bool,
    pub show_trails: bool,
    pub show_fx: bool,
    pub precision_player: bool,
    pub target_dist: f32,
    pub target_angle: f32,
    pub show_target: bool,
    pub ai_turn_speed: f32,
    pub compare_raw: bool,
    pub show_scar: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Project {
    pub version: u32,
    pub mo2_dir: String,
    pub profile: String,
    pub source: Option<Source>,
    /// The annotation list as it stood (time, text, track).
    pub annos: Option<Vec<(f32, String, u16)>>,
    pub time: f32,
    pub speed: f32,
    pub looping: bool,
    pub selected: Option<usize>,
    /// NPC as `Plugin.esp|0x123456` (by the defining plugin).
    pub npc: Option<String>,
    pub weapon: Option<usize>,
    pub hidden_pieces: Vec<String>,
    pub scene: SceneState,
    pub camera: [f32; 6],
    pub px_per_sec: f32,
    pub offset: f32,
    pub oar_external: Vec<String>,
    pub oar_selected: Option<String>,
}

fn category_name(c: sds_anno::Category) -> String {
    format!("{c:?}")
}

/// The studio's state as a project.
pub fn capture(app: &StudioApp) -> Project {
    let s = &app.scene;
    Project {
        version: 1,
        mo2_dir: app.settings.mo2_dir.clone(),
        profile: app.settings.profile.clone(),
        source: app.doc.as_ref().map(|d| match &d.source {
            ClipSource::Vfs(p) => Source::Vfs(p.clone()),
            ClipSource::Disk(p) => Source::Disk(p.display().to_string()),
        }),
        annos: app.doc.as_ref().map(|d| d.annos.iter().map(|a| (a.time, a.text.clone(), a.track)).collect()),
        time: app.play.time,
        speed: app.play.speed,
        looping: app.play.looping,
        selected: app.selected,
        npc: match (&app.npc, &app.loaded) {
            (Some(n), Some(l)) => Some(format!("{}|0x{:06X}", l.db.plugin_name(n.look.key), n.look.key.id)),
            _ => None,
        },
        weapon: app.npc.as_ref().and_then(|n| n.weapon),
        hidden_pieces: app.character.as_ref().map(|c| c.pieces.iter().filter(|p| !p.visible).map(|p| p.label.clone()).collect()).unwrap_or_default(),
        scene: SceneState {
            root_motion: s.root_motion,
            show_path: s.show_path,
            show_grid: s.show_grid,
            sdscombat: s.sdscombat,
            hidden: s.hidden.iter().map(|c| category_name(*c)).collect(),
            view3d: s.view3d,
            show_body: s.show_body,
            show_skeleton: s.show_skeleton,
            follow: s.follow,
            show_precision: s.show_precision,
            show_trails: s.show_trails,
            show_fx: s.show_fx,
            precision_player: s.precision_player,
            target_dist: s.target_dist,
            target_angle: s.target_angle,
            show_target: s.show_target,
            ai_turn_speed: s.ai_turn_speed,
            compare_raw: s.compare_raw,
            show_scar: s.show_scar,
        },
        camera: [app.camera.yaw, app.camera.pitch, app.camera.dist, app.camera.target[0], app.camera.target[1], app.camera.target[2]],
        px_per_sec: app.timeline.px_per_sec,
        offset: app.timeline.offset,
        oar_external: app.oar_external.iter().map(|m| m.folder.clone()).collect(),
        oar_selected: app.oar_edit.as_ref().map(|e| e.folder.clone()),
    }
}

pub fn write(project: &Project, path: &Path) -> Result<(), String> {
    let s = serde_json::to_string_pretty(project).map_err(|e| e.to_string())?;
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(path, s).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn read(path: &Path) -> Result<Project, String> {
    let b = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_slice(&b).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn default_dir() -> PathBuf {
    crate::settings::dir().join("Projects")
}

pub fn autosave_path() -> PathBuf {
    crate::settings::dir().join("last_session.sdsproj")
}

/// Put a project into the studio. Needs the load order for vfs clips and
/// NPCs; callers hold the project back until it has loaded.
pub fn apply(app: &mut StudioApp, p: &Project) {
    if !p.mo2_dir.is_empty() && (p.mo2_dir != app.settings.mo2_dir || p.profile != app.settings.profile) {
        app.log(sds_i18n::tf!("Проект сделан на сборке «{}» / «{}», сейчас «{}» / «{}»", "The project was made on build «{}» / «{}», now «{}» / «{}»", p.mo2_dir, p.profile, app.settings.mo2_dir, app.settings.profile));
    }
    // External OAR folders first, so the clip's submod resolves.
    for f in &p.oar_external {
        if Path::new(f).is_dir() && !app.oar_external.iter().any(|m| &m.folder == f) {
            crate::oar_panel::add_external(app, Path::new(f));
        }
    }
    // NPC before the clip: opening a clip loads the folder body only without one.
    if let (Some(label), Some(l)) = (&p.npc, &app.loaded) {
        let db = l.db.clone();
        let key = label.split_once('|').and_then(|(plugin, id)| {
            let module = db.plugins.iter().position(|x| x.name.eq_ignore_ascii_case(plugin))? as u16;
            let id = u32::from_str_radix(id.trim_start_matches("0x"), 16).ok()?;
            Some(sds_records::FormKey { module, id })
        });
        match key {
            Some(k) if db.get(k).is_some() => {
                app.set_npc(k);
                let wanted = app.npc.as_ref().map(|n| p.weapon.filter(|w| *w < n.look.weapons.len()));
                if let (Some(w), Some(n)) = (wanted, &mut app.npc) {
                    if n.weapon != w {
                        n.weapon = w;
                        app.rebuild_npc();
                    }
                }
            }
            _ => app.log(sds_i18n::tf!("NPC {label} из проекта не найден в сборке", "NPC {label} from the project not found in the build")),
        }
    }
    match &p.source {
        Some(Source::Vfs(path)) => app.open_vfs_file(path),
        Some(Source::Disk(path)) => app.open_disk_file(PathBuf::from(path)),
        None => {}
    }
    if let (Some(doc), Some(list)) = (&mut app.doc, &p.annos) {
        let annos: Vec<Anno> = list.iter().map(|(t, s, tr)| Anno { time: *t, text: s.clone(), track: *tr }).collect();
        if annos != doc.annos {
            // One undo step back to the file's own annotations.
            doc.edit(None, |a| *a = annos);
        }
    }
    if let Some(c) = &mut app.character {
        for piece in &mut c.pieces {
            piece.visible = !p.hidden_pieces.contains(&piece.label);
        }
    }
    let s = &p.scene;
    let sc = &mut app.scene;
    sc.root_motion = s.root_motion;
    sc.show_path = s.show_path;
    sc.show_grid = s.show_grid;
    sc.sdscombat = s.sdscombat;
    sc.hidden = sds_anno::Category::ALL.iter().copied().filter(|c| s.hidden.contains(&category_name(*c))).collect();
    sc.view3d = s.view3d;
    sc.show_body = s.show_body;
    sc.show_skeleton = s.show_skeleton;
    sc.follow = s.follow;
    sc.show_precision = s.show_precision;
    sc.show_trails = s.show_trails;
    sc.show_fx = s.show_fx;
    sc.precision_player = s.precision_player;
    sc.target_dist = s.target_dist;
    sc.target_angle = s.target_angle;
    sc.show_target = s.show_target;
    sc.ai_turn_speed = s.ai_turn_speed;
    sc.compare_raw = s.compare_raw;
    sc.show_scar = s.show_scar;
    let c = p.camera;
    if c[2] > 0.0 {
        app.camera.yaw = c[0];
        app.camera.pitch = c[1];
        app.camera.dist = c[2];
        app.camera.target = [c[3], c[4], c[5]];
    }
    app.play.time = p.time;
    app.play.speed = if p.speed > 0.0 { p.speed } else { 1.0 };
    app.play.looping = p.looping;
    app.play.playing = false;
    app.selected = p.selected.filter(|i| app.doc.as_ref().is_some_and(|d| *i < d.annos.len()));
    if p.px_per_sec > 0.0 {
        app.timeline.px_per_sec = p.px_per_sec;
        app.timeline.offset = p.offset;
        app.timeline.fit_next = false;
    }
    if let Some(f) = &p.oar_selected {
        crate::oar_panel::select(app, f);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_file() {
        let p = Project {
            version: 1,
            source: Some(Source::Vfs("meshes/actors/character/animations/1hm_idle.hkx".into())),
            annos: Some(vec![(0.5, "HitFrame".into(), 0)]),
            npc: Some("Rebalance.esp|0x45243C".into()),
            time: 1.25,
            ..Default::default()
        };
        let path = std::env::temp_dir().join(format!("sds_proj_test_{}.sdsproj", std::process::id()));
        write(&p, &path).unwrap();
        let back = read(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(back.time, 1.25);
        assert_eq!(back.annos.unwrap()[0].1, "HitFrame");
        assert!(matches!(back.source, Some(Source::Vfs(_))));
        // Old/partial files load with defaults.
        let partial: Project = serde_json::from_str(r#"{"time": 2.0}"#).unwrap();
        assert_eq!(partial.time, 2.0);
    }
}
