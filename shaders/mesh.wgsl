// Basic opaque mesh shader for Sea of Lost Souls.
//
// Vertex format: position (vec3) + normal (vec3). Per-instance model matrix is
// supplied as four vec4 vertex attributes (locations 2..=5). Camera view*proj
// and world-space light direction come from a uniform buffer. Shading is a
// simple Lambert term over a single directional light plus a small ambient
// fill, which reads fine for grey-box ship hulls.

struct Camera {
    view_proj: mat4x4<f32>,
    // xyz = direction TO the light (normalized), w unused.
    light_dir: vec4<f32>,
    // rgb = base color, w unused.
    base_color: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> camera: Camera;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    // Instance model matrix, one vec4 per row-as-column.
    @location(2) model_0: vec4<f32>,
    @location(3) model_1: vec4<f32>,
    @location(4) model_2: vec4<f32>,
    @location(5) model_3: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
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
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let n = normalize(in.world_normal);
    let l = normalize(camera.light_dir.xyz);
    let lambert = max(dot(n, l), 0.0);
    let ambient = 0.15;
    let lit = camera.base_color.rgb * (ambient + lambert * 0.85);
    return vec4<f32>(lit, 1.0);
}
