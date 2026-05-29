//! `sol-render` - the wgpu renderer for Sea of Lost Souls.
//!
//! A deliberately small but real renderer: instance/adapter/device/queue,
//! a configured surface, a depth buffer, an orbit camera packed into a uniform
//! buffer, and one opaque mesh pipeline with simple directional/Lambert shading
//! (see `shaders/mesh.wgsl`). It clears to a dark space color and draws a set
//! of mesh instances.
//!
//! Rendering NEVER feeds gameplay: this crate reads camera + instance data and
//! draws; it must not write back into `sol-sim`.

#![forbid(unsafe_code)]

mod camera;
mod mesh;

pub use camera::OrbitCamera;
pub use mesh::{CpuMesh, CpuTexture, GpuMesh, Vertex};

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

/// Camera/material uniform uploaded to the shader. Layout must match the
/// `Camera` struct in `shaders/mesh.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct CameraUniform {
    view_proj: [[f32; 4]; 4],
    light_dir: [f32; 4],
    base_color: [f32; 4],
    /// Per-frame world-space offset added to nebula + star vertex positions
    /// in the background shader, so they follow the camera pivot like a
    /// skybox and never clip (fix-it.md items 15 + 16). xyz = offset; w
    /// unused.
    bg_offset: [f32; 4],
}

impl CameraUniform {
    fn new(
        view_proj: Mat4,
        light_dir: Vec3,
        base_color: Vec3,
        aspect: f32,
        bg_offset: Vec3,
    ) -> Self {
        Self {
            view_proj: view_proj.to_cols_array_2d(),
            light_dir: [light_dir.x, light_dir.y, light_dir.z, 0.0],
            // w carries the screen aspect (used to keep star sprites square).
            base_color: [base_color.x, base_color.y, base_color.z, aspect],
            bg_offset: [bg_offset.x, bg_offset.y, bg_offset.z, 0.0],
        }
    }
}

/// One drawable instance: a reference-free model transform applied to a mesh.
/// The mesh itself is supplied separately to [`Renderer::render`].
#[derive(Clone, Copy, Debug)]
pub struct MeshInstance {
    pub model: Mat4,
    /// Per-instance RGBA color. The mesh shader multiplies it onto the hull's
    /// livery-band texels only (a faction color); other texels are unaffected.
    pub tint: [f32; 4],
}

impl MeshInstance {
    pub fn new(model: Mat4) -> Self {
        Self {
            model,
            tint: [1.0, 1.0, 1.0, 1.0],
        }
    }

    pub fn with_tint(model: Mat4, tint: [f32; 4]) -> Self {
        Self { model, tint }
    }
}

/// Per-instance GPU data: the model matrix as four vec4 columns (locations
/// 2..=5 in the shader).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct InstanceRaw {
    model: [[f32; 4]; 4],
    tint: [f32; 4],
}

impl InstanceRaw {
    fn layout() -> wgpu::VertexBufferLayout<'static> {
        const ATTRS: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
            2 => Float32x4,
            3 => Float32x4,
            4 => Float32x4,
            5 => Float32x4,
            7 => Float32x4,
        ];
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<InstanceRaw>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &ATTRS,
        }
    }
}

/// A vertex-colored background vertex: position + linear RGB. Shared by the
/// nebula sphere, the star field, and the ground reference grid.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct BgVertex {
    pub pos: [f32; 3],
    pub color: [f32; 3],
}

impl BgVertex {
    pub fn new(pos: [f32; 3], color: [f32; 3]) -> Self {
        Self { pos, color }
    }

    fn layout() -> wgpu::VertexBufferLayout<'static> {
        const ATTRS: [wgpu::VertexAttribute; 2] =
            wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3];
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<BgVertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &ATTRS,
        }
    }
}

/// One star sprite instance: world center, color, and clip-space size.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct StarInstance {
    pub center: [f32; 3],
    pub color: [f32; 3],
    pub size: f32,
}

impl StarInstance {
    pub fn new(center: [f32; 3], color: [f32; 3], size: f32) -> Self {
        Self {
            center,
            color,
            size,
        }
    }

    fn layout() -> wgpu::VertexBufferLayout<'static> {
        const ATTRS: [wgpu::VertexAttribute; 3] =
            wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32];
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<StarInstance>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &ATTRS,
        }
    }
}

/// One translucent "sensor field" sphere instance for the sensors-manager view:
/// a world-space center + radius and an RGBA fill. Drawn inverted with depth
/// write, so overlapping spheres merge into one solid color (see `sphere.wgsl`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct SensorSphere {
    pub center_radius: [f32; 4],
    pub color: [f32; 4],
}

