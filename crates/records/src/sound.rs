//! What a sound event plays: SNDR (sound descriptor) or SOUN (sound
//! marker → its SNDR) by EditorID, the way `SoundPlay.<EDID>` names it.
//!
//! SNDR (SSE): ANAM file paths under `Data\Sound\` (one picked at random),
//! GNAM category (SNCT), LNAM looping byte at offset 1 (0x08 loop,
//! 0x10/0x20 envelope), BNAM = frequency shift i8 %, frequency variance
//! i8 %, priority u8, dB variance u8, static attenuation u16 (dB × 100).
//! SNCT: PNAM parent category, VNAM static volume multiplier u16 / 65535.
//! MGEF SNDD: (type u32, SNDR) pairs — 0 draw/sheathe, 1 charge, 2 ready,
//! 3 release, 4 cast loop, 5 on hit.

use crate::{u32_at, zstr, Db, FormKey};
use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct SoundDesc {
    pub key: Option<FormKey>,
    pub edid: String,
    /// `sound/...` paths as written (the player falls back to .xwm / .fuz).
    pub files: Vec<String>,
    pub category: String,
    /// Category chain volume, 0..1.
    pub category_volume: f32,
    /// BNAM static attenuation, dB (positive = quieter).
    pub attenuation_db: f32,
    pub db_variance: f32,
    /// Percent.
    pub freq_shift: f32,
    pub freq_variance: f32,
    pub looping: bool,
    pub notes: Vec<String>,
}

impl SoundDesc {
    /// Linear gain of the static part (attenuation and category).
    pub fn gain(&self) -> f32 {
        10f32.powf(-self.attenuation_db / 20.0) * self.category_volume
    }
}

/// MGEF sound slot names.
pub fn mgef_sound_slot(kind: u32) -> &'static str {
    match kind {
        0 => sds_i18n::t!("достать/убрать", "draw/sheathe"),
        1 => sds_i18n::t!("заряд", "charge"),
        2 => sds_i18n::t!("готов", "ready"),
        3 => sds_i18n::t!("выпуск", "release"),
        4 => sds_i18n::t!("цикл каста", "cast loop"),
        5 => sds_i18n::t!("попадание", "hit"),
        _ => "?",
    }
}

fn sound_path(p: &str) -> String {
    let p = p.replace('\\', "/");
    let mut p = p.trim_start_matches('/');
    // Some descriptors spell the path from Data (`Data\Sound\fx\...`).
    if p.len() > 5 && p[..5].eq_ignore_ascii_case("data/") {
        p = &p[5..];
    }
    if p.to_ascii_lowercase().starts_with("sound/") {
        p.to_string()
    } else {
        format!("sound/{p}")
    }
}

impl Db {
    /// EditorID (lower case) → SNDR or SOUN, for `SoundPlay.<EDID>`.
    pub fn sound_index(&self) -> HashMap<String, FormKey> {
        let mut out = HashMap::new();
        for sig in [b"SOUN", b"SNDR"] {
            for k in self.by_sig.get(sig).into_iter().flatten() {
                if let Some(r) = self.get(*k) {
                    let e = r.edid();
                    if !e.is_empty() {
                        // SNDR wins a name clash: it goes in last.
                        out.insert(e.to_ascii_lowercase(), *k);
                    }
                }
            }
        }
        out
    }

    /// A SNDR (or SOUN, followed to its SNDR) as something playable.
    pub fn sound_desc(&self, k: FormKey) -> Option<SoundDesc> {
        let mut rec = self.get(k)?;
        let mut notes = Vec::new();
        if &rec.sig == b"SOUN" {
            let soun = rec;
            match soun.first(b"SDSC").and_then(|d| u32_at(d, 0)).and_then(|raw| self.link(soun, raw)).and_then(|sk| self.get(sk)) {
                Some(r) if &r.sig == b"SNDR" => {
                    notes.push(format!("SOUN {} → SNDR {}", soun.edid(), r.edid()));
                    rec = r;
                }
                _ => {
                    notes.push(sds_i18n::tf!("SOUN {} без дескриптора", "SOUN {} without a descriptor", soun.edid()));
                    return Some(SoundDesc { key: Some(k), edid: soun.edid(), notes, category_volume: 1.0, ..Default::default() });
                }
            }
        }
        if &rec.sig != b"SNDR" {
            return None;
        }
        let mut d = SoundDesc { key: Some(k), edid: rec.edid(), category_volume: 1.0, notes, ..Default::default() };
        for (s, data) in rec.subs() {
            match s {
                b"ANAM" => {
                    let p = zstr(data);
                    if !p.is_empty() {
                        d.files.push(sound_path(&p));
                    }
                }
                b"LNAM" if data.len() >= 2 => d.looping = data[1] & 0x08 != 0,
                b"BNAM" if data.len() >= 6 => {
                    d.freq_shift = data[0] as i8 as f32;
                    d.freq_variance = data[1] as i8 as f32;
                    d.db_variance = data[3] as f32;
                    d.attenuation_db = u16::from_le_bytes([data[4], data[5]]) as f32 / 100.0;
                }
                b"GNAM" => {
                    // Category chain: multiply static volumes up to the root.
                    let mut cat = u32_at(data, 0).and_then(|raw| self.link(rec, raw));
                    let mut depth = 0;
                    while let Some(ck) = cat {
                        let Some(c) = self.get(ck).filter(|r| &r.sig == b"SNCT") else { break };
                        if depth == 0 {
                            d.category = c.edid();
                        }
                        if let Some(v) = c.first(b"VNAM").filter(|v| v.len() >= 2) {
                            d.category_volume *= u16::from_le_bytes([v[0], v[1]]) as f32 / 65535.0;
                        }
                        cat = c.first(b"PNAM").and_then(|p| u32_at(p, 0)).and_then(|raw| self.link(c, raw));
                        depth += 1;
                        if depth > 8 {
                            break;
                        }
                    }
                }
                _ => {}
            }
        }
        if d.files.is_empty() {
            d.notes.push(sds_i18n::t!("у дескриптора нет файлов (составной или пустой)", "the descriptor has no files (compound or empty)").into());
        }
        Some(d)
    }

