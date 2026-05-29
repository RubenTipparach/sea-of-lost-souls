// Translucent "sensor field" spheres for the sensors-manager view.
//
// Rendered inverted (front faces culled, so the far interior shell shows) with a
// flat, slightly transparent fill. The pipeline writes depth, so only the
// nearest shell survives per pixel: overlapping spheres merge into one solid
// translucent color instead of stacking alpha.

struct Camera {
    view_proj: mat4x4<f32>,
    light_dir: vec4<f32>,
    base_color: vec4<f32>,
};
@group(0) @binding(0) var<uniform> camera: Camera;

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) center_radius: vec4<f32>,
    @location(2) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    let world = in.center_radius.xyz + in.pos * in.center_radius.w;
    var out: VsOut;
    out.clip = camera.view_proj * vec4<f32>(world, 1.0);
    out.color = in.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return in.color;
}