impl SensorSphere {
    pub fn new(center: Vec3, radius: f32, color: [f32; 4]) -> Self {
        Self {
            center_radius: [center.x, center.y, center.z, radius],
            color,
        }
    }

    fn layout() -> wgpu::VertexBufferLayout<'static> {
        const ATTRS: [wgpu::VertexAttribute; 2] =
            wgpu::vertex_attr_array![1 => Float32x4, 2 => Float32x4];
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<SensorSphere>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &ATTRS,
        }
    }
}

/// Per-vertex layout for the unit sphere mesh (position only); the instance
/// buffer (`SensorSphere`) supplies center, radius, and color.
fn sphere_vertex_layout() -> wgpu::VertexBufferLayout<'static> {
    const ATTRS: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![0 => Float32x3];
    wgpu::VertexBufferLayout {
        array_stride: (std::mem::size_of::<f32>() * 3) as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &ATTRS,
    }
}

/// A unit UV sphere centered at the origin: positions plus CCW-outward indices.
/// (No normals: the sensor field uses a flat fill.)
fn unit_sphere(stacks: u32, slices: u32) -> (Vec<[f32; 3]>, Vec<u32>) {
    let row = slices + 1;
    let mut verts = Vec::with_capacity(((stacks + 1) * row) as usize);
    for i in 0..=stacks {
        let phi = i as f32 / stacks as f32 * std::f32::consts::PI;
        let (sp, cp) = phi.sin_cos();
        for j in 0..=slices {
            let theta = j as f32 / slices as f32 * std::f32::consts::TAU;
            let (st, ct) = theta.sin_cos();
            verts.push([sp * ct, cp, sp * st]);
        }
    }
    let mut indices = Vec::with_capacity((stacks * slices * 6) as usize);
    for i in 0..stacks {
        for j in 0..slices {
            let a = i * row + j;
            indices.extend_from_slice(&[a, a + 1, a + row, a + 1, a + row + 1, a + row]);
        }
    }
    (verts, indices)
}

/// A screen-space HUD vertex: clip-space (NDC) 2D position + RGBA color. Used
/// for the band-selection box and health bars (projected to NDC on the CPU and
/// alpha-blended over the scene). Shared by the overlay line and triangle
/// pipelines.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct OverlayVertex {
    pub pos: [f32; 2],
    pub color: [f32; 4],
}

impl OverlayVertex {
    pub fn new(pos: [f32; 2], color: [f32; 4]) -> Self {
        Self { pos, color }
    }

    fn layout() -> wgpu::VertexBufferLayout<'static> {
        const ATTRS: [wgpu::VertexAttribute; 2] =
            wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4];
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<OverlayVertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &ATTRS,
        }
    }
}

/// A reusable dynamic vertex buffer that grows to fit per-frame overlay
/// geometry (gizmo lines and HUD vertices change every frame).
struct DynamicBuffer {
    buffer: wgpu::Buffer,
    capacity: u64,
    count: u32,
}

impl DynamicBuffer {
    fn new(device: &wgpu::Device, label: &'static str, capacity: u64) -> Self {
        Self {
            buffer: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: capacity,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            capacity,
            count: 0,
        }
    }

    /// Upload `data`, growing the buffer if needed. Records the vertex count.
    fn upload<T: Pod>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        label: &'static str,
        data: &[T],
    ) {
        self.count = data.len() as u32;
        if data.is_empty() {
            return;
        }
        let bytes: &[u8] = bytemuck::cast_slice(data);
        let needed = bytes.len() as u64;
        if needed > self.capacity {
            self.capacity = needed.next_power_of_two();
            self.buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: self.capacity,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        queue.write_buffer(&self.buffer, 0, bytes);
    }
}

/// Uploaded background geometry: nebula sphere, star sprites, grid points.
struct Environment {
    nebula_vbuf: wgpu::Buffer,
    nebula_ibuf: wgpu::Buffer,
    nebula_index_count: u32,
    star_inst_buf: wgpu::Buffer,
    star_count: u32,
    grid_vbuf: wgpu::Buffer,
    grid_count: u32,
}

const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// Dark space clear color.
const CLEAR_COLOR: wgpu::Color = wgpu::Color {
    r: 0.01,
    g: 0.012,
    b: 0.02,
    a: 1.0,
};

/// World-space direction TO the key light, also where the sun sprite is placed.
pub const LIGHT_DIR: [f32; 3] = [0.4, 0.8, 0.45];

