//! `sds_anim_studio.json` next to the exe.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TimeUnit {
    #[default]
    Frames,
    Seconds,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// true: build the Data view from an MO2 profile; false: from `data_dir`.
    pub use_mo2: bool,
    pub mo2_dir: String,
    pub profile: String,
    pub data_dir: String,
    /// Output mod folder: edited clips and anno.txt exports go here, keeping
    /// their Data-relative path. Never the source mod.
    pub output_dir: String,
    pub time_unit: TimeUnit,
    pub snap_to_frames: bool,
    pub load_on_start: bool,
    pub recent: Vec<String>,
    pub recent_projects: Vec<String>,
    /// Sound simulation on/off and master volume, 0..100 %.
    pub sound_on: bool,
    pub volume: f32,
    /// UI language: "ru" or "en".
    pub language: String,
    /// Ground material (MATT EditorID) footsteps sound on.
    pub ground: String,
    /// Favorite annotations of the add menus.
    pub favorites: Vec<String>,
    /// Numbers of animmotion / animrotation under the root-motion row.
    pub motion_values: bool,
}

impl Settings {
    /// Makes `language` the UI language.
    pub fn apply_language(&self) {
        sds_i18n::set(if self.language.eq_ignore_ascii_case("en") { sds_i18n::Lang::En } else { sds_i18n::Lang::Ru });
    }

    /// The slider is in perceived loudness: square it for amplitude.
    pub fn volume_linear(&self) -> f32 {
        let v = (self.volume / 100.0).clamp(0.0, 1.0);
        v * v
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            use_mo2: true,
            // Fresh install: pick the MO2 instance (or a Data folder) in the Library tab.
            mo2_dir: String::new(),
            profile: String::new(),
            data_dir: String::new(),
            output_dir: dir().join("Output").join("SDS Anim Studio Output").display().to_string(),
            time_unit: TimeUnit::Frames,
            snap_to_frames: true,
            load_on_start: false,
            recent: Vec::new(),
            recent_projects: Vec::new(),
            sound_on: true,
            volume: 80.0,
            language: "ru".into(),
            ground: "MaterialStone".into(),
            favorites: Vec::new(),
            motion_values: true,
        }
    }
}

pub fn dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn path() -> PathBuf {
    dir().join("sds_anim_studio.json")
}

pub fn layout_path() -> PathBuf {
    dir().join("sds_anim_studio_layout.json")
}

impl Settings {
    pub fn load() -> Settings {
        std::fs::read(path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path(), s);
        }
    }

    pub fn push_recent(&mut self, item: String) {
        self.recent.retain(|r| *r != item);
        self.recent.insert(0, item);
        self.recent.truncate(15);
    }
}
