//! `extract <mo2> <profile> <out dir> <path>...`: copy load-order files out (BSA or loose) for inspection.
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let vfs = sds_vfs::Vfs::build(&sds_vfs::Source::Mo2 { instance: a[1].clone().into(), profile: a[2].clone() }, &Default::default()).unwrap();
    for p in &a[4..] {
        let out = std::path::Path::new(&a[3]).join(p.rsplit('/').next().unwrap());
        match vfs.read(p) {
            Ok(b) => { std::fs::write(&out, b).unwrap(); println!("{} <- {}", out.display(), vfs.origin(p).unwrap_or_default()); }
            Err(e) => println!("{p}: {e}"),
        }
    }
}
