#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod audio;
mod character;
mod combo;
mod combo_panel;
mod doc;
mod fx;
mod library;
mod npc_panel;
mod oar;
mod oar_panel;
mod panels;
mod pie;
mod precision_view;
mod project;
mod render;
mod scar_panel;
mod sds_path;
mod settings;
mod sound;
mod theme;
mod timeline;
mod viewport3d;

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--sound-test") {
        sound_test(&args[2..]);
        return Ok(());
    }
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let open_on_start = args.get(1).map(std::path::PathBuf::from).filter(|p| p.is_file());
    let shot = arg("--screenshot").map(|p| app::Shot {
        path: p.into(),
        open: arg("--open"),
        time: arg("--time").and_then(|t| t.parse().ok()),
        select: arg("--select").and_then(|t| t.parse().ok()),
        frames: 8,
        opened: false,
        save: false,
        npc: arg("--npc"),
        select_text: arg("--select-text"),
        save_project: None,
    });

    let cam_dist = arg("--dist").and_then(|v| v.parse::<f32>().ok());
    let cam_yaw = arg("--yaw").and_then(|v| v.parse::<f32>().ok());
    let show_skeleton = args.iter().any(|a| a == "--skeleton");
    let oar_mode = args.iter().any(|a| a == "--oar");
    let focus_tab = if args.iter().any(|a| a == "--npc-tab") { Some("npc".to_string()) } else { arg("--tab") };
    let target = (arg("--target-dist").and_then(|v| v.parse::<f32>().ok()), arg("--target-angle").and_then(|v| v.parse::<f32>().ok()));
    let open_project = arg("--project").map(std::path::PathBuf::from);
    let save_project = arg("--save-project").map(std::path::PathBuf::from);
    let save_after_open = args.iter().any(|a| a == "--save");
    let lang = arg("--lang");
    let combo_sim = arg("--combo-sim").and_then(|v| v.parse::<u32>().ok());

    let native_options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default()
            .with_title("SDS Anim Studio")
            .with_inner_size([1600.0, 950.0])
            .with_min_inner_size([1024.0, 600.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        "sds-anim-studio",
        native_options,
        Box::new(move |cc| {
            theme::apply(&cc.egui_ctx);
            let mut app = app::StudioApp::new();
            app.gpu = cc.wgpu_render_state.clone().map(render::Gpu::new);
            if let Some(p) = open_on_start {
                app.open_disk_file(p);
            }
            app.lib_mode_oar = oar_mode;
            if let Some(l) = &lang {
                app.settings.language = l.clone();
                app.settings.apply_language();
            }
            if let Some(d) = cam_dist {
                app.camera.dist = d;
            }
            if let Some(y) = cam_yaw {
                app.camera.yaw = y.to_radians();
            }
            app.scene.show_skeleton = show_skeleton;
            let tab = match focus_tab.as_deref() {
                Some("npc") => Some(app::Tab::Npc),
                Some("scene") => Some(app::Tab::Scene),
                Some("anno") => Some(app::Tab::Annotations),
                Some("inspector") => Some(app::Tab::Inspector),
                Some("oar") => Some(app::Tab::Oar),
                Some("combo") => Some(app::Tab::Combo),
                Some("scar") => Some(app::Tab::Scar),
                _ => None,
            };
            if let Some(at) = tab.and_then(|t| app.dock.find_tab(&t)) {
                let _ = app.dock.set_active_tab(at);
            }
            if let Some(d) = target.0 {
                app.scene.target_dist = d;
            }
            if let Some(a) = target.1 {
                app.scene.target_angle = a;
            }
            if let Some(p) = &open_project {
                app.open_project(p);
            }
            app.shot = shot;
            if std::env::args().any(|a| a == "--custom-open") {
                app.custom_open = true;
            }
            if std::env::args().any(|a| a == "--favs-open") {
                app.favs_open = true;
            }
            if std::env::args().any(|a| a == "--combo-next") {
                app.combo.only_next = true;
            }
            if let Some(n) = combo_sim {
                app.combo.auto_steps = n;
                if let Some(s) = &mut app.shot {
                    s.frames += n * 8;
                }
            }
            if let Some(s) = &mut app.shot {
                s.save_project = save_project;
                s.save = save_after_open;
            }
            Ok(Box::new(app))
        }),
    )
}

/// `--sound-test <vfs path>...`: parse each sound of the configured build and
/// check XAudio2 accepts it (silent: nothing is played).
fn sound_test(paths: &[String]) {
    let s = settings::Settings::load();
    let vfs = match sds_vfs::Vfs::build(&sds_vfs::Source::Mo2 { instance: s.mo2_dir.clone().into(), profile: s.profile.clone() }, &Default::default()) {
        Ok(v) => v,
        Err(e) => return println!("vfs: {e}"),
    };
    let audio = match audio::Audio::start(0.0) {
        Ok(a) => a,
        Err(e) => return println!("audio: {e}"),
    };
    for p in paths {
        let r = vfs.read(p).map_err(|e| e.to_string()).and_then(|b| audio::parse(&b)).and_then(|c| {
            let info = format!("{} {:.2} s", c.format, c.seconds);
            audio.probe(std::sync::Arc::new(c)).map(|_| info)
        });
        println!("{p}: {r:?}");
    }
}
