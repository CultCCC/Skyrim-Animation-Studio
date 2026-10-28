//! Application settings — the things that describe this machine, not this
//! document.
//!
//! Data folders point at a game install and a mod staging area. Those are
//! properties of the workstation: they are the same for every project you
//! open, and they are wrong to carry inside a project file that might be
//! shared. NifSkope makes the same call (Settings → Resources is global), and
//! keeping them per-project meant every new project silently lost its
//! textures until you re-picked the folder.
//!
//! Stored as JSON next to the user's other config so it survives restarts,
//! using the same hand-rolled `Json` as the project format — `crates/core` is
//! std-only by design.

use crate::json::Json;
use std::path::{Path, PathBuf};

const FORMAT_VERSION: u32 = 1;

/// Settings that belong to the installation rather than to a project.
///
/// `Default` is written out by hand rather than derived: `node_lod`
/// defaults to TRUE, and a derived `bool::default()` would silently
/// turn the canvas's level of detail off for every fresh install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppSettings {
    /// Ordered asset folders, searched top to bottom, first hit wins — so a
    /// mod folder listed above the game's Data overrides it, mirroring the
    /// game's own loose-file layering.
    pub data_roots: Vec<PathBuf>,
    /// Plugins the EDITOR loads without — the session selection for
    /// the Records/World workspaces. plugins.txt is never touched.
    pub disabled_plugins: Vec<String>,
    /// Draw the whole worldspace behind a loaded block, from the game's
    /// baked LOD. A workstation preference like the data roots: it depends
    /// on the machine's patience, not on the document.
    pub horizon: bool,
    /// Tooltip language. KEYWORDS never translate — node titles,
    /// parameter names, menu items and every other UI label stay
    /// English (the locked decision), because they are the vocabulary
    /// the file format, the manual and the community share. What
    /// switches is the explanatory PROSE: node help and parameter
    /// tooltips, from the corpus in `crate::graph_ru`.
    pub language: Language,
    /// Project directories, most recently used first. Written on every
    /// open and save; read by File > Open Recent and the startup card.
    /// A path that no longer exists is KEPT — a network drive that is
    /// down tonight is not a project the user deleted — and merely
    /// shown as unreachable.
    pub recent_projects: Vec<PathBuf>,
    /// Collapse node bodies on the canvas when zoomed out.
    ///
    /// On by default, which is what every node editor of this class
    /// does: snarl rasterises text once and scales the layer mesh, so
    /// small text turns to mush and an overview of ten systems reads
    /// better as headers. Off is for reading a whole graph's
    /// parameters at a glance without zooming in node by node — a
    /// workstation preference like the horizon, not a document
    /// property.
    pub node_lod: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        AppSettings {
            data_roots: Vec::new(),
            disabled_plugins: Vec::new(),
            horizon: false,
            language: Language::default(),
            recent_projects: Vec::new(),
            node_lod: true,
        }
    }
}

/// The two tooltip languages. A workstation preference, not a
/// document property — it lives beside the data roots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Language {
    #[default]
    English,
    Russian,
}

impl Language {
    pub fn tag(self) -> &'static str {
        match self {
            Language::English => "en",
            Language::Russian => "ru",
        }
    }

    pub fn from_tag(tag: &str) -> Language {
        // Anything unrecognized reads as English rather than as an
        // error: a settings file from a future build must not break
        // this one.
        if tag == "ru" {
            Language::Russian
        } else {
            Language::English
        }
    }
}

/// The file that flips Aether PORTABLE: its presence beside the
/// executable moves EVERYTHING the app persists — `settings.json`,
/// `layout.json`, the `library/` — into the exe's own folder instead
/// of the per-user config directory.
///
/// Exists because antivirus heuristics dislike a fresh unsigned exe
/// writing into `%APPDATA%` on first launch (reported against the
/// packaged build, 2026-09-02). The release package ships the marker,
/// so a distributed Aether is self-contained by default; a dev build
/// has none and keeps the per-user location, so `target/debug` does
/// not silently fork away from the developer's own settings. Content
/// is ignored — the file's existence is the switch, and deleting it
/// switches back.
pub const PORTABLE_MARKER: &str = "portable.txt";

/// `Some(dir)` when the portable marker sits in `exe_dir` — split out
/// pure so the rule is testable without an exe.
pub fn portable_root(exe_dir: &Path) -> Option<PathBuf> {
    exe_dir
        .join(PORTABLE_MARKER)
        .is_file()
        .then(|| exe_dir.to_path_buf())
}

