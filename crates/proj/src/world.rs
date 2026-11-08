//! The load order as the editor sees it: records by FormID string, search
//! lists for the form picker, spell → projectile, and the HomingProjectiles
//! configs every enabled mod ships.

use sds_records::{f32_at, u32_at, Db, FormKey};
use sds_vfs::Vfs;
use std::collections::HashMap;

use crate::model::CondFormat;

pub const CONFIG_DIR: &str = "homingprojectiles";
pub const DLL_PATH: &str = "skse/plugins/newprojectilestmp.dll";

/// Record types the picker lists.
pub const PICK_SIGS: &[&[u8; 4]] = &[b"PROJ", b"SPEL", b"MGEF", b"EXPL", b"WEAP", b"KYWD", b"NPC_"];

#[derive(Debug, Clone)]
pub struct Entry {
    pub key: FormKey,
    pub sig: [u8; 4],
    pub edid: String,
    pub name: String,
    /// `Plugin.esp|0x123456`.
    pub formid: String,
    /// Lowercase "edid name formid" for search.
    pub hay: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Lookup {
    Found { key: FormKey, sig: [u8; 4], edid: String, name: String },
    /// The player reference 0x14 (not a base record).
    Player,
    /// A plain hex id outside the indexed record types (REFR etc.).
    Unindexed,
    NoPlugin(String),
    NoRecord,
    Bad,
}

#[derive(Debug, Clone)]
pub struct ConfigFile {
    /// `homingprojectiles/x.json` (VFS path, lowercase).
    pub vfs_path: String,
    /// File name with its original case.
    pub name: String,
    pub origin: String,
}

/// Projectile facts for the chain view.
#[derive(Debug, Clone, Default)]
pub struct ProjInfo {
    pub kind: &'static str,
    pub speed: f32,
    pub range: f32,
    pub gravity: f32,
    pub explosion: Option<FormKey>,
}

pub struct World {
    pub db: Db,
    pub entries: Vec<Entry>,
    by_key: HashMap<FormKey, usize>,
    plugin_index: HashMap<String, u16>,
    pub configs: Vec<ConfigFile>,
    /// Format the installed DLL reads, from its strings (None: no DLL).
    pub dll_format: Option<CondFormat>,
    pub dll_origin: Option<String>,
}

impl World {
    pub fn build(vfs: &Vfs, language: &str) -> World {
        let db = Db::build(vfs, language);
        let mut entries = Vec::new();
        for sig in PICK_SIGS {
            let Some(keys) = db.by_sig.get(*sig) else { continue };
            for k in keys {
                let Some(r) = db.get(*k) else { continue };
                let edid = r.edid();
                let name = db.name(r);
                let formid = format!("{}|0x{:06X}", db.plugin_name(*k), k.id);
                let hay = format!("{} {} {}", edid, name, formid).to_lowercase();
                entries.push(Entry { key: *k, sig: **sig, edid, name, formid, hay });
            }
        }
        let by_key = entries.iter().enumerate().map(|(i, e)| (e.key, i)).collect();
        let plugin_index = db.plugins.iter().enumerate().map(|(i, p)| (p.name.to_ascii_lowercase(), i as u16)).collect();

        let mut configs: Vec<ConfigFile> = vfs
            .paths(|p| p.starts_with("homingprojectiles/") && p.ends_with(".json") && !p.contains("schema") && p.matches('/').count() == 1)
            .into_iter()
            .map(|p| {
                let lower = p.rsplit('/').next().unwrap_or(&p).to_string();
                // The VFS keys are lowercase; take the real case from the folder.
                let name = vfs
                    .loose_path(&p)
                    .and_then(|lp| std::fs::read_dir(lp.parent()?).ok())
                    .and_then(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).find(|n| n.to_lowercase() == lower))
                    .unwrap_or(lower);
                ConfigFile { origin: vfs.origin(&p).unwrap_or_default(), vfs_path: p, name }
            })
            .collect();
        configs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

        let (dll_format, dll_origin) = match vfs.read(DLL_PATH) {
            Ok(b) => (Some(detect_dll_format(&b)), vfs.origin(DLL_PATH)),
            Err(_) => (None, None),
        };
        World { db, entries, by_key, plugin_index, configs, dll_format, dll_origin }
    }

    pub fn entry(&self, k: FormKey) -> Option<&Entry> {
        self.by_key.get(&k).map(|i| &self.entries[*i])
    }

