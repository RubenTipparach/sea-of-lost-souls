// Basic textured mesh shader for Sea of Lost Souls.
//
// Vertex format: position (vec3) + normal (vec3) + uv (vec2, location 6). The
// per-instance model matrix is four vec4 vertex attributes (locations 2..=5).
// Camera view*proj, world-space light direction, and a base-color tint come
// from a uniform buffer; the ship's baked texture is sampled with nearest
// filtering for a crisp pixel look. Shading is a simple Lambert term plus a
// small ambient fill, multiplied by the texture.

struct Camera {
    view_proj: mat4x4<f32>,
    // xyz = direction TO the light (normalized), w unused.
    light_dir: vec4<f32>,
    // rgb = tint multiplied over the texture (white = untinted), w unused.
    base_color: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> camera: Camera;

@group(1) @binding(0)
var base_tex: texture_2d<f32>;
@group(1) @binding(1)
var base_sampler: sampler;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    // Instance model matrix, one vec4 per column.
    @location(2) model_0: vec4<f32>,
    @location(3) model_1: vec4<f32>,
    @location(4) model_2: vec4<f32>,
    @location(5) model_3: vec4<f32>,
    @location(6) uv: vec2<f32>,
    @location(7) tint: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) tint: vec4<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    let model = mat4x4<f32>(in.model_0, in.model_1, in.model_2, in.model_3);

    var out: VertexOutput;
    let world_pos = model * vec4<f32>(in.position, 1.0);
    out.clip_position = camera.view_proj * world_pos;

    // Assumes uniform scale (true for our ships); upper 3x3 rotates normals.
    let normal_mat = mat3x3<f32>(
        model[0].xyz,
        model[1].xyz,
        model[2].xyz,
    );
    out.world_normal = normalize(normal_mat * in.normal);
    out.uv = in.uv;
    out.tint = in.tint;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let n = normalize(in.world_normal);
    let l = normalize(camera.light_dir.xyz);
    let lambert = max(dot(n, l), 0.0);
    let ambient = 0.18;
    let tex = textureSample(base_tex, base_sampler, in.uv);
    // The texture alpha is a 3-band mask: ~0 = plain hull (grey, no livery),
    // ~0.63 = livery (multiply by the per-instance team color), ~1.0 = emissive
    // window. So a grey hull keeps a colored team livery and lit windows.
    let livery = f32(tex.a > 0.2 && tex.a <= 0.85);
    let window = f32(tex.a > 0.85);
    let albedo = tex.rgb * mix(vec3<f32>(1.0), in.tint.rgb, livery);
    let lit = albedo * camera.base_color.rgb * (ambient + lambert * 0.85);
    let emissive = tex.rgb * window * 1.6;
    return vec4<f32>(lit + emissive, 1.0);
}
