//! Open Animation Replacer libraries: what the load order (or a folder on
//! disk) holds, who competes for a clip, and exporting a submod.
//!
//! Layout: `meshes/actors/<project>/animations/OpenAnimationReplacer/
//! <ModName>/config.json` (mod: name, author, description, presets) and
//! `<ModName>/<SubMod>/config.json` (+ `user.json`, which fully shadows it)
//! next to the replacement `.hkx` files at their original relative paths.
//! Winners: highest `priority` among submods whose conditions pass; load
//! order plays no part.

use sds_vfs::Vfs;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const OAR: &str = "/openanimationreplacer/";

#[derive(Debug, Clone, PartialEq)]
pub enum Place {
    Vfs(String),
    Disk(PathBuf),
}

impl Place {
    pub fn read(&self, vfs: Option<&Vfs>) -> std::io::Result<Vec<u8>> {
        match self {
            Place::Vfs(p) => match vfs {
                Some(v) => v.read(p),
                None => Err(std::io::Error::other(sds_i18n::t!("сборка не загружена", "build not loaded"))),
            },
            Place::Disk(p) => std::fs::read(p),
        }
    }

    pub fn label(&self) -> String {
        match self {
            Place::Vfs(p) => p.clone(),
            Place::Disk(p) => p.display().to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct OarSub {
    /// Key: vfs folder path or disk folder path.
    pub folder: String,
    pub folder_name: String,
    pub name: String,
    pub priority: i64,
    pub disabled: bool,
    /// Effective config: user.json when present, else config.json.
    pub config: Value,
    pub has_user: bool,
    /// Where the effective config comes from (mod name or disk path).
    pub config_origin: String,
    /// (path relative to the submod folder, lowercase with `/`; where it is).
    pub files: Vec<(String, Place)>,
    /// `character`, `draugr`, ...
    pub project: String,
}

#[derive(Debug, Clone)]
pub struct OarMod {
    pub folder: String,
    pub folder_name: String,
    pub name: String,
    pub author: String,
    pub description: String,
    pub config: Option<Value>,
    pub subs: Vec<OarSub>,
    pub external: bool,
}

fn text(v: &Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

fn parse_json(bytes: &[u8]) -> Option<Value> {
    // Some configs ship with a BOM.
    let s = String::from_utf8_lossy(bytes);
    serde_json::from_str(s.trim_start_matches('\u{feff}')).ok()
}

/// `meshes/actors/<project>/animations/...` → project (lowercase).
pub fn project_of(path: &str) -> String {
    let p = path.replace('\\', "/").to_lowercase();
    p.find("/actors/")
        .map(|i| &p[i + 8..])
        .or_else(|| p.strip_prefix("actors/"))
        .and_then(|r| r.split('/').next())
        .unwrap_or("character")
        .to_string()
}

fn sub_from(folder: String, folder_name: String, config: Option<Value>, user: Option<Value>, origin: String, files: Vec<(String, Place)>, project: String) -> OarSub {
    let has_user = user.is_some();
    let config = user.or(config).unwrap_or(Value::Object(Default::default()));
    let name = text(&config, "name");
    OarSub {
        name: if name.is_empty() { folder_name.clone() } else { name },
        priority: config.get("priority").and_then(|v| v.as_i64()).unwrap_or(0),
        disabled: config.get("disabled").and_then(|v| v.as_bool()).unwrap_or(false),
        config,
        has_user,
        config_origin: origin,
        files,
        project,
        folder,
        folder_name,
    }
}

fn finish(mods: BTreeMap<String, OarMod>) -> Vec<OarMod> {
    let mut out: Vec<OarMod> = mods.into_values().filter(|m| !m.subs.is_empty()).collect();
    for m in &mut out {
        m.subs.sort_by(|a, b| b.priority.cmp(&a.priority));
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

/// Every OAR mod in the load order. `anims` = the library's clip paths.
pub fn scan_vfs(vfs: &Vfs, anims: &[String]) -> Vec<OarMod> {
    let mut configs = vfs.paths(|p| p.contains(OAR) && p.ends_with("/config.json"));
    configs.sort();
    let mut mods: BTreeMap<String, OarMod> = BTreeMap::new();
    for cfg in &configs {
        let i = cfg.find(OAR).unwrap() + OAR.len();
        let parts: Vec<&str> = cfg[i..].split('/').collect();
        let mod_folder = format!("{}{}", &cfg[..i], parts[0]);
        let json = vfs.read(cfg).ok().and_then(|b| parse_json(&b));
        let m = mods.entry(mod_folder.clone()).or_insert_with(|| OarMod {
            folder: mod_folder.clone(),
            folder_name: parts[0].to_string(),
            name: parts[0].to_string(),
            author: String::new(),
            description: String::new(),
            config: None,
            subs: Vec::new(),
            external: false,
        });
        match parts.len() {
            2 => {
                if let Some(j) = &json {
                    let n = text(j, "name");
                    if !n.is_empty() {
                        m.name = n;
                    }
                    m.author = text(j, "author");
                    m.description = text(j, "description");
                }
                m.config = json;
            }
            3 => {
                let folder = format!("{mod_folder}/{}", parts[1]);
                let user_path = format!("{folder}/user.json");
                let user = vfs.read(&user_path).ok().and_then(|b| parse_json(&b));
                let origin = vfs.origin(if user.is_some() { &user_path } else { cfg }).unwrap_or_default();
                let prefix = format!("{folder}/");
                let files = anims
                    .iter()
                    .filter(|a| a.starts_with(&prefix))
                    .map(|a| (a[prefix.len()..].to_string(), Place::Vfs(a.clone())))
                    .collect();
                m.subs.push(sub_from(folder, parts[1].to_string(), json, user, origin, files, project_of(cfg)));
            }
            _ => {}
        }
    }
    finish(mods)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

/// OAR mods under a folder on disk: a mod's own folder, an
/// `OpenAnimationReplacer` folder, or a whole mod / Data tree.
pub fn scan_disk(root: &Path) -> Result<Vec<OarMod>, String> {
    let mut all = Vec::new();
    walk(root, &mut all);
    let configs: Vec<&PathBuf> = all.iter().filter(|p| p.file_name().is_some_and(|n| n.eq_ignore_ascii_case("config.json"))).collect();
    if configs.is_empty() {
        return Err(sds_i18n::tf!("в {} нет config.json", "no config.json in {}", root.display()));
    }
    let mut mods: BTreeMap<String, OarMod> = BTreeMap::new();
    for cfg in configs {
        let dir = cfg.parent().unwrap().to_path_buf();
        let json = std::fs::read(cfg).ok().and_then(|b| parse_json(&b));
        // A submod config carries priority/conditions or sits next to its
        // clips; the mod-level one (name, author, description) does neither.
        let direct_hkx = all.iter().any(|p| p.parent() == Some(dir.as_path()) && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("hkx")));
        let is_sub = json.as_ref().is_some_and(|j| j.get("priority").is_some() || j.get("conditions").is_some()) || direct_hkx;
        let mod_dir = if is_sub { dir.parent().unwrap_or(&dir).to_path_buf() } else { dir.clone() };
        let key = mod_dir.display().to_string();
        let m = mods.entry(key.clone()).or_insert_with(|| OarMod {
            folder: key.clone(),
            folder_name: mod_dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            name: mod_dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            author: String::new(),
            description: String::new(),
            config: None,
            subs: Vec::new(),
            external: true,
        });
        if !is_sub {
            if let Some(j) = &json {
                let n = text(j, "name");
                if !n.is_empty() {
                    m.name = n;
                }
                m.author = text(j, "author");
                m.description = text(j, "description");
            }
            m.config = json;
            continue;
        }
        let user_path = dir.join("user.json");
        let user = std::fs::read(&user_path).ok().and_then(|b| parse_json(&b));
        let origin = if user.is_some() { user_path.display().to_string() } else { cfg.display().to_string() };
        let files = all
            .iter()
            .filter(|p| p.starts_with(&dir) && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("hkx")))
            .map(|p| (p.strip_prefix(&dir).unwrap().to_string_lossy().replace('\\', "/").to_lowercase(), Place::Disk(p.clone())))
            .collect();
        let folder_name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        m.subs.push(sub_from(dir.display().to_string(), folder_name, json, user, origin, files, project_of(&dir.to_string_lossy())));
    }
    Ok(finish(mods))
}

/// Where a clip sits: (project, OAR mod folder name, submod folder name, path relative to the submod / animations folder).
pub fn locate(path: &str) -> Option<(String, Option<(String, String)>, String)> {
    let p = path.replace('\\', "/").to_lowercase();
    let project = project_of(&p);
    if let Some(i) = p.find(OAR) {
        let rest = &p[i + OAR.len()..];
        let parts: Vec<&str> = rest.splitn(3, '/').collect();
        if parts.len() == 3 {
            return Some((project, Some((parts[0].to_string(), parts[1].to_string())), parts[2].to_string()));
        }
        return None;
    }
    let i = p.find("/animations/")?;
    Some((project, None, p[i + "/animations/".len()..].to_string()))
}

pub struct Competitor {
    pub mod_name: String,
    pub sub_name: String,
    pub priority: i64,
    pub disabled: bool,
    pub external: bool,
    pub folder: String,
    pub place: Place,
}

/// Every submod replacing `project` / `rel`, highest priority first.
pub fn competitors<'a>(libs: impl Iterator<Item = &'a OarMod>, project: &str, rel: &str) -> Vec<Competitor> {
    let mut out = Vec::new();
    for m in libs {
        for s in &m.subs {
            if s.project != project {
                continue;
            }
            if let Some((_, place)) = s.files.iter().find(|(r, _)| r == rel) {
                out.push(Competitor {
                    mod_name: m.name.clone(),
                    sub_name: s.name.clone(),
                    priority: s.priority,
                    disabled: s.disabled,
                    external: m.external,
                    folder: s.folder.clone(),
                    place: place.clone(),
                });
            }
        }
    }
    out.sort_by(|a, b| b.priority.cmp(&a.priority));
    out
}

pub struct ExportSpec {
    pub mod_folder: String,
    pub sub_folder: String,
    /// Write `user.json` instead of `config.json` (an override of someone else's submod in place).
    pub as_user_json: bool,
    pub copy_hkx: bool,
    pub config: Value,
    /// Mod-level config.json to write when the OAR mod folder is new.
    pub mod_config: Option<Value>,
}

#[derive(Default)]
pub struct Report {
    pub dest: PathBuf,
    pub written: usize,
    pub edited: usize,
    pub failed: Vec<String>,
}

/// Write the submod into `out_root` as a mod folder MO2 can enable.
/// `edited(rel)` returns replacement bytes for clips edited in the studio.
pub fn export(sub: &OarSub, spec: &ExportSpec, out_root: &Path, vfs: Option<&Vfs>, edited: &dyn Fn(&str) -> Option<Vec<u8>>) -> Result<Report, String> {
    if spec.mod_folder.trim().is_empty() || spec.sub_folder.trim().is_empty() {
        return Err(sds_i18n::t!("пустое имя папки мода или сабмода", "empty mod or submod folder name").into());
    }
    let bad = |s: &str| s.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']) || s.trim() == "." || s.trim() == "..";
    if bad(&spec.mod_folder) || bad(&spec.sub_folder) {
        return Err(sds_i18n::t!("недопустимые символы в имени папки", "invalid characters in the folder name").into());
    }
    let mod_dir = out_root
        .join("meshes")
        .join("actors")
        .join(&sub.project)
        .join("animations")
        .join("OpenAnimationReplacer")
        .join(spec.mod_folder.trim());
    let dest = mod_dir.join(spec.sub_folder.trim());
    std::fs::create_dir_all(&dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    let mut report = Report { dest: dest.clone(), ..Default::default() };

    if let Some(mc) = &spec.mod_config {
        let p = mod_dir.join("config.json");
        if !p.exists() {
            let s = serde_json::to_string_pretty(mc).map_err(|e| e.to_string())?;
            std::fs::write(&p, s).map_err(|e| format!("{}: {e}", p.display()))?;
            report.written += 1;
        }
    }
    let cfg_name = if spec.as_user_json { "user.json" } else { "config.json" };
    let s = serde_json::to_string_pretty(&spec.config).map_err(|e| e.to_string())?;
    std::fs::write(dest.join(cfg_name), s).map_err(|e| format!("{cfg_name}: {e}"))?;
    report.written += 1;

    if spec.copy_hkx {
        for (rel, place) in &sub.files {
            let target = dest.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
            if let Place::Disk(src) = place {
                if std::fs::canonicalize(src).ok() == std::fs::canonicalize(&target).ok() && target.exists() {
                    report.failed.push(sds_i18n::tf!("{rel}: назначение совпадает с источником — пропущен", "{rel}: destination equals the source — skipped"));
                    continue;
                }
            }
            let (bytes, was_edited) = match edited(rel) {
                Some(b) => (Ok(b), true),
                None => (place.read(vfs), false),
            };
            match bytes {
                Ok(b) => {
                    if let Some(parent) = target.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    match std::fs::write(&target, b) {
                        Ok(()) => {
                            report.written += 1;
                            if was_edited {
                                report.edited += 1;
                            }
                        }
                        Err(e) => report.failed.push(format!("{rel}: {e}")),
                    }
                }
                Err(e) => report.failed.push(format!("{rel}: {e}")),
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locate_paths() {
        let (p, s, r) = locate("meshes/actors/character/animations/openanimationreplacer/sds boss/fase1/bfco_attack3.hkx").unwrap();
        assert_eq!((p.as_str(), s.unwrap(), r.as_str()), ("character", ("sds boss".to_string(), "fase1".to_string()), "bfco_attack3.hkx"));
        let (p, s, r) = locate("meshes/actors/draugr/animations/1hm_attack1.hkx").unwrap();
        assert_eq!((p.as_str(), s, r.as_str()), ("draugr", None, "1hm_attack1.hkx"));
    }

    #[test]
    fn disk_scan_and_export() {
        let root = std::env::temp_dir().join(format!("sds_oar_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let sub = root.join("OpenAnimationReplacer").join("My Mod").join("Base");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(root.join("OpenAnimationReplacer/My Mod/config.json"), r#"{"name":"My Mod","author":"Me"}"#).unwrap();
        std::fs::write(sub.join("config.json"), "\u{feff}{\"name\":\"Base\",\"priority\":500,\"conditions\":[]}").unwrap();
        std::fs::write(sub.join("BFCO_Attack1.hkx"), b"x").unwrap();
        let mods = scan_disk(&root.join("OpenAnimationReplacer")).unwrap();
        assert_eq!(mods.len(), 1);
        assert_eq!((mods[0].name.as_str(), mods[0].author.as_str()), ("My Mod", "Me"));
        let s = &mods[0].subs[0];
        assert_eq!((s.name.as_str(), s.priority, s.files[0].0.as_str()), ("Base", 500, "bfco_attack1.hkx"));
        let out = root.join("out");
        let mut cfg = s.config.clone();
        cfg["priority"] = 600.into();
        let spec = ExportSpec { mod_folder: "New".into(), sub_folder: "Base2".into(), as_user_json: false, copy_hkx: true, config: cfg, mod_config: Some(serde_json::json!({"name":"New"})) };
        let r = export(s, &spec, &out, None, &|rel| (rel == "bfco_attack1.hkx").then(|| b"edited".to_vec())).unwrap();
        assert_eq!((r.written, r.edited, r.failed.len()), (3, 1, 0));
        let dest = out.join("meshes/actors/character/animations/OpenAnimationReplacer/New/Base2");
        assert_eq!(std::fs::read(dest.join("bfco_attack1.hkx")).unwrap(), b"edited");
        let back: Value = serde_json::from_slice(&std::fs::read(dest.join("config.json")).unwrap()).unwrap();
        assert_eq!(back["priority"], 600);
        let _ = std::fs::remove_dir_all(&root);
    }
}
