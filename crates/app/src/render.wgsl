// SDS Anim Studio viewport: lit textured meshes and coloured lines.

struct Camera {
    view_proj: mat4x4<f32>,
    // xyz = direction TO the light (world), w unused.
    light: vec4<f32>,
};
@group(0) @binding(0) var<uniform> camera: Camera;

struct Material {
    tint: vec4<f32>,
    // x = alpha-test threshold (0 = off), y = 1 when the texture is the
    // white fallback (no diffuse found).
    params: vec4<f32>,
};
@group(1) @binding(0) var diffuse: texture_2d<f32>;
@group(1) @binding(1) var diffuse_sampler: sampler;
@group(1) @binding(2) var<uniform> material: Material;

struct MeshIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
};

struct MeshOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
};

@vertex
fn vs_mesh(v: MeshIn) -> MeshOut {
    var o: MeshOut;
    o.clip = camera.view_proj * vec4<f32>(v.pos, 1.0);
    o.normal = v.normal;
    o.uv = v.uv;
    return o;
}

@fragment
fn fs_mesh(f: MeshOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let tex = textureSample(diffuse, diffuse_sampler, f.uv);
    if (material.params.x > 0.0 && tex.a < material.params.x) {
        discard;
    }
    var n = normalize(f.normal);
    if (!front) {
        n = -n;
    }
    let l = normalize(camera.light.xyz);
    let diff = max(dot(n, l), 0.0);
    // Soft fill from below-behind so the dark side keeps its shape.
    let fill = max(dot(n, -l), 0.0) * 0.15;
    let shade = 0.55 + 0.75 * diff + fill;
    let base = tex.rgb * material.tint.rgb;
    return vec4<f32>(base * shade, 1.0);
}

struct FxIn {
    @location(0) pos: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) additive: f32,
    @location(4) mode: f32,
    @location(5) grey: vec3<f32>,
};

struct FxOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) additive: f32,
    @location(3) mode: f32,
    @location(4) grey: vec3<f32>,
};

@group(1) @binding(0) var fx_tex: texture_2d<f32>;
@group(1) @binding(1) var fx_sampler: sampler;
@group(1) @binding(2) var fx_ramp: texture_2d<f32>;
@group(1) @binding(3) var fx_ramp_sampler: sampler;

@vertex
fn vs_fx(v: FxIn) -> FxOut {
    var o: FxOut;
    o.clip = camera.view_proj * vec4<f32>(v.pos, 1.0);
    o.uv = v.uv;
    o.color = v.color;
    o.additive = v.additive;
    o.mode = v.mode;
    o.grey = v.grey;
    return o;
}

// Greyscale-to-palette (BSEffectShader SLSF1 bits 4/5), as Effect.hlsl and
// Aether's shader do it: colour = ramp(base.g, colour.r × BaseColor.r) ×
// BaseColorScale; alpha = ramp(base.a, alpha × BaseColor.a).
// grey = (base colour scale, colour row factor, alpha factor).
// Output is premultiplied: additive effects write alpha 0 (src + dst).
@fragment
fn fs_fx(f: FxOut) -> @location(0) vec4<f32> {
    let tex = textureSample(fx_tex, fx_sampler, f.uv);
    var c = tex * f.color;
    let mode = u32(f.mode + 0.5);
    if ((mode & 1u) != 0u) {
        let ramp = textureSampleLevel(fx_ramp, fx_ramp_sampler, vec2<f32>(clamp(tex.g, 0.0, 1.0), clamp(f.color.r * f.grey.y, 0.0, 1.0)), 0.0);
        c = vec4<f32>(ramp.rgb * f.grey.x, c.a);
    }
    if ((mode & 2u) != 0u) {
        let ramp = textureSampleLevel(fx_ramp, fx_ramp_sampler, vec2<f32>(clamp(tex.a, 0.0, 1.0), clamp(f.color.a * f.grey.z, 0.0, 1.0)), 0.0);
        c.a = ramp.a;
    }
    let a = clamp(c.a, 0.0, 1.0);
    return vec4<f32>(c.rgb * a, a * (1.0 - f.additive));
}

struct LineIn {
    @location(0) pos: vec3<f32>,
    @location(1) color: vec4<f32>,
};

struct LineOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_line(v: LineIn) -> LineOut {
    var o: LineOut;
    o.clip = camera.view_proj * vec4<f32>(v.pos, 1.0);
    o.color = v.color;
    return o;
}

@fragment
fn fs_line(f: LineOut) -> @location(0) vec4<f32> {
    return vec4<f32>(f.color.rgb * f.color.a, f.color.a);
}
