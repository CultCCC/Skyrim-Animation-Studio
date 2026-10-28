//! Just enough of the load order's records to dress an NPC.
//!
//! Each active plugin is opened from its loose file and only the top-level
//! groups we need are read (NPC_, RACE, ARMO, ARMA, OTFT, LVLI, LVLN, WEAP);
//! the rest is skipped by group size, so 485 plugins index in about a
//! second. Records are keyed by (origin module, object id) — the module that
//! FIRST defines the form — and the last plugin in load order wins.
//! Localized names come from the `.STRINGS` files of the game language.

pub mod fx;
pub mod npc;
pub mod sound;

use sds_vfs::Vfs;
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};

pub const WANTED: [&[u8; 4]; 21] = [b"NPC_", b"RACE", b"ARMO", b"ARMA", b"OTFT", b"LVLI", b"LVLN", b"WEAP", b"SPEL", b"MGEF", b"ARTO", b"ENCH", b"KYWD", b"SNDR", b"SOUN", b"SNCT", b"FSTS", b"FSTP", b"IPDS", b"IPCT", b"MATT"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FormKey {
    /// Index of the defining plugin in [`Db::plugins`].
    pub module: u16,
    pub id: u32,
}

#[derive(Debug, Clone)]
pub struct Plugin {
    pub name: String,
    pub masters: Vec<String>,
    /// Masters as plugin indices (u16::MAX when not loaded).
    pub master_idx: Vec<u16>,
    pub localized: bool,
}

#[derive(Debug, Clone)]
pub struct Rec {
    pub sig: [u8; 4],
    pub flags: u32,
    /// Plugin the winning copy is in (its form ids resolve through it).
    pub plugin: u16,
    pub raw_form: u32,
    /// Decoded (inflated) payload.
    pub data: Vec<u8>,
}

impl Rec {
    pub fn subs(&self) -> Subs<'_> {
        Subs { d: &self.data, pos: 0, big: None }
    }

    pub fn first(&self, sig: &[u8; 4]) -> Option<&[u8]> {
        self.subs().find(|(s, _)| *s == sig).map(|(_, d)| d)
    }

    pub fn edid(&self) -> String {
        self.first(b"EDID").map(zstr).unwrap_or_default()
    }
}

pub struct Subs<'a> {
    d: &'a [u8],
    pos: usize,
    big: Option<usize>,
}

impl<'a> Iterator for Subs<'a> {
    type Item = (&'a [u8; 4], &'a [u8]);
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.pos + 6 > self.d.len() {
                return None;
            }
            let sig: &[u8; 4] = self.d[self.pos..self.pos + 4].try_into().ok()?;
            let mut size = u16::from_le_bytes([self.d[self.pos + 4], self.d[self.pos + 5]]) as usize;
            self.pos += 6;
            if let Some(b) = self.big.take() {
                size = b;
            }
            if self.pos + size > self.d.len() {
                return None;
            }
            let data = &self.d[self.pos..self.pos + size];
            self.pos += size;
            if sig == b"XXXX" && data.len() == 4 {
                self.big = Some(u32::from_le_bytes(data.try_into().unwrap()) as usize);
                continue;
            }
            return Some((sig, data));
        }
    }
}

pub fn zstr(d: &[u8]) -> String {
    let end = d.iter().position(|&b| b == 0).unwrap_or(d.len());
    // Plugins are Windows-1252 (or 1251 in Russian mods); keep ASCII exact
    // and decode the rest as UTF-8 where it is valid.
    String::from_utf8_lossy(&d[..end]).into_owned()
}

pub fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    d.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()))
}

pub fn f32_at(d: &[u8], o: usize) -> Option<f32> {
    u32_at(d, o).map(f32::from_bits)
}

#[derive(Default)]
pub struct Db {
    pub plugins: Vec<Plugin>,
    pub records: HashMap<FormKey, Rec>,
    pub by_sig: HashMap<[u8; 4], Vec<FormKey>>,
    strings: HashMap<u16, HashMap<u32, String>>,
    pub warnings: Vec<String>,
}

