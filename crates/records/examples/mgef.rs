//! `mgef <mo2> <profile> <plugin> <hex id>`: dump a SPEL's effects and the MGEF DATA form ids by offset.
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let vfs = sds_vfs::Vfs::build(&sds_vfs::Source::Mo2 { instance: a[1].clone().into(), profile: a[2].clone() }, &Default::default()).unwrap();
    let db = sds_records::Db::build(&vfs, "russian");
    let module = db.plugins.iter().position(|p| p.name.eq_ignore_ascii_case(&a[3])).unwrap() as u16;
    let id = u32::from_str_radix(a[4].trim_start_matches("0x"), 16).unwrap() & 0xFFFFFF;
    let spel = db.get(sds_records::FormKey { module, id }).expect("no form");
    println!("{} {}", String::from_utf8_lossy(&spel.sig), spel.edid());
    for (s, d) in spel.subs() {
        if s == b"EFID" {
            let k = db.link(spel, u32::from_le_bytes(d[..4].try_into().unwrap())).unwrap();
            let m = db.get(k).unwrap();
            println!(" MGEF {} «{}»", m.edid(), db.name(m));
            let data = m.first(b"DATA").unwrap();
            for off in (0..data.len().min(152)).step_by(4) {
                let raw = u32::from_le_bytes(data[off..off + 4].try_into().unwrap());
                if let Some(fk) = db.link(m, raw) {
                    if let Some(r) = db.get(fk) {
                        println!("   @{off}: {} {} {}", String::from_utf8_lossy(&r.sig), r.edid(), r.first(b"MODL").map(sds_records::zstr).unwrap_or_default());
                    }
                }
            }
        }
    }
}
