//! Preview export: the posed character alone on a transparent background,
//! as a PNG of one moment or an animated GIF of the whole clip.
//!
//! The camera is orthographic and framed once per clip over the character's
//! bounds at every sampled frame, so the actor never leaves the picture and
//! does not jump in size between frames. Root motion is off: the actor plays
//! in place, the way a radial-menu preview wants it.

use crate::app::StudioApp;
use crate::paired::Skinned;
use crate::render::{look_at, mat_mul, DrawMesh, Frame};
use std::path::Path;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ExportOpts {
    /// PNG side, px.
    pub png_size: u32,
    /// GIF side, px.
    pub gif_size: u32,
    pub fps: f32,
    /// Camera yaw around the actor, degrees (0 = straight in front).
    pub yaw: f32,
    /// Camera height angle, degrees.
    pub pitch: f32,
    /// Hard cap on GIF frames (long clips are sampled sparser).
    pub max_frames: u32,
    /// Extra ambient light of previews.
    pub ambient: f32,
    /// NPC (EditorID) the batch export dresses the actor as; empty = keep.
    pub npc: String,
    /// Side of the picture in game units, the same for every clip, so all
    /// previews share one scale; the feet rest on the bottom margin.
    /// 0 = fit each clip on its own.
    pub world_size: f32,
}

impl Default for ExportOpts {
    fn default() -> Self {
        ExportOpts { png_size: 512, gif_size: 320, fps: 25.0, yaw: 20.0, pitch: 8.0, max_frames: 250, ambient: 0.3, npc: "Sven".into(), world_size: 170.0 }
    }
}

/// Skinned meshes of every actor at time `t`, in place (no root motion).
fn skinned_at(app: &StudioApp, t: f32) -> Option<Vec<Skinned>> {
    app.character.as_ref()?;
    let root = crate::sds_path::root(crate::sds_path::npc_scale(app), false, None, t);
    Some(crate::paired::skin_all(app, t, &root))
}

struct View {
    view_proj: [f32; 16],
    light: [f32; 3],
}

