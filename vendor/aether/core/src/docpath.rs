//! How a document stores a path to a file it does not own.
//!
//! A project that records `C:\Users\someone\...` is worse than useless
//! the moment it is shared or the drive is remounted — this crate
//! already says so about asset folders ([`crate::settings`]), and the
//! same sentence applies to the NIF a graph was imported from.
//!
//! Blender solved exactly this problem and the answer is two
//! characters: a stored path beginning `//` is relative to the
//! directory of the document that holds it (`BLI_path_rel` /
//! `BLI_path_abs`, `blenlib/intern/path_utils.cc`, documented and
//! unit-tested as exact inverses). There is no "is relative" flag and
//! no path struct; the prefix IS the type.
//!
//! Aether needs one form Blender does not, and it matters more than the
//! other two: a NIF is usually a GAME asset under a configured data
//! root, and `//../../../Steam/steamapps/common/...` is portable by
//! accident at best. `data:meshes/fx/foo.nif` is portable on purpose —
//! it names the asset the way the game names it, and resolves against
//! whatever install the recipient has configured.
//!
//! So three forms, tried in this order:
//!
//! | stored | means |
//! |---|---|
//! | `data:meshes/fx/foo.nif` | under a configured data root — portable to any install |
//! | `//nifs/foo.nif` | relative to the project directory — portable with the project |
//! | `C:\Games\...\foo.nif` | neither — kept verbatim, and REPORTED |
//!
//! Two rules that are easy to get wrong and are pinned by tests:
//!
//! **In memory a path is always ABSOLUTE.** Encoding happens at the
//! serialization boundary and nowhere else. Blender keeps the `//` form
//! in memory because DNA is its memory format, and pays for it with
//! `filepath_abs` mirrors, `fix_relpaths_library`, a rebase pass and a
//! temporary swap of `Main::filepath` during save that exists only to
//! fix a path-corruption bug. Aether has no legacy files to stay
//! compatible with, so Save-As rebasing is free: encode again from the
//! same absolute path.
//!
//! **Nothing here touches the filesystem.** Autosave calls the same
//! save path every few minutes; an `exists()` or a `canonicalize()` in
//! here would block on a dead network root, and would quietly downgrade
//! a temporarily-missing file to an absolute path that stops being
//! portable when it comes back. `BLI_path_rel` is pure string work too.
//! The one exception is [`DocPaths::decode`]'s choice of WHICH data
//! root a `data:` tail lives under, which probes — and falls back to
//! the first root rather than returning nothing, so the stored form is
//! never lost.

use std::path::{Component, Path, PathBuf};

/// Marks a path stored relative to the project directory. Blender's,
/// and its own comment concedes `//foo` is a legal absolute POSIX path
/// and that reinterpreting it "isn't standard in any OS". Windows UNC
/// is `\\`, not `//`, so the practical collision here is nil — but
/// [`DocPaths::decode`] tests this prefix FIRST and the rule is written
/// down rather than left to be rediscovered from a hand-edited file.
pub const PROJECT_PREFIX: &str = "//";

/// Marks a path stored relative to a configured data root.
pub const DATA_PREFIX: &str = "data:";

/// Why a path could not be stored portably.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unportable {
    /// The project has never been saved, so there is no directory to be
    /// relative to, and the file is under no data root.
    ProjectNotSaved,
    /// On a different drive or share from both the project and every
    /// data root — no relative path exists at all.
    NoCommonRoot,
    /// The path is not valid UTF-8, so it cannot go through JSON
    /// without being changed into a different path.
    NotUtf8,
}

impl std::fmt::Display for Unportable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unportable::ProjectNotSaved => {
                write!(f, "the project has not been saved anywhere yet")
            }
            Unportable::NoCommonRoot => write!(
                f,
                "it is on a different drive from the project and from every data folder"
            ),
            Unportable::NotUtf8 => write!(f, "its name is not valid UTF-8"),
        }
    }
}

/// The two anchors a stored path can be relative to.
#[derive(Debug, Clone, Default)]
pub struct DocPaths {
    /// The project DIRECTORY — never a file inside it.
    ///
    /// Blender's own primitive cuts at the LAST slash of its base, so
    /// `/foo/bar/` and `/foo/bar` give different answers for the same
    /// input (their test file pins both). Naming this `project_dir` and
    /// treating it as a directory at both ends is how the two cannot
    /// disagree.
    pub project_dir: Option<PathBuf>,
    /// Data roots in search order, as [`crate::settings::AppSettings`]
    /// holds them. First match wins, which is the rule the asset
    /// resolver already follows.
    pub data_roots: Vec<PathBuf>,
}

