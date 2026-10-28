//! Asset path resolution, modelled on NifSkope.
//!
//! Texture paths inside a NIF are game-relative and inconsistent in practice:
//! some are `textures\effects\foo.dds`, some drop the `textures\` prefix, some
//! use forward slashes, some are absolute paths left over from an exporter.
//! NifSkope resolves them with a fixed search order that mod authors have
//! relied on for years, so Aether uses the same one (verified against the
//! niftools texture-management docs):
//!
//! 1. the path as-is, if it exists;
//! 2. the directory of the NIF being edited;
//! 3. the application directory;
//! 4. the configured data folders, in order — an entry starting with `./` or
//!    `.\` is relative to the NIF's own directory rather than absolute;
//! 5. registered archives (BSA/BA2);
//! 6. if still unresolved, prepend `textures\` and retry the whole sequence.
//!
//! Step 6 is what makes half the real-world NIFs work at all.

use std::path::{Path, PathBuf};

/// Where a resolved asset came from — the UI shows this so a user can tell a
/// loose override from the archive copy it is shadowing.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AssetSource {
    /// A loose file on disk.
    Loose(PathBuf),
    /// An entry inside an archive: `(archive path, normalized entry name)`.
    Archive(PathBuf, String),
}

impl AssetSource {
    /// Short label for the UI.
    pub fn label(&self) -> String {
        match self {
            AssetSource::Loose(p) => p.display().to_string(),
            AssetSource::Archive(archive, entry) => {
                let name = archive
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| archive.display().to_string());
                format!("{name} : {entry}")
            }
        }
    }
}

/// An archive that can answer "do you have this file, and what are its bytes".
/// Implemented by the BSA reader; kept as a trait so resolution can be tested
/// without shipping a real archive in the repo.
/// `Send + Sync` is load-bearing, not decoration: the world builder
/// parses a block's missing models on a worker pool, and every one of
/// those reads goes through here. Both shipped implementations read
/// POSITIONALLY (`seek_read`/`read_at`: an absolute offset per call,
/// no shared cursor consulted), so the bound describes what the code
/// already does rather than asking implementors for a promise they
/// would have to add locking to keep.
pub trait Archive: std::fmt::Debug + Send + Sync {
    /// Path of the archive itself (for reporting).
    fn path(&self) -> &Path;
    /// Does the archive contain `normalized` (lowercase, forward slashes)?
    fn contains(&self, normalized: &str) -> bool;
    /// Read the entry's bytes.
    fn read(&self, normalized: &str) -> std::io::Result<Vec<u8>>;
    /// Every entry, normalized — used to populate the texture browser.
    fn entries(&self) -> Vec<String>;

    /// The entries under `prefix`, normalized.
    ///
    /// The default answers it by filtering [`Archive::entries`], which is
    /// correct but pays for the whole archive: a `Vec<String>` of every
    /// name, allocated and dropped, per call. An implementation that can
    /// answer a prefix directly should — measuring the LOD index, this
    /// one call over 14 stock archives was 116 ms of a 131 ms horizon,
    /// and none of it was the horizon.
    fn entries_under(&self, prefix: &str) -> Vec<String> {
        self.entries()
            .into_iter()
            .filter(|e| e.starts_with(prefix))
            .collect()
    }
}

/// Extensions tried in turn when the exact one is missing, mirroring
/// NifSkope's "alternate extensions" option. `.dds` is always first because
/// that is what the game actually ships.
pub const ALTERNATE_EXTENSIONS: &[&str] = &["dds", "tga", "bmp"];

/// What counts as a texture for the browser. Public so a caller can ask
/// `list_under` for the same set instead of restating it.
pub const TEXTURE_EXTENSIONS: &[&str] = &["dds", "tga", "bmp", "png"];