fn read_exact_at(f: &mut std::fs::File, pos: u64, len: usize) -> std::io::Result<Vec<u8>> {
    f.seek(SeekFrom::Start(pos))?;
    let mut b = vec![0u8; len];
    f.read_exact(&mut b)?;
    Ok(b)
}

impl Db {
    pub fn build(vfs: &Vfs, language: &str) -> Db {
        let mut db = Db::default();
        let names: Vec<String> = vfs.plugins.clone();
        for name in &names {
            db.plugins.push(Plugin { name: name.clone(), masters: Vec::new(), master_idx: Vec::new(), localized: false });
        }
        let index_of: HashMap<String, u16> = names.iter().enumerate().map(|(i, n)| (n.to_ascii_lowercase(), i as u16)).collect();
        for (pi, name) in names.iter().enumerate() {
            let Some(path) = vfs.loose_path(name) else {
                db.warnings.push(sds_i18n::tf!("{name}: плагин не найден как файл", "{name}: plugin not found as a file"));
                continue;
            };
            if let Err(e) = db.scan(pi as u16, &path, &index_of) {
                db.warnings.push(format!("{name}: {e}"));
            }
        }
        // Deleted forms and forms whose winner changed type drop out.
        let records = &db.records;
        for (sig, keys) in db.by_sig.iter_mut() {
            keys.sort();
            keys.dedup();
            keys.retain(|k| records.get(k).is_some_and(|r| &r.sig == sig));
        }
        db.load_strings(vfs, language);
        db
    }

    fn scan(&mut self, pi: u16, path: &std::path::Path, index_of: &HashMap<String, u16>) -> std::io::Result<()> {
        let mut f = std::fs::File::open(path)?;
        let len = f.metadata()?.len();
        let head = read_exact_at(&mut f, 0, 24)?;
        if &head[..4] != b"TES4" {
            return Err(std::io::Error::other("not a plugin"));
        }
        let tes4_size = u32::from_le_bytes(head[4..8].try_into().unwrap()) as usize;
        let tes4_flags = u32::from_le_bytes(head[8..12].try_into().unwrap());
        let tes4 = read_exact_at(&mut f, 24, tes4_size)?;
        let mut masters = Vec::new();
        for (sig, d) in (Subs { d: &tes4, pos: 0, big: None }) {
            if sig == b"MAST" {
                masters.push(zstr(d));
            }
        }
        let p = &mut self.plugins[pi as usize];
        p.master_idx = masters.iter().map(|m| *index_of.get(&m.to_ascii_lowercase()).unwrap_or(&u16::MAX)).collect();
        p.masters = masters;
        p.localized = tes4_flags & 0x80 != 0;

        let mut pos = 24 + tes4_size as u64;
        while pos + 24 <= len {
            let gh = read_exact_at(&mut f, pos, 24)?;
            if &gh[..4] != b"GRUP" {
                return Err(std::io::Error::other(format!("expected GRUP at {pos}")));
            }
            let gsize = u32::from_le_bytes(gh[4..8].try_into().unwrap()) as u64;
            let label: [u8; 4] = gh[8..12].try_into().unwrap();
            if WANTED.iter().any(|w| **w == label) && gsize > 24 {
                let body = read_exact_at(&mut f, pos + 24, (gsize - 24) as usize)?;
                self.read_group(pi, &body);
            }
            pos += gsize.max(24);
        }
        Ok(())
    }

