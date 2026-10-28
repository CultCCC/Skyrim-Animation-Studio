//! `verify_kf <skeleton.hkx> <anim.hkx> <anim.kf>`: compare decoded local
//! poses with the keys of a KF exported from the same clip.
use aether_core::sequence::transform_keys_from;
use aether_nif::typed::controllers::NiControllerSequence;
use aether_nif::typed::{Ctx, TypedBlock};
use sds_hkx::anim::{local_pose, read_animation, read_skeletons};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let skel = read_skeletons(&std::fs::read(&a[1]).unwrap()).unwrap().remove(0);
    let anim = read_animation(&std::fs::read(&a[2]).unwrap()).unwrap();
    let kf = aether_nif::NifFile::parse(&std::fs::read(&a[3]).unwrap()).unwrap();
    let ctx = Ctx::of(&kf.header).unwrap();
    println!("skeleton {} bones, anim {} tracks {:.3}s", skel.bones.len(), anim.tracks, anim.duration);
    let (mut worst_r, mut worst_t, mut n, mut sum_r) = (0.0f32, 0.0f32, 0usize, 0.0f64);
    let mut worst_bone = String::new();
    for block in &kf.blocks {
        if block.type_name != "NiControllerSequence" {
            continue;
        }
        let seq = NiControllerSequence::decode(block, ctx).unwrap();
        for cb in &seq.controlled_blocks {
            let name = cb.node_name.resolve(&kf.header).unwrap_or_default().to_string();
            let Some(bone) = skel.bone(&name) else { continue };
            let Some(ib) = cb.interpolator.get() else { continue };
            let Some(keys) = transform_keys_from(&kf, ctx, &kf.blocks[ib]) else { continue };
            for (t, q) in &keys.rotations {
                let pose = local_pose(&skel, &anim, *t);
                let r = pose[bone].r; // xyzw
                let d = (r[0] * q[1] + r[1] * q[2] + r[2] * q[3] + r[3] * q[0]).abs().min(1.0);
                let ang = 2.0 * d.acos().to_degrees();
                if std::env::var("BONE").is_ok_and(|b| b == name) { println!("{t:.4} {ang:.3}"); }
                sum_r += ang as f64;
                n += 1;
                if ang > worst_r {
                    worst_r = ang;
                    worst_bone = format!("{name} t={t:.3} got {r:?} want(wxyz) {q:?}");
                }
            }
            for k in &keys.translations.keys {
                let pose = local_pose(&skel, &anim, k.time);
                let p = pose[bone].t;
                let d = ((p[0] - k.value[0]).powi(2) + (p[1] - k.value[1]).powi(2) + (p[2] - k.value[2]).powi(2)).sqrt();
                worst_t = worst_t.max(d);
            }
        }
    }
    println!("rotation keys {n}: mean {:.3}°, worst {:.3}° ({worst_bone}); translation worst {:.4}", sum_r / n.max(1) as f64, worst_r, worst_t);
}
