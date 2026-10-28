//! What pixel formats do the textures on this machine actually use, and
//! which of them does dds.rs reject? Walks every data root (loose files
//! and archives) so the answer is data, not guesswork.

use aether_core::bsa::BsaArchive;
use aether_core::dds;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn classify(bytes: &[u8]) -> String {
    if bytes.len() < 128 || &bytes[0..4] != b"DDS " {
        return "not-a-dds".into();
    }
    let u32_at = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
    let pf = 76;
    let flags = u32_at(pf + 4);
    let four_cc = &bytes[pf + 8..pf + 12];
    let bits = u32_at(pf + 12);
    if flags & 0x4 != 0 {
        let cc = String::from_utf8_lossy(four_cc).into_owned();
        if cc == "DX10" {
            let dxgi = u32_at(128);
            return format!("DX10 dxgi={dxgi}");
        }
        // Numeric fourccs are legacy D3DFMT codes (e.g. 116 = A32B32G32R32F).
        if four_cc[1..] == [0, 0, 0] {
            return format!("D3DFMT {}", four_cc[0]);
        }
        return cc;
    }
    let kind = if flags & 0x40 != 0 {
        "rgb"
    } else if flags & 0x2_0000 != 0 {
        "luminance"
    } else if flags & 0x20 != 0 {
        "pal8"
    } else if flags & 0x2 != 0 {
        "alpha-only"
    } else {
        "flags?"
    };
    format!(
        "{kind}{bits} r={:#x} g={:#x} b={:#x} a={:#x}",
        u32_at(pf + 16),
        u32_at(pf + 20),
        u32_at(pf + 24),
        u32_at(pf + 28)
    )
}

#[derive(Default)]
struct Bucket {
    count: usize,
    fails: usize,
    example: String,
    error: String,
}

fn main() {
    let roots = [
        r"C:\Users\newri\Projects\CPlusPlus\Skyrim VFX Editor\GameData",
        r"C:\Users\newri\Projects\CPlusPlus\Skyrim VFX Editor\GameDataRP",
    ];

    let mut buckets: BTreeMap<String, Bucket> = BTreeMap::new();
    let mut tga = 0usize;
    let mut tally = |name: &str, bytes: &[u8]| {
        let key = classify(bytes);
        let b = buckets.entry(key).or_default();
        b.count += 1;
        if b.example.is_empty() {
            b.example = name.to_string();
        }
        if let Err(e) = dds::decode_capped(bytes, 64) {
            b.fails += 1;
            if b.error.is_empty() {
                b.error = e.to_string();
            }
        }
    };

    for root in roots {
        let root = Path::new(root);
        let mut stack = vec![root.to_path_buf()];
        let mut archives: Vec<PathBuf> = Vec::new();
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in rd.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                let ext = path
                    .extension()
                    .map(|e| e.to_string_lossy().to_ascii_lowercase())
                    .unwrap_or_default();
                match ext.as_str() {
                    "dds" => {
                        if let Ok(bytes) = std::fs::read(&path) {
                            tally(&path.to_string_lossy(), &bytes);
                        }
                    }
                    "tga" => tga += 1,
                    "bsa" => archives.push(path),
                    _ => {}
                }
            }
        }
        for bsa_path in archives {
            let Ok(bsa) = BsaArchive::open(&bsa_path) else {
                continue;
            };
            let names: Vec<String> = bsa.names().to_vec();
            for name in names {
                if name.ends_with(".dds") {
                    if let Ok(bytes) = bsa.read_entry(&name) {
                        tally(&format!("{}::{name}", bsa_path.display()), &bytes);
                    }
                } else if name.ends_with(".tga") {
                    tga += 1;
                }
            }
        }
    }

    println!("{:>7} {:>6}  format", "count", "fails");
    for (key, b) in &buckets {
        println!("{:>7} {:>6}  {key}", b.count, b.fails);
        if b.fails > 0 {
            println!("                 first fail: {} ({})", b.example, b.error);
        }
    }
    println!("loose/archived .tga files: {tga}");
}
