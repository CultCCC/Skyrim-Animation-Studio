//! `npcs <mo2> <profile> <filter>`: index the load order and list matching NPCs.
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let vfs = sds_vfs::Vfs::build(&sds_vfs::Source::Mo2 { instance: a[1].clone().into(), profile: a[2].clone() }, &Default::default()).unwrap();
    let t = std::time::Instant::now();
    let db = sds_records::Db::build(&vfs, "russian");
    println!("indexed {} records in {:.2}s; warnings {}", db.records.len(), t.elapsed().as_secs_f32(), db.warnings.len());
    for w in db.warnings.iter().take(5) {
        println!("  {w}");
    }
    for (s, v) in &db.by_sig {
        println!("  {} {}", String::from_utf8_lossy(s), v.len());
    }
    let f = a.get(3).map(|s| s.to_lowercase()).unwrap_or_default();
    let mut n = 0;
    for k in &db.by_sig[b"NPC_"] {
        let r = db.get(*k).unwrap();
        let (e, name) = (r.edid(), db.name(r));
        if e.to_lowercase().contains(&f) || name.to_lowercase().contains(&f) {
            println!("{} {e} «{name}»", db.label(*k));
            if let Some(l) = db.npc_look(*k) {
                println!("   race {} female {} scale {:.2} weight {:.0}
   skel {} | {}
   facegen {:?}", l.race, l.female, l.scale, l.weight, l.skeleton_nif, l.skeleton_hkx, l.facegen.iter().filter(|f| vfs.exists(f)).collect::<Vec<_>>());
                for p in &l.parts {
                    println!("   {:?} {:08x} {} -> {} {}", p.kind, p.slots, p.source, p.model, if vfs.exists(&p.model) { "" } else { "(НЕТ ФАЙЛА)" });
                }
                for w in &l.weapons {
                    println!("   WEAP {} «{}» {} {}", w.edid, w.name, w.type_name(), w.model);
                }
                for n in &l.notes {
                    println!("   note: {n}");
                }
            }
            n += 1;
            if n > 15 { break; }
        }
    }
}

#[allow(dead_code)]
fn unused() {}
