//! Procedural primitive geometry for the test interceptor hull.
//!
//! NOTE: ships are normally STATIC authored assets (see CLAUDE.md hard rule).
//! This generator is a build-time tool that produces the *authored* `model.glb`
//! committed to the repo - it is NOT runtime ship generation. It exists so the
//! scaffold has a real, valid ship asset to compile and render.

use glam::Vec3;

/// A simple CPU mesh: interleaved position+normal vertices and u32 indices.
#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
}

impl Mesh {
    /// Append a single flat-shaded quad (counter-clockwise winding when viewed
    /// from the side the normal points toward). Splits into two triangles.
    fn push_quad(&mut self, a: Vec3, b: Vec3, c: Vec3, d: Vec3) {
        let normal = (b - a).cross(c - a).normalize_or_zero();
        let base = self.positions.len() as u32;
        for v in [a, b, c, d] {
            self.positions.push(v.to_array());
            self.normals.push(normal.to_array());
        }
        // a,b,c and a,c,d
        self.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    /// Append an axis-aligned box spanning `min..max`, with outward normals.
    fn push_box(&mut self, min: Vec3, max: Vec3) {
        let p = |x: f32, y: f32, z: f32| Vec3::new(x, y, z);
        let (a, b) = (min, max);
        // +X face
        self.push_quad(
            p(b.x, a.y, a.z),
            p(b.x, a.y, b.z),
            p(b.x, b.y, b.z),
            p(b.x, b.y, a.z),
        );
        // -X face
        self.push_quad(
            p(a.x, a.y, b.z),
            p(a.x, a.y, a.z),
            p(a.x, b.y, a.z),
            p(a.x, b.y, b.z),
        );
        // +Y face
        self.push_quad(
            p(a.x, b.y, a.z),
            p(b.x, b.y, a.z),
            p(b.x, b.y, b.z),
            p(a.x, b.y, b.z),
        );
        // -Y face
        self.push_quad(
            p(a.x, a.y, b.z),
            p(b.x, a.y, b.z),
            p(b.x, a.y, a.z),
            p(a.x, a.y, a.z),
        );
        // +Z face
        self.push_quad(
            p(a.x, a.y, b.z),
            p(a.x, b.y, b.z),
            p(b.x, b.y, b.z),
            p(b.x, a.y, b.z),
        );
        // -Z face
        self.push_quad(
            p(b.x, a.y, a.z),
            p(b.x, b.y, a.z),
            p(a.x, b.y, a.z),
            p(a.x, a.y, a.z),
        );
    }

    /// Append a 4-sided pyramid: a square base at `z_base` (spanning
    /// `+/- half` in x/y) tapering to an apex at `+z` (the nose tip).
    fn push_nose(&mut self, half: f32, z_base: f32, z_tip: f32) {
        let apex = Vec3::new(0.0, 0.0, z_tip);
        let bl = Vec3::new(-half, -half, z_base);
        let br = Vec3::new(half, -half, z_base);
        let tr = Vec3::new(half, half, z_base);
        let tl = Vec3::new(-half, half, z_base);
        // Four triangular faces (wind so normals face outward/forward).
        for (p0, p1) in [(br, bl), (tr, br), (tl, tr), (bl, tl)] {
            let normal = (p1 - p0).cross(apex - p0).normalize_or_zero();
            let base = self.positions.len() as u32;
            for v in [p0, p1, apex] {
                self.positions.push(v.to_array());
                self.normals.push(normal.to_array());
            }
            self.indices.extend_from_slice(&[base, base + 1, base + 2]);
        }
    }
}

/// Build the interceptor hull: a stretched fuselage along +Z, a tapered nose,
/// and two swept wings. Origin is the ship center; +Z is forward.
pub fn build_interceptor_hull() -> Mesh {
    let mut mesh = Mesh::default();

    // Fuselage: elongated along Z, slim in X/Y.
    mesh.push_box(Vec3::new(-0.35, -0.3, -1.6), Vec3::new(0.35, 0.3, 1.4));

    // Nose cone forward of the fuselage.
    mesh.push_nose(0.32, 1.4, 2.4);

    // Port and starboard wings: thin slabs swept back, below center.
    // Starboard (+X)
    mesh.push_box(Vec3::new(0.35, -0.12, -0.9), Vec3::new(1.5, 0.02, 0.4));
    // Port (-X)
    mesh.push_box(Vec3::new(-1.5, -0.12, -0.9), Vec3::new(-0.35, 0.02, 0.4));

    // A small tail fin on top for silhouette.
    mesh.push_box(Vec3::new(-0.05, 0.3, -1.5), Vec3::new(0.05, 0.85, -0.9));

    mesh
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hull_is_nonempty_and_indexed() {
        let m = build_interceptor_hull();
        assert!(!m.positions.is_empty());
        assert_eq!(m.positions.len(), m.normals.len());
        assert_eq!(m.indices.len() % 3, 0);
        // Every index must be in range.
        let n = m.positions.len() as u32;
        assert!(m.indices.iter().all(|&i| i < n));
    }
}
