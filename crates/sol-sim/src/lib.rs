//! `sol-sim` - the deterministic simulation core of Sea of Lost Souls.
//!
//! INVARIANT: this crate MUST stay deterministic and pure. No rendering, no
//! wall-clock (`SystemTime`/`Instant`), no OS/thread RNG, no unordered
//! iteration that affects state. Bit-identical simulation across native and
//! wasm is what makes lockstep multiplayer and reproducible tests possible
//! (see `design.md` §12-§13). Randomness flows only from the seeded [`Rng`]
//! below.

#![forbid(unsafe_code)]

use glam::{Quat, Vec3};
use std::collections::HashMap;

pub mod rng;

pub use rng::Rng;

/// Fixed simulation rate. The sim advances in steps of [`TICK_DT`] seconds; the
/// renderer interpolates between the previous and current state, so visual
/// smoothness is independent of this rate. 60 Hz gives crisp input-to-sim
/// latency; drop it to 30 (one line) if single-threaded wasm CPU or future
/// lockstep command bandwidth becomes the constraint.
pub const TICK_HZ: u32 = 60;
/// Seconds per fixed step (`1 / TICK_HZ`).
pub const TICK_DT: f32 = 1.0 / TICK_HZ as f32;

/// Separation: ships stay at least `(rA + rB) * SEPARATION_SPACING` apart; the
/// overlap is turned into a corrective velocity scaled by `SEPARATION_GAIN`.
const SEPARATION_SPACING: f32 = 1.15;
const SEPARATION_GAIN: f32 = 3.0;

/// Salvage harvested per second while a resourcer sits in a node's gather range.
const HARVEST_RATE: f32 = 60.0;

/// Crew complement a carrier (the Ark) contributes to the player's pools. Other
/// ships draw from these; the carrier itself requires no separate crew.
const CARRIER_CREW_OPS: u32 = 2000;
const CARRIER_CREW_PILOTS: u32 = 100;

/// World-space position and orientation of an entity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub pos: Vec3,
    pub rot: Quat,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            pos: Vec3::ZERO,
            rot: Quat::IDENTITY,
        }
    }
}

/// Linear velocity in world units per second.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Velocity {
    pub linear: Vec3,
}

/// Which side an entity belongs to. Phase 1 only spawns `Player` + `Neutral`,
/// but the enum is here so net-synced state carries team from day one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Team {
    Player,
    Enemy,
    Neutral,
}

/// Combat stance for a ship (player-facing AI toggle).
/// - `Passive`: never auto-engage; only fire on a direct `Attack` command.
/// - `Defensive`: auto-engage hostiles in weapon range, respond to nearby allies
///   under attack (SOS), and return to an anchor position when combat ends.
/// - `Aggressive`: auto-acquire hostiles out to sensor range and chase them down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stance {
    Passive,
    Defensive,
    Aggressive,
}

impl Stance {
    /// Next stance in the Passive -> Defensive -> Aggressive cycle.
    pub fn next(self) -> Self {
        match self {
            Stance::Passive => Stance::Defensive,
            Stance::Defensive => Stance::Aggressive,
            Stance::Aggressive => Stance::Passive,
        }
    }

    /// Short, stable label for the HUD.
    pub fn label(self) -> &'static str {
        match self {
            Stance::Passive => "Passive",
            Stance::Defensive => "Defensive",
            Stance::Aggressive => "Aggressive",
        }
    }
}

/// Ship taxonomy from `design.md` §4. The class identifies which static
/// blueprint (stats + mesh) an entity uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShipClass {
    Fighter,
    Bomber,
    Corvette,
    Resourcer,
    FrigateGeneral,
    FrigateMissile,
    CapitalDestroyer,
    Carrier,
}

impl ShipClass {
    /// All classes in a fixed order (stable iteration for registry building).
    pub const ALL: [ShipClass; 8] = [
        ShipClass::Fighter,
        ShipClass::Bomber,
        ShipClass::Corvette,
        ShipClass::Resourcer,
        ShipClass::FrigateGeneral,
        ShipClass::FrigateMissile,
        ShipClass::CapitalDestroyer,
        ShipClass::Carrier,
    ];

    /// Stable string id (matches the authored ship ids / `ship.json`).
    pub fn id(self) -> &'static str {
        match self {
            ShipClass::Fighter => "fighter",
            ShipClass::Bomber => "bomber",
            ShipClass::Corvette => "corvette",
            ShipClass::Resourcer => "resourcer",
            ShipClass::FrigateGeneral => "frigate_general",
            ShipClass::FrigateMissile => "frigate_missile",
            ShipClass::CapitalDestroyer => "capital_destroyer",
            ShipClass::Carrier => "carrier",
        }
    }

    /// Crew category: strike craft are flown by pilots, everything else is run
    /// by operations crew (see `design.md` §5).
    pub fn crew_kind(self) -> CrewKind {
        match self {
            ShipClass::Fighter | ShipClass::Bomber => CrewKind::Pilots,
            _ => CrewKind::Ops,
        }
    }
}

/// The two crew pools of the logistics roster (`design.md` §5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CrewKind {
    Ops,
    Pilots,
}

/// Weapon archetype (`design.md` §6.8). `Ballistic` and `Bomb` are unguided
/// projectiles fired on a lead/intercept solution (the bomb is slow, heavy, and
/// short ranged); `Missile` is a guided projectile that homes with a turn limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WeaponKind {
    Ballistic,
    Bomb,
    Missile,
}

/// A ship's weapon mount (one per armed class in this prototype; per-hardpoint
/// loadouts land later). All numbers are deterministic, demo-tuned.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Weapon {
    pub kind: WeaponKind,
    /// Max engagement range (world units): acquire + fire within this.
    pub range: f32,
    /// Hull damage per hit.
    pub damage: f32,
    /// Projectile speed (world units/s).
    pub speed: f32,
    /// Seconds between shots.
    pub cooldown: f32,
    /// Homing turn rate (deg/s) for `Missile`; ignored by unguided kinds.
    pub turn_rate_deg: f32,
    /// Projectile hit radius (added to the target radius for the swept test).
    pub proj_radius: f32,
}

/// Per-class weapon loadout. Returns `None` for unarmed classes (resourcer and
/// the carrier, which the fleet protects). Demo-tuned; eventual values live in
/// `ship.json`.
fn weapon_for(class: ShipClass) -> Option<Weapon> {
    let w = |kind, range, damage, speed, cooldown, turn_rate_deg, proj_radius| {
        Some(Weapon {
            kind,
            range,
            damage,
            speed,
            cooldown,
            turn_rate_deg,
            proj_radius,
        })
    };
    match class {
        ShipClass::Fighter => w(WeaponKind::Ballistic, 22.0, 6.0, 90.0, 0.18, 0.0, 0.4),
        ShipClass::Bomber => w(WeaponKind::Bomb, 16.0, 60.0, 45.0, 2.2, 0.0, 1.0),
        ShipClass::Corvette => w(WeaponKind::Ballistic, 26.0, 10.0, 85.0, 0.5, 0.0, 0.5),
        ShipClass::Resourcer => None,
        ShipClass::FrigateGeneral => w(WeaponKind::Ballistic, 38.0, 18.0, 80.0, 0.9, 0.0, 0.6),
        ShipClass::FrigateMissile => w(WeaponKind::Missile, 55.0, 45.0, 38.0, 2.6, 70.0, 0.8),
        ShipClass::CapitalDestroyer => w(WeaponKind::Ballistic, 50.0, 40.0, 70.0, 1.4, 0.0, 0.9),
        ShipClass::Carrier => None,
    }
}

/// Static, per-class stats loaded once into the [`ShipRegistry`]. These are the
/// gameplay numbers the schema carries in `ship.json` (mobility + build); the
/// steering sim (Stage 7) reads `max_speed`/`accel`/`turn_rate_deg`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Blueprint {
    pub class: ShipClass,
    pub mass: f32,
    pub max_speed: f32,
    pub accel: f32,
    pub turn_rate_deg: f32,
    /// Operating crew the ship needs (allocated from the roster, not spent).
    pub crew: u32,
    /// Gameplay radius (world units): separation spacing and pick sphere.
    pub radius: f32,
    /// Hull points at full health. Combat (later phase) drains the entity's
    /// `hull`; the HUD reads `hull / max_hull` for the health bar.
    pub max_hull: f32,
    /// Salvage cargo capacity. Only resourcers carry (> 0); a ship with zero
    /// capacity cannot accept a gather order.
    pub cargo: f32,
    /// Salvage cost to build this class at the mothership.
    pub cost: f32,
    /// Build time in seconds (demo-tuned; shorter than the eventual values).
    pub build_time: f32,
    /// Sensor range (world units): how far this ship detects other teams.
    /// Drives enemy AI target acquisition and the sensors-manager fog of war.
    pub sensor_range: f32,
    /// Weapon mount, or `None` for unarmed classes (resourcer, carrier).
    pub weapon: Option<Weapon>,
    /// Shield capacity (0 for strike craft). Damage hits the shield before the
    /// hull, modulated by facing (`design.md` §6.8); the HUD shows it in blue.
    pub max_shield: f32,
    /// Shield recharge per second, applied after [`SHIELD_REGEN_DELAY`] with no
    /// hits.
    pub shield_regen: f32,
}

/// Seconds a ship must go un-hit before its shield starts to recharge.
pub const SHIELD_REGEN_DELAY: f32 = 4.0;

/// Per-class shield capacity and recharge rate (per second). Strike craft have
/// none; capitals and the carrier carry heavy shields. Demo-tuned.
fn shield_for(class: ShipClass) -> (f32, f32) {
    let max = match class {
        ShipClass::Fighter | ShipClass::Bomber => 0.0,
        ShipClass::Corvette => 40.0,
        ShipClass::Resourcer => 30.0,
        ShipClass::FrigateGeneral => 300.0,
        ShipClass::FrigateMissile => 280.0,
        ShipClass::CapitalDestroyer => 1500.0,
        ShipClass::Carrier => 4000.0,
    };
    (max, max * 0.06)
}

/// Read-only table of per-class blueprints. Lookups are by key (deterministic);
/// it is never iterated in a way that affects sim state.
#[derive(Clone, Debug)]
pub struct ShipRegistry {
    blueprints: HashMap<ShipClass, Blueprint>,
}

