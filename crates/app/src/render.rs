//! The 3D viewport's GPU side, the way Aether does it (its gpu.rs, "Option
//! B"): egui's pass has no depth buffer, so the scene renders in a pass of
//! our own into an offscreen MSAA target with depth, resolved into a texture
//! egui shows as an image. The encoder is submitted inside `App::ui`, before
//! egui's own frame, so the picture is never a frame late.
//!
//! Skinned vertices change every frame, so vertex buffers are rewritten per
//! draw; index buffers, textures and material bind groups are cached by the
//! caller's mesh id.

use eframe::egui_wgpu::RenderState;
use eframe::wgpu;
use std::collections::HashMap;

const MSAA: u32 = 4;
const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct MeshVertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
}

#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct LineVertex {
    pub pos: [f32; 3],
    pub color: [f32; 4],
}

/// A vertex of a transparent effect batch (particles, effect meshes, trails).
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct FxVertex {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
    pub color: [f32; 4],
    /// 1 = additive, 0 = alpha blend.
    pub additive: f32,
    /// Greyscale bits: 1 colour from the palette, 2 alpha from the palette.
    pub mode: f32,
    /// (base colour scale, colour ramp row factor, alpha ramp factor).
    pub grey: [f32; 3],
}

pub struct FxBatch {
    pub verts: Vec<FxVertex>,
    pub indices: Vec<u32>,
    /// Game-relative texture; empty = white.
    pub texture: String,
    /// Greyscale palette texture (empty = none).
    pub palette: String,
}

pub struct DrawMesh<'a> {
    pub id: u64,
    pub vertices: &'a [MeshVertex],
    pub indices: &'a [u16],
    pub texture: &'a str,
    pub alpha_test: f32,
    pub tint: [f32; 4],
}

pub struct Frame<'a> {
    pub view_proj: [f32; 16],
    pub light: [f32; 3],
    pub meshes: Vec<DrawMesh<'a>>,
    /// Depth-tested lines (grid, path).
    pub lines: Vec<LineVertex>,
    /// Lines drawn over everything (skeleton, markers).
    pub overlay: Vec<LineVertex>,
    /// Transparent effects, after the opaque meshes.
    pub fx: Vec<FxBatch>,
    pub clear: [f64; 3],
}

fn bytes<T>(v: &[T]) -> &[u8] {
    // SAFETY: only used with the repr(C) all-f32 vertex structs above and
    // u16 indices: no padding, no invalid bit patterns, read-only borrow.
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) }
}

struct Target {
    msaa: wgpu::TextureView,
    depth: wgpu::TextureView,
    resolve: wgpu::TextureView,
    size: (u32, u32),
    egui_id: egui::TextureId,
}

struct MeshCache {
    vbuf: wgpu::Buffer,
    vcap: usize,
    ibuf: wgpu::Buffer,
    icount: u32,
    bind: wgpu::BindGroup,
}

struct Texture {
    view: wgpu::TextureView,
    fallback: bool,
}

pub struct Gpu {
    rs: RenderState,
    mesh_pipeline: wgpu::RenderPipeline,
    mesh_pipeline_2s: wgpu::RenderPipeline,
    line_pipeline: wgpu::RenderPipeline,
    overlay_pipeline: wgpu::RenderPipeline,
    camera_buf: wgpu::Buffer,
    camera_bind: wgpu::BindGroup,
    material_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    textures: HashMap<String, Texture>,
    meshes: HashMap<u64, MeshCache>,
    lines_buf: Option<(wgpu::Buffer, usize)>,
    fx_pipeline: wgpu::RenderPipeline,
    fx_layout: wgpu::BindGroupLayout,
    fx_binds: HashMap<String, wgpu::BindGroup>,
    clamp_sampler: wgpu::Sampler,
    fx_vbuf: Option<(wgpu::Buffer, usize)>,
    fx_ibuf: Option<(wgpu::Buffer, usize)>,
    overlay_buf: Option<(wgpu::Buffer, usize)>,
    target: Option<Target>,
}