impl DocPaths {
    pub fn new(project_dir: Option<PathBuf>, data_roots: Vec<PathBuf>) -> DocPaths {
        DocPaths {
            project_dir,
            data_roots,
        }
    }

    /// The portable spelling of `abs`, or why there isn't one.
    ///
    /// The order is the point: a file under a data root gets the
    /// `data:` form even when it ALSO sits inside the project
    /// directory, because that is the only form that survives a
    /// different install letter. Swapping the two branches would store
    /// a machine-local `//../../Steam/...` chain for every vanilla
    /// file — which decodes perfectly on the machine that wrote it,
    /// and is exactly the failure this exists to prevent.
    pub fn encode(&self, abs: &Path) -> Result<String, Unportable> {
        let text = abs.to_str().ok_or(Unportable::NotUtf8)?;
        // A lossy conversion would substitute U+FFFD and record a
        // DIFFERENT path, which resolves to nothing and reads to the
        // user as "it lost my file again". `to_str` refuses instead.
        let _ = text;

        if let Some(tail) = self.under_data_root(abs) {
            // Separators normalized, CASE KEPT. It is tempting to run
            // `assets::normalize` here — this is a game asset path and
            // that is the spelling archives are keyed by — but it
            // LOWERCASES, and what comes back out of `decode` is handed
            // to the filesystem, not to the archive reader. On a
            // case-sensitive host that would stop resolving, and even
            // on Windows it would make the round trip inexact for no
            // gain: matching a path against a data root is already
            // case-insensitive on the way IN (`strip_prefix_ci`), and
            // every consumer that needs the archive spelling
            // normalizes its own input.
            return Ok(format!("{DATA_PREFIX}{tail}"));
        }

        let Some(dir) = self.project_dir.as_deref() else {
            return Err(Unportable::ProjectNotSaved);
        };
        match relative_to(dir, abs) {
            Some(rel) => Ok(format!("{PROJECT_PREFIX}{rel}")),
            None => Err(Unportable::NoCommonRoot),
        }
    }

    /// The portable spelling if there is one, the absolute path
    /// verbatim if there is not.
    ///
    /// What a SAVE writes: an unportable path is kept exactly as it is,
    /// like `BLI_path_rel`'s early return, because losing the reference
    /// is worse than storing one that only works here. The caller asks
    /// [`Self::encode`] separately when it wants to REPORT the problem.
    pub fn encode_or_absolute(&self, abs: &Path) -> Option<String> {
        match self.encode(abs) {
            Ok(s) => Some(s),
            Err(Unportable::NotUtf8) => None,
            Err(_) => abs.to_str().map(str::to_string),
        }
    }

    /// The inverse of [`Self::encode`] for the same `DocPaths`.
    pub fn decode(&self, stored: &str) -> PathBuf {
        if let Some(rel) = stored.strip_prefix(DATA_PREFIX) {
            let rel = rel.trim_start_matches('/');
            // Which root it lives under is the one thing worth probing
            // — the same asset may exist in several. Falling back to
            // the first keeps the reference addressable either way.
            for root in &self.data_roots {
                let candidate = join_slashed(root, rel);
                if candidate.exists() {
                    return candidate;
                }
            }
            if let Some(root) = self.data_roots.first() {
                return join_slashed(root, rel);
            }
            return PathBuf::from(rel);
        }
        if let Some(rel) = stored.strip_prefix(PROJECT_PREFIX) {
            if let Some(dir) = self.project_dir.as_deref() {
                return join_slashed(dir, rel);
            }
            return PathBuf::from(rel);
        }
        PathBuf::from(stored)
    }

    /// The tail of `abs` below whichever data root contains it, with
    /// the roots tried in order.
    fn under_data_root(&self, abs: &Path) -> Option<String> {
        for root in &self.data_roots {
            if let Some(rel) = strip_prefix_ci(root, abs) {
                return Some(rel);
            }
        }
        None
    }
}

