//! `sol-app` - the entry point that wires winit + `sol-render` together and
//! drives the Homeworld-style orbit camera. Native opens a window; wasm grabs a
//! canvas (see [`start`]). The test interceptor mesh is loaded from its GLB
//! (with a code-generated placeholder fallback if loading fails).

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::Arc;

use glam::{Mat4, Vec3, Vec4};
use sol_render::{
    BgVertex, CpuMesh, GpuMesh, MeshInstance, OrbitCamera, OverlayVertex, RenderOutcome, Renderer,
    StarInstance, Vertex,
};
use sol_sim::{Command, EntityId, Patrol, ShipClass, Team, World};

/// Group-move arrangement applied by [`App::issue_move`]. Local UI state only
/// (never enters the sim): it just maps a click to per-ship slot targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Formation {
    /// Class-segregated "military parade": carrier centered, capitals trailing,
    /// frigates flanking either side, fighters/bombers on the wings. Falls back
    /// to a grid if the selection has no carrier (mothership) to anchor it.
    Parade,
    /// Compact square grid (every ship gets a distinct cell).
    Grid,
    /// Single line abreast, perpendicular to the move direction.
    Line,
}

impl Formation {
    /// Cycle order for the HUD button / `F` key.
    fn next(self) -> Self {
        match self {
            Formation::Parade => Formation::Grid,
            Formation::Grid => Formation::Line,
            Formation::Line => Formation::Parade,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Formation::Parade => "Parade",
            Formation::Grid => "Grid",
            Formation::Line => "Line",
        }
    }
}
use web_time::Instant;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, TouchPhase, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

/// Min/max orbit distance (dolly clamp).
const MIN_DISTANCE: f32 = 1.5;
const MAX_DISTANCE: f32 = 80.0;
/// Pitch clamp to avoid gimbal flip at the poles.
const PITCH_LIMIT: f32 = std::f32::consts::FRAC_PI_2 - 0.05;

/// Orbit-camera input controller (right-mouse-drag orbits; wheel dollies).
struct CameraController {
    orbiting: bool,
    last_cursor: Option<(f64, f64)>,
    orbit_speed: f32,
    zoom_speed: f32,
    /// Active touch points as (id, x, y); drives one-finger orbit + pinch zoom.
    touches: Vec<(u64, f64, f64)>,
    /// Distance between the first two touches on the previous pinch sample.
    last_pinch: Option<f64>,
}

impl Default for CameraController {
    fn default() -> Self {
        Self {
            orbiting: false,
            last_cursor: None,
            orbit_speed: 0.005,
            zoom_speed: 0.1,
            touches: Vec::new(),
            last_pinch: None,
        }
    }
}

impl CameraController {
    fn on_mouse_button(&mut self, button: MouseButton, state: ElementState) {
        if button == MouseButton::Right {
            self.orbiting = state == ElementState::Pressed;
            if !self.orbiting {
                self.last_cursor = None;
            }
        }
    }

    fn orbit_by(&self, camera: &mut OrbitCamera, dx: f32, dy: f32) {
        camera.yaw -= dx * self.orbit_speed;
        camera.pitch = (camera.pitch + dy * self.orbit_speed).clamp(-PITCH_LIMIT, PITCH_LIMIT);
    }

    fn on_cursor_moved(&mut self, camera: &mut OrbitCamera, x: f64, y: f64) {
        if self.orbiting {
            if let Some((px, py)) = self.last_cursor {
                self.orbit_by(camera, (x - px) as f32, (y - py) as f32);
            }
        }
        self.last_cursor = Some((x, y));
    }

    /// Touch input: one finger orbits (like an RMB drag), two fingers pinch to
    /// zoom. This is the mobile path for the camera; it shares `orbit_by` and
    /// the same distance clamp as the mouse path.
    fn on_touch(&mut self, camera: &mut OrbitCamera, phase: TouchPhase, id: u64, x: f64, y: f64) {
        match phase {
            TouchPhase::Started => {
                self.touches.push((id, x, y));
                self.last_pinch = None;
            }
            TouchPhase::Moved => {
                let prev = self.touches.iter_mut().find(|t| t.0 == id).map(|t| {
                    let p = (t.1, t.2);
                    t.1 = x;
                    t.2 = y;
                    p
                });
                if self.touches.len() == 1 {
                    if let Some((px, py)) = prev {
                        self.orbit_by(camera, (x - px) as f32, (y - py) as f32);
                    }
                } else if self.touches.len() >= 2 {
                    let a = self.touches[0];
                    let b = self.touches[1];
                    let dist = ((a.1 - b.1).powi(2) + (a.2 - b.2).powi(2)).sqrt();
                    if let Some(last) = self.last_pinch {
                        // Pinching apart (dist grows) dollies in.
                        if last > 1.0 {
                            let ratio = (dist / last) as f32;
                            if ratio > 0.0 {
                                camera.distance =
                                    (camera.distance / ratio).clamp(MIN_DISTANCE, MAX_DISTANCE);
                            }
                        }
                    }
                    self.last_pinch = Some(dist);
                }
            }
            TouchPhase::Ended | TouchPhase::Cancelled => {
                self.touches.retain(|t| t.0 != id);
                self.last_pinch = None;
            }
        }
    }

    fn on_scroll(&mut self, camera: &mut OrbitCamera, delta: MouseScrollDelta) {
        let scroll = match delta {
            MouseScrollDelta::LineDelta(_, y) => y,
            MouseScrollDelta::PixelDelta(p) => (p.y as f32) * 0.02,
        };
        // Scale dolly by distance for a consistent feel across zoom levels.
        let factor = 1.0 - scroll * self.zoom_speed;
        camera.distance = (camera.distance * factor).clamp(MIN_DISTANCE, MAX_DISTANCE);
    }
}

/// Graphics resources, created once the window/surface exists.
struct Graphics {
    renderer: Renderer,
    /// One GPU mesh per ship class (each class has a distinct authored hull).
    meshes: HashMap<ShipClass, GpuMesh>,
}

/// Async-init result slot, shared with the spawned setup future on web.
#[cfg(target_arch = "wasm32")]
type PendingGraphics = std::rc::Rc<std::cell::RefCell<Option<Graphics>>>;

