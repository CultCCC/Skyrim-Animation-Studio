//! `find <mo2 dir> <profile> <substring>...`: list load-order paths containing all substrings, with their origin.
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let src = sds_vfs::Source::Mo2 { instance: a[1].clone().into(), profile: a[2].clone() };
    let vfs = sds_vfs::Vfs::build(&src, &Default::default()).unwrap();
    let mut hits = vfs.paths(|p| a[3..].iter().all(|s| p.contains(&s.to_lowercase())));
    hits.sort();
    for h in hits.iter().take(40) {
        println!("{h}  <- {}", vfs.origin(h).unwrap_or_default());
    }
    println!("{} hits", hits.len());
}
