//! Spawn pattern preview: the figure's points around the caster, in an
//! orbit view (drag to turn, wheel to zoom). Points come from the DLL's own
//! formulas; the origin node height is approximate.

use crate::theme;
use egui::{pos2, vec2, Align2, Color32, FontId, Pos2, Sense, Stroke};
use sds_i18n::t;
use sds_proj::model::Pattern;
use sds_proj::pattern::{self, V3};

#[derive(Debug, Clone, Copy)]
pub struct Orbit {
    pub yaw: f32,
    pub pitch: f32,
    pub zoom: f32,
}

impl Default for Orbit {
    fn default() -> Self {
        Orbit { yaw: -0.6, pitch: 0.45, zoom: 1.0 }
    }
}

/// Rough height of common nodes on a standing human (game units).
fn node_height(origin: Option<&str>, follower: bool) -> f32 {
    let Some(o) = origin else { return if follower { 0.0 } else { 100.0 } };
    let o = o.to_lowercase();
    if o.contains("head") {
        120.0
    } else if o.contains("spine2") || o.contains("spn2") || o.contains("neck") {
        100.0
    } else if o.contains("spine") || o.contains("spn") {
        80.0
    } else if o.contains("hand") || o.contains("magic") || o.contains("weapon") {
        95.0
    } else if o.contains("pelvis") {
        70.0
    } else if o.contains("root") || o.contains("foot") {
        0.0
    } else {
        90.0
    }
}

pub fn show(ui: &mut egui::Ui, orbit: &mut Orbit, p: &Pattern, rotation: &str, follower: bool) {
    let z0 = node_height(p.origin.as_deref(), follower);
    let pts = pattern::points(p, z0);
    let sight: V3 = [0.0, 1500.0, 100.0];
    let dirs = if follower { Vec::new() } else { pattern::directions(&pts, rotation, sight) };

    let w = ui.available_width().min(520.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(w, w * 0.72), Sense::click_and_drag());
    if resp.dragged() {
        let d = resp.drag_delta();
        orbit.yaw += d.x * 0.01;
        orbit.pitch = (orbit.pitch + d.y * 0.01).clamp(-1.5, 1.5);
    }
    if resp.hovered() {
        let s = ui.input(|i| i.smooth_scroll_delta.y);
        if s != 0.0 {
            orbit.zoom = (orbit.zoom * (1.0 + s * 0.002)).clamp(0.1, 10.0);
        }
    }
    if resp.double_clicked() {
        *orbit = Orbit::default();
    }
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, Color32::from_rgb(20, 20, 24));

    let extent = pts.points.iter().map(|q| (q[0] * q[0] + q[1] * q[1] + (q[2] - 60.0).powi(2)).sqrt()).fold(160.0f32, f32::max);
    let scale = rect.height() * 0.42 / extent * orbit.zoom;
    let c = rect.center() + vec2(0.0, rect.height() * 0.12);
    let (sy, cy) = orbit.yaw.sin_cos();
    let (sp, cp) = orbit.pitch.sin_cos();
    let proj = |v: V3| -> Pos2 {
        // Yaw around Z, then tilt; screen x = rotated x, screen y = -z (with pitch).
        let x = v[0] * cy - v[1] * sy;
        let y = v[0] * sy + v[1] * cy;
        let z = v[2] - 60.0;
        let sy2 = z * cp - y * sp;
        pos2(c.x + x * scale, c.y - sy2 * scale)
    };

    // Ground rings.
    for r in [100.0f32, 200.0, 400.0] {
        let ring: Vec<Pos2> = (0..=48).map(|i| i as f32 / 48.0 * std::f32::consts::TAU).map(|a| proj([a.cos() * r, a.sin() * r, 0.0])).collect();
        painter.add(egui::Shape::line(ring, Stroke::new(1.0_f32, Color32::from_rgb(40, 40, 48))));
    }
    // Caster: body line, head, view arrow.
    let body = Stroke::new(3.0_f32, Color32::from_rgb(150, 150, 165));
    painter.line_segment([proj([0.0, 0.0, 0.0]), proj([0.0, 0.0, 120.0])], body);
    painter.circle_stroke(proj([0.0, 0.0, 128.0]), 8.0f32.max(9.0 * scale), Stroke::new(2.0_f32, Color32::from_rgb(150, 150, 165)));
    let a0 = proj([0.0, 0.0, 100.0]);
    let a1 = proj([0.0, 90.0, 100.0]);
    painter.arrow(a0, a1 - a0, Stroke::new(2.0_f32, theme::WARN));
    painter.text(a1, Align2::LEFT_BOTTOM, t!("взгляд", "view"), FontId::proportional(11.0), theme::WARN);

    // Centre and figure.
    let cpos = proj(pts.center);
    painter.circle_stroke(cpos, 4.0, Stroke::new(1.0_f32, theme::DIM));
    let shape = p.figure.shape.as_str();
    if matches!(shape, "Line" | "Circle" | "HalfCircle") && pts.points.len() > 1 {
        let mut line: Vec<Pos2> = pts.points.iter().map(|q| proj(*q)).collect();
        if shape == "Circle" {
            line.push(line[0]);
        }
        painter.add(egui::Shape::line(line, Stroke::new(1.0_f32, Color32::from_rgb(70, 110, 90))));
    }
    let n = pts.points.len();
    for (i, q) in pts.points.iter().enumerate() {
        let t = if n > 1 { i as f32 / (n - 1) as f32 } else { 0.0 };
        let col = Color32::from_rgb((90.0 + 150.0 * t) as u8, (200.0 - 80.0 * t) as u8, (240.0 - 120.0 * t) as u8);
        let sp = proj(*q);
        if let Some(d) = dirs.get(i) {
            let e = proj([q[0] + d[0] * 45.0, q[1] + d[1] * 45.0, q[2] + d[2] * 45.0]);
            painter.arrow(sp, e - sp, Stroke::new(1.2_f32, col.gamma_multiply(0.8)));
        }
        painter.circle_filled(sp, 4.5, col);
        if n <= 24 {
            painter.text(sp + vec2(5.0, -5.0), Align2::LEFT_BOTTOM, i.to_string(), FontId::proportional(10.0), theme::DIM);
        }
    }
    let note = if follower {
        t!("Спутники: точки вокруг кастера. Тяни — вращать, колесо — масштаб, двойной клик — сброс.", "Followers: points around the caster. Drag to turn, wheel to zoom, double-click to reset.")
    } else {
        t!("Точки спавна и направления (rotation). Тяни — вращать, колесо — масштаб.", "Spawn points and directions (rotation). Drag to turn, wheel to zoom.")
    };
    painter.text(rect.left_top() + vec2(6.0, 6.0), Align2::LEFT_TOP, note, FontId::proportional(11.0), theme::DIM);
    if p.origin.is_some() {
        painter.text(rect.left_bottom() + vec2(6.0, -6.0), Align2::LEFT_BOTTOM, t!("высота узла origin — примерная", "origin node height is approximate"), FontId::proportional(10.5), theme::DIM);
    }
}
