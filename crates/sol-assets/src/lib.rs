//! `sol-assets` — the shared asset contract for Sea of Lost Souls.
//!
//! These `serde` types mirror the authoring `ship.json` schema (see
//! `design.md` §15) and are the contract shared by `sol-shipc` (the compiler /
//! validator) and the runtime app. Authoring format is JSON (camelCase);
//! compiled runtime data is binary (`bincode`).

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Top-level authored ship definition. Fields use camelCase on the wire to
/// match the editor's export and `design.md` §15.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ShipDef {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub faction: String,
    pub class: String,
    /// Relative path (within the ship directory) to the GLB mesh.
    pub model: String,
    pub mass: f32,
    pub crew: Crew,
    pub hull: Hull,
    pub colliders: Vec<Collider>,
    pub shields: Shields,
    pub power_grid: PowerGrid,
    pub hardpoints: Vec<Hardpoint>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Crew {
    pub min: u32,
    pub optimal: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Hull {
    pub sections: Vec<String>,
    pub integrity: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Collider {
    /// Collider kind, e.g. `convexHull` or `box`.
    #[serde(rename = "type")]
    pub kind: String,
    /// GLB node the collider geometry/transform is taken from.
    pub node: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Shields {
    pub facings: u32,
    pub capacity: f32,
    pub regen: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PowerGrid {
    pub reactor: Reactor,
    pub capacitors: Vec<Capacitor>,
    pub conduits: Vec<Conduit>,
    pub subsystems: Vec<Subsystem>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Reactor {
    /// GLB node placing the reactor in the hull.
    pub node: String,
    pub output: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Capacitor {
    pub node: String,
    pub capacity: f32,
}

/// A power conduit (graph edge). `from`/`to` are logical power-graph ids:
/// reactor ids, subsystem ids, or the virtual `bus_main` node. They are NOT
/// necessarily GLB node names (e.g. `bus_main` is logical-only).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Conduit {
    pub from: String,
    pub to: String,
    pub capacity: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Subsystem {
    /// Logical power-graph id (e.g. `engines`).
    pub id: String,
    /// GLB node placing the subsystem in the hull.
    pub node: String,
    pub draw: f32,
    pub priority: i32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Hardpoint {
    pub id: String,
    /// GLB node placing the hardpoint in the hull.
    pub node: String,
    /// Mount kind, e.g. `fixed` or `turret`.
    pub mount: String,
    pub weapon: String,
    pub arc_deg: f32,
    pub draw_per_shot: f32,
}

impl ShipDef {
    /// Parse a `ship.json` from a string.
    pub fn from_json_str(s: &str) -> anyhow::Result<Self> {
        Ok(serde_json::from_str(s)?)
    }

    /// Load and parse a `ship.json` from disk.
    pub fn load_json(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("reading {}: {e}", path.display()))?;
        Self::from_json_str(&text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The canonical test ship must round-trip exactly through these structs.
    const TEST_SHIP: &str = include_str!("../../../assets/ships/test_interceptor/ship.json");

    #[test]
    fn deserializes_test_interceptor() {
        let def = ShipDef::from_json_str(TEST_SHIP).expect("parse");
        assert_eq!(def.schema_version, 1);
        assert_eq!(def.id, "test_interceptor");
        assert_eq!(def.class, "strike");
        assert_eq!(def.mass, 60.0);
        assert_eq!(def.crew.min, 1);
        assert_eq!(def.hull.sections, vec!["fore", "aft"]);
        assert_eq!(def.colliders[0].kind, "convexHull");
        assert_eq!(def.colliders[0].node, "hull");
        assert_eq!(def.shields.facings, 4);
        assert_eq!(def.power_grid.reactor.node, "reactor_core");
        assert_eq!(def.power_grid.subsystems.len(), 3);
        assert_eq!(def.power_grid.conduits.len(), 4);
        assert_eq!(def.hardpoints[0].id, "nose_gun");
        assert_eq!(def.hardpoints[0].arc_deg, 10.0);
        assert_eq!(def.hardpoints[0].draw_per_shot, 4.0);
    }
}