/// The renderer. Owns the wgpu device/queue/surface and the mesh pipeline.
pub struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    depth_view: wgpu::TextureView,

    pipeline: wgpu::RenderPipeline,
    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    /// Base-color texture layout + a shared nearest sampler (pixel-art look).
    texture_bgl: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,

    /// Vertex-colored background pipelines (nebula triangles, far star points,
    /// near grid points) and the uploaded environment geometry.
    bg_tri_pipeline: wgpu::RenderPipeline,
    star_pipeline: wgpu::RenderPipeline,
    particle_pipeline: wgpu::RenderPipeline,
    points_near_pipeline: wgpu::RenderPipeline,
    environment: Option<Environment>,
    /// When false, the nebula + backdrop stars are skipped on draw so the
    /// build overlay's centered preview gets a clean black backdrop
    /// (fix-it.md item 17). The grid still renders.
    background_visible: bool,

    /// World-space line gizmos (selection ground-circle, elevation pole, dashed
    /// move line); drawn on top of the scene so selection stays visible.
    line_pipeline: wgpu::RenderPipeline,
    gizmo_lines: DynamicBuffer,
    /// Additive world-space sprites for transient effects (harvest motes),
    /// drawn with the star pipeline from a per-frame buffer.
    particles: DynamicBuffer,
    /// Screen-space HUD overlays (band-select box + health bars), clip-space.
    overlay_tri_pipeline: wgpu::RenderPipeline,
    overlay_line_pipeline: wgpu::RenderPipeline,
    overlay_tris: DynamicBuffer,
    overlay_lines: DynamicBuffer,
    /// Full-screen dim quad for sensors-manager view, drawn over the 3D scene
    /// but under the gizmo lines so sensor rings stay bright. Empty = no dim.
    scene_dim: DynamicBuffer,
    /// Full-screen vertical gradient quad drawn *behind* the scene for the build
    /// overlay's preview backdrop (fix-it.md item 17). Empty = no backdrop.
    build_backdrop: DynamicBuffer,
    /// Inverted translucent "sensor field" spheres (sensors-manager view). The
    /// unit sphere mesh is instanced per ship; depth write merges overlaps.
    sphere_pipeline: wgpu::RenderPipeline,
    sphere_vbuf: wgpu::Buffer,
    sphere_ibuf: wgpu::Buffer,
    sphere_index_count: u32,
    sensor_spheres: DynamicBuffer,

    /// Reusable per-frame instance buffer (grown as needed).
    instance_buffer: wgpu::Buffer,
    instance_capacity: u64,
}

