//! `gen-test-ship` subcommand: procedurally build the test interceptor's
//! authored `model.glb`, then verify it round-trips through the `gltf` crate
//! with all required anchor node names present.

use crate::{geometry, glb};
use anyhow::{bail, Context};
use std::path::Path;

/// Node names the generated GLB must contain (the hull mesh plus every anchor
/// referenced by `ship.json`). Kept in sync with `assets/.../ship.json`.
pub const REQUIRED_NODES: &[&str] = &[
    "hull",
    "reactor_core",
    "cap_main",
    "engine_mount",
    "shield_emitter",
    "sensor_array",
    "hp_nose",
];

/// Generate `assets/ships/test_interceptor/model.glb`.
pub fn run() -> anyhow::Result<()> {
    let out_path = Path::new("assets/ships/test_interceptor/model.glb");

    let hull = geometry::build_interceptor_hull();
    let glb_bytes = glb::build_glb(&hull).context("building GLB")?;

    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(out_path, &glb_bytes)
        .with_context(|| format!("writing {}", out_path.display()))?;
    println!(
        "wrote {} ({} bytes, {} vertices, {} triangles)",
        out_path.display(),
        glb_bytes.len(),
        hull.positions.len(),
        hull.indices.len() / 3,
    );

    // Round-trip verification: read it back and assert node names. Fail loudly.
    verify_node_names(out_path)?;
    println!(
        "verified: all {} required nodes present",
        REQUIRED_NODES.len()
    );
    Ok(())
}

/// Load `path` with the `gltf` crate and assert every required node name is
/// present. Returns an error naming the first missing node.
pub fn verify_node_names(path: &Path) -> anyhow::Result<()> {
    let (doc, _buffers, _images) =
        gltf::import(path).with_context(|| format!("re-reading {}", path.display()))?;

    let present: std::collections::HashSet<&str> = doc.nodes().filter_map(|n| n.name()).collect();

    let missing: Vec<&str> = REQUIRED_NODES
        .iter()
        .copied()
        .filter(|n| !present.contains(n))
        .collect();

    if !missing.is_empty() {
        bail!(
            "generated GLB {} is missing required node(s): {}",
            path.display(),
            missing.join(", ")
        );
    }

    // Sanity: the hull node must actually carry mesh geometry.
    let hull_has_mesh = doc
        .nodes()
        .any(|n| n.name() == Some("hull") && n.mesh().is_some());
    if !hull_has_mesh {
        bail!("node \"hull\" exists but carries no mesh");
    }

    // Sanity: the baked pixel-art texture must be present (and decodable, since
    // `gltf::import` above would have failed otherwise).
    if doc.images().count() == 0 {
        bail!("generated GLB has no baked texture image");
    }

    Ok(())
}