/// The configured search locations. Mirrors NifSkope's
/// Options → Settings → Resources: a list of folders and a list of archives.
#[derive(Debug, Default)]
pub struct AssetPaths {
    /// Data folders, searched in order. `./` or `.\` prefix = relative to the
    /// NIF's directory.
    pub folders: Vec<PathBuf>,
    /// Registered archives, searched after the folders.
    pub archives: Vec<Box<dyn Archive>>,
    /// Directory of the NIF currently being edited, if any.
    pub nif_dir: Option<PathBuf>,
    /// Directory the editor itself runs from.
    pub app_dir: Option<PathBuf>,
    /// Try `.tga`/`.bmp` when the requested extension is missing.
    pub alternate_extensions: bool,
}

impl AssetPaths {
    /// A data root as the single configured folder — the common case, and what
    /// the Project panel's "Data root" field feeds in.
    pub fn with_data_root(root: impl Into<PathBuf>) -> Self {
        AssetPaths {
            folders: vec![root.into()],
            ..AssetPaths::default()
        }
    }

    /// Resolve a NIF-relative asset path. Returns the first hit in NifSkope's
    /// search order, or `None`.
    pub fn resolve(&self, raw: &str) -> Option<AssetSource> {
        let cleaned = raw.trim().trim_start_matches(['/', '\\']);
        if cleaned.is_empty() {
            return None;
        }

        for candidate in self.candidate_paths(cleaned) {
            if let Some(found) = self.search_everywhere(&candidate) {
                return Some(found);
            }
        }
        None
    }

    /// All spellings of `path` worth trying, in order: as given, then with a
    /// `textures/` prefix (NifSkope's path correction), then the alternate
    /// extensions of each if enabled.
    fn candidate_paths(&self, path: &str) -> Vec<String> {
        let normalized = normalize(path);
        let mut bases = vec![normalized.clone()];
        // Everything after the last `data/` SEGMENT.
        //
        // Two real cases, one rule. The Creation Kit, xLODGen and DynDOLOD
        // write paths rooted at the game folder ("data\textures\..."),
        // while every data root here already points AT Data. And Bethesda's
        // own HD build pipeline baked absolute build-machine paths into
        // shipping meshes — the Riverwood block alone asks for 26 textures
        // spelled "skyrimhd\build\pc\data\textures\plants\leek01.dds",
        // every one of which is really "textures\plants\leek01.dds".
        //
        // The last segment, not the first, so a nested build path collapses
        // all the way. The original spelling stays first: a mod that really
        // does keep a `textures/data/` folder must still win.
        if let Some(rest) = after_last_data_segment(&normalized) {
            bases.push(rest);
        }
        for base in bases.clone() {
            if !base.starts_with("textures/") {
                bases.push(format!("textures/{base}"));
            }
        }

        if !self.alternate_extensions {
            return bases;
        }
        let mut out = Vec::new();
        for base in bases {
            let stem = match base.rsplit_once('.') {
                Some((stem, _)) => stem.to_string(),
                None => base.clone(),
            };
            out.push(base);
            for ext in ALTERNATE_EXTENSIONS {
                let alt = format!("{stem}.{ext}");
                if !out.contains(&alt) {
                    out.push(alt);
                }
            }
        }
        out
    }

    /// Steps 1-5 for one exact spelling.
    fn search_everywhere(&self, normalized: &str) -> Option<AssetSource> {
        // 1. Absolute path, taken literally.
        let as_given = Path::new(normalized);
        if as_given.is_absolute() && as_given.is_file() {
            return Some(AssetSource::Loose(as_given.to_path_buf()));
        }

        // 2. Next to the NIF.
        if let Some(dir) = &self.nif_dir {
            let p = dir.join(normalized);
            if p.is_file() {
                return Some(AssetSource::Loose(p));
            }
        }

        // 3. Application directory.
        if let Some(dir) = &self.app_dir {
            let p = dir.join(normalized);
            if p.is_file() {
                return Some(AssetSource::Loose(p));
            }
        }

        // 4. Configured folders, `./` meaning "relative to the NIF".
        for folder in &self.folders {
            let Some(base) = self.resolve_folder(folder) else {
                continue;
            };
            let p = base.join(normalized);
            if p.is_file() {
                return Some(AssetSource::Loose(p));
            }
        }

        // 5. Archives.
        for archive in &self.archives {
            if archive.contains(normalized) {
                return Some(AssetSource::Archive(
                    archive.path().to_path_buf(),
                    normalized.to_string(),
                ));
            }
        }
        None
    }

