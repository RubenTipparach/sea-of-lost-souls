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
}

impl CameraUniform {
    fn new(view_proj: Mat4, light_dir: Vec3, base_color: Vec3) -> Self {
        Self {
            view_proj: view_proj.to_cols_array_2d(),
            light_dir: [light_dir.x, light_dir.y, light_dir.z, 0.0],
            base_color: [base_color.x, base_color.y, base_color.z, 1.0],
        }
    }
}

/// One drawable instance: a reference-free model transform applied to a mesh.
/// The mesh itself is supplied separately to [`Renderer::render`].
#[derive(Clone, Copy, Debug)]
pub struct MeshInstance {
    pub model: Mat4,
    /// Per-instance RGBA tint multiplied over the texture (team / selection).
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

/// Uploaded background geometry: nebula sphere, star points, grid points.
struct Environment {
    nebula_vbuf: wgpu::Buffer,
    nebula_ibuf: wgpu::Buffer,
    nebula_index_count: u32,
    star_vbuf: wgpu::Buffer,
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
    points_far_pipeline: wgpu::RenderPipeline,
    points_near_pipeline: wgpu::RenderPipeline,
    environment: Option<Environment>,

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
        let camera_uniform = CameraUniform::new(Mat4::IDENTITY, Vec3::Y, Vec3::ONE);
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
        );
        let points_far_pipeline = bg_pipeline(
            &device,
            &bg_layout,
            &bg_shader,
            config.format,
            wgpu::PrimitiveTopology::PointList,
            false,
            wgpu::CompareFunction::Always,
        );
        let points_near_pipeline = bg_pipeline(
            &device,
            &bg_layout,
            &bg_shader,
            config.format,
            wgpu::PrimitiveTopology::PointList,
            true,
            wgpu::CompareFunction::Less,
        );

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
            points_far_pipeline,
            points_near_pipeline,
            environment: None,
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
        stars: &[BgVertex],
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
        let star_vbuf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("stars.vbuf"),
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
            star_vbuf,
            star_count: stars.len() as u32,
            grid_vbuf,
            grid_count: grid.len() as u32,
        });
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
        let light_dir = Vec3::new(0.4, 0.8, 0.45).normalize();
        let uniform = CameraUniform::new(camera.view_proj(self.aspect()), light_dir, Vec3::ONE);
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

            // Background first: nebula + stars (no depth write) then the ground
            // grid (depth-tested), so ships render over them correctly.
            if let Some(env) = &self.environment {
                pass.set_bind_group(0, &self.camera_bind_group, &[]);
                pass.set_pipeline(&self.bg_tri_pipeline);
                pass.set_vertex_buffer(0, env.nebula_vbuf.slice(..));
                pass.set_index_buffer(env.nebula_ibuf.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..env.nebula_index_count, 0, 0..1);
                pass.set_pipeline(&self.points_far_pipeline);
                pass.set_vertex_buffer(0, env.star_vbuf.slice(..));
                pass.draw(0..env.star_count, 0..1);
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
fn bg_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    topology: wgpu::PrimitiveTopology,
    depth_write: bool,
    depth_compare: wgpu::CompareFunction,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("background pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            buffers: &[BgVertex::layout()],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
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