impl ShipRegistry {
    /// Build the registry with Phase 1 placeholder stats (see the plan's ship
    /// suite table). Tuned later; the point now is that the sim reads them.
    pub fn with_defaults() -> Self {
        // (class, mass t, max_speed, accel, turn deg/s, radius, max_hull, cargo).
        // Speeds are tuned for the current demo scene (tens of units across);
        // radius drives separation and picking; crew/cost/build come from the
        // per-class match below.
        type Row = (ShipClass, f32, f32, f32, f32, f32, f32, f32);
        let rows: [Row; 8] = [
            (ShipClass::Fighter, 18.0, 16.0, 24.0, 130.0, 1.0, 80.0, 0.0),
            (ShipClass::Bomber, 30.0, 12.0, 16.0, 90.0, 1.5, 120.0, 0.0),
            (
                ShipClass::Corvette,
                220.0,
                10.0,
                12.0,
                70.0,
                2.25,
                400.0,
                0.0,
            ),
            (
                ShipClass::Resourcer,
                140.0,
                8.0,
                9.0,
                55.0,
                2.5,
                300.0,
                200.0,
            ),
            (
                ShipClass::FrigateGeneral,
                4000.0,
                6.0,
                5.0,
                30.0,
                4.25,
                2000.0,
                0.0,
            ),
            (
                ShipClass::FrigateMissile,
                4200.0,
                5.5,
                4.5,
                28.0,
                4.5,
                1900.0,
                0.0,
            ),
            (
                ShipClass::CapitalDestroyer,
                16000.0,
                4.0,
                3.0,
                14.0,
                6.0,
                8000.0,
                0.0,
            ),
            (
                ShipClass::Carrier,
                40000.0,
                3.0,
                2.0,
                8.0,
                8.0,
                20000.0,
                0.0,
            ),
        ];
        let mut blueprints = HashMap::new();
        for (class, mass, max_speed, accel, turn_rate_deg, radius, max_hull, cargo) in rows {
            let (max_shield, shield_regen) = shield_for(class);
            // Crew requirement (of the class's CrewKind), salvage cost, and build
            // seconds. Carriers require no crew: they PROVIDE the pool (see
            // CARRIER_CREW_*). Demo-tuned; eventual values live in ship.json.
            let (crew, cost, build_time, sensor_range): (u32, f32, f32, f32) = match class {
                ShipClass::Fighter => (1, 30.0, 4.0, 30.0),
                ShipClass::Bomber => (2, 50.0, 6.0, 28.0),
                ShipClass::Corvette => (10, 120.0, 8.0, 40.0),
                ShipClass::Resourcer => (3, 80.0, 7.0, 35.0),
                ShipClass::FrigateGeneral => (100, 300.0, 16.0, 55.0),
                ShipClass::FrigateMissile => (100, 340.0, 18.0, 60.0),
                ShipClass::CapitalDestroyer => (400, 900.0, 30.0, 70.0),
                ShipClass::Carrier => (0, 5000.0, 60.0, 90.0),
            };
            blueprints.insert(
                class,
                Blueprint {
                    class,
                    mass,
                    max_speed,
                    accel,
                    turn_rate_deg,
                    crew,
                    radius,
                    max_hull,
                    cargo,
                    cost,
                    build_time,
                    sensor_range,
                    weapon: weapon_for(class),
                    max_shield,
                    shield_regen,
                },
            );
        }
        Self { blueprints }
    }

    pub fn get(&self, class: ShipClass) -> &Blueprint {
        // Every class is inserted in `with_defaults`, so this never panics.
        &self.blueprints[&class]
    }
}

impl Default for ShipRegistry {
    fn default() -> Self {
        Self::with_defaults()
    }
}

/// Stable per-entity identity. Incrementing for Phase 1; a generational
/// free-list lands with combat/despawn in M1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EntityId(pub u32);

/// A simulated ship: its class, team, and mass.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ship {
    pub class: ShipClass,
    pub team: Team,
    pub mass: f32,
}

/// A circular patrol path in the XZ plane. Each step advances `angle` by
/// `angular_speed * dt` and snaps the entity onto the circle, facing its
/// direction of travel. Deterministic (a pure function of accumulated dt), so
/// it is safe to run inside the sim.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Patrol {
    pub center: Vec3,
    pub radius: f32,
    pub height: f32,
    pub angular_speed: f32,
    pub angle: f32,
}

impl Patrol {
    /// Current point on the circle.
    pub fn point(&self) -> Vec3 {
        self.center
            + Vec3::new(
                self.radius * self.angle.cos(),
                self.height,
                self.radius * self.angle.sin(),
            )
    }

    /// Unit tangent (direction of travel) at the current angle.
    pub fn heading(&self) -> Vec3 {
        Vec3::new(-self.angle.sin(), 0.0, self.angle.cos())
    }
}

/// Stable id for a resource node (asteroid / salvage site).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u32);

/// A harvestable salvage site. Positions/amounts are seeded (deterministic for
/// gameplay); resourcers drain `amount` down to zero. Purely sim state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResourceNode {
    pub id: NodeId,
    pub pos: Vec3,
    pub amount: f32,
    pub max_amount: f32,
    /// Footprint radius (separation/picking and the gather standoff distance).
    pub radius: f32,
}

/// A resourcer's active gather task: harvest `node` until full, return to the
/// mothership to deposit, repeat until the node is empty.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gather {
    pub node: NodeId,
    pub carrying: f32,
    /// True while heading back to deposit; false while harvesting.
    pub returning: bool,
}

/// A ship under construction at the mothership: which class, and how much build
/// time has accrued. The queue is processed front-first in [`World::step`];
/// salvage is paid up front when the build is enqueued.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BuildItem {
    pub class: ShipClass,
    pub progress: f32,
}

/// One simulated entity. `prev_transform` holds the previous step's transform
/// so the renderer can interpolate; the sim itself never reads it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Entity {
    pub id: EntityId,
    pub transform: Transform,
    pub prev_transform: Transform,
    pub velocity: Velocity,
    pub ship: Ship,
    /// Current hull points; starts at the blueprint `max_hull`. A ship despawns
    /// when this reaches zero; the HUD reads `hull / max_hull`.
    pub hull: f32,
    /// Current shield points; absorbs damage before the hull (facing-modulated)
    /// and recharges after [`SHIELD_REGEN_DELAY`]. Starts at `max_shield`.
    pub shield: f32,
    /// Seconds left before the shield may recharge again (reset on every hit).
    pub shield_regen_cooldown: f32,
    /// Active move order target, if any (steered toward with arrive + turn).
    pub order: Option<Vec3>,
    /// Optional circular patrol; when set, the step drives the transform.
    pub patrol: Option<Patrol>,
    /// Active gather task (resourcers only); overrides move orders/patrol.
    pub gather: Option<Gather>,
    /// Current combat target (auto-acquired nearest hostile in weapon range).
    pub target: Option<EntityId>,
    /// Player/AI commanded attack target: pursue to weapon range and focus fire.
    /// Overrides auto-acquisition; cleared by a move/stop order or target death.
    pub attack_target: Option<EntityId>,
    /// Seconds until this ship's weapon can fire again.
    pub weapon_cooldown: f32,
    /// Combat stance (player ships only in this build; enemies use the inbound-
    /// AI block). Drives auto-engagement, SOS response, and anchor return.
    pub stance: Stance,
    /// "Home" position the ship returns to after a Defensive engagement ends.
    /// Set on spawn, on every `MoveTo`, and when stance is set to Defensive.
    pub anchor: Option<Vec3>,
}

/// Stable id for an in-flight projectile (incrementing; transient lifetime).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProjectileId(pub u32);

/// A projectile in flight: a `sol-sim` entity with position + velocity,
/// integrated at the fixed timestep (`design.md` §6.8). Unguided kinds fly
/// straight; `Missile` homes toward `target` with a turn limit. `prev_pos` lets
/// the renderer draw an interpolated tracer; the sim hit-tests the swept segment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Projectile {
    pub id: ProjectileId,
    pub kind: WeaponKind,
    /// Owning team; a projectile only damages other teams.
    pub team: Team,
    /// The ship that fired this round (so a victim can retaliate against it).
    pub owner: EntityId,
    pub pos: Vec3,
    pub prev_pos: Vec3,
    pub vel: Vec3,
    /// Homing target for `Missile` (ignored by unguided kinds).
    pub target: Option<EntityId>,
    pub damage: f32,
    pub speed: f32,
    pub turn_rate_deg: f32,
    pub radius: f32,
    /// Remaining lifetime (s); the projectile despawns at zero.
    pub life: f32,
}

/// A transient combat effect produced during a step, for the renderer to turn
/// into particles/flashes. Output only: deterministic (every peer computes the
/// same stream), never read back into the sim, and not part of the checksum.
/// Drained each frame via [`World::drain_combat_events`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CombatEvent {
    /// A weapon fired: muzzle position, aim direction, and the weapon kind.
    Fired {
        pos: Vec3,
        dir: Vec3,
        kind: WeaponKind,
    },
    /// A projectile struck a ship at `pos`; `shielded` is true when the shield
    /// absorbed any of the hit (a blue ripple) vs. a bare hull hit (orange).
    Hit { pos: Vec3, shielded: bool },
}

/// A player/AI order. Orders are queued and applied at fixed-step boundaries
/// (the lockstep seam), never as direct mutations from input. Steering for
/// `MoveTo` lands in Stage 7; for now movement is expressed as a target
/// velocity so the command path and determinism are exercised end to end.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Command {
    /// Steer an entity toward a world point (arrive behavior plus separation).
    MoveTo { entity: EntityId, target: Vec3 },
    /// Set an entity's linear velocity directly (used by tests).
    SetVelocity { entity: EntityId, linear: Vec3 },
    /// Halt an entity and clear any move/gather order.
    Stop { entity: EntityId },
    /// Order a ship to engage a specific target: pursue it to weapon range and
    /// focus fire. Ignored for unarmed classes. Overrides auto-acquisition.
    Attack { entity: EntityId, target: EntityId },
    /// Set a ship's combat stance (player toggle: Passive/Defensive/Aggressive).
    /// Switching to Defensive snapshots the current position as the new anchor.
    SetStance { entity: EntityId, stance: Stance },
    /// Send a resourcer to harvest a node (ignored for ships with no cargo).
    Gather { entity: EntityId, node: NodeId },
    /// Queue a ship of `class` for construction at the mothership; salvage is
    /// debited immediately (ignored if unaffordable).
    Build { class: ShipClass },
}

/// The deterministic simulation world.
///
/// Entities live in a `Vec` in stable spawn order (iteration order is
/// deterministic, required for lockstep). The seeded [`Rng`] owns all gameplay
/// randomness; the [`ShipRegistry`] is read-only static data.
#[derive(Clone, Debug)]
pub struct World {
    pub entities: Vec<Entity>,
    /// In-flight projectiles (transient combat entities; `design.md` §6.8).
    pub projectiles: Vec<Projectile>,
    /// Transient combat effects from the last step, drained by the renderer.
    /// Output only; cleared at the start of every [`World::step`].
    pub combat_events: Vec<CombatEvent>,
    /// Harvestable salvage sites (asteroids); drained by resourcers.
    pub resource_nodes: Vec<ResourceNode>,
    /// Salvage banked at the mothership (the player's resource pool).
    pub salvage: f32,
    /// Enemy team's salvage pool, fed by enemy harvesters depositing at the
    /// enemy mothership. Spent by the commander AI to queue new enemy builds.
    pub enemy_salvage: f32,
    /// Ships under construction at the mothership (front item builds first).
    pub build_queue: Vec<BuildItem>,
    /// Enemy mothership's build queue, populated by the commander AI.
    pub enemy_build_queue: Vec<BuildItem>,
    /// Counter the commander rotates through combat classes with on each queue.
    pub enemy_build_index: u32,
    pub registry: ShipRegistry,
    pub rng: Rng,
    /// Number of fixed steps taken; part of the determinism checksum.
    pub tick: u64,
    next_id: u32,
    next_node_id: u32,
    next_projectile_id: u32,
    /// Orders accumulated since the last step, drained at the next step.
    pending: Vec<Command>,
}

impl World {
    /// Create an empty world seeded deterministically.
    pub fn new(seed: u64) -> Self {
        Self {
            entities: Vec::new(),
            projectiles: Vec::new(),
            combat_events: Vec::new(),
            resource_nodes: Vec::new(),
            salvage: 0.0,
            enemy_salvage: 0.0,
            build_queue: Vec::new(),
            enemy_build_queue: Vec::new(),
            enemy_build_index: 0,
            registry: ShipRegistry::with_defaults(),
            rng: Rng::new(seed),
            tick: 0,
            next_id: 0,
            next_node_id: 0,
            next_projectile_id: 0,
            pending: Vec::new(),
        }
    }

