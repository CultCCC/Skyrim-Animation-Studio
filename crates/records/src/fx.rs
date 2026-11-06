//! What a spell looks like: SPEL/ENCH → effects (EFID + EFIT) → MGEF art.
//!
//! MGEF DATA offsets (SSE, checked on SDS and vanilla records): 32 hit
//! shader (EFSH), 36 enchant shader (EFSH), 92 casting art (ARTO), 96 hit
//! effect art (ARTO), 116 enchant art (ARTO). ARTO keeps its model in MODL.
//! EFIT = magnitude f32, area u32, duration u32.
//! MGEF DATA 72 projectile (PROJ), 76 explosion (EXPL), 84 delivery
//! (0 self, 1 touch, 2 aimed, 3 target actor, 4 target location).
//! PROJ DATA: 2 type u16, 4 gravity, 8 speed, 12 range, 36 explosion,
//! 40 flight sound, 72 collision radius, 76 lifetime; model in MODL.
//! EXPL DATA: 4 sound, 8 sound (no-AV variant), 32 radius; model in MODL.

use crate::{f32_at, u32_at, zstr, Db, FormKey, Rec};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtSlot {
    /// On the casting hand while casting.
    Casting,
    /// On whoever the effect lands on (the caster for self spells).
    Hit,
    /// On the enchanted weapon.
    Enchant,
}

