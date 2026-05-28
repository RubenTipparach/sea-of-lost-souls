// Vertex-colored background: the nebula sphere and the ground reference grid.
// ALL nebula detail is baked into per-vertex colors (see `sol-procgen`), so the
// fragment stage just emits the interpolated vertex color, no procedural noise.
// Reuses the camera uniform (group 0) for view*proj.

struct Camera {
    view_proj: mat4x4<f32>,
    light_dir: vec4<f32>,
    base_color: vec4<f32>,
    // xyz: world-space offset that the host updates per frame so the nebula
    // and ground grid ride the camera pivot (skybox-style). w unused.
    bg_offset: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> camera: Camera;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) color: vec3<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec3<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position =
        camera.view_proj * vec4<f32>(in.position + camera.bg_offset.xyz, 1.0);
    out.color = in.color;
    return out;
}

// Grid path: the host rebuilds the grid each frame with positions already
// anchored to the camera pivot (snapped to nearest 10), so we don't add
// bg_offset here.
@vertex
fn vs_grid(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = camera.view_proj * vec4<f32>(in.position, 1.0);
    out.color = in.color;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color, 1.0);
}