    /// Spawn a harvestable resource node, returning its stable id.
    pub fn spawn_resource_node(&mut self, pos: Vec3, amount: f32, radius: f32) -> NodeId {
        let id = NodeId(self.next_node_id);
        self.next_node_id += 1;
        self.resource_nodes.push(ResourceNode {
            id,
            pos,
            amount,
            max_amount: amount,
            radius,
        });
        id
    }

    /// Look up a resource node by id.
    pub fn node(&self, id: NodeId) -> Option<&ResourceNode> {
        self.resource_nodes.iter().find(|n| n.id == id)
    }

    /// Total crew pools `(ops, pilots)` provided by all carriers (the Arks).
    pub fn crew_capacity(&self) -> (u32, u32) {
        let carriers = self
            .entities
            .iter()
            .filter(|e| e.ship.class == ShipClass::Carrier)
            .count() as u32;
        (carriers * CARRIER_CREW_OPS, carriers * CARRIER_CREW_PILOTS)
    }

    /// Crew `(ops, pilots)` committed to existing ships AND queued builds, so a
    /// queued build reserves its crew immediately (prevents over-committing).
    /// Carriers consume none (they only provide).
    pub fn crew_used(&self) -> (u32, u32) {
        let (mut ops, mut pilots) = (0u32, 0u32);
        let classes = self
            .entities
            .iter()
            .map(|e| e.ship.class)
            .chain(self.build_queue.iter().map(|b| b.class));
        for class in classes {
            if class == ShipClass::Carrier {
                continue;
            }
            let n = self.registry.get(class).crew;
            match class.crew_kind() {
                CrewKind::Ops => ops = ops.saturating_add(n),
                CrewKind::Pilots => pilots = pilots.saturating_add(n),
            }
        }
        (ops, pilots)
    }

    /// Free crew `(ops, pilots)` = capacity minus used (signed; can go negative
    /// if a carrier is lost while its ships still fly).
    pub fn crew_free(&self) -> (i32, i32) {
        let (co, cp) = self.crew_capacity();
        let (uo, up) = self.crew_used();
        (co as i32 - uo as i32, cp as i32 - up as i32)
    }

    /// Whether a build of `class` can be afforded right now (salvage + crew).
    pub fn can_build(&self, class: ShipClass) -> bool {
        let bp = self.registry.get(class);
        if bp.cost <= 0.0 || self.salvage < bp.cost {
            return false;
        }
        let (free_ops, free_pilots) = self.crew_free();
        match class.crew_kind() {
            CrewKind::Ops => free_ops >= bp.crew as i32,
            CrewKind::Pilots => free_pilots >= bp.crew as i32,
        }
    }

    /// Spawn a ship at `transform` and return its stable id.
    pub fn spawn(&mut self, ship: Ship, transform: Transform) -> EntityId {
        let id = EntityId(self.next_id);
        self.next_id += 1;
        let bp = self.registry.get(ship.class);
        let (max_hull, max_shield) = (bp.max_hull, bp.max_shield);
        self.entities.push(Entity {
            id,
            transform,
            prev_transform: transform,
            velocity: Velocity::default(),
            ship,
            hull: max_hull,
            shield: max_shield,
            shield_regen_cooldown: 0.0,
            order: None,
            patrol: None,
            gather: None,
            target: None,
            attack_target: None,
            weapon_cooldown: 0.0,
            // Motherships and harvesters default to Passive so they never go off
            // to hunt; the commander AI / SOS / explicit orders move them.
            stance: match ship.class {
                ShipClass::Carrier | ShipClass::Resourcer => Stance::Passive,
                _ => Stance::Defensive,
            },
            anchor: Some(transform.pos),
        });
        id
    }

    /// Spawn a ship of `class`/`team` at a position, taking mass from the
    /// registry blueprint.
    pub fn spawn_class(&mut self, class: ShipClass, team: Team, pos: Vec3) -> EntityId {
        let mass = self.registry.get(class).mass;
        self.spawn(
            Ship { class, team, mass },
            Transform {
                pos,
                rot: Quat::IDENTITY,
            },
        )
    }

    /// Spawn a ship that follows a circular `patrol` around a point, starting on
    /// the circle and facing its direction of travel.
    pub fn spawn_patrol(&mut self, class: ShipClass, team: Team, patrol: Patrol) -> EntityId {
        let id = self.spawn_class(class, team, patrol.point());
        if let Some(e) = self.entities.last_mut() {
            e.patrol = Some(patrol);
            e.transform.rot = Quat::from_rotation_arc(Vec3::Z, patrol.heading());
            e.prev_transform = e.transform;
        }
        id
    }

    /// Queue an order to be applied at the next [`World::step`].
    pub fn enqueue(&mut self, command: Command) {
        self.pending.push(command);
    }

    /// Take this step's combat effects (muzzle flashes, impacts), clearing the
    /// buffer. The renderer calls this after each step to spawn VFX.
    pub fn drain_combat_events(&mut self) -> Vec<CombatEvent> {
        std::mem::take(&mut self.combat_events)
    }

    /// Enemy commander AI: a per-team strategic layer that runs the enemy
    /// economy. Stateless across steps (decisions derive from current world
    /// state); deterministic and side-effect free outside the sim.
    ///   - Dispatches idle enemy harvesters to the nearest live resource node.
    ///   - When the enemy build queue is empty and enough salvage has accrued,
    ///     queues a new combat ship, rotating between Fighter/Corvette/Bomber.
    ///
    /// Combat-ship movement is still driven by the per-ship enemy AI block
    /// (advance on the player mothership / standoff in sensor range).
    fn enemy_commander_step(&mut self) {
        let live_nodes: Vec<(NodeId, Vec3)> = self
            .resource_nodes
            .iter()
            .filter(|n| n.amount > 0.0)
            .map(|n| (n.id, n.pos))
            .collect();
        for e in &mut self.entities {
            if e.ship.team != Team::Enemy || e.ship.class != ShipClass::Resourcer {
                continue;
            }
            if e.gather.is_some() {
                continue;
            }
            let my_pos = e.transform.pos;
            let mut best_d2 = f32::INFINITY;
            let mut best: Option<NodeId> = None;
            for &(nid, npos) in &live_nodes {
                let d2 = (npos - my_pos).length_squared();
                if d2 < best_d2 {
                    best_d2 = d2;
                    best = Some(nid);
                }
            }
            if let Some(node) = best {
                e.gather = Some(Gather {
                    node,
                    carrying: 0.0,
                    returning: false,
                });
            }
        }
        // Build funding: rotate Fighter -> Corvette -> Bomber, queue the next
        // affordable class when the line is idle.
        if self.enemy_build_queue.is_empty() {
            const CLASSES: [ShipClass; 3] =
                [ShipClass::Fighter, ShipClass::Corvette, ShipClass::Bomber];
            let class = CLASSES[(self.enemy_build_index as usize) % CLASSES.len()];
            let cost = self.registry.get(class).cost;
            if self.enemy_salvage >= cost {
                self.enemy_salvage -= cost;
                self.enemy_build_queue.push(BuildItem {
                    class,
                    progress: 0.0,
                });
                self.enemy_build_index = self.enemy_build_index.wrapping_add(1);
            }
        }
    }

    /// Look up an entity by id (linear scan; entity counts are small in
    /// Phase 1).
    pub fn entity(&self, id: EntityId) -> Option<&Entity> {
        self.entities.iter().find(|e| e.id == id)
    }

    fn entity_mut(&mut self, id: EntityId) -> Option<&mut Entity> {
        self.entities.iter_mut().find(|e| e.id == id)
    }

    /// Drain and apply all queued commands. Orders take effect here. Exposed so
    /// single-player pause can register commands immediately (build debits and
    /// queues, move/stop orders set) WITHOUT advancing the world; [`World::step`]
    /// calls it each tick.
    pub fn apply_commands(&mut self) {
        let pending = std::mem::take(&mut self.pending);
        for command in pending {
            match command {
                Command::MoveTo { entity, target } => {
                    if let Some(e) = self.entity_mut(entity) {
                        e.order = Some(target);
                        e.patrol = None; // a direct order overrides a patrol
                        e.gather = None; // and cancels any gather task
                        e.attack_target = None; // and breaks off an attack
                        e.anchor = Some(target); // the new home for Defensive return
                    }
                }
                Command::SetStance { entity, stance } => {
                    if let Some(e) = self.entity_mut(entity) {
                        e.stance = stance;
                        // Defensive returns to where it stood when the order was given.
                        if stance == Stance::Defensive {
                            e.anchor = Some(e.transform.pos);
                        }
                    }
                }
                Command::SetVelocity { entity, linear } => {
                    if let Some(e) = self.entity_mut(entity) {
                        e.velocity.linear = linear;
                    }
                }
                Command::Stop { entity } => {
                    if let Some(e) = self.entity_mut(entity) {
                        e.order = None;
                        e.gather = None;
                        e.attack_target = None;
                        e.velocity.linear = Vec3::ZERO;
                    }
                }
                Command::Attack { entity, target } => {
                    // Only armed ships can be ordered to attack, and never to
                    // attack themselves.
                    let armed = entity != target
                        && self
                            .entity(entity)
                            .is_some_and(|e| self.registry.get(e.ship.class).weapon.is_some());
                    if armed {
                        if let Some(e) = self.entity_mut(entity) {
                            e.attack_target = Some(target);
                            e.gather = None;
                            e.patrol = None;
                        }
                    }
                }
                Command::Gather { entity, node } => {
                    // Only ships with cargo capacity (resourcers) can gather.
                    let can = self
                        .entity(entity)
                        .is_some_and(|e| self.registry.get(e.ship.class).cargo > 0.0);
                    if can {
                        if let Some(e) = self.entity_mut(entity) {
                            e.gather = Some(Gather {
                                node,
                                carrying: 0.0,
                                returning: false,
                            });
                            e.order = None;
                            e.patrol = None;
                        }
                    }
                }
                Command::Build { class } => {
                    // Affordable means salvage AND free crew (queued builds
                    // already count toward used crew, so this can't over-commit).
                    if self.can_build(class) {
                        self.salvage -= self.registry.get(class).cost;
                        self.build_queue.push(BuildItem {
                            class,
                            progress: 0.0,
                        });
                    }
                }
            }
        }
    }