impl Gpu {
    pub fn new(rs: RenderState) -> Gpu {
        let device = &rs.device;
        let shader = device.create_shader_module(wgpu::include_wgsl!("render.wgsl"));
        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sds-camera-layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sds-material-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
            ],
        });
        let camera_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sds-camera"),
            size: 80,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let camera_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sds-camera-bind"),
            layout: &camera_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: camera_buf.as_entire_binding() }],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("sds-wrap"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 8,
            ..Default::default()
        });

        let mesh_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sds-mesh-layout"),
            bind_group_layouts: &[Some(&camera_layout), Some(&material_layout)],
            immediate_size: 0,
        });
        let line_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sds-line-layout"),
            bind_group_layouts: &[Some(&camera_layout)],
            immediate_size: 0,
        });
        let blend_target = [Some(wgpu::ColorTargetState {
            format: COLOR_FORMAT,
            blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
            write_mask: wgpu::ColorWrites::ALL,
        })];
        let depth = |write: bool, compare: wgpu::CompareFunction| {
            Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(write),
                depth_compare: Some(compare),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            })
        };
        let mesh_pipe = |cull: Option<wgpu::Face>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("sds-mesh"),
                layout: Some(&mesh_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_mesh"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<MeshVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2],
                    }],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_mesh"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &blend_target,
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: cull,
                    ..Default::default()
                },
                depth_stencil: depth(true, wgpu::CompareFunction::Less),
                multisample: wgpu::MultisampleState { count: MSAA, ..Default::default() },
                multiview_mask: None,
                cache: None,
            })
        };
        let mesh_pipeline = mesh_pipe(Some(wgpu::Face::Back));
        let mesh_pipeline_2s = mesh_pipe(None);
        let line_pipe = |tested: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("sds-line"),
                layout: Some(&line_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_line"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<LineVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4],
                    }],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_line"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &blend_target,
                }),
                primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::LineList, ..Default::default() },
                depth_stencil: if tested {
                    depth(false, wgpu::CompareFunction::LessEqual)
                } else {
                    depth(false, wgpu::CompareFunction::Always)
                },
                multisample: wgpu::MultisampleState { count: MSAA, ..Default::default() },
                multiview_mask: None,
                cache: None,
            })
        };
        let fx_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sds-fx-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let fx_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sds-fx-pl"),
            bind_group_layouts: &[Some(&camera_layout), Some(&fx_layout)],
            immediate_size: 0,
        });
        let fx_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sds-fx"),
            layout: Some(&fx_pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_fx"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<FxVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32x4, 3 => Float32, 4 => Float32, 5 => Float32x3],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_fx"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &blend_target,
            }),
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, cull_mode: None, ..Default::default() },
            depth_stencil: depth(false, wgpu::CompareFunction::LessEqual),
            multisample: wgpu::MultisampleState { count: MSAA, ..Default::default() },
            multiview_mask: None,
            cache: None,
        });
        let clamp_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("sds-clamp"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let line_pipeline = line_pipe(true);
        let overlay_pipeline = line_pipe(false);
        Gpu {
            rs,
            mesh_pipeline,
            mesh_pipeline_2s,
            line_pipeline,
            overlay_pipeline,
            camera_buf,
            camera_bind,
            material_layout,
            sampler,
            textures: HashMap::new(),
            meshes: HashMap::new(),
            lines_buf: None,
            fx_pipeline,
            fx_layout,
            fx_binds: HashMap::new(),
            clamp_sampler,
            fx_vbuf: None,
            fx_ibuf: None,
            overlay_buf: None,
            target: None,
        }
    }

    /// Forget cached meshes (a new character was loaded).
    pub fn clear_meshes(&mut self) {
        self.meshes.clear();
    }

    fn texture(&mut self, path: &str, load: &mut dyn FnMut(&str) -> Option<(u32, u32, Vec<u8>)>) -> &Texture {
        if !self.textures.contains_key(path) {
            let (img, fallback) = match (!path.is_empty()).then(|| load(path)).flatten() {
                Some(img) => (img, false),
                None => ((1, 1, vec![255, 255, 255, 255]), true),
            };
            let (w, h, rgba) = img;
            let levels = mip_chain(w, h, rgba);
            let device = &self.rs.device;
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("sds-diffuse"),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: levels.len() as u32,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: COLOR_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            for (level, (lw, lh, data)) in levels.iter().enumerate() {
                self.rs.queue.write_texture(
                    wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: level as u32, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                    data,
                    wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * lw), rows_per_image: None },
                    wgpu::Extent3d { width: *lw, height: *lh, depth_or_array_layers: 1 },
                );
            }
            let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
            self.textures.insert(path.to_string(), Texture { view, fallback });
        }
        &self.textures[path]
    }

    fn ensure_target(&mut self, size: (u32, u32)) {
        if self.target.as_ref().is_some_and(|t| t.size == size) {
            return;
        }
        let device = &self.rs.device;
        let base = wgpu::TextureDescriptor {
            label: Some("sds-viewport"),
            size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: MSAA,
            dimension: wgpu::TextureDimension::D2,
            format: COLOR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        };
        let msaa = device.create_texture(&base).create_view(&wgpu::TextureViewDescriptor::default());
        let depth = device
            .create_texture(&wgpu::TextureDescriptor { format: DEPTH_FORMAT, ..base })
            .create_view(&wgpu::TextureViewDescriptor::default());
        let resolve = device
            .create_texture(&wgpu::TextureDescriptor {
                sample_count: 1,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                ..base
            })
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut renderer = self.rs.renderer.write();
        let egui_id = match self.target.take() {
            Some(old) => {
                renderer.update_egui_texture_from_wgpu_texture(device, &resolve, wgpu::FilterMode::Linear, old.egui_id);
                old.egui_id
            }
            None => renderer.register_native_texture(device, &resolve, wgpu::FilterMode::Linear),
        };
        self.target = Some(Target { msaa, depth, resolve, size, egui_id });
    }

    fn line_buffer(device: &wgpu::Device, slot: &mut Option<(wgpu::Buffer, usize)>, n: usize) {
        if slot.as_ref().is_none_or(|(_, cap)| *cap < n) {
            let cap = n.next_power_of_two().max(1024);
            *slot = Some((
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("sds-lines"),
                    size: (cap * std::mem::size_of::<LineVertex>()) as u64,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                cap,
            ));
        }
    }

    /// Render a frame; returns the texture to show.
    pub fn render(
        &mut self,
        size: (u32, u32),
        frame: &Frame,
        load_texture: &mut dyn FnMut(&str) -> Option<(u32, u32, Vec<u8>)>,
    ) -> egui::TextureId {
        let size = (size.0.max(1), size.1.max(1));
        self.ensure_target(size);

        // Camera.
        let mut cam = [0f32; 20];
        cam[..16].copy_from_slice(&frame.view_proj);
        cam[16..19].copy_from_slice(&frame.light);
        self.rs.queue.write_buffer(&self.camera_buf, 0, bytes(&cam));

        // Meshes: (re)upload vertices, cache indices and materials.
        for m in &frame.meshes {
            let fallback = self.texture(m.texture, load_texture).fallback;
            let device = self.rs.device.clone();
            let needs_new = self.meshes.get(&m.id).is_none_or(|c| c.vcap < m.vertices.len());
            if needs_new {
                let view = &self.textures[m.texture].view;
                let mut indices = m.indices.to_vec();
                if indices.len() % 2 == 1 {
                    indices.push(0);
                }
                let ibuf = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("sds-index"),
                    size: (indices.len() * 2).max(4) as u64,
                    usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                self.rs.queue.write_buffer(&ibuf, 0, bytes(&indices));
                let vcap = m.vertices.len().max(1);
                let vbuf = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("sds-vertex"),
                    size: (vcap * std::mem::size_of::<MeshVertex>()) as u64,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                let mut params = [0f32; 8];
                params[..4].copy_from_slice(&m.tint);
                params[4] = m.alpha_test;
                params[5] = if fallback { 1.0 } else { 0.0 };
                let ubuf = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("sds-material"),
                    size: 32,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                self.rs.queue.write_buffer(&ubuf, 0, bytes(&params));
                let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("sds-material-bind"),
                    layout: &self.material_layout,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) },
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                        wgpu::BindGroupEntry { binding: 2, resource: ubuf.as_entire_binding() },
                    ],
                });
                self.meshes.insert(m.id, MeshCache { vbuf, vcap, ibuf, icount: m.indices.len() as u32, bind });
            }
            let c = &self.meshes[&m.id];
            self.rs.queue.write_buffer(&c.vbuf, 0, bytes(m.vertices));
        }

        let device = self.rs.device.clone();
        Self::line_buffer(&device, &mut self.lines_buf, frame.lines.len());
        Self::line_buffer(&device, &mut self.overlay_buf, frame.overlay.len());
        if !frame.lines.is_empty() {
            self.rs.queue.write_buffer(&self.lines_buf.as_ref().unwrap().0, 0, bytes(&frame.lines));
        }
        if !frame.overlay.is_empty() {
            self.rs.queue.write_buffer(&self.overlay_buf.as_ref().unwrap().0, 0, bytes(&frame.overlay));
        }

        // Effects: one vertex and one index buffer per frame, a draw range per batch.
        let mut fx_ranges: Vec<(String, u32, u32, i32)> = Vec::new();
        if !frame.fx.is_empty() {
            let mut verts: Vec<FxVertex> = Vec::new();
            let mut idx: Vec<u32> = Vec::new();
            for b in &frame.fx {
                if b.indices.is_empty() {
                    continue;
                }
                let base = verts.len() as i32;
                let first = idx.len() as u32;
                verts.extend_from_slice(&b.verts);
                idx.extend_from_slice(&b.indices);
                fx_ranges.push((format!("{}|{}", b.texture, b.palette), first, b.indices.len() as u32, base));
            }
            let grow = |slot: &mut Option<(wgpu::Buffer, usize)>, bytes_len: usize, usage: wgpu::BufferUsages, device: &wgpu::Device| {
                if slot.as_ref().is_none_or(|(_, cap)| *cap < bytes_len) {
                    let cap = bytes_len.next_power_of_two().max(4096);
                    *slot = Some((device.create_buffer(&wgpu::BufferDescriptor { label: Some("sds-fx"), size: cap as u64, usage: usage | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false }), cap));
                }
            };
            grow(&mut self.fx_vbuf, std::mem::size_of_val(verts.as_slice()), wgpu::BufferUsages::VERTEX, &device);
            grow(&mut self.fx_ibuf, idx.len() * 4, wgpu::BufferUsages::INDEX, &device);
            if !verts.is_empty() {
                self.rs.queue.write_buffer(&self.fx_vbuf.as_ref().unwrap().0, 0, bytes(&verts));
                self.rs.queue.write_buffer(&self.fx_ibuf.as_ref().unwrap().0, 0, bytes(&idx));
            }
            for (key, ..) in &fx_ranges {
                if !self.fx_binds.contains_key(key) {
                    let (tex, pal) = key.split_once('|').unwrap_or((key.as_str(), ""));
                    self.texture(tex, load_texture);
                    self.texture(pal, load_texture);
                    let view = &self.textures[tex].view;
                    let pview = &self.textures[pal].view;
                    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("sds-fx-bind"),
                        layout: &self.fx_layout,
                        entries: &[
                            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) },
                            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(pview) },
                            wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&self.clamp_sampler) },
                        ],
                    });
                    self.fx_binds.insert(key.clone(), bind);
                }
            }
        }

        let target = self.target.as_ref().unwrap();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("sds-viewport") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("sds-scene"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.msaa,
                    resolve_target: Some(&target.resolve),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: frame.clear[0], g: frame.clear[1], b: frame.clear[2], a: 1.0 }),
                        store: wgpu::StoreOp::Discard,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &target.depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.camera_bind, &[]);
            for m in &frame.meshes {
                let c = &self.meshes[&m.id];
                if c.icount == 0 {
                    continue;
                }
                // Skyrim bodies are single-sided; alpha-tested cards (hair) are not.
                pass.set_pipeline(if m.alpha_test > 0.0 { &self.mesh_pipeline_2s } else { &self.mesh_pipeline });
                pass.set_bind_group(1, &c.bind, &[]);
                pass.set_vertex_buffer(0, c.vbuf.slice(..));
                pass.set_index_buffer(c.ibuf.slice(..), wgpu::IndexFormat::Uint16);
                pass.draw_indexed(0..c.icount, 0, 0..1);
            }
            if !fx_ranges.is_empty() {
                pass.set_pipeline(&self.fx_pipeline);
                pass.set_vertex_buffer(0, self.fx_vbuf.as_ref().unwrap().0.slice(..));
                pass.set_index_buffer(self.fx_ibuf.as_ref().unwrap().0.slice(..), wgpu::IndexFormat::Uint32);
                for (tex, first, count, base) in &fx_ranges {
                    pass.set_bind_group(1, &self.fx_binds[tex], &[]);
                    pass.draw_indexed(*first..*first + *count, *base, 0..1);
                }
            }
            if !frame.lines.is_empty() {
                pass.set_pipeline(&self.line_pipeline);
                pass.set_vertex_buffer(0, self.lines_buf.as_ref().unwrap().0.slice(..));
                pass.draw(0..frame.lines.len() as u32, 0..1);
            }
            if !frame.overlay.is_empty() {
                pass.set_pipeline(&self.overlay_pipeline);
                pass.set_vertex_buffer(0, self.overlay_buf.as_ref().unwrap().0.slice(..));
                pass.draw(0..frame.overlay.len() as u32, 0..1);
            }
        }
        self.rs.queue.submit(Some(encoder.finish()));
        target.egui_id
    }
}