/// Application state. Built lazily on `resumed` (the winit 0.30 lifecycle point
/// where a window/surface can be created on all platforms incl. web).
struct App {
    window: Option<Arc<Window>>,
    graphics: Option<Graphics>,
    camera: OrbitCamera,
    controller: CameraController,
    /// Deterministic simulation; stepped at a fixed rate, rendered interpolated.
    world: World,
    /// Wall-clock of the previous frame. App-side only; the sim never sees it.
    last_frame: Option<Instant>,
    /// Real time accumulated but not yet consumed by a fixed step.
    accumulator: f32,
    /// Locally-selected ships (UI state only; never enters the sim).
    selected: Vec<EntityId>,
    /// Active group-move arrangement (cycled with `F` / the HUD button).
    formation: Formation,
    /// In-progress band-select rectangle as (start, current) in physical pixels.
    /// Set while dragging (desktop LMB, or mobile with the box-select button
    /// held); drawn as a HUD rectangle and finalized on release.
    select_box: Option<((f64, f64), (f64, f64))>,
    /// Mobile: true while the "Box Select" button is held, so a one-finger drag
    /// draws a band box instead of orbiting the camera.
    box_select_armed: bool,
    /// Latest cursor position in physical pixels.
    cursor: (f64, f64),
    /// Left/right mouse press positions, for click-vs-drag discrimination.
    lmb_down: Option<(f64, f64)>,
    rmb_down: Option<(f64, f64)>,
    /// Touch tap tracking (a tap is a short, still, single-finger touch).
    touch_count: u32,
    tap_id: Option<u64>,
    tap_start: (f64, f64),
    tap_moved: bool,
    /// On web, adapter/device acquisition is async; the result lands here.
    #[cfg(target_arch = "wasm32")]
    pending: Option<PendingGraphics>,
}

impl App {
    fn new() -> Self {
        let mut world = World::new(0x5EA0_5005);
        spawn_demo_fleet(&mut world);
        Self {
            window: None,
            graphics: None,
            camera: OrbitCamera::default(),
            controller: CameraController::default(),
            world,
            last_frame: None,
            accumulator: 0.0,
            selected: Vec::new(),
            formation: Formation::Parade,
            select_box: None,
            box_select_armed: false,
            cursor: (0.0, 0.0),
            lmb_down: None,
            rmb_down: None,
            touch_count: 0,
            tap_id: None,
            tap_start: (0.0, 0.0),
            tap_moved: false,
            #[cfg(target_arch = "wasm32")]
            pending: None,
        }
    }
}

impl App {
    /// Native: build the renderer synchronously (pollster blocks on the async
    /// adapter/device request).
    #[cfg(not(target_arch = "wasm32"))]
    fn init_graphics(&mut self, window: Arc<Window>) {
        let size = window.inner_size();
        let (w, h) = (size.width.max(1), size.height.max(1));

        let mut renderer = match Renderer::new_blocking(Arc::clone(&window), w, h) {
            Ok(r) => r,
            Err(e) => {
                log::error!("failed to create renderer: {e:#}");
                return;
            }
        };
        let meshes = upload_class_meshes(&renderer);
        let (nebula, nebula_idx, stars, grid) = build_environment();
        renderer.set_environment(&nebula, &nebula_idx, &stars, &grid);

        self.camera.focus = Vec3::ZERO;
        self.camera.distance = 30.0;
        self.graphics = Some(Graphics { renderer, meshes });
        self.window = Some(window);
    }

    /// Web: spawn an async task to build the renderer; the result is dropped
    /// into a shared cell and picked up on the next redraw.
    #[cfg(target_arch = "wasm32")]
    fn init_graphics(&mut self, window: Arc<Window>) {
        self.camera.focus = Vec3::ZERO;
        self.camera.distance = 30.0;
        self.window = Some(Arc::clone(&window));

        let slot: PendingGraphics = std::rc::Rc::new(std::cell::RefCell::new(None));
        self.pending = Some(std::rc::Rc::clone(&slot));

        let (w, h) = web_drawable_size().unwrap_or_else(|| {
            let s = window.inner_size();
            (s.width.max(1), s.height.max(1))
        });
        wasm_bindgen_futures::spawn_local(async move {
            match Renderer::new(Arc::clone(&window), w, h).await {
                Ok(mut renderer) => {
                    let meshes = upload_class_meshes(&renderer);
                    let (nebula, nebula_idx, stars, grid) = build_environment();
                    renderer.set_environment(&nebula, &nebula_idx, &stars, &grid);
                    *slot.borrow_mut() = Some(Graphics { renderer, meshes });
                    window.request_redraw();
                }
                Err(e) => {
                    log::error!("failed to create renderer: {e:#}");
                    show_overlay_error(&format!("Renderer init failed: {e}"));
                }
            }
        });
    }

    /// On web, move a finished async-built renderer into place.
    #[cfg(target_arch = "wasm32")]
    fn take_pending(&mut self) {
        if self.graphics.is_none() {
            if let Some(slot) = &self.pending {
                if let Some(g) = slot.borrow_mut().take() {
                    self.graphics = Some(g);
                    hide_loading_overlay();
                }
            }
        }
    }

