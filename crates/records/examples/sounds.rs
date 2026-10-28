//! `sounds <mo2> <profile> <EDID>...`: resolve SoundPlay descriptors and show
//! their files, the providing mod and the container format.
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let vfs = sds_vfs::Vfs::build(&sds_vfs::Source::Mo2 { instance: a[1].clone().into(), profile: a[2].clone() }, &Default::default()).unwrap();
    let db = sds_records::Db::build(&vfs, "russian");
    let index = db.sound_index();
    println!("{} sound EditorIDs", index.len());
    for e in &a[3..] {
        let Some(k) = index.get(&e.to_ascii_lowercase()) else {
            println!("{e}: not found");
            continue;
        };
        let d = db.sound_desc(*k).unwrap();
        println!("{e}: {:?}", (&d.category, d.category_volume, d.attenuation_db, d.db_variance, d.freq_shift, d.freq_variance, d.looping, &d.notes));
        for f in &d.files {
            println!("  ANAM {f}");
            let stem = f.rsplit_once('.').map(|(s, _)| s).unwrap_or(f);
            for cand in [f.clone(), format!("{stem}.xwm"), format!("{stem}.fuz")] {
                if let Ok(b) = vfs.read(&cand) {
                    println!("   {cand} <- {} [{}] {} bytes", vfs.origin(&cand).unwrap_or_default(), String::from_utf8_lossy(&b[..b.len().min(12)][8..]), b.len());
                    break;
                }
            }
        }
    }
}