/// Box-filtered mip levels, full size first.
fn mip_chain(w: u32, h: u32, rgba: Vec<u8>) -> Vec<(u32, u32, Vec<u8>)> {
    let mut out = vec![(w, h, rgba)];
    loop {
        let (pw, ph, prev) = out.last().unwrap();
        let (pw, ph) = (*pw, *ph);
        if pw == 1 && ph == 1 {
            break;
        }
        let (nw, nh) = ((pw / 2).max(1), (ph / 2).max(1));
        let mut next = vec![0u8; (nw * nh * 4) as usize];
        for y in 0..nh {
            for x in 0..nw {
                for c in 0..4 {
                    let mut sum = 0u32;
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        let sx = (x * 2 + dx).min(pw - 1);
                        let sy = (y * 2 + dy).min(ph - 1);
                        sum += prev[((sy * pw + sx) * 4 + c) as usize] as u32;
                    }
                    next[((y * nw + x) * 4 + c) as usize] = (sum / 4) as u8;
                }
            }
        }
        out.push((nw, nh, next));
    }
    out
}

// ---- camera maths (column-major, right-handed, Z up, wgpu depth 0..1) -----

pub fn mat_mul(a: &[f32; 16], b: &[f32; 16]) -> [f32; 16] {
    let mut o = [0.0; 16];
    for c in 0..4 {
        for r in 0..4 {
            o[c * 4 + r] = (0..4).map(|k| a[k * 4 + r] * b[c * 4 + k]).sum();
        }
    }
    o
}

