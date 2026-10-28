//! `skel <a.hkx> [b.hkx]`: bone lists (and differences) of skeleton files; for clips, the binding.
fn main() {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    let mut lists = Vec::new();
    for p in &paths {
        let b = std::fs::read(p).unwrap();
        if let Ok(s) = sds_hkx::anim::read_skeletons(&b) {
            println!("{p}: {} skeletons, first {} bones", s.len(), s[0].bones.len());
            lists.push(s[0].bones.clone());
        } else if let Ok(a) = sds_hkx::anim::read_animation(&b) {
            println!("{p}: anim tracks {} binding {:?}", a.tracks, &a.track_to_bone[..a.track_to_bone.len().min(12)]);
        }
    }
    if lists.len() == 2 {
        for i in 0..lists[0].len().max(lists[1].len()) {
            let (a, b) = (lists[0].get(i), lists[1].get(i));
            if a != b {
                println!("  {i}: {a:?} vs {b:?}");
            }
        }
    }
}