/// Orthographic camera that fits the character at all `times`.
fn frame_view(app: &StudioApp, opts: &ExportOpts, times: &[f32]) -> Option<View> {
    let (yaw, pitch) = (opts.yaw.to_radians(), opts.pitch.to_radians());
    let dir = [pitch.cos() * yaw.sin(), pitch.cos() * yaw.cos(), pitch.sin()];
    let center = [0.0, 0.0, 64.0];
    let eye = [center[0] + dir[0] * 1000.0, center[1] + dir[1] * 1000.0, center[2] + dir[2] * 1000.0];
    let view = look_at(eye, center, [0.0, 0.0, 1.0]);
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for &t in times {
        for mesh in skinned_at(app, t)? {
            for v in mesh.verts.iter().step_by(3) {
                let p = v.pos;
                for (r, l) in [0usize, 1, 2].iter().zip(0..3) {
                    let q = view[*r] * p[0] + view[4 + r] * p[1] + view[8 + r] * p[2] + view[12 + r];
                    lo[l] = lo[l].min(q);
                    hi[l] = hi[l].max(q);
                }
            }
        }
    }
    if lo[0] > hi[0] {
        return None;
    }
    let (cx, cy) = ((lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5);
    let (w, h) = (hi[0] - lo[0], hi[1] - lo[1]);
    let (l, r, b, tp) = if opts.world_size > 0.0 {
        // One scale for all: centred across, standing on the bottom margin.
        let half = opts.world_size * 0.5;
        let margin = opts.world_size * 0.03;
        let b = lo[1] - margin;
        if w > opts.world_size || h + margin > opts.world_size {
            eprintln!("EXTENT overflow {w:.0}x{h:.0} > {}", opts.world_size);
        }
        (cx - half, cx + half, b, b + opts.world_size)
    } else {
        let half = (w.max(h) * 0.5 * 1.06).max(1.0);
        (cx - half, cx + half, cy - half, cy + half)
    };
    eprintln!("EXTENT {w:.0} {h:.0}");
    let (n, f) = (-hi[2] - 50.0, -lo[2] + 50.0);
    let mut o = [0f32; 16];
    o[0] = 2.0 / (r - l);
    o[5] = 2.0 / (tp - b);
    o[10] = 1.0 / (n - f);
    o[12] = -(r + l) / (r - l);
    o[13] = -(tp + b) / (tp - b);
    o[14] = n / (n - f);
    o[15] = 1.0;
    let light = [dir[0] * 0.8 + dir[1] * 0.35, dir[1] * 0.8 - dir[0] * 0.35, 0.9];
    Some(View { view_proj: mat_mul(&o, &view), light })
}

fn render_at(app: &mut StudioApp, view: &View, t: f32, size: u32, opts: &ExportOpts) -> Option<Vec<u8>> {
    let skinned = skinned_at(app, t)?;
    if std::env::var_os("SDS_SEAM").is_some() {
        let head = skinned.iter().find(|s| s.texture.contains("#face"));
        let body = skinned.iter().find(|s| s.texture.to_ascii_lowercase().contains("malebody"));
        if let (Some(h), Some(b)) = (head, body) {
            let mut d: Vec<f32> = h
                .verts
                .iter()
                .map(|v| b.verts.iter().map(|w| (0..3).map(|k| (v.pos[k] - w.pos[k]).powi(2)).sum::<f32>()).fold(f32::MAX, f32::min).sqrt())
                .filter(|d| *d < 3.0)
                .collect();
            d.sort_by(|a, b| a.partial_cmp(b).unwrap());
            eprintln!("SEAM t={t:.2} n={} min={:?} med={:?} max={:?}", d.len(), d.first(), d.get(d.len() / 2), d.last());
        }
    }
    let meshes: Vec<DrawMesh> = skinned.iter().map(|s| s.draw()).collect();
    let frame = Frame { view_proj: view.view_proj, light: view.light, meshes, lines: Vec::new(), overlay: Vec::new(), fx: Vec::new(), clear: [0.0; 3], clear_alpha: 0.0, ambient: opts.ambient };
    let vfs = app.loaded.as_ref().map(|l| l.vfs.clone());
    let mut load = |path: &str| -> Option<(u32, u32, Vec<u8>)> {
        let img = crate::character::load_texture(vfs.as_ref()?, path);
        if img.is_none() && std::env::var_os("SDS_TEX_DEBUG").is_some() {
            eprintln!("TEX missing {path}");
        }
        img
    };
    let gpu = app.gpu.as_mut()?;
    Some(gpu.capture((size, size), &frame, &mut load))
}

fn duration(app: &StudioApp) -> f32 {
    app.doc.as_ref().map_or(0.0, |d| d.duration())
}

fn gif_times(app: &StudioApp, opts: &ExportOpts) -> (Vec<f32>, f32) {
    let dur = duration(app);
    let mut fps = opts.fps.max(1.0);
    let mut n = (dur * fps).round().max(1.0) as u32;
    if n > opts.max_frames {
        n = opts.max_frames.max(1);
        fps = n as f32 / dur.max(1e-3);
    }
    ((0..n).map(|i| i as f32 / fps).collect(), fps)
}

/// PNG of the pose at `t` (None = the middle of the clip).
pub fn export_png(app: &mut StudioApp, path: &Path, t: Option<f32>, opts: &ExportOpts) -> Result<(), String> {
    if app.character.is_none() {
        return Err(sds_i18n::t!("персонаж не загружен", "no character loaded").into());
    }
    crate::paired::sync(app);
    let t = t.unwrap_or(duration(app) * 0.5);
    // Framed over the whole clip, so a PNG matches its GIF.
    let (times, _) = gif_times(app, opts);
    let view = frame_view(app, opts, &times).ok_or("empty mesh")?;
    let rgba = render_at(app, &view, t, opts.png_size, opts).ok_or("render failed")?;
    let png = aether_core::png::encode_rgba(opts.png_size, opts.png_size, &rgba).ok_or("png encode")?;
    std::fs::write(path, png).map_err(|e| e.to_string())
}

/// Animated GIF of the whole clip, looping, transparent background.
pub fn export_gif(app: &mut StudioApp, path: &Path, opts: &ExportOpts) -> Result<usize, String> {
    if app.character.is_none() {
        return Err(sds_i18n::t!("персонаж не загружен", "no character loaded").into());
    }
    crate::paired::sync(app);
    let (times, fps) = gif_times(app, opts);
    let view = frame_view(app, opts, &times).ok_or("empty mesh")?;
    let mut frames = Vec::with_capacity(times.len());
    for &t in &times {
        frames.push(render_at(app, &view, t, opts.gif_size, opts).ok_or("render failed")?);
    }
    let delay = (100.0 / fps).round().max(2.0) as u16;
    let bytes = gif::encode(opts.gif_size as u16, opts.gif_size as u16, &frames, delay);
    std::fs::write(path, bytes).map_err(|e| e.to_string())?;
    Ok(frames.len())
}

/// Minimal GIF89a writer: one global palette (median cut over all frames),
/// index 0 transparent, disposal "restore to background", infinite loop.
pub mod gif {
    const ALPHA_CUT: u8 = 128;

    pub fn encode(w: u16, h: u16, frames: &[Vec<u8>], delay_cs: u16) -> Vec<u8> {
        let palette = median_cut(frames, 255);
        let mut map = Mapper::new(&palette);
        let mut out = Vec::new();
        out.extend_from_slice(b"GIF89a");
        out.extend_from_slice(&w.to_le_bytes());
        out.extend_from_slice(&h.to_le_bytes());
        out.extend_from_slice(&[0xF7, 0, 0]);
        // Global table: 0 = transparent, then the palette, padded to 256.
        let mut table = vec![0u8; 256 * 3];
        for (i, c) in palette.iter().enumerate() {
            table[(i + 1) * 3..(i + 2) * 3].copy_from_slice(c);
        }
        out.extend_from_slice(&table);
        out.extend_from_slice(&[0x21, 0xFF, 0x0B]);
        out.extend_from_slice(b"NETSCAPE2.0");
        out.extend_from_slice(&[0x03, 0x01, 0x00, 0x00, 0x00]);
        for f in frames {
            out.extend_from_slice(&[0x21, 0xF9, 0x04, (2 << 2) | 1]);
            out.extend_from_slice(&delay_cs.to_le_bytes());
            out.extend_from_slice(&[0, 0]);
            out.push(0x2C);
            out.extend_from_slice(&[0, 0, 0, 0]);
            out.extend_from_slice(&w.to_le_bytes());
            out.extend_from_slice(&h.to_le_bytes());
            out.push(0);
            let idx: Vec<u8> = f.chunks_exact(4).map(|p| if p[3] < ALPHA_CUT { 0 } else { map.index([p[0], p[1], p[2]]) }).collect();
            out.push(8);
            for block in lzw(&idx, 8).chunks(255) {
                out.push(block.len() as u8);
                out.extend_from_slice(block);
            }
            out.push(0);
        }
        out.push(0x3B);
        out
    }

    fn median_cut(frames: &[Vec<u8>], colors: usize) -> Vec<[u8; 3]> {
        let total: usize = frames.iter().map(|f| f.len() / 4).sum();
        let step = (total / 400_000).max(1);
        let mut px: Vec<[u8; 3]> = Vec::new();
        let mut k = 0usize;
        for f in frames {
            for p in f.chunks_exact(4) {
                k += 1;
                if p[3] >= ALPHA_CUT && k % step == 0 {
                    px.push([p[0], p[1], p[2]]);
                }
            }
        }
        if px.is_empty() {
            return vec![[0, 0, 0]];
        }
        let mut boxes: Vec<Vec<[u8; 3]>> = vec![px];
        while boxes.len() < colors {
            // Split the box with the widest channel range (weighted by size).
            let mut best = None;
            for (i, b) in boxes.iter().enumerate() {
                if b.len() < 2 {
                    continue;
                }
                let (ch, range) = widest(b);
                let score = range as usize * (b.len() as f64).sqrt() as usize;
                if best.is_none_or(|(_, _, s)| score > s) && range > 0 {
                    best = Some((i, ch, score));
                }
            }
            let Some((i, ch, _)) = best else { break };
            let mut b = boxes.swap_remove(i);
            b.sort_unstable_by_key(|c| c[ch]);
            let hi = b.split_off(b.len() / 2);
            boxes.push(b);
            boxes.push(hi);
        }
        boxes
            .iter()
            .map(|b| {
                let mut s = [0u64; 3];
                for c in b {
                    for k in 0..3 {
                        s[k] += c[k] as u64;
                    }
                }
                let n = b.len().max(1) as u64;
                [(s[0] / n) as u8, (s[1] / n) as u8, (s[2] / n) as u8]
            })
            .collect()
    }

    fn widest(b: &[[u8; 3]]) -> (usize, u8) {
        let (mut lo, mut hi) = ([255u8; 3], [0u8; 3]);
        for c in b {
            for k in 0..3 {
                lo[k] = lo[k].min(c[k]);
                hi[k] = hi[k].max(c[k]);
            }
        }
        (0..3).map(|k| (k, hi[k] - lo[k])).max_by_key(|x| x.1).unwrap()
    }

    /// Nearest palette entry, cached per 6-bit RGB cell. Indices are +1 (0 is transparent).
    struct Mapper<'a> {
        pal: &'a [[u8; 3]],
        cache: Vec<u8>,
    }

    impl<'a> Mapper<'a> {
        fn new(pal: &'a [[u8; 3]]) -> Self {
            Mapper { pal, cache: vec![0; 1 << 18] }
        }
        fn index(&mut self, c: [u8; 3]) -> u8 {
            let key = ((c[0] as usize >> 2) << 12) | ((c[1] as usize >> 2) << 6) | (c[2] as usize >> 2);
            if self.cache[key] == 0 {
                let q = [(c[0] & 0xFC) as i32 + 2, (c[1] & 0xFC) as i32 + 2, (c[2] & 0xFC) as i32 + 2];
                let best = self
                    .pal
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, p)| {
                        let d = |k: usize| (p[k] as i32 - q[k]).pow(2);
                        2 * d(0) + 4 * d(1) + 3 * d(2)
                    })
                    .map_or(0, |(i, _)| i);
                self.cache[key] = best as u8 + 1;
            }
            self.cache[key]
        }
    }

    /// GIF-flavoured variable-width LZW (LSB-first codes, clear on a full table).
    pub fn lzw(data: &[u8], min: u8) -> Vec<u8> {
        let clear = 1u32 << min;
        let eoi = clear + 1;
        let mut size = min as u32 + 1;
        let mut next = eoi + 1;
        let mut dict: std::collections::HashMap<(u32, u8), u32> = std::collections::HashMap::new();
        let mut out = Vec::new();
        let (mut acc, mut nbits) = (0u64, 0u32);
        let mut emit = |code: u32, size: u32, out: &mut Vec<u8>| {
            acc |= (code as u64) << nbits;
            nbits += size;
            while nbits >= 8 {
                out.push(acc as u8);
                acc >>= 8;
                nbits -= 8;
            }
        };
        emit(clear, size, &mut out);
        let mut cur: Option<u32> = None;
        for &b in data {
            let Some(c) = cur else {
                cur = Some(b as u32);
                continue;
            };
            if let Some(&n) = dict.get(&(c, b)) {
                cur = Some(n);
                continue;
            }
            emit(c, size, &mut out);
            if next == 4096 {
                emit(clear, size, &mut out);
                dict.clear();
                next = eoi + 1;
                size = min as u32 + 1;
            } else {
                dict.insert((c, b), next);
                next += 1;
                if next > (1 << size) && size < 12 {
                    size += 1;
                }
            }
            cur = Some(b as u32);
        }
        if let Some(c) = cur {
            emit(c, size, &mut out);
        }
        emit(eoi, size, &mut out);
        if nbits > 0 {
            out.push(acc as u8);
        }
        out
    }
}

