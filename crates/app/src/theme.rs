//! Dark theme in the DS Anim Studio spirit, plus the category colours the
//! timeline, viewport and inspector share.

use egui::Color32;
use sds_anno::Category;

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
    fonts.font_data.insert("main".into(), Arc::new(FontData::from_static(include_bytes!("../assets/fonts/DejaVuSans.ttf"))));
    fonts.font_data.insert("mono".into(), Arc::new(FontData::from_static(include_bytes!("../assets/fonts/CascadiaMono.ttf"))));
    fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "main".into());
    let mono = fonts.families.entry(FontFamily::Monospace).or_default();
    mono.insert(0, "mono".into());
    mono.insert(1, "main".into());
    ctx.set_fonts(fonts);
}

pub fn category_color(c: Category) -> Color32 {
    match c {
        Category::Precision => Color32::from_rgb(220, 70, 70),
        Category::Pie => Color32::from_rgb(190, 110, 230),
        Category::SdsCombat => Color32::from_rgb(70, 190, 200),
        Category::Scar => Color32::from_rgb(240, 160, 60),
        Category::Combo => Color32::from_rgb(90, 170, 90),
        Category::Attack => Color32::from_rgb(230, 210, 80),
        Category::Sound => Color32::from_rgb(110, 150, 230),
        Category::Foot => Color32::from_rgb(140, 140, 140),
        Category::Motion => Color32::from_rgb(120, 200, 140),
        Category::Other => Color32::from_rgb(180, 180, 190),
    }
}
