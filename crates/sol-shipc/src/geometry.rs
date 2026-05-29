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
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

impl Mesh {
    /// Append a single flat-shaded quad (counter-clockwise winding when viewed
    /// from the side the normal points toward). Splits into two triangles. Each
    /// face spans the full [0,1] UV range so the baked pixel-art tiles per panel.
    fn push_quad(&mut self, a: Vec3, b: Vec3, c: Vec3, d: Vec3) {
        // Outward normal and reversed (a,c,b / a,d,c) winding so the face renders
        // front-facing under the renderer's CCW-front, back-cull pipeline.
        let normal = (c - a).cross(b - a).normalize_or_zero();
        let base = self.positions.len() as u32;
        let uvs = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        for (v, uv) in [a, b, c, d].into_iter().zip(uvs) {
            self.positions.push(v.to_array());
            self.normals.push(normal.to_array());
            self.uvs.push(uv);
        }
        self.indices
            .extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);
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
            let normal = (apex - p0).cross(p1 - p0).normalize_or_zero();
            let base = self.positions.len() as u32;
            let uvs = [[0.0, 0.0], [1.0, 0.0], [0.5, 1.0]];
            for (v, uv) in [p0, p1, apex].into_iter().zip(uvs) {
                self.positions.push(v.to_array());
                self.normals.push(normal.to_array());
                self.uvs.push(uv);
            }
            self.indices.extend_from_slice(&[base, base + 2, base + 1]);
        }
    }

    /// Uniformly scale all positions so the farthest vertex sits `target` units
    /// from the origin. Keeps every authored hull a consistent model footprint
    /// (distinct shapes, similar size); the renderer applies the per-class
    /// display scale on top.
    fn normalize_radius(&mut self, target: f32) {
        let max = self
            .positions
            .iter()
            .map(|p| Vec3::from(*p).length())
            .fold(0.0_f32, f32::max);
        if max > 1e-4 {
            let s = target / max;
            for p in &mut self.positions {
                p[0] *= s;
                p[1] *= s;
                p[2] *= s;
            }
        }
    }
}

/// Distinct placeholder hull silhouettes, one per ship class. Authored offline
/// (committed GLBs), never generated at runtime.
#[derive(Clone, Copy, Debug)]
pub enum HullKind {
    Fighter,
    Bomber,
    Corvette,
    Resourcer,
    FrigateGeneral,
    FrigateMissile,
    CapitalDestroyer,
    Carrier,
}

/// Build a class's hull from boxes + a nose, then normalize its footprint so
/// the per-class display scale stays meaningful. Origin is the center; +Z fwd.
pub fn build_hull(kind: HullKind) -> Mesh {
    let mut m = Mesh::default();
    let v = |x: f32, y: f32, z: f32| Vec3::new(x, y, z);
    match kind {
        HullKind::Fighter => {
            m.push_box(v(-0.35, -0.3, -1.6), v(0.35, 0.3, 1.4));
            m.push_nose(0.32, 1.4, 2.4);
            m.push_box(v(0.35, -0.12, -0.9), v(1.5, 0.02, 0.4));
            m.push_box(v(-1.5, -0.12, -0.9), v(-0.35, 0.02, 0.4));
            m.push_box(v(-0.05, 0.3, -1.5), v(0.05, 0.85, -0.9));
        }
        HullKind::Bomber => {
            m.push_box(v(-0.55, -0.4, -1.5), v(0.55, 0.4, 1.2));
            m.push_nose(0.5, 1.2, 1.9);
            m.push_box(v(0.55, -0.2, -1.0), v(1.3, 0.1, 0.6));
            m.push_box(v(-1.3, -0.2, -1.0), v(-0.55, 0.1, 0.6));
            m.push_box(v(0.28, -0.55, -1.7), v(0.72, -0.05, -0.3));
            m.push_box(v(-0.72, -0.55, -1.7), v(-0.28, -0.05, -0.3));
        }
        HullKind::Corvette => {
            m.push_box(v(-0.4, -0.35, -2.2), v(0.4, 0.35, 1.8));
            m.push_nose(0.36, 1.8, 2.8);
            m.push_box(v(-0.06, 0.35, -1.8), v(0.06, 1.05, -0.9));
            m.push_box(v(0.4, -0.1, -0.6), v(0.9, 0.05, 0.5));
            m.push_box(v(-0.9, -0.1, -0.6), v(-0.4, 0.05, 0.5));
        }
        HullKind::Resourcer => {
            m.push_box(v(-0.7, -0.5, -1.6), v(0.7, 0.5, 1.0));
            m.push_box(v(0.4, -0.45, 1.0), v(0.72, 0.45, 2.3));
            m.push_box(v(-0.72, -0.45, 1.0), v(-0.4, 0.45, 2.3));
            m.push_box(v(-0.45, 0.5, -1.4), v(0.45, 0.95, -0.5));
        }
        HullKind::FrigateGeneral => {
            m.push_box(v(-0.6, -0.5, -2.6), v(0.6, 0.5, 2.0));
            m.push_nose(0.55, 2.0, 3.0);
            m.push_box(v(-0.08, 0.5, -2.2), v(0.08, 1.35, -0.9));
            m.push_box(v(0.6, -0.2, -1.2), v(1.1, 0.1, 0.7));
            m.push_box(v(-1.1, -0.2, -1.2), v(-0.6, 0.1, 0.7));
        }
        HullKind::FrigateMissile => {
            m.push_box(v(-0.6, -0.5, -2.6), v(0.6, 0.5, 2.0));
            m.push_nose(0.55, 2.0, 2.9);
            m.push_box(v(-0.5, 0.5, -1.9), v(0.5, 1.15, -0.1));
            m.push_box(v(0.6, -0.2, -1.0), v(1.0, 0.1, 0.5));
            m.push_box(v(-1.0, -0.2, -1.0), v(-0.6, 0.1, 0.5));
        }
        HullKind::CapitalDestroyer => {
            m.push_box(v(-0.9, -0.7, -3.2), v(0.9, 0.7, 2.4));
            m.push_nose(0.8, 2.4, 3.4);
            m.push_box(v(-0.12, 0.7, -2.6), v(0.12, 1.8, -1.2));
            m.push_box(v(-0.12, 0.7, -0.6), v(0.12, 1.5, 0.4));
            m.push_box(v(0.9, -0.3, -1.6), v(1.5, 0.2, 0.7));
            m.push_box(v(-1.5, -0.3, -1.6), v(-0.9, 0.2, 0.7));
        }
        HullKind::Carrier => {
            m.push_box(v(-1.6, -0.4, -3.0), v(1.6, 0.2, 3.0));
            m.push_box(v(0.85, 0.2, -2.4), v(1.4, 1.15, -0.7));
            m.push_box(v(-1.6, -0.75, -2.6), v(-0.85, -0.3, 1.2));
            m.push_box(v(0.85, -0.75, -2.6), v(1.6, -0.3, 1.2));
        }
    }
    m.normalize_radius(2.5);
    m
}

/// The test interceptor hull (the fighter silhouette), used by `gen-test-ship`.
pub fn build_interceptor_hull() -> Mesh {
    build_hull(HullKind::Fighter)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hull_is_nonempty_and_indexed() {
        let m = build_interceptor_hull();
        assert!(!m.positions.is_empty());
        assert_eq!(m.positions.len(), m.normals.len());
        assert_eq!(m.positions.len(), m.uvs.len());
        assert_eq!(m.indices.len() % 3, 0);
        // Every index must be in range.
        let n = m.positions.len() as u32;
        assert!(m.indices.iter().all(|&i| i < n));
    }
}
