//! Temporary scan: find real NIFs containing BSPSysSubTexModifier,
//! NiPSysSpawnModifier or NiTriStrips across loose files and BSA archives.
//!
//! Run: cargo run --release -p aether-core --example block_scan

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use aether_core::bsa::BsaArchive;
use aether_nif::NifFile;

const TARGETS: [&str; 3] = ["BSPSysSubTexModifier", "NiPSysSpawnModifier", "NiTriStrips"];

const ROOTS: [&str; 2] = [
    r"C:\Users\newri\Projects\CPlusPlus\Skyrim VFX Editor\GameData",
    r"C:\Users\newri\Projects\CPlusPlus\Skyrim VFX Editor\GameDataRP",
];

const FIXTURES: &str = r"C:\Users\newri\Projects\CPlusPlus\Skyrim VFX Editor Rust\crates\nif\tests\fixtures";

#[derive(Default)]
struct Stats {
    /// target type -> total block occurrences across all files
    totals: BTreeMap<String, u64>,
    /// target type -> (file id, file size in bytes, blocks of that type in it)
    hits: BTreeMap<String, Vec<(String, u64, u64)>>,
    /// target type -> fixture file names containing it
    fixture_hits: BTreeMap<String, Vec<String>>,
    parsed: u64,
    parse_failures: u64,
}

impl Stats {
    fn record(&mut self, id: &str, size: u64, nif: &NifFile, fixture: bool) {
        self.parsed += 1;
        for target in TARGETS {
            let n = nif
                .blocks
                .iter()
                .filter(|b| b.type_name == target)
                .count() as u64;
            if n == 0 {
                continue;
            }
            *self.totals.entry(target.to_string()).or_default() += n;
            self.hits
                .entry(target.to_string())
                .or_default()
                .push((id.to_string(), size, n));
            if fixture {
                self.fixture_hits
                    .entry(target.to_string())
                    .or_default()
                    .push(id.to_string());
            }
        }
    }
}

fn walk(dir: &Path, nifs: &mut Vec<PathBuf>, bsas: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, nifs, bsas);
        } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            match ext.to_ascii_lowercase().as_str() {
                "nif" => nifs.push(path),
                "bsa" => bsas.push(path),
                _ => {}
            }
        }
    }
}

fn main() {
    let mut stats = Stats::default();

    for root in ROOTS {
        let root = Path::new(root);
        if !root.is_dir() {
            eprintln!("root missing, skipped: {}", root.display());
            continue;
        }
        let mut nifs = Vec::new();
        let mut bsas = Vec::new();
        walk(root, &mut nifs, &mut bsas);
        eprintln!(
            "{}: {} loose .nif, {} .bsa",
            root.display(),
            nifs.len(),
            bsas.len()
        );

        for path in &nifs {
            let Ok(data) = std::fs::read(path) else {
                stats.parse_failures += 1;
                continue;
            };
            match NifFile::parse(&data) {
                Ok(nif) => stats.record(&path.display().to_string(), data.len() as u64, &nif, false),
                Err(_) => stats.parse_failures += 1,
            }
        }

        for path in &bsas {
            let archive = match BsaArchive::open(path) {
                Ok(a) => a,
                Err(e) => {
                    eprintln!("BSA open failed {}: {:?}", path.display(), e);
                    continue;
                }
            };
            let names: Vec<String> = archive
                .names()
                .iter()
                .filter(|n| n.to_ascii_lowercase().ends_with(".nif"))
                .cloned()
                .collect();
            eprintln!("  {}: {} .nif entries", path.display(), names.len());
            for name in names {
                let Ok(data) = archive.read_entry(&name) else {
                    stats.parse_failures += 1;
                    continue;
                };
                let id = format!("{}::{}", path.display(), name);
                match NifFile::parse(&data) {
                    Ok(nif) => stats.record(&id, data.len() as u64, &nif, false),
                    Err(_) => stats.parse_failures += 1,
                }
            }
        }
    }

    // Fixtures.
    let fixtures = Path::new(FIXTURES);
    if fixtures.is_dir() {
        let mut nifs = Vec::new();
        let mut bsas = Vec::new();
        walk(fixtures, &mut nifs, &mut bsas);
        for path in &nifs {
            let Ok(data) = std::fs::read(path) else {
                stats.parse_failures += 1;
                continue;
            };
            match NifFile::parse(&data) {
                Ok(nif) => stats.record(&path.display().to_string(), data.len() as u64, &nif, true),
                Err(_) => stats.parse_failures += 1,
            }
        }
    }

    println!();
    println!(
        "parsed {} NIFs, {} parse/read failures",
        stats.parsed, stats.parse_failures
    );
    for target in TARGETS {
        let total = stats.totals.get(target).copied().unwrap_or(0);
        println!();
        println!("=== {} : {} blocks total ===", target, total);
        let mut files = stats.hits.get(target).cloned().unwrap_or_default();
        files.sort_by_key(|(_, size, _)| *size);
        println!("files containing it: {}", files.len());
        for (id, size, n) in files.iter().take(10) {
            println!("  {:>10} bytes  x{:<3}  {}", size, n, id);
        }
        let fixture_hits = stats.fixture_hits.get(target).cloned().unwrap_or_default();
        println!("fixture hits: {:?}", fixture_hits);
    }
}
