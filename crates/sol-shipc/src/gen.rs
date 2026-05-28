//! `gen-test-ship` subcommand: procedurally build the test interceptor's
//! authored `model.glb`, then verify it round-trips through the `gltf` crate
//! with all required anchor node names present.

use crate::geometry::{self, HullKind};
use crate::glb;
use anyhow::{bail, Context};
use sol_assets::{
    Build, Capacitor, Cargo, Collider, Conduit, Crew, Hardpoint, Hull, Mobility, PowerGrid,
    Production, Reactor, Shields, ShipDef, Subsystem,
};
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

/// Per-class authoring spec for the placeholder fleet. Stats mirror the demo
/// values in `sol-sim`'s registry; the full pipeline (sim loading compiled
/// `.ship` data) folds these together later.
#[derive(Clone, Copy)]
struct ClassSpec {
    id: &'static str,
    name: &'static str,
    class: &'static str,
    hull: HullKind,
    accent: [u8; 3],
    mass: f32,
    max_speed: f32,
    accel: f32,
    turn_deg: f32,
    crew: u32,
    matter: f32,
    build_time: f32,
    cargo: Option<f32>,
    bays: Option<u32>,
}

const SPECS: [ClassSpec; 8] = [
    ClassSpec {
        id: "fighter",
        name: "Fighter",
        class: "strike",
        hull: HullKind::Fighter,
        accent: [60, 150, 220],
        mass: 18.0,
        max_speed: 16.0,
        accel: 24.0,
        turn_deg: 130.0,
        crew: 2,
        matter: 30.0,
        build_time: 12.0,
        cargo: None,
        bays: None,
    },
    ClassSpec {
        id: "bomber",
        name: "Bomber",
        class: "strike",
        hull: HullKind::Bomber,
        accent: [210, 120, 40],
        mass: 30.0,
        max_speed: 12.0,
        accel: 16.0,
        turn_deg: 90.0,
        crew: 3,
        matter: 50.0,
        build_time: 18.0,
        cargo: None,
        bays: None,
    },
    ClassSpec {
        id: "corvette",
        name: "Corvette",
        class: "light_line",
        hull: HullKind::Corvette,
        accent: [80, 200, 150],
        mass: 220.0,
        max_speed: 10.0,
        accel: 12.0,
        turn_deg: 70.0,
        crew: 6,
        matter: 120.0,
        build_time: 28.0,
        cargo: None,
        bays: None,
    },
    ClassSpec {
        id: "resourcer",
        name: "Resourcer",
        class: "utility",
        hull: HullKind::Resourcer,
        accent: [220, 200, 90],
        mass: 140.0,
        max_speed: 8.0,
        accel: 9.0,
        turn_deg: 55.0,
        crew: 3,
        matter: 80.0,
        build_time: 22.0,
        cargo: Some(200.0),
        bays: None,
    },
    ClassSpec {
        id: "frigate_general",
        name: "General Frigate",
        class: "frigate",
        hull: HullKind::FrigateGeneral,
        accent: [150, 160, 180],
        mass: 4000.0,
        max_speed: 6.0,
        accel: 5.0,
        turn_deg: 30.0,
        crew: 18,
        matter: 300.0,
        build_time: 50.0,
        cargo: None,
        bays: None,
    },
    ClassSpec {
        id: "frigate_missile",
        name: "Missile Frigate",
        class: "frigate",
        hull: HullKind::FrigateMissile,
        accent: [200, 80, 90],
        mass: 4200.0,
        max_speed: 5.5,
        accel: 4.5,
        turn_deg: 28.0,
        crew: 18,
        matter: 340.0,
        build_time: 55.0,
        cargo: None,
        bays: None,
    },
    ClassSpec {
        id: "capital_destroyer",
        name: "Destroyer",
        class: "capital",
        hull: HullKind::CapitalDestroyer,
        accent: [170, 120, 210],
        mass: 16000.0,
        max_speed: 4.0,
        accel: 3.0,
        turn_deg: 14.0,
        crew: 60,
        matter: 900.0,
        build_time: 110.0,
        cargo: None,
        bays: None,
    },
    ClassSpec {
        id: "carrier",
        name: "Carrier",
        class: "ark",
        hull: HullKind::Carrier,
        accent: [120, 180, 230],
        mass: 40000.0,
        max_speed: 3.0,
        accel: 2.0,
        turn_deg: 8.0,
        crew: 120,
        matter: 5000.0,
        build_time: 300.0,
        cargo: None,
        bays: Some(2),
    },
];

