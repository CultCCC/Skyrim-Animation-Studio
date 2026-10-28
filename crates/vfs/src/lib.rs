//! The game's Data folder as an MO2 profile sees it.
//!
//! Priority, lowest first: BSA archives (vanilla `sResourceArchiveList`, then
//! the archives named after each active plugin in load order), then loose
//! files: the game's own Data, every enabled mod in modlist order, and the
//! overwrite folder last. Loose files always beat archives, as in the game.
//!
//! Paths are keyed the way Aether keys them: lowercase, forward slashes, no
//! leading separator ([`normalize`]).

use aether_core::bsa::BsaArchive;
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

pub use aether_core::assets::normalize;

/// The parts of an MO2 instance that decide what the game sees.
#[derive(Debug, Clone)]
pub struct Mo2Instance {
    pub root: PathBuf,
    pub game_dir: PathBuf,
    pub mods_dir: PathBuf,
    pub profiles_dir: PathBuf,
    pub overwrite_dir: PathBuf,
    pub selected_profile: Option<String>,
}

impl Mo2Instance {
    /// Read `ModOrganizer.ini` in `root`.
    pub fn open(root: &Path) -> io::Result<Mo2Instance> {
        let ini = std::fs::read(root.join("ModOrganizer.ini"))?;
        let ini = String::from_utf8_lossy(&ini);
        let mut values: HashMap<String, String> = HashMap::new();
        for line in ini.lines() {
            if let Some((k, v)) = line.split_once('=') {
                values.entry(k.trim().to_string()).or_insert_with(|| unquote(v.trim()));
            }
        }
        let base = values
            .get("base_directory")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| root.to_path_buf());
        let dir = |key: &str, default: &str| -> PathBuf {
            match values.get(key).filter(|v| !v.is_empty()) {
                Some(v) => PathBuf::from(v.replace("%BASE_DIR%", &base.to_string_lossy())),
                None => base.join(default),
            }
        };
        let game_dir = values
            .get("gamePath")
            .map(PathBuf::from)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "ModOrganizer.ini has no gamePath"))?;
        Ok(Mo2Instance {
            root: root.to_path_buf(),
            game_dir,
            mods_dir: dir("mod_directory", "mods"),
            profiles_dir: dir("profiles_directory", "profiles"),
            overwrite_dir: dir("overwrite_directory", "overwrite"),
            selected_profile: values.get("selected_profile").cloned(),
        })
    }

    pub fn profiles(&self) -> Vec<String> {
        let mut out: Vec<String> = std::fs::read_dir(&self.profiles_dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().join("modlist.txt").is_file())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        out.sort();
        out
    }

    /// Enabled mods of a profile, lowest priority first.
    pub fn enabled_mods(&self, profile: &str) -> io::Result<Vec<String>> {
        let text = std::fs::read(self.profiles_dir.join(profile).join("modlist.txt"))?;
        let text = String::from_utf8_lossy(&text);
        // modlist.txt lists the highest priority first.
        let mut mods: Vec<String> = text
            .lines()
            .filter_map(|l| l.strip_prefix('+'))
            .filter(|name| !name.ends_with("_separator"))
            .map(str::to_string)
            .collect();
        mods.reverse();
        Ok(mods)
    }
}

/// `@ByteArray(F:\\SDS)` and `"..."` forms in Qt ini files.
fn unquote(v: &str) -> String {
    let v = v
        .strip_prefix("@ByteArray(")
        .and_then(|s| s.strip_suffix(')'))
        .unwrap_or(v);
    let v = v.trim_matches('"');
    v.replace("\\\\", "\\")
}