/// `--export-batch <list.txt> --export-dir <dir>`: one line per clip, either
/// a VFS path or a clip name (file stem, `.hkx` optional) resolved against
/// the load order's animations. Each clip gets `<name>.png` (pose at the
/// middle) and `<name>.gif`; `index.csv` lists what was resolved.
pub struct Batch {
    pub queries: Vec<String>,
    pub out: std::path::PathBuf,
    pub next: usize,
    pub report: Vec<String>,
    /// `--npc <EditorID>`: dress the actor as this NPC (empty hands) first.
    pub npc: Option<String>,
}

impl Batch {
    pub fn from_file(list: &Path, out: std::path::PathBuf) -> std::io::Result<Batch> {
        let text = std::fs::read_to_string(list)?;
        let queries = text.lines().map(|l| l.trim().trim_start_matches('\u{feff}').to_string()).filter(|l| !l.is_empty() && !l.starts_with('#')).collect();
        std::fs::create_dir_all(&out)?;
        Ok(Batch { queries, out, next: 0, report: vec!["name;status;vfs path;candidates;frames".into()], npc: None })
    }
}

/// Lower-case file stem of a VFS path.
fn stem(p: &str) -> String {
    let f = crate::library::file_name(p).to_ascii_lowercase();
    f.strip_suffix(".hkx").unwrap_or(&f).to_string()
}