    /// Expand a configured folder entry; `./x` is relative to the NIF's dir.
    fn resolve_folder(&self, folder: &Path) -> Option<PathBuf> {
        let s = folder.to_string_lossy();
        let rel = s
            .strip_prefix("./")
            .or_else(|| s.strip_prefix(".\\"))
            .map(str::to_string);
        match rel {
            Some(rel) => self.nif_dir.as_ref().map(|d| d.join(rel)),
            None => Some(folder.to_path_buf()),
        }
    }

    /// Read a resolved asset's bytes.
    pub fn read(&self, source: &AssetSource) -> std::io::Result<Vec<u8>> {
        match source {
            AssetSource::Loose(p) => std::fs::read(p),
            AssetSource::Archive(archive_path, entry) => {
                let archive = self
                    .archives
                    .iter()
                    .find(|a| a.path() == archive_path)
                    .ok_or_else(|| {
                        std::io::Error::new(
                            std::io::ErrorKind::NotFound,
                            format!("archive {} is no longer registered", archive_path.display()),
                        )
                    })?;
                archive.read(entry)
            }
        }
    }

    /// Convenience: resolve and read in one step.
    pub fn load(&self, raw: &str) -> Option<Vec<u8>> {
        let source = self.resolve(raw)?;
        self.read(&source).ok()
    }

    /// Every texture reachable through the configured paths, normalized and
    /// deduplicated — the backing list for the texture browser.
    pub fn list_textures(&self) -> Vec<String> {
        self.list_under("", TEXTURE_EXTENSIONS)
            .into_iter()
            .map(|(name, _)| name)
            .collect()
    }

