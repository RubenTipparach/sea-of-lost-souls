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

pub mod rng;

pub use rng::Rng;

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

/// Marker component for a ship, carrying its mass (in tonnes, matching
/// `ship.json`). Mass will later feed Newtonian steering / momentum.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ship {
    pub mass: f32,
}

/// A minimal entity bundle for the scaffolding. The real game will use an ECS
/// (`bevy_ecs`) with stable iteration order; this struct keeps the core types
/// usable and testable today without pulling in a graphical or unordered
/// dependency.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Body {
    pub transform: Transform,
    pub velocity: Velocity,
    pub ship: Ship,
}

/// The deterministic simulation world.
///
/// Entities are stored in a `Vec` so iteration order is stable (index order),
/// which is required for determinism. The seeded [`Rng`] is owned here so all
/// gameplay randomness is reproducible from the initial seed.
#[derive(Clone, Debug)]
pub struct World {
    pub bodies: Vec<Body>,
    pub rng: Rng,
    /// Accumulated number of fixed steps taken; useful for checksums/turns.
    pub tick: u64,
}

impl World {
    /// Create an empty world seeded deterministically.
    pub fn new(seed: u64) -> Self {
        Self {
            bodies: Vec::new(),
            rng: Rng::new(seed),
            tick: 0,
        }
    }

    /// Spawn a body and return its stable index.
    pub fn spawn(&mut self, body: Body) -> usize {
        self.bodies.push(body);
        self.bodies.len() - 1
    }

    /// Advance the simulation by one fixed timestep `dt` (seconds).
    ///
    /// Integrates positions from velocities (explicit Euler). Iteration is in
    /// stable index order; no wall-clock or OS RNG is consulted.
    pub fn step(&mut self, dt: f32) {
        for body in &mut self.bodies {
            body.transform.pos += body.velocity.linear * dt;
        }
        self.tick += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integrates_position_from_velocity() {
        let mut world = World::new(42);
        let idx = world.spawn(Body {
            transform: Transform::default(),
            velocity: Velocity {
                linear: Vec3::new(1.0, 0.0, -2.0),
            },
            ship: Ship { mass: 60.0 },
        });
        // 10 steps of 0.1s == 1.0s of integration.
        for _ in 0..10 {
            world.step(0.1);
        }
        let p = world.bodies[idx].transform.pos;
        assert!((p.x - 1.0).abs() < 1e-4);
        assert!((p.z + 2.0).abs() < 1e-4);
        assert_eq!(world.tick, 10);
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
