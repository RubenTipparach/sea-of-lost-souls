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
/// smoothness is independent of this rate. 30 Hz is the RTS default: cheap on
/// single-threaded wasm and light on the (future) lockstep command stream.
/// Raising it to 60 is a one-line change here.
pub const TICK_HZ: u32 = 30;
/// Seconds per fixed step (`1 / TICK_HZ`).
pub const TICK_DT: f32 = 1.0 / TICK_HZ as f32;

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
        // (class, mass t, max_speed, accel, turn deg/s, crew)
        let rows: [(ShipClass, f32, f32, f32, f32, u32); 8] = [
            (ShipClass::Fighter, 18.0, 140.0, 80.0, 120.0, 2),
            (ShipClass::Bomber, 30.0, 110.0, 55.0, 80.0, 3),
            (ShipClass::Corvette, 220.0, 85.0, 35.0, 55.0, 6),
            (ShipClass::Resourcer, 140.0, 60.0, 25.0, 40.0, 3),
            (ShipClass::FrigateGeneral, 4000.0, 45.0, 12.0, 22.0, 18),
            (ShipClass::FrigateMissile, 4200.0, 42.0, 11.0, 20.0, 18),
            (ShipClass::CapitalDestroyer, 16000.0, 28.0, 6.0, 10.0, 60),
            (ShipClass::Carrier, 40000.0, 18.0, 3.0, 6.0, 120),
        ];
        let mut blueprints = HashMap::new();
        for (class, mass, max_speed, accel, turn_rate_deg, crew) in rows {
            blueprints.insert(
                class,
                Blueprint {
                    class,
                    mass,
                    max_speed,
                    accel,
                    turn_rate_deg,
                    crew,
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

/// One simulated entity. `prev_transform` holds the previous step's transform
/// so the renderer can interpolate; the sim itself never reads it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Entity {
    pub id: EntityId,
    pub transform: Transform,
    pub prev_transform: Transform,
    pub velocity: Velocity,
    pub ship: Ship,
}

/// A player/AI order. Orders are queued and applied at fixed-step boundaries
/// (the lockstep seam), never as direct mutations from input. Steering for
/// `MoveTo` lands in Stage 7; for now movement is expressed as a target
/// velocity so the command path and determinism are exercised end to end.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Command {
    /// Set an entity's linear velocity directly (placeholder for steering).
    SetVelocity { entity: EntityId, linear: Vec3 },
    /// Halt an entity.
    Stop { entity: EntityId },
}

/// The deterministic simulation world.
///
/// Entities live in a `Vec` in stable spawn order (iteration order is
/// deterministic, required for lockstep). The seeded [`Rng`] owns all gameplay
/// randomness; the [`ShipRegistry`] is read-only static data.
#[derive(Clone, Debug)]
pub struct World {
    pub entities: Vec<Entity>,
    pub registry: ShipRegistry,
    pub rng: Rng,
    /// Number of fixed steps taken; part of the determinism checksum.
    pub tick: u64,
    next_id: u32,
    /// Orders accumulated since the last step, drained at the next step.
    pending: Vec<Command>,
}

impl World {
    /// Create an empty world seeded deterministically.
    pub fn new(seed: u64) -> Self {
        Self {
            entities: Vec::new(),
            registry: ShipRegistry::with_defaults(),
            rng: Rng::new(seed),
            tick: 0,
            next_id: 0,
            pending: Vec::new(),
        }
    }

    /// Spawn a ship at `transform` and return its stable id.
    pub fn spawn(&mut self, ship: Ship, transform: Transform) -> EntityId {
        let id = EntityId(self.next_id);
        self.next_id += 1;
        self.entities.push(Entity {
            id,
            transform,
            prev_transform: transform,
            velocity: Velocity::default(),
            ship,
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

    /// Advance the simulation by one fixed timestep `dt` (seconds):
    /// apply queued commands, snapshot transforms for interpolation, then
    /// integrate. Iteration is in stable order; no wall-clock or OS RNG.
    pub fn step(&mut self, dt: f32) {
        // 1) Apply orders queued since the last step, in arrival order.
        let pending = std::mem::take(&mut self.pending);
        for command in pending {
            match command {
                Command::SetVelocity { entity, linear } => {
                    if let Some(e) = self.entity_mut(entity) {
                        e.velocity.linear = linear;
                    }
                }
                Command::Stop { entity } => {
                    if let Some(e) = self.entity_mut(entity) {
                        e.velocity.linear = Vec3::ZERO;
                    }
                }
            }
        }

        // 2) Snapshot for render interpolation, then integrate (explicit Euler).
        for e in &mut self.entities {
            e.prev_transform = e.transform;
            e.transform.pos += e.velocity.linear * dt;
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
            for v in [e.velocity.linear.x, e.velocity.linear.y, e.velocity.linear.z] {
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
        let mut world = World::new(42);
        let id = world.spawn(fighter(), Transform::default());
        world.enqueue(Command::SetVelocity {
            entity: id,
            linear: Vec3::new(1.0, 0.0, -2.0),
        });
        // 30 steps of TICK_DT == 1.0s of integration (the command applies on
        // the first step, before integration).
        for _ in 0..TICK_HZ {
            world.step(TICK_DT);
        }
        let p = world.entity(id).unwrap().transform.pos;
        assert!((p.x - 1.0).abs() < 1e-4, "x = {}", p.x);
        assert!((p.z + 2.0).abs() < 1e-4, "z = {}", p.z);
        assert_eq!(world.tick, TICK_HZ as u64);
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
    fn rng_is_deterministic_for_same_seed() {
        let mut a = Rng::new(123);
        let mut b = Rng::new(123);
        for _ in 0..1000 {
            assert_eq!(a.next_u32(), b.next_u32());
        }
    }
}