    fn redraw(&mut self) {
        #[cfg(target_arch = "wasm32")]
        self.take_pending();

        if self.graphics.is_none() {
            // Graphics not ready yet (web async init); keep polling.
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }

        // Apply queued UI commands (DOM buttons on web) at the frame boundary.
        #[cfg(target_arch = "wasm32")]
        self.drain_ui_queue();

        // Advance the fixed-step sim by however much real time has elapsed,
        // clamped so a backgrounded tab does not trigger a step spiral. The
        // accumulator and clock live here, never in the deterministic sim.
        let now = Instant::now();
        let frame_dt = match self.last_frame.replace(now) {
            Some(prev) => (now - prev).as_secs_f32().min(0.25),
            None => 0.0,
        };
        self.accumulator += frame_dt;
        let dt = sol_sim::TICK_DT;
        let mut steps = 0;
        while self.accumulator >= dt && steps < 8 {
            self.world.step(dt);
            self.accumulator -= dt;
            steps += 1;
        }
        let alpha = if dt > 0.0 {
            (self.accumulator / dt).clamp(0.0, 1.0)
        } else {
            0.0
        };

        // Window drawable size, for projecting world points to the HUD overlay.
        let (vw, vh) = match &self.window {
            Some(w) => {
                let s = w.inner_size();
                (s.width.max(1) as f32, s.height.max(1) as f32)
            }
            None => (1.0, 1.0),
        };

        // Interpolate each entity and group instances by class (one draw per
        // distinct hull mesh). Selected ships also emit selection gizmos (a y=0
        // ground circle + elevation pole + dashed move line) and a HUD health
        // bar, so position, depth, and orders read at a glance.
        let mut groups: HashMap<ShipClass, Vec<MeshInstance>> = HashMap::new();
        let mut gizmo_lines: Vec<BgVertex> = Vec::new();
        let mut overlay_tris: Vec<OverlayVertex> = Vec::new();
        let mut overlay_lines: Vec<OverlayVertex> = Vec::new();
        for e in &self.world.entities {
            let pos = e.prev_transform.pos.lerp(e.transform.pos, alpha);
            let rot = e.prev_transform.rot.slerp(e.transform.rot, alpha);
            let scale = Vec3::splat(class_scale(e.ship.class));
            let model = Mat4::from_scale_rotation_translation(scale, rot, pos);
            let selected = self.selected.contains(&e.id);
            let tint = if selected {
                [0.45, 1.0, 0.65, 1.0]
            } else {
                [1.0, 1.0, 1.0, 1.0]
            };
            groups
                .entry(e.ship.class)
                .or_default()
                .push(MeshInstance::with_tint(model, tint));

            if selected {
                let r = self.world.registry.get(e.ship.class).radius;
                let ground = Vec3::new(pos.x, 0.0, pos.z);
                push_circle(&mut gizmo_lines, ground, r * 1.4 + 0.6, [0.3, 1.0, 0.6], 28);
                push_line(&mut gizmo_lines, ground, pos, [0.2, 0.6, 0.4]);
                if let Some(target) = e.order {
                    push_dashes(&mut gizmo_lines, pos, target, [1.0, 0.75, 0.25], 1.2, 0.9);
                    push_circle(&mut gizmo_lines, target, 0.8, [1.0, 0.75, 0.25], 16);
                }
                let max_hull = self.world.registry.get(e.ship.class).max_hull;
                let frac = (e.hull / max_hull.max(1.0)).clamp(0.0, 1.0);
                if let Some((sx, sy)) = self.project_to_screen(pos + Vec3::Y * (r * 0.8 + 1.2)) {
                    push_health_bar(
                        &mut overlay_tris,
                        &mut overlay_lines,
                        sx,
                        sy,
                        frac,
                        (vw, vh),
                    );
                }
            }
        }

        // Band-select rectangle (desktop LMB drag or mobile box-select hold).
        if let Some((a, b)) = self.select_box {
            let rect = [a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1)];
            push_rect_px(&mut overlay_tris, rect, [0.3, 0.7, 1.0, 0.12], (vw, vh));
            push_rect_outline_px(&mut overlay_lines, rect, [0.55, 0.85, 1.0, 0.9], (vw, vh));
        }

        let gfx = self.graphics.as_mut().unwrap();
        gfx.renderer
            .set_overlays(&gizmo_lines, &overlay_tris, &overlay_lines);
        let render_groups: Vec<(&GpuMesh, &[MeshInstance])> = groups
            .iter()
            .filter_map(|(class, insts)| gfx.meshes.get(class).map(|m| (m, insts.as_slice())))
            .collect();
        match gfx.renderer.render_groups(&self.camera, &render_groups) {
            RenderOutcome::NeedsReconfigure => {
                if let Some(window) = &self.window {
                    let size = window.inner_size();
                    gfx.renderer.resize(size.width, size.height);
                }
            }
            RenderOutcome::Presented | RenderOutcome::Skipped => {}
        }
        // Keep animating (the sim advances continuously).
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

/// Pixels of cursor/touch travel still treated as a click (vs a drag).
const CLICK_SLOP: f64 = 6.0;
/// Pixels of touch travel still treated as a tap.
const TAP_SLOP: f64 = 14.0;

fn dist2(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dy) = (a.0 - b.0, a.1 - b.1);
    dx * dx + dy * dy
}

/// Grid of formation slots on the move plane, centered on `center`, so a group
/// move never sends every ship to one point (they would stack and fight).
fn formation_slots(center: Vec3, n: usize, spacing: f32) -> Vec<Vec3> {
    if n == 0 {
        return Vec::new();
    }
    let cols = (n as f32).sqrt().ceil() as usize;
    let rows = n.div_ceil(cols);
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let (c, r) = (i % cols, i / cols);
        let ox = (c as f32 - (cols as f32 - 1.0) * 0.5) * spacing;
        let oz = (r as f32 - (rows as f32 - 1.0) * 0.5) * spacing;
        out.push(center + Vec3::new(ox, 0.0, oz));
    }
    out
}

/// Single line abreast, centered on `center`, perpendicular to `forward`.
fn line_slots(center: Vec3, forward: Vec3, n: usize, spacing: f32) -> Vec<Vec3> {
    if n == 0 {
        return Vec::new();
    }
    let right = formation_right(forward);
    (0..n)
        .map(|i| center + right * ((i as f32 - (n as f32 - 1.0) * 0.5) * spacing))
        .collect()
}

/// The rightward formation axis for a (XZ-plane) forward direction.
fn formation_right(forward: Vec3) -> Vec3 {
    let f = Vec3::new(forward.x, 0.0, forward.z).normalize_or_zero();
    let f = if f.length_squared() < 1e-6 {
        Vec3::Z
    } else {
        f
    };
    Vec3::new(f.z, 0.0, -f.x)
}

/// Alternate a paired class between right (+) and left (-) lanes of magnitude
/// `mag`, so each such class fills both flanks evenly. State is keyed by `mag`.
fn paired_column(sides: &mut HashMap<i32, bool>, mag: i32) -> i32 {
    let go_left = sides.entry(mag).or_insert(false);
    let col = if *go_left { -mag } else { mag };
    *go_left = !*go_left;
    col
}

/// Front-to-back ordering within a parade column (capitals lead, resourcers
/// trail). Lower ranks sit nearer the front.
fn class_depth_rank(class: ShipClass) -> u8 {
    match class {
        ShipClass::Carrier => 0,
        ShipClass::CapitalDestroyer => 1,
        ShipClass::FrigateGeneral => 2,
        ShipClass::FrigateMissile => 3,
        ShipClass::Corvette => 4,
        ShipClass::Bomber => 5,
        ShipClass::Fighter => 6,
        ShipClass::Resourcer => 9,
    }
}

// --- gizmo / HUD overlay geometry -----------------------------------------

/// World-space line segment (two `BgVertex`, drawn with the line pipeline).
fn push_line(v: &mut Vec<BgVertex>, a: Vec3, b: Vec3, color: [f32; 3]) {
    v.push(BgVertex::new(a.to_array(), color));
    v.push(BgVertex::new(b.to_array(), color));
}

