//! `sds_projectile_studio.json` next to the exe. A fresh install borrows the
//! MO2 instance, profile and language from SDS Anim Studio's settings when
//! they sit in the same folder.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Condition format to write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ExportFormat {
    /// The format the installed DLL reads (falls back to the file's own).
    #[default]
    Auto,
    Object,
    Array,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub use_mo2: bool,
    pub mo2_dir: String,
    pub profile: String,
    pub data_dir: String,
    /// Output mod folder: exports land in `<output>/HomingProjectiles/`.
    pub output_dir: String,
    pub language: String,
    pub load_on_start: bool,
    pub export_format: ExportFormat,
    /// Picking a record makes a `key_<EditorID>` FormIDs entry.
    pub auto_keys: bool,
    pub recent: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            use_mo2: true,
            mo2_dir: String::new(),
            profile: String::new(),
            data_dir: String::new(),
            output_dir: dir().join("Output").join("SDS Projectile Studio Output").display().to_string(),
            language: "ru".into(),
            load_on_start: true,
            export_format: ExportFormat::Auto,
            auto_keys: true,
            recent: Vec::new(),
        }
    }
}

pub fn dir() -> PathBuf {
    std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())).unwrap_or_else(|| PathBuf::from("."))
}

fn path() -> PathBuf {
    dir().join("sds_projectile_studio.json")
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct AnimStudio {
    use_mo2: bool,
    mo2_dir: String,
    profile: String,
    data_dir: String,
    language: String,
}

impl Settings {
    pub fn load() -> Settings {
        if let Some(s) = std::fs::read(path()).ok().and_then(|b| serde_json::from_slice(&b).ok()) {
            return s;
        }
        let mut s = Settings::default();
        if let Some(a) = std::fs::read(dir().join("sds_anim_studio.json")).ok().and_then(|b| serde_json::from_slice::<AnimStudio>(&b).ok()) {
            s.use_mo2 = a.use_mo2 || !a.mo2_dir.is_empty();
            s.mo2_dir = a.mo2_dir;
            s.profile = a.profile;
            s.data_dir = a.data_dir;
            if !a.language.is_empty() {
                s.language = a.language;
            }
        }
        s
    }

    pub fn save(&self) {
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path(), s);
        }
    }

    pub fn apply_language(&self) {
        sds_i18n::set(if self.language.eq_ignore_ascii_case("en") { sds_i18n::Lang::En } else { sds_i18n::Lang::Ru });
    }

    pub fn source(&self) -> Option<sds_vfs::Source> {
        if self.use_mo2 {
            (!self.mo2_dir.is_empty() && !self.profile.is_empty()).then(|| sds_vfs::Source::Mo2 { instance: self.mo2_dir.clone().into(), profile: self.profile.clone() })
        } else {
            (!self.data_dir.is_empty()).then(|| sds_vfs::Source::DataDir(self.data_dir.clone().into()))
        }
    }

    pub fn out_dir(&self) -> PathBuf {
        PathBuf::from(&self.output_dir).join("HomingProjectiles")
    }
}