    /// Every asset under `prefix` whose extension is in `extensions`,
    /// normalized, deduplicated and sorted, WITH the source it came from.
    ///
    /// `prefix` is a normalized DIRECTORY prefix ending in '/'
    /// ("meshes/terrain/tamriel/"); empty means everything. Extensions are
    /// lowercase and without a dot.
    ///
    /// Precedence is `resolve`'s, unchanged and for the same reason: the
    /// folders are walked in order and then the archives, first spelling
    /// wins, so a loose file shadows the archived copy exactly like the
    /// game.
    ///
    /// Returning the [`AssetSource`] rather than just the name is what lets
    /// a caller READ what it enumerated. Re-resolving would be both slower
    /// and wrong: `resolve` tries a `textures/` prefix, a `data/`-stripped
    /// spelling and the alternate extensions, so it can legitimately land
    /// on a different file than the one that was listed.
    pub fn list_under(&self, prefix: &str, extensions: &[&str]) -> Vec<(String, AssetSource)> {
        let mut out: Vec<(String, AssetSource)> = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for folder in &self.folders {
            let Some(base) = self.resolve_folder(folder) else {
                continue;
            };
            // Seed the walk INSIDE the prefix: a scoped listing must not
            // walk a 200,000-file Data root to throw almost all of it away.
            let start = if prefix.is_empty() {
                base.clone()
            } else {
                base.join(prefix)
            };
            if !start.is_dir() {
                continue;
            }
            collect_under(&base, &start, extensions, &mut out, &mut seen);
        }
        for archive in &self.archives {
            for entry in archive.entries_under(prefix) {
                if has_extension(&entry, extensions)
                    && seen.insert(entry.clone())
                {
                    let source = AssetSource::Archive(archive.path().to_path_buf(), entry.clone());
                    out.push((entry, source));
                }
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }
}

/// Everything after the last `data/` path SEGMENT, if there is one and it
/// leaves something behind.
///
/// Segment, never substring: `metadata/x.dds` must not become `/x.dds`,
/// and a file literally named `data` is not a folder.
fn after_last_data_segment(normalized: &str) -> Option<String> {
    let mut cut = None;
    for (i, _) in normalized.match_indices("data/") {
        // Either the very start of the path, or preceded by a separator.
        if i == 0 || normalized.as_bytes()[i - 1] == b'/' {
            cut = Some(i + "data/".len());
        }
    }
    let rest = &normalized[cut?..];
    (!rest.is_empty()).then(|| rest.to_string())
}

/// Lowercase, forward slashes — the form archives are keyed by and the form
/// comparisons happen in, so `Textures\FX\A.DDS` and `textures/fx/a.dds` are
/// the same asset.
pub fn normalize(path: &str) -> String {
    path.trim()
        .trim_start_matches(['/', '\\'])
        .replace('\\', "/")
        .to_lowercase()
}

fn has_extension(normalized: &str, extensions: &[&str]) -> bool {
    match normalized.rsplit_once('.') {
        Some((_, ext)) => extensions.contains(&ext),
        None => false,
    }
}

/// Walk `dir`, recording matching files as paths relative to `root`.
fn collect_under(
    root: &Path,
    dir: &Path,
    extensions: &[&str],
    out: &mut Vec<(String, AssetSource)>,
    seen: &mut std::collections::HashSet<String>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // `entry.file_type()` comes out of the directory enumeration
        // itself; `path.is_dir()` is a separate stat per entry. On the
        // one data root here that holds a loose LOD tree that is 11,057
        // extra syscalls, and it showed up as 73 ms of a horizon build.
        //
        // The fallback keeps the old meaning exactly: `is_dir` FOLLOWS
        // symlinks, `file_type` reports the link itself, so a symlink is
        // the one case that still pays for a stat.
        let is_dir = match entry.file_type() {
            Ok(ft) if ft.is_symlink() => path.is_dir(),
            Ok(ft) => ft.is_dir(),
            Err(_) => path.is_dir(),
        };
        if is_dir {
            collect_under(root, &path, extensions, out, seen);
        } else if let Ok(rel) = path.strip_prefix(root) {
            let key = normalize(&rel.to_string_lossy());
            if has_extension(&key, extensions) && seen.insert(key.clone()) {
                out.push((key, AssetSource::Loose(path)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Build a throwaway data tree; returns its root.
    fn make_tree(name: &str, files: &[&str]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("aether-assets-{name}"));
        let _ = fs::remove_dir_all(&root);
        for f in files {
            let p = root.join(f);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, b"x").unwrap();
        }
        root
    }

    #[derive(Debug)]
    struct FakeArchive {
        path: PathBuf,
        entries: Vec<String>,
    }

    impl Archive for FakeArchive {
        fn path(&self) -> &Path {
            &self.path
        }
        fn contains(&self, normalized: &str) -> bool {
            self.entries.iter().any(|e| e == normalized)
        }
        fn read(&self, normalized: &str) -> std::io::Result<Vec<u8>> {
            if self.contains(normalized) {
                Ok(b"archived".to_vec())
            } else {
                Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    normalized.to_string(),
                ))
            }
        }
        fn entries(&self) -> Vec<String> {
            self.entries.clone()
        }
    }

    /// Two real families of over-rooted paths, one rule. The Creation Kit,
    /// xLODGen and DynDOLOD write paths rooted at the game folder while
    /// every data root points at Data; and Bethesda's own HD build baked
    /// absolute build-machine paths into shipping meshes — the Riverwood
    /// block asks for 26 textures spelled `skyrimhd\build\pc\data\...`,
    /// none of which resolved before this.
    #[test]
    fn a_data_segment_anywhere_is_tried_without_its_prefix() {
        let root = make_tree(
            "dataprefix",
            &[
                "textures/terrain/tamriel/tamriel.4.4.-12.dds",
                "textures/plants/leek01.dds",
            ],
        );
        let paths = AssetPaths::with_data_root(&root);
        let terrain = AssetSource::Loose(root.join("textures/terrain/tamriel/tamriel.4.4.-12.dds"));
        let leek = AssetSource::Loose(root.join("textures/plants/leek01.dds"));

        for (spelling, want) in [
            ("Data\\textures\\terrain\\tamriel\\tamriel.4.4.-12.dds", &terrain),
            ("data\\terrain\\tamriel\\tamriel.4.4.-12.dds", &terrain),
            ("textures\\terrain\\tamriel\\tamriel.4.4.-12.dds", &terrain),
            ("skyrimhd\\build\\pc\\data\\textures\\plants\\Leek01.dds", &leek),
            ("plants\\leek01.dds", &leek),
        ] {
            assert_eq!(paths.resolve(spelling).as_ref(), Some(want), "{spelling}");
        }
    }

    /// `data` has to be a path SEGMENT. A folder called `metadata` shares
    /// the four letters and must not be cut in half.
    #[test]
    fn a_data_substring_is_not_a_data_segment() {
        let root = make_tree("datasubstring", &["textures/metadata/x.dds"]);
        let paths = AssetPaths::with_data_root(&root);
        assert_eq!(
            paths.resolve("metadata\\x.dds"),
            Some(AssetSource::Loose(root.join("textures/metadata/x.dds")))
        );
    }

    /// ...but the original spelling still goes first, so a mod that really
    /// does ship a `textures\data\` folder is not hijacked by the fallback.
    #[test]
    fn the_original_spelling_still_wins() {
        let root = make_tree(
            "dataliteral",
            &["textures/data/thing.dds", "textures/thing.dds"],
        );
        let paths = AssetPaths::with_data_root(&root);
        assert_eq!(
            paths.resolve("data\\thing.dds"),
            Some(AssetSource::Loose(root.join("textures/data/thing.dds")))
        );
    }

    #[test]
    fn missing_textures_prefix_is_recovered() {
        // The single most common real-world case: the NIF says
        // "effects\fxglow.dds" but the file lives under "textures\".
        let root = make_tree("prefix", &["textures/effects/fxglow.dds"]);
        let paths = AssetPaths::with_data_root(&root);

        let found = paths.resolve("effects\\fxglow.dds").expect("should resolve");
        assert_eq!(
            found,
            AssetSource::Loose(root.join("textures/effects/fxglow.dds"))
        );
        // And the already-correct spelling resolves to the same file.
        assert_eq!(paths.resolve("textures/effects/fxglow.dds"), Some(found));
    }

    #[test]
    fn backslashes_and_case_do_not_matter() {
        let root = make_tree("case", &["textures/fx/a.dds"]);
        let paths = AssetPaths::with_data_root(&root);
        assert!(paths.resolve("Textures\\FX\\A.DDS").is_some());
        assert!(paths.resolve("textures/fx/a.dds").is_some());
    }

    #[test]
    fn earlier_folders_override_later_ones() {
        // NifSkope semantics: the folder list is a priority order. A mod
        // folder listed above the game's Data must win for the same path.
        let mod_root = make_tree("prio-mod", &["textures/fx/a.dds"]);
        let game_root = make_tree("prio-game", &["textures/fx/a.dds", "textures/fx/b.dds"]);
        let paths = AssetPaths {
            folders: vec![mod_root.clone(), game_root.clone()],
            ..AssetPaths::default()
        };
        assert_eq!(
            paths.resolve("textures/fx/a.dds"),
            Some(AssetSource::Loose(mod_root.join("textures/fx/a.dds"))),
            "the higher-priority folder wins for a shadowed path"
        );
        assert_eq!(
            paths.resolve("textures/fx/b.dds"),
            Some(AssetSource::Loose(game_root.join("textures/fx/b.dds"))),
            "paths only in the lower folder still resolve"
        );
    }

    #[test]
    fn loose_files_win_over_archives() {
        // A loose file shadows the archived copy, exactly like the game.
        let root = make_tree("shadow", &["textures/fx/a.dds"]);
        let paths = AssetPaths {
            folders: vec![root.clone()],
            archives: vec![Box::new(FakeArchive {
                path: PathBuf::from("Skyrim - Textures.bsa"),
                entries: vec!["textures/fx/a.dds".into()],
            })],
            ..AssetPaths::default()
        };
        assert_eq!(
            paths.resolve("textures/fx/a.dds"),
            Some(AssetSource::Loose(root.join("textures/fx/a.dds")))
        );
    }

    #[test]
    fn archives_are_searched_when_no_loose_file_exists() {
        let paths = AssetPaths {
            archives: vec![Box::new(FakeArchive {
                path: PathBuf::from("Skyrim - Textures.bsa"),
                entries: vec!["textures/fx/only_archived.dds".into()],
            })],
            ..AssetPaths::default()
        };
        // Note the missing prefix: correction must apply to archives too.
        let found = paths.resolve("fx\\only_archived.dds").expect("resolves");
        assert!(matches!(found, AssetSource::Archive(_, ref e)
            if e == "textures/fx/only_archived.dds"));
        assert_eq!(paths.read(&found).unwrap(), b"archived".to_vec());
    }

    #[test]
    fn dot_prefixed_folders_are_relative_to_the_nif() {
        let root = make_tree("relative", &["meshes/fx/textures/a.dds"]);
        let paths = AssetPaths {
            folders: vec![PathBuf::from("./textures")],
            nif_dir: Some(root.join("meshes/fx")),
            ..AssetPaths::default()
        };
        assert_eq!(
            paths.resolve("a.dds"),
            Some(AssetSource::Loose(root.join("meshes/fx/textures/a.dds")))
        );
    }

    #[test]
    fn alternate_extensions_only_apply_when_enabled() {
        let root = make_tree("altext", &["textures/fx/b.tga"]);
        let mut paths = AssetPaths::with_data_root(&root);
        assert_eq!(paths.resolve("textures/fx/b.dds"), None);
        paths.alternate_extensions = true;
        assert_eq!(
            paths.resolve("textures/fx/b.dds"),
            Some(AssetSource::Loose(root.join("textures/fx/b.tga")))
        );
    }

    #[test]
    fn texture_listing_merges_folders_and_archives_without_duplicates() {
        let root = make_tree(
            "listing",
            &["textures/fx/a.dds", "textures/fx/b.dds", "meshes/x.nif"],
        );
        let paths = AssetPaths {
            folders: vec![root.clone()],
            archives: vec![Box::new(FakeArchive {
                path: PathBuf::from("T.bsa"),
                // One overlaps a loose file, one is archive-only.
                entries: vec!["textures/fx/a.dds".into(), "textures/fx/c.dds".into()],
            })],
            ..AssetPaths::default()
        };
        assert_eq!(
            paths.list_textures(),
            vec!["textures/fx/a.dds", "textures/fx/b.dds", "textures/fx/c.dds"],
            "meshes are excluded and the overlapping texture appears once"
        );
    }

    #[test]
    fn unresolvable_paths_report_nothing_rather_than_guessing() {
        let root = make_tree("missing", &["textures/fx/a.dds"]);
        let paths = AssetPaths::with_data_root(&root);
        assert_eq!(paths.resolve("textures/fx/nope.dds"), None);
        assert_eq!(paths.resolve(""), None);
        assert_eq!(paths.resolve("   "), None);
    }
}