    /// MGEF SNDD sounds of an effect: (slot, SNDR).
    pub fn effect_sounds(&self, mgef: FormKey) -> Vec<(u32, FormKey)> {
        let Some(m) = self.get(mgef).filter(|r| &r.sig == b"MGEF") else { return Vec::new() };
        let Some(d) = m.first(b"SNDD") else { return Vec::new() };
        d.chunks_exact(8)
            .filter_map(|c| {
                let kind = u32::from_le_bytes(c[..4].try_into().unwrap());
                let raw = u32::from_le_bytes(c[4..].try_into().unwrap());
                (raw != 0).then(|| self.link(m, raw)).flatten().map(|k| (kind, k))
            })
            .collect()
    }
}

/// What a footstep event resolves through, for the inspector.
#[derive(Debug, Clone)]
pub struct FootstepTrace {
    pub sound: FormKey,
    /// "FSTS → FSTP → IPDS → MATT → IPCT".
    pub chain: String,
}

impl Db {
    /// The swing whoosh of a weapon: WEAP TNAM ("attack fail" sound in the
    /// editor; the game plays it on `weaponSwing`).
    pub fn weapon_swing(&self, weap: FormKey) -> Option<FormKey> {
        let w = self.get(weap).filter(|r| &r.sig == b"WEAP")?;
        w.first(b"TNAM").and_then(|d| u32_at(d, 0)).filter(|&raw| raw != 0).and_then(|raw| self.link(w, raw))
    }

    /// The footstep set an ARMA gives its wearer (SNDD).
    pub fn arma_footsteps(&self, arma: FormKey) -> Option<FormKey> {
        let a = self.get(arma).filter(|r| &r.sig == b"ARMA")?;
        a.first(b"SNDD").and_then(|d| u32_at(d, 0)).filter(|&raw| raw != 0).and_then(|raw| self.link(a, raw))
    }

    /// A record of `sig` by EditorID.
    pub fn by_edid(&self, sig: &[u8; 4], edid: &str) -> Option<FormKey> {
        self.by_sig.get(sig)?.iter().copied().find(|k| self.get(*k).is_some_and(|r| r.edid().eq_ignore_ascii_case(edid)))
    }

    /// Every MATT (EditorID, key), sorted: the ground choices.
    pub fn materials(&self) -> Vec<(String, FormKey)> {
        let mut v: Vec<(String, FormKey)> = self.by_sig.get(b"MATT").into_iter().flatten().filter_map(|k| self.get(*k).map(|r| (r.edid(), *k))).collect();
        v.sort();
        v
    }

    /// The sound a footstep tag (`FootLeft`, `FootScuffRight`, …) plays from
    /// footstep set `fsts` on `ground`: the first FSTP of the set with that
    /// tag (walk group first), its impact set's entry for the ground material
    /// or its nearest parent material, that impact's sound.
    pub fn footstep_sound(&self, fsts: FormKey, tag: &str, ground: Option<FormKey>) -> Option<FootstepTrace> {
        let set = self.get(fsts).filter(|r| &r.sig == b"FSTS")?;
        let list = set.first(b"DATA")?;
        let fstp = list
            .chunks_exact(4)
            .filter_map(|c| self.link(set, u32::from_le_bytes(c.try_into().unwrap())))
            .filter_map(|k| self.get(k).filter(|r| &r.sig == b"FSTP").map(|r| (k, r)))
            .find(|(_, r)| r.first(b"ANAM").map(zstr).is_some_and(|t| t.eq_ignore_ascii_case(tag)))?;
        let ipds_key = fstp.1.first(b"DATA").and_then(|d| u32_at(d, 0)).and_then(|raw| self.link(fstp.1, raw))?;
        let ipds = self.get(ipds_key)?;
        let pairs: Vec<(Option<FormKey>, Option<FormKey>)> = ipds
            .subs()
            .filter(|(s, d)| *s == b"PNAM" && d.len() == 8)
            .map(|(_, d)| (self.link(ipds, u32_at(d, 0).unwrap()), self.link(ipds, u32_at(d, 4).unwrap())))
            .collect();
        // The ground, then its parents.
        let mut chain_mats = Vec::new();
        let mut m = ground;
        while let Some(k) = m {
            if chain_mats.contains(&k) || chain_mats.len() > 8 {
                break;
            }
            chain_mats.push(k);
            m = self.get(k).and_then(|r| r.first(b"PNAM").and_then(|d| u32_at(d, 0)).and_then(|raw| self.link(r, raw)));
        }
        let (mat, ipct) = chain_mats
            .iter()
            .find_map(|mk| pairs.iter().find(|(pm, _)| *pm == Some(*mk)).map(|(pm, pi)| (*pm, *pi)))
            .or_else(|| pairs.first().copied())?;
        let ipct_key = ipct?;
        let ipct = self.get(ipct_key)?;
        let sound = ipct.first(b"SNAM").and_then(|d| u32_at(d, 0)).and_then(|raw| self.link(ipct, raw))?;
        let ed = |k: Option<FormKey>| k.and_then(|k| self.get(k)).map(|r| r.edid()).unwrap_or_else(|| "?".into());
        Some(FootstepTrace {
            sound,
            chain: format!("{} → {} → {} → {} → {}", set.edid(), fstp.1.edid(), ipds.edid(), ed(mat), ipct.edid()),
        })
    }
}
