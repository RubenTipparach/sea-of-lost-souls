//! Mesh data: CPU-side vertex/index/uv buffers, an optional baked texture, and
//! glTF loading. GPU upload lives on the renderer (it owns the sampler and the
//! texture bind group layout).

use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use std::path::Path;

/// A single mesh vertex: position + normal + uv. POD for direct upload.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
}

impl Vertex {
    pub fn new(position: Vec3, normal: Vec3) -> Self {
        Self {
            position: position.to_array(),
            normal: normal.to_array(),
            uv: [0.0, 0.0],
        }
    }

    /// Vertex buffer layout matching `shaders/mesh.wgsl`. Locations 0/1 are
    /// position/normal; 2..=5 are the per-instance model matrix; 6 is the UV.
    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        const ATTRS: [wgpu::VertexAttribute; 3] =
            wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 6 => Float32x2];
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &ATTRS,
        }
    }
}

/// A decoded RGBA8 texture baked into a ship GLB, sampled with nearest
/// filtering for a crisp pixel-art look.
#[derive(Clone, Debug)]
pub struct CpuTexture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// CPU-side mesh data plus an optional baked texture. Loaded from glTF, then
/// uploaded with [`crate::Renderer::upload_mesh`].
#[derive(Clone, Debug, Default)]
pub struct CpuMesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub texture: Option<CpuTexture>,
}

impl CpuMesh {
    pub fn new(vertices: Vec<Vertex>, indices: Vec<u32>) -> Self {
        Self {
            vertices,
            indices,
            texture: None,
        }
    }

    /// Load the first mesh primitive found in a glTF/GLB file (native).
    pub fn from_gltf_file(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let (doc, buffers, images) = gltf::import(path.as_ref())?;
        Self::from_gltf_doc(&doc, &buffers, &images)
    }

    /// Load the first mesh primitive from an in-memory GLB/glTF (web, where the
    /// compiled asset is embedded with `include_bytes!`).
    pub fn from_gltf_slice(bytes: &[u8]) -> anyhow::Result<Self> {
        let (doc, buffers, images) = gltf::import_slice(bytes)?;
        Self::from_gltf_doc(&doc, &buffers, &images)
    }

    /// Load the first mesh primitive: positions are required; normals are
    /// derived flat if absent; UVs default to 0; the base-color texture is
    /// extracted if the primitive's material has one.
    pub fn from_gltf_doc(
        doc: &gltf::Document,
        buffers: &[gltf::buffer::Data],
        images: &[gltf::image::Data],
    ) -> anyhow::Result<Self> {
        for mesh in doc.meshes() {
            for prim in mesh.primitives() {
                let reader = prim.reader(|b| Some(&buffers[b.index()]));
                let positions: Vec<[f32; 3]> = match reader.read_positions() {
                    Some(p) => p.collect(),
                    None => continue,
                };
                let indices: Vec<u32> = match reader.read_indices() {
                    Some(i) => i.into_u32().collect(),
                    // Non-indexed primitive: synthesize a trivial index list.
                    None => (0..positions.len() as u32).collect(),
                };
                let normals: Vec<[f32; 3]> = match reader.read_normals() {
                    Some(n) => n.collect(),
                    None => compute_flat_normals(&positions, &indices),
                };
                let uvs: Vec<[f32; 2]> = match reader.read_tex_coords(0) {
                    Some(tc) => tc.into_f32().collect(),
                    None => vec![[0.0, 0.0]; positions.len()],
                };

                let vertices = positions
                    .iter()
                    .enumerate()
                    .map(|(i, p)| Vertex {
                        position: *p,
                        normal: normals[i],
                        uv: uvs.get(i).copied().unwrap_or([0.0, 0.0]),
                    })
                    .collect();

                let texture = primitive_texture(&prim, images);
                return Ok(Self {
                    vertices,
                    indices,
                    texture,
                });
            }
        }
        anyhow::bail!("glTF document contains no mesh primitive with positions")
    }
}

/// GPU-resident mesh ready to draw, with its own base-color texture bind group.
#[derive(Debug)]
pub struct GpuMesh {
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: wgpu::Buffer,
    pub index_count: u32,
    pub texture_bind_group: wgpu::BindGroup,
}

/// Extract a primitive's base-color texture as RGBA8, if it has one.
fn primitive_texture(prim: &gltf::Primitive, images: &[gltf::image::Data]) -> Option<CpuTexture> {
    let info = prim
        .material()
        .pbr_metallic_roughness()
        .base_color_texture()?;
    let image = images.get(info.texture().source().index())?;
    Some(to_rgba8(image))
}

/// Convert a decoded glTF image to RGBA8 (handles the common RGB8/RGBA8 cases).
fn to_rgba8(image: &gltf::image::Data) -> CpuTexture {
    use gltf::image::Format;
    let (width, height) = (image.width, image.height);
    let rgba = match image.format {
        Format::R8G8B8A8 => image.pixels.clone(),
        Format::R8G8B8 => {
            let mut out = Vec::with_capacity((width * height * 4) as usize);
            for c in image.pixels.chunks_exact(3) {
                out.extend_from_slice(&[c[0], c[1], c[2], 255]);
            }
            out
        }
        // Unexpected format: flat gray so it is visible, not a crash.
        _ => vec![160; (width * height * 4) as usize],
    };
    CpuTexture {
        width,
        height,
        rgba,
    }
}

/// Derive flat per-triangle normals when a glTF primitive lacks them.
fn compute_flat_normals(positions: &[[f32; 3]], indices: &[u32]) -> Vec<[f32; 3]> {
    let mut normals = vec![Vec3::ZERO; positions.len()];
    for tri in indices.chunks_exact(3) {
        let (a, b, c) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
        let pa = Vec3::from(positions[a]);
        let pb = Vec3::from(positions[b]);
        let pc = Vec3::from(positions[c]);
        let n = (pb - pa).cross(pc - pa);
        normals[a] += n;
        normals[b] += n;
        normals[c] += n;
    }
    normals
        .into_iter()
        .map(|n| n.normalize_or_zero().to_array())
        .collect()
}
