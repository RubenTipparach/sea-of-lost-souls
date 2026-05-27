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
    /// Current hull points; starts at the blueprint `max_hull`. Combat drains it
    /// later; the HUD reads `hull / max_hull` for the health bar.
    pub hull: f32,
    /// Active move order target, if any (steered toward with arrive + turn).
    pub order: Option<Vec3>,
    /// Optional circular patrol; when set, the step drives the transform.
    pub patrol: Option<Patrol>,
    /// Active gather task (resourcers only); overrides move orders/patrol.
    pub gather: Option<Gather>,
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
    /// Harvestable salvage sites (asteroids); drained by resourcers.
    pub resource_nodes: Vec<ResourceNode>,
    /// Salvage banked at the mothership (the player's resource pool).
    pub salvage: f32,
    /// Ships under construction at the mothership (front item builds first).
    pub build_queue: Vec<BuildItem>,
    pub registry: ShipRegistry,
    pub rng: Rng,
    /// Number of fixed steps taken; part of the determinism checksum.
    pub tick: u64,
    next_id: u32,
    next_node_id: u32,
    /// Orders accumulated since the last step, drained at the next step.
    pending: Vec<Command>,
}

impl World {
    /// Create an empty world seeded deterministically.
    pub fn new(seed: u64) -> Self {
        Self {
            entities: Vec::new(),
            resource_nodes: Vec::new(),
            salvage: 0.0,
            build_queue: Vec::new(),
            registry: ShipRegistry::with_defaults(),
            rng: Rng::new(seed),
            tick: 0,
            next_id: 0,
            next_node_id: 0,
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
        let max_hull = self.registry.get(ship.class).max_hull;
        self.entities.push(Entity {
            id,
            transform,
            prev_transform: transform,
            velocity: Velocity::default(),
            ship,
            hull: max_hull,
            order: None,
            patrol: None,
            gather: None,
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
                        e.velocity.linear = Vec3::ZERO;
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

        // Deposit point: the player's mothership (first carrier in spawn order).
        let depot: Option<(Vec3, f32)> = self
            .entities
            .iter()
            .find(|e| e.ship.class == ShipClass::Carrier && e.ship.team == Team::Player)
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
                let range = self.registry.get(e.ship.class).sensor_range;
                let max_d2 = range * range;
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
                e.order = best.or(rally);
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
                    depot
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

        // 4) Resource harvest / deposit, in a separate pass so the node list and
        // the salvage pool aren't aliased with the entity iteration. Stable
        // (entity index) order keeps it deterministic.
        for i in 0..self.entities.len() {
            let Some(g) = self.entities[i].gather else {
                continue;
            };
            let pos = self.entities[i].transform.pos;
            let bp = self.registry.get(self.entities[i].ship.class);
            let (ship_r, cargo) = (bp.radius, bp.cargo);

            if g.returning {
                // Deposit at the mothership, then head back out (or finish).
                if let Some((dpos, dr)) = depot {
                    if (pos - dpos).length() <= dr + ship_r + 2.5 {
                        self.salvage += g.carrying;
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

        // 5) Production: advance the front build at the mothership; when it
        // finishes, spawn the ship near the carrier at a spread angle.
        if let Some(front) = self.build_queue.first_mut() {
            front.progress += dt;
        }
        if let Some(front) = self.build_queue.first().copied() {
            if front.progress >= self.registry.get(front.class).build_time {
                self.build_queue.remove(0);
                let (center, cr) = depot.unwrap_or((Vec3::ZERO, 8.0));
                // Golden-angle spread so successive builds don't stack.
                let angle = self.next_id as f32 * 2.399_963;
                let off = Vec3::new(angle.cos(), 0.0, angle.sin()) * (cr + 5.0);
                self.spawn_class(front.class, Team::Player, center + off);
            }
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
            h = fnv1a(h, e.gather.map(|g| g.carrying).unwrap_or(0.0).to_bits());
        }
        h = fnv1a(h, self.salvage.to_bits());
        for n in &self.resource_nodes {
            h = fnv1a(h, n.id.0);
            h = fnv1a(h, n.amount.to_bits());
        }
        for b in &self.build_queue {
            h = fnv1a(h, b.class as u32);
            h = fnv1a(h, b.progress.to_bits());
        }
        h
    }
}

/// One FNV-1a round over a 32-bit word.
fn fnv1a(h: u64, x: u32) -> u64 {
    (h ^ x as u64).wrapping_mul(0x0000_0100_0000_01b3)
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
