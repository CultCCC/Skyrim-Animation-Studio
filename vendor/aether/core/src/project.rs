//! Project model: a named container of particle graphs plus project-wide
//! settings, saved as a directory with a stable-diff JSON manifest, autosave
//! backups, and named snapshots — the "project system" of modern DCC tools.
//!
//! On-disk layout:
//!
//! ```text
//! MyEffect.aether/
//!   project.json        <- manifest: settings + graph list
//!   graphs/<slug>.json   <- one file per graph document
//!   backups/            <- rotating autosaves (project.json.1 ...)
//!   snapshots/<name>/   <- user-named full copies ("before big rework")
//! ```

use crate::graph::Graph;
use crate::json::Json;
use std::path::{Path, PathBuf};

pub const PROJECT_DIR_EXT: &str = "aether";
pub const MANIFEST: &str = "project.json";
pub const FORMAT_VERSION: u32 = 1;

/// Export target game.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetGame {
    SkyrimSE,
    SkyrimLE,
}

impl TargetGame {
    pub fn name(self) -> &'static str {
        match self {
            TargetGame::SkyrimSE => "Skyrim SE",
            TargetGame::SkyrimLE => "Skyrim LE",
        }
    }

    fn stable_id(self) -> &'static str {
        match self {
            TargetGame::SkyrimSE => "skyrim_se",
            TargetGame::SkyrimLE => "skyrim_le",
        }
    }

    fn from_stable_id(s: &str) -> Option<Self> {
        match s {
            "skyrim_se" => Some(TargetGame::SkyrimSE),
            "skyrim_le" => Some(TargetGame::SkyrimLE),
            _ => None,
        }
    }
}

/// Project-wide settings, editable in the Project panel.
#[derive(Debug, Clone)]
pub struct ProjectSettings {
    pub target_game: TargetGame,
    /// Data folders found in a project written before they moved to
    /// [`crate::settings::AppSettings`]. Read on load, merged into the app
    /// settings by the caller, and never written back — asset folders
    /// describe the workstation, not the document, and a project file that
    /// carries someone else's drive letters is worse than useless when
    /// shared.
    pub legacy_data_roots: Vec<PathBuf>,
    pub autosave_minutes: u32,
    pub autosave_keep: u32,
}

impl Default for ProjectSettings {
    fn default() -> Self {
        ProjectSettings {
            target_game: TargetGame::SkyrimSE,
            legacy_data_roots: Vec::new(),
            autosave_minutes: 5,
            autosave_keep: 10,
        }
    }
}

/// One graph document plus its metadata.
#[derive(Debug, Clone)]
pub struct GraphEntry {
    /// File-name slug, unique in the project (`fire_burst`).
    pub slug: String,
    /// Display name ("Fire Burst").
    pub name: String,
    pub graph: Graph,
    /// The NIF this graph was decompiled from, ABSOLUTE.
    ///
    /// The reference model, the splice target, and the thing every
    /// `GraphNode::source_block` index addresses — so a graph without
    /// it describes only the remainder of a document nobody can name,
    /// and `export_bytes` refuses rather than writing the apparatus
    /// alone.
    ///
    /// Absolute in memory, portable on disk: [`crate::docpath`] encodes
    /// it at the serialization boundary and nowhere else. It lives on
    /// the ENTRY rather than in a side map for the same reason
    /// `GraphNode::source_block` lives on the node — a side map would
    /// be lost on reload.
    ///
    /// It is in the MANIFEST, not in `graphs/<slug>.json`, deliberately:
    /// `Graph::from_json` returns an `Option` and the loader turns
    /// `None` into an empty graph with no warning channel, so a new
    /// field that could fail to parse there would silently delete the
    /// user's work. The manifest reader is all `unwrap_or`, and the
    /// manifest is the only file `save_to` backs up.
    pub source_nif: Option<PathBuf>,
    /// What the viewport had selected in this graph's tab at save time.
    pub selection: Option<StoredSelection>,
    /// How many blocks that file had when this project was saved.
    ///
    /// The dangerous case is not a MISSING source — that is loud and
    /// harmless — but a CHANGED one: `source_block` is an index, so a
    /// file re-exported from NifSkope between save and reopen makes the
    /// splice write the apparatus into the wrong slots, silently, over
    /// a file the user believes is being edited in place. A count is a
    /// signal rather than a proof, so its consumer warns.
    pub source_blocks: Option<u32>,
}

