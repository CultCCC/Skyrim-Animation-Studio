// Effect shader (EFSH membrane) over an actor mesh: the fill texture
// scrolling across it, optionally looked up in a palette by luminance, plus
// a fresnel edge glow. Additive (premultiplied, alpha 0), no depth writes.

struct Camera {
    view_proj: mat4x4<f32>,
    light: vec4<f32>,
};
@group(0) @binding(0) var<uniform> camera: Camera;

struct Shell {
    // rgb × alpha of the fill layer.
    fill: vec4<f32>,
    edge: vec4<f32>,
    // xy = UV scale, zw = UV offset.
    uv: vec4<f32>,
    // xyz = camera position, w = edge falloff.
    eye: vec4<f32>,
    // x = palette lookup, y = has a fill texture.
    misc: vec4<f32>,
};
@group(1) @binding(0) var fill_tex: texture_2d<f32>;
@group(1) @binding(1) var fill_sampler: sampler;
@group(1) @binding(2) var palette_tex: texture_2d<f32>;
@group(1) @binding(3) var palette_sampler: sampler;
@group(1) @binding(4) var<uniform> shell: Shell;

struct MeshIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
};

struct ShellOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) world: vec3<f32>,
};

@vertex
fn vs_shell(v: MeshIn) -> ShellOut {
    var o: ShellOut;
    o.clip = camera.view_proj * vec4<f32>(v.pos, 1.0);
    o.normal = v.normal;
    o.uv = v.uv;
    o.world = v.pos;
    return o;
}

@fragment
fn fs_shell(f: ShellOut) -> @location(0) vec4<f32> {
    let v = normalize(shell.eye.xyz - f.world);
    var n = normalize(f.normal);
    if (dot(n, v) < 0.0) {
        n = -n;
    }
    let rim = pow(1.0 - clamp(dot(n, v), 0.0, 1.0), max(shell.eye.w, 0.05));
    let tex = textureSample(fill_tex, fill_sampler, f.uv * shell.uv.xy + shell.uv.zw);
    let lum = dot(tex.rgb, vec3<f32>(0.299, 0.587, 0.114));
    let pal = textureSample(palette_tex, palette_sampler, vec2<f32>(clamp(lum, 0.01, 0.99), 0.5)).rgb;
    let base = select(tex.rgb, pal, shell.misc.x > 0.5);
    let fill = base * shell.fill.rgb * shell.fill.a * tex.a * shell.misc.y;
    let edge = shell.edge.rgb * shell.edge.a * rim;
    return vec4<f32>(fill + edge, 0.0);
}