    /// `Plugin.esp|0xID` or `0xID` (Skyrim.esm / DLC by load index) → record.
    pub fn lookup(&self, s: &str) -> Lookup {
        let s = s.trim();
        let (plugin, hex) = match s.split_once('|') {
            Some((p, h)) => (Some(p.trim()), h.trim()),
            None => (None, s),
        };
        let Some(h) = hex.strip_prefix("0x").or_else(|| hex.strip_prefix("0X")) else { return Lookup::Bad };
        let Ok(raw) = u32::from_str_radix(h, 16) else { return Lookup::Bad };
        let key = match plugin {
            Some(p) => {
                let Some(m) = self.plugin_index.get(&p.to_ascii_lowercase()) else { return Lookup::NoPlugin(p.to_string()) };
                FormKey { module: *m, id: raw & 0x00FF_FFFF }
            }
            None => {
                if raw == 0x14 {
                    return Lookup::Player;
                }
                FormKey { module: (raw >> 24) as u16, id: raw & 0x00FF_FFFF }
            }
        };
        match self.db.get(key) {
            Some(r) => Lookup::Found { key, sig: r.sig, edid: r.edid(), name: self.db.name(r) },
            None if plugin.is_none() => Lookup::Unindexed,
            None => Lookup::NoRecord,
        }
    }

    pub fn key_of(&self, s: &str) -> Option<FormKey> {
        match self.lookup(s) {
            Lookup::Found { key, .. } => Some(key),
            _ => None,
        }
    }

    pub fn formid(&self, k: FormKey) -> String {
        format!("{}|0x{:06X}", self.db.plugin_name(k), k.id)
    }

    /// Short display: EditorID (name).
    pub fn describe(&self, s: &str) -> String {
        match self.lookup(s) {
            Lookup::Found { sig, edid, name, .. } => {
                let sig = String::from_utf8_lossy(&sig).into_owned();
                if name.is_empty() || name == edid {
                    format!("{sig} {edid}")
                } else {
                    format!("{sig} {edid} «{name}»")
                }
            }
            Lookup::Player => sds_i18n::t!("Игрок", "Player").into(),
            Lookup::Unindexed => s.to_string(),
            Lookup::NoPlugin(p) => sds_i18n::tf!("{p} не в загрузке", "{p} not loaded"),
            Lookup::NoRecord => sds_i18n::t!("запись не найдена", "record not found").into(),
            Lookup::Bad => sds_i18n::t!("неверный FormID", "bad FormID").into(),
        }
    }

    /// EditorID or the raw string.
    pub fn short(&self, s: &str) -> String {
        match self.lookup(s) {
            Lookup::Found { edid, .. } if !edid.is_empty() => edid,
            Lookup::Player => sds_i18n::t!("Игрок", "Player").into(),
            _ => s.to_string(),
        }
    }

    /// The projectile a spell fires: the first effect that has one.
    pub fn spell_projectile(&self, spell: FormKey) -> Option<FormKey> {
        let rec = self.db.get(spell)?;
        if &rec.sig != b"SPEL" {
            return None;
        }
        for (sig, d) in rec.subs() {
            if sig != b"EFID" {
                continue;
            }
            let Some(m) = u32_at(d, 0).and_then(|raw| self.db.link(rec, raw)) else { continue };
            if let Some(p) = self.mgef_projectile(m) {
                return Some(p);
            }
        }
        None
    }

    pub fn mgef_projectile(&self, mgef: FormKey) -> Option<FormKey> {
        let r = self.db.get(mgef)?;
        let d = r.first(b"DATA")?;
        u32_at(d, 72).and_then(|raw| self.db.link(r, raw))
    }

    /// Spells whose effects fire projectile `p` (for "who launches this").
    pub fn spells_firing(&self, p: FormKey) -> Vec<FormKey> {
        let mut out = Vec::new();
        if let Some(keys) = self.db.by_sig.get(b"SPEL") {
            for k in keys {
                if self.spell_projectile(*k) == Some(p) {
                    out.push(*k);
                }
            }
        }
        out
    }

    pub fn proj_info(&self, p: FormKey) -> Option<ProjInfo> {
        let r = self.db.get(p)?;
        if &r.sig != b"PROJ" {
            return None;
        }
        let d = r.first(b"DATA")?;
        let t = d.get(2..4).map(|b| u16::from_le_bytes([b[0], b[1]])).unwrap_or(0);
        let kind = match t {
            1 => "Missile",
            2 => "Lobber",
            4 => "Beam",
            8 => "Flame",
            0x10 => "Cone",
            0x20 => "Barrier",
            0x40 => "Arrow",
            _ => "?",
        };
        Some(ProjInfo {
            kind,
            gravity: f32_at(d, 4).unwrap_or(0.0),
            speed: f32_at(d, 8).unwrap_or(0.0),
            range: f32_at(d, 12).unwrap_or(0.0),
            explosion: u32_at(d, 36).and_then(|raw| self.db.link(r, raw)),
        })
    }
}

/// Builds with CK-style condition entries know the `invert` and `OR` keys;
/// the older object form does not.
pub fn detect_dll_format(dll: &[u8]) -> CondFormat {
    let has = |needle: &[u8]| dll.windows(needle.len() + 2).any(|w| w[0] == 0 && &w[1..=needle.len()] == needle && w[needle.len() + 1] == 0);
    if has(b"invert") || has(b"formID") {
        CondFormat::Array
    } else {
        CondFormat::Object
    }
}