/// World-space ring on the y=0 plane (XZ), as `segments` line segments.
fn push_circle(v: &mut Vec<BgVertex>, center: Vec3, radius: f32, color: [f32; 3], segments: usize) {
    let mut prev = center + Vec3::new(radius, 0.0, 0.0);
    for k in 1..=segments {
        let a = k as f32 / segments as f32 * std::f32::consts::TAU;
        let p = center + Vec3::new(radius * a.cos(), 0.0, radius * a.sin());
        push_line(v, prev, p, color);
        prev = p;
    }
}

/// World-space dashed segment from `a` to `b` (dash length + gap in units).
fn push_dashes(v: &mut Vec<BgVertex>, a: Vec3, b: Vec3, color: [f32; 3], dash: f32, gap: f32) {
    let total = b - a;
    let len = total.length();
    if len < 1e-3 {
        return;
    }
    let dir = total / len;
    let step = (dash + gap).max(0.01);
    let mut t = 0.0;
    while t < len {
        let seg_end = (t + dash).min(len);
        push_line(v, a + dir * t, a + dir * seg_end, color);
        t += step;
    }
}

/// Convert a physical-pixel point to clip-space NDC (origin top-left, y down).
fn px_to_ndc(px: f32, py: f32, w: f32, h: f32) -> [f32; 2] {
    [2.0 * px / w - 1.0, 1.0 - 2.0 * py / h]
}

/// Filled screen-space rectangle (two triangles). `rect` is `[x0, y0, x1, y1]`
/// in physical pixels; `view` is the drawable `(width, height)`.
fn push_rect_px(v: &mut Vec<OverlayVertex>, rect: [f64; 4], color: [f32; 4], view: (f32, f32)) {
    let (w, h) = view;
    let [x0, y0, x1, y1] = rect.map(|c| c as f32);
    let tl = px_to_ndc(x0, y0, w, h);
    let tr = px_to_ndc(x1, y0, w, h);
    let br = px_to_ndc(x1, y1, w, h);
    let bl = px_to_ndc(x0, y1, w, h);
    for p in [tl, tr, br, tl, br, bl] {
        v.push(OverlayVertex::new(p, color));
    }
}

/// Outline of a screen-space rectangle (four line segments). See [`push_rect_px`].
fn push_rect_outline_px(
    v: &mut Vec<OverlayVertex>,
    rect: [f64; 4],
    color: [f32; 4],
    view: (f32, f32),
) {
    let (w, h) = view;
    let [x0, y0, x1, y1] = rect.map(|c| c as f32);
    let tl = px_to_ndc(x0, y0, w, h);
    let tr = px_to_ndc(x1, y0, w, h);
    let br = px_to_ndc(x1, y1, w, h);
    let bl = px_to_ndc(x0, y1, w, h);
    for (a, b) in [(tl, tr), (tr, br), (br, bl), (bl, tl)] {
        v.push(OverlayVertex::new(a, color));
        v.push(OverlayVertex::new(b, color));
    }
}

/// A health bar centered at screen point `(sx, sy)`: a dark backing, a fill of
/// width proportional to `frac` (green full -> red empty), and a faint outline.
fn push_health_bar(
    tris: &mut Vec<OverlayVertex>,
    lines: &mut Vec<OverlayVertex>,
    sx: f64,
    sy: f64,
    frac: f32,
    view: (f32, f32),
) {
    const HALF_W: f64 = 22.0;
    const HALF_H: f64 = 3.5;
    let (x0, y0, x1, y1) = (sx - HALF_W, sy - HALF_H, sx + HALF_W, sy + HALF_H);
    push_rect_px(tris, [x0, y0, x1, y1], [0.0, 0.0, 0.0, 0.55], view);
    let fill_x1 = x0 + 1.0 + (x1 - x0 - 2.0) * frac as f64;
    let fill_color = [1.0 - frac, 0.25 + 0.7 * frac, 0.18, 0.95];
    push_rect_px(
        tris,
        [x0 + 1.0, y0 + 1.0, fill_x1, y1 - 1.0],
        fill_color,
        view,
    );
    push_rect_outline_px(lines, [x0, y0, x1, y1], [0.7, 0.8, 0.92, 0.7], view);
}

impl App {
    /// Build a world-space ray from a screen pixel (selection + move picking).
    fn screen_ray(&self, sx: f64, sy: f64) -> Option<(Vec3, Vec3)> {
        let size = self.window.as_ref()?.inner_size();
        let (w, h) = (size.width.max(1) as f32, size.height.max(1) as f32);
        let ndc_x = 2.0 * sx as f32 / w - 1.0;
        let ndc_y = 1.0 - 2.0 * sy as f32 / h;
        let inv = self.camera.view_proj(w / h).inverse();
        let near = inv * Vec4::new(ndc_x, ndc_y, 0.0, 1.0);
        let far = inv * Vec4::new(ndc_x, ndc_y, 1.0, 1.0);
        if near.w.abs() < 1e-6 || far.w.abs() < 1e-6 {
            return None;
        }
        let near = near.truncate() / near.w;
        let far = far.truncate() / far.w;
        Some((near, (far - near).normalize_or_zero()))
    }

    /// Nearest ship whose bounding sphere the screen ray hits.
    fn pick(&self, sx: f64, sy: f64) -> Option<EntityId> {
        let (origin, dir) = self.screen_ray(sx, sy)?;
        let mut best: Option<(f32, EntityId)> = None;
        for e in &self.world.entities {
            let center = e.transform.pos;
            let r = self.world.registry.get(e.ship.class).radius;
            let t = (center - origin).dot(dir);
            if t <= 0.0 {
                continue;
            }
            let closest = origin + dir * t;
            if (closest - center).length() <= r && best.is_none_or(|(bt, _)| t < bt) {
                best = Some((t, e.id));
            }
        }
        best.map(|(_, id)| id)
    }

    /// Intersect the screen ray with the horizontal plane at `plane_y`.
    fn ground_point(&self, sx: f64, sy: f64, plane_y: f32) -> Option<Vec3> {
        let (origin, dir) = self.screen_ray(sx, sy)?;
        if dir.y.abs() < 1e-5 {
            return None;
        }
        let t = (plane_y - origin.y) / dir.y;
        (t > 0.0).then_some(origin + dir * t)
    }

