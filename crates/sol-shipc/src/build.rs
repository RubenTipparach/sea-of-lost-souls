//! `build` subcommand: the asset-compile gate. Parses each ship's `ship.json`,
//! loads its `model.glb`, validates GLB-placement nodes and the power-graph
//! connectivity, then emits a compiled `.ship` (bincode) and a copy of the GLB.
//!
//! Any validation failure returns an error; `main` maps that to a nonzero exit.

use anyhow::{bail, Context};
use sol_assets::ShipDef;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

/// The logical bus node every conduit fans out from. It is NOT a GLB node, so
/// it is deliberately excluded from GLB node-existence checks.
const VIRTUAL_BUS: &str = "bus_main";

/// Compile every `<ships_dir>/*/ship.json` into `<out_dir>`.
pub fn run(ships_dir: &Path, out_dir: &Path) -> anyhow::Result<()> {
    if !ships_dir.is_dir() {
        bail!("ships dir {} is not a directory", ships_dir.display());
    }
    std::fs::create_dir_all(out_dir)
        .with_context(|| format!("creating out dir {}", out_dir.display()))?;

    // Stable, sorted iteration so output is reproducible.
    let mut ship_dirs: Vec<PathBuf> = std::fs::read_dir(ships_dir)
        .with_context(|| format!("reading {}", ships_dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    ship_dirs.sort();

    let mut compiled = 0usize;
    for dir in ship_dirs {
        let ship_json = dir.join("ship.json");
        if !ship_json.exists() {
            // Not a ship directory; skip silently.
            continue;
        }
        compile_one(&dir, &ship_json, out_dir)
            .with_context(|| format!("compiling {}", dir.display()))?;
        compiled += 1;
    }

    if compiled == 0 {
        bail!("no ships found under {}", ships_dir.display());
    }
    println!("compiled {compiled} ship(s) into {}", out_dir.display());
    Ok(())
}

fn compile_one(dir: &Path, ship_json: &Path, out_dir: &Path) -> anyhow::Result<()> {
    let def = ShipDef::load_json(ship_json)?;
    println!("- {} ({})", def.id, def.name);

    // Load the sibling GLB referenced by `model`.
    let glb_path = dir.join(&def.model);
    if !glb_path.exists() {
        bail!("model {} not found", glb_path.display());
    }
    let (doc, _buffers, _images) =
        gltf::import(&glb_path).with_context(|| format!("loading {}", glb_path.display()))?;
    let glb_nodes: HashSet<String> = doc
        .nodes()
        .filter_map(|n| n.name().map(|s| s.to_string()))
        .collect();

    validate_glb_nodes(&def, &glb_nodes)?;
    validate_power_graph(&def)?;

    // Emit compiled artifact: bincode of the validated ShipDef.
    let ship_out = out_dir.join(format!("{}.ship", def.id));
    let bytes = bincode::serialize(&def).context("bincode-serializing ShipDef")?;
    std::fs::write(&ship_out, &bytes).with_context(|| format!("writing {}", ship_out.display()))?;

    // Copy the validated GLB alongside it.
    let glb_out = out_dir.join(format!("{}.glb", def.id));
    std::fs::copy(&glb_path, &glb_out)
        .with_context(|| format!("copying GLB to {}", glb_out.display()))?;

    println!(
        "  -> {} ({} bytes) + {}",
        ship_out.display(),
        bytes.len(),
        glb_out.display()
    );
    Ok(())
}

/// Every node referenced for GLB placement must exist in the GLB. `bus_main`
/// is logical-only and is never checked here.
fn validate_glb_nodes(def: &ShipDef, glb_nodes: &HashSet<String>) -> anyhow::Result<()> {
    let mut refs: Vec<(&str, &str)> = Vec::new();
    for c in &def.colliders {
        refs.push(("collider", &c.node));
    }
    refs.push(("reactor", &def.power_grid.reactor.node));
    for c in &def.power_grid.capacitors {
        refs.push(("capacitor", &c.node));
    }
    for s in &def.power_grid.subsystems {
        refs.push(("subsystem", &s.node));
    }
    for h in &def.hardpoints {
        refs.push(("hardpoint", &h.node));
    }

    let mut missing = Vec::new();
    for (kind, node) in refs {
        debug_assert_ne!(
            node, VIRTUAL_BUS,
            "a GLB-placement field must never reference the virtual bus"
        );
        if !glb_nodes.contains(node) {
            missing.push(format!("{kind} node \"{node}\""));
        }
    }
    if !missing.is_empty() {
        bail!(
            "GLB-placement node(s) missing from model: {}",
            missing.join(", ")
        );
    }
    Ok(())
}

/// Validate the power-graph connectivity LOGICALLY: the reactor id and every
/// subsystem id must be reachable through conduits via the virtual `bus_main`.
///
/// Conduits are undirected for connectivity purposes (power can be traced
/// either way through the bus). Node ids in this graph are logical power ids
/// (reactor node id, subsystem ids, `bus_main`) — NOT GLB nodes.
fn validate_power_graph(def: &ShipDef) -> anyhow::Result<()> {
    let grid = &def.power_grid;

    // Logical id of the reactor end of the graph.
    let reactor_id = grid.reactor.node.as_str();

    // Build an undirected adjacency list from conduits.
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut known: HashSet<&str> = HashSet::new();

    for c in &grid.conduits {
        adj.entry(c.from.as_str()).or_default().push(c.to.as_str());
        adj.entry(c.to.as_str()).or_default().push(c.from.as_str());
        known.insert(c.from.as_str());
        known.insert(c.to.as_str());
    }

    // The bus must actually appear in the conduit graph.
    if !known.contains(VIRTUAL_BUS) {
        bail!(
            "power graph has no \"{VIRTUAL_BUS}\" node; conduits must route through the virtual bus"
        );
    }

    // BFS reachability from the reactor id.
    let reachable = reachable_from(&adj, reactor_id);

    let mut unreachable = Vec::new();
    if !reachable.contains(reactor_id) {
        // reactor id not even present in the conduit graph
        unreachable.push(format!("reactor \"{reactor_id}\""));
    }
    for s in &grid.subsystems {
        if !reachable.contains(s.id.as_str()) {
            unreachable.push(format!("subsystem \"{}\"", s.id));
        }
    }

    if !unreachable.is_empty() {
        bail!(
            "power graph not connected: {} unreachable from reactor through \"{VIRTUAL_BUS}\"",
            unreachable.join(", ")
        );
    }

    Ok(())
}

/// BFS over the undirected adjacency list; returns the set of reachable nodes
/// (including `start` iff it appears in the graph).
fn reachable_from<'a>(adj: &HashMap<&'a str, Vec<&'a str>>, start: &'a str) -> HashSet<&'a str> {
    let mut seen = HashSet::new();
    let mut queue = VecDeque::new();
    if adj.contains_key(start) {
        seen.insert(start);
        queue.push_back(start);
    }
    while let Some(node) = queue.pop_front() {
        if let Some(neighbors) = adj.get(node) {
            for &n in neighbors {
                if seen.insert(n) {
                    queue.push_back(n);
                }
            }
        }
    }
    seen
}
