fn main() {
    let p = std::env::args().nth(1).unwrap();
    let b = std::fs::read(&p).unwrap();
    let info = sds_hkx::read_clip(&b).unwrap();
    let o = sds_hkx::write::write_annotations(&b, &info.annotations).unwrap();
    let first = b.iter().zip(&o).position(|(x, y)| x != y);
    println!("{} -> {} bytes, first diff at {:?}", b.len(), o.len(), first);
    if let Some(i) = first {
        let s = i.saturating_sub(16);
        println!("orig {:02x?}\nnew  {:02x?}", &b[s..(s + 48).min(b.len())], &o[s..(s + 48).min(o.len())]);
    }
}