impl Renderer {
    /// Create a renderer for a surface target (a window or canvas). `width`
    /// and `height` are the initial drawable size in physical pixels.
    ///
    /// Async because adapter/device acquisition is async on web; native
    /// callers can use [`pollster::block_on`] (re-exported as
    /// [`Renderer::new_blocking`] on native).
    pub async fn new(
        target: impl Into<wgpu::SurfaceTarget<'static>>,
        width: u32,
        height: u32,
    ) -> anyhow::Result<Self> {
        // InstanceDescriptor is not `Default` in wgpu 29 (it carries a
        // non-Default display handle), so build it explicitly.
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::from_env().unwrap_or_else(wgpu::Backends::all),
            flags: wgpu::InstanceFlags::from_build_config(),
            memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
            backend_options: wgpu::BackendOptions::default(),
            display: None,
        });

        let surface = instance.create_surface(target)?;

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .map_err(|e| anyhow::anyhow!("no suitable GPU adapter: {e}"))?;

        // WebGL2 can't do all of WebGPU's defaults; downlevel limits keep the
        // app within the fallback's capabilities.
        let required_limits = if cfg!(target_arch = "wasm32") {
            wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits())
        } else {
            wgpu::Limits::default()
        };

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("sol-render device"),
                required_features: wgpu::Features::empty(),
                required_limits,
                experimental_features: wgpu::ExperimentalFeatures::default(),
                memory_hints: wgpu::MemoryHints::default(),
                trace: wgpu::Trace::Off,
            })
            .await
            .map_err(|e| anyhow::anyhow!("device request failed: {e}"))?;

        let surface_caps = surface.get_capabilities(&adapter);
        let format = surface_caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(surface_caps.formats[0]);

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: width.max(1),
            height: height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: surface_caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let depth_view = create_depth_view(&device, &config);

        // Camera uniform + bind group.
        let camera_uniform =
            CameraUniform::new(Mat4::IDENTITY, Vec3::Y, Vec3::ONE, 1.0, Vec3::ZERO);
        let camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("camera uniform"),
            contents: bytemuck::bytes_of(&camera_uniform),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let camera_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("camera bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("camera bind group"),
            layout: &camera_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });

        // Base-color texture: a sampled 2D texture + a shared nearest sampler.
        let texture_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("texture bgl"),
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
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("nearest sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        // Mesh pipeline.
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mesh shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../../../shaders/mesh.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mesh pipeline layout"),
            bind_group_layouts: &[Some(&camera_bgl), Some(&texture_bgl)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mesh pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[Vertex::layout(), InstanceRaw::layout()],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        // Background pipelines (vertex-colored; reuse the camera bind group).
        let bg_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("background shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../shaders/background.wgsl").into(),
            ),
        });
        let bg_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("background pipeline layout"),
            bind_group_layouts: &[Some(&camera_bgl)],
            immediate_size: 0,
        });
        let bg_tri_pipeline = bg_pipeline(
            &device,
            &bg_layout,
            &bg_shader,
            config.format,
            wgpu::PrimitiveTopology::TriangleList,
            false,
            wgpu::CompareFunction::Always,
            "fs_main",
            // Nebula rides the camera pivot (uses bg_offset).
            "vs_main",
        );
        let points_near_pipeline = bg_pipeline(
            &device,
            &bg_layout,
            &bg_shader,
            config.format,
            wgpu::PrimitiveTopology::PointList,
            true,
            wgpu::CompareFunction::Less,
            "fs_main",
            // Grid: host already supplies snapped world positions; skip offset.
            "vs_grid",
        );
        // Star sprites: instanced additive quads (corners from the vertex index).
        let star_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("stars shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../../../shaders/stars.wgsl").into()),
        });
        let additive = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        };
        // Two pipelines off the same star shader: vs_main applies bg_offset
        // (backdrop stars ride the camera pivot, fix-it.md item 16),
        // vs_particle uses raw world coords (harvest dust, hit sparks).
        let star_buffers = [StarInstance::layout()];
        let star_targets = [Some(wgpu::ColorTargetState {
            format: config.format,
            blend: Some(wgpu::BlendState {
                color: additive,
                alpha: additive,
            }),
            write_mask: wgpu::ColorWrites::ALL,
        })];
        let star_primitive = wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: None,
            unclipped_depth: false,
            polygon_mode: wgpu::PolygonMode::Fill,
            conservative: false,
        };
        let star_depth = wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::Always),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        };
        let mk_star = |label: &'static str, vs: &'static str| wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(&bg_layout),
            vertex: wgpu::VertexState {
                module: &star_shader,
                entry_point: Some(vs),
                buffers: &star_buffers,
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &star_shader,
                entry_point: Some("fs_main"),
                targets: &star_targets,
                compilation_options: Default::default(),
            }),
            primitive: star_primitive,
            depth_stencil: Some(star_depth.clone()),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        };
        let star_pipeline = device.create_render_pipeline(&mk_star("star pipeline", "vs_main"));
        let particle_pipeline =
            device.create_render_pipeline(&mk_star("particle pipeline", "vs_particle"));

        // World-space line gizmos: reuse the vertex-colored background shader,
        // drawn as line segments on top of the scene (depth test Always, no
        // write) so the selection circle/pole/move line stay visible.
        let line_pipeline = bg_pipeline(
            &device,
            &bg_layout,
            &bg_shader,
            config.format,
            wgpu::PrimitiveTopology::LineList,
            false,
            wgpu::CompareFunction::Always,
            "fs_main",
            // World-space lines: caller passes world positions; no offset.
            "vs_grid",
        );

        // Screen-space HUD overlay pipelines (clip-space, no camera, alpha
        // blended): one for filled triangles (health bars / box fill), one for
        // line segments (box outline).
        let overlay_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("overlay shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../../../shaders/overlay.wgsl").into()),
        });
        let overlay_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("overlay pipeline layout"),
            bind_group_layouts: &[],
            immediate_size: 0,
        });
        let overlay_tri_pipeline = overlay_pipeline(
            &device,
            &overlay_layout,
            &overlay_shader,
            config.format,
            wgpu::PrimitiveTopology::TriangleList,
        );
        let overlay_line_pipeline = overlay_pipeline(
            &device,
            &overlay_layout,
            &overlay_shader,
            config.format,
            wgpu::PrimitiveTopology::LineList,
        );

        // Sensor-field spheres: inverted (front-culled) translucent shells with
        // depth write, so overlapping spheres merge into one solid color.
        let sphere_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sphere shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../../../shaders/sphere.wgsl").into()),
        });
        let alpha_over = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::SrcAlpha,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        };
        let sphere_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sensor sphere pipeline"),
            layout: Some(&bg_layout),
            vertex: wgpu::VertexState {
                module: &sphere_shader,
                entry_point: Some("vs_main"),
                buffers: &[sphere_vertex_layout(), SensorSphere::layout()],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &sphere_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState {
                        color: alpha_over,
                        alpha: alpha_over,
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                // Cull the near (front) hemisphere so the far interior shell shows.
                cull_mode: Some(wgpu::Face::Front),
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                // Write depth so only the nearest shell survives per pixel (no
                // stacking), and test against the scene so ships occlude the field.
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let (sphere_verts, sphere_indices) = unit_sphere(20, 28);
        let sphere_vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sphere.vbuf"),
            contents: bytemuck::cast_slice(&sphere_verts),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let sphere_ibuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sphere.ibuf"),
            contents: bytemuck::cast_slice(&sphere_indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        let sphere_index_count = sphere_indices.len() as u32;

        let gizmo_lines = DynamicBuffer::new(&device, "gizmo lines", 4096);
        let particles = DynamicBuffer::new(&device, "particles", 4096);
        let overlay_tris = DynamicBuffer::new(&device, "overlay tris", 4096);
        let overlay_lines = DynamicBuffer::new(&device, "overlay lines", 1024);
        let scene_dim = DynamicBuffer::new(&device, "scene dim", 256);
        let build_backdrop = DynamicBuffer::new(&device, "build backdrop", 256);
        let sensor_spheres = DynamicBuffer::new(&device, "sensor spheres", 256);

        let instance_capacity = 256;
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("instance buffer"),
            size: instance_capacity * std::mem::size_of::<InstanceRaw>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Ok(Self {
            surface,
            device,
            queue,
            config,
            depth_view,
            pipeline,
            camera_buffer,
            camera_bind_group,
            texture_bgl,
            sampler,
            bg_tri_pipeline,
            star_pipeline,
            particle_pipeline,
            points_near_pipeline,
            environment: None,
            background_visible: true,
            line_pipeline,
            gizmo_lines,
            particles,
            overlay_tri_pipeline,
            overlay_line_pipeline,
            overlay_tris,
            overlay_lines,
            scene_dim,
            build_backdrop,
            sphere_pipeline,
            sphere_vbuf,
            sphere_ibuf,
            sphere_index_count,
            sensor_spheres,
            instance_buffer,
            instance_capacity,
        })
    }

    /// Block on [`Renderer::new`] (native convenience).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new_blocking(
        target: impl Into<wgpu::SurfaceTarget<'static>>,
        width: u32,
        height: u32,
    ) -> anyhow::Result<Self> {
        pollster::block_on(Self::new(target, width, height))
    }

    /// Borrow the device (e.g. to upload meshes).
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    /// Current surface aspect ratio (width / height).
    pub fn aspect(&self) -> f32 {
        self.config.width as f32 / self.config.height.max(1) as f32
    }

    /// Reconfigure the surface and depth buffer after a resize.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        self.depth_view = create_depth_view(&self.device, &self.config);
    }

    /// Upload a CPU mesh (and its baked texture) to a drawable GPU mesh.
    pub fn upload_mesh(&self, cpu: &CpuMesh, label: &str) -> GpuMesh {
        let vertex_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(&format!("{label}.vbuf")),
                contents: bytemuck::cast_slice(&cpu.vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let index_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(&format!("{label}.ibuf")),
                contents: bytemuck::cast_slice(&cpu.indices),
                usage: wgpu::BufferUsages::INDEX,
            });
        let view = self.upload_texture(cpu.texture.as_ref(), label);
        let texture_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(&format!("{label}.tex_bg")),
            layout: &self.texture_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        GpuMesh {
            vertex_buffer,
            index_buffer,
            index_count: cpu.indices.len() as u32,
            texture_bind_group,
        }
    }

    /// Upload a `CpuTexture` (or a 1x1 white fallback) and return its view.
    fn upload_texture(&self, tex: Option<&CpuTexture>, label: &str) -> wgpu::TextureView {
        let white = [255u8, 255, 255, 255];
        let (w, h, data): (u32, u32, &[u8]) = match tex {
            Some(t) if !t.rgba.is_empty() => (t.width.max(1), t.height.max(1), &t.rgba),
            _ => (1, 1, &white),
        };
        let size = wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(&format!("{label}.tex")),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            texture.as_image_copy(),
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * w),
                rows_per_image: Some(h),
            },
            size,
        );
        texture.create_view(&wgpu::TextureViewDescriptor::default())
    }

    /// Upload the background environment: nebula sphere, star points, and the
    /// ground reference grid. Call once after creating the renderer.
    pub fn set_environment(
        &mut self,
        nebula: &[BgVertex],
        nebula_indices: &[u32],
        stars: &[StarInstance],
        grid: &[BgVertex],
    ) {
        let nebula_vbuf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("nebula.vbuf"),
                contents: bytemuck::cast_slice(nebula),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let nebula_ibuf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("nebula.ibuf"),
                contents: bytemuck::cast_slice(nebula_indices),
                usage: wgpu::BufferUsages::INDEX,
            });
        let star_inst_buf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("stars.instances"),
                contents: bytemuck::cast_slice(stars),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let grid_vbuf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("grid.vbuf"),
                contents: bytemuck::cast_slice(grid),
                usage: wgpu::BufferUsages::VERTEX,
            });
        self.environment = Some(Environment {
            nebula_vbuf,
            nebula_ibuf,
            nebula_index_count: nebula_indices.len() as u32,
            star_inst_buf,
            star_count: stars.len() as u32,
            grid_vbuf,
            grid_count: grid.len() as u32,
        });
    }

    /// Show or hide the nebula + backdrop stars (fix-it.md item 17). Used by
    /// the build overlay to render the previewed ship against a clean
    /// gradient backdrop. The grid keeps rendering either way.
    pub fn set_background_visible(&mut self, visible: bool) {
        self.background_visible = visible;
    }

    /// Set (or clear) the build-overlay backdrop: a full-screen vertical
    /// gradient drawn behind the scene (fix-it.md item 17). `Some((top, bottom))`
    /// are linear-RGB colors for the top and bottom of the screen; `None`
    /// clears it. Pair with `set_background_visible(false)` so only the
    /// gradient (not the nebula) shows behind the previewed ship.
    pub fn set_build_backdrop(&mut self, colors: Option<([f32; 3], [f32; 3])>) {
        match colors {
            None => self.build_backdrop.count = 0,
            Some((top, bot)) => {
                let t = [top[0], top[1], top[2], 1.0];
                let b = [bot[0], bot[1], bot[2], 1.0];
                // Two triangles covering NDC; +y is the top of the screen.
                let tl = OverlayVertex::new([-1.0, 1.0], t);
                let tr = OverlayVertex::new([1.0, 1.0], t);
                let bl = OverlayVertex::new([-1.0, -1.0], b);
                let br = OverlayVertex::new([1.0, -1.0], b);
                let quad = [tl, tr, br, tl, br, bl];
                self.build_backdrop
                    .upload(&self.device, &self.queue, "build backdrop", &quad);
            }
        }
    }

    /// Re-upload only the grid points (fix-it.md item 13: the grid follows the
    /// camera pivot each frame). Cheap: the grid is hundreds of vertices.
    pub fn set_grid(&mut self, grid: &[BgVertex]) {
        let Some(env) = self.environment.as_mut() else {
            return;
        };
        let grid_vbuf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("grid.vbuf"),
                contents: bytemuck::cast_slice(grid),
                usage: wgpu::BufferUsages::VERTEX,
            });
        env.grid_vbuf = grid_vbuf;
        env.grid_count = grid.len() as u32;
    }

    /// Upload per-frame HUD overlays, consumed by the next `render_groups`:
    /// `gizmo_lines` are world-space line segments (selection circle / elevation
    /// pole / dashed move line); `overlay_tris` and `overlay_lines` are
    /// clip-space (NDC) HUD geometry (health bars + band-select box). Pass empty
    /// slices to clear. Call each frame before `render_groups`.
    pub fn set_overlays(
        &mut self,
        gizmo_lines: &[BgVertex],
        overlay_tris: &[OverlayVertex],
        overlay_lines: &[OverlayVertex],
    ) {
        self.gizmo_lines
            .upload(&self.device, &self.queue, "gizmo lines", gizmo_lines);
        self.overlay_tris
            .upload(&self.device, &self.queue, "overlay tris", overlay_tris);
        self.overlay_lines
            .upload(&self.device, &self.queue, "overlay lines", overlay_lines);
    }

    /// Upload per-frame additive world-space particle sprites (harvest motes),
    /// drawn with the star pipeline. Pass empty to clear.
    pub fn set_particles(&mut self, particles: &[StarInstance]) {
        self.particles
            .upload(&self.device, &self.queue, "particles", particles);
    }

    /// Set the sensors-manager scene dim: a full-screen alpha quad drawn over
    /// the 3D scene (ships, nebula, grid) but UNDER the world-space gizmo lines,
    /// so sensor rings and the tactical grid stay bright. Alpha <= 0 clears it.
    pub fn set_scene_dim(&mut self, rgba: [f32; 4]) {
        let verts: &[OverlayVertex] = &if rgba[3] <= 0.0 {
            Vec::new()
        } else {
            // Two triangles covering clip space [-1, 1].
            vec![
                OverlayVertex::new([-1.0, -1.0], rgba),
                OverlayVertex::new([1.0, -1.0], rgba),
                OverlayVertex::new([1.0, 1.0], rgba),
                OverlayVertex::new([-1.0, -1.0], rgba),
                OverlayVertex::new([1.0, 1.0], rgba),
                OverlayVertex::new([-1.0, 1.0], rgba),
            ]
        };
        self.scene_dim
            .upload(&self.device, &self.queue, "scene dim", verts);
    }

    /// Upload the sensors-manager "sensor field" spheres (one per ship), drawn as
    /// inverted translucent shells. Pass empty to clear. Call before
    /// `render_groups`.
    pub fn set_sensor_spheres(&mut self, spheres: &[SensorSphere]) {
        self.sensor_spheres
            .upload(&self.device, &self.queue, "sensor spheres", spheres);
    }

    /// Draw several mesh groups in one pass: each group is a mesh plus the
    /// instances drawn with it, so the whole fleet renders with a single clear.
    /// Groups share one instance buffer via contiguous per-group ranges.
    /// `Lost`/`Outdated` mean the caller should reconfigure (re-`resize`).
    pub fn render_groups(
        &mut self,
        camera: &OrbitCamera,
        groups: &[(&GpuMesh, &[MeshInstance])],
    ) -> RenderOutcome {
        // Update camera uniform.
        let light_dir = Vec3::from(LIGHT_DIR).normalize();
        let uniform = CameraUniform::new(
            camera.view_proj(self.aspect()),
            light_dir,
            Vec3::ONE,
            self.aspect(),
            // Nebula + stars ride the camera pivot so they never clip.
            camera.focus,
        );
        self.queue
            .write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(&uniform));

        // Flatten every group's instances into one buffer, recording each
        // group's contiguous [start, end) range.
        let mut raw: Vec<InstanceRaw> = Vec::new();
        let mut ranges: Vec<(u32, u32)> = Vec::with_capacity(groups.len());
        for (_, instances) in groups {
            let start = raw.len() as u32;
            raw.extend(instances.iter().map(|i| InstanceRaw {
                model: i.model.to_cols_array_2d(),
                tint: i.tint,
            }));
            ranges.push((start, raw.len() as u32));
        }
        if raw.len() as u64 > self.instance_capacity {
            self.instance_capacity = (raw.len() as u64).next_power_of_two();
            self.instance_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("instance buffer"),
                size: self.instance_capacity * std::mem::size_of::<InstanceRaw>() as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !raw.is_empty() {
            self.queue
                .write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(&raw));
        }

        // wgpu 29 returns an enum (not a Result). Map non-success states to a
        // caller-actionable outcome.
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return RenderOutcome::Skipped
            }
            wgpu::CurrentSurfaceTexture::Outdated
            | wgpu::CurrentSurfaceTexture::Lost
            | wgpu::CurrentSurfaceTexture::Validation => return RenderOutcome::NeedsReconfigure,
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame encoder"),
            });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("mesh pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(CLEAR_COLOR),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            // Build-overlay backdrop: a full-screen gradient drawn before
            // everything else (no depth write) so the previewed ship sits on a
            // clean horizon gradient instead of the nebula (fix-it.md item 17).
            if self.build_backdrop.count > 0 {
                pass.set_pipeline(&self.overlay_tri_pipeline);
                pass.set_vertex_buffer(0, self.build_backdrop.buffer.slice(..));
                pass.draw(0..self.build_backdrop.count, 0..1);
            }

            // Background first: nebula + stars (no depth write) then the ground
            // grid (depth-tested), so ships render over them correctly.
            if let Some(env) = &self.environment {
                pass.set_bind_group(0, &self.camera_bind_group, &[]);
                if self.background_visible {
                    pass.set_pipeline(&self.bg_tri_pipeline);
                    pass.set_vertex_buffer(0, env.nebula_vbuf.slice(..));
                    pass.set_index_buffer(env.nebula_ibuf.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..env.nebula_index_count, 0, 0..1);
                    pass.set_pipeline(&self.star_pipeline);
                    pass.set_vertex_buffer(0, env.star_inst_buf.slice(..));
                    pass.draw(0..6, 0..env.star_count);
                }
                pass.set_pipeline(&self.points_near_pipeline);
                pass.set_vertex_buffer(0, env.grid_vbuf.slice(..));
                pass.draw(0..env.grid_count, 0..1);
            }

            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.camera_bind_group, &[]);
            pass.set_vertex_buffer(1, self.instance_buffer.slice(..));
            for ((mesh, _), &(start, end)) in groups.iter().zip(&ranges) {
                if start == end {
                    continue;
                }
                pass.set_bind_group(1, &mesh.texture_bind_group, &[]);
                pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.index_count, 0, start..end);
            }

            // Additive harvest-mote / explosion sprites: world-space, so they
            // use the particle pipeline (no bg_offset).
            if self.particles.count > 0 {
                pass.set_pipeline(&self.particle_pipeline);
                pass.set_bind_group(0, &self.camera_bind_group, &[]);
                pass.set_vertex_buffer(0, self.particles.buffer.slice(..));
                pass.draw(0..6, 0..self.particles.count);
            }

            // Sensors-manager scene dim: a full-screen alpha quad over the 3D
            // scene but beneath the gizmos, so sensor rings/grid stay bright.
            if self.scene_dim.count > 0 {
                pass.set_pipeline(&self.overlay_tri_pipeline);
                pass.set_vertex_buffer(0, self.scene_dim.buffer.slice(..));
                pass.draw(0..self.scene_dim.count, 0..1);
            }

            // Sensor-field spheres: inverted translucent shells over the dimmed
            // scene, beneath the gizmo blips. One instance per ship; depth write
            // merges overlaps into one solid color.
            if self.sensor_spheres.count > 0 {
                pass.set_pipeline(&self.sphere_pipeline);
                pass.set_bind_group(0, &self.camera_bind_group, &[]);
                pass.set_vertex_buffer(0, self.sphere_vbuf.slice(..));
                pass.set_vertex_buffer(1, self.sensor_spheres.buffer.slice(..));
                pass.set_index_buffer(self.sphere_ibuf.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..self.sphere_index_count, 0, 0..self.sensor_spheres.count);
            }

            // World-space selection gizmos on top of the ships (camera-transformed
            // line segments, depth test Always).
            if self.gizmo_lines.count > 0 {
                pass.set_pipeline(&self.line_pipeline);
                pass.set_bind_group(0, &self.camera_bind_group, &[]);
                pass.set_vertex_buffer(0, self.gizmo_lines.buffer.slice(..));
                pass.draw(0..self.gizmo_lines.count, 0..1);
            }

            // Screen-space HUD: filled triangles (health bars + box fill) then
            // line outlines (box border). No camera, alpha blended.
            if self.overlay_tris.count > 0 {
                pass.set_pipeline(&self.overlay_tri_pipeline);
                pass.set_vertex_buffer(0, self.overlay_tris.buffer.slice(..));
                pass.draw(0..self.overlay_tris.count, 0..1);
            }
            if self.overlay_lines.count > 0 {
                pass.set_pipeline(&self.overlay_line_pipeline);
                pass.set_vertex_buffer(0, self.overlay_lines.buffer.slice(..));
                pass.draw(0..self.overlay_lines.count, 0..1);
            }

            // TODO: egui overlay pass goes here once the HUD lands (design.md §10).
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
        RenderOutcome::Presented
    }

    /// Convenience: draw a single mesh's instances (one group).
    pub fn render(
        &mut self,
        camera: &OrbitCamera,
        mesh: &GpuMesh,
        instances: &[MeshInstance],
    ) -> RenderOutcome {
        self.render_groups(camera, &[(mesh, instances)])
    }
}

