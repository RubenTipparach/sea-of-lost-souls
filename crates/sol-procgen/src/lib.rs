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
    /// Per-star sprite size (clip-space fraction of half-height).
    pub star_sizes: Vec<f32>,
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

    // UV sphere tessellation. Nebula detail is carried by REAL per-vertex colors
    // (no shader noise), so higher frequency means more vertices: this density is
    // chosen to resolve the fine cloud field sampled in `nebula_color` without
    // aliasing. Bump these (and the noise frequency) together for finer clouds.
    let rings = 128usize; // latitude bands
    let sectors = 256usize; // longitude steps
    let mut nebula_positions = Vec::new();
    let mut nebula_colors = Vec::new();
    for i in 0..=rings {
        let v = i as f32 / rings as f32;
        let phi = v * std::f32::consts::PI; // 0..pi
        for j in 0..=sectors {
            let u = j as f32 / sectors as f32;
            let theta = u * std::f32::consts::TAU; // 0..2pi
            let dir = Vec3::new(phi.sin() * theta.cos(), phi.cos(), phi.sin() * theta.sin());
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
    let star_count = 900;
    let mut star_positions = Vec::with_capacity(star_count);
    let mut star_colors = Vec::with_capacity(star_count);
    let mut star_sizes = Vec::with_capacity(star_count);
    for _ in 0..star_count {
        let dir = rng.unit_vec();
        star_positions.push((dir * (radius * 0.97)).to_array());
        let b = rng.range(0.4, 1.0);
        // Mostly white, a few warm/cool tints.
        let tint = match (rng.next_u64() % 5) as u8 {
            0 => Vec3::new(1.0, 0.85, 0.7),
            1 => Vec3::new(0.75, 0.85, 1.0),
            _ => Vec3::new(1.0, 1.0, 1.0),
        };
        star_colors.push((tint * b).to_array());
        // SNES-style size variety: mostly tiny, some medium, a few bright.
        let roll = rng.unit();
        let size = if roll > 0.97 {
            rng.range(0.016, 0.026)
        } else if roll > 0.85 {
            rng.range(0.008, 0.013)
        } else {
            rng.range(0.0028, 0.006)
        };
        star_sizes.push(size);
    }

    Background {
        nebula_positions,
        nebula_colors,
        nebula_indices,
        star_positions,
        star_colors,
        star_sizes,
    }
}

/// A seeded salvage site: where an asteroid sits, how much it holds, and its
/// footprint radius. Positions/amounts are gameplay-affecting, so they are
/// deterministic from the seed (identical across peers); only the visual rock
/// mesh is elaborated freely by the renderer.
pub struct ResourceSite {
    pub pos: [f32; 3],
    pub amount: f32,
    pub radius: f32,
}

/// Generate `count` salvage sites in a rough belt around the origin, seeded from
/// `seed`. Deterministic: the same seed yields the same field on every peer, so
/// it is safe to drive gameplay (see `design.md` §7).
pub fn generate_resource_field(seed: u64, count: usize) -> Vec<ResourceSite> {
    let mut rng = Rng(seed ^ 0x51E5_F00D_1234_9ABC);
    (0..count)
        .map(|_| {
            let angle = rng.range(0.0, std::f32::consts::TAU);
            let dist = rng.range(45.0, 95.0);
            let y = rng.range(-6.0, 6.0);
            ResourceSite {
                pos: [dist * angle.cos(), y, dist * angle.sin()],
                amount: rng.range(300.0, 900.0),
                radius: rng.range(2.0, 4.0),
            }
        })
        .collect()
}

/// Per-vertex nebula color: a dark base + vertical gradient, the seeded lobes
/// broken up into filaments by a real noise field, and faint cloudiness
/// everywhere. ALL detail lives in this per-vertex value (the shader just
/// interpolates it), so finer clouds come from more vertices + higher noise
/// frequency, not from a fragment-shader effect.
fn nebula_color(dir: Vec3, base: Vec3, lobes: &[Lobe]) -> Vec3 {
    let mut c = base;
    // A gentle vertical gradient (a touch lighter "up").
    c += Vec3::new(0.006, 0.008, 0.014) * (dir.y * 0.5 + 0.5);

    // Cloud field: broad structure + finer filaments. Frequencies are kept under
    // the vertex Nyquist limit for the tessellation above so it doesn't alias.
    let broad = fbm3(dir * 1.7 + Vec3::splat(11.0));
    let fine = fbm3(dir * 4.5 + Vec3::splat(37.0));
    let cloud = (0.45 * broad + 0.55 * fine).clamp(0.0, 1.0);

    for lobe in lobes {
        let d = dir.dot(lobe.dir).clamp(-1.0, 1.0);
        // Soft falloff from the lobe center (d near 1), textured by the cloud
        // field so each lobe reads as wispy filaments rather than a smooth blob.
        let w = (-(1.0 - d) * lobe.tightness).exp();
        c += lobe.color * w * (0.2 + 1.6 * cloud);
    }

    // A little standalone nebulosity in the densest parts of the cloud field.
    c += Vec3::new(0.05, 0.05, 0.075) * (cloud * cloud * 0.5);
    c.clamp(Vec3::ZERO, Vec3::splat(1.0))
}

/// Hash an integer lattice point to [0, 1).
fn hash3(p: Vec3) -> f32 {
    let h = p.dot(Vec3::new(127.1, 311.7, 74.7));
    (h.sin() * 43758.547).fract().abs()
}

/// Trilinearly interpolated 3D value noise in [0, 1].
fn vnoise3(p: Vec3) -> f32 {
    let i = p.floor();
    let f = p - i;
    // Smoothstep weights.
    let u = f * f * (3.0 - 2.0 * f);
    let mix = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let corner = |dx: f32, dy: f32, dz: f32| hash3(i + Vec3::new(dx, dy, dz));
    let x00 = mix(corner(0.0, 0.0, 0.0), corner(1.0, 0.0, 0.0), u.x);
    let x10 = mix(corner(0.0, 1.0, 0.0), corner(1.0, 1.0, 0.0), u.x);
    let x01 = mix(corner(0.0, 0.0, 1.0), corner(1.0, 0.0, 1.0), u.x);
    let x11 = mix(corner(0.0, 1.0, 1.0), corner(1.0, 1.0, 1.0), u.x);
    let y0 = mix(x00, x10, u.y);
    let y1 = mix(x01, x11, u.y);
    mix(y0, y1, u.z)
}

/// Fractal value noise (3 octaves), normalized to [0, 1].
fn fbm3(p: Vec3) -> f32 {
    let (mut f, mut amp, mut norm) = (0.0, 0.5, 0.0);
    let mut q = p;
    for _ in 0..3 {
        f += amp * vnoise3(q);
        norm += amp;
        q *= 2.0;
        amp *= 0.5;
    }
    f / norm
}
