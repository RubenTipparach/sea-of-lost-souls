//! Mesh data: CPU-side vertex/index buffers and their GPU upload.

use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use std::path::Path;
use wgpu::util::DeviceExt;

/// A single mesh vertex: position + normal. POD so it can be uploaded directly.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
}

impl Vertex {
    pub fn new(position: Vec3, normal: Vec3) -> Self {
        Self {
            position: position.to_array(),
            normal: normal.to_array(),
        }
    }

    /// Vertex buffer layout matching `shaders/mesh.wgsl` (locations 0 and 1).
    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        const ATTRS: [wgpu::VertexAttribute; 2] =
            wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3];
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &ATTRS,
        }
    }
}

/// CPU-side mesh data (positions, normals, indices). Caller-supplied or loaded
/// from glTF, then uploaded with [`CpuMesh::upload`].
#[derive(Clone, Debug, Default)]
pub struct CpuMesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

impl CpuMesh {
    pub fn new(vertices: Vec<Vertex>, indices: Vec<u32>) -> Self {
        Self { vertices, indices }
    }

    /// Load the first mesh primitive found in a glTF/GLB file. Positions are
    /// required; if normals are absent they are derived as flat per-triangle
    /// normals so the Lambert shader still works.
    pub fn from_gltf_file(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let (doc, buffers, _images) = gltf::import(path.as_ref())?;
        Self::from_gltf_doc(&doc, &buffers)
    }

    /// Load the first mesh primitive from already-parsed glTF data.
    pub fn from_gltf_doc(
        doc: &gltf::Document,
        buffers: &[gltf::buffer::Data],
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

                let vertices = positions
                    .iter()
                    .zip(normals.iter())
                    .map(|(p, n)| Vertex {
                        position: *p,
                        normal: *n,
                    })
                    .collect();
                return Ok(Self { vertices, indices });
            }
        }
        anyhow::bail!("glTF document contains no mesh primitive with positions")
    }

    /// Upload to the GPU, producing a drawable [`GpuMesh`].
    pub fn upload(&self, device: &wgpu::Device, label: &str) -> GpuMesh {
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(&format!("{label}.vbuf")),
            contents: bytemuck::cast_slice(&self.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(&format!("{label}.ibuf")),
            contents: bytemuck::cast_slice(&self.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        GpuMesh {
            vertex_buffer,
            index_buffer,
            index_count: self.indices.len() as u32,
        }
    }
}

/// GPU-resident mesh ready to draw.
#[derive(Debug)]
pub struct GpuMesh {
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: wgpu::Buffer,
    pub index_count: u32,
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
