//! The persistent user library: named graph fragments on disk,
//! grouped into virtual folders — "save this system, use it in the
//! next mod" (user roadmap, 2026-08-05).
//!
//! One item per file under the config directory's `library/`, JSON,
//! reusing the graph serializer. The FILE is the truth: the folder is
//! a field, not a directory, so "folders" are virtual groupings the
//! panel renders — moving an item between folders cannot lose it, and
//! a hand-copied .json from another machine simply appears.

use crate::graph::Graph;
use crate::json::Json;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct LibraryItem {
    pub name: String,
    /// Virtual folder ("" = top level).
    pub folder: String,
    pub fragment: Graph,
}

impl LibraryItem {
    pub fn to_json(&self) -> Json {
        Json::obj()
            .with("format_version", Json::Num(FORMAT_VERSION as f64))
            .with("name", Json::Str(self.name.clone()))
            .with("folder", Json::Str(self.folder.clone()))
            .with("fragment", self.fragment.to_json())
    }

    pub fn from_json(json: &Json) -> Option<LibraryItem> {
        // An item written before nodes carried their authored bytes is
        // NOT loadable, on purpose. Its nodes would paste with every
        // unmodelled field rebuilt from `Default` -- which woke
        // inactive modifiers, switched on emitters the file leaves
        // alone, and dropped `has_rotation_speeds`, crashing the game.
        // Silently offering such an item again would be offering the
        // same defect; the item is reported as outdated and the user
        // saves it again from a file that can supply the bytes.
        if json
            .get("format_version")
            .and_then(Json::as_u64)
            .unwrap_or(0)
            < FORMAT_VERSION as u64
        {
            return None;
        }
        Some(LibraryItem {
            name: json.get("name")?.as_str()?.to_string(),
            folder: json
                .get("folder")
                .and_then(|f| f.as_str())
                .unwrap_or("")
                .to_string(),
            fragment: Graph::from_json(json.get("fragment")?)?,
        })
    }
}

/// The library format's own version.
///
/// 2 is the first that carries [`crate::graph::AuthoredBlock`] with
/// each node -- the bytes of the block it was authored from, which is
/// what makes a paste keep the fields the graph does not model. Items
/// written by an earlier build cannot supply them and are refused
/// rather than pasted lossily.
pub const FORMAT_VERSION: u32 = 2;

/// `<config>/Aether/library`, beside settings.json. The
/// `AETHER_LIBRARY_DIR` env var overrides it — what lets the
/// screenshot harness use a scratch library instead of the user's,
/// and a portable install keep items beside the binary.
pub fn library_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("AETHER_LIBRARY_DIR") {
        return Some(PathBuf::from(dir));
    }
    Some(crate::settings::AppSettings::config_path()?.parent()?.join("library"))
}

/// The filename an item saves under: folder and name flattened to a
/// slug. Saving the same folder/name again REPLACES the item — that
/// is what "update my flame preset" means — while a different name is
/// a different file and can never clobber a neighbour.
pub fn item_slug(folder: &str, name: &str) -> String {
    let mut slug = String::new();
    for part in [folder, name] {
        if !slug.is_empty() {
            slug.push_str("--");
        }
        for c in part.chars() {
            if c.is_ascii_alphanumeric() {
                slug.push(c.to_ascii_lowercase());
            } else if !slug.ends_with('-') && !slug.is_empty() {
                slug.push('-');
            }
        }
        while slug.ends_with('-') {
            slug.pop();
        }
    }
    if slug.is_empty() {
        slug.push_str("item");
    }
    slug
}

/// Write an item into `dir`, creating the directory on first use.
pub fn save(dir: &Path, item: &LibraryItem) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(format!("{}.json", item_slug(&item.folder, &item.name)));
    std::fs::write(&path, item.to_json().pretty())
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// Every readable item in `dir`, sorted by (folder, name); unreadable
/// files are reported by path rather than silently skipped.
pub fn scan(dir: &Path) -> (Vec<(PathBuf, LibraryItem)>, Vec<String>) {
    let mut items = Vec::new();
    let mut broken = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        // No directory yet = an empty library, the normal first run.
        Err(_) => return (items, broken),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let parsed = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| Json::parse(&text).ok())
            .and_then(|json| LibraryItem::from_json(&json));
        match parsed {
            Some(item) => items.push((path, item)),
            None => broken.push(format!("{}", path.display())),
        }
    }
    items.sort_by(|a, b| {
        (a.1.folder.as_str(), a.1.name.as_str()).cmp(&(b.1.folder.as_str(), b.1.name.as_str()))
    });
    (items, broken)
}
