//! The NPC's path through the open clip against a target dummy, with or
//! without SDSCombat.dll (see `sds_anno::sdscombat`).

use crate::app::StudioApp;
use aether_core::mesh::Transform;
use sds_anno::sdscombat::{simulate, Path, Scene, Settings};
use sds_vfs::Vfs;

pub fn load_settings(vfs: &Vfs) -> (Settings, Option<String>) {
    let mut s = Settings::default();
    let p = "skse/plugins/sdscombat.ini";
    match vfs.read(p) {
        Ok(b) => {
            s.apply_ini(&String::from_utf8_lossy(&b));
            (s, Some(format!("{p} ({})", vfs.origin(p).unwrap_or_default())))
        }
        Err(_) => (s, None),
    }
}

/// Target position from the scene's distance and angle (0° = straight ahead,
/// positive = to the NPC's right).
pub fn target_pos(app: &StudioApp) -> [f32; 2] {
    let a = app.scene.target_angle.to_radians();
    [app.scene.target_dist * a.sin(), app.scene.target_dist * a.cos()]
}

pub fn npc_scale(app: &StudioApp) -> f32 {
    app.character.as_ref().map_or(1.0, |c| c.scale)
}

/// The path with SDSCombat as the scene says, or forced off (`raw`).
pub fn compute(app: &StudioApp, raw: bool) -> Option<Path> {
    let doc = app.doc.as_ref()?;
    let k = npc_scale(app);
    let settings = app.loaded.as_ref().map(|l| l.sdscombat.0.clone()).unwrap_or_default();
    let items: Vec<(f32, &str)> = doc.annos.iter().map(|a| (a.time, a.text.as_str())).collect();
    let motion = |t: f32| {
        let p = doc.motion.position(t);
        ([p[0] * k, p[1] * k, p[2] * k], doc.motion.yaw(t))
    };
    let scene = Scene { target: target_pos(app), npc_scale: k, ai_turn_speed: app.scene.ai_turn_speed, sdscombat: !raw && app.sdscombat_active() };
    Some(simulate(&items, doc.duration(), &motion, !doc.motion.moves.is_empty(), &scene, &settings))
}

/// Root placement at `t` from a computed path (root motion off: standing still).
pub fn root(k: f32, root_motion: bool, path: Option<&Path>, t: f32) -> Transform {
    let (pos, heading) = match path {
        Some(p) if root_motion => p.at(t),
        _ => ([0.0; 3], 0.0),
    };
    let yaw = -heading.to_radians();
    let (s, c) = yaw.sin_cos();
    Transform::from_av(pos, [c, -s, 0.0, s, c, 0.0, 0.0, 0.0, 1.0], k)
}
