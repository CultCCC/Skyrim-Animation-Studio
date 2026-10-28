//! `cargo run -p sds-hkx --example dump -- <file.hkx>...` — print what the studio reads.
fn main() {
    for path in std::env::args().skip(1) {
        let bytes = std::fs::read(&path).expect("read");
        match sds_hkx::read_clip(&bytes) {
            Ok(c) => {
                println!("{path}: {} {:.4}s frames={} tracks={} annoTracks={} skel={:?} motion={}",
                    c.kind, c.duration, c.frames, c.transform_tracks, c.annotation_tracks, c.skeleton_name, c.has_extracted_motion);
                for a in c.annotations.iter().filter(|a| !a.text.starts_with("anim")).take(std::env::var("DUMP_ALL").map(|_| usize::MAX).unwrap_or(12)) {
                    println!("  [{}] {:.4} {}", a.track, a.time, a.text);
                }
                println!("  ... {} annotations", c.annotations.len());
            }
            Err(e) => println!("{path}: ERROR {e}"),
        }
    }
}