impl ArtSlot {
    pub fn label(self) -> &'static str {
        match self {
            ArtSlot::Casting => sds_i18n::t!("арт каста", "casting art"),
            ArtSlot::Hit => sds_i18n::t!("арт эффекта", "effect art"),
            ArtSlot::Enchant => sds_i18n::t!("арт зачарования", "enchantment art"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Visual {
    pub effect: String,
    pub slot: ArtSlot,
    pub art: String,
    /// `meshes/...`
    pub model: String,
    /// EFIT duration, seconds (0 = instant).
    pub duration: f32,
}

#[derive(Debug, Clone)]
pub struct SpellVisuals {
    pub key: FormKey,
    pub sig: String,
    pub edid: String,
    pub name: String,
    pub visuals: Vec<Visual>,
    /// Effects with no art at all, by name.
    pub silent: Vec<String>,
    /// MGEF sounds: (effect, SNDD slot, SNDR).
    pub sounds: Vec<(String, u32, FormKey)>,
    /// What the spell launches: the first effect with a projectile.
    pub projectile: Option<SpellProjectile>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjKind {
    Missile,
    Lobber,
    Beam,
    Flame,
    Cone,
    Barrier,
    Arrow,
}

impl ProjKind {
    fn from_raw(v: u16) -> ProjKind {
        match v {
            2 => ProjKind::Lobber,
            4 => ProjKind::Beam,
            8 => ProjKind::Flame,
            0x10 => ProjKind::Cone,
            0x20 => ProjKind::Barrier,
            0x40 => ProjKind::Arrow,
            _ => ProjKind::Missile,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ProjKind::Missile => "Missile",
            ProjKind::Lobber => "Lobber",
            ProjKind::Beam => "Beam",
            ProjKind::Flame => "Flame",
            ProjKind::Cone => "Cone",
            ProjKind::Barrier => "Barrier",
            ProjKind::Arrow => "Arrow",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Projectile {
    pub key: FormKey,
    pub edid: String,
    /// `meshes/...`, empty when it has none.
    pub model: String,
    pub kind: ProjKind,
    /// Multiplier of world gravity (lobbers).
    pub gravity: f32,
    /// Units per second.
    pub speed: f32,
    pub range: f32,
    /// Seconds, 0 = unlimited.
    pub lifetime: f32,
    /// Looping flight sound.
    pub sound: Option<FormKey>,
    pub explosion: Option<Explosion>,
}

#[derive(Debug, Clone)]
pub struct Explosion {
    pub key: FormKey,
    pub edid: String,
    pub model: String,
    pub sound: Option<FormKey>,
    pub radius: f32,
}

#[derive(Debug, Clone)]
pub struct SpellProjectile {
    pub effect: String,
    /// MGEF delivery (2 aimed, 4 target location, ...).
    pub delivery: u32,
    pub projectile: Option<Projectile>,
    /// The MGEF's explosion, else the projectile's own.
    pub explosion: Option<Explosion>,
}

/// What Precision's trail definitions match a weapon by.
#[derive(Debug, Clone, Default)]
pub struct WeaponFacts {
    pub name: String,
    pub keywords: Vec<String>,
    pub enchantment: String,
    pub effect_name: String,
    pub effect_keywords: Vec<String>,
    /// The costliest effect's enchant shader (EFSH), as (plugin, local id).
    pub effect_shader: Option<(String, u32)>,
}

fn meshes_path(p: &str) -> String {
    let p = p.replace('\\', "/");
    let p = p.trim_start_matches('/');
    if p.to_ascii_lowercase().starts_with("meshes/") {
        p.to_string()
    } else {
        format!("meshes/{p}")
    }
}

impl Db {
    /// A form by its defining plugin and local id (`0x494a2d|Rebalance.esp`).
    pub fn find(&self, plugin: &str, id: u32) -> Option<FormKey> {
        let module = self.plugins.iter().position(|p| p.name.eq_ignore_ascii_case(plugin.trim()))? as u16;
        let k = FormKey { module, id: id & 0x00FF_FFFF };
        self.get(k).map(|_| k)
    }

    fn arto_model(&self, mgef: &Rec, raw: u32) -> Option<(String, String)> {
        let k = self.link(mgef, raw)?;
        let art = self.get(k).filter(|r| &r.sig == b"ARTO")?;
        let model = art.first(b"MODL").map(zstr).filter(|m| !m.is_empty())?;
        Some((art.edid(), meshes_path(&model)))
    }

    fn keywords(&self, rec: &Rec) -> Vec<String> {
        rec.first(b"KWDA")
            .map(|d| d.chunks_exact(4).filter_map(|c| self.link(rec, u32::from_le_bytes(c.try_into().unwrap()))).filter_map(|k| self.get(k)).map(|r| r.edid()).collect())
            .unwrap_or_default()
    }

    /// Effects of a SPEL or ENCH, with their art.
    pub fn spell_visuals(&self, k: FormKey) -> Option<SpellVisuals> {
        let spel = self.get(k).filter(|r| &r.sig == b"SPEL" || &r.sig == b"ENCH")?;
        let mut out = SpellVisuals {
            key: k,
            sig: String::from_utf8_lossy(&spel.sig).into_owned(),
            edid: spel.edid(),
            name: self.name(spel),
            visuals: Vec::new(),
            silent: Vec::new(),
            sounds: Vec::new(),
            projectile: None,
        };
        let mut current: Option<FormKey> = None;
        let push = |mk: FormKey, duration: f32, out: &mut SpellVisuals| {
            let Some(m) = self.get(mk).filter(|r| &r.sig == b"MGEF") else { return };
            let Some(data) = m.first(b"DATA") else { return };
            let effect = format!("{} «{}»", m.edid(), self.name(m));
            if out.projectile.is_none() {
                let proj = u32_at(data, 72).and_then(|raw| self.link(m, raw)).and_then(|k| self.projectile(k));
                if let Some(p) = proj {
                    let own = u32_at(data, 76).and_then(|raw| self.link(m, raw)).and_then(|k| self.explosion(k));
                    let explosion = own.or_else(|| p.explosion.clone());
                    out.projectile = Some(SpellProjectile { effect: effect.clone(), delivery: u32_at(data, 84).unwrap_or(0), projectile: Some(p), explosion });
                }
            }
            for (kind, sk) in self.effect_sounds(mk) {
                out.sounds.push((effect.clone(), kind, sk));
            }
            let before = out.visuals.len();
            for (off, slot) in [(92, ArtSlot::Casting), (96, ArtSlot::Hit), (116, ArtSlot::Enchant)] {
                if let Some((art, model)) = u32_at(data, off).and_then(|raw| self.arto_model(m, raw)) {
                    out.visuals.push(Visual { effect: effect.clone(), slot, art, model, duration });
                }
            }
            if out.visuals.len() == before {
                out.silent.push(effect);
            }
        };
        for (s, d) in spel.subs() {
            match s {
                b"EFID" => {
                    if let Some(prev) = current.take() {
                        push(prev, 0.0, &mut out);
                    }
                    current = u32_at(d, 0).and_then(|raw| self.link(spel, raw));
                }
                b"EFIT" => {
                    if let Some(mk) = current.take() {
                        let dur = u32_at(d, 8).unwrap_or(0) as f32;
                        push(mk, dur, &mut out);
                    }
                }
                _ => {}
            }
        }
        if let Some(prev) = current {
            push(prev, 0.0, &mut out);
        }
        Some(out)
    }

    /// A PROJ record with its explosion.
    pub fn projectile(&self, k: FormKey) -> Option<Projectile> {
        let p = self.get(k).filter(|r| &r.sig == b"PROJ")?;
        let d = p.first(b"DATA")?;
        let kind = d.get(2..4).map(|b| u16::from_le_bytes([b[0], b[1]])).unwrap_or(1);
        let model = p.first(b"MODL").map(zstr).filter(|m| !m.is_empty()).map(|m| meshes_path(&m)).unwrap_or_default();
        Some(Projectile {
            key: k,
            edid: p.edid(),
            model,
            kind: ProjKind::from_raw(kind),
            gravity: f32_at(d, 4).unwrap_or(0.0),
            speed: f32_at(d, 8).unwrap_or(0.0),
            range: f32_at(d, 12).unwrap_or(0.0),
            lifetime: f32_at(d, 76).unwrap_or(0.0),
            sound: u32_at(d, 40).and_then(|raw| self.link(p, raw)),
            explosion: u32_at(d, 36).and_then(|raw| self.link(p, raw)).and_then(|e| self.explosion(e)),
        })
    }

    /// An EXPL record.
    pub fn explosion(&self, k: FormKey) -> Option<Explosion> {
        let e = self.get(k).filter(|r| &r.sig == b"EXPL")?;
        let d = e.first(b"DATA").unwrap_or(&[]);
        let model = e.first(b"MODL").map(zstr).filter(|m| !m.is_empty()).map(|m| meshes_path(&m)).unwrap_or_default();
        let sound = [4, 8].into_iter().find_map(|o| u32_at(d, o).and_then(|raw| self.link(e, raw)));
        Some(Explosion { key: k, edid: e.edid(), model, sound, radius: f32_at(d, 32).unwrap_or(0.0) })
    }

    /// Name, keywords and enchantment facts of a weapon.
    pub fn weapon_facts(&self, k: FormKey) -> WeaponFacts {
        let Some(w) = self.get(k) else { return WeaponFacts::default() };
        let mut f = WeaponFacts { name: self.name(w), keywords: self.keywords(w), ..Default::default() };
        let ench = w.first(b"EITM").and_then(|d| u32_at(d, 0)).and_then(|raw| self.link(w, raw)).and_then(|ek| self.get(ek));
        if let Some(e) = ench {
            f.enchantment = self.name(e);
            // The costliest effect: the one with the biggest magnitude × base cost.
            let mut best: Option<(f32, FormKey)> = None;
            let mut current: Option<FormKey> = None;
            for (s, d) in e.subs() {
                match s {
                    b"EFID" => current = u32_at(d, 0).and_then(|raw| self.link(e, raw)),
                    b"EFIT" => {
                        if let Some(mk) = current.take() {
                            let mag = f32_at(d, 0).unwrap_or(0.0).max(1.0);
                            let cost = self.get(mk).and_then(|m| m.first(b"DATA")).and_then(|dd| f32_at(dd, 4)).unwrap_or(0.0);
                            if best.is_none_or(|(c, _)| mag * cost > c) {
                                best = Some((mag * cost, mk));
                            }
                        }
                    }
                    _ => {}
                }
            }
            if let Some(m) = best.and_then(|(_, mk)| self.get(mk)) {
                f.effect_name = self.name(m);
                f.effect_keywords = self.keywords(m);
                f.effect_shader = m.first(b"DATA").and_then(|d| u32_at(d, 36)).and_then(|raw| self.link(m, raw)).map(|sk| (self.plugin_name(sk).to_string(), sk.id));
            }
        }
        f
    }
}
