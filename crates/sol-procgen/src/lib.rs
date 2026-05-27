//! `sol-procgen` - seeded procedural generation for Sea of Lost Souls.
//!
//! Hosts the seeded generators for the space environment. Gameplay-affecting
//! fields (gravity, sensor occlusion, hazards) must be reproducible from a seed
//! across peers; purely visual layers (like the background here) are derived
//! from the same seed but never feed back into gameplay, so they may be
//! elaborated freely. See `design.md` §7.
//!
//! The background is the Homeworld-style blend: a vertex-colored nebula sphere
//! (smooth gradients at any FOV) plus a star field. We approximate the classic
//! edge-detect + Delaunay vertex placement with a tessellated sphere whose
//! vertices are colored by a procedural nebula field, which yields the same
//! gradient look without the offline mesh-reduction pipeline.

#![forbid(unsafe_code)]

use glam::Vec3;

/// CPU-side background geometry: a vertex-colored nebula sphere (triangles) and
/// a set of star points. Positions are in world units (a large sphere drawn as
/// a distant backdrop); colors are linear RGB.
pub struct Background {
    pub nebula_positions: Vec<[f32; 3]>,
    pub nebula_colors: Vec<[f32; 3]>,
    pub nebula_indices: Vec<u32>,
    pub star_positions: Vec<[f32; 3]>,
    pub star_colors: Vec<[f32; 3]>,
}

/// A small, fast, seeded RNG (splitmix64). Deterministic for reproducible
/// backgrounds; this output never feeds gameplay.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform f32 in [0, 1).
    fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }

    /// A uniformly distributed unit vector on the sphere.
    fn unit_vec(&mut self) -> Vec3 {
        let z = self.range(-1.0, 1.0);
        let t = self.range(0.0, std::f32::consts::TAU);
        let r = (1.0 - z * z).max(0.0).sqrt();
        Vec3::new(r * t.cos(), z, r * t.sin())
    }
}

/// A colored nebula lobe: a soft blob of color centered on a direction.
struct Lobe {
    dir: Vec3,
    color: Vec3,
    tightness: f32,
}

/// Generate the background for `seed`, on a sphere of `radius` world units.
pub fn generate_background(seed: u64, radius: f32) -> Background {
    let mut rng = Rng(seed ^ 0xA17C_3F19_22B5_77E1);

    // A handful of colored nebula lobes over a dark base. Colors are linear and
    // intentionally moderate (the sRGB target brightens them on write).
    let palette = [
        Vec3::new(0.10, 0.05, 0.22), // violet
        Vec3::new(0.04, 0.12, 0.20), // teal
        Vec3::new(0.18, 0.06, 0.14), // magenta
        Vec3::new(0.05, 0.08, 0.20), // blue
        Vec3::new(0.16, 0.10, 0.05), // amber
    ];
    let lobe_count = 7;
    let lobes: Vec<Lobe> = (0..lobe_count)
        .map(|_| Lobe {
            dir: rng.unit_vec(),
            color: palette[(rng.next_u64() as usize) % palette.len()] * rng.range(0.6, 1.4),
            tightness: rng.range(1.5, 5.0),
        })
        .collect();

    let base = Vec3::new(0.012, 0.014, 0.028);

    // UV sphere tessellation.
    let rings = 48usize; // latitude bands
    let sectors = 96usize; // longitude steps
    let mut nebula_positions = Vec::new();
    let mut nebula_colors = Vec::new();
    for i in 0..=rings {
        let v = i as f32 / rings as f32;
        let phi = v * std::f32::consts::PI; // 0..pi
        for j in 0..=sectors {
            let u = j as f32 / sectors as f32;
            let theta = u * std::f32::consts::TAU; // 0..2pi
            let dir = Vec3::new(
                phi.sin() * theta.cos(),
                phi.cos(),
                phi.sin() * theta.sin(),
            );
            let color = nebula_color(dir, base, &lobes);
            nebula_positions.push((dir * radius).to_array());
            nebula_colors.push(color.to_array());
        }
    }

    let row = sectors + 1;
    let mut nebula_indices = Vec::new();
    for i in 0..rings {
        for j in 0..sectors {
            let a = (i * row + j) as u32;
            let b = a + 1;
            let c = a + row as u32;
            let d = c + 1;
            // Two triangles; wound so we see them from inside the sphere.
            nebula_indices.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }

    // Star field: bright points just inside the nebula sphere.
    let star_count = 700;
    let mut star_positions = Vec::with_capacity(star_count);
    let mut star_colors = Vec::with_capacity(star_count);
    for _ in 0..star_count {
        let dir = rng.unit_vec();
        star_positions.push((dir * (radius * 0.97)).to_array());
        let b = rng.range(0.35, 1.0);
        // Mostly white, a few warm/cool tints.
        let tint = match (rng.next_u64() % 5) as u8 {
            0 => Vec3::new(1.0, 0.85, 0.7),
            1 => Vec3::new(0.75, 0.85, 1.0),
            _ => Vec3::new(1.0, 1.0, 1.0),
        };
        star_colors.push((tint * b).to_array());
    }

    Background {
        nebula_positions,
        nebula_colors,
        nebula_indices,
        star_positions,
        star_colors,
    }
}

fn nebula_color(dir: Vec3, base: Vec3, lobes: &[Lobe]) -> Vec3 {
    let mut c = base;
    // A gentle vertical gradient (a touch lighter "up").
    c += Vec3::new(0.006, 0.008, 0.014) * (dir.y * 0.5 + 0.5);
    for lobe in lobes {
        let d = dir.dot(lobe.dir).clamp(-1.0, 1.0);
        // Soft falloff from the lobe center (d near 1).
        let w = (-(1.0 - d) * lobe.tightness).exp();
        c += lobe.color * w;
    }
    c.clamp(Vec3::ZERO, Vec3::splat(1.0))
}
