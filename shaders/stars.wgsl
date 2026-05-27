// Star sprites: instanced camera-facing quads (SNES-style point sprites) with
// per-star size and color, additively blended into the backdrop. Corners come
// from the vertex index (6 per star); the sun is just a big bright star.

struct Camera {
    view_proj: mat4x4<f32>,
    light_dir: vec4<f32>,
    // rgb = tint; w = screen aspect (width / height) for square sprites.
    base_color: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> camera: Camera;

struct StarInput {
    @location(0) center: vec3<f32>,
    @location(1) color: vec3<f32>,
    @location(2) size: f32,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec3<f32>,
    @location(1) corner: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, in: StarInput) -> VertexOutput {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, 1.0),
    );
    let corner = corners[vi];

    var clip = camera.view_proj * vec4<f32>(in.center, 1.0);
    let aspect = camera.base_color.w;
    // Offset in clip space, scaled by w so the on-screen size is constant.
    clip.x += corner.x * in.size * clip.w / aspect;
    clip.y += corner.y * in.size * clip.w;

    var out: VertexOutput;
    out.clip_position = clip;
    out.color = in.color;
    out.corner = corner;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Round, soft glow; premultiplied for additive blending.
    let glow = clamp(1.0 - length(in.corner), 0.0, 1.0);
    let a = glow * glow;
    return vec4<f32>(in.color * a, a);
}
