//! `roundtrip <file.hkx>...`: rewrite every clip with its own annotations plus
//! one extra, through `write_annotations` (which verifies itself), and report.
use sds_hkx::{read_clip, write::write_annotations, Annotation};

fn main() {
    let (mut ok, mut same, mut fail) = (0, 0, 0);
    for path in std::env::args().skip(1) {
        let bytes = std::fs::read(&path).unwrap();
        let Ok(info) = read_clip(&bytes) else { continue };
        match write_annotations(&bytes, &info.annotations) {
            Ok(out) => {
                ok += 1;
                if out == bytes {
                    same += 1;
                }
                let mut more = info.annotations.clone();
                more.push(Annotation { time: info.duration * 0.5, text: "SDS_NPC_TurnSpeed.0.5".into(), track: 0 });
                if let Err(e) = write_annotations(&bytes, &more) {
                    println!("{path}: edit FAILED {e}");
                    fail += 1;
                }
            }
            Err(e) => {
                println!("{path}: FAILED {e}");
                fail += 1;
            }
        }
    }
    println!("rewritten {ok}, byte-identical {same}, failures {fail}");
}