/// The viewport camera pose, as saved with the project.
///
/// Only the pose: `scene_radius`, FOV and the like are derived from
/// the scene on load, and a stale copy of a derived value is a bug
/// waiting for a divergence.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewState {
    pub target: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub orthographic: bool,
}

/// What the viewport had selected, as saved with the project.
///
/// Mirrors the app's selection: a drawn shape addressed by mesh index
/// plus the parent-walk cursor, or a bare block. Mesh indices are
/// stable for an unchanged source NIF (extraction is deterministic in
/// file order, pinned by test); a CHANGED source already warns through
/// `source_blocks`, and the app clamps what no longer resolves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoredSelection {
    Drawn { mesh: u32, up: u8 },
    Block(u32),
}

/// An open project.
#[derive(Debug, Clone)]
pub struct Project {
    pub name: String,
    pub description: String,
    pub settings: ProjectSettings,
    pub graphs: Vec<GraphEntry>,
    /// Directory this project was loaded from / saved to.
    pub path: Option<PathBuf>,
    /// Which graph tab was active when the project was saved.
    pub active_graph: usize,
    /// The viewport camera pose at save time.
    pub view: Option<ViewState>,
}

impl Project {
    pub fn new(name: impl Into<String>) -> Self {
        Project {
            name: name.into(),
            description: String::new(),
            settings: ProjectSettings::default(),
            graphs: Vec::new(),
            path: None,
            active_graph: 0,
            view: None,
        }
    }