    /// Advance the simulation by one fixed timestep `dt` (seconds): apply queued
    /// commands, snapshot transforms for interpolation, then integrate. Iteration
    /// is in stable order; no wall-clock or OS RNG.
    pub fn step(&mut self, dt: f32) {
        // 1) Apply queued orders at this step boundary, in arrival order.
        self.apply_commands();
        // This step's transient combat effects accumulate here; clear last step's.
        self.combat_events.clear();

        // 2) Snapshot positions+radii BEFORE moving anyone, so separation is
        // order independent (ship A's move can't change ship B's input this step).
        let snapshot: Vec<(EntityId, Vec3, f32)> = self
            .entities
            .iter()
            .map(|e| {
                (
                    e.id,
                    e.transform.pos,
                    self.registry.get(e.ship.class).radius,
                )
            })
            .collect();

        // Deposit point for each team: the first carrier in spawn order on that
        // team, used as both the harvester drop-off and the enemy AI rally point.
        let depot: Option<(Vec3, f32)> = self
            .entities
            .iter()
            .find(|e| e.ship.class == ShipClass::Carrier && e.ship.team == Team::Player)
            .map(|e| (e.transform.pos, self.registry.get(e.ship.class).radius));
        let enemy_depot: Option<(Vec3, f32)> = self
            .entities
            .iter()
            .find(|e| e.ship.class == ShipClass::Carrier && e.ship.team == Team::Enemy)
            .map(|e| (e.transform.pos, self.registry.get(e.ship.class).radius));

        // 2b) Enemy AI (movement only): each enemy steers toward the nearest
        // player ship within its sensor range; lacking direct contact it advances
        // on the mothership (`depot`) as its objective, and idles only if there is
        // no mothership. Deterministic: reads this step's start positions, writes
        // only orders, and iterates in stable id order, so it is order independent.
        let player_positions: Vec<Vec3> = self
            .entities
            .iter()
            .filter(|e| e.ship.team == Team::Player)
            .map(|e| e.transform.pos)
            .collect();
        if !player_positions.is_empty() {
            let rally = depot.map(|(p, _)| p);
            for e in &mut self.entities {
                if e.ship.team != Team::Enemy {
                    continue;
                }
                let bp = self.registry.get(e.ship.class);
                let max_d2 = bp.sensor_range * bp.sensor_range;
                let my = e.transform.pos;
                let mut best_d2 = f32::INFINITY;
                let mut best: Option<Vec3> = None;
                for &p in &player_positions {
                    let d2 = (p - my).length_squared();
                    if d2 <= max_d2 && d2 < best_d2 {
                        best_d2 = d2;
                        best = Some(p);
                    }
                }
                // Armed enemies close to a weapon-range standoff and hold there to
                // fire; unarmed ones (none in the demo) advance onto the contact.
                let goal = best.map(|p| match bp.weapon {
                    Some(w) => {
                        let to = my - p;
                        let d = to.length();
                        if d > 1e-3 {
                            p + to / d * (w.range * 0.7)
                        } else {
                            p
                        }
                    }
                    None => p,
                });
                e.order = goal.or(rally);
            }
        }

        // 2b.1) Enemy commander AI: schedule harvesters and fund builds.
        self.enemy_commander_step();

        // 2c) Stance behavior (player team):
        //   - SOS: an attack on a friendly broadcasts; nearby non-passive armed
        //     allies adopt the attacker as their `attack_target` (one-step delay
        //     since this reads last step's combat targets).
        //   - Pursuit: commanded attacks (any stance) and Aggressive auto-targets
        //     steer to a weapon-range standoff of their target.
        //   - Return-to-anchor: Defensive ships with nothing to engage steer
        //     toward `anchor` so they drift back after a fight ends.
        const SOS_RADIUS: f32 = 35.0;
        let positions: Vec<(EntityId, Team, Vec3)> = self
            .entities
            .iter()
            .map(|v| (v.id, v.ship.team, v.transform.pos))
            .collect();
        // Pairs of (attacker, victim, attacker_team) drawn from last step's targets.
        let attackers: Vec<(EntityId, EntityId, Team)> = self
            .entities
            .iter()
            .filter_map(|a| a.target.map(|tid| (a.id, tid, a.ship.team)))
            .collect();
        for (atk_id, vic_id, atk_team) in &attackers {
            let Some(&(_, vteam, vpos)) = positions.iter().find(|v| v.0 == *vic_id) else {
                continue;
            };
            if vteam == *atk_team || vteam != Team::Player {
                continue;
            }
            for e in &mut self.entities {
                if e.ship.team != vteam
                    || e.id == *vic_id
                    || e.stance == Stance::Passive
                    || e.attack_target.is_some()
                    || e.gather.is_some()
                    || self.registry.get(e.ship.class).weapon.is_none()
                {
                    continue;
                }
                if (e.transform.pos - vpos).length_squared() <= SOS_RADIUS * SOS_RADIUS {
                    e.attack_target = Some(*atk_id);
                }
            }
        }
        for e in &mut self.entities {
            if e.ship.team != Team::Player {
                continue;
            }
            let weapon_range = self
                .registry
                .get(e.ship.class)
                .weapon
                .map(|w| w.range)
                .unwrap_or(0.0);
            // Pursue a commanded attack target, or (Aggressive only) the current
            // auto-acquired target.
            let pursue_id = if e.attack_target.is_some() {
                e.attack_target
            } else if e.stance == Stance::Aggressive {
                e.target
            } else {
                None
            };
            if let Some(aid) = pursue_id {
                match positions.iter().find(|p| p.0 == aid) {
                    Some(&(_, _, tpos)) if weapon_range > 0.0 => {
                        let to = e.transform.pos - tpos;
                        let d = to.length();
                        e.order = Some(if d > 1e-3 {
                            tpos + to / d * (weapon_range * 0.7)
                        } else {
                            tpos
                        });
                    }
                    Some(_) => {} // unarmed somehow: nothing to do
                    None => {
                        e.attack_target = None;
                        e.order = None;
                    }
                }
                continue;
            }
            // Defensive: nothing to engage, so drift back to the anchor.
            if e.stance == Stance::Defensive && e.gather.is_none() && e.target.is_none() {
                if let Some(a) = e.anchor {
                    e.order = Some(a);
                }
            }
        }

        // 3) Advance each entity. Patrols follow their circle; otherwise steer
        // toward a goal (a gather target approached to just outside its surface,
        // else a manual move order) with arrive + separation, clamped to limits.
        for e in &mut self.entities {
            e.prev_transform = e.transform;

            if let Some(p) = e.patrol.as_mut() {
                p.angle += p.angular_speed * dt;
                let pos = p.point();
                let heading = p.heading();
                e.transform.pos = pos;
                e.transform.rot = Quat::from_rotation_arc(Vec3::Z, heading);
                continue;
            }

            let bp = self.registry.get(e.ship.class);
            let my_pos = e.transform.pos;

            // This step's steering goal, and whether arriving clears a move order.
            let mut goal: Option<Vec3> = None;
            let mut clear_order = false;
            if let Some(g) = e.gather {
                let target = if g.returning {
                    // Return to the harvester's own team mothership.
                    match e.ship.team {
                        Team::Enemy => enemy_depot,
                        _ => depot,
                    }
                } else {
                    self.resource_nodes
                        .iter()
                        .find(|n| n.id == g.node)
                        .map(|n| (n.pos, n.radius))
                };
                if let Some((center, tr)) = target {
                    let to = center - my_pos;
                    let d = to.length();
                    let standoff = tr + bp.radius + 0.6;
                    if d > standoff {
                        goal = Some(center - to / d.max(1e-4) * standoff);
                    }
                    // Else already in range: park (no goal); the harvest pass
                    // below performs the extraction / deposit.
                }
            } else if let Some(target) = e.order {
                goal = Some(target);
                clear_order = true;
            }

            // Desired velocity is zero unless we have a goal; an arrived or idle
            // ship eases to a stop (it never separates, so it holds station).
            let mut desired = Vec3::ZERO;
            if let Some(target) = goal {
                let to = target - my_pos;
                let dist = to.length();
                let arrive = bp.radius * 0.5 + 0.3;
                if dist <= arrive {
                    if clear_order {
                        e.order = None;
                    }
                } else {
                    // Arrive: ease down inside the stopping distance.
                    let slowing = (bp.max_speed * bp.max_speed) / (2.0 * bp.accel.max(0.01));
                    let speed = if dist < slowing {
                        bp.max_speed * (dist / slowing)
                    } else {
                        bp.max_speed
                    };
                    desired = to / dist * speed;

                    // Separation: only moving ships push out of overlap, so the
                    // stationary fleet never drifts.
                    let mut push = Vec3::ZERO;
                    for &(id, pos, r) in &snapshot {
                        if id == e.id {
                            continue;
                        }
                        let off = my_pos - pos;
                        let d = off.length();
                        let min_d = (bp.radius + r) * SEPARATION_SPACING;
                        if d > 1e-4 && d < min_d {
                            push += off / d * (min_d - d);
                        }
                    }
                    desired += push * SEPARATION_GAIN;
                }
            }

            if desired.length() > bp.max_speed {
                desired = desired.normalize() * bp.max_speed;
            }
            let dv = (desired - e.velocity.linear).clamp_length_max(bp.accel * dt);
            e.velocity.linear += dv;
            e.transform.pos += e.velocity.linear * dt;

            // Turn toward the heading, clamped to the blueprint turn rate.
            let v = e.velocity.linear;
            if v.length_squared() > 1e-4 {
                let target_rot = Quat::from_rotation_arc(Vec3::Z, v.normalize());
                e.transform.rot = rotate_toward(
                    e.transform.rot,
                    target_rot,
                    bp.turn_rate_deg.to_radians() * dt,
                );
            }
        }

        // 3c) Combat (design.md §6.8). Auto-engage: every armed ship locks the
        // nearest hostile in weapon range and fires on cooldown; projectiles are
        // entities integrated below. Deterministic: a positions snapshot taken
        // before firing, stable id-order iteration, seeded math, no wall-clock.
        let combatants: Vec<(EntityId, Team, Vec3, Vec3, f32, bool)> = self
            .entities
            .iter()
            .map(|e| {
                let bp = self.registry.get(e.ship.class);
                (
                    e.id,
                    e.ship.team,
                    e.transform.pos,
                    e.velocity.linear,
                    bp.radius,
                    bp.weapon.is_some(),
                )
            })
            .collect();

        // Firing pass: tick cooldowns, (re)acquire targets, emit new projectiles.
        let mut new_projectiles: Vec<Projectile> = Vec::new();
        let mut events: Vec<CombatEvent> = Vec::new();
        let mut next_pid = self.next_projectile_id;
        for e in &mut self.entities {
            let bp = self.registry.get(e.ship.class);
            let Some(weapon) = bp.weapon else {
                e.target = None;
                continue;
            };
            e.weapon_cooldown = (e.weapon_cooldown - dt).max(0.0);
            let team = e.ship.team;
            let my_id = e.id;
            let my_pos = e.transform.pos;
            let weapon_d2 = weapon.range * weapon.range;
            // Auto-acquisition range depends on stance: Passive skips it entirely,
            // Defensive only looks inside weapon range, Aggressive reaches out to
            // sensor range so it can pick targets to chase down. Firing itself
            // (below) is always gated by the actual weapon range.
            let acquire_d2 = match e.stance {
                Stance::Passive => 0.0,
                Stance::Defensive => weapon_d2,
                Stance::Aggressive => bp.sensor_range * bp.sensor_range,
            };
            // Target priority: a commanded attack target (at any range, so the
            // ship can pursue and lock) wins; else keep the current auto-target if
            // still in acquire range; else acquire the nearest hostile (stable id
            // order breaks ties). A vanished commanded target clears the attack.
            let mut target: Option<(EntityId, Vec3, Vec3)> = None;
            if let Some(aid) = e.attack_target {
                match combatants.iter().find(|c| c.0 == aid && c.1 != team) {
                    Some(c) => target = Some((c.0, c.2, c.3)),
                    None => e.attack_target = None,
                }
            }
            if target.is_none() {
                target = e.target.and_then(|id| {
                    combatants
                        .iter()
                        .find(|c| {
                            c.0 == id
                                && c.1 != team
                                && (c.2 - my_pos).length_squared() <= acquire_d2
                        })
                        .map(|c| (c.0, c.2, c.3))
                });
            }
            if target.is_none() && acquire_d2 > 0.0 {
                let mut best_d2 = f32::INFINITY;
                for &(id, t, pos, vel, _r, _armed) in &combatants {
                    if t == team || id == my_id {
                        continue;
                    }
                    let d2 = (pos - my_pos).length_squared();
                    if d2 <= acquire_d2 && d2 < best_d2 {
                        best_d2 = d2;
                        target = Some((id, pos, vel));
                    }
                }
            }
            e.target = target.map(|(id, _, _)| id);
            // Fire only when the target is within weapon range (a commanded target
            // may still be out of range while the ship closes).
            let in_range =
                target.is_some_and(|(_, tpos, _)| (tpos - my_pos).length_squared() <= weapon_d2);
            if let (true, true, Some((tid, tpos, tvel))) =
                (e.weapon_cooldown <= 0.0, in_range, target)
            {
                let dir = if weapon.kind == WeaponKind::Missile {
                    (tpos - my_pos).normalize_or_zero()
                } else {
                    intercept_dir(my_pos, tpos, tvel, weapon.speed)
                };
                if dir != Vec3::ZERO {
                    new_projectiles.push(Projectile {
                        id: ProjectileId(next_pid),
                        kind: weapon.kind,
                        team,
                        owner: my_id,
                        pos: my_pos,
                        prev_pos: my_pos,
                        vel: dir * weapon.speed,
                        target: (weapon.kind == WeaponKind::Missile).then_some(tid),
                        damage: weapon.damage,
                        speed: weapon.speed,
                        turn_rate_deg: weapon.turn_rate_deg,
                        radius: weapon.proj_radius,
                        life: weapon.range / weapon.speed * 1.4 + 0.3,
                    });
                    next_pid += 1;
                    e.weapon_cooldown = weapon.cooldown;
                    events.push(CombatEvent::Fired {
                        pos: my_pos + dir * bp.radius,
                        dir,
                        kind: weapon.kind,
                    });
                }
            }
        }
        self.next_projectile_id = next_pid;

        // Projectile pass: home (missiles), integrate, swept hit-test against
        // hostile ships, then apply damage. Newly fired shots are appended after,
        // so they wait one step before moving (no instant-hit at the muzzle).
        // Each hit records (target, attacker, damage, hit-from direction, impact
        // point) so we can apply the facing shield model, emit an impact effect,
        // and let the victim flee or retaliate against the attacker.
        let mut damage: Vec<(EntityId, EntityId, f32, Vec3, Vec3)> = Vec::new();
        let mut surviving: Vec<Projectile> = Vec::with_capacity(self.projectiles.len());
        for mut p in std::mem::take(&mut self.projectiles) {
            p.prev_pos = p.pos;
            if p.kind == WeaponKind::Missile {
                if let Some(c) = p
                    .target
                    .and_then(|tid| combatants.iter().find(|c| c.0 == tid))
                {
                    let desired = (c.2 - p.pos).normalize_or_zero() * p.speed;
                    if desired != Vec3::ZERO {
                        let max_turn = p.turn_rate_deg.to_radians() * dt;
                        p.vel = steer_velocity(p.vel, desired, max_turn, p.speed);
                    }
                }
            }
            let a = p.pos;
            p.pos += p.vel * dt;
            let b = p.pos;
            p.life -= dt;
            // Earliest hostile hit along the swept segment (CCD; no tunneling).
            let mut best_t = f32::INFINITY;
            let mut best_id: Option<EntityId> = None;
            for &(id, t, c, _vel, r, _armed) in &combatants {
                if t == p.team {
                    continue;
                }
                if let Some(hit_t) = segment_sphere_t(a, b, c, r + p.radius) {
                    if hit_t < best_t {
                        best_t = hit_t;
                        best_id = Some(id);
                    }
                }
            }
            if let Some(id) = best_id {
                let impact = a.lerp(b, best_t);
                damage.push((id, p.owner, p.damage, (-p.vel).normalize_or_zero(), impact));
            } else if p.life > 0.0 {
                surviving.push(p);
            }
        }
        self.projectiles = surviving;
        // Apply hits in order: the shield absorbs first (strongest on the front,
        // weakest on the rear), the rest hits the hull; any hit pauses regen and
        // emits an impact effect (blue if the shield absorbed any, else orange).
        // A non-combat (unarmed or Passive) victim flees toward the nearest armed
        // ally; an armed combat ship without a commanded target retaliates by
        // locking the attacker as its `attack_target`.
        for (target_id, attacker_id, dmg, hit_from, impact) in damage {
            // Precompute everything that needs the snapshot, since `entity_mut`
            // below takes an exclusive borrow on `self.entities`.
            let Some((_, vteam, vpos, _, _, varmed)) =
                combatants.iter().find(|c| c.0 == target_id).copied()
            else {
                continue;
            };
            let attacker = combatants.iter().find(|c| c.0 == attacker_id).copied();
            let flee_to: Option<Vec3> = if varmed {
                None
            } else {
                let mut best_d2 = f32::INFINITY;
                let mut best: Option<Vec3> = None;
                for &(cid, ct, cp, _, _, ca) in &combatants {
                    if cid == target_id || ct != vteam || !ca {
                        continue;
                    }
                    let d2 = (cp - vpos).length_squared();
                    if d2 < best_d2 {
                        best_d2 = d2;
                        best = Some(cp);
                    }
                }
                best
            };
            if let Some(e) = self.entity_mut(target_id) {
                let facing = hit_from.dot(e.transform.rot * Vec3::Z);
                let (shield, hull) = apply_hit(e.shield, e.hull, dmg, facing);
                let shielded = shield < e.shield;
                events.push(CombatEvent::Hit {
                    pos: impact,
                    shielded,
                });
                e.shield = shield;
                e.hull = hull;
                e.shield_regen_cooldown = SHIELD_REGEN_DELAY;
                if !varmed || e.stance == Stance::Passive {
                    if let Some(p) = flee_to {
                        e.order = Some(p);
                    }
                } else if e.attack_target.is_none() {
                    if let Some((aid, ateam, _, _, _, _)) = attacker {
                        if ateam != e.ship.team {
                            e.attack_target = Some(aid);
                        }
                    }
                }
            }
        }
        // Despawn destroyed ships; `retain` keeps the survivors in spawn order.
        self.entities.retain(|e| e.hull > 0.0);
        self.projectiles.append(&mut new_projectiles);
        self.combat_events.append(&mut events);

        // Shield recharge: after the post-hit delay elapses, shields regenerate
        // toward `max_shield`. Stable order; runs for every shielded survivor.
        for e in &mut self.entities {
            let bp = self.registry.get(e.ship.class);
            if bp.max_shield <= 0.0 {
                continue;
            }
            if e.shield_regen_cooldown > 0.0 {
                e.shield_regen_cooldown = (e.shield_regen_cooldown - dt).max(0.0);
            } else if e.shield < bp.max_shield {
                e.shield = (e.shield + bp.shield_regen * dt).min(bp.max_shield);
            }
        }

        // 4) Resource harvest / deposit, in a separate pass so the node list and
        // the salvage pool aren't aliased with the entity iteration. Stable
        // (entity index) order keeps it deterministic.
        for i in 0..self.entities.len() {
            let Some(g) = self.entities[i].gather else {
                continue;
            };
            let pos = self.entities[i].transform.pos;
            let team = self.entities[i].ship.team;
            let bp = self.registry.get(self.entities[i].ship.class);
            let (ship_r, cargo) = (bp.radius, bp.cargo);
            // Each harvester deposits at its own team's mothership.
            let team_depot = match team {
                Team::Enemy => enemy_depot,
                _ => depot,
            };

            if g.returning {
                // Deposit at the mothership, then head back out (or finish).
                if let Some((dpos, dr)) = team_depot {
                    if (pos - dpos).length() <= dr + ship_r + 2.5 {
                        match team {
                            Team::Enemy => self.enemy_salvage += g.carrying,
                            _ => self.salvage += g.carrying,
                        }
                        let empty = self
                            .resource_nodes
                            .iter()
                            .find(|n| n.id == g.node)
                            .is_none_or(|n| n.amount <= 0.0);
                        let gm = self.entities[i].gather.as_mut().unwrap();
                        gm.carrying = 0.0;
                        if empty {
                            self.entities[i].gather = None;
                        } else {
                            gm.returning = false;
                        }
                    }
                }
            } else if let Some(n) = self.resource_nodes.iter_mut().find(|n| n.id == g.node) {
                if n.amount <= 0.0 {
                    let gm = self.entities[i].gather.as_mut().unwrap();
                    if gm.carrying > 0.0 {
                        gm.returning = true;
                    } else {
                        self.entities[i].gather = None;
                    }
                } else if (pos - n.pos).length() <= n.radius + ship_r + 2.5 {
                    let want = (HARVEST_RATE * dt).min(cargo - g.carrying).max(0.0);
                    let got = want.min(n.amount);
                    n.amount -= got;
                    let gm = self.entities[i].gather.as_mut().unwrap();
                    gm.carrying += got;
                    if gm.carrying >= cargo - 1e-3 || n.amount <= 0.0 {
                        gm.returning = true;
                    }
                }
            } else {
                // The node vanished: bank what we carry, else drop the task.
                let gm = self.entities[i].gather.as_mut().unwrap();
                if gm.carrying > 0.0 {
                    gm.returning = true;
                } else {
                    self.entities[i].gather = None;
                }
            }
        }

        // 5) Production: advance each team's front build at its mothership; when
        // it finishes, spawn the ship near the carrier at a spread angle.
        if let Some(front) = self.build_queue.first_mut() {
            front.progress += dt;
        }
        let player_done = self
            .build_queue
            .first()
            .copied()
            .filter(|f| f.progress >= self.registry.get(f.class).build_time);
        if let Some(front) = player_done {
            self.build_queue.remove(0);
            let (center, cr) = depot.unwrap_or((Vec3::ZERO, 8.0));
            let angle = self.next_id as f32 * 2.399_963;
            let off = Vec3::new(angle.cos(), 0.0, angle.sin()) * (cr + 5.0);
            self.spawn_class(front.class, Team::Player, center + off);
        }
        if let Some(front) = self.enemy_build_queue.first_mut() {
            front.progress += dt;
        }
        let enemy_done = self
            .enemy_build_queue
            .first()
            .copied()
            .filter(|f| f.progress >= self.registry.get(f.class).build_time);
        if let Some(front) = enemy_done {
            self.enemy_build_queue.remove(0);
            let (center, cr) = enemy_depot.unwrap_or((Vec3::ZERO, 8.0));
            let angle = self.next_id as f32 * 2.399_963;
            let off = Vec3::new(angle.cos(), 0.0, angle.sin()) * (cr + 5.0);
            self.spawn_class(front.class, Team::Enemy, center + off);
        }

        self.tick += 1;
    }