/// Build a placeholder `ShipDef` for a class: per-class stats plus the standard
/// power grid / colliders / hardpoints shared by every generated hull (all hulls
/// carry the same anchor nodes, so this validates against any of them).
fn ship_def(spec: &ClassSpec) -> ShipDef {
    ShipDef {
        schema_version: 2,
        id: spec.id.to_string(),
        name: spec.name.to_string(),
        faction: "player".to_string(),
        class: spec.class.to_string(),
        model: "model.glb".to_string(),
        mass: spec.mass,
        mobility: Mobility {
            max_speed: spec.max_speed,
            accel: spec.accel,
            turn_rate_deg: spec.turn_deg,
        },
        build: Build {
            matter: spec.matter,
            time: spec.build_time,
        },
        cargo: spec.cargo.map(|capacity| Cargo { capacity }),
        production: spec.bays.map(|bays| Production { bays }),
        crew: Crew {
            min: spec.crew,
            optimal: spec.crew,
        },
        hull: Hull {
            sections: vec!["fore".to_string(), "aft".to_string()],
            integrity: 80.0,
        },
        colliders: vec![Collider {
            kind: "convexHull".to_string(),
            node: "hull".to_string(),
        }],
        shields: Shields {
            facings: 4,
            capacity: 40.0,
            regen: 4.0,
        },
        power_grid: PowerGrid {
            reactor: Reactor {
                node: "reactor_core".to_string(),
                output: 20.0,
            },
            capacitors: vec![Capacitor {
                node: "cap_main".to_string(),
                capacity: 15.0,
            }],
            conduits: vec![
                conduit("reactor_core", "bus_main", 20.0),
                conduit("bus_main", "engines", 10.0),
                conduit("bus_main", "shields", 8.0),
                conduit("bus_main", "sensors", 4.0),
            ],
            subsystems: vec![
                subsystem("engines", "engine_mount", 8.0, 2),
                subsystem("shields", "shield_emitter", 5.0, 3),
                subsystem("sensors", "sensor_array", 2.0, 1),
            ],
        },
        hardpoints: vec![Hardpoint {
            id: "nose_gun".to_string(),
            node: "hp_nose".to_string(),
            mount: "fixed".to_string(),
            weapon: "pulse_cannon".to_string(),
            arc_deg: 10.0,
            draw_per_shot: 4.0,
        }],
    }
}

fn conduit(from: &str, to: &str, capacity: f32) -> Conduit {
    Conduit {
        from: from.to_string(),
        to: to.to_string(),
        capacity,
    }
}

fn subsystem(id: &str, node: &str, draw: f32, priority: i32) -> Subsystem {
    Subsystem {
        id: id.to_string(),
        node: node.to_string(),
        draw,
        priority,
    }
}

/// Generate the full placeholder fleet: a distinct hull GLB + `ship.json` per
/// class under `assets/ships/<id>/`.
pub fn run_ships() -> anyhow::Result<()> {
    for spec in SPECS {
        let dir = Path::new("assets/ships").join(spec.id);
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;

        let hull = geometry::build_hull(spec.hull);
        let glb_bytes = glb::build_glb(&hull, spec.accent).context("building GLB")?;
        let glb_path = dir.join("model.glb");
        std::fs::write(&glb_path, &glb_bytes)
            .with_context(|| format!("writing {}", glb_path.display()))?;

        let def = ship_def(&spec);
        let json = serde_json::to_string_pretty(&def).context("serializing ship.json")? + "\n";
        std::fs::write(dir.join("ship.json"), json)
            .with_context(|| format!("writing {}/ship.json", dir.display()))?;

        verify_node_names(&glb_path)?;
        println!("wrote {} ({} bytes)", spec.id, glb_bytes.len());
    }
    println!("generated {} class ships", SPECS.len());
    Ok(())
}

/// Generate `assets/ships/test_interceptor/model.glb`.
pub fn run() -> anyhow::Result<()> {
    let out_path = Path::new("assets/ships/test_interceptor/model.glb");

    let hull = geometry::build_interceptor_hull();
    let glb_bytes = glb::build_glb(&hull, [40, 95, 150]).context("building GLB")?;

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
