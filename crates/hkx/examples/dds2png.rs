//! `dds2png <in.dds> <out.png>`: decode with Aether's DDS reader (debugging textures).
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let img = aether_core::dds::decode_capped(&std::fs::read(&a[1]).unwrap(), 1024).unwrap();
    std::fs::write(&a[2], aether_core::png::encode_rgba(img.width, img.height, &img.rgba).unwrap()).unwrap();
    let px = |x: u32, y: u32| img.pixel(x, y);
    println!("{}x{} corners {:?} {:?} mid {:?}", img.width, img.height, px(0, 0), px(img.width - 1, img.height - 1), px(img.width / 2, img.height / 2));
}
