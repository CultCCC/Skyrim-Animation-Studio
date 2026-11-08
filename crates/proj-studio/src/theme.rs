//! Dark theme and fonts shared with SDS Anim Studio, plus node colours.

use egui::Color32;


pub fn apply(ctx: &egui::Context) {
    ctx.set_theme(egui::ThemePreference::Dark);
    install_fonts(ctx);
    ctx.global_style_mut(|style| {
        style.interaction.selectable_labels = false;
        use egui::{FontFamily::Proportional, FontId, TextStyle};
        style.text_styles = [
            (TextStyle::Small, FontId::new(11.5, Proportional)),
            (TextStyle::Body, FontId::new(13.5, Proportional)),
            (TextStyle::Button, FontId::new(13.5, Proportional)),
            (TextStyle::Heading, FontId::new(17.0, Proportional)),
            (TextStyle::Monospace, FontId::new(12.5, egui::FontFamily::Monospace)),
        ]
        .into();
        style.spacing.interact_size.y = 21.0;
        style.spacing.item_spacing.y = 4.0;
        #[cfg(debug_assertions)]
        {
            style.debug.show_unaligned = false;
        }
    });
    let mut v = egui::Visuals::dark();
    v.panel_fill = Color32::from_rgb(28, 28, 32);
    v.window_fill = Color32::from_rgb(32, 32, 37);
    v.extreme_bg_color = Color32::from_rgb(18, 18, 21);
    v.faint_bg_color = Color32::from_rgb(36, 36, 41);
    v.selection.bg_fill = Color32::from_rgb(60, 92, 150);
    ctx.set_visuals(v);
}

fn install_fonts(ctx: &egui::Context) {
    use egui::{FontData, FontDefinitions, FontFamily};
    use std::sync::Arc;
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert("main".into(), Arc::new(FontData::from_static(include_bytes!("../../app/assets/fonts/DejaVuSans.ttf"))));
    fonts.font_data.insert("mono".into(), Arc::new(FontData::from_static(include_bytes!("../../app/assets/fonts/CascadiaMono.ttf"))));
    fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "main".into());
    let mono = fonts.families.entry(FontFamily::Monospace).or_default();
    mono.insert(0, "mono".into());
    mono.insert(1, "main".into());
    ctx.set_fonts(fonts);
}

use sds_proj::chain::Kind;

pub fn kind_color(k: Kind) -> Color32 {
    match k {
        Kind::Trigger => Color32::from_rgb(230, 170, 60),
        Kind::Func => Color32::from_rgb(150, 150, 165),
        Kind::Homing => Color32::from_rgb(220, 80, 80),
        Kind::Emitter => Color32::from_rgb(190, 110, 230),
        Kind::Follower => Color32::from_rgb(70, 190, 200),
        Kind::Multicast => Color32::from_rgb(90, 170, 90),
        Kind::McItem => Color32::from_rgb(120, 200, 140),
        Kind::Projectile => Color32::from_rgb(110, 150, 230),
        Kind::Loop | Kind::Missing => Color32::from_rgb(240, 60, 60),
        Kind::Note => Color32::from_rgb(120, 120, 120),
    }
}

pub const ERR: Color32 = Color32::from_rgb(240, 90, 80);
pub const WARN: Color32 = Color32::from_rgb(235, 190, 70);
pub const OK: Color32 = Color32::from_rgb(120, 200, 120);
pub const DIM: Color32 = Color32::from_rgb(140, 140, 150);
