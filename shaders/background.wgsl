// Vertex-colored background: the nebula sphere (fs_nebula adds fine GPU noise on
// top of the per-vertex gradient) and the ground reference grid (fs_main, flat).
// Reuses the camera uniform (group 0) for view*proj.

struct Camera {
    view_proj: mat4x4<f32>,
    light_dir: vec4<f32>,
    base_color: vec4<f32>,
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
    @location(1) world_pos: vec3<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = camera.view_proj * vec4<f32>(in.position, 1.0);
    out.color = in.color;
    out.world_pos = in.position;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color, 1.0);
}

fn hash(p: vec3<f32>) -> f32 {
    return fract(sin(dot(p, vec3<f32>(12.9898, 78.233, 37.719))) * 43758.5453);
}

fn vnoise(p: vec3<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let c000 = hash(i + vec3<f32>(0.0, 0.0, 0.0));
    let c100 = hash(i + vec3<f32>(1.0, 0.0, 0.0));
    let c010 = hash(i + vec3<f32>(0.0, 1.0, 0.0));
    let c110 = hash(i + vec3<f32>(1.0, 1.0, 0.0));
    let c001 = hash(i + vec3<f32>(0.0, 0.0, 1.0));
    let c101 = hash(i + vec3<f32>(1.0, 0.0, 1.0));
    let c011 = hash(i + vec3<f32>(0.0, 1.0, 1.0));
    let c111 = hash(i + vec3<f32>(1.0, 1.0, 1.0));
    let x00 = mix(c000, c100, u.x);
    let x10 = mix(c010, c110, u.x);
    let x01 = mix(c001, c101, u.x);
    let x11 = mix(c011, c111, u.x);
    let y0 = mix(x00, x10, u.y);
    let y1 = mix(x01, x11, u.y);
    return mix(y0, y1, u.z);
}

fn fbm(p: vec3<f32>) -> f32 {
    var f = 0.0;
    var amp = 0.5;
    var q = p;
    for (var k = 0; k < 4; k = k + 1) {
        f = f + amp * vnoise(q);
        q = q * 2.0;
        amp = amp * 0.5;
    }
    return f;
}

@fragment
fn fs_nebula(in: VertexOutput) -> @location(0) vec4<f32> {
    let dir = normalize(in.world_pos);
    // Fine, high-frequency variation layered over the per-vertex gradient.
    let detail = 0.55 + 0.85 * fbm(dir * 9.0);
    return vec4<f32>(in.color * detail, 1.0);
}
