//! Build a binary glTF (`.glb`) for the test ship using `gltf-json`, then
//! assemble the GLB container by hand (12-byte header + JSON chunk + BIN chunk).
//!
//! The hull carries a baked pixel-art texture: a small RGBA image is generated
//! offline (NOT at runtime, per CLAUDE.md), PNG-encoded, and embedded in the
//! GLB as an image + sampler + texture + material, with per-vertex UVs. The
//! game renderer samples it with nearest filtering for a crisp pixel look.

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
    // Pixel-art hull texture, generated offline and PNG-encoded for embedding.
    let (tex_w, tex_h, tex_rgba) = hull_texture();
    let png_bytes = encode_png(tex_w, tex_h, &tex_rgba).context("encoding hull texture PNG")?;

    // --- Binary buffer: [positions][normals][uvs][indices][png] ---
    let positions_bytes: &[u8] = bytemuck_cast(&mesh.positions);
    let normals_bytes: &[u8] = bytemuck_cast(&mesh.normals);
    let uvs_bytes: &[u8] = bytemuck_cast(&mesh.uvs);
    let indices_bytes: &[u8] = bytemuck_cast(&mesh.indices);

    let mut bin = Vec::new();
    let pos_offset = bin.len();
    bin.extend_from_slice(positions_bytes);
    let norm_offset = bin.len();
    bin.extend_from_slice(normals_bytes);
    let uv_offset = bin.len();
    bin.extend_from_slice(uvs_bytes);
    let idx_offset = bin.len();
    bin.extend_from_slice(indices_bytes);
    // Keep the image bufferView 4-byte aligned.
    while bin.len() % 4 != 0 {
        bin.push(0);
    }
    let img_offset = bin.len();
    bin.extend_from_slice(&png_bytes);
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

    let array_view =
        |offset: usize, length: usize, stride: Option<usize>, target| json::buffer::View {
            buffer: json::Index::new(0),
            byte_length: USize64(length as u64),
            byte_offset: Some(USize64(offset as u64)),
            byte_stride: stride.map(json::buffer::Stride),
            name: None,
            target: Some(Valid(target)),
            extensions: None,
            extras: Default::default(),
        };
    let pos_view = array_view(
        pos_offset,
        positions_bytes.len(),
        Some(12),
        json::buffer::Target::ArrayBuffer,
    );
    let norm_view = array_view(
        norm_offset,
        normals_bytes.len(),
        Some(12),
        json::buffer::Target::ArrayBuffer,
    );
    let uv_view = array_view(
        uv_offset,
        uvs_bytes.len(),
        Some(8),
        json::buffer::Target::ArrayBuffer,
    );
    let idx_view = array_view(
        idx_offset,
        indices_bytes.len(),
        None,
        json::buffer::Target::ElementArrayBuffer,
    );
    // Image bufferView: raw bytes, no stride, no target.
    let img_view = json::buffer::View {
        buffer: json::Index::new(0),
        byte_length: USize64(png_bytes.len() as u64),
        byte_offset: Some(USize64(img_offset as u64)),
        byte_stride: None,
        name: None,
        target: None,
        extensions: None,
        extras: Default::default(),
    };

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
    let uv_accessor = json::Accessor {
        buffer_view: Some(json::Index::new(2)),
        byte_offset: Some(USize64(0)),
        count: USize64(mesh.uvs.len() as u64),
        component_type: Valid(json::accessor::GenericComponentType(
            json::accessor::ComponentType::F32,
        )),
        type_: Valid(json::accessor::Type::Vec2),
        min: None,
        max: None,
        name: None,
        normalized: false,
        sparse: None,
        extensions: None,
        extras: Default::default(),
    };
    let idx_accessor = json::Accessor {
        buffer_view: Some(json::Index::new(3)),
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

    // Image + sampler + texture + material referencing the baked PNG.
    let image = json::Image {
        buffer_view: Some(json::Index::new(4)),
        mime_type: Some(json::image::MimeType("image/png".to_string())),
        name: None,
        uri: None,
        extensions: None,
        extras: Default::default(),
    };
    let sampler = json::texture::Sampler {
        mag_filter: Some(Valid(json::texture::MagFilter::Nearest)),
        min_filter: Some(Valid(json::texture::MinFilter::Nearest)),
        wrap_s: Valid(json::texture::WrappingMode::Repeat),
        wrap_t: Valid(json::texture::WrappingMode::Repeat),
        name: None,
        extensions: None,
        extras: Default::default(),
    };
    let texture = json::Texture {
        sampler: Some(json::Index::new(0)),
        source: json::Index::new(0),
        name: None,
        extensions: None,
        extras: Default::default(),
    };
    let material = json::Material {
        pbr_metallic_roughness: json::material::PbrMetallicRoughness {
            base_color_texture: Some(json::texture::Info {
                index: json::Index::new(0),
                tex_coord: 0,
                extensions: None,
                extras: Default::default(),
            }),
            metallic_factor: json::material::StrengthFactor(0.0),
            roughness_factor: json::material::StrengthFactor(0.9),
            ..Default::default()
        },
        ..Default::default()
    };

    let mut attributes = std::collections::BTreeMap::new();
    attributes.insert(Valid(json::mesh::Semantic::Positions), json::Index::new(0));
    attributes.insert(Valid(json::mesh::Semantic::Normals), json::Index::new(1));
    attributes.insert(Valid(json::mesh::Semantic::TexCoords(0)), json::Index::new(2));

    let primitive = json::mesh::Primitive {
        attributes,
        indices: Some(json::Index::new(3)),
        material: Some(json::Index::new(0)),
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
        accessors: vec![pos_accessor, norm_accessor, uv_accessor, idx_accessor],
        buffers: vec![buffer],
        buffer_views: vec![pos_view, norm_view, uv_view, idx_view, img_view],
        images: vec![image],
        samplers: vec![sampler],
        textures: vec![texture],
        materials: vec![material],
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

/// A small, deterministic pixel-art hull texture (RGBA8): steel plating with
/// panel grid lines, a blue accent band, and scattered warm lights. Authored
/// offline and baked into the GLB; never generated at runtime.
fn hull_texture() -> (u32, u32, Vec<u8>) {
    const S: u32 = 32;
    let mut px = vec![0u8; (S * S * 4) as usize];
    for y in 0..S {
        for x in 0..S {
            let i = ((y * S + x) * 4) as usize;
            // Base steel with a subtle 1px dither.
            let dither = ((x ^ y) & 1) as i32 * 6;
            let mut r = 90 + dither;
            let mut g = 100 + dither;
            let mut b = 116 + dither;
            // Panel grid lines every 8 px.
            if x % 8 == 0 || y % 8 == 0 {
                r -= 34;
                g -= 34;
                b -= 34;
            }
            // A blue accent band.
            if (10..=13).contains(&y) {
                r = 40;
                g = 95;
                b = 150;
            }
            // Scattered warm "lights".
            if x % 8 == 4 && y % 8 == 4 {
                r = 210;
                g = 200;
                b = 130;
            }
            px[i] = r.clamp(0, 255) as u8;
            px[i + 1] = g.clamp(0, 255) as u8;
            px[i + 2] = b.clamp(0, 255) as u8;
            px[i + 3] = 255;
        }
    }
    (S, S, px)
}

/// Encode RGBA8 pixels to PNG bytes for embedding in the GLB.
fn encode_png(width: u32, height: u32, rgba: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().context("writing PNG header")?;
        writer
            .write_image_data(rgba)
            .context("writing PNG image data")?;
    }
    Ok(out)
}
