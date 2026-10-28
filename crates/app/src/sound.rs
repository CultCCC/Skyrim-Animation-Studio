//! Sound simulation: while the clip plays, every sound event the playhead
//! passes is played the way the game would — `SoundPlay.<EDID>` annotations
//! (SNDR / SOUN by EditorID) and the effect sounds of PIE spells (MGEF
//! SNDD: release on the cast, hit when cast on self). The descriptor picks
//! a random file and adds its attenuation, category volume, dB and pitch
//! variance; playback speed follows the timeline. Scrubbing is silent.

use crate::app::{Loaded, StudioApp};
use crate::audio::{Audio, Clip};
use sds_records::sound::SoundDesc;
use sds_records::FormKey;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// One sound the clip triggers.
pub struct SoundEvent {
    pub time: f32,
    pub key: Option<FormKey>,
    pub edid: String,
    /// Why an engine event (swing, footstep) has no sound.
    pub note: Option<String>,
}

/// The sound the ENGINE plays for an animation event: `weaponSwing` /
/// `weaponLeftSwing` → the weapon's swing sound (WEAP TNAM; unarmed:
/// WPNSwingUnarmed), `Foot*` → the feet armor's footstep set on the chosen
/// ground. None when the event makes no engine sound; Err says why not.
pub fn engine_sound(app: &StudioApp, text: &str) -> Option<Result<(FormKey, String), String>> {
    let l = app.loaded.as_ref()?;
    let db = &l.db;
    let t = text.trim();
    let lower = t.to_ascii_lowercase();
    if lower == "weaponswing" || lower == "weaponleftswing" {
        let weapon = app.npc.as_ref().and_then(|n| n.weapon.and_then(|w| n.look.weapons.get(w)));
        return Some(match weapon {
            Some(w) => match db.weapon_swing(w.key) {
                Some(k) => Ok((k, sds_i18n::tf!("взмах оружия {} (WEAP TNAM)", "swing of {} (WEAP TNAM)", w.edid))),
                None => Err(sds_i18n::tf!("у оружия {} нет звука взмаха (TNAM)", "weapon {} has no swing sound (TNAM)", w.edid)),
            },
            None => match db.by_edid(b"SNDR", "WPNSwingUnarmed") {
                Some(k) => Ok((k, sds_i18n::t!("без оружия: WPNSwingUnarmed", "unarmed: WPNSwingUnarmed").into())),
                None => Err("WPNSwingUnarmed".into()),
            },
        });
    }
    if !lower.starts_with("foot") {
        return None;
    }
    // Footstep set: the worn feet piece (slot 37), else the barefoot set.
    const FEET: u32 = 1 << 7;
    let from_npc = app.npc.as_ref().and_then(|n| {
        let mut parts: Vec<&sds_records::npc::Part> = n.look.parts.iter().filter(|p| p.slots & FEET != 0).collect();
        parts.sort_by_key(|p| p.kind != sds_records::npc::PartKind::Armor);
        parts.iter().find_map(|p| p.arma.and_then(|a| db.arma_footsteps(a)))
    });
    let fsts = from_npc.or_else(|| db.by_edid(b"FSTS", "FSTBarefootFootstepSet")).or_else(|| db.by_edid(b"FSTS", "DefaultFootstepSet"));
    let Some(fsts) = fsts else { return Some(Err(sds_i18n::t!("нет набора шагов (FSTS)", "no footstep set (FSTS)").into())) };
    let ground = db.by_edid(b"MATT", &app.settings.ground);
    Some(match db.footstep_sound(fsts, t, ground) {
        Some(tr) => Ok((tr.sound, tr.chain)),
        None => Err(sds_i18n::tf!("{}: в наборе шагов нет такого тега", "{}: the footstep set has no such tag", t)),
    })
}

#[derive(Default)]
pub struct SoundSim {
    audio: Option<Result<Audio, String>>,
    /// (Db identity, EditorID → SNDR/SOUN).
    index: Option<(usize, HashMap<String, FormKey>)>,
    descs: HashMap<FormKey, Option<SoundDesc>>,
    clips: HashMap<String, Option<Arc<Clip>>>,
    warned: HashSet<String>,
    rng: u32,
    was_playing: bool,
    volume: Option<f32>,
    speed: Option<f32>,
}

impl SoundSim {
    fn audio(&mut self, volume: f32) -> Option<&Audio> {
        let a = self.audio.get_or_insert_with(|| Audio::start(volume));
        a.as_ref().ok()
    }

