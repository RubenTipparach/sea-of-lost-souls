//! A small, deterministic PRNG (PCG-XSH-RR 64/32).
//!
//! INVARIANT: this is the ONLY source of randomness allowed in `sol-sim`. It
//! is seeded explicitly and contains no OS/thread entropy, so the same seed
//! yields the same stream on every platform (native and wasm) - required for
//! lockstep determinism. Do NOT replace this with `rand`'s thread RNG.
//!
//! Reference: M. E. O'Neill, "PCG: A Family of Simple Fast Space-Efficient
//! Statistically Good Algorithms for Random Number Generation" (2014).

/// Multiplier from the PCG reference implementation (LCG, 64-bit state).
const PCG_MULT: u64 = 6_364_136_223_846_793_005;
/// Default stream-selection increment from the PCG reference implementation.
const PCG_INC: u64 = 1_442_695_040_888_963_407;

/// A deterministic 64/32 PCG random number generator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rng {
    state: u64,
    inc: u64,
}

impl Rng {
    /// Seed the generator. Uses the canonical PCG seeding routine so the
    /// stream is well-distributed even for small seeds like `0` or `1`.
    pub fn new(seed: u64) -> Self {
        let mut rng = Self {
            state: 0,
            inc: PCG_INC,
        };
        // Canonical pcg32_srandom: step, add seed, step again.
        rng.step();
        rng.state = rng.state.wrapping_add(seed);
        rng.step();
        rng
    }

    /// Seed with both an initial state and a stream selector, so independent
    /// streams can be derived (e.g. per-system sub-RNGs) without correlation.
    pub fn with_stream(seed: u64, stream: u64) -> Self {
        let mut rng = Self {
            state: 0,
            // `inc` must be odd; shifting left by 1 and setting the low bit
            // guarantees that for any `stream`.
            inc: (stream << 1) | 1,
        };
        rng.step();
        rng.state = rng.state.wrapping_add(seed);
        rng.step();
        rng
    }

    #[inline]
    fn step(&mut self) {
        self.state = self.state.wrapping_mul(PCG_MULT).wrapping_add(self.inc);
    }

    /// Next pseudo-random `u32` (PCG-XSH-RR output function).
    #[inline]
    pub fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.step();
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        xorshifted.rotate_right(rot)
    }

    /// Next pseudo-random `u64`, assembled from two 32-bit draws.
    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        let hi = self.next_u32() as u64;
        let lo = self.next_u32() as u64;
        (hi << 32) | lo
    }

    /// Uniform `f32` in the half-open range `[0, 1)`.
    ///
    /// Uses the top 24 bits (the f32 mantissa width) so the result is exactly
    /// representable and platform-independent.
    #[inline]
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 * (1.0 / (1u32 << 24) as f32)
    }

    /// Uniform `u32` in `[0, bound)` using rejection sampling to avoid modulo
    /// bias. `bound` of 0 returns 0.
    #[inline]
    pub fn gen_below(&mut self, bound: u32) -> u32 {
        if bound == 0 {
            return 0;
        }
        // Reject the biased low tail so the result is uniform.
        let threshold = bound.wrapping_neg() % bound;
        loop {
            let r = self.next_u32();
            if r >= threshold {
                return r % bound;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_stream() {
        let mut a = Rng::new(7);
        let mut b = Rng::new(7);
        for _ in 0..10_000 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn different_seeds_diverge() {
        let mut a = Rng::new(1);
        let mut b = Rng::new(2);
        assert_ne!(a.next_u32(), b.next_u32());
    }

    #[test]
    fn f32_in_unit_interval() {
        let mut r = Rng::new(99);
        for _ in 0..100_000 {
            let x = r.next_f32();
            assert!((0.0..1.0).contains(&x));
        }
    }

    #[test]
    fn gen_below_respects_bound() {
        let mut r = Rng::new(5);
        for _ in 0..100_000 {
            assert!(r.gen_below(6) < 6);
        }
        assert_eq!(r.gen_below(0), 0);
    }
}