    /// Project a world point to physical-pixel screen coords. Returns `None`
    /// when the point is behind the camera (so HUD/band-box code skips it
    /// rather than wrapping it onto the screen).
    fn project_to_screen(&self, world: Vec3) -> Option<(f64, f64)> {
        let size = self.window.as_ref()?.inner_size();
        let (w, h) = (size.width.max(1) as f32, size.height.max(1) as f32);
        let clip = self.camera.view_proj(w / h) * world.extend(1.0);
        if clip.w <= 1e-6 {
            return None;
        }
        let ndc = clip.truncate() / clip.w;
        let px = (ndc.x * 0.5 + 0.5) * w;
        let py = (1.0 - (ndc.y * 0.5 + 0.5)) * h;
        Some((px as f64, py as f64))
    }

    /// XZ centroid of the current selection (the move-direction anchor).
    fn selection_centroid(&self) -> Vec3 {
        let mut sum = Vec3::ZERO;
        let mut n = 0.0;
        for id in &self.selected {
            if let Some(e) = self.world.entity(*id) {
                sum += e.transform.pos;
                n += 1.0;
            }
        }
        if n > 0.0 {
            sum / n
        } else {
            Vec3::ZERO
        }
    }

    fn selection_centroid_y(&self) -> f32 {
        let mut sum = 0.0;
        let mut n = 0.0;
        for id in &self.selected {
            if let Some(e) = self.world.entity(*id) {
                sum += e.transform.pos.y;
                n += 1.0;
            }
        }
        if n > 0.0 {
            sum / n
        } else {
            0.0
        }
    }

    /// Left click / tap on a ship selects it; on empty space, clears.
    fn on_select_click(&mut self, sx: f64, sy: f64) {
        match self.pick(sx, sy) {
            Some(id) => self.selected = vec![id],
            None => self.selected.clear(),
        }
    }

    /// Right click / mobile move: order the selection into a formation here.
    fn on_move_click(&mut self, sx: f64, sy: f64) {
        if self.selected.is_empty() {
            return;
        }
        let plane_y = self.selection_centroid_y();
        if let Some(center) = self.ground_point(sx, sy, plane_y) {
            self.issue_move(center);
        }
    }

    /// Mobile tap: select a tapped ship, else move the current selection here.
    fn on_tap(&mut self, sx: f64, sy: f64) {
        if let Some(id) = self.pick(sx, sy) {
            self.selected = vec![id];
        } else if !self.selected.is_empty() {
            self.on_move_click(sx, sy);
        }
    }

    fn issue_move(&mut self, center: Vec3) {
        let ids = self.selected.clone();
        if ids.is_empty() {
            return;
        }
        let max_r = ids
            .iter()
            .filter_map(|id| self.world.entity(*id))
            .map(|e| self.world.registry.get(e.ship.class).radius)
            .fold(1.0_f32, f32::max);
        let spacing = max_r * 2.5 + 1.0;
        let forward = self.formation_forward(center);
        let assignments: Vec<(EntityId, Vec3)> = match self.formation {
            Formation::Parade if self.selection_has_carrier() => {
                self.parade_slots(&ids, center, forward, max_r)
            }
            Formation::Line => ids
                .iter()
                .copied()
                .zip(line_slots(center, forward, ids.len(), spacing))
                .collect(),
            // Grid, or Parade without a mothership to anchor it.
            _ => ids
                .iter()
                .copied()
                .zip(formation_slots(center, ids.len(), spacing))
                .collect(),
        };
        for (id, slot) in assignments {
            self.world.enqueue(Command::MoveTo {
                entity: id,
                target: slot,
            });
        }
    }

    fn selection_has_carrier(&self) -> bool {
        self.selected.iter().any(|id| {
            self.world
                .entity(*id)
                .is_some_and(|e| e.ship.class == ShipClass::Carrier)
        })
    }

    /// Move direction for formations: from the selection centroid toward the
    /// clicked center (XZ). Falls back to +Z when clicking near the centroid.
    fn formation_forward(&self, center: Vec3) -> Vec3 {
        let d = center - self.selection_centroid();
        let d = Vec3::new(d.x, 0.0, d.z);
        if d.length_squared() > 1e-3 {
            d.normalize()
        } else {
            Vec3::Z
        }
    }

    /// "Military parade": class-segregated columns oriented to `forward`. The
    /// carrier holds front-center; capitals trail it, frigates take the inner
    /// flanks, corvettes the next lanes, fighters/bombers the wings, and
    /// resourcers the rear. Each (column, depth) cell is unique so ships never
    /// stack (the sim's separation then only fine-tunes spacing).
    fn parade_slots(
        &self,
        ids: &[EntityId],
        center: Vec3,
        forward: Vec3,
        max_r: f32,
    ) -> Vec<(EntityId, Vec3)> {
        let right = formation_right(forward);
        let col_space = max_r * 2.5 + 2.0;
        let row_space = max_r * 2.2 + 2.0;

        // Phase 1: assign each ship a signed column from its class, splitting
        // paired-lane classes evenly left/right in selection order.
        let mut sides = HashMap::<i32, bool>::new();
        let mut by_col: std::collections::BTreeMap<i32, Vec<(EntityId, ShipClass)>> =
            std::collections::BTreeMap::new();
        for &id in ids {
            let Some(e) = self.world.entity(id) else {
                continue;
            };
            let class = e.ship.class;
            let col = match class {
                ShipClass::Carrier | ShipClass::CapitalDestroyer | ShipClass::Resourcer => 0,
                ShipClass::FrigateGeneral | ShipClass::FrigateMissile => {
                    paired_column(&mut sides, 1)
                }
                ShipClass::Corvette => paired_column(&mut sides, 2),
                ShipClass::Fighter | ShipClass::Bomber => paired_column(&mut sides, 3),
            };
            by_col.entry(col).or_default().push((id, class));
        }

        // Phase 2: within each column, order by class (capitals front) and stack
        // front-to-back so no two ships share a cell.
        let mut out = Vec::with_capacity(ids.len());
        for (col, mut ships) in by_col {
            ships.sort_by_key(|(_, c)| class_depth_rank(*c));
            for (depth, (id, _)) in ships.into_iter().enumerate() {
                let pos = center + right * (col as f32 * col_space)
                    - forward * (depth as f32 * row_space);
                out.push((id, pos));
            }
        }
        out
    }

