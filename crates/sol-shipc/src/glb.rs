//! Build a binary glTF (`.glb`) for the test ship using `gltf-json`, then
//! assemble the GLB container by hand (12-byte header + JSON chunk + BIN chunk).

use crate::geometry::Mesh;
use anyhow::Context;
use gltf_json as json;
use json::validation::Checked::Valid;
use json::validation::USize64;

/// Named anchor nodes the pipeline expects, with sensible local positions
/// (ship space: +Z forward, +Y up). These are empty/child nodes (no mesh).
const ANCHORS: &[(&str, [f32; 3])] = &[
    ("reactor_core", [0.0, 0.0, -0.6]),
    ("cap_main", [0.0, -0.1, 0.2]),
    ("engine_mount", [0.0, 0.0, -1.6]),
    ("shield_emitter", [0.0, 0.35, 0.0]),
    ("sensor_array", [0.0, 0.25, 1.0]),
    ("hp_nose", [0.0, 0.0, 2.4]),
];

/// Align a length up to the next multiple of 4 (GLB chunk alignment).
fn align4(n: usize) -> usize {
    (n + 3) & !3
}

/// Build the GLB byte container for the interceptor.
pub fn build_glb(mesh: &Mesh) -> anyhow::Result<Vec<u8>> {
    // --- Binary buffer: [positions f32x3][normals f32x3][indices u32] ---
    let positions_bytes: &[u8] = bytemuck_cast(&mesh.positions);
    let normals_bytes: &[u8] = bytemuck_cast(&mesh.normals);
    let indices_bytes: &[u8] = bytemuck_cast(&mesh.indices);

    let mut bin = Vec::new();
    let pos_offset = bin.len();
    bin.extend_from_slice(positions_bytes);
    let norm_offset = bin.len();
    bin.extend_from_slice(normals_bytes);
    let idx_offset = bin.len();
    bin.extend_from_slice(indices_bytes);
    // BIN chunk must be 4-byte aligned; pad with zeros.
    while bin.len() % 4 != 0 {
        bin.push(0);
    }
    let bin_len = bin.len();

    // Position min/max are REQUIRED by the glTF spec for the POSITION accessor.
    let (min, max) = position_bounds(&mesh.positions);

    // --- JSON document ---
    let buffer = json::Buffer {
        byte_length: USize64(bin_len as u64),
        name: None,
        // No URI: data lives in the GLB BIN chunk.
        uri: None,
        extensions: None,
        extras: Default::default(),
    };

    let view = |offset: usize, length: usize, stride: Option<usize>, target| json::buffer::View {
        buffer: json::Index::new(0),
        byte_length: USize64(length as u64),
        byte_offset: Some(USize64(offset as u64)),
        byte_stride: stride.map(json::buffer::Stride),
        name: None,
        target: Some(Valid(target)),
        extensions: None,
        extras: Default::default(),
    };
    let pos_view = view(
        pos_offset,
        positions_bytes.len(),
        Some(12),
        json::buffer::Target::ArrayBuffer,
    );
    let norm_view = view(
        norm_offset,
        normals_bytes.len(),
        Some(12),
        json::buffer::Target::ArrayBuffer,
    );
    let idx_view = view(
        idx_offset,
        indices_bytes.len(),
        None,
        json::buffer::Target::ElementArrayBuffer,
    );

    let pos_accessor = json::Accessor {
        buffer_view: Some(json::Index::new(0)),
        byte_offset: Some(USize64(0)),
        count: USize64(mesh.positions.len() as u64),
        component_type: Valid(json::accessor::GenericComponentType(
            json::accessor::ComponentType::F32,
        )),
        type_: Valid(json::accessor::Type::Vec3),
        min: Some(json::serialize::to_value(min).unwrap()),
        max: Some(json::serialize::to_value(max).unwrap()),
        name: None,
        normalized: false,
        sparse: None,
        extensions: None,
        extras: Default::default(),
    };
    let norm_accessor = json::Accessor {
        buffer_view: Some(json::Index::new(1)),
        byte_offset: Some(USize64(0)),
        count: USize64(mesh.normals.len() as u64),
        component_type: Valid(json::accessor::GenericComponentType(
            json::accessor::ComponentType::F32,
        )),
        type_: Valid(json::accessor::Type::Vec3),
        min: None,
        max: None,
        name: None,
        normalized: false,
        sparse: None,
        extensions: None,
        extras: Default::default(),
    };
    let idx_accessor = json::Accessor {
        buffer_view: Some(json::Index::new(2)),
        byte_offset: Some(USize64(0)),
        count: USize64(mesh.indices.len() as u64),
        component_type: Valid(json::accessor::GenericComponentType(
            json::accessor::ComponentType::U32,
        )),
        type_: Valid(json::accessor::Type::Scalar),
        min: None,
        max: None,
        name: None,
        normalized: false,
        sparse: None,
        extensions: None,
        extras: Default::default(),
    };

    let mut attributes = std::collections::BTreeMap::new();
    attributes.insert(Valid(json::mesh::Semantic::Positions), json::Index::new(0));
    attributes.insert(Valid(json::mesh::Semantic::Normals), json::Index::new(1));

    let primitive = json::mesh::Primitive {
        attributes,
        indices: Some(json::Index::new(2)),
        material: None,
        mode: Valid(json::mesh::Mode::Triangles),
        targets: None,
        extensions: None,
        extras: Default::default(),
    };
    let gltf_mesh = json::Mesh {
        primitives: vec![primitive],
        weights: None,
        name: Some("hull_mesh".to_string()),
        extensions: None,
        extras: Default::default(),
    };

    // Nodes: index 0 is the hull (carries the mesh). Anchors are children.
    let mut nodes = Vec::new();
    let hull_node = json::Node {
        mesh: Some(json::Index::new(0)),
        name: Some("hull".to_string()),
        // Children indices are 1..=ANCHORS.len().
        children: Some((1..=ANCHORS.len() as u32).map(json::Index::new).collect()),
        camera: None,
        extensions: None,
        extras: Default::default(),
        matrix: None,
        rotation: None,
        scale: None,
        translation: None,
        skin: None,
        weights: None,
    };
    nodes.push(hull_node);
    for (name, pos) in ANCHORS {
        nodes.push(json::Node {
            mesh: None,
            name: Some((*name).to_string()),
            children: None,
            camera: None,
            extensions: None,
            extras: Default::default(),
            matrix: None,
            rotation: None,
            scale: None,
            translation: Some(*pos),
            skin: None,
            weights: None,
        });
    }

    let scene = json::Scene {
        nodes: vec![json::Index::new(0)],
        name: Some("test_interceptor".to_string()),
        extensions: None,
        extras: Default::default(),
    };

    let root = json::Root {
        accessors: vec![pos_accessor, norm_accessor, idx_accessor],
        buffers: vec![buffer],
        buffer_views: vec![pos_view, norm_view, idx_view],
        meshes: vec![gltf_mesh],
        nodes,
        scenes: vec![scene],
        scene: Some(json::Index::new(0)),
        ..Default::default()
    };

    let json_bytes = root.to_vec().context("serializing glTF JSON")?;
    Ok(assemble_glb(&json_bytes, &bin))
}