    /// Order-independent, platform-independent state hash (FNV-1a over the tick
    /// and every entity's id/transform/velocity bits). Identical seeds and
    /// command streams must produce identical checksums on native and wasm.
    pub fn checksum(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        h = fnv1a(h, self.tick as u32);
        h = fnv1a(h, (self.tick >> 32) as u32);
        for e in &self.entities {
            h = fnv1a(h, e.id.0);
            for v in [e.transform.pos.x, e.transform.pos.y, e.transform.pos.z] {
                h = fnv1a(h, v.to_bits());
            }
            let r = e.transform.rot;
            for v in [r.x, r.y, r.z, r.w] {
                h = fnv1a(h, v.to_bits());
            }
            for v in [
                e.velocity.linear.x,
                e.velocity.linear.y,
                e.velocity.linear.z,
            ] {
                h = fnv1a(h, v.to_bits());
            }
            h = fnv1a(h, e.hull.to_bits());
            h = fnv1a(h, e.shield.to_bits());
            h = fnv1a(h, e.stance as u32);
            let (ax, ay, az, ap) = match e.anchor {
                Some(a) => (a.x, a.y, a.z, 1u32),
                None => (0.0, 0.0, 0.0, 0u32),
            };
            h = fnv1a(h, ap);
            h = fnv1a(h, ax.to_bits());
            h = fnv1a(h, ay.to_bits());
            h = fnv1a(h, az.to_bits());
            h = fnv1a(h, e.gather.map(|g| g.carrying).unwrap_or(0.0).to_bits());
        }
        h = fnv1a(h, self.salvage.to_bits());
        h = fnv1a(h, self.enemy_salvage.to_bits());
        for n in &self.resource_nodes {
            h = fnv1a(h, n.id.0);
            h = fnv1a(h, n.amount.to_bits());
        }
        for b in &self.build_queue {
            h = fnv1a(h, b.class as u32);
            h = fnv1a(h, b.progress.to_bits());
        }
        for b in &self.enemy_build_queue {
            h = fnv1a(h, b.class as u32);
            h = fnv1a(h, b.progress.to_bits());
        }
        for p in &self.projectiles {
            h = fnv1a(h, p.id.0);
            for v in [p.pos.x, p.pos.y, p.pos.z] {
                h = fnv1a(h, v.to_bits());
            }
        }
        h
    }
}