/// The clip a name means: same stem, the plain character folder before an
/// OAR/DAR replacer and before creatures, shortest path first.
pub fn resolve(anims: &[String], query: &str) -> (Option<String>, usize) {
    let q = query.trim().to_ascii_lowercase().replace('\\', "/");
    if q.contains('/') {
        return (anims.iter().find(|a| a.to_ascii_lowercase().replace('\\', "/") == q).cloned(), 1);
    }
    let q = q.strip_suffix(".hkx").unwrap_or(&q).to_string();
    let mut c: Vec<&String> = anims.iter().filter(|a| stem(a) == q).collect();
    let rank = |a: &str| {
        let l = a.to_ascii_lowercase().replace('\\', "/");
        let replacer = l.contains("openanimationreplacer") || l.contains("dynamicanimationreplacer");
        (!l.starts_with("meshes/actors/character/"), replacer, l.len())
    };
    c.sort_by_key(|a| rank(a));
    (c.first().map(|s| s.to_string()), c.len())
}

fn safe_name(q: &str) -> String {
    let q = q.rsplit(['/', '\\']).next().unwrap_or(q);
    let q = q.strip_suffix(".hkx").unwrap_or(q);
    q.chars().map(|c| if c.is_alphanumeric() || "-_ .".contains(c) { c } else { '_' }).collect()
}

