#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! SDS Projectile Studio — a GUI for NewProjectilesTMP (fenix31415) chains.
//!
//! Harness: `--screenshot out.png [--open SDSFoo.json] [--select trigger:0]
//! [--tab chains|json|issues] [--lang en] [--frames n] [--size WxH]`.

mod app;
mod chain_view;
mod inspector;
mod panels;
mod picker;
mod preview;
mod settings;
mod templates;
mod theme;

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().collect();
    let open_file = args.get(1).map(std::path::PathBuf::from).filter(|p| p.is_file());
    let arg = move |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let size = arg("--size").and_then(|s| s.split_once('x').and_then(|(w, h)| Some([w.parse::<f32>().ok()?, h.parse::<f32>().ok()?]))).unwrap_or([1600.0, 950.0]);
    let shot = arg("--screenshot").map(|p| app::Shot { path: p.into(), open: arg("--open"), select: arg("--select"), frames: arg("--frames").and_then(|f| f.parse().ok()).unwrap_or(10) });
    let tab = arg("--tab");
    let lang = arg("--lang");


    let native_options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default().with_title("SDS Projectile Studio").with_inner_size(size).with_min_inner_size([1024.0, 600.0]).with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        "sds-projectile-studio",
        native_options,
        Box::new(move |cc| {
            theme::apply(&cc.egui_ctx);
            let mut a = app::App::new();
            if let Some(l) = &lang {
                a.settings.language = l.clone();
                a.settings.apply_language();
            }
            if let Some(p) = open_file {
                a.open_disk(p);
            }
            a.center = match tab.as_deref() {
                Some("json") => app::Center::Json,
                Some("issues") => app::Center::Issues,
                _ => app::Center::Chains,
            };
            // Harness: `--new Name.json --add-chain homing,shotgun,...`
            if let Some(n) = arg("--new") {
                a.new_config(n);
                for t in arg("--add-chain").unwrap_or_default().split(',') {
                    let tp = match t {
                        "homing" => templates::Tpl::Homing,
                        "recast" => templates::Tpl::Recast,
                        "shotgun" => templates::Tpl::Shotgun,
                        "emitter" => templates::Tpl::Emitter,
                        "followers" => templates::Tpl::Followers,
                        "empty" => templates::Tpl::Empty,
                        _ => continue,
                    };
                    if let Some(d) = a.doc_mut() {
                        templates::add(&mut d.cfg, tp, Some("Skyrim.esm|0x10FBED".into()));
                    }
                }
                if let Some(s) = arg("--select").and_then(|s| app::parse_sel(&s)) {
                    a.select(s);
                }
            }
            if shot.is_some() {
                a.settings_open = false;
            }
            a.shot = shot;
            Ok(Box::new(a))
        }),
    )
}