/// One FNV-1a round over a 32-bit word.
fn fnv1a(h: u64, x: u32) -> u64 {
    (h ^ x as u64).wrapping_mul(0x0000_0100_0000_01b3)
}

/// Apply a hit of `dmg` to a `(shield, hull)` pair, returning the new values.
/// `facing` is the dot of the incoming direction with the target's forward in
/// `[-1, 1]`: +1 struck the front (shield strongest), 0 the side, -1 the rear
/// (shield ineffective). The shield absorbs what it can of the facing-scaled
/// damage; the remainder hits the hull (`design.md` §6.8 shield facing).
fn apply_hit(shield: f32, hull: f32, dmg: f32, facing: f32) -> (f32, f32) {
    let shield_mult = 0.5 + 0.5 * facing.clamp(-1.0, 1.0);
    let absorbed = (dmg * shield_mult).min(shield);
    (shield - absorbed, hull - (dmg - absorbed))
}

/// Deterministic lead/intercept aim (`design.md` §6.8): direction a shot of
/// `speed` should travel from `shooter` to hit a target at `tpos` moving at
/// `tvel`. Solves `|tpos + tvel*t - shooter| = speed*t` for the earliest t >= 0;
/// if the target outruns the round (no solution), aims at its current position.
fn intercept_dir(shooter: Vec3, tpos: Vec3, tvel: Vec3, speed: f32) -> Vec3 {
    let d = tpos - shooter;
    let a = tvel.dot(tvel) - speed * speed;
    let b = 2.0 * d.dot(tvel);
    let c = d.dot(d);
    let t = if a.abs() < 1e-4 {
        // Target speed ~= projectile speed: the quadratic degenerates to linear.
        if b.abs() < 1e-6 {
            -1.0
        } else {
            -c / b
        }
    } else {
        let disc = b * b - 4.0 * a * c;
        if disc < 0.0 {
            -1.0
        } else {
            let sq = disc.sqrt();
            // Smallest positive root of a*t^2 + b*t + c = 0.
            let t0 = (-b - sq) / (2.0 * a);
            let t1 = (-b + sq) / (2.0 * a);
            let lo = t0.min(t1);
            let hi = t0.max(t1);
            if lo > 0.0 {
                lo
            } else if hi > 0.0 {
                hi
            } else {
                -1.0
            }
        }
    };
    let aim = if t > 0.0 { tpos + tvel * t } else { tpos };
    (aim - shooter).normalize_or_zero()
}

/// Rotate the velocity `vel` toward `desired` by at most `max_turn` radians,
/// keeping the magnitude `speed` (guided-missile steering with a turn limit).
fn steer_velocity(vel: Vec3, desired: Vec3, max_turn: f32, speed: f32) -> Vec3 {
    let cur = vel.normalize_or_zero();
    let des = desired.normalize_or_zero();
    if cur == Vec3::ZERO {
        return des * speed;
    }
    let angle = cur.angle_between(des);
    if angle <= max_turn || angle < 1e-5 {
        return des * speed;
    }
    let axis = cur.cross(des);
    let axis = if axis.length_squared() < 1e-8 {
        cur.any_orthonormal_vector() // nearly anti-parallel: pick any perpendicular
    } else {
        axis.normalize()
    };
    (Quat::from_axis_angle(axis, max_turn) * cur) * speed
}

/// Earliest fraction t in `[0, 1]` at which the swept segment `a -> b` enters a
/// sphere of radius `r` at `c`, or `None` if it never does. Used for CCD-style
/// projectile hit detection (no tunneling).
fn segment_sphere_t(a: Vec3, b: Vec3, c: Vec3, r: f32) -> Option<f32> {
    let m = a - c;
    let d = b - a;
    let aa = d.dot(d);
    if aa < 1e-12 {
        return (m.dot(m) <= r * r).then_some(0.0);
    }
    let cc = m.dot(m) - r * r;
    if cc <= 0.0 {
        return Some(0.0); // started inside the sphere
    }
    let bb = 2.0 * m.dot(d);
    let disc = bb * bb - 4.0 * aa * cc;
    if disc < 0.0 {
        return None;
    }
    let t = (-bb - disc.sqrt()) / (2.0 * aa);
    (0.0..=1.0).contains(&t).then_some(t)
}