impl AppSettings {
    /// The ONE directory everything persistent lives in. Portable
    /// (marker beside the exe) wins; otherwise `APPDATA` on Windows,
    /// `XDG_CONFIG_HOME` or `~/.config` elsewhere. No `dirs` crate —
    /// this is two environment variables and the no-deps rule applies
    /// here too. `layout.json` and the library derive from
    /// [`AppSettings::config_path`], so they follow automatically.
    pub fn config_dir() -> Option<PathBuf> {
        if let Some(dir) = std::env::current_exe()
            .ok()
            .as_deref()
            .and_then(Path::parent)
            .and_then(portable_root)
        {
            return Some(dir);
        }
        let base = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from))
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config"))
            })?;
        Some(base.join("Aether"))
    }

    /// Where the settings live for the current user (or, portable,
    /// for this install).
    pub fn config_path() -> Option<PathBuf> {
        Some(Self::config_dir()?.join("settings.json"))
    }

    /// Load from the given file. A missing file is not an error — it just
    /// means "no settings yet", which is the normal first run.
    pub fn load_from(path: &Path) -> Result<AppSettings, String> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(AppSettings::default())
            }
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        AppSettings::from_json(&text)
    }

    pub fn from_json(text: &str) -> Result<AppSettings, String> {
        let json = Json::parse(text).map_err(|e| e.to_string())?;
        let mut settings = AppSettings::default();
        if let Some(list) = json.get("disabled_plugins").and_then(Json::as_arr) {
            settings.disabled_plugins = list
                .iter()
                .filter_map(|v| v.as_str())
                .map(str::to_string)
                .collect();
        }
        if let Some(roots) = json.get("data_roots").and_then(Json::as_arr) {
            settings.data_roots = roots
                .iter()
                .filter_map(Json::as_str)
                .map(PathBuf::from)
                .collect();
        }
        // Absent in every settings file written before the horizon
        // existed, which must read as "off" rather than as a parse error.
        settings.horizon = json
            .get("horizon")
            .and_then(Json::as_bool)
            .unwrap_or(false);
        settings.language = json
            .get("language")
            .and_then(Json::as_str)
            .map(Language::from_tag)
            .unwrap_or_default();
        if let Some(list) = json.get("recent_projects").and_then(Json::as_arr) {
            settings.recent_projects = list
                .iter()
                .filter_map(Json::as_str)
                .map(PathBuf::from)
                .collect();
        }
        // Absent in every settings file written before the toggle
        // existed, and those must keep the level of detail they had:
        // `unwrap_or(true)`, not `unwrap_or_default()`.
        settings.node_lod = json.get("node_lod").and_then(Json::as_bool).unwrap_or(true);
        Ok(settings)
    }

    pub fn to_json(&self) -> String {
        Json::obj()
            .with("format", Json::Str("aether-settings".into()))
            .with("format_version", Json::Num(FORMAT_VERSION as f64))
            .with(
                "disabled_plugins",
                Json::Arr(
                    self.disabled_plugins
                        .iter()
                        .map(|s| Json::Str(s.clone()))
                        .collect(),
                ),
            )
            .with(
                "data_roots",
                Json::Arr(
                    self.data_roots
                        .iter()
                        .map(|p| Json::Str(p.display().to_string()))
                        .collect(),
                ),
            )
            .with("horizon", Json::Bool(self.horizon))
            .with("node_lod", Json::Bool(self.node_lod))
            .with("language", Json::Str(self.language.tag().into()))
            .with(
                "recent_projects",
                Json::Arr(
                    self.recent_projects
                        .iter()
                        .map(|p| Json::Str(p.display().to_string()))
                        .collect(),
                ),
            )
            .pretty()
    }

    pub fn save_to(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::write(path, self.to_json()).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Record a project as most recently used. Returns whether the list
    /// changed, so the caller knows whether to persist.
    pub fn remember_project(&mut self, dir: PathBuf) -> bool {
        if self.recent_projects.first() == Some(&dir) {
            return false;
        }
        self.recent_projects.retain(|p| p != &dir);
        self.recent_projects.insert(0, dir);
        self.recent_projects.truncate(10);
        true
    }

    /// Add a folder, keeping the list free of duplicates. Returns whether it
    /// was new, so the caller knows whether to persist.
    pub fn add_data_root(&mut self, root: PathBuf) -> bool {
        if self.data_roots.contains(&root) {
            return false;
        }
        self.data_roots.push(root);
        true
    }

    /// Merge folders discovered elsewhere — projects written before data
    /// folders moved out of the project file still carry them, and dropping
    /// them silently would break those projects' previews.
    pub fn merge_data_roots(&mut self, roots: &[PathBuf]) -> bool {
        let mut changed = false;
        for root in roots {
            changed |= self.add_data_root(root.clone());
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_through_json() {
        let settings = AppSettings {
            // Non-empty on purpose: an empty list round-trips through
            // a serializer that never writes the field at all.
            disabled_plugins: vec!["Dawnguard.esm".to_string(), "cc-thing.esl".to_string()],
            data_roots: vec![
                PathBuf::from(r"C:\Games\Skyrim\Data"),
                PathBuf::from(r"D:\Mods\MyEffect"),
            ],
            horizon: false,
            // NON-default on purpose: the default value round-trips
            // through a serializer that never writes the field at all.
            language: Language::Russian,
            recent_projects: vec![
                PathBuf::from(r"C:\Work\Fire.aether"),
                PathBuf::from(r"C:\Work\Frost.aether"),
            ],
            // NON-default too: this one defaults to TRUE, so a
            // serializer that never wrote it would still round-trip a
            // `true` and hide the omission.
            node_lod: false,
        };
        let parsed = AppSettings::from_json(&settings.to_json()).expect("parses");
        // Order is priority, so a round trip that reorders would silently
        // change which folder wins.
        assert_eq!(parsed, settings);
    }

    /// The portable rule: the marker file IS the switch, both ways.
    /// Everything else (layout.json, the library) derives from
    /// `config_path`, so this one rule is the whole behavior.
    #[test]
    fn the_portable_marker_moves_the_config_beside_the_exe() {
        let dir = std::env::temp_dir().join("aether-portable-rule-7c2e");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");

        assert_eq!(portable_root(&dir), None, "no marker, no portable");

        std::fs::write(dir.join(PORTABLE_MARKER), "any content").expect("marker");
        assert_eq!(portable_root(&dir), Some(dir.clone()));

        // A DIRECTORY named like the marker is not the switch — only a
        // file is, so a stray folder cannot flip storage modes.
        let dir2 = std::env::temp_dir().join("aether-portable-rule-7c2e-b");
        let _ = std::fs::remove_dir_all(&dir2);
        std::fs::create_dir_all(dir2.join(PORTABLE_MARKER)).expect("decoy dir");
        assert_eq!(portable_root(&dir2), None);

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dir2);
    }

    /// The recents list is MRU: re-opening a known project moves it to
    /// the front instead of duplicating it, and the list stays bounded.
    #[test]
    fn remember_project_is_a_bounded_mru() {
        let mut s = AppSettings::default();
        let a = PathBuf::from(r"C:\p\A.aether");
        let b = PathBuf::from(r"C:\p\B.aether");
        assert!(s.remember_project(a.clone()));
        assert!(s.remember_project(b.clone()));
        assert_eq!(s.recent_projects, vec![b.clone(), a.clone()]);
        // Re-opening A promotes it, no duplicate.
        assert!(s.remember_project(a.clone()));
        assert_eq!(s.recent_projects, vec![a.clone(), b.clone()]);
        // Re-opening the CURRENT front is a no-op, so autosave-adjacent
        // callers do not rewrite settings.json every time.
        assert!(!s.remember_project(a.clone()));
        // Bounded at 10.
        for i in 0..12 {
            s.remember_project(PathBuf::from(format!(r"C:\p\{i}.aether")));
        }
        assert_eq!(s.recent_projects.len(), 10);
    }

    #[test]
    fn a_missing_file_reads_as_empty_settings() {
        // First run must not look like a failure.
        let path = std::env::temp_dir().join("aether-settings-does-not-exist-4a1f.json");
        let _ = std::fs::remove_file(&path);
        assert_eq!(
            AppSettings::load_from(&path).expect("missing is fine"),
            AppSettings::default()
        );
    }

    #[test]
    fn saving_then_loading_preserves_the_folder_order() {
        let path = std::env::temp_dir().join("aether-settings-round-trip-4a1f.json");
        let _ = std::fs::remove_file(&path);
        let settings = AppSettings {
            disabled_plugins: Vec::new(),
            data_roots: vec![PathBuf::from("/a/mod"), PathBuf::from("/game/data")],
            horizon: false,
            language: Language::default(),
            recent_projects: Vec::new(),
            node_lod: true,
        };
        settings.save_to(&path).expect("writes");
        assert_eq!(AppSettings::load_from(&path).expect("reads"), settings);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn adding_a_duplicate_folder_is_a_no_op() {
        // The picker returns the same path if you browse to it twice, and a
        // duplicated root would search the same folder twice per lookup.
        let mut settings = AppSettings::default();
        assert!(settings.add_data_root(PathBuf::from("/game/data")));
        assert!(!settings.add_data_root(PathBuf::from("/game/data")));
        assert_eq!(settings.data_roots.len(), 1);
    }

    #[test]
    fn merging_legacy_project_roots_keeps_existing_priority() {
        // Opening an old project must not reshuffle folders the user already
        // ordered by hand.
        let mut settings = AppSettings {
            disabled_plugins: Vec::new(),
            data_roots: vec![PathBuf::from("/mod"), PathBuf::from("/game")],
            horizon: false,
            language: Language::default(),
            recent_projects: Vec::new(),
            node_lod: true,
        };
        let changed = settings.merge_data_roots(&[
            PathBuf::from("/game"),
            PathBuf::from("/other"),
        ]);
        assert!(changed);
        assert_eq!(
            settings.data_roots,
            vec![
                PathBuf::from("/mod"),
                PathBuf::from("/game"),
                PathBuf::from("/other"),
            ]
        );
    }
}