/// `abs` spelled relative to the directory `base`, with forward slashes
/// and `../` where it has to climb.
///
/// Forward slashes ALWAYS, on every platform. Blender writes native
/// separators and then pays for it with a whole load-time pass
/// (`clean_paths`) that rewrites every path in every datablock; that
/// pass exists only because the writer did not pick one spelling.
/// Windows accepts forward slashes, so there is nothing to buy back.
///
/// Returns `None` when no relative path exists — a different Windows
/// drive or a different UNC share, which is `BLI_path_rel`'s own early
/// return.
fn relative_to(base: &Path, abs: &Path) -> Option<String> {
    let b: Vec<Component> = base.components().collect();
    let a: Vec<Component> = abs.components().collect();
    // The prefix (`C:`, `\\server\share`) and the root must match, or
    // there is no path from one to the other.
    let root_of = |c: &[Component]| -> Vec<String> {
        c.iter()
            .take_while(|x| matches!(x, Component::Prefix(_) | Component::RootDir))
            .map(|x| x.as_os_str().to_string_lossy().to_lowercase())
            .collect()
    };
    if root_of(&b) != root_of(&a) {
        return None;
    }
    let mut i = 0;
    while i < b.len() && i < a.len() && same_component(&b[i], &a[i]) {
        i += 1;
    }
    let mut out = String::new();
    for _ in i..b.len() {
        out.push_str("../");
    }
    for (n, c) in a[i..].iter().enumerate() {
        if n > 0 {
            out.push('/');
        }
        out.push_str(&c.as_os_str().to_string_lossy());
    }
    (!out.is_empty()).then_some(out)
}

/// Case-insensitively, because Windows is and the game is.
fn same_component(a: &Component, b: &Component) -> bool {
    a.as_os_str()
        .to_string_lossy()
        .eq_ignore_ascii_case(&b.as_os_str().to_string_lossy())
}

/// `abs` below `root`, as a slash-separated tail.
fn strip_prefix_ci(root: &Path, abs: &Path) -> Option<String> {
    let r: Vec<Component> = root.components().collect();
    let a: Vec<Component> = abs.components().collect();
    if a.len() <= r.len() {
        return None;
    }
    for (x, y) in r.iter().zip(a.iter()) {
        if !same_component(x, y) {
            return None;
        }
    }
    let tail: Vec<String> = a[r.len()..]
        .iter()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
    (!tail.is_empty()).then(|| tail.join("/"))
}