/// One loose-file source.
#[derive(Debug, Clone)]
pub struct Root {
    /// Mod name as MO2 shows it; "Data" for the game folder, "Overwrite".
    pub name: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Loc {
    Loose(u32),
    Archive(u32),
}

pub struct Archive {
    pub name: String,
    pub bsa: BsaArchive,
}

/// What to build a [`Vfs`] from.
#[derive(Debug, Clone)]
pub enum Source {
    Mo2 { instance: PathBuf, profile: String },
    /// A plain Data folder (vanilla install or a merged folder).
    DataDir(PathBuf),
}

#[derive(Default)]
pub struct Progress {
    pub files: AtomicUsize,
    pub roots_done: AtomicUsize,
    pub roots_total: AtomicUsize,
}

pub struct Vfs {
    pub roots: Vec<Root>,
    pub archives: Vec<Archive>,
    /// Active plugins in load order.
    pub plugins: Vec<String>,
    pub profile_dir: Option<PathBuf>,
    files: HashMap<String, Loc>,
    pub warnings: Vec<String>,
}

const VANILLA_ARCHIVES: &[&str] = &[
    "Skyrim - Misc.bsa",
    "Skyrim - Shaders.bsa",
    "Skyrim - Interface.bsa",
    "Skyrim - Animations.bsa",
    "Skyrim - Meshes0.bsa",
    "Skyrim - Meshes1.bsa",
    "Skyrim - Sounds.bsa",
    "Skyrim - Voices_en0.bsa",
    "Skyrim - Textures0.bsa",
    "Skyrim - Textures1.bsa",
    "Skyrim - Textures2.bsa",
    "Skyrim - Textures3.bsa",
    "Skyrim - Textures4.bsa",
    "Skyrim - Textures5.bsa",
    "Skyrim - Textures6.bsa",
    "Skyrim - Textures7.bsa",
    "Skyrim - Textures8.bsa",
    "Skyrim - Patch.bsa",
];

const IMPLICIT_PLUGINS: &[&str] = &[
    "Skyrim.esm",
    "Update.esm",
    "Dawnguard.esm",
    "HearthFires.esm",
    "Dragonborn.esm",
];

impl Vfs {
    pub fn build(source: &Source, progress: &Progress) -> io::Result<Vfs> {
        let mut roots: Vec<Root> = Vec::new();
        let mut profile_dir = None;
        match source {
            Source::DataDir(dir) => roots.push(Root { name: "Data".into(), path: dir.clone() }),
            Source::Mo2 { instance, profile } => {
                let mo2 = Mo2Instance::open(instance)?;
                roots.push(Root { name: "Data".into(), path: mo2.game_dir.join("Data") });
                for name in mo2.enabled_mods(profile)? {
                    let path = mo2.mods_dir.join(&name);
                    if path.is_dir() {
                        roots.push(Root { name, path });
                    }
                }
                // A custom overwrite folder can be one of the mods; keep one copy, last.
                roots.retain(|r| r.path != mo2.overwrite_dir);
                roots.push(Root { name: "Overwrite".into(), path: mo2.overwrite_dir.clone() });
                profile_dir = Some(mo2.profiles_dir.join(profile));
            }
        }
        progress.roots_total.store(roots.len(), Ordering::Relaxed);

        // Loose files: later roots win.
        let mut files: HashMap<String, Loc> = HashMap::new();
        for (i, root) in roots.iter().enumerate() {
            walk(&root.path, &root.path, &mut |rel| {
                files.insert(rel, Loc::Loose(i as u32));
                progress.files.fetch_add(1, Ordering::Relaxed);
            });
            progress.roots_done.fetch_add(1, Ordering::Relaxed);
        }

        let mut warnings = Vec::new();
        let plugins = read_plugins(profile_dir.as_deref(), &roots, &files);

        // Archives: vanilla list, then plugin-named ones; later archives win
        // among archives, and never over a loose file.
        let mut archive_names: Vec<String> = archive_list(profile_dir.as_deref());
        for p in &plugins {
            let stem = p.rsplit_once('.').map(|(s, _)| s).unwrap_or(p);
            archive_names.push(format!("{stem}.bsa"));
            archive_names.push(format!("{stem} - Textures.bsa"));
        }
        let mut archives: Vec<Archive> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for name in archive_names {
            let key = normalize(&name);
            if !seen.insert(key.clone()) {
                continue;
            }
            let Some(Loc::Loose(r)) = files.get(&key).copied() else { continue };
            let path = roots[r as usize].path.join(&name);
            match BsaArchive::open(&path) {
                Ok(bsa) => archives.push(Archive { name, bsa }),
                Err(e) => warnings.push(format!("{}: {e}", path.display())),
            }
        }
        // Archives fill only paths no loose file has; a later archive beats an earlier one.
        let mut from_archive: HashMap<String, Loc> = HashMap::new();
        for (i, a) in archives.iter().enumerate() {
            for n in a.bsa.names() {
                from_archive.insert(n.clone(), Loc::Archive(i as u32));
            }
        }
        for (k, v) in from_archive {
            files.entry(k).or_insert(v);
        }

        Ok(Vfs { roots, archives, plugins, profile_dir, files, warnings })
    }

    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    pub fn locate(&self, path: &str) -> Option<Loc> {
        self.files.get(&normalize(path)).copied()
    }