/// Result of a frame render.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderOutcome {
    /// Frame submitted and presented.
    Presented,
    /// Surface was occluded/timed out; nothing drawn, try again next frame.
    Skipped,
    /// Surface is lost/outdated; caller should reconfigure (resize) and retry.
    NeedsReconfigure,
}

/// Build a vertex-colored background pipeline (no instancing, no texture; reuses
/// the camera bind group). Topology + depth behavior vary per use (nebula
/// triangles and far stars skip depth; the near grid is depth-tested).
#[allow(clippy::too_many_arguments)]
fn bg_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    topology: wgpu::PrimitiveTopology,
    depth_write: bool,
    depth_compare: wgpu::CompareFunction,
    frag_entry: &str,
    vert_entry: &str,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("background pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some(vert_entry),
            buffers: &[BgVertex::layout()],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(frag_entry),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: None,
            unclipped_depth: false,
            polygon_mode: wgpu::PolygonMode::Fill,
            conservative: false,
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(depth_write),
            depth_compare: Some(depth_compare),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

/// Build a clip-space HUD overlay pipeline (no camera bind group, alpha
/// blended, depth always/no-write so it draws on top). `topology` selects
/// filled triangles (health bars) vs line segments (box outline).
fn overlay_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    topology: wgpu::PrimitiveTopology,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("overlay pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            buffers: &[OverlayVertex::layout()],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: None,
            unclipped_depth: false,
            polygon_mode: wgpu::PolygonMode::Fill,
            conservative: false,
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::Always),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

fn create_depth_view(
    device: &wgpu::Device,
    config: &wgpu::SurfaceConfiguration,
) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("depth texture"),
        size: wgpu::Extent3d {
            width: config.width.max(1),
            height: config.height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}