    /// Replace the selection with all player ships whose projected center lies
    /// inside the screen rectangle (physical pixels).
    fn band_select(&mut self, a: (f64, f64), b: (f64, f64)) {
        let (minx, maxx) = (a.0.min(b.0), a.0.max(b.0));
        let (miny, maxy) = (a.1.min(b.1), a.1.max(b.1));
        let mut hits = Vec::new();
        for e in &self.world.entities {
            if e.ship.team != Team::Player {
                continue;
            }
            if let Some((px, py)) = self.project_to_screen(e.transform.pos) {
                if px >= minx && px <= maxx && py >= miny && py <= maxy {
                    hits.push(e.id);
                }
            }
        }
        self.selected = hits;
    }

    /// Finalize a band box: a real drag selects the enclosed ships; a tiny box
    /// (a tap/click) falls back to single-pick select.
    fn finalize_box(&mut self, a: (f64, f64), b: (f64, f64)) {
        if dist2(a, b) >= CLICK_SLOP * CLICK_SLOP {
            self.band_select(a, b);
        } else {
            self.on_select_click(b.0, b.1);
        }
    }

    fn select_all_player(&mut self) {
        self.selected = self
            .world
            .entities
            .iter()
            .filter(|e| e.ship.team == Team::Player)
            .map(|e| e.id)
            .collect();
    }

    fn stop_selected(&mut self) {
        for id in self.selected.clone() {
            self.world.enqueue(Command::Stop { entity: id });
        }
    }