/// Assemble the 12-byte GLB header + JSON chunk + BIN chunk.
/// JSON chunk is padded with spaces (0x20); BIN chunk with zeros.
fn assemble_glb(json_bytes: &[u8], bin: &[u8]) -> Vec<u8> {
    const GLB_MAGIC: u32 = 0x4654_6C67; // "glTF"
    const CHUNK_JSON: u32 = 0x4E4F_534A; // "JSON"
    const CHUNK_BIN: u32 = 0x004E_4942; // "BIN\0"

    let json_padded_len = align4(json_bytes.len());
    let bin_padded_len = align4(bin.len());

    let total_len = 12 + (8 + json_padded_len) + (8 + bin_padded_len);

    let mut out = Vec::with_capacity(total_len);
    // Header.
    out.extend_from_slice(&GLB_MAGIC.to_le_bytes());
    out.extend_from_slice(&2u32.to_le_bytes()); // version 2
    out.extend_from_slice(&(total_len as u32).to_le_bytes());

    // JSON chunk.
    out.extend_from_slice(&(json_padded_len as u32).to_le_bytes());
    out.extend_from_slice(&CHUNK_JSON.to_le_bytes());
    out.extend_from_slice(json_bytes);
    out.extend(std::iter::repeat_n(
        b' ',
        json_padded_len - json_bytes.len(),
    ));

    // BIN chunk.
    out.extend_from_slice(&(bin_padded_len as u32).to_le_bytes());
    out.extend_from_slice(&CHUNK_BIN.to_le_bytes());
    out.extend_from_slice(bin);
    out.extend(std::iter::repeat_n(0u8, bin_padded_len - bin.len()));

    out
}

/// Component-wise min/max of all positions.
fn position_bounds(positions: &[[f32; 3]]) -> ([f32; 3], [f32; 3]) {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for p in positions {
        for i in 0..3 {
            min[i] = min[i].min(p[i]);
            max[i] = max[i].max(p[i]);
        }
    }
    (min, max)
}

/// Reinterpret a slice of POD arrays as bytes (little-endian; glTF is LE).
fn bytemuck_cast<T: bytemuck::Pod>(slice: &[T]) -> &[u8] {
    bytemuck::cast_slice(slice)
}