/// Runs one clip of the batch per frame; closes the app when done.
pub fn run_batch(app: &mut StudioApp, ctx: &egui::Context) {
    if app.batch.is_none() || app.loaded.is_none() || app.loading.is_some() || app.gpu.is_none() {
        return;
    }
    ctx.request_repaint();
    if let Some(edid) = app.batch.as_mut().unwrap().npc.take().filter(|e| !e.is_empty()) {
        app.set_npc_by_edid(&edid, false);
    }
    let (query, out) = {
        let b = app.batch.as_ref().unwrap();
        match b.queries.get(b.next) {
            Some(q) => (q.clone(), b.out.clone()),
            None => {
                let _ = std::fs::write(b.out.join("index.csv"), b.report.join("\n"));
                if std::env::args().any(|a| a == "--dump-anims") {
                    let _ = std::fs::write(b.out.join("anims.txt"), app.loaded.as_ref().unwrap().library.anims.join("\n"));
                }
                eprintln!("BATCH done -> {}", b.out.display());
                for l in &app.log {
                    eprintln!("LOG {l}");
                }
                app.batch = None;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }
        }
    };
    app.batch.as_mut().unwrap().next += 1;
    // `name|clip`: the output file is called `name`, the clip is looked up by `clip`.
    let (label, query) = match query.split_once('|') {
        Some((l, q)) => (l.trim().to_string(), q.trim().to_string()),
        None => (query.clone(), query),
    };
    let (path, n) = resolve(&app.loaded.as_ref().unwrap().library.anims, &query);
    let name = safe_name(&label);
    let query = label;
    let line = match path {
        None => format!("{query};not found;;0;0"),
        Some(p) => {
            app.open_vfs_file(&p);
            let opts = app.settings.export.clone();
            let png = export_png(app, &out.join(format!("{name}.png")), None, &opts);
            let gif = export_gif(app, &out.join(format!("{name}.gif")), &opts);
            match (png, gif) {
                (Ok(()), Ok(f)) => format!("{query};ok;{p};{n};{f}"),
                (a, b) => format!("{query};error {:?} {:?};{p};{n};0", a.err(), b.err()),
            }
        }
    };
    eprintln!("BATCH {line}");
    app.batch.as_mut().unwrap().report.push(line);
}

