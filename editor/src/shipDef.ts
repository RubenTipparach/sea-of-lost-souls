// Authoring schema shared with the Rust pipeline (sol-shipc / sol-assets).
// This MUST stay in sync with assets/ships/<id>/ship.json - it is the contract
// between the editor (writer) and the deterministic engine (reader).
// See design.md §15 & §16.

export type MountKind = 'fixed' | 'turret';
export type ColliderKind = 'convexHull' | 'box' | 'sphere' | 'capsule';

export interface Crew {
  min: number;
  optimal: number;
}

export interface Hull {
  sections: string[];
  integrity: number;
}

export interface Collider {
  type: ColliderKind;
  node: string;
}

export interface Shields {
  facings: number;
  capacity: number;
  regen: number;
}

export interface Reactor {
  node: string;
  output: number;
}

export interface Capacitor {
  node: string;
  capacity: number;
}

export interface Conduit {
  from: string;
  to: string;
  capacity: number;
}

export interface Subsystem {
  id: string;
  node: string;
  draw: number;
  priority: number;
}

export interface PowerGrid {
  reactor: Reactor;
  capacitors: Capacitor[];
  conduits: Conduit[];
  subsystems: Subsystem[];
}

export interface Hardpoint {
  id: string;
  node: string;
  mount: MountKind;
  weapon: string;
  arcDeg: number;
  drawPerShot: number;
}

export interface ShipDef {
  schemaVersion: number;
  id: string;
  name: string;
  faction: string;
  class: string;
  model: string;
  mass: number;
  crew: Crew;
  hull: Hull;
  colliders: Collider[];
  shields: Shields;
  powerGrid: PowerGrid;
  hardpoints: Hardpoint[];
}

// Canonical default - matches assets/ships/test_interceptor/ship.json so that
// Export produces a valid, pipeline-ready file immediately.
export function defaultShipDef(): ShipDef {
  return {
    schemaVersion: 1,
    id: 'test_interceptor',
    name: 'Test Interceptor',
    faction: 'player',
    class: 'strike',
    model: 'model.glb',
    mass: 60,
    crew: { min: 1, optimal: 1 },
    hull: { sections: ['fore', 'aft'], integrity: 80 },
    colliders: [{ type: 'convexHull', node: 'hull' }],
    shields: { facings: 4, capacity: 40, regen: 4 },
    powerGrid: {
      reactor: { node: 'reactor_core', output: 20 },
      capacitors: [{ node: 'cap_main', capacity: 15 }],
      conduits: [{ from: 'reactor_core', to: 'bus_main', capacity: 20 }],
      subsystems: [
        { id: 'engines', node: 'engine_mount', draw: 8, priority: 2 },
        { id: 'shields', node: 'shield_emitter', draw: 5, priority: 3 },
        { id: 'sensors', node: 'sensor_array', draw: 2, priority: 1 },
      ],
    },
    hardpoints: [
      {
        id: 'nose_gun',
        node: 'hp_nose',
        mount: 'fixed',
        weapon: 'pulse_cannon',
        arcDeg: 10,
        drawPerShot: 4,
      },
    ],
  };
}

export function serializeShipDef(def: ShipDef): string {
  return JSON.stringify(def, null, 2) + '\n';
}
