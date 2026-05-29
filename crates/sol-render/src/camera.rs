//! Orbit camera: produces the view+projection matrix uploaded to the GPU.

use glam::{Mat4, Vec3};

/// A Homeworld-style orbit camera defined by a focus point, spherical angles,
/// and a dolly distance. Yaw rotates around world up (+Y); pitch tilts up/down.
#[derive(Clone, Copy, Debug)]
pub struct OrbitCamera {
    /// Point the camera orbits and looks at.
    pub focus: Vec3,
    /// Horizontal angle (radians).
    pub yaw: f32,
    /// Vertical angle (radians), clamped away from the poles by the caller.
    pub pitch: f32,
    /// Distance from focus to eye (dolly / zoom).
    pub distance: f32,
    /// Vertical field of view (radians).
    pub fov_y: f32,
    pub z_near: f32,
    pub z_far: f32,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            focus: Vec3::ZERO,
            yaw: 0.6,
            pitch: 0.4,
            distance: 8.0,
            fov_y: 60f32.to_radians(),
            z_near: 0.05,
            // Far plane doubled (fix-it.md item 15) so the nebula + stars never
            // visibly clip as the camera flies around.
            z_far: 2000.0,
        }
    }
}

impl OrbitCamera {
    /// World-space eye position derived from the spherical coordinates.
    pub fn eye(&self) -> Vec3 {
        let cp = self.pitch.cos();
        let dir = Vec3::new(cp * self.yaw.sin(), self.pitch.sin(), cp * self.yaw.cos());
        self.focus + dir * self.distance
    }

    /// Combined view * projection matrix for the given surface aspect ratio.
    ///
    /// Uses wgpu/WebGPU clip space (z in `[0, 1]`), so we use
    /// `Mat4::perspective_rh` (not the GL `_gl` variant).
    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        let view = Mat4::look_at_rh(self.eye(), self.focus, Vec3::Y);
        let proj = Mat4::perspective_rh(self.fov_y, aspect.max(0.0001), self.z_near, self.z_far);
        proj * view
    }
}
