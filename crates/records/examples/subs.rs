//! `subs <mo2> <profile> <plugin> <hex id>...`: subrecord signatures and sizes of records.
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let vfs = sds_vfs::Vfs::build(&sds_vfs::Source::Mo2 { instance: a[1].clone().into(), profile: a[2].clone() }, &Default::default()).unwrap();
    let db = sds_records::Db::build(&vfs, "russian");
    for id in &a[4..] {
        let id = u32::from_str_radix(id.trim_start_matches("0x"), 16).unwrap();
        let Some(k) = db.find(&a[3], id) else { println!("{id:06X}: not found"); continue };
        let r = db.get(k).unwrap();
        println!("{} {} [{}]", String::from_utf8_lossy(&r.sig), r.edid(), db.plugin_name(k));
        for (s, d) in r.subs() {
            let link = if d.len() == 4 { db.link(r, u32::from_le_bytes(d.try_into().unwrap())).and_then(|k| db.get(k)).map(|x| format!(" -> {} {}", String::from_utf8_lossy(&x.sig), x.edid())).unwrap_or_default() } else { String::new() };
            let txt = if d.last() == Some(&0) && d.len() > 1 && d[..d.len()-1].iter().all(|c| c.is_ascii_graphic() || *c == b' ' || *c == b'\\') { format!(" \"{}\"", String::from_utf8_lossy(&d[..d.len()-1])) } else { String::new() };
            println!("  {} {}{}{}", String::from_utf8_lossy(s), d.len(), link, txt);
        }
    }
}
