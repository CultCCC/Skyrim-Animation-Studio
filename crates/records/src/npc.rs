//! What an NPC looks like: race skeleton, skin, outfit armor, FaceGen head
//! and weapons, resolved the way the game dresses a freshly spawned actor
//! (default outfit, first entries of leveled lists).
//!
//! Field layouts (SSE): ACBS flags @0 (0x1 female), template flags @18
//! (0x01 traits, 0x100 inventory); RACE DATA heights @16/@20; ANAM after the
//! MNAM / FNAM markers = male / female skeleton; ARMO lists its ARMAs as
//! MODL form ids; ARMA MOD2/MOD3 = male/female world models, RNAM + MODL =
//! races; BOD2/BODT first u32 = biped slot mask (bit n = slot 30 + n);
//! LVLO = level u16, pad, form u32, count; WEAP DNAM byte 0 = animation type.

use crate::{f32_at, u32_at, zstr, Db, FormKey, Rec};

pub const SLOT_HEAD: u32 = 1 << 0; // 30
pub const SLOT_HAIR: u32 = 1 << 1; // 31
pub const SLOT_SHIELD: u32 = 1 << 9; // 39

#[derive(Debug, Clone, PartialEq)]
pub enum PartKind {
    /// Race / NPC skin (body, hands, feet) — shown where no armor covers.
    Skin,
    Armor,
}

#[derive(Debug, Clone)]
pub struct Part {
    pub kind: PartKind,
    /// ARMO editor id (and ARMA in brackets).
    pub source: String,
    pub slots: u32,
    /// `meshes/...`
    pub model: String,
    pub shield: bool,
    /// The ARMA it comes from (footstep set, …).
    pub arma: Option<FormKey>,
}

#[derive(Debug, Clone)]
pub struct Weapon {
    pub key: FormKey,
    pub edid: String,
    pub name: String,
    pub model: String,
    pub anim_type: u8,
}

impl Weapon {
    pub fn type_name(&self) -> &'static str {
        match self.anim_type {
            0 => sds_i18n::t!("рукопашный", "unarmed"),
            1 => sds_i18n::t!("одноручный меч", "one-handed sword"),
            2 => sds_i18n::t!("кинжал", "dagger"),
            3 => sds_i18n::t!("одноручный топор", "one-handed axe"),
            4 => sds_i18n::t!("булава", "mace"),
            5 => sds_i18n::t!("двуручный меч", "two-handed sword"),
            6 => sds_i18n::t!("двуручный топор/молот", "two-handed axe/hammer"),
            7 => sds_i18n::t!("лук", "bow"),
            8 => sds_i18n::t!("посох", "staff"),
            9 => sds_i18n::t!("арбалет", "crossbow"),
            _ => "?",
        }
    }

    /// Node the weapon hangs from when drawn.
    pub fn hand_node(&self) -> &'static str {
        if matches!(self.anim_type, 7) {
            "SHIELD"
        } else {
            "WEAPON"
        }
    }
}

#[derive(Debug, Clone)]
pub struct NpcLook {
    pub key: FormKey,
    pub edid: String,
    pub name: String,
    pub race: String,
    pub female: bool,
    pub scale: f32,
    pub weight: f32,
    /// `meshes/...skeleton.nif` and its Havok twin.
    pub skeleton_nif: String,
    pub skeleton_hkx: String,
    /// FaceGen candidates, own first.
    pub facegen: Vec<String>,
    /// Generic race head for NPCs without FaceGen.
    pub head_fallback: String,
    pub parts: Vec<Part>,
    pub weapons: Vec<Weapon>,
    pub notes: Vec<String>,
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

fn slots(rec: &Rec) -> u32 {
    rec.first(b"BOD2").or_else(|| rec.first(b"BODT")).and_then(|d| u32_at(d, 0)).unwrap_or(0)
}

impl Db {
    fn rec_sig(&self, k: FormKey, sig: &[u8; 4]) -> Option<&Rec> {
        self.get(k).filter(|r| &r.sig == sig)
    }

    /// First entry of a leveled list, followed into nested lists.
    fn level_first(&self, k: FormKey, depth: usize) -> Option<FormKey> {
        let r = self.get(k)?;
        if &r.sig != b"LVLI" && &r.sig != b"LVLN" {
            return Some(k);
        }
        if depth > 8 {
            return None;
        }
        let entry = r.subs().find(|(s, _)| *s == b"LVLO").and_then(|(_, d)| u32_at(d, 4))?;
        self.level_first(self.link(r, entry)?, depth + 1)
    }

    /// The NPC that supplies `flag` data (template chain), and the chain for notes.
    fn template_for(&self, k: FormKey, flag: u16) -> FormKey {
        let mut cur = k;
        for _ in 0..8 {
            let Some(r) = self.rec_sig(cur, b"NPC_") else { return cur };
            let tflags = r.first(b"ACBS").and_then(|d| d.get(18..20)).map(|b| u16::from_le_bytes([b[0], b[1]])).unwrap_or(0);
            if tflags & flag == 0 {
                return cur;
            }
            let Some(t) = r.first(b"TPLT").and_then(|d| u32_at(d, 0)).and_then(|raw| self.link(r, raw)) else { return cur };
            match self.level_first(t, 0) {
                Some(next) => cur = next,
                None => return cur,
            }
        }
        cur
    }