    /// Tap detection for touch: returns the position on a clean, still,
    /// single-finger tap, so one-finger drags still orbit and pinches zoom.
    fn on_touch_tap(&mut self, phase: TouchPhase, id: u64, x: f64, y: f64) -> Option<(f64, f64)> {
        match phase {
            TouchPhase::Started => {
                self.touch_count += 1;
                if self.touch_count == 1 {
                    self.tap_id = Some(id);
                    self.tap_start = (x, y);
                    self.tap_moved = false;
                } else {
                    self.tap_moved = true; // a second finger is a pinch, not a tap
                }
                None
            }
            TouchPhase::Moved => {
                if self.tap_id == Some(id) && dist2(self.tap_start, (x, y)) > TAP_SLOP * TAP_SLOP {
                    self.tap_moved = true;
                }
                None
            }
            TouchPhase::Ended | TouchPhase::Cancelled => {
                self.touch_count = self.touch_count.saturating_sub(1);
                let tap = self.tap_id == Some(id) && !self.tap_moved && phase == TouchPhase::Ended;
                if self.tap_id == Some(id) {
                    self.tap_id = None;
                }
                tap.then_some((x, y))
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn drain_ui_queue(&mut self) {
        let cmds = UI_QUEUE.with(|q| std::mem::take(&mut *q.borrow_mut()));
        for cmd in cmds {
            match cmd {
                UiCmd::SelectAll => self.select_all_player(),
                UiCmd::Stop => self.stop_selected(),
                UiCmd::Clear => self.selected.clear(),
                UiCmd::CycleFormation => {
                    self.formation = self.formation.next();
                    set_formation_label(self.formation.label());
                }
                UiCmd::BoxSelectArm(on) => self.box_select_armed = on,
            }
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes().with_title("Sea of Lost Souls");
        // On web, ask winit to append the rendering canvas to the document body.
        #[cfg(target_arch = "wasm32")]
        let attrs = {
            use winit::platform::web::WindowAttributesExtWebSys;
            attrs.with_append(true)
        };
        match event_loop.create_window(attrs) {
            Ok(window) => self.init_graphics(Arc::new(window)),
            Err(e) => log::error!("failed to create window: {e:#}"),
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(gfx) = self.graphics.as_mut() {
                    gfx.renderer.resize(size.width, size.height);
                }
            }
            WindowEvent::MouseInput { button, state, .. } => {
                let pos = self.cursor;
                match (button, state) {
                    (MouseButton::Left, ElementState::Pressed) => {
                        self.lmb_down = Some(pos);
                        self.select_box = None;
                    }
                    (MouseButton::Left, ElementState::Released) => {
                        let started = self.lmb_down.take();
                        let box_drag = self.select_box.take();
                        if let Some(p0) = started {
                            match box_drag {
                                // A drag past the click slop is a band selection.
                                Some(_) => self.band_select(p0, pos),
                                None => {
                                    if dist2(p0, pos) < CLICK_SLOP * CLICK_SLOP {
                                        self.on_select_click(pos.0, pos.1);
                                    }
                                }
                            }
                        }
                    }
                    (MouseButton::Right, ElementState::Pressed) => {
                        self.rmb_down = Some(pos);
                        self.controller.on_mouse_button(button, state);
                    }
                    (MouseButton::Right, ElementState::Released) => {
                        self.controller.on_mouse_button(button, state);
                        if let Some(p0) = self.rmb_down.take() {
                            if dist2(p0, pos) < CLICK_SLOP * CLICK_SLOP {
                                self.on_move_click(pos.0, pos.1);
                            }
                        }
                    }
                    _ => {}
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x, position.y);
                // An LMB drag past the click slop starts/updates a band box.
                if let Some(p0) = self.lmb_down {
                    if dist2(p0, self.cursor) >= CLICK_SLOP * CLICK_SLOP {
                        self.select_box = Some((p0, self.cursor));
                    }
                }
                self.controller
                    .on_cursor_moved(&mut self.camera, position.x, position.y);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                self.controller.on_scroll(&mut self.camera, delta);
            }
            WindowEvent::Touch(t) => {
                let (tx, ty) = (t.location.x, t.location.y);
                if self.box_select_armed {
                    // Box-select button held: a one-finger drag draws the band
                    // box (no orbit/tap) and selects the enclosed ships.
                    match t.phase {
                        TouchPhase::Started => self.select_box = Some(((tx, ty), (tx, ty))),
                        TouchPhase::Moved => {
                            if let Some((s, _)) = self.select_box {
                                self.select_box = Some((s, (tx, ty)));
                            }
                        }
                        TouchPhase::Ended => {
                            if let Some((a, b)) = self.select_box.take() {
                                self.finalize_box(a, b);
                            }
                        }
                        TouchPhase::Cancelled => self.select_box = None,
                    }
                } else {
                    if let Some((px, py)) = self.on_touch_tap(t.phase, t.id, tx, ty) {
                        self.on_tap(px, py);
                    }
                    self.controller
                        .on_touch(&mut self.camera, t.phase, t.id, tx, ty);
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                use winit::keyboard::{Key, NamedKey};
                if event.state == ElementState::Pressed {
                    match event.logical_key.as_ref() {
                        Key::Character("a") | Key::Character("A") => self.select_all_player(),
                        Key::Character("s") | Key::Character("S") => self.stop_selected(),
                        // `C` cycles formations; `F` is reserved for focus-on-
                        // selection in the design's binding table (design.md §9.5).
                        Key::Character("c") | Key::Character("C") => {
                            self.formation = self.formation.next();
                            log::info!("formation: {}", self.formation.label());
                        }
                        Key::Named(NamedKey::Escape) => self.selected.clear(),
                        _ => {}
                    }
                }
            }
            WindowEvent::RedrawRequested => self.redraw(),
            _ => {}
        }
    }
}

/// Run the app: build the event loop and drive it. Shared by native `main` and
/// the wasm `start` entry point.
pub fn run() {
    let event_loop = EventLoop::new().expect("create event loop");
    // Continuous redraw so the orbit camera feels responsive.
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App::new();
    event_loop.run_app(&mut app).expect("run event loop");
}

/// Spawn a small starting fleet so the fixed-step sim and interpolation have
/// something to drive. All ships share the placeholder mesh for now (per-class
/// meshes land in Stage 3/4); only their positions and classes differ.
/// Per-class display scale (visual only; every ship shares the interceptor-
/// sized base hull for now). Fighters read small, capital ships large, until
/// each class gets its own authored hull.
fn class_scale(class: ShipClass) -> f32 {
    match class {
        ShipClass::Fighter => 0.4,
        ShipClass::Bomber => 0.6,
        ShipClass::Corvette => 0.9,
        ShipClass::Resourcer => 1.0,
        ShipClass::FrigateGeneral => 1.7,
        ShipClass::FrigateMissile => 1.8,
        ShipClass::CapitalDestroyer => 2.4,
        ShipClass::Carrier => 3.2,
    }
}

fn spawn_demo_fleet(world: &mut World) {
    // Stationary core: the carrier and a few escorts holding formation.
    world.spawn_class(ShipClass::Carrier, Team::Player, Vec3::ZERO);
    let core = [
        (ShipClass::FrigateGeneral, Vec3::new(-9.0, 0.0, 7.0)),
        (ShipClass::Corvette, Vec3::new(9.0, 0.0, 7.0)),
        (ShipClass::Resourcer, Vec3::new(0.0, 0.0, 11.0)),
        (ShipClass::Bomber, Vec3::new(0.0, -2.5, -9.0)),
    ];
    for (class, pos) in core {
        world.spawn_class(class, Team::Player, pos);
    }
    // A flight of fighters patrolling a circle around the fleet.
    const PATROL: u32 = 5;
    for k in 0..PATROL {
        let angle = (k as f32 / PATROL as f32) * std::f32::consts::TAU;
        world.spawn_patrol(
            ShipClass::Fighter,
            Team::Player,
            Patrol {
                center: Vec3::ZERO,
                radius: 20.0,
                height: 2.0,
                angular_speed: 0.35,
                angle,
            },
        );
    }
}

/// Upload a GPU mesh for every ship class (each has a distinct authored hull).
fn upload_class_meshes(renderer: &Renderer) -> HashMap<ShipClass, GpuMesh> {
    let mut meshes = HashMap::new();
    for &class in &ShipClass::ALL {
        let cpu = load_class_mesh(class);
        meshes.insert(class, renderer.upload_mesh(&cpu, class.id()));
    }
    meshes
}

/// Build the background environment: a seeded nebula sphere + star field from
/// `sol-procgen`, plus a y=0 reference grid that sells movement and scale.
fn build_environment() -> (Vec<BgVertex>, Vec<u32>, Vec<StarInstance>, Vec<BgVertex>) {
    const RADIUS: f32 = 800.0;
    let bg = sol_procgen::generate_background(0x5EA0_5005, RADIUS);
    let nebula: Vec<BgVertex> = bg
        .nebula_positions
        .iter()
        .zip(&bg.nebula_colors)
        .map(|(p, c)| BgVertex::new(*p, *c))
        .collect();
    let mut stars: Vec<StarInstance> = bg
        .star_positions
        .iter()
        .zip(&bg.star_colors)
        .zip(&bg.star_sizes)
        .map(|((p, c), s)| StarInstance::new(*p, *c, *s))
        .collect();
    // A bright sun in the key-light direction anchors the scene lighting.
    let sun_dir = Vec3::from(sol_render::LIGHT_DIR).normalize();
    stars.push(StarInstance::new(
        (sun_dir * (RADIUS * 0.95)).to_array(),
        [1.0, 0.95, 0.8],
        0.07,
    ));
    (nebula, bg.nebula_indices, stars, build_grid())
}

/// A grid of dim dots on the y=0 plane, fading with distance, as a spatial
/// reference for movement and speed.
fn build_grid() -> Vec<BgVertex> {
    const N: i32 = 25;
    const SPACING: f32 = 6.0;
    let half = (N - 1) as f32 * 0.5 * SPACING;
    let mut pts = Vec::with_capacity((N * N) as usize);
    for i in 0..N {
        for j in 0..N {
            let x = i as f32 * SPACING - half;
            let z = j as f32 * SPACING - half;
            let d = (x * x + z * z).sqrt();
            let fade = (1.0 - d / (half * 1.15)).clamp(0.0, 1.0);
            let c = 0.05 + 0.12 * fade;
            pts.push(BgVertex::new([x, 0.0, z], [c * 0.55, c * 0.7, c]));
        }
    }
    pts
}

/// Load a class's authored hull GLB (on-disk on native, embedded on web),
/// falling back to a code-generated placeholder so the app always runs.
fn load_class_mesh(class: ShipClass) -> CpuMesh {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let path = format!("assets/ships/{}/model.glb", class.id());
        match CpuMesh::from_gltf_file(&path) {
            Ok(m) => return m,
            Err(e) => log::warn!("loading {path} failed: {e:#}; using placeholder"),
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        match CpuMesh::from_gltf_slice(class_glb_bytes(class)) {
            Ok(m) => return m,
            Err(e) => log::warn!("embedded GLB for {} failed: {e:#}", class.id()),
        }
    }
    placeholder_ship_mesh()
}

/// The committed per-class GLBs, embedded for the web build (the compiled-asset
/// path is git-ignored, so we embed the authored model.glb directly).
#[cfg(target_arch = "wasm32")]
fn class_glb_bytes(class: ShipClass) -> &'static [u8] {
    match class {
        ShipClass::Fighter => include_bytes!("../../../assets/ships/fighter/model.glb"),
        ShipClass::Bomber => include_bytes!("../../../assets/ships/bomber/model.glb"),
        ShipClass::Corvette => include_bytes!("../../../assets/ships/corvette/model.glb"),
        ShipClass::Resourcer => include_bytes!("../../../assets/ships/resourcer/model.glb"),
        ShipClass::FrigateGeneral => {
            include_bytes!("../../../assets/ships/frigate_general/model.glb")
        }
        ShipClass::FrigateMissile => {
            include_bytes!("../../../assets/ships/frigate_missile/model.glb")
        }
        ShipClass::CapitalDestroyer => {
            include_bytes!("../../../assets/ships/capital_destroyer/model.glb")
        }
        ShipClass::Carrier => include_bytes!("../../../assets/ships/carrier/model.glb"),
    }
}

/// A simple code-generated ship silhouette used if the GLB can't be loaded.
/// (Renderer fallback only - NOT a substitute for the authored asset.)
fn placeholder_ship_mesh() -> CpuMesh {
    // A single elongated tetra-ish hull: enough to confirm the renderer works.
    let v = |p: Vec3, n: Vec3| Vertex::new(p, n);
    let nose = Vec3::new(0.0, 0.0, 2.0);
    let tail_tl = Vec3::new(-0.6, 0.4, -1.5);
    let tail_tr = Vec3::new(0.6, 0.4, -1.5);
    let tail_b = Vec3::new(0.0, -0.5, -1.5);

    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let tri = |a: Vec3, b: Vec3, c: Vec3, vs: &mut Vec<Vertex>, is: &mut Vec<u32>| {
        let n = (b - a).cross(c - a).normalize_or_zero();
        let base = vs.len() as u32;
        vs.push(v(a, n));
        vs.push(v(b, n));
        vs.push(v(c, n));
        is.extend_from_slice(&[base, base + 1, base + 2]);
    };
    tri(nose, tail_tr, tail_tl, &mut vertices, &mut indices);
    tri(nose, tail_b, tail_tr, &mut vertices, &mut indices);
    tri(nose, tail_tl, tail_b, &mut vertices, &mut indices);
    tri(tail_tl, tail_tr, tail_b, &mut vertices, &mut indices);
    CpuMesh::new(vertices, indices)
}

// --- wasm entry point ------------------------------------------------------

/// Physical (device-pixel) drawable size taken from the browser viewport. More
/// reliable than winit's initial `inner_size()` on web, which can report 0
/// before the canvas is laid out (notably on mobile), leaving a 1x1 surface
/// that gets stretched to a flat color across the screen.
#[cfg(target_arch = "wasm32")]
fn web_drawable_size() -> Option<(u32, u32)> {
    let win = web_sys::window()?;
    let dpr = win.device_pixel_ratio();
    let w = win.inner_width().ok()?.as_f64()?;
    let h = win.inner_height().ok()?.as_f64()?;
    let pw = ((w * dpr).round() as u32).max(1);
    let ph = ((h * dpr).round() as u32).max(1);
    Some((pw, ph))
}

#[cfg(target_arch = "wasm32")]
fn loading_overlay() -> Option<web_sys::Element> {
    web_sys::window()?.document()?.get_element_by_id("loading")
}

/// Remove the HTML "Initializing..." overlay once the renderer is live.
#[cfg(target_arch = "wasm32")]
fn hide_loading_overlay() {
    if let Some(el) = loading_overlay() {
        el.remove();
    }
}

/// Surface a fatal init error in the overlay so it is visible on devices
/// without dev tools (e.g. phones).
#[cfg(target_arch = "wasm32")]
fn show_overlay_error(msg: &str) {
    if let Some(el) = loading_overlay() {
        el.set_text_content(Some(msg));
    }
}

/// DOM control-panel commands, pushed from button click listeners and drained
/// by the app each frame. This is the wasm command seam: the mobile DOM panel
/// and (later) desktop input feed the SAME selection/order path.
#[cfg(target_arch = "wasm32")]
#[derive(Clone, Copy)]
enum UiCmd {
    SelectAll,
    Stop,
    Clear,
    CycleFormation,
    /// Mobile: arm (true) / disarm (false) the band-box drag while the button
    /// is held.
    BoxSelectArm(bool),
}

#[cfg(target_arch = "wasm32")]
thread_local! {
    static UI_QUEUE: std::cell::RefCell<Vec<UiCmd>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(target_arch = "wasm32")]
fn push_ui(cmd: UiCmd) {
    UI_QUEUE.with(|q| q.borrow_mut().push(cmd));
}

/// Attach the DOM control buttons (by id) to the UI command queue. Called
/// before the event loop starts (which never returns on web).
#[cfg(target_arch = "wasm32")]
fn wire_dom_controls() {
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    for (id, cmd) in [
        ("sol-select-all", UiCmd::SelectAll),
        ("sol-stop", UiCmd::Stop),
        ("sol-clear", UiCmd::Clear),
        ("sol-formation", UiCmd::CycleFormation),
    ] {
        if let Some(el) = doc.get_element_by_id(id) {
            let cb = Closure::<dyn FnMut()>::new(move || push_ui(cmd));
            let _ = el.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
            cb.forget();
        }
    }
    // Box-select is a press-and-hold: arm on pointer down, disarm on release
    // (or cancel/leave), so a finger drag on the canvas draws the band box.
    if let Some(el) = doc.get_element_by_id("sol-box-select") {
        for (event, on) in [
            ("pointerdown", true),
            ("pointerup", false),
            ("pointercancel", false),
            ("pointerleave", false),
        ] {
            let cb = Closure::<dyn FnMut()>::new(move || push_ui(UiCmd::BoxSelectArm(on)));
            let _ = el.add_event_listener_with_callback(event, cb.as_ref().unchecked_ref());
            cb.forget();
        }
    }
}

/// Update the formation HUD button label to the active formation.
#[cfg(target_arch = "wasm32")]
fn set_formation_label(name: &str) {
    if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
        if let Some(el) = doc.get_element_by_id("sol-formation") {
            el.set_text_content(Some(&format!("Form: {name}")));
        }
    }
}

/// wasm entry point. Trunk calls this on load. Sets up panic/log hooks, wires
/// the DOM controls, then starts the app (which creates the canvas).
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
    let _ = console_log::init_with_level(log::Level::Info);

    wire_dom_controls();
    run();
}