    pub fn exists(&self, path: &str) -> bool {
        self.locate(path).is_some()
    }

    pub fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        let key = normalize(path);
        match self.files.get(&key) {
            Some(Loc::Loose(r)) => std::fs::read(self.roots[*r as usize].path.join(&key)),
            Some(Loc::Archive(a)) => Ok(self.archives[*a as usize].bsa.read_entry(&key)?),
            None => Err(io::Error::new(io::ErrorKind::NotFound, format!("not in the load order: {path}"))),
        }
    }

    /// The file on disk, for loose files.
    pub fn loose_path(&self, path: &str) -> Option<PathBuf> {
        let key = normalize(path);
        match self.files.get(&key)? {
            Loc::Loose(r) => Some(self.roots[*r as usize].path.join(&key)),
            Loc::Archive(_) => None,
        }
    }

    /// Mod (or archive) that provides the winning copy.
    pub fn origin(&self, path: &str) -> Option<String> {
        Some(match self.locate(path)? {
            Loc::Loose(r) => self.roots[r as usize].name.clone(),
            Loc::Archive(a) => format!("{} (BSA)", self.archives[a as usize].name),
        })
    }

    /// Every path that matches, unsorted.
    pub fn paths(&self, mut keep: impl FnMut(&str) -> bool) -> Vec<String> {
        self.files.keys().filter(|k| keep(k)).cloned().collect()
    }

    /// True when SKSE\Plugins\SDSCombat.dll is in the load order: its
    /// annotations only take part in previews then.
    pub fn has_sdscombat(&self) -> bool {
        self.exists("skse/plugins/sdscombat.dll")
    }
}

fn walk(base: &Path, dir: &Path, sink: &mut dyn FnMut(String)) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let Ok(ft) = e.file_type() else { continue };
        let p = e.path();
        if ft.is_dir() {
            walk(base, &p, sink);
        } else if let Ok(rel) = p.strip_prefix(base) {
            let rel = normalize(&rel.to_string_lossy());
            // MO2 metadata, not game files.
            if rel == "meta.ini" {
                continue;
            }
            sink(rel);
        }
    }
}

fn read_plugins(profile: Option<&Path>, roots: &[Root], files: &HashMap<String, Loc>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let push = |name: &str, out: &mut Vec<String>| {
        if files.contains_key(&normalize(name)) && !out.iter().any(|p| p.eq_ignore_ascii_case(name)) {
            out.push(name.to_string());
        }
    };
    for p in IMPLICIT_PLUGINS {
        push(p, &mut out);
    }
    // Creation Club content listed by the game.
    if let Some(data) = roots.first() {
        if let Ok(ccc) = std::fs::read_to_string(data.path.join("..").join("Skyrim.ccc")) {
            for l in ccc.lines() {
                push(l.trim(), &mut out);
            }
        }
    }
    if let Some(profile) = profile {
        if let Ok(txt) = std::fs::read(profile.join("plugins.txt")) {
            for l in String::from_utf8_lossy(&txt).lines() {
                if let Some(name) = l.trim().strip_prefix('*') {
                    push(name, &mut out);
                }
            }
        }
    }
    out
}

fn archive_list(profile: Option<&Path>) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(ini) = profile.and_then(|p| std::fs::read(p.join("Skyrim.ini")).ok()) {
        for l in String::from_utf8_lossy(&ini).lines() {
            let l = l.trim();
            for key in ["sResourceArchiveList=", "sResourceArchiveList2="] {
                if let Some(v) = l.strip_prefix(key) {
                    out.extend(v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()));
                }
            }
        }
    }
    if out.is_empty() {
        out = VANILLA_ARCHIVES.iter().map(|s| s.to_string()).collect();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qt_values() {
        assert_eq!(unquote("@ByteArray(F:\\\\SDS\\\\SkyrimSDS)"), "F:\\SDS\\SkyrimSDS");
        assert_eq!(unquote("@ByteArray(SDS 0.55 - Community Shaders)"), "SDS 0.55 - Community Shaders");
    }
}