    /// Why there is no sound, once the engine was tried.
    pub fn error(&self) -> Option<&str> {
        self.audio.as_ref().and_then(|a| a.as_ref().err()).map(|s| s.as_str())
    }

    fn rand(&mut self) -> f32 {
        // xorshift: variance only, no need for more.
        let mut x = if self.rng == 0 { 0x9E37_79B9 } else { self.rng };
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x >> 8) as f32 / (1u32 << 24) as f32
    }

    pub fn lookup(&mut self, l: &Loaded, edid: &str) -> Option<FormKey> {
        let id = Arc::as_ptr(&l.db) as usize;
        if self.index.as_ref().is_none_or(|(i, _)| *i != id) {
            self.index = Some((id, l.db.sound_index()));
            self.descs.clear();
        }
        self.index.as_ref().and_then(|(_, m)| m.get(&edid.trim().to_ascii_lowercase()).copied())
    }

    pub fn describe(&mut self, l: &Loaded, key: FormKey) -> Option<SoundDesc> {
        self.descs.entry(key).or_insert_with(|| l.db.sound_desc(key)).clone()
    }

    /// The file as the game finds it: as named, else `.xwm`, else `.fuz`.
    fn clip(&mut self, l: &Loaded, path: &str) -> Result<Arc<Clip>, String> {
        if let Some(c) = self.clips.get(path) {
            return c.clone().ok_or_else(|| sds_i18n::tf!("{path}: не читается", "{path}: unreadable"));
        }
        let stem = path.rsplit_once('.').map(|(s, _)| s).unwrap_or(path);
        let mut result = Err(sds_i18n::tf!("{path}: файла нет в сборке (и .xwm / .fuz тоже)", "{path}: no such file in the build (nor .xwm / .fuz)"));
        for cand in [path.to_string(), format!("{stem}.xwm"), format!("{stem}.fuz")] {
            if let Ok(bytes) = l.vfs.read(&cand) {
                result = crate::audio::parse(&bytes).map(Arc::new).map_err(|e| format!("{cand}: {e}"));
                break;
            }
        }
        self.clips.insert(path.to_string(), result.as_ref().ok().cloned());
        result
    }

    /// Plays one descriptor now. `Err` says why not.
    pub fn play(&mut self, l: &Loaded, key: FormKey, volume: f32, speed: f32) -> Result<String, String> {
        let d = self.describe(l, key).ok_or(sds_i18n::t!("не SNDR / SOUN", "not an SNDR / SOUN"))?;
        if d.files.is_empty() {
            return Err(format!("{}: {}", d.edid, d.notes.join("; ")));
        }
        let pick = ((self.rand() * d.files.len() as f32) as usize).min(d.files.len() - 1);
        let path = d.files[pick].clone();
        let clip = self.clip(l, &path)?;
        let db = self.rand() * d.db_variance;
        let gain = d.gain() * 10f32.powf(-db / 20.0);
        let var = (self.rand() * 2.0 - 1.0) * d.freq_variance;
        let ratio = ((1.0 + d.freq_shift / 100.0) * (1.0 + var / 100.0)).max(0.05);
        self.audio(volume);
        let audio = match &self.audio {
            Some(Ok(a)) => a,
            Some(Err(e)) => return Err(e.clone()),
            None => return Err(sds_i18n::t!("нет звука", "no sound").into()),
        };
        audio.set_speed(speed);
        audio.play(clip.clone(), gain, ratio, d.looping);
        Ok(sds_i18n::tf!("{} → {} ({}, {:.2} с)", "{} → {} ({}, {:.2} s)", d.edid, path, clip.format, clip.seconds))
    }

    pub fn stop(&mut self) {
        if let Some(Ok(a)) = &self.audio {
            a.stop_all();
        }
    }

    fn sync(&mut self, volume: f32, speed: f32) {
        if let Some(Ok(a)) = &self.audio {
            if self.volume != Some(volume) {
                a.set_volume(volume);
                self.volume = Some(volume);
            }
            if self.speed != Some(speed) {
                a.set_speed(speed);
                self.speed = Some(speed);
            }
        }
    }
}