/// The viewport's export buttons: PNG of the current pose, GIF of the clip.
pub fn toolbar(app: &mut StudioApp, ui: &mut egui::Ui) {
    let can = app.character.is_some() && app.doc.is_some() && app.gpu.is_some();
    let base = app.doc.as_ref().map(|d| safe_name(&d.name)).unwrap_or_default();
    if ui.add_enabled(can, egui::Button::new(sds_i18n::t!("PNG позы", "Pose PNG"))).on_hover_text(sds_i18n::t!("Снимок текущей позы на прозрачном фоне", "Snapshot of the current pose on a transparent background")).clicked() {
        if let Some(p) = rfd::FileDialog::new().add_filter("PNG", &["png"]).set_file_name(format!("{base}.png")).save_file() {
            let opts = app.settings.export.clone();
            let t = app.play.time;
            match export_png(app, &p, Some(t), &opts) {
                Ok(()) => app.log(sds_i18n::tf!("PNG сохранён: {}", "PNG saved: {}", p.display())),
                Err(e) => app.log(sds_i18n::tf!("PNG не сохранён: {e}", "PNG not saved: {e}")),
            }
        }
    }
    if ui.add_enabled(can, egui::Button::new(sds_i18n::t!("Запись GIF", "Record GIF"))).on_hover_text(sds_i18n::t!("Весь клип в анимированный GIF, актёр на прозрачном фоне", "The whole clip as an animated GIF, the actor on a transparent background")).clicked() {
        if let Some(p) = rfd::FileDialog::new().add_filter("GIF", &["gif"]).set_file_name(format!("{base}.gif")).save_file() {
            let opts = app.settings.export.clone();
            match export_gif(app, &p, &opts) {
                Ok(n) => app.log(sds_i18n::tf!("GIF сохранён: {} ({n} кадров)", "GIF saved: {} ({n} frames)", p.display())),
                Err(e) => app.log(sds_i18n::tf!("GIF не сохранён: {e}", "GIF not saved: {e}")),
            }
        }
    }
    ui.menu_button("⚙", |ui| {
        let o = &mut app.settings.export;
        ui.add(egui::Slider::new(&mut o.gif_size, 64..=1024).text(sds_i18n::t!("GIF, px", "GIF, px")));
        ui.add(egui::Slider::new(&mut o.png_size, 64..=2048).text(sds_i18n::t!("PNG, px", "PNG, px")));
        ui.add(egui::Slider::new(&mut o.fps, 5.0..=50.0).text(sds_i18n::t!("кадров/с", "fps")));
        ui.add(egui::Slider::new(&mut o.yaw, -180.0..=180.0).text(sds_i18n::t!("поворот камеры, °", "camera yaw, °")));
        ui.add(egui::Slider::new(&mut o.pitch, -30.0..=60.0).text(sds_i18n::t!("высота камеры, °", "camera pitch, °")));
        ui.add(egui::Slider::new(&mut o.max_frames, 10..=600).text(sds_i18n::t!("макс. кадров GIF", "max GIF frames")));
        ui.add(egui::Slider::new(&mut o.ambient, 0.0..=1.0).text(sds_i18n::t!("подсветка превью", "preview ambient")));
        ui.add(egui::Slider::new(&mut o.world_size, 0.0..=600.0).text(sds_i18n::t!("кадр, ед. игры (0 = по клипу)", "frame, game units (0 = per clip)")));
    });
}