/// Join a slash-separated tail (possibly with `../`) onto a directory.
fn join_slashed(base: &Path, rel: &str) -> PathBuf {
    let mut out = base.to_path_buf();
    for part in rel.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(project: &str, roots: &[&str]) -> DocPaths {
        DocPaths::new(
            Some(PathBuf::from(project)),
            roots.iter().map(PathBuf::from).collect(),
        )
    }

    /// encode then decode is the identity, for every form — including
    /// the two that differ only by how far they have to climb.
    ///
    /// All four cases live in ONE test on purpose. Blender's own
    /// primitive cuts at the LAST slash of its base, so getting the
    /// directory-vs-file convention wrong at one end breaks the SIBLING
    /// and SUBDIRECTORY cases while the data-root case sails through;
    /// splitting them up is how that trap hides.
    #[test]
    fn every_form_survives_a_round_trip() {
        let dp = paths(r"C:\work\fire.aether", &[r"C:\Steam\Skyrim\Data"]);
        for original in [
            // under a data root, and MIXED CASE on purpose: the tail is
            // handed back to the filesystem, so lowercasing it would
            // make this an identity only up to case — and would stop
            // resolving outright on a case-sensitive host.
            r"C:\Steam\Skyrim\Data\Meshes\FX\GiantCampfire01.nif",
            // inside the project directory
            r"C:\work\fire.aether\nifs\foo.nif",
            // a SIBLING of the project directory: needs ../
            r"C:\work\models\foo.nif",
            // far above it
            r"C:\foo.nif",
        ] {
            let abs = PathBuf::from(original);
            let stored = dp.encode(&abs).unwrap_or_else(|e| panic!("{original}: {e}"));
            let back = dp.decode(&stored);
            assert_eq!(
                back,
                abs,
                "{original} stored as {stored} came back as {}",
                back.display()
            );
        }
    }

    /// A file under BOTH a data root and the project directory stores
    /// as `data:`.
    ///
    /// Swap the two branches in `encode` and every vanilla NIF records
    /// a machine-local `//../../Steam/...` chain instead. That decodes
    /// perfectly on the machine that wrote it, so no round-trip test
    /// can see it — the project simply fails to open on anyone else's.
    #[test]
    fn the_data_root_form_wins_over_the_project_form() {
        let dp = paths(r"C:\Steam\Skyrim\Data\projects\fx.aether", &[r"C:\Steam\Skyrim\Data"]);
        let abs = PathBuf::from(r"C:\Steam\Skyrim\Data\projects\fx.aether\nifs\a.nif");
        let stored = dp.encode(&abs).expect("portable");
        assert!(
            stored.starts_with(DATA_PREFIX),
            "stored as {stored}, but it is under a data root"
        );
        assert_eq!(stored, "data:projects/fx.aether/nifs/a.nif");
    }

    /// A data root spelled in a different case still matches, and the
    /// FILE's own case survives.
    ///
    /// Two rules pulling opposite ways, so they get one test: the match
    /// must be case-insensitive (Windows is, and a user typing their
    /// install path will not match Steam's capitalisation), while the
    /// tail must be preserved (it is handed back to the filesystem).
    /// Lowercasing the tail passes the round-trip on Windows and breaks
    /// silently everywhere else.
    #[test]
    fn a_data_root_matches_case_insensitively_but_keeps_the_files_case() {
        let dp = paths(r"C:\work\fx.aether", &[r"c:\steam\skyrim\DATA"]);
        let abs = PathBuf::from(r"C:\Steam\Skyrim\Data\Meshes\FX\Foo.nif");
        assert_eq!(dp.encode(&abs).as_deref(), Ok("data:Meshes/FX/Foo.nif"));
    }

    /// Another drive has no relative spelling at all, and saying so is
    /// the point: `encode` reports it, `encode_or_absolute` keeps the
    /// path byte-for-byte.
    ///
    /// Drop the root comparison and the `../` walker runs across
    /// drives, emitting a chain that decodes to a path which does not
    /// exist — so the project loses the file on the machine that
    /// authored it.
    #[test]
    fn a_path_on_another_drive_stays_absolute_and_is_named() {
        let dp = paths(r"C:\work\fire.aether", &[r"C:\Steam\Skyrim\Data"]);
        let abs = PathBuf::from(r"D:\mods\foo.nif");
        assert_eq!(dp.encode(&abs), Err(Unportable::NoCommonRoot));
        let stored = dp.encode_or_absolute(&abs).expect("kept");
        assert_eq!(stored, r"D:\mods\foo.nif");
        assert_eq!(dp.decode(&stored), abs);
    }

    /// An unsaved project has no directory to be relative to. The
    /// data-root form still works, because it does not need one.
    #[test]
    fn an_unsaved_project_can_still_store_a_game_asset() {
        let dp = DocPaths::new(None, vec![PathBuf::from(r"C:\Steam\Skyrim\Data")]);
        let asset = PathBuf::from(r"C:\Steam\Skyrim\Data\meshes\a.nif");
        assert_eq!(dp.encode(&asset).as_deref(), Ok("data:meshes/a.nif"));
        let loose = PathBuf::from(r"C:\elsewhere\a.nif");
        assert_eq!(dp.encode(&loose), Err(Unportable::ProjectNotSaved));
        // ...and it is still kept, so saving before the project has a
        // home does not throw the reference away.
        assert_eq!(
            dp.encode_or_absolute(&loose).as_deref(),
            Some(r"C:\elsewhere\a.nif")
        );
    }

    /// Encoding never touches the disk.
    ///
    /// Add a `canonicalize()` or an `exists()` guard and autosave —
    /// which calls the same save path every few minutes — blocks on a
    /// dead network root, while every temporarily-unavailable file
    /// quietly downgrades to an absolute path that will not be portable
    /// when it comes back.
    #[test]
    fn encoding_touches_no_filesystem() {
        let dp = paths(
            r"Q:\no\such\project.aether",
            &[r"Q:\no\such\install\Data"],
        );
        let abs = PathBuf::from(r"Q:\no\such\install\Data\meshes\ghost.nif");
        assert_eq!(dp.encode(&abs).as_deref(), Ok("data:meshes/ghost.nif"));
    }

    /// The stored form is platform-independent: forward slashes, always.
    #[test]
    fn the_stored_form_uses_forward_slashes() {
        let dp = paths(r"C:\work\fire.aether", &[]);
        let stored = dp
            .encode(&PathBuf::from(r"C:\work\fire.aether\nifs\deep\a.nif"))
            .expect("portable");
        assert_eq!(stored, "//nifs/deep/a.nif");
        assert!(!stored.contains('\\'), "{stored} carries a native separator");
    }
}