/// Rotate `from` toward `to` by at most `max_angle` radians (turn-rate clamp).
fn rotate_toward(from: Quat, to: Quat, max_angle: f32) -> Quat {
    let dot = from.dot(to).abs().clamp(-1.0, 1.0);
    let angle = 2.0 * dot.acos();
    if angle <= max_angle || angle < 1e-5 {
        to
    } else {
        from.slerp(to, max_angle / angle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fighter() -> Ship {
        Ship {
            class: ShipClass::Fighter,
            team: Team::Player,
            mass: 18.0,
        }
    }

    #[test]
    fn integrates_position_from_velocity() {
        // With no move order a ship eases to a stop, but a set velocity still
        // integrates for the first step (before the decay dominates).
        let mut world = World::new(42);
        let id = world.spawn(fighter(), Transform::default());
        world.enqueue(Command::SetVelocity {
            entity: id,
            linear: Vec3::new(1.0, 0.0, -2.0),
        });
        world.step(TICK_DT);
        let p = world.entity(id).unwrap().transform.pos;
        assert!(p.x > 0.0 && p.z < 0.0, "did not integrate: {p:?}");
        assert_eq!(world.tick, 1);
    }

    #[test]
    fn prev_transform_trails_current_for_interpolation() {
        let mut world = World::new(1);
        let id = world.spawn(fighter(), Transform::default());
        world.enqueue(Command::SetVelocity {
            entity: id,
            linear: Vec3::new(10.0, 0.0, 0.0),
        });
        world.step(TICK_DT);
        let e = world.entity(id).unwrap();
        assert_eq!(e.prev_transform.pos, Vec3::ZERO);
        assert!(e.transform.pos.x > 0.0);
    }

    /// The core lockstep guarantee: identical seed + identical command stream
    /// (issued at identical ticks) yields identical end-state checksums.
    #[test]
    fn identical_seed_and_commands_match_checksums() {
        fn run() -> u64 {
            let mut w = World::new(7);
            let a = w.spawn_class(ShipClass::Fighter, Team::Player, Vec3::new(-5.0, 0.0, 0.0));
            let b = w.spawn_class(ShipClass::Corvette, Team::Player, Vec3::new(5.0, 0.0, 0.0));
            for t in 0..120u32 {
                if t == 0 {
                    w.enqueue(Command::SetVelocity {
                        entity: a,
                        linear: Vec3::new(3.0, 1.0, 0.0),
                    });
                    w.enqueue(Command::SetVelocity {
                        entity: b,
                        linear: Vec3::new(-2.0, 0.0, 4.0),
                    });
                }
                if t == 60 {
                    w.enqueue(Command::Stop { entity: a });
                }
                w.step(TICK_DT);
            }
            w.checksum()
        }
        assert_eq!(run(), run());
    }

    #[test]
    fn different_commands_diverge() {
        let mut a = World::new(7);
        let mut b = World::new(7);
        let ea = a.spawn_class(ShipClass::Fighter, Team::Player, Vec3::ZERO);
        let eb = b.spawn_class(ShipClass::Fighter, Team::Player, Vec3::ZERO);
        a.enqueue(Command::SetVelocity {
            entity: ea,
            linear: Vec3::X,
        });
        b.enqueue(Command::SetVelocity {
            entity: eb,
            linear: Vec3::NEG_X,
        });
        for _ in 0..10 {
            a.step(TICK_DT);
            b.step(TICK_DT);
        }
        assert_ne!(a.checksum(), b.checksum());
    }

    #[test]
    fn registry_has_every_class() {
        let reg = ShipRegistry::with_defaults();
        for class in ShipClass::ALL {
            assert_eq!(reg.get(class).class, class);
            assert!(reg.get(class).max_speed > 0.0);
        }
        assert_eq!(ShipClass::Fighter.crew_kind(), CrewKind::Pilots);
        assert_eq!(ShipClass::Carrier.crew_kind(), CrewKind::Ops);
    }

    #[test]
    fn patrol_orbits_at_constant_radius() {
        let mut w = World::new(3);
        let id = w.spawn_patrol(
            ShipClass::Fighter,
            Team::Player,
            Patrol {
                center: Vec3::ZERO,
                radius: 10.0,
                height: 0.0,
                angular_speed: 1.0,
                angle: 0.0,
            },
        );
        for _ in 0..200 {
            w.step(TICK_DT);
        }
        let p = w.entity(id).unwrap().transform.pos;
        let r = (p.x * p.x + p.z * p.z).sqrt();
        assert!((r - 10.0).abs() < 1e-3, "patrol radius drifted: {r}");
    }

    #[test]
    fn move_order_arrives_and_stops() {
        let mut w = World::new(1);
        let id = w.spawn_class(ShipClass::Corvette, Team::Player, Vec3::ZERO);
        let target = Vec3::new(15.0, 0.0, 0.0);
        w.enqueue(Command::MoveTo { entity: id, target });
        for _ in 0..(TICK_HZ * 12) {
            w.step(TICK_DT);
        }
        let e = w.entity(id).unwrap();
        assert!(e.order.is_none(), "order was not cleared on arrival");
        assert!(
            (e.transform.pos - target).length() < 1.0,
            "did not arrive: {:?}",
            e.transform.pos
        );
        assert!(
            e.velocity.linear.length() < 0.5,
            "did not stop: {:?}",
            e.velocity.linear
        );
    }

    #[test]
    fn enemy_ai_closes_on_player_in_sensor_range() {
        let mut w = World::new(7);
        w.spawn_class(ShipClass::Corvette, Team::Player, Vec3::ZERO);
        // Fighter sensor range is 30 > 20, so it detects and approaches.
        let e = w.spawn_class(ShipClass::Fighter, Team::Enemy, Vec3::new(20.0, 0.0, 0.0));
        let start = w.entity(e).unwrap().transform.pos.length();
        for _ in 0..(TICK_HZ * 3) {
            w.step(TICK_DT);
        }
        let end = w.entity(e).unwrap().transform.pos.length();
        assert!(
            end < start - 1.0,
            "enemy should close on the player: {start} -> {end}"
        );
    }

    #[test]
    fn enemy_ai_holds_without_contact_or_mothership() {
        let mut w = World::new(7);
        // Player side is a lone Corvette (no mothership/rally point).
        w.spawn_class(ShipClass::Corvette, Team::Player, Vec3::ZERO);
        // Far beyond any sensor range: no contact and no rally, so the enemy holds.
        let far = Vec3::new(300.0, 0.0, 0.0);
        let e = w.spawn_class(ShipClass::Fighter, Team::Enemy, far);
        for _ in 0..(TICK_HZ * 3) {
            w.step(TICK_DT);
        }
        let ent = w.entity(e).unwrap();
        assert!(ent.order.is_none(), "should have no target without contact");
        assert!(
            (ent.transform.pos - far).length() < 0.5,
            "should hold position: {:?}",
            ent.transform.pos
        );
    }

    #[test]
    fn enemy_ai_advances_on_mothership_without_contact() {
        let mut w = World::new(9);
        w.spawn_class(ShipClass::Carrier, Team::Player, Vec3::ZERO);
        // Enemy far beyond its own sensor range of any player, so it has no direct
        // contact, yet it still advances on the mothership objective.
        let far = Vec3::new(300.0, 0.0, 0.0);
        let e = w.spawn_class(ShipClass::Fighter, Team::Enemy, far);
        let start = w.entity(e).unwrap().transform.pos.length();
        for _ in 0..(TICK_HZ * 3) {
            w.step(TICK_DT);
        }
        let end = w.entity(e).unwrap().transform.pos.length();
        assert!(
            end < start - 1.0,
            "enemy should advance on the mothership: {start} -> {end}"
        );
    }

    #[test]
    fn ship_destroys_hostile_in_weapon_range() {
        let mut w = World::new(11);
        // Corvette (range 26, 10 dmg / 0.5s) vs an enemy fighter (80 hull) at 15.
        w.spawn_class(ShipClass::Corvette, Team::Player, Vec3::ZERO);
        let e = w.spawn_class(ShipClass::Fighter, Team::Enemy, Vec3::new(15.0, 0.0, 0.0));
        let mut steps = 0;
        while w.entity(e).is_some() && steps < TICK_HZ * 30 {
            w.step(TICK_DT);
            steps += 1;
        }
        assert!(
            w.entity(e).is_none(),
            "hostile in range should be destroyed"
        );
    }

    #[test]
    fn no_fire_out_of_weapon_range() {
        let mut w = World::new(11);
        w.spawn_class(ShipClass::Corvette, Team::Player, Vec3::ZERO);
        // 200 away: beyond sensor and weapon range, with no mothership to rally on.
        let e = w.spawn_class(ShipClass::Fighter, Team::Enemy, Vec3::new(200.0, 0.0, 0.0));
        let hull0 = w.entity(e).unwrap().hull;
        for _ in 0..(TICK_HZ * 3) {
            w.step(TICK_DT);
        }
        assert!(w.projectiles.is_empty(), "nothing should fire out of range");
        assert_eq!(w.entity(e).unwrap().hull, hull0, "no damage out of range");
    }

    #[test]
    fn missile_frigate_destroys_target() {
        let mut w = World::new(13);
        w.spawn_class(ShipClass::FrigateMissile, Team::Player, Vec3::ZERO);
        // Fighter at 40: inside the frigate's 55 range but outside the fighter's own
        // 30 sensor range, so it sits while homing missiles (45 dmg) destroy it.
        let e = w.spawn_class(ShipClass::Fighter, Team::Enemy, Vec3::new(40.0, 0.0, 0.0));
        let mut steps = 0;
        while w.entity(e).is_some() && steps < TICK_HZ * 30 {
            w.step(TICK_DT);
            steps += 1;
        }
        assert!(
            w.entity(e).is_none(),
            "homing missiles should destroy the target"
        );
    }

    #[test]
    fn combat_is_deterministic() {
        let run = || {
            let mut w = World::new(7);
            w.spawn_class(ShipClass::Carrier, Team::Player, Vec3::ZERO);
            w.spawn_class(ShipClass::Corvette, Team::Player, Vec3::new(8.0, 0.0, 0.0));
            w.spawn_class(ShipClass::Fighter, Team::Enemy, Vec3::new(60.0, 0.0, 10.0));
            w.spawn_class(
                ShipClass::FrigateMissile,
                Team::Enemy,
                Vec3::new(-55.0, 0.0, -20.0),
            );
            for _ in 0..(TICK_HZ * 12) {
                w.step(TICK_DT);
            }
            w.checksum()
        };
        assert_eq!(run(), run(), "combat must be bit-identical across runs");
    }

    #[test]
    fn attack_order_focuses_commanded_target() {
        let mut w = World::new(17);
        let a = w.spawn_class(ShipClass::Corvette, Team::Player, Vec3::ZERO);
        // A nearer enemy the corvette would auto-pick, and the commanded one.
        w.spawn_class(ShipClass::Fighter, Team::Enemy, Vec3::new(15.0, 0.0, 0.0));
        let far = w.spawn_class(ShipClass::Fighter, Team::Enemy, Vec3::new(40.0, 0.0, 5.0));
        w.enqueue(Command::Attack {
            entity: a,
            target: far,
        });
        w.step(TICK_DT);
        assert_eq!(
            w.entity(a).unwrap().target,
            Some(far),
            "commanded target should win over the nearer auto-pick"
        );
    }

    #[test]
    fn attack_order_pursues_and_destroys_target() {
        let mut w = World::new(17);
        let a = w.spawn_class(ShipClass::Corvette, Team::Player, Vec3::ZERO);
        // Target sits beyond the corvette's 26 range, so it must close to engage.
        let far = w.spawn_class(ShipClass::Fighter, Team::Enemy, Vec3::new(40.0, 0.0, 0.0));
        w.enqueue(Command::Attack {
            entity: a,
            target: far,
        });
        let mut steps = 0;
        while w.entity(far).is_some() && steps < TICK_HZ * 40 {
            w.step(TICK_DT);
            steps += 1;
        }
        assert!(
            w.entity(far).is_none(),
            "attacker should pursue and destroy its commanded target"
        );
        assert!(
            w.entity(a).is_some(),
            "the corvette should survive the fighter"
        );
    }

    #[test]
    fn passive_does_not_auto_engage() {
        let mut w = World::new(31);
        let p = w.spawn_class(ShipClass::Corvette, Team::Player, Vec3::ZERO);
        w.enqueue(Command::SetStance {
            entity: p,
            stance: Stance::Passive,
        });
        let e = w.spawn_class(ShipClass::Fighter, Team::Enemy, Vec3::new(15.0, 0.0, 0.0));
        let enemy_hull_start = w.entity(e).unwrap().hull;
        for _ in 0..(TICK_HZ * 3) {
            w.step(TICK_DT);
        }
        // The passive corvette should never have acquired the enemy or fired.
        assert_eq!(w.entity(p).unwrap().target, None);
        let enemy_hull = w.entity(e).map(|x| x.hull).unwrap_or(0.0);
        assert!(
            enemy_hull >= enemy_hull_start - 1e-3,
            "passive ship should not damage the enemy: {enemy_hull} vs {enemy_hull_start}",
        );
    }

    #[test]
    fn sos_rallies_nearby_allies() {
        let mut w = World::new(33);
        let a = w.spawn_class(ShipClass::Corvette, Team::Player, Vec3::ZERO);
        w.spawn_class(ShipClass::Corvette, Team::Player, Vec3::new(5.0, 0.0, 0.0));
        let enemy = w.spawn_class(ShipClass::Fighter, Team::Enemy, Vec3::new(12.0, 0.0, 0.0));
        // Two steps: one for the enemy to acquire the closer corvette, one for SOS
        // to wake the further one with an attack_target.
        for _ in 0..6 {
            w.step(TICK_DT);
        }
        assert_eq!(
            w.entity(a).unwrap().attack_target,
            Some(enemy),
            "SOS should rally the ally to the attacker"
        );
    }

    #[test]
    fn defensive_returns_to_anchor() {
        let mut w = World::new(35);
        let id = w.spawn_class(ShipClass::Corvette, Team::Player, Vec3::ZERO);
        // Displace it; anchor (origin) was set at spawn time. With no hostile in
        // range, the Defensive anchor-return order should drift it back.
        w.entities[0].transform.pos = Vec3::new(20.0, 0.0, 0.0);
        for _ in 0..(TICK_HZ * 5) {
            w.step(TICK_DT);
        }
        let end = w.entity(id).unwrap().transform.pos.length();
        assert!(end < 3.0, "should drift back toward anchor: {end}");
    }

    #[test]
    fn aggressive_pursues_out_of_weapon_range_hostile() {
        let mut w = World::new(37);
        let p = w.spawn_class(ShipClass::Corvette, Team::Player, Vec3::ZERO);
        w.enqueue(Command::SetStance {
            entity: p,
            stance: Stance::Aggressive,
        });
        // Fighter at 35: beyond the corvette's 26 weapon range but inside its
        // 40 sensor range, so aggressive auto-acquisition picks it up and pursues.
        let e = w.spawn_class(ShipClass::Fighter, Team::Enemy, Vec3::new(35.0, 0.0, 0.0));
        let start_pos = w.entity(p).unwrap().transform.pos;
        for _ in 0..(TICK_HZ * 4) {
            w.step(TICK_DT);
        }
        let moved =
            (w.entity(p).map(|x| x.transform.pos).unwrap_or(start_pos) - start_pos).length();
        let killed = w.entity(e).is_none();
        assert!(
            moved > 2.0 || killed,
            "aggressive should close on (or destroy) an out-of-range hostile: moved {moved}, killed {killed}",
        );
    }

    #[test]
    fn enemy_commander_dispatches_idle_harvesters() {
        let mut w = World::new(61);
        w.spawn_class(ShipClass::Carrier, Team::Enemy, Vec3::new(100.0, 0.0, 0.0));
        let h = w.spawn_class(
            ShipClass::Resourcer,
            Team::Enemy,
            Vec3::new(102.0, 0.0, 0.0),
        );
        let node = w.spawn_resource_node(Vec3::new(110.0, 0.0, 0.0), 500.0, 4.0);
        // No Gather command: the commander should assign one on the first step.
        w.step(TICK_DT);
        let g = w
            .entity(h)
            .and_then(|e| e.gather)
            .expect("commander should assign a gather task");
        assert_eq!(g.node, node);
    }

    #[test]
    fn enemy_commander_funds_a_build_when_affordable() {
        let mut w = World::new(63);
        w.spawn_class(ShipClass::Carrier, Team::Enemy, Vec3::ZERO);
        // Cheapest fighter (30 salvage); seed enough to fund one.
        w.enemy_salvage = 100.0;
        w.step(TICK_DT);
        assert!(
            !w.enemy_build_queue.is_empty(),
            "commander should queue an affordable build"
        );
        assert!(
            w.enemy_salvage < 100.0,
            "queueing should debit the enemy salvage pool"
        );
    }

    #[test]
    fn enemy_harvester_deposits_to_enemy_salvage() {
        let mut w = World::new(51);
        w.spawn_class(ShipClass::Carrier, Team::Enemy, Vec3::new(100.0, 0.0, 0.0));
        let h = w.spawn_class(ShipClass::Resourcer, Team::Enemy, Vec3::new(95.0, 0.0, 8.0));
        let node = w.spawn_resource_node(Vec3::new(105.0, 0.0, 8.0), 500.0, 4.0);
        w.enqueue(Command::Gather { entity: h, node });
        // Multiple harvest+deposit cycles within 25s.
        for _ in 0..(TICK_HZ * 25) {
            w.step(TICK_DT);
        }
        assert!(
            w.enemy_salvage > 0.0,
            "enemy harvester should deposit to enemy salvage: {}",
            w.enemy_salvage,
        );
        assert_eq!(w.salvage, 0.0, "player salvage should not change");
    }

    #[test]
    fn non_combat_flees_toward_armed_ally_on_hit() {
        let mut w = World::new(41);
        let v = w.spawn_class(ShipClass::Resourcer, Team::Player, Vec3::ZERO);
        // Armed escort sits on +z; an enemy on -x opens fire.
        w.spawn_class(
            ShipClass::FrigateGeneral,
            Team::Player,
            Vec3::new(0.0, 0.0, 10.0),
        );
        w.spawn_class(ShipClass::Fighter, Team::Enemy, Vec3::new(-12.0, 0.0, 0.0));
        let start_z = w.entity(v).unwrap().transform.pos.z;
        for _ in 0..(TICK_HZ * 4) {
            w.step(TICK_DT);
        }
        let ve = w.entity(v).expect("resourcer should survive");
        assert!(
            ve.transform.pos.z > start_z + 1.0,
            "resourcer should flee toward the +z escort: {} -> {}",
            start_z,
            ve.transform.pos.z
        );
    }

    #[test]
    fn defensive_retaliates_against_attacker() {
        let mut w = World::new(43);
        let v = w.spawn_class(ShipClass::Corvette, Team::Player, Vec3::ZERO);
        let atk = w.spawn_class(ShipClass::Fighter, Team::Enemy, Vec3::new(15.0, 0.0, 0.0));
        let mut retaliated = false;
        for _ in 0..(TICK_HZ * 2) {
            w.step(TICK_DT);
            if let Some(ve) = w.entity(v) {
                if ve.attack_target == Some(atk) {
                    retaliated = true;
                    break;
                }
            }
        }
        assert!(
            retaliated,
            "defensive corvette should retaliate against its attacker"
        );
    }

    #[test]
    fn combat_emits_fire_and_hit_events() {
        let mut w = World::new(21);
        w.spawn_class(ShipClass::Corvette, Team::Player, Vec3::ZERO);
        w.spawn_class(ShipClass::Fighter, Team::Enemy, Vec3::new(12.0, 0.0, 0.0));
        let mut saw_fired = false;
        let mut saw_hit = false;
        for _ in 0..(TICK_HZ * 5) {
            w.step(TICK_DT);
            for ev in w.drain_combat_events() {
                match ev {
                    CombatEvent::Fired { .. } => saw_fired = true,
                    CombatEvent::Hit { .. } => saw_hit = true,
                }
            }
        }
        assert!(saw_fired, "a weapon should have fired");
        assert!(saw_hit, "a projectile should have struck a ship");
    }

    #[test]
    fn shield_absorbs_by_facing() {
        // Front hit: shield fully engaged (absorbs min(dmg, shield)).
        let (s, h) = apply_hit(40.0, 400.0, 60.0, 1.0);
        assert!(s.abs() < 1e-3 && (h - 380.0).abs() < 1e-3, "front: {s},{h}");
        // Rear hit: shield ineffective, all damage to hull.
        let (s, h) = apply_hit(40.0, 400.0, 60.0, -1.0);
        assert!(
            (s - 40.0).abs() < 1e-3 && (h - 340.0).abs() < 1e-3,
            "rear: {s},{h}"
        );
        // Side hit: half engaged (absorbs 30, 30 to hull).
        let (s, h) = apply_hit(40.0, 400.0, 60.0, 0.0);
        assert!(
            (s - 10.0).abs() < 1e-3 && (h - 370.0).abs() < 1e-3,
            "side: {s},{h}"
        );
    }

    #[test]
    fn shields_regenerate_after_delay() {
        let mut w = World::new(3);
        let id = w.spawn_class(ShipClass::FrigateGeneral, Team::Player, Vec3::ZERO);
        let max = w.registry.get(ShipClass::FrigateGeneral).max_shield;
        w.entities[0].shield = 0.0; // drained; no enemies, so it recharges
        for _ in 0..(TICK_HZ * 8) {
            w.step(TICK_DT);
        }
        let s = w.entity(id).unwrap().shield;
        assert!(
            s > 0.0 && s <= max + 1e-3,
            "shield should regenerate: {s} / {max}"
        );
    }

    #[test]
    fn separation_pushes_overlapping_ships_apart() {
        let mut w = World::new(2);
        let a = w.spawn_class(ShipClass::Corvette, Team::Player, Vec3::ZERO);
        let b = w.spawn_class(ShipClass::Corvette, Team::Player, Vec3::new(0.05, 0.0, 0.0));
        // Both ordered far away on the same heading: separation must spread them.
        let target = Vec3::new(0.0, 0.0, 80.0);
        w.enqueue(Command::MoveTo { entity: a, target });
        w.enqueue(Command::MoveTo { entity: b, target });
        for _ in 0..100 {
            w.step(TICK_DT);
        }
        let pa = w.entity(a).unwrap().transform.pos;
        let pb = w.entity(b).unwrap().transform.pos;
        let r = w.registry.get(ShipClass::Corvette).radius;
        assert!(
            (pa - pb).length() > r,
            "ships still overlapping ({} apart, radius {r})",
            (pa - pb).length()
        );
    }

    #[test]
    fn resourcer_harvests_node_and_banks_salvage() {
        let mut w = World::new(9);
        w.spawn_class(ShipClass::Carrier, Team::Player, Vec3::ZERO); // depot
        let r = w.spawn_class(ShipClass::Resourcer, Team::Player, Vec3::new(6.0, 0.0, 0.0));
        let node = w.spawn_resource_node(Vec3::new(40.0, 0.0, 0.0), 150.0, 3.0);
        w.enqueue(Command::Gather { entity: r, node });
        // Long enough for a full out-harvest-return-deposit cycle.
        for _ in 0..(TICK_HZ * 60) {
            w.step(TICK_DT);
        }
        assert_eq!(w.node(node).unwrap().amount, 0.0, "node not depleted");
        assert!(
            (w.salvage - 150.0).abs() < 1.0,
            "salvage not banked: {}",
            w.salvage
        );
        // Task clears once the node is exhausted.
        assert!(w.entity(r).unwrap().gather.is_none());
    }

    #[test]
    fn non_resourcer_cannot_gather() {
        let mut w = World::new(1);
        let f = w.spawn_class(ShipClass::Fighter, Team::Player, Vec3::ZERO);
        let node = w.spawn_resource_node(Vec3::new(20.0, 0.0, 0.0), 100.0, 3.0);
        w.enqueue(Command::Gather { entity: f, node });
        w.step(TICK_DT);
        assert!(w.entity(f).unwrap().gather.is_none(), "fighter took gather");
    }

    #[test]
    fn gather_is_deterministic() {
        fn run() -> u64 {
            let mut w = World::new(11);
            w.spawn_class(ShipClass::Carrier, Team::Player, Vec3::ZERO);
            let r = w.spawn_class(ShipClass::Resourcer, Team::Player, Vec3::new(6.0, 0.0, 0.0));
            let node = w.spawn_resource_node(Vec3::new(35.0, 2.0, -10.0), 400.0, 3.5);
            w.enqueue(Command::Gather { entity: r, node });
            for _ in 0..(TICK_HZ * 20) {
                w.step(TICK_DT);
            }
            w.checksum()
        }
        assert_eq!(run(), run());
    }

    #[test]
    fn build_spends_salvage_and_spawns_a_ship() {
        let mut w = World::new(4);
        w.spawn_class(ShipClass::Carrier, Team::Player, Vec3::ZERO);
        w.salvage = 500.0;
        let before = w.entities.len();
        let cost = w.registry.get(ShipClass::Corvette).cost;
        w.enqueue(Command::Build {
            class: ShipClass::Corvette,
        });
        w.step(TICK_DT);
        // Paid up front and queued.
        assert_eq!(w.salvage, 500.0 - cost);
        assert_eq!(w.build_queue.len(), 1);
        // Finishes within its build time and spawns one ship.
        let bt = w.registry.get(ShipClass::Corvette).build_time;
        for _ in 0..((bt * TICK_HZ as f32) as u32 + 2) {
            w.step(TICK_DT);
        }
        assert!(w.build_queue.is_empty(), "build did not finish");
        assert_eq!(w.entities.len(), before + 1, "no ship spawned");
        assert!(w
            .entities
            .iter()
            .any(|e| e.ship.class == ShipClass::Corvette));
    }

    #[test]
    fn carrier_provides_crew_and_ships_reserve_it() {
        let mut w = World::new(4);
        w.spawn_class(ShipClass::Carrier, Team::Player, Vec3::ZERO);
        assert_eq!(w.crew_capacity(), (2000, 100));
        assert_eq!(w.crew_free(), (2000, 100)); // carrier itself uses none
        w.spawn_class(
            ShipClass::FrigateGeneral,
            Team::Player,
            Vec3::new(20.0, 0.0, 0.0),
        );
        w.spawn_class(ShipClass::Fighter, Team::Player, Vec3::new(-20.0, 0.0, 0.0));
        // Frigate draws 100 ops; fighter draws 1 pilot.
        assert_eq!(w.crew_free(), (2000 - 100, 100 - 1));
    }

    #[test]
    fn apply_commands_registers_orders_without_advancing() {
        // The active-pause path: commands take effect, but nothing advances.
        let mut w = World::new(4);
        w.spawn_class(ShipClass::Carrier, Team::Player, Vec3::ZERO);
        let id = w.spawn_class(ShipClass::Corvette, Team::Player, Vec3::ZERO);
        w.salvage = 500.0;
        let cost = w.registry.get(ShipClass::Fighter).cost;
        let tick0 = w.tick;

        w.enqueue(Command::Build {
            class: ShipClass::Fighter,
        });
        w.enqueue(Command::MoveTo {
            entity: id,
            target: Vec3::new(20.0, 0.0, 0.0),
        });
        w.apply_commands();

        // Build debited + queued immediately, but it has not progressed.
        assert_eq!(w.salvage, 500.0 - cost);
        assert_eq!(w.build_queue.len(), 1);
        assert_eq!(w.build_queue[0].progress, 0.0);
        // Move order registered, but the ship has not moved and no tick elapsed.
        assert_eq!(w.entity(id).unwrap().order, Some(Vec3::new(20.0, 0.0, 0.0)));
        assert_eq!(w.entity(id).unwrap().transform.pos, Vec3::ZERO);
        assert_eq!(w.tick, tick0);
    }

    #[test]
    fn build_rejected_without_crew() {
        let mut w = World::new(4);
        // No carrier means no crew pool, so even flush with salvage we can't build.
        w.salvage = 1000.0;
        w.enqueue(Command::Build {
            class: ShipClass::Fighter,
        });
        w.step(TICK_DT);
        assert!(w.build_queue.is_empty(), "built with no crew capacity");
        assert_eq!(w.salvage, 1000.0, "salvage spent despite no crew");
    }

    #[test]
    fn build_rejected_when_unaffordable() {
        let mut w = World::new(4);
        w.spawn_class(ShipClass::Carrier, Team::Player, Vec3::ZERO);
        w.salvage = 5.0; // far below any cost
        w.enqueue(Command::Build {
            class: ShipClass::Fighter,
        });
        w.step(TICK_DT);
        assert!(w.build_queue.is_empty());
        assert_eq!(w.salvage, 5.0);
    }

    #[test]
    fn hull_starts_at_blueprint_max() {
        let mut w = World::new(5);
        let id = w.spawn_class(ShipClass::Carrier, Team::Player, Vec3::ZERO);
        let e = w.entity(id).unwrap();
        assert_eq!(e.hull, w.registry.get(ShipClass::Carrier).max_hull);
    }

    #[test]
    fn rng_is_deterministic_for_same_seed() {
        let mut a = Rng::new(123);
        let mut b = Rng::new(123);
        for _ in 0..1000 {
            assert_eq!(a.next_u32(), b.next_u32());
        }
    }
}
