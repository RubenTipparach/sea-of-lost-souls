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
        // (class, mass t, max_speed, accel, turn deg/s, crew, radius, max_hull).
        // Speeds are tuned for the current demo scene (tens of units across), not
        // the plan's eventual large-world values; radius drives separation and
        // picking; max_hull is placeholder until combat tuning lands.
        type Row = (ShipClass, f32, f32, f32, f32, u32, f32, f32);
        let rows: [Row; 8] = [
            (ShipClass::Fighter, 18.0, 16.0, 24.0, 130.0, 2, 1.0, 80.0),
            (ShipClass::Bomber, 30.0, 12.0, 16.0, 90.0, 3, 1.5, 120.0),
            (ShipClass::Corvette, 220.0, 10.0, 12.0, 70.0, 6, 2.25, 400.0),
            (ShipClass::Resourcer, 140.0, 8.0, 9.0, 55.0, 3, 2.5, 300.0),
            (
                ShipClass::FrigateGeneral,
                4000.0,
                6.0,
                5.0,
                30.0,
                18,
                4.25,
                2000.0,
            ),
            (
                ShipClass::FrigateMissile,
                4200.0,
                5.5,
                4.5,
                28.0,
                18,
                4.5,
                1900.0,
            ),
            (
                ShipClass::CapitalDestroyer,
                16000.0,
                4.0,
                3.0,
                14.0,
                60,
                6.0,
                8000.0,
            ),
            (
                ShipClass::Carrier,
                40000.0,
                3.0,
                2.0,
                8.0,
                120,
                8.0,
                20000.0,
            ),
        ];
        let mut blueprints = HashMap::new();
        for (class, mass, max_speed, accel, turn_rate_deg, crew, radius, max_hull) in rows {
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
    /// Halt an entity and clear any move order.
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

    /// Advance the simulation by one fixed timestep `dt` (seconds):
    /// apply queued commands, snapshot transforms for interpolation, then
    /// integrate. Iteration is in stable order; no wall-clock or OS RNG.
    pub fn step(&mut self, dt: f32) {
        // 1) Apply queued orders at this step boundary, in arrival order.
        let pending = std::mem::take(&mut self.pending);
        for command in pending {
            match command {
                Command::MoveTo { entity, target } => {
                    if let Some(e) = self.entity_mut(entity) {
                        e.order = Some(target);
                        e.patrol = None; // a direct order overrides a patrol
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
                        e.velocity.linear = Vec3::ZERO;
                    }
                }
            }
        }

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

        // 3) Advance each entity. Patrols follow their circle; otherwise run
        // arrive + separation steering, clamped to the blueprint's limits.
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

            // Desired velocity is zero unless we have a move order; an arrived or
            // idle ship eases to a stop (it never separates, so it holds station).
            let mut desired = Vec3::ZERO;
            if let Some(target) = e.order {
                let to = target - my_pos;
                let dist = to.length();
                let arrive = bp.radius * 0.5 + 0.3;
                if dist <= arrive {
                    e.order = None;
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