    pub fn npc_look(&self, k: FormKey) -> Option<NpcLook> {
        let npc = self.rec_sig(k, b"NPC_")?;
        let mut notes = Vec::new();
        let traits_key = self.template_for(k, 0x01);
        let inv_key = self.template_for(k, 0x100);
        if traits_key != k {
            notes.push(sds_i18n::tf!("внешность из шаблона {}", "appearance from template {}", self.label(traits_key)));
        }
        if inv_key != k {
            notes.push(sds_i18n::tf!("инвентарь из шаблона {}", "inventory from template {}", self.label(inv_key)));
        }
        let traits = self.rec_sig(traits_key, b"NPC_")?;
        let inv = self.rec_sig(inv_key, b"NPC_")?;

        let female = traits.first(b"ACBS").and_then(|d| u32_at(d, 0)).unwrap_or(0) & 1 != 0;
        let npc_height = traits.first(b"NAM6").and_then(|d| f32_at(d, 0)).filter(|h| *h > 0.0).unwrap_or(1.0);
        let weight = traits.first(b"NAM7").and_then(|d| f32_at(d, 0)).unwrap_or(50.0);
        let race_key = traits.first(b"RNAM").and_then(|d| u32_at(d, 0)).and_then(|raw| self.link(traits, raw));
        let race = race_key.and_then(|r| self.rec_sig(r, b"RACE"));

        // Skeleton and race height.
        let (mut skel_m, mut skel_f) = (String::new(), String::new());
        let mut race_height = 1.0;
        let mut race_skin = None;
        let mut armor_race = None;
        if let Some(r) = race {
            let mut which = 0;
            for (s, d) in r.subs() {
                match s {
                    b"MNAM" => which = 1,
                    b"FNAM" => which = 2,
                    b"ANAM" if which == 1 && skel_m.is_empty() => skel_m = zstr(d),
                    b"ANAM" if which == 2 && skel_f.is_empty() => skel_f = zstr(d),
                    _ => {}
                }
            }
            if let Some(d) = r.first(b"DATA") {
                race_height = f32_at(d, if female { 20 } else { 16 }).filter(|h| *h > 0.0).unwrap_or(1.0);
            }
            race_skin = r.first(b"WNAM").and_then(|d| u32_at(d, 0)).and_then(|raw| self.link(r, raw));
            armor_race = r.first(b"RNAM").and_then(|d| u32_at(d, 0)).and_then(|raw| self.link(r, raw));
        }
        let skel = if female && !skel_f.is_empty() { skel_f } else { skel_m };
        let skeleton_nif = if skel.is_empty() { "meshes/actors/character/character assets/skeleton.nif".into() } else { meshes_path(&skel) };
        let skeleton_hkx = skeleton_nif.trim_end_matches(".nif").to_string() + ".hkx";

        let races: Vec<FormKey> = race_key.into_iter().chain(armor_race).collect();
        // An ARMA made for DefaultRace is what the game falls back to when
        // none fits the actor's race (custom races without additional-race
        // lists, e.g. SDS boss races).
        let default_race = self.find("Skyrim.esm", 0x19);
        let pick_arma_by = |armo: &Rec, kind: PartKind, used: u32, out: &mut Vec<Part>, loose: bool| {
            for (s, d) in armo.subs() {
                if s != b"MODL" || d.len() != 4 {
                    continue;
                }
                let Some(ak) = self.link(armo, u32_at(d, 0).unwrap()) else { continue };
                let Some(arma) = self.rec_sig(ak, b"ARMA") else { continue };
                let mut fits = arma.first(b"RNAM").and_then(|d| u32_at(d, 0)).and_then(|raw| self.link(arma, raw)).is_some_and(|r| races.contains(&r));
                if !fits {
                    fits = arma
                        .subs()
                        .filter(|(s, d)| *s == b"MODL" && d.len() == 4)
                        .filter_map(|(_, d)| self.link(arma, u32_at(d, 0).unwrap()))
                        .any(|r| races.contains(&r));
                }
                if !fits && loose {
                    fits = default_race.is_some() && arma.first(b"RNAM").and_then(|d| u32_at(d, 0)).and_then(|raw| self.link(arma, raw)) == default_race;
                }
                if !fits {
                    continue;
                }
                let aslots = slots(arma);
                // A skin piece shows only where no worn armor takes one of
                // the slots its skin ARMO claims (a one-piece skin ARMA may
                // span more slots, e.g. a circlet under a mask; those do not count).
                let oslots = slots(armo);
                let relevant = if aslots & oslots != 0 { aslots & oslots } else { aslots };
                if kind == PartKind::Skin && relevant & used != 0 {
                    continue;
                }
                let (first, second) = if female { (b"MOD3", b"MOD2") } else { (b"MOD2", b"MOD3") };
                let model = arma.first(first).or_else(|| arma.first(second)).map(zstr).unwrap_or_default();
                if model.is_empty() {
                    continue;
                }
                out.push(Part {
                    kind: kind.clone(),
                    source: format!("{} [{}]", armo.edid(), arma.edid()),
                    slots: aslots,
                    model: meshes_path(&model),
                    shield: aslots & SLOT_SHIELD != 0,
                    arma: Some(ak),
                });
                // One ARMA per race and armor, as the game picks.
                break;
            }
        };
        let pick_arma = |armo: &Rec, kind: PartKind, used: u32, out: &mut Vec<Part>| {
            let before = out.len();
            pick_arma_by(armo, kind.clone(), used, out, false);
            if out.len() == before {
                pick_arma_by(armo, kind, used, out, true);
            }
        };

        // Outfit armor, then skin under it.
        let mut parts = Vec::new();
        let mut used = 0u32;
        let outfit = inv.first(b"DOFT").and_then(|d| u32_at(d, 0)).and_then(|raw| self.link(inv, raw));
        if let Some(o) = outfit.and_then(|o| self.rec_sig(o, b"OTFT")) {
            if let Some(list) = o.first(b"INAM") {
                for c in list.chunks_exact(4) {
                    let Some(item) = self.link(o, u32::from_le_bytes(c.try_into().unwrap())).and_then(|k| self.level_first(k, 0)) else { continue };
                    let Some(armo) = self.rec_sig(item, b"ARMO") else { continue };
                    let s = slots(armo);
                    if s & used != 0 {
                        notes.push(sds_i18n::tf!("{} пропущен: слоты заняты", "{} skipped: slots taken", armo.edid()));
                        continue;
                    }
                    used |= s;
                    pick_arma(armo, PartKind::Armor, used, &mut parts);
                }
            }
        } else {
            notes.push(sds_i18n::t!("аутфита нет", "no outfit").into());
        }
        let skin = traits.first(b"WNAM").and_then(|d| u32_at(d, 0)).and_then(|raw| self.link(traits, raw)).or(race_skin);
        if let Some(armo) = skin.and_then(|s| self.rec_sig(s, b"ARMO")) {
            pick_arma(armo, PartKind::Skin, used, &mut parts);
            // pick_arma stops at the first fitting ARMA; the skin has several.
            for (s, d) in armo.subs() {
                if s != b"MODL" || d.len() != 4 {
                    continue;
                }
                let Some(ak) = self.link(armo, u32_at(d, 0).unwrap()) else { continue };
                if let Some(arma) = self.rec_sig(ak, b"ARMA") {
                    let already = parts.iter().any(|p| p.source.ends_with(&format!("[{}]", arma.edid())));
                    if !already {
                        let one = Rec { data: rebuild_single(armo, d), ..armo.clone() };
                        pick_arma(&one, PartKind::Skin, used, &mut parts);
                    }
                }
            }
        }

        // Weapons from the inventory (leveled entries resolved to their first item).
        let mut weapons = Vec::new();
        for (s, d) in inv.subs() {
            if s != b"CNTO" {
                continue;
            }
            let Some(item) = u32_at(d, 0).and_then(|raw| self.link(inv, raw)).and_then(|k| self.level_first(k, 0)) else { continue };
            let Some(w) = self.rec_sig(item, b"WEAP") else { continue };
            let model = w.first(b"MODL").map(zstr).unwrap_or_default();
            if model.is_empty() || weapons.iter().any(|x: &Weapon| x.key == item) {
                continue;
            }
            weapons.push(Weapon {
                key: item,
                edid: w.edid(),
                name: self.name(w),
                model: meshes_path(&model),
                anim_type: w.first(b"DNAM").and_then(|d| d.first().copied()).unwrap_or(0),
            });
        }

        // FaceGen: meshes/actors/character/facegendata/facegeom/<plugin>/<id>.nif
        let facegen = [k, traits_key]
            .iter()
            .map(|fk| format!("meshes/actors/character/facegendata/facegeom/{}/{:08x}.nif", self.plugin_name(*fk).to_ascii_lowercase(), fk.id))
            .collect();

        Some(NpcLook {
            key: k,
            edid: npc.edid(),
            name: self.name(npc),
            race: race.map(|r| r.edid()).unwrap_or_default(),
            female,
            scale: npc_height * race_height,
            weight,
            skeleton_nif,
            skeleton_hkx,
            facegen,
            head_fallback: format!("meshes/actors/character/character assets/{}head.nif", if female { "female" } else { "male" }),
            parts,
            weapons,
            notes,
        })
    }
}

/// A copy of an ARMO payload that lists only one armature (`MODL` = `arma`),
/// so the shared picker can be reused for each skin piece.
fn rebuild_single(armo: &Rec, arma: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for (s, d) in armo.subs() {
        if s == b"MODL" && d.len() == 4 && d != arma {
            continue;
        }
        out.extend_from_slice(s);
        out.extend_from_slice(&(d.len() as u16).to_le_bytes());
        out.extend_from_slice(d);
    }
    out
}