    fn read_group(&mut self, pi: u16, body: &[u8]) {
        let mut o = 0;
        while o + 24 <= body.len() {
            let sig: [u8; 4] = body[o..o + 4].try_into().unwrap();
            let size = u32::from_le_bytes(body[o + 4..o + 8].try_into().unwrap()) as usize;
            if &sig == b"GRUP" {
                // Nested groups do not occur in these top-level types; skip.
                o += size.max(24);
                continue;
            }
            let flags = u32::from_le_bytes(body[o + 8..o + 12].try_into().unwrap());
            let raw_form = u32::from_le_bytes(body[o + 12..o + 16].try_into().unwrap());
            let end = (o + 24 + size).min(body.len());
            let payload = &body[o + 24..end];
            o = end;
            let key = self.resolve(pi, raw_form);
            if flags & 0x20 != 0 {
                // Deleted: the form is gone from here on.
                self.records.remove(&key);
                continue;
            }
            let data = if flags & 0x0004_0000 != 0 && payload.len() >= 4 {
                let n = u32::from_le_bytes(payload[..4].try_into().unwrap()) as usize;
                match aether_codecs::inflate::zlib_decompress(&payload[4..], n) {
                    Ok(d) => d,
                    Err(_) => continue,
                }
            } else {
                payload.to_vec()
            };
            self.by_sig.entry(sig).or_default().push(key);
            self.records.insert(key, Rec { sig, flags, plugin: pi, raw_form, data });
        }
    }

    /// A form id as written in plugin `pi` → its identity.
    pub fn resolve(&self, pi: u16, raw: u32) -> FormKey {
        let p = &self.plugins[pi as usize];
        let mi = (raw >> 24) as usize;
        let module = if mi < p.master_idx.len() { p.master_idx[mi] } else { pi };
        FormKey { module, id: raw & 0x00FF_FFFF }
    }

    /// Resolve a form id read from `rec`'s payload.
    pub fn link(&self, rec: &Rec, raw: u32) -> Option<FormKey> {
        if raw == 0 {
            return None;
        }
        let k = self.resolve(rec.plugin, raw);
        (k.module != u16::MAX).then_some(k)
    }

    pub fn get(&self, k: FormKey) -> Option<&Rec> {
        self.records.get(&k)
    }

    pub fn plugin_name(&self, k: FormKey) -> &str {
        self.plugins.get(k.module as usize).map(|p| p.name.as_str()).unwrap_or("?")
    }

    /// `Plugin.esp|0x123456` — the address form xEdit and houseCARL use.
    pub fn label(&self, k: FormKey) -> String {
        format!("{}|{:06X}", self.plugin_name(k), k.id)
    }

    fn load_strings(&mut self, vfs: &Vfs, language: &str) {
        let lang = language.to_ascii_lowercase();
        for (pi, p) in self.plugins.iter().enumerate() {
            if !p.localized {
                continue;
            }
            let stem = p.name.rsplit_once('.').map(|(s, _)| s).unwrap_or(&p.name).to_ascii_lowercase();
            for l in [lang.as_str(), "english"] {
                if let Ok(b) = vfs.read(&format!("strings/{stem}_{l}.strings")) {
                    self.strings.insert(pi as u16, parse_strings(&b));
                    break;
                }
            }
        }
    }

    /// FULL name of a record (localized plugins look it up in .STRINGS).
    pub fn name(&self, rec: &Rec) -> String {
        let Some(d) = rec.first(b"FULL") else { return String::new() };
        if self.plugins[rec.plugin as usize].localized && d.len() == 4 {
            let id = u32::from_le_bytes(d.try_into().unwrap());
            // The STRINGS file of the plugin that wrote this FULL.
            return self.strings.get(&rec.plugin).and_then(|s| s.get(&id)).cloned().unwrap_or_default();
        }
        zstr(d)
    }
}

/// `.STRINGS`: count, data size, (id, offset) directory, zero-terminated text.
fn parse_strings(b: &[u8]) -> HashMap<u32, String> {
    let mut out = HashMap::new();
    let (Some(count), Some(_)) = (u32_at(b, 0), u32_at(b, 4)) else { return out };
    let base = 8 + count as usize * 8;
    for i in 0..count as usize {
        let (Some(id), Some(off)) = (u32_at(b, 8 + i * 8), u32_at(b, 12 + i * 8)) else { break };
        let start = base + off as usize;
        if start < b.len() {
            out.insert(id, zstr(&b[start..]));
        }
    }
    out
}