    pub fn slug_for(&self, display_name: &str) -> String {
        let base: String = display_name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '_'
                }
            })
            .collect();
        let base = base.trim_matches('_').to_string();
        let base = if base.is_empty() {
            "graph".to_string()
        } else {
            base
        };
        let mut slug = base.clone();
        let mut i = 2;
        while self.graphs.iter().any(|g| g.slug == slug) {
            slug = format!("{base}_{i}");
            i += 1;
        }
        slug
    }

    pub fn add_graph(&mut self, name: impl Into<String>, graph: Graph) -> usize {
        let name = name.into();
        let slug = self.slug_for(&name);
        self.graphs.push(GraphEntry {
            slug,
            name,
            graph,
            selection: None,
            source_nif: None,
            source_blocks: None,
        });
        self.graphs.len() - 1
    }

    /// Which graphs need their source NIF reopened, paired with the
    /// index of the graph — and therefore of the slot.
    ///
    /// Enumerates BEFORE filtering. `filter_map(..).enumerate()` shifts
    /// every index after the first sourceless graph, which attaches one
    /// file's meshes, sequences and `NifFile` to a different tab —
    /// silent, and reads as a rendering fault rather than as an
    /// indexing one.
    pub fn sources_to_reattach(&self) -> Vec<(usize, PathBuf)> {
        self.graphs
            .iter()
            .enumerate()
            .filter_map(|(i, g)| g.source_nif.clone().map(|p| (i, p)))
            .collect()
    }

    /// The graphs whose source cannot be stored portably, and why.
    ///
    /// Pure — no filesystem — so a panel may call it every frame.
    pub fn unportable_sources(
        &self,
        paths: &crate::docpath::DocPaths,
    ) -> Vec<(usize, PathBuf, crate::docpath::Unportable)> {
        self.graphs
            .iter()
            .enumerate()
            .filter_map(|(i, g)| {
                let p = g.source_nif.as_ref()?;
                paths.encode(p).err().map(|why| (i, p.clone(), why))
            })
            .collect()
    }

    /// Copy a graph's source NIF into `<project>/nifs/`, so the project
    /// carries the file it edits and the stored form becomes the
    /// `//nifs/...` spelling that survives any machine.
    ///
    /// Idempotent on the file that is already there: identical bytes
    /// reuse the copy. DIFFERENT bytes refuse — two sources sharing a
    /// basename must not silently clobber each other, and the caller
    /// can rename one and try again.
    pub fn vendor_source(project_dir: &Path, source: &Path) -> std::io::Result<PathBuf> {
        let name = source.file_name().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "source has no file name")
        })?;
        let dir = project_dir.join("nifs");
        std::fs::create_dir_all(&dir)?;
        let dst = dir.join(name);
        if dst.exists() {
            let same = std::fs::read(&dst)? == std::fs::read(source)?;
            if !same {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    format!(
                        "{} already holds a DIFFERENT file; rename one of them",
                        dst.display()
                    ),
                ));
            }
            return Ok(dst);
        }
        std::fs::copy(source, &dst)?;
        Ok(dst)
    }

    // ---- JSON ----

    fn manifest_json(&self, paths: &crate::docpath::DocPaths) -> Json {
        let graphs = self
            .graphs
            .iter()
            .map(|g| {
                let mut o = Json::obj()
                    .with("slug", Json::Str(g.slug.clone()))
                    .with("name", Json::Str(g.name.clone()));
                // Written only when there IS one, so a project with no
                // imported file stays diff-identical to what it was.
                if let Some(stored) = g
                    .source_nif
                    .as_deref()
                    .and_then(|p| paths.encode_or_absolute(p))
                {
                    o = o.with("source_nif", Json::Str(stored));
                    if let Some(n) = g.source_blocks {
                        o = o.with("source_blocks", Json::Num(n as f64));
                    }
                }
                match g.selection {
                    Some(StoredSelection::Drawn { mesh, up }) => {
                        o = o
                            .with("sel_mesh", Json::Num(mesh as f64))
                            .with("sel_up", Json::Num(up as f64));
                    }
                    Some(StoredSelection::Block(b)) => {
                        o = o.with("sel_block", Json::Num(b as f64));
                    }
                    None => {}
                }
                o
            })
            .collect();
        let settings = Json::obj()
            .with(
                "target_game",
                Json::Str(self.settings.target_game.stable_id().to_string()),
            )
            .with(
                "autosave_minutes",
                Json::Num(self.settings.autosave_minutes as f64),
            )
            .with(
                "autosave_keep",
                Json::Num(self.settings.autosave_keep as f64),
            );
        Json::obj()
            .with("format", Json::Str("aether-project".into()))
            .with("format_version", Json::Num(FORMAT_VERSION as f64))
            .with("name", Json::Str(self.name.clone()))
            .with("description", Json::Str(self.description.clone()))
            .with("settings", settings)
            .with("active_graph", Json::Num(self.active_graph as f64))
            .with(
                "view",
                match &self.view {
                    Some(v) => Json::obj()
                        .with("target_x", Json::Num(v.target[0] as f64))
                        .with("target_y", Json::Num(v.target[1] as f64))
                        .with("target_z", Json::Num(v.target[2] as f64))
                        .with("yaw", Json::Num(v.yaw as f64))
                        .with("pitch", Json::Num(v.pitch as f64))
                        .with("distance", Json::Num(v.distance as f64))
                        .with("orthographic", Json::Bool(v.orthographic)),
                    None => Json::Null,
                },
            )
            .with("graphs", Json::Arr(graphs))
    }

    /// Save the whole project into `dir` (created if missing).
    ///
    /// `data_roots` is what makes a stored source path portable: a NIF
    /// under a configured install is written as `data:meshes/...` and
    /// resolves against whatever install the next machine has. See
    /// [`crate::docpath`]. Passing an empty slice is legal and simply
    /// means every path falls back to the project-relative or absolute
    /// form.
    pub fn save_to(&mut self, dir: &Path, data_roots: &[PathBuf]) -> std::io::Result<()> {
        // The destination, not `self.path`: Save-As must re-encode
        // against where the project is GOING, or every relative path
        // would still point out of the old directory. Free here only
        // because the in-memory form is absolute.
        let paths =
            crate::docpath::DocPaths::new(Some(dir.to_path_buf()), data_roots.to_vec());
        std::fs::create_dir_all(dir.join("graphs"))?;

        // Rotate a backup of the previous manifest before overwriting.
        let manifest_path = dir.join(MANIFEST);
        if manifest_path.exists() {
            let backups = dir.join("backups");
            std::fs::create_dir_all(&backups)?;
            rotate_backups(
                &backups,
                "project.json",
                self.settings.autosave_keep as usize,
            )?;
            std::fs::copy(&manifest_path, backups.join("project.json.1"))?;
        }

        std::fs::write(&manifest_path, self.manifest_json(&paths).pretty())?;
        for g in &self.graphs {
            let path = dir.join("graphs").join(format!("{}.json", g.slug));
            std::fs::write(path, g.graph.to_json().pretty())?;
        }
        self.path = Some(dir.to_path_buf());
        Ok(())
    }

    /// Load a project directory.
    ///
    /// `data_roots` resolves the `data:` form; see [`Self::save_to`].
    pub fn load_from(dir: &Path, data_roots: &[PathBuf]) -> std::io::Result<Project> {
        let paths =
            crate::docpath::DocPaths::new(Some(dir.to_path_buf()), data_roots.to_vec());
        let manifest_text = std::fs::read_to_string(dir.join(MANIFEST))?;
        let manifest = Json::parse(&manifest_text).map_err(io_invalid)?;

        let name = manifest
            .get("name")
            .and_then(Json::as_str)
            .unwrap_or("Untitled")
            .to_string();
        let description = manifest
            .get("description")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_string();

        let mut settings = ProjectSettings::default();
        if let Some(s) = manifest.get("settings") {
            if let Some(tg) = s
                .get("target_game")
                .and_then(Json::as_str)
                .and_then(TargetGame::from_stable_id)
            {
                settings.target_game = tg;
            }
            if let Some(n) = s.get("autosave_minutes").and_then(Json::as_u64) {
                settings.autosave_minutes = n as u32;
            }
            if let Some(n) = s.get("autosave_keep").and_then(Json::as_u64) {
                settings.autosave_keep = n as u32;
            }
            // Every spelling asset folders have had in a project file, newest
            // first. All of them now feed the app settings instead.
            for key in ["data_roots", "asset_roots"] {
                if let Some(roots) = s.get(key).and_then(Json::as_arr) {
                    for r in roots.iter().filter_map(Json::as_str).map(PathBuf::from) {
                        if !settings.legacy_data_roots.contains(&r) {
                            settings.legacy_data_roots.push(r);
                        }
                    }
                }
            }
            if let Some(root) = s.get("data_root").and_then(Json::as_str) {
                let root = PathBuf::from(root);
                if !settings.legacy_data_roots.contains(&root) {
                    settings.legacy_data_roots.push(root);
                }
            }
        }

        let mut graphs = Vec::new();
        if let Some(list) = manifest.get("graphs").and_then(Json::as_arr) {
            for gj in list {
                let slug = gj
                    .get("slug")
                    .and_then(Json::as_str)
                    .unwrap_or_default()
                    .to_string();
                let gname = gj
                    .get("name")
                    .and_then(Json::as_str)
                    .unwrap_or(&slug)
                    .to_string();
                let gpath = dir.join("graphs").join(format!("{slug}.json"));
                let graph = match std::fs::read_to_string(&gpath) {
                    Ok(text) => Json::parse(&text)
                        .ok()
                        .and_then(|j| Graph::from_json(&j))
                        .unwrap_or_default(),
                    Err(_) => Graph::default(),
                };
                // `if let`, like every other manifest field: a project
                // saved before this key existed must load with the
                // field absent, not fail. `?` or `expect` here would
                // make every pre-existing project unopenable.
                let source_nif = gj
                    .get("source_nif")
                    .and_then(Json::as_str)
                    .map(|s| paths.decode(s));
                let source_blocks = gj
                    .get("source_blocks")
                    .and_then(Json::as_u64)
                    .map(|n| n as u32);
                let selection = match (
                    gj.get("sel_mesh").and_then(Json::as_u64),
                    gj.get("sel_up").and_then(Json::as_u64),
                    gj.get("sel_block").and_then(Json::as_u64),
                ) {
                    (Some(mesh), up, _) => Some(StoredSelection::Drawn {
                        mesh: mesh as u32,
                        up: up.unwrap_or(0) as u8,
                    }),
                    (None, _, Some(b)) => Some(StoredSelection::Block(b as u32)),
                    _ => None,
                };
                graphs.push(GraphEntry {
                    slug,
                    name: gname,
                    graph,
                    selection,
                    source_nif,
                    source_blocks,
                });
            }
        }

        // Clamped on READ, not trusted: a manifest edited by hand (or a
        // graph deleted since the save) must not select a tab that does
        // not exist.
        let active_graph = manifest
            .get("active_graph")
            .and_then(Json::as_u64)
            .map(|n| n as usize)
            .unwrap_or(0)
            .min(graphs.len().saturating_sub(1));
        let view = manifest.get("view").and_then(|v| {
            Some(ViewState {
                target: [
                    v.get("target_x")?.as_f64()? as f32,
                    v.get("target_y")?.as_f64()? as f32,
                    v.get("target_z")?.as_f64()? as f32,
                ],
                yaw: v.get("yaw")?.as_f64()? as f32,
                pitch: v.get("pitch")?.as_f64()? as f32,
                distance: v.get("distance")?.as_f64()? as f32,
                orthographic: v.get("orthographic").and_then(Json::as_bool).unwrap_or(false),
            })
        });
        // A pose with a non-finite number in it poisons every matrix it
        // touches; better no restore than a black viewport.
        let view = view.filter(|v| {
            v.target.iter().all(|c| c.is_finite())
                && v.yaw.is_finite()
                && v.pitch.is_finite()
                && v.distance.is_finite()
                && v.distance > 0.0
        });

        Ok(Project {
            name,
            description,
            settings,
            graphs,
            path: Some(dir.to_path_buf()),
            active_graph,
            view,
        })
    }

    /// Copy the current saved state into `snapshots/<name>/`.
    pub fn snapshot(&mut self, name: &str) -> std::io::Result<PathBuf> {
        let dir = self.path.clone().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "project has not been saved yet",
            )
        })?;
        let slug: String = name
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        let snap = dir.join("snapshots").join(slug);
        std::fs::create_dir_all(snap.join("graphs"))?;
        std::fs::copy(dir.join(MANIFEST), snap.join(MANIFEST))?;
        for g in &self.graphs {
            let file = format!("{}.json", g.slug);
            let src = dir.join("graphs").join(&file);
            if src.exists() {
                std::fs::copy(src, snap.join("graphs").join(&file))?;
            }
        }
        Ok(snap)
    }
}