/// Every sound event of the open clip.
pub fn events(app: &StudioApp) -> Vec<SoundEvent> {
    let (Some(doc), Some(l)) = (&app.doc, &app.loaded) else { return Vec::new() };
    let mut out = Vec::new();
    for a in &doc.annos {
        if let sds_anno::Kind::Sound { descriptor } = sds_anno::parse(&a.text).kind {
            if !descriptor.trim().is_empty() {
                out.push(SoundEvent { time: a.time, key: None, edid: descriptor.trim().to_string(), note: None });
            }
            continue;
        }
        match engine_sound(app, &a.text) {
            Some(Ok((k, how))) => out.push(SoundEvent { time: a.time, key: Some(k), edid: how, note: None }),
            Some(Err(why)) => out.push(SoundEvent { time: a.time, key: None, edid: a.text.clone(), note: Some(why) }),
            None => {}
        }
    }
    let items: Vec<(f32, &str)> = doc.annos.iter().map(|a| (a.time, a.text.as_str())).collect();
    for cue in crate::pie::resolve(&items, doc.duration(), &l.pie, Some(&l.db)) {
        for s in cue.sounds {
            out.push(SoundEvent { time: s.time, key: Some(s.sound), edid: s.label, note: None });
        }
    }
    out
}

impl StudioApp {
    /// After the playhead moved from `prev` to `now` (`wrapped`: through the
    /// loop point) during playback.
    pub fn sound_tick(&mut self, prev: f32, now: f32, wrapped: bool, duration: f32) {
        let mut sim = std::mem::take(&mut self.sound);
        let (volume, speed) = (self.settings.volume_linear(), self.play.speed);
        sim.sync(volume, speed);
        // A jump (the ruler dragged while playing) is a seek, not playback.
        let seek = !wrapped && (now < prev || now - prev > 0.25);
        if self.settings.sound_on && self.loaded.is_some() && !seek {
            let fresh = !sim.was_playing;
            let hit = |t: f32| {
                let after = if fresh { t >= prev } else { t > prev };
                if wrapped {
                    after && t <= duration || t <= now
                } else {
                    after && t <= now
                }
            };
            let due: Vec<SoundEvent> = events(self).into_iter().filter(|e| hit(e.time)).collect();
            if wrapped {
                if let Some(Ok(a)) = &sim.audio {
                    a.stop_loops();
                }
            }
            for e in due {
                let l = self.loaded.as_ref().unwrap();
                let key = e.key.or_else(|| sim.lookup(l, &e.edid));
                let r = match (key, &e.note) {
                    (_, Some(why)) => Err(format!("{}: {why}", e.edid)),
                    (Some(k), None) => sim.play(l, k, volume, speed),
                    (None, None) => Err(sds_i18n::tf!("{}: нет SNDR / SOUN с таким EditorID", "{}: no SNDR / SOUN with this EditorID", e.edid)),
                };
                if let Err(msg) = r {
                    if sim.warned.insert(msg.clone()) {
                        self.log(sds_i18n::tf!("Звук: {msg}", "Sound: {msg}"));
                    }
                }
            }
            if let Some(msg) = sim.error().map(str::to_string) {
                if sim.warned.insert(msg.clone()) {
                    self.log(sds_i18n::tf!("Звук отключён: {msg}", "Sound disabled: {msg}"));
                }
            }
        }
        sim.was_playing = true;
        self.sound = sim;
    }

    /// The clip ran out without looping: let the tails ring.
    pub fn sound_ended(&mut self) {
        self.sound.was_playing = false;
    }

    /// Playback stopped: silence, like pausing a video.
    pub fn sound_pause(&mut self) {
        if self.sound.was_playing {
            self.sound.stop();
            self.sound.was_playing = false;
        }
        let (v, s) = (self.settings.volume_linear(), self.play.speed);
        self.sound.sync(v, s);
    }

    /// The inspector's "listen" button.
    pub fn preview_sound(&mut self, edid: &str, key: Option<FormKey>) {
        let mut sim = std::mem::take(&mut self.sound);
        let msg = match &self.loaded {
            Some(l) => match key.or_else(|| sim.lookup(l, edid)) {
                Some(k) => sim.play(l, k, self.settings.volume_linear(), 1.0),
                None => Err(sds_i18n::tf!("{edid}: нет SNDR / SOUN с таким EditorID", "{edid}: no SNDR / SOUN with this EditorID")),
            },
            None => Err(sds_i18n::t!("сборка не загружена", "build not loaded").into()),
        };
        self.sound = sim;
        match msg {
            Ok(m) => self.log(format!("▶ {m}")),
            Err(m) => self.log(sds_i18n::tf!("Звук: {m}", "Sound: {m}")),
        }
    }

    /// Descriptor details for the inspector.
    pub fn sound_info(&mut self, edid: &str) -> Option<SoundDesc> {
        let l = self.loaded.as_ref()?;
        let k = self.sound.lookup(l, edid)?;
        self.sound.describe(l, k)
    }
}