pub fn perspective(fovy: f32, aspect: f32, near: f32, far: f32) -> [f32; 16] {
    let f = 1.0 / (fovy * 0.5).tan();
    let mut m = [0.0; 16];
    m[0] = f / aspect;
    m[5] = f;
    m[10] = far / (near - far);
    m[11] = -1.0;
    m[14] = near * far / (near - far);
    m
}

pub fn look_at(eye: [f32; 3], target: [f32; 3], up: [f32; 3]) -> [f32; 16] {
    let sub = |a: [f32; 3], b: [f32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let norm = |v: [f32; 3]| {
        let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-9);
        [v[0] / l, v[1] / l, v[2] / l]
    };
    let cross = |a: [f32; 3], b: [f32; 3]| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let f = norm(sub(target, eye));
    let s = norm(cross(f, up));
    let u = cross(s, f);
    [
        s[0], u[0], -f[0], 0.0, //
        s[1], u[1], -f[1], 0.0, //
        s[2], u[2], -f[2], 0.0, //
        -dot(s, eye), -dot(u, eye), dot(f, eye), 1.0,
    ]
}

/// World point → normalized device coords (x, y in -1..1) and depth; None behind the camera.
pub fn project(m: &[f32; 16], p: [f32; 3]) -> Option<[f32; 3]> {
    let x = m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12];
    let y = m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13];
    let z = m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14];
    let w = m[3] * p[0] + m[7] * p[1] + m[11] * p[2] + m[15];
    if w <= 1e-6 {
        return None;
    }
    Some([x / w, y / w, z / w])
}