fn io_invalid(e: impl std::error::Error + Send + Sync + 'static) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, e)
}

/// Shift `name.1` -> `name.2` -> ... keeping at most `keep` entries.
fn rotate_backups(dir: &Path, name: &str, keep: usize) -> std::io::Result<()> {
    if keep == 0 {
        return Ok(());
    }
    let nth = |i: usize| dir.join(format!("{name}.{i}"));
    let last = nth(keep);
    if last.exists() {
        std::fs::remove_file(&last)?;
    }
    for i in (1..keep).rev() {
        let from = nth(i);
        if from.exists() {
            std::fs::rename(from, nth(i + 1))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{GraphNode, NodeKind};

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("aether-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn project_roundtrip() {
        let dir = temp_dir("proj");
        let mut p = Project::new("Frost Trap");
        p.description = "Test effect".into();
        p.settings.target_game = TargetGame::SkyrimLE;

        let mut g = Graph::new();
        g.insert(GraphNode::new(NodeKind::ParticleSystem, [1.0, 2.0]));
        p.add_graph("Main Burst", g);

        p.save_to(&dir, &[]).unwrap();
        let loaded = Project::load_from(&dir, &[]).unwrap();
        assert_eq!(loaded.name, "Frost Trap");
        assert_eq!(loaded.settings.target_game, TargetGame::SkyrimLE);
        assert_eq!(loaded.graphs.len(), 1);
        assert_eq!(loaded.graphs[0].name, "Main Burst");
        assert_eq!(loaded.graphs[0].graph.nodes.len(), 1);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A tab's source NIF survives the round trip, and it survives the
    /// project being MOVED — which is the whole point of storing it
    /// portably rather than as a drive letter.
    ///
    /// Planted defect this catches: writing the key and never reading
    /// it (or the reverse). That is the standard failure of a two-sided
    /// format change, and it looks exactly like the feature working
    /// until the next session. Copying the directory is what proves the
    /// path is RESOLVED rather than memorised.
    #[test]
    fn a_source_nif_round_trips_and_survives_a_move() {
        let dir = temp_dir("src_nif");
        let root = dir.join("install/Data");
        let mut p = Project::new("Forge");
        p.add_graph("Bellows", Graph::new());
        p.add_graph("No File", Graph::new());
        let asset = root.join("meshes/fx/forge.nif");
        p.graphs[0].source_nif = Some(asset.clone());
        p.graphs[0].source_blocks = Some(137);

        let a = dir.join("a.aether");
        let roots = [root.clone()];
        p.save_to(&a, &roots).unwrap();

        // Stored portably: the manifest must not name the drive.
        let text = std::fs::read_to_string(a.join(MANIFEST)).unwrap();
        assert!(
            text.contains("data:meshes/fx/forge.nif"),
            "manifest stored an unportable path:\n{text}"
        );

        // ...and a graph with no file writes no key at all, so an
        // ordinary project's manifest is unchanged by this feature.
        let entries = text.matches("source_nif").count();
        assert_eq!(entries, 1, "only the graph that HAS a source names one");

        // Move the whole project and load it from there.
        let b = dir.join("moved.aether");
        copy_tree(&a, &b);
        let loaded = Project::load_from(&b, &roots).unwrap();
        assert_eq!(loaded.graphs[0].source_nif.as_deref(), Some(asset.as_path()));
        assert_eq!(loaded.graphs[0].source_blocks, Some(137));
        assert_eq!(loaded.graphs[1].source_nif, None);
        assert_eq!(loaded.graphs[1].source_blocks, None);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A project saved before this key existed still opens.
    ///
    /// Planted defect: reading the new key with `?` or `expect`, which
    /// makes every project saved before today unopenable — a
    /// total-loss regression a happy-path round trip cannot see, since
    /// it only ever reads manifests it just wrote.
    #[test]
    fn an_older_manifest_loads_with_no_source() {
        let dir = temp_dir("old_manifest");
        std::fs::create_dir_all(dir.join("graphs")).unwrap();
        std::fs::write(
            dir.join(MANIFEST),
            r#"{"format":"aether-project","format_version":1,"name":"Old",
               "description":"","settings":{"target_game":"skyrim_se"},
               "graphs":[{"slug":"main","name":"Main"}]}"#,
        )
        .unwrap();
        let mut g = Graph::new();
        g.insert(GraphNode::new(NodeKind::ParticleSystem, [0.0, 0.0]));
        std::fs::write(dir.join("graphs/main.json"), g.to_json().pretty()).unwrap();

        let loaded = Project::load_from(&dir, &[]).expect("an old project still opens");
        assert_eq!(loaded.graphs.len(), 1);
        assert_eq!(loaded.graphs[0].source_nif, None);
        assert_eq!(
            loaded.graphs[0].graph.nodes.len(),
            1,
            "and its graph came through, not an empty default"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The reattach list is indexed by GRAPH, not by position among the
    /// graphs that happen to have a source.
    ///
    /// Planted defect: `filter_map(..).enumerate()`, which returns
    /// `[(0,p0),(1,p2)]` and hands graph #2's file, meshes and
    /// sequences to tab #1 — silent cross-wiring that reads as a
    /// rendering bug rather than an indexing one.
    #[test]
    fn reattach_indices_are_graph_indices() {
        let mut p = Project::new("Three");
        for n in ["A", "B", "C"] {
            p.add_graph(n, Graph::new());
        }
        let p0 = PathBuf::from("/x/a.nif");
        let p2 = PathBuf::from("/x/c.nif");
        p.graphs[0].source_nif = Some(p0.clone());
        p.graphs[2].source_nif = Some(p2.clone());
        assert_eq!(p.sources_to_reattach(), vec![(0, p0), (2, p2)]);
    }

    /// An unportable source is NAMED rather than dropped.
    ///
    /// Planted defect: blanking a path that cannot be made relative.
    /// The reference would then be gone from the next save — the
    /// opposite of what Blender does, which writes a missing library
    /// straight back out so that saving never destroys the link.
    #[test]
    fn an_unportable_source_is_kept_and_reported() {
        let dir = temp_dir("unportable");
        let mut p = Project::new("Off Drive");
        p.add_graph("Main", Graph::new());
        // A sibling of the project with no data root in play is still
        // portable; make it genuinely unportable instead.
        let far = PathBuf::from(r"\\server\share\mods\a.nif");
        p.graphs[0].source_nif = Some(far.clone());
        let a = dir.join("p.aether");
        p.save_to(&a, &[]).unwrap();

        let paths = crate::docpath::DocPaths::new(Some(a.clone()), Vec::new());
        let bad = p.unportable_sources(&paths);
        assert_eq!(bad.len(), 1, "the UNC path must be reported");
        assert_eq!(bad[0].0, 0);

        let loaded = Project::load_from(&a, &[]).unwrap();
        assert_eq!(
            loaded.graphs[0].source_nif.as_deref(),
            Some(far.as_path()),
            "and kept verbatim, not thrown away"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn copy_tree(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap() {
            let e = e.unwrap();
            let dst = to.join(e.file_name());
            if e.file_type().unwrap().is_dir() {
                copy_tree(&e.path(), &dst);
            } else {
                std::fs::copy(e.path(), dst).unwrap();
            }
        }
    }

    #[test]
    fn backups_rotate() {
        let dir = temp_dir("backup");
        let mut p = Project::new("B");
        p.save_to(&dir, &[]).unwrap();
        p.save_to(&dir, &[]).unwrap();
        p.save_to(&dir, &[]).unwrap();
        assert!(dir.join("backups/project.json.1").exists());
        assert!(dir.join("backups/project.json.2").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn snapshots_copy_graphs() {
        let dir = temp_dir("snap");
        let mut p = Project::new("S");
        p.add_graph("Fire", Graph::new());
        p.save_to(&dir, &[]).unwrap();
        let snap = p.snapshot("before rework").unwrap();
        assert!(snap.join(MANIFEST).exists());
        assert!(snap.join("graphs/fire.json").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[cfg(test)]
mod vendor_tests {
    use super::Project;

    /// The three outcomes, in one temp tree: a fresh copy, an identical
        /// re-copy reused, and a basename collision with different bytes
    /// REFUSED rather than overwritten.
    #[test]
    fn vendoring_copies_reuses_and_refuses() {
        let root = std::env::temp_dir().join("aether_vendor_test");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("proj")).unwrap();
        std::fs::create_dir_all(root.join("elsewhere")).unwrap();
        let src = root.join("elsewhere/model.nif");
        std::fs::write(&src, b"AAAA").unwrap();

        let dst = Project::vendor_source(&root.join("proj"), &src).unwrap();
        assert_eq!(dst, root.join("proj/nifs/model.nif"));
        assert_eq!(std::fs::read(&dst).unwrap(), b"AAAA");

        // Identical bytes: reused, not an error.
        let again = Project::vendor_source(&root.join("proj"), &src).unwrap();
        assert_eq!(again, dst);

        // Same basename, different bytes: refused, and the vendored
        // copy keeps its original content.
        let other = root.join("elsewhere/model2.nif");
        std::fs::write(&other, b"BBBB").unwrap();
        let renamed = root.join("elsewhere/model.nif");
        std::fs::write(&renamed, b"BBBB").unwrap();
        let err = Project::vendor_source(&root.join("proj"), &renamed).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&dst).unwrap(), b"AAAA", "the copy was clobbered");

        // ...and the vendored copy STORES portably, which is the point:
        // the project-relative form survives any machine.
        let dp = crate::docpath::DocPaths::new(Some(root.join("proj")), Vec::new());
        assert_eq!(dp.encode(&dst).unwrap(), "//nifs/model.nif");
    }
}
