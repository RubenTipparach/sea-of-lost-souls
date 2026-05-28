//! `sol-app` - the entry point that wires winit + `sol-render` together and
//! drives the Homeworld-style orbit camera. Native opens a window; wasm grabs a
//! canvas (see [`start`]). The test interceptor mesh is loaded from its GLB
//! (with a code-generated placeholder fallback if loading fails).

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::Arc;

use glam::{EulerRot, Mat4, Quat, Vec3, Vec4};
use sol_render::{
    BgVertex, CpuMesh, CpuTexture, GpuMesh, MeshInstance, OrbitCamera, OverlayVertex,
    RenderOutcome, Renderer, SensorSphere, StarInstance, Vertex,
};
#[cfg(target_arch = "wasm32")]
use sol_sim::CrewKind;
#[cfg(target_arch = "wasm32")]
use sol_sim::Stance;
use sol_sim::{CombatEvent, Command, EntityId, Patrol, ShipClass, Team, WeaponKind, World};

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

/// Min/max orbit distance (dolly clamp). The ceiling is high enough to pull back
/// to the sensors-manager battlefield overview.
const MIN_DISTANCE: f32 = 1.5;
const MAX_DISTANCE: f32 = 400.0;
/// Pitch clamp to avoid gimbal flip at the poles.
const PITCH_LIMIT: f32 = std::f32::consts::FRAC_PI_2 - 0.05;

/// Sensors-manager battlefield framing: pull the camera way out and tilt toward
/// a top-down view so the whole engagement reads at once.
const SENSORS_VIEW_DISTANCE: f32 = 300.0;
const SENSORS_VIEW_PITCH: f32 = 1.15;
/// Sensor-field sphere fill (RGBA). A low alpha keeps it a slight, solid-reading
/// translucent volume; depth write means overlaps do not stack.
const SENSOR_FIELD_COLOR: [f32; 4] = [0.28, 0.58, 1.0, 0.16];
/// How long a ship-death explosion VFX plays (seconds; visual only).
const EXPLOSION_DURATION: f32 = 0.7;

/// A transient ship-death explosion (visual only; never read by the sim). Emits
/// an expanding additive particle burst from `pos` over [`EXPLOSION_DURATION`].
#[derive(Clone, Copy)]
struct Explosion {
    pos: Vec3,
    /// `App::elapsed` when the ship died.
    start: f32,
    /// Burst size, scaled from the destroyed ship's class.
    scale: f32,
}

/// Transient weapon/impact VFX (a muzzle flash, an orange hull-hit spark, or a
/// blue shield-ripple). Spawned from `CombatEvent`s drained out of the sim.
#[derive(Clone, Copy)]
enum SparkKind {
    Muzzle,
    HitHull,
    HitShield,
}

#[derive(Clone, Copy)]
struct Spark {
    pos: Vec3,
    /// Muzzle aim direction (zero for impacts).
    dir: Vec3,
    start: f32,
    kind: SparkKind,
    color: [f32; 3],
}

fn spark_duration(kind: SparkKind) -> f32 {
    match kind {
        SparkKind::Muzzle => 0.10,
        SparkKind::HitHull => 0.22,
        SparkKind::HitShield => 0.28,
    }
}

/// Build-overlay turntable: default pitch, auto-spin rate (rad/s), and the idle
/// delay (s) after a manual drag before the spin resumes and the pitch eases
/// back to `PREVIEW_PITCH`.
const PREVIEW_PITCH: f32 = 0.32;
const PREVIEW_SPIN: f32 = 0.5;
const PREVIEW_RETURN_DELAY: f32 = 2.0;

/// Held pan/elevation keys (WASD + arrows pan the focus; Q/E change altitude).
/// Read every frame so panning is smooth and framerate-independent.
#[derive(Default)]
struct PanKeys {
    forward: bool,
    back: bool,
    left: bool,
    right: bool,
    rise: bool,
    fall: bool,
}

impl PanKeys {
    /// Net (right, forward, elevation) input in [-1, 1] per axis from held keys.
    fn axes(&self) -> (f32, f32, f32) {
        let axis = |pos: bool, neg: bool| (pos as i32 - neg as i32) as f32;
        (
            axis(self.right, self.left),
            axis(self.forward, self.back),
            axis(self.rise, self.fall),
        )
    }
}

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
    /// Shared procedural rock, instanced at every resource node.
    asteroid: GpuMesh,
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
    /// Wall-clock seconds since start, for purely visual animation (particles).
    /// Render-only; never read by the sim.
    elapsed: f32,
    /// Active ship-death explosions (visual only; emitted into the particle pass).
    explosions: Vec<Explosion>,
    /// Active muzzle / impact sparks drained from the sim's combat-event stream.
    sparks: Vec<Spark>,
    /// Last frame's entity positions/classes, used to detect deaths (an id that
    /// vanished) and spawn an explosion at its last position. Render-only.
    tracked: HashMap<EntityId, (Vec3, ShipClass)>,
    /// Locally-selected ships (UI state only; never enters the sim).
    selected: Vec<EntityId>,
    /// Active group-move arrangement (cycled with `F` / the HUD button).
    formation: Formation,
    /// Whether the full-screen build overlay is open (shows a 3D ship preview;
    /// in-world input is suspended while it is up).
    build_open: bool,
    /// Ship class currently previewed in the build overlay.
    build_preview: ShipClass,
    /// Build-overlay turntable orientation. Auto-spins, but a drag on the ship
    /// takes over (`preview_dragging`); `preview_idle` counts seconds since the
    /// last drag so the spin can resume after a short pause.
    preview_yaw: f32,
    preview_pitch: f32,
    preview_dragging: bool,
    preview_idle: f32,
    /// Camera lock-on target: while set, the camera focus follows this ship each
    /// frame (orbit/zoom still work). Cleared the moment the player pans. UI
    /// state only; never enters the sim.
    focus_target: Option<EntityId>,
    /// Sensors-manager view: dims the scene, pulls back to a battlefield view,
    /// and draws the tactical grid + sensor spheres. Local UI only.
    sensors_open: bool,
    /// Camera framing (distance, pitch) saved on entering sensors mode, restored
    /// on exit so the player returns to their prior zoom.
    sensors_saved_view: Option<(f32, f32)>,
    /// Single-player pause: when set, the fixed-step sim is not advanced (the
    /// camera and UI still work). App-side only; a lockstep-synced pause is a
    /// future multiplayer concern.
    paused: bool,
    /// In-progress band-select rectangle as (start, current) in physical pixels.
    /// Set while dragging (desktop LMB, or mobile with the box-select button
    /// held); drawn as a HUD rectangle and finalized on release.
    select_box: Option<((f64, f64), (f64, f64))>,
    /// Mobile: true while the "Box Select" button is held, so a one-finger drag
    /// draws a band box instead of orbiting the camera.
    box_select_armed: bool,
    /// Held camera-pan / elevation keys (applied each frame).
    pan_keys: PanKeys,
    /// Whether Shift is held (Shift+A select all, Shift+S stop; shows a hint).
    shift_down: bool,
    /// Whether a real mouse cursor is inside the window (gates edge-scroll, so
    /// it never fires on touch devices that have no hovering cursor).
    cursor_in_window: bool,
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
        spawn_resource_field(&mut world);
        Self {
            window: None,
            graphics: None,
            camera: OrbitCamera::default(),
            controller: CameraController::default(),
            world,
            last_frame: None,
            accumulator: 0.0,
            elapsed: 0.0,
            explosions: Vec::new(),
            sparks: Vec::new(),
            tracked: HashMap::new(),
            selected: Vec::new(),
            formation: Formation::Parade,
            build_open: false,
            build_preview: ShipClass::Fighter,
            preview_yaw: 0.0,
            preview_pitch: PREVIEW_PITCH,
            preview_dragging: false,
            preview_idle: PREVIEW_RETURN_DELAY,
            focus_target: None,
            sensors_open: false,
            sensors_saved_view: None,
            paused: false,
            select_box: None,
            box_select_armed: false,
            pan_keys: PanKeys::default(),
            shift_down: false,
            cursor_in_window: false,
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
        let asteroid = renderer.upload_mesh(&build_asteroid_mesh(), "asteroid");
        let (nebula, nebula_idx, stars, grid) = build_environment();
        renderer.set_environment(&nebula, &nebula_idx, &stars, &grid);

        self.camera.focus = Vec3::ZERO;
        self.camera.distance = 30.0;
        self.graphics = Some(Graphics {
            renderer,
            meshes,
            asteroid,
        });
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
                    let asteroid = renderer.upload_mesh(&build_asteroid_mesh(), "asteroid");
                    let (nebula, nebula_idx, stars, grid) = build_environment();
                    renderer.set_environment(&nebula, &nebula_idx, &stars, &grid);
                    *slot.borrow_mut() = Some(Graphics {
                        renderer,
                        meshes,
                        asteroid,
                    });
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
        self.elapsed += frame_dt;
        // Camera panning/elevation is app-side and keeps working while paused.
        self.apply_camera_pan(frame_dt);
        let dt = sol_sim::TICK_DT;
        // Single-player pause freezes the sim (no steps); the accumulator and
        // interpolation alpha hold, so the fleet renders frozen in place.
        if !self.paused {
            self.accumulator += frame_dt;
            let mut steps = 0;
            while self.accumulator >= dt && steps < 8 {
                self.world.step(dt);
                self.accumulator -= dt;
                steps += 1;
                // Drain this step's combat effects into VFX sparks; the renderer
                // emits them as additive particles + shield-ripple rings below.
                let start = self.elapsed;
                for ev in self.world.drain_combat_events() {
                    let spark = match ev {
                        CombatEvent::Fired { pos, dir, kind } => Spark {
                            pos,
                            dir,
                            start,
                            kind: SparkKind::Muzzle,
                            color: projectile_style(kind).0,
                        },
                        CombatEvent::Hit { pos, shielded } => Spark {
                            pos,
                            dir: Vec3::ZERO,
                            start,
                            kind: if shielded {
                                SparkKind::HitShield
                            } else {
                                SparkKind::HitHull
                            },
                            color: if shielded {
                                [0.4, 0.78, 1.0]
                            } else {
                                [1.0, 0.6, 0.25]
                            },
                        },
                    };
                    self.sparks.push(spark);
                }
            }
        } else {
            // Paused: orders still take effect immediately (active pause) so the
            // player can build/queue at their leisure, but the world does not
            // advance (no movement, harvest, or build progress).
            self.world.apply_commands();
        }
        let alpha = if dt > 0.0 {
            (self.accumulator / dt).clamp(0.0, 1.0)
        } else {
            0.0
        };

        // Build overlay: render a slow turntable preview of the selected ship in
        // place of the fleet (the DOM overlay frames it with the class list and
        // stats). The sim keeps running underneath so the economy stays live.
        if self.build_open {
            let class = self.build_preview;
            // Turntable: auto-spin by default, but a pointer drag on the empty
            // centre of the overlay rotates the ship by hand; after a short idle
            // the spin resumes and the pitch eases back to the default pose.
            let (dx, dy, dragging) = read_preview_drag();
            self.preview_dragging = dragging;
            if dragging {
                self.preview_yaw -= dx * 0.01;
                self.preview_pitch =
                    (self.preview_pitch + dy * 0.01).clamp(-PITCH_LIMIT, PITCH_LIMIT);
                self.preview_idle = 0.0;
            } else {
                self.preview_idle += frame_dt;
                if self.preview_idle >= PREVIEW_RETURN_DELAY {
                    self.preview_yaw += PREVIEW_SPIN * frame_dt;
                    let k = (frame_dt * 4.0).min(1.0); // exp-ish ease back to pose
                    self.preview_pitch += (PREVIEW_PITCH - self.preview_pitch) * k;
                }
            }
            let radius = self.world.registry.get(class).radius;
            let cam = OrbitCamera {
                focus: Vec3::ZERO,
                pitch: self.preview_pitch,
                yaw: self.preview_yaw,
                distance: radius * 3.5 + 6.0,
                ..OrbitCamera::default()
            };
            let model = Mat4::from_scale_rotation_translation(
                Vec3::splat(class_scale(class)),
                Quat::IDENTITY,
                Vec3::ZERO,
            );
            let inst = [MeshInstance::with_tint(
                model,
                livery_color(class, Team::Player),
            )];
            #[cfg(target_arch = "wasm32")]
            {
                set_resources(self.world.matter, 0.0);
                set_crew(&self.world);
                update_build_overlay(&self.world, class);
            }
            let gfx = self.graphics.as_mut().unwrap();
            gfx.renderer.set_overlays(&[], &[], &[]);
            gfx.renderer.set_particles(&[]);
            // Clear any sensors-mode overlays so they can't bleed onto the preview.
            gfx.renderer.set_scene_dim([0.0, 0.0, 0.0, 0.0]);
            gfx.renderer.set_sensor_spheres(&[]);
            let groups: Vec<(&GpuMesh, &[MeshInstance])> = gfx
                .meshes
                .get(&class)
                .map(|m| (m, inst.as_slice()))
                .into_iter()
                .collect();
            let _ = gfx.renderer.render_groups(&cam, &groups);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }

        // Camera lock-on: while a target is set, the focus follows the ship's
        // interpolated position (orbit/zoom still work). A pan clears the lock in
        // `apply_camera_pan`; a destroyed target clears it here.
        if let Some(tid) = self.focus_target {
            match self.world.entity(tid) {
                Some(e) => {
                    self.camera.focus = e.prev_transform.pos.lerp(e.transform.pos, alpha);
                }
                None => {
                    self.focus_target = None;
                    #[cfg(target_arch = "wasm32")]
                    set_focus_ui(false);
                }
            }
        }

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
        let mut asteroid_instances: Vec<MeshInstance> = Vec::new();
        let mut particles: Vec<StarInstance> = Vec::new();
        let mut carrying_total = 0.0f32;
        // Fog of war: an enemy is visible only inside some player ship's sensor
        // sphere. Local UI only (the sim's own AI detection lives in `World::step`).
        let detected: std::collections::HashSet<EntityId> = {
            let players: Vec<(Vec3, f32)> = self
                .world
                .entities
                .iter()
                .filter(|e| e.ship.team == Team::Player)
                .map(|e| {
                    (
                        e.transform.pos,
                        self.world.registry.get(e.ship.class).sensor_range,
                    )
                })
                .collect();
            self.world
                .entities
                .iter()
                .filter(|e| e.ship.team == Team::Enemy)
                .filter(|e| {
                    players
                        .iter()
                        .any(|&(pp, sr)| (e.transform.pos - pp).length_squared() <= sr * sr)
                })
                .map(|e| e.id)
                .collect()
        };
        let sensors = self.sensors_open;
        for e in &self.world.entities {
            // Fog of war: skip enemies that no player sensor currently sees.
            if e.ship.team == Team::Enemy && !detected.contains(&e.id) {
                continue;
            }
            let pos = e.prev_transform.pos.lerp(e.transform.pos, alpha);
            let rot = e.prev_transform.rot.slerp(e.transform.rot, alpha);
            let scale = Vec3::splat(class_scale(e.ship.class));
            let model = Mat4::from_scale_rotation_translation(scale, rot, pos);
            let selected = self.selected.contains(&e.id);
            // The hull's livery band is tinted by this faction color; selection
            // is shown by the ground gizmo + bars, not a hull tint.
            let livery = livery_color(e.ship.class, e.ship.team);
            // In sensors mode, small/medium ships collapse into blips (drawn in the
            // sensors overlay below); only the mothership and capitals keep a model.
            if !sensors || sensors_shows_model(e.ship.class) {
                groups
                    .entry(e.ship.class)
                    .or_default()
                    .push(MeshInstance::with_tint(model, livery));
            }

            let r = self.world.registry.get(e.ship.class).radius;

            if selected {
                let ground = Vec3::new(pos.x, 0.0, pos.z);
                push_circle(&mut gizmo_lines, ground, r * 1.4 + 0.6, [0.3, 1.0, 0.6], 28);
                push_line(&mut gizmo_lines, ground, pos, [0.2, 0.6, 0.4]);
                // Red lead line + reticle to a commanded attack target; otherwise
                // the gold move line to the order point.
                if let Some(te) = e.attack_target.and_then(|tid| self.world.entity(tid)) {
                    let tpos = te.prev_transform.pos.lerp(te.transform.pos, alpha);
                    let tr = self.world.registry.get(te.ship.class).radius;
                    push_dashes(&mut gizmo_lines, pos, tpos, [1.0, 0.32, 0.27], 1.4, 0.8);
                    push_circle(
                        &mut gizmo_lines,
                        tpos,
                        tr * 1.5 + 0.8,
                        [1.0, 0.32, 0.27],
                        22,
                    );
                } else if let Some(target) = e.order {
                    push_dashes(&mut gizmo_lines, pos, target, [1.0, 0.75, 0.25], 1.2, 0.9);
                    push_circle(&mut gizmo_lines, target, 0.8, [1.0, 0.75, 0.25], 16);
                }
                // Teal haul line: to the node while harvesting, to the depot when
                // returning a full load.
                if let Some(g) = e.gather {
                    let dest = if g.returning {
                        carrier_pos(&self.world)
                    } else {
                        self.world.node(g.node).map(|n| n.pos)
                    };
                    if let Some(d) = dest {
                        push_dashes(&mut gizmo_lines, pos, d, [0.3, 0.85, 0.95], 1.0, 0.8);
                    }
                }
            }

            // HUD bars above the ship (suppressed in the zoomed-out sensors view):
            // a blue shield bar (top), the hull bar (when selected or damaged, so
            // combat is legible), then a gold cargo / harvest bar for resourcers.
            let bp = self.world.registry.get(e.ship.class);
            let hf = (e.hull / bp.max_hull.max(1.0)).clamp(0.0, 1.0);
            let sfrac = if bp.max_shield > 0.0 {
                (e.shield / bp.max_shield).clamp(0.0, 1.0)
            } else {
                1.0
            };
            let damaged = hf < 0.999 || sfrac < 0.999;
            if !sensors && (selected || damaged || e.gather.is_some()) {
                if let Some((sx, sy)) = self.project_to_screen(pos + Vec3::Y * (r * 0.8 + 1.2)) {
                    let mut row = sy;
                    if bp.max_shield > 0.0 && (selected || damaged) {
                        let sc = [0.3, 0.65, 1.0, 0.95];
                        push_bar(
                            &mut overlay_tris,
                            &mut overlay_lines,
                            sx,
                            row,
                            sfrac,
                            sc,
                            (vw, vh),
                        );
                        row += 9.0;
                    }
                    if selected || damaged {
                        let hc = [1.0 - hf, 0.25 + 0.7 * hf, 0.18, 0.95];
                        push_bar(
                            &mut overlay_tris,
                            &mut overlay_lines,
                            sx,
                            row,
                            hf,
                            hc,
                            (vw, vh),
                        );
                        row += 9.0;
                    }
                    if let Some(g) = e.gather {
                        let cap = self.world.registry.get(e.ship.class).cargo.max(1.0);
                        let cf = (g.carrying / cap).clamp(0.0, 1.0);
                        let cc = [1.0, 0.78, 0.3, 0.95];
                        push_bar(
                            &mut overlay_tris,
                            &mut overlay_lines,
                            sx,
                            row,
                            cf,
                            cc,
                            (vw, vh),
                        );
                    }
                }
            }

            // Dust (purely visual additive motes): fine dust while mining, and a
            // trailing plume behind a full hauler on the move.
            if let Some(g) = e.gather {
                carrying_total += g.carrying;
                let cap = self.world.registry.get(e.ship.class).cargo.max(1.0);
                if !g.returning {
                    if let Some(n) = self.world.node(g.node) {
                        if (pos - n.pos).length() <= n.radius + r + 3.0 {
                            let surface = n.pos + (pos - n.pos).normalize_or_zero() * n.radius;
                            for k in 0..10u32 {
                                let kk = k as f32;
                                let phase = (self.elapsed * 0.6 + kk * 0.1).fract();
                                let jit = Vec3::new(
                                    (self.elapsed * 2.1 + kk).sin(),
                                    (self.elapsed * 1.7 + kk * 1.7).cos() + 0.4,
                                    (self.elapsed * 1.9 + kk * 0.9).sin(),
                                ) * (0.5 * (1.0 - phase));
                                let p = surface.lerp(pos, phase) + jit;
                                let f = (1.0 - phase) * 0.12 + 0.03;
                                particles.push(StarInstance::new(
                                    p.to_array(),
                                    [f, f * 0.86, f * 0.66],
                                    0.004 + 0.004 * (1.0 - phase),
                                ));
                            }
                        }
                    }
                } else if g.carrying > 0.55 * cap {
                    let v = e.velocity.linear;
                    let speed = v.length();
                    if speed > 1.0 {
                        let back = -v / speed;
                        for k in 0..7u32 {
                            let kk = k as f32;
                            let d = (kk + (self.elapsed * 3.0).fract()) * 0.9;
                            let jit = Vec3::new(
                                (self.elapsed * 1.3 + kk * 2.0).sin(),
                                (self.elapsed * 1.1 + kk).sin() * 0.4,
                                (self.elapsed * 1.7 + kk * 1.3).cos(),
                            ) * 0.35;
                            let p = pos + back * d + jit;
                            let fade = (1.0 - d / 7.0).clamp(0.0, 1.0);
                            let f = fade * 0.1 + 0.02;
                            particles.push(StarInstance::new(
                                p.to_array(),
                                [f, f * 0.9, f * 0.72],
                                0.004 + 0.004 * fade,
                            ));
                        }
                    }
                }
            }
        }

        // Resource nodes: one instanced rock each, tinted brown and dimming as it
        // depletes; per-node rotation gives variety from the one shared mesh.
        for n in &self.world.resource_nodes {
            let frac = (n.amount / n.max_amount.max(1.0)).clamp(0.0, 1.0);
            let rot = Quat::from_euler(
                EulerRot::XYZ,
                n.id.0 as f32 * 0.7,
                n.id.0 as f32 * 1.3,
                n.id.0 as f32 * 0.4,
            );
            let model = Mat4::from_scale_rotation_translation(Vec3::splat(n.radius), rot, n.pos);
            // Texture supplies the rock color; tint only dims as the node empties.
            let v = 0.55 + 0.45 * frac;
            asteroid_instances.push(MeshInstance::with_tint(model, [v, v, v, 1.0]));
        }

        // Projectiles: a kind-colored tracer (the recent travel segment) plus an
        // additive glow at the head. Drawn in both views so combat reads clearly.
        for p in &self.world.projectiles {
            let head = p.prev_pos.lerp(p.pos, alpha);
            let (col, glow) = projectile_style(p.kind);
            push_line(&mut gizmo_lines, p.prev_pos, head, col);
            particles.push(StarInstance::new(head.to_array(), col, glow));
        }

        // Ship-death explosions: an entity that vanished since last frame was
        // destroyed (only deaths despawn), so spawn a burst at its last position.
        let current: HashMap<EntityId, (Vec3, ShipClass)> = self
            .world
            .entities
            .iter()
            .map(|e| (e.id, (e.transform.pos, e.ship.class)))
            .collect();
        let mut new_booms: Vec<Explosion> = Vec::new();
        for (id, (dpos, class)) in &self.tracked {
            if !current.contains_key(id) {
                new_booms.push(Explosion {
                    pos: *dpos,
                    start: self.elapsed,
                    scale: explosion_scale(*class),
                });
            }
        }
        self.explosions.extend(new_booms);
        self.tracked = current;

        // Emit each live explosion as an expanding additive shell that fades out.
        let elapsed = self.elapsed;
        self.explosions
            .retain(|ex| elapsed - ex.start < EXPLOSION_DURATION);
        for ex in &self.explosions {
            let age = (elapsed - ex.start).max(0.0);
            let t = age / EXPLOSION_DURATION;
            let fade = (1.0 - t).clamp(0.0, 1.0);
            let radius = ex.scale * (0.4 + 2.2 * t);
            for k in 0..16u32 {
                let kk = k as f32;
                let a1 = kk * 2.399_963 + ex.start * 7.0;
                let a2 = kk * 1.7 + ex.start * 3.0;
                let dir = Vec3::new(a1.cos() * a2.sin(), a2.cos(), a1.sin() * a2.sin());
                let p = ex.pos + dir * radius;
                let c = fade * 0.9;
                particles.push(StarInstance::new(
                    p.to_array(),
                    [c, c * 0.55, c * 0.22],
                    0.006 + 0.02 * fade,
                ));
            }
        }

        // Muzzle flashes + impact sparks (drained from the sim each step). Each is
        // a brief additive burst; shield hits also get a blue expanding ring.
        self.sparks
            .retain(|s| elapsed - s.start < spark_duration(s.kind));
        for s in &self.sparks {
            let dur = spark_duration(s.kind);
            let age = (elapsed - s.start).max(0.0);
            let t = (age / dur).clamp(0.0, 1.0);
            let fade = 1.0 - t;
            let c0 = s.color;
            match s.kind {
                SparkKind::Muzzle => {
                    for k in 0..6u32 {
                        let kk = k as f32;
                        let a1 = kk * 2.399_963 + s.start * 9.0;
                        let a2 = kk * 1.13 + s.start * 4.0;
                        let lateral = Vec3::new(a1.cos(), a2.cos() * 0.5, a1.sin()) * 0.18;
                        let p = s.pos + s.dir * (0.2 + 1.4 * t) + lateral;
                        let f = fade * 0.9;
                        particles.push(StarInstance::new(
                            p.to_array(),
                            [c0[0] * f, c0[1] * f, c0[2] * f],
                            0.01 + 0.015 * fade,
                        ));
                    }
                }
                SparkKind::HitHull => {
                    for k in 0..10u32 {
                        let kk = k as f32;
                        let a1 = kk * 2.399_963 + s.start * 11.0;
                        let a2 = kk * 1.7 + s.start * 5.0;
                        let dir = Vec3::new(a1.cos() * a2.sin(), a2.cos(), a1.sin() * a2.sin());
                        let p = s.pos + dir * (0.4 + 2.0 * t);
                        let f = fade * 0.95;
                        particles.push(StarInstance::new(
                            p.to_array(),
                            [c0[0] * f, c0[1] * f, c0[2] * f],
                            0.008 + 0.018 * fade,
                        ));
                    }
                }
                SparkKind::HitShield => {
                    // Expanding ring on the XZ plane through the impact, plus a
                    // few additive motes. Reads as a shield ripple.
                    let radius = 0.5 + 2.4 * t;
                    let ring = [c0[0] * fade, c0[1] * fade, c0[2] * fade];
                    push_circle(&mut gizmo_lines, s.pos, radius, ring, 22);
                    for k in 0..6u32 {
                        let kk = k as f32;
                        let a = kk * std::f32::consts::FRAC_PI_3 + s.start * 6.0;
                        let p = s.pos + Vec3::new(a.cos(), 0.0, a.sin()) * radius;
                        let f = fade * 0.85;
                        particles.push(StarInstance::new(
                            p.to_array(),
                            [c0[0] * f, c0[1] * f, c0[2] * f],
                            0.008 + 0.012 * fade,
                        ));
                    }
                }
            }
        }

        // Band-select rectangle (desktop LMB drag or mobile box-select hold).
        if let Some((a, b)) = self.select_box {
            let rect = [a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1)];
            push_rect_px(&mut overlay_tris, rect, [0.3, 0.7, 1.0, 0.12], (vw, vh));
            push_rect_outline_px(&mut overlay_lines, rect, [0.55, 0.85, 1.0, 0.9], (vw, vh));
        }

        // Sensors-manager overlay: a tactical grid, a translucent sensor-field
        // sphere around each player ship, and a team-colored blip (sized by class)
        // for every visible ship whose model is hidden. The mothership and capitals
        // keep their model as anchors. The scene is dimmed (at submit, below) so
        // these read clearly. Local UI only; fog of war matches the main loop.
        let mut sensor_spheres: Vec<SensorSphere> = Vec::new();
        if sensors {
            push_sensor_grid(
                &mut gizmo_lines,
                self.camera.focus,
                160.0,
                12.0,
                [0.13, 0.34, 0.52],
            );
            for e in &self.world.entities {
                if e.ship.team == Team::Enemy && !detected.contains(&e.id) {
                    continue;
                }
                let pos = e.prev_transform.pos.lerp(e.transform.pos, alpha);
                if e.ship.team == Team::Player {
                    let sr = self.world.registry.get(e.ship.class).sensor_range;
                    sensor_spheres.push(SensorSphere::new(pos, sr, SENSOR_FIELD_COLOR));
                }
                if !sensors_shows_model(e.ship.class) {
                    push_blip(
                        &mut gizmo_lines,
                        pos,
                        blip_size(e.ship.class),
                        blip_color(e.ship.team),
                    );
                }
            }
        }

        // Resource + crew + build HUD readouts (web).
        #[cfg(target_arch = "wasm32")]
        {
            set_resources(self.world.matter, carrying_total);
            set_crew(&self.world);
            let status = match self.world.build_queue.first() {
                Some(b) => {
                    let bt = self.world.registry.get(b.class).build_time.max(0.001);
                    let pct = (b.progress / bt * 100.0).clamp(0.0, 100.0) as i32;
                    format!(
                        "Building {} {}%  (queue {})",
                        class_label(b.class),
                        pct,
                        self.world.build_queue.len()
                    )
                }
                None => String::new(),
            };
            set_build_status(&status);
            let (sv, sd) = self.selected_stance_ui();
            set_stance_ui(sv, sd);
        }
        #[cfg(not(target_arch = "wasm32"))]
        let _ = carrying_total;

        let gfx = self.graphics.as_mut().unwrap();
        gfx.renderer
            .set_overlays(&gizmo_lines, &overlay_tris, &overlay_lines);
        gfx.renderer.set_particles(&particles);
        // Dim the 3D scene in sensors mode (the grid/spheres/blips stay bright).
        gfx.renderer.set_scene_dim(if sensors {
            [0.0, 0.01, 0.05, 0.62]
        } else {
            [0.0, 0.0, 0.0, 0.0]
        });
        gfx.renderer.set_sensor_spheres(&sensor_spheres);
        let mut render_groups: Vec<(&GpuMesh, &[MeshInstance])> = groups
            .iter()
            .filter_map(|(class, insts)| gfx.meshes.get(class).map(|m| (m, insts.as_slice())))
            .collect();
        render_groups.push((&gfx.asteroid, asteroid_instances.as_slice()));
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
        ShipClass::Salvager => 7,
        ShipClass::Resourcer => 9,
    }
}

/// Mobile pan input as `(right, forward, elevation)` in [-1, 1], read from the
/// JS joystick/slider via window globals. Always zero on native (desktop uses
/// keys + edge-scroll).
#[cfg(target_arch = "wasm32")]
fn read_pan_input() -> (f32, f32, f32) {
    use wasm_bindgen::JsValue;
    let Some(win) = web_sys::window() else {
        return (0.0, 0.0, 0.0);
    };
    let target: JsValue = win.into();
    let read = |name: &str| -> f32 {
        js_sys::Reflect::get(&target, &JsValue::from_str(name))
            .ok()
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0) as f32
    };
    (read("__solPanX"), read("__solPanY"), read("__solElev"))
}

#[cfg(not(target_arch = "wasm32"))]
fn read_pan_input() -> (f32, f32, f32) {
    (0.0, 0.0, 0.0)
}

/// Build-overlay turntable drag from JS: accumulated pointer delta since the
/// last frame and whether a drag is in progress. The deltas are reset to zero on
/// read so each frame consumes only its own movement. Zero on native.
#[cfg(target_arch = "wasm32")]
fn read_preview_drag() -> (f32, f32, bool) {
    use wasm_bindgen::JsValue;
    let Some(win) = web_sys::window() else {
        return (0.0, 0.0, false);
    };
    let target: JsValue = win.into();
    let read = |name: &str| -> f32 {
        js_sys::Reflect::get(&target, &JsValue::from_str(name))
            .ok()
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0) as f32
    };
    let (dx, dy) = (read("__solPrevDX"), read("__solPrevDY"));
    let dragging = read("__solPrevDrag") != 0.0;
    // Consume the deltas so the next frame starts fresh.
    let _ = js_sys::Reflect::set(
        &target,
        &JsValue::from_str("__solPrevDX"),
        &JsValue::from_f64(0.0),
    );
    let _ = js_sys::Reflect::set(
        &target,
        &JsValue::from_str("__solPrevDY"),
        &JsValue::from_f64(0.0),
    );
    (dx, dy, dragging)
}

#[cfg(not(target_arch = "wasm32"))]
fn read_preview_drag() -> (f32, f32, bool) {
    (0.0, 0.0, false)
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

/// Tactical "blip" for a ship in the sensors view: a ring at the ship's altitude
/// plus a pole down to the reference plane with a small foot ring, so position
/// and altitude both read at a glance. `size` scales with the ship class.
fn push_blip(v: &mut Vec<BgVertex>, pos: Vec3, size: f32, color: [f32; 3]) {
    let dim = [color[0] * 0.5, color[1] * 0.5, color[2] * 0.5];
    let ground = Vec3::new(pos.x, 0.0, pos.z);
    push_circle(v, pos, size, color, 16);
    push_line(v, pos, ground, dim);
    push_circle(v, ground, size * 0.5, dim, 12);
}

/// Tactical reference grid on the y=0 plane, snapped to and centered on `center`
/// so it follows the view. `half` is the extent each way, `spacing` the pitch.
fn push_sensor_grid(v: &mut Vec<BgVertex>, center: Vec3, half: f32, spacing: f32, color: [f32; 3]) {
    let cx = (center.x / spacing).round() * spacing;
    let cz = (center.z / spacing).round() * spacing;
    let n = (half / spacing) as i32;
    for i in -n..=n {
        let off = i as f32 * spacing;
        push_line(
            v,
            Vec3::new(cx - half, 0.0, cz + off),
            Vec3::new(cx + half, 0.0, cz + off),
            color,
        );
        push_line(
            v,
            Vec3::new(cx + off, 0.0, cz - half),
            Vec3::new(cx + off, 0.0, cz + half),
            color,
        );
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

/// A small HUD bar centered at screen point `(sx, sy)`: a dark backing, a fill
/// of width proportional to `frac` drawn in `fill_color`, and a faint outline.
/// Used for both the health bar (green -> red) and the cargo bar (gold).
fn push_bar(
    tris: &mut Vec<OverlayVertex>,
    lines: &mut Vec<OverlayVertex>,
    sx: f64,
    sy: f64,
    frac: f32,
    fill_color: [f32; 4],
    view: (f32, f32),
) {
    const HALF_W: f64 = 22.0;
    const HALF_H: f64 = 3.5;
    let (x0, y0, x1, y1) = (sx - HALF_W, sy - HALF_H, sx + HALF_W, sy + HALF_H);
    push_rect_px(tris, [x0, y0, x1, y1], [0.0, 0.0, 0.0, 0.55], view);
    let fill_x1 = x0 + 1.0 + (x1 - x0 - 2.0) * frac as f64;
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

    /// Nearest resource node whose bounding sphere the screen ray hits.
    fn pick_node(&self, sx: f64, sy: f64) -> Option<sol_sim::NodeId> {
        let (origin, dir) = self.screen_ray(sx, sy)?;
        let mut best: Option<(f32, sol_sim::NodeId)> = None;
        for n in &self.world.resource_nodes {
            let t = (n.pos - origin).dot(dir);
            if t <= 0.0 {
                continue;
            }
            let closest = origin + dir * t;
            // A little slack so small far rocks stay tappable.
            if (closest - n.pos).length() <= n.radius + 1.0 && best.is_none_or(|(bt, _)| t < bt) {
                best = Some((t, n.id));
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

    /// Pan + elevate the camera focus from all input sources (held WASD/arrow
    /// keys, desktop edge-scroll, mobile joystick + slider). App-side only;
    /// never touches the sim. `dt` is real frame time, so panning is smooth and
    /// framerate independent.
    fn apply_camera_pan(&mut self, dt: f32) {
        // The build overlay suspends in-world camera control.
        if self.build_open {
            return;
        }
        let (mut rx, mut fy, mut elev) = self.pan_keys.axes();

        // Desktop edge-scroll: cursor near an edge pans that way. Gated on a real
        // hovering cursor with no drag in progress, so it never fires on touch
        // (no hover) or while orbiting / band-boxing.
        if self.cursor_in_window && self.lmb_down.is_none() && self.rmb_down.is_none() {
            if let Some(win) = &self.window {
                let s = win.inner_size();
                let (w, h) = (s.width.max(1) as f64, s.height.max(1) as f64);
                const EDGE: f64 = 22.0;
                let (cx, cy) = self.cursor;
                if cx <= EDGE {
                    rx -= 1.0;
                } else if cx >= w - EDGE {
                    rx += 1.0;
                }
                if cy <= EDGE {
                    fy += 1.0;
                } else if cy >= h - EDGE {
                    fy -= 1.0;
                }
            }
        }

        // Mobile joystick + elevation slider (window globals; zero on native).
        let (mx, my, me) = read_pan_input();
        rx += mx;
        fy += my;
        elev += me;

        if rx.abs() + fy.abs() + elev.abs() < 1e-4 {
            return;
        }
        // Panning by hand breaks any camera lock-on (the focus is being moved).
        if self.focus_target.is_some() {
            self.focus_target = None;
            #[cfg(target_arch = "wasm32")]
            set_focus_ui(false);
        }
        rx = rx.clamp(-1.0, 1.0);
        fy = fy.clamp(-1.0, 1.0);
        elev = elev.clamp(-1.0, 1.0);

        // Camera-relative ground axes (from yaw); speed scales with zoom so the
        // on-screen pan rate feels consistent across distances.
        let yaw = self.camera.yaw;
        let forward = Vec3::new(-yaw.sin(), 0.0, -yaw.cos());
        let right = Vec3::new(yaw.cos(), 0.0, -yaw.sin());
        let pan_speed = (self.camera.distance * 0.7).clamp(3.0, 60.0);
        let elev_speed = (self.camera.distance * 0.5).clamp(2.0, 40.0);
        self.camera.focus += (right * rx + forward * fy) * (pan_speed * dt);
        self.camera.focus.y += elev * elev_speed * dt;

        // Keep focus in sane bounds so the camera can't drift off forever.
        self.camera.focus.x = self.camera.focus.x.clamp(-400.0, 400.0);
        self.camera.focus.y = self.camera.focus.y.clamp(-120.0, 120.0);
        self.camera.focus.z = self.camera.focus.z.clamp(-400.0, 400.0);
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

    /// Right click / mobile move: order the selection here. Clicking a hostile
    /// ship orders the (armed) selection to attack it; clicking a resource node
    /// with any resourcer selected sends those resourcers to harvest it; otherwise
    /// it is a formation move to the clicked point.
    fn on_move_click(&mut self, sx: f64, sy: f64) {
        if self.selected.is_empty() {
            return;
        }
        // Attack a clicked hostile (takes priority over a move to that spot).
        if let Some(id) = self.pick(sx, sy) {
            if self
                .world
                .entity(id)
                .is_some_and(|e| e.ship.team == Team::Enemy)
            {
                self.issue_attack(id);
                return;
            }
        }
        if let Some(node) = self.pick_node(sx, sy) {
            let resourcers: Vec<EntityId> = self
                .selected
                .iter()
                .copied()
                .filter(|id| {
                    self.world
                        .entity(*id)
                        .is_some_and(|e| e.ship.class == ShipClass::Resourcer)
                })
                .collect();
            if !resourcers.is_empty() {
                for id in resourcers {
                    self.world.enqueue(Command::Gather { entity: id, node });
                }
                return;
            }
        }
        let plane_y = self.selection_centroid_y();
        if let Some(center) = self.ground_point(sx, sy, plane_y) {
            self.issue_move(center);
        }
    }

    /// Mobile tap: tapping a hostile with ships selected attacks it; tapping any
    /// other ship selects it; tapping empty space moves the current selection.
    fn on_tap(&mut self, sx: f64, sy: f64) {
        if let Some(id) = self.pick(sx, sy) {
            let hostile = self
                .world
                .entity(id)
                .is_some_and(|e| e.ship.team == Team::Enemy);
            if hostile && !self.selected.is_empty() {
                self.issue_attack(id);
            } else {
                self.selected = vec![id];
            }
        } else if !self.selected.is_empty() {
            self.on_move_click(sx, sy);
        }
    }

    /// Order every selected ship to attack `target` (the sim ignores it for
    /// unarmed classes).
    fn issue_attack(&mut self, target: EntityId) {
        for id in self.selected.clone() {
            self.world.enqueue(Command::Attack { entity: id, target });
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
                ShipClass::Corvette | ShipClass::Salvager => paired_column(&mut sides, 2),
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

    fn toggle_pause(&mut self) {
        self.paused = !self.paused;
        log::info!("paused: {}", self.paused);
        #[cfg(target_arch = "wasm32")]
        set_pause_ui(self.paused);
    }

    /// The ship a Focus press locks onto: the first selected ship, else the
    /// player mothership (carrier), else any player ship.
    fn focus_candidate(&self) -> Option<EntityId> {
        if let Some(id) = self.selected.first().copied() {
            return Some(id);
        }
        let mut fallback = None;
        for e in &self.world.entities {
            if e.ship.team != Team::Player {
                continue;
            }
            if e.ship.class == ShipClass::Carrier {
                return Some(e.id);
            }
            fallback.get_or_insert(e.id);
        }
        fallback
    }

    /// Toggle the sensors-manager view (dimmed scene + tactical grid + sensor
    /// spheres) and pull the camera out to a battlefield overview, restoring the
    /// prior framing on exit. Local UI only.
    fn toggle_sensors(&mut self) {
        self.sensors_open = !self.sensors_open;
        if self.sensors_open {
            self.sensors_saved_view = Some((self.camera.distance, self.camera.pitch));
            self.camera.distance = SENSORS_VIEW_DISTANCE;
            self.camera.pitch = SENSORS_VIEW_PITCH;
        } else if let Some((distance, pitch)) = self.sensors_saved_view.take() {
            self.camera.distance = distance;
            self.camera.pitch = pitch;
        }
        #[cfg(target_arch = "wasm32")]
        set_sensors_ui(self.sensors_open);
    }

    /// Stance dropdown state for the current selection: `(value, disabled)`.
    /// `value` is one of `passive`/`defensive`/`aggressive`/`mixed`; `disabled`
    /// is true when nothing is selected.
    #[cfg(target_arch = "wasm32")]
    fn selected_stance_ui(&self) -> (&'static str, bool) {
        let mut iter = self
            .selected
            .iter()
            .filter_map(|id| self.world.entity(*id).map(|e| e.stance));
        let Some(first) = iter.next() else {
            return ("defensive", true);
        };
        if iter.all(|s| s == first) {
            let v = match first {
                Stance::Passive => "passive",
                Stance::Defensive => "defensive",
                Stance::Aggressive => "aggressive",
            };
            (v, false)
        } else {
            ("mixed", false)
        }
    }

    /// Assign a specific stance to every selected ship (from the HUD dropdown).
    #[cfg(target_arch = "wasm32")]
    fn set_selection_stance(&mut self, stance: Stance) {
        for id in self.selected.clone() {
            self.world
                .enqueue(Command::SetStance { entity: id, stance });
        }
    }

    /// Cycle every selected ship's stance (Passive -> Defensive -> Aggressive ->
    /// ...) in lockstep with the first selected ship's current stance. No-op when
    /// nothing is selected.
    fn cycle_stance(&mut self) {
        let Some(&first) = self.selected.first() else {
            return;
        };
        let Some(current) = self.world.entity(first).map(|e| e.stance) else {
            return;
        };
        let next = current.next();
        for id in self.selected.clone() {
            self.world.enqueue(Command::SetStance {
                entity: id,
                stance: next,
            });
        }
    }

    /// Lock the camera onto the focus candidate, or unlock if already locked
    /// onto it (so the button toggles).
    fn toggle_focus(&mut self) {
        match self.focus_candidate() {
            Some(id) if self.focus_target == Some(id) => {
                self.focus_target = None;
                #[cfg(target_arch = "wasm32")]
                set_focus_ui(false);
            }
            Some(id) => {
                self.focus_target = Some(id);
                #[cfg(target_arch = "wasm32")]
                set_focus_ui(true);
            }
            None => {}
        }
    }

    /// Keyboard handling. WASD + arrows pan and Q/E change elevation (held; the
    /// camera applies them each frame). `Shift+A` selects all and `Shift+S`
    /// stops (so A/S stay free to pan); `C` cycles formation; `Esc` clears.
    fn on_key(&mut self, event: winit::event::KeyEvent) {
        use winit::keyboard::{Key, NamedKey};
        let pressed = event.state == ElementState::Pressed;
        // One-shot actions fire on the initial press, not on auto-repeat.
        let first = pressed && !event.repeat;
        // Pause toggles at any time, including over the build overlay.
        if first && event.logical_key == Key::Named(NamedKey::Space) {
            self.toggle_pause();
            return;
        }
        // While the build overlay is open, in-world keys are suspended; Esc closes.
        if self.build_open {
            if first && event.logical_key == Key::Named(NamedKey::Escape) {
                self.build_open = false;
                #[cfg(target_arch = "wasm32")]
                show_build_overlay(false);
            }
            return;
        }
        match event.logical_key.as_ref() {
            Key::Character("w") | Key::Character("W") | Key::Named(NamedKey::ArrowUp) => {
                self.pan_keys.forward = pressed;
            }
            Key::Character("d") | Key::Character("D") | Key::Named(NamedKey::ArrowRight) => {
                self.pan_keys.right = pressed;
            }
            Key::Named(NamedKey::ArrowDown) => self.pan_keys.back = pressed,
            Key::Named(NamedKey::ArrowLeft) => self.pan_keys.left = pressed,
            // A/S pan unless Shift turns them into Select All / Stop.
            Key::Character("a") | Key::Character("A") => {
                if self.shift_down {
                    if first {
                        self.select_all_player();
                    }
                    self.pan_keys.left = false;
                } else {
                    self.pan_keys.left = pressed;
                }
            }
            Key::Character("s") | Key::Character("S") => {
                if self.shift_down {
                    if first {
                        self.stop_selected();
                    }
                    self.pan_keys.back = false;
                } else {
                    self.pan_keys.back = pressed;
                }
            }
            Key::Character("e") | Key::Character("E") => self.pan_keys.rise = pressed,
            Key::Character("q") | Key::Character("Q") => self.pan_keys.fall = pressed,
            Key::Character("c") | Key::Character("C") => {
                if first {
                    self.formation = self.formation.next();
                    log::info!("formation: {}", self.formation.label());
                }
            }
            Key::Character("f") | Key::Character("F") => {
                if first {
                    self.toggle_focus();
                }
            }
            Key::Character("v") | Key::Character("V") => {
                if first {
                    self.toggle_sensors();
                }
            }
            Key::Character("n") | Key::Character("N") => {
                if first {
                    self.cycle_stance();
                }
            }
            Key::Named(NamedKey::Escape) => {
                if first {
                    self.selected.clear();
                }
            }
            _ => {}
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
                UiCmd::OpenBuild => {
                    self.build_open = true;
                    // Start each open from a clean spinning pose.
                    self.preview_yaw = 0.0;
                    self.preview_pitch = PREVIEW_PITCH;
                    self.preview_idle = PREVIEW_RETURN_DELAY;
                    self.preview_dragging = false;
                    show_build_overlay(true);
                }
                UiCmd::CloseBuild => {
                    self.build_open = false;
                    show_build_overlay(false);
                }
                UiCmd::PreviewBuild(class) => self.build_preview = class,
                UiCmd::BuildSelected => self.world.enqueue(Command::Build {
                    class: self.build_preview,
                }),
                UiCmd::TogglePause => self.toggle_pause(),
                UiCmd::ToggleFocus => self.toggle_focus(),
                UiCmd::ToggleSensors => self.toggle_sensors(),
                UiCmd::SetStance(stance) => self.set_selection_stance(stance),
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
            WindowEvent::ModifiersChanged(mods) => {
                let shift = mods.state().shift_key();
                if shift != self.shift_down {
                    self.shift_down = shift;
                    #[cfg(target_arch = "wasm32")]
                    show_shift_hint(shift);
                }
            }
            WindowEvent::CursorEntered { .. } => self.cursor_in_window = true,
            WindowEvent::CursorLeft { .. } => self.cursor_in_window = false,
            WindowEvent::KeyboardInput { event, .. } => self.on_key(event),
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
        ShipClass::Salvager => 1.0,
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

    // Enemy mothership across the map with two harvesters nearby; the commander
    // AI will dispatch them to gather matter and fund new builds.
    world.spawn_class(
        ShipClass::Carrier,
        Team::Enemy,
        Vec3::new(220.0, 0.0, -180.0),
    );
    for (i, off) in [Vec3::new(-12.0, 0.0, 0.0), Vec3::new(12.0, 0.0, 4.0)]
        .into_iter()
        .enumerate()
    {
        let _ = i;
        world.spawn_class(
            ShipClass::Resourcer,
            Team::Enemy,
            Vec3::new(220.0, 0.0, -180.0) + off,
        );
    }

    // An enemy strike group inbound from one flank. It starts beyond the fleet's
    // sensors (hidden by fog of war) and advances on the mothership, popping into
    // view as it crosses a player sensor sphere.
    let enemy = [
        (ShipClass::Corvette, Vec3::new(120.0, 0.0, -90.0)),
        (ShipClass::Fighter, Vec3::new(128.0, 4.0, -84.0)),
        (ShipClass::Fighter, Vec3::new(128.0, -4.0, -96.0)),
        (ShipClass::Bomber, Vec3::new(134.0, 0.0, -90.0)),
        (ShipClass::FrigateGeneral, Vec3::new(140.0, 0.0, -92.0)),
    ];
    for (class, pos) in enemy {
        world.spawn_class(class, Team::Enemy, pos);
    }
}

/// Tracer color (linear RGB) and additive glow size for a projectile kind.
fn projectile_style(kind: WeaponKind) -> ([f32; 3], f32) {
    match kind {
        WeaponKind::Ballistic => ([1.0, 0.92, 0.55], 0.006),
        WeaponKind::Bomb => ([1.0, 0.55, 0.2], 0.013),
        WeaponKind::Missile => ([1.0, 0.66, 0.3], 0.01),
        // Disabler: cyan beam tracer so the player can see the salvager work.
        WeaponKind::Disabler => ([0.45, 0.85, 1.0], 0.008),
    }
}

/// Death-explosion burst size, scaled from the destroyed ship's class.
fn explosion_scale(class: ShipClass) -> f32 {
    match class {
        ShipClass::Fighter | ShipClass::Bomber => 1.4,
        ShipClass::Corvette | ShipClass::Resourcer | ShipClass::Salvager => 2.4,
        ShipClass::FrigateGeneral | ShipClass::FrigateMissile => 4.0,
        ShipClass::CapitalDestroyer => 6.0,
        ShipClass::Carrier => 8.0,
    }
}

/// Whether a class keeps its 3D model in the sensors-manager view (vs. collapsing
/// into a blip). The mothership and capital ships stay as readable anchors.
fn sensors_shows_model(class: ShipClass) -> bool {
    matches!(class, ShipClass::Carrier | ShipClass::CapitalDestroyer)
}

/// Sensors-view blip radius (world units), scaled by class so larger ships read
/// as bigger blips. Capital/carrier values are unused (they keep their model).
fn blip_size(class: ShipClass) -> f32 {
    match class {
        ShipClass::Fighter => 1.2,
        ShipClass::Bomber => 1.5,
        ShipClass::Resourcer => 1.8,
        ShipClass::Corvette | ShipClass::Salvager => 2.1,
        ShipClass::FrigateGeneral | ShipClass::FrigateMissile => 2.9,
        ShipClass::CapitalDestroyer | ShipClass::Carrier => 3.6,
    }
}

/// Sensors-view blip color, by team (blue friendly, red enemy, grey neutral).
fn blip_color(team: Team) -> [f32; 3] {
    match team {
        Team::Player => [0.35, 0.65, 1.0],
        Team::Enemy => [1.0, 0.32, 0.28],
        Team::Neutral => [0.75, 0.78, 0.82],
    }
}

/// Per-instance faction livery color. The mesh shader multiplies it onto the
/// hull's livery band (the rest of the hull stays neutral grey), so one baked
/// hull serves every faction: to add a faction, add a `Team` arm here. Resourcers
/// are always yellow regardless of side, for at-a-glance identification.
fn livery_color(class: ShipClass, team: Team) -> [f32; 4] {
    if class == ShipClass::Resourcer {
        return [1.0, 0.82, 0.12, 1.0]; // harvester yellow
    }
    match team {
        Team::Player => [0.30, 0.55, 1.0, 1.0],   // blue
        Team::Enemy => [1.0, 0.28, 0.24, 1.0],    // red
        Team::Neutral => [0.72, 0.74, 0.78, 1.0], // neutral grey
    }
}

/// Position of the player's mothership (the matter depot), if any.
fn carrier_pos(world: &World) -> Option<Vec3> {
    world
        .entities
        .iter()
        .find(|e| e.ship.class == ShipClass::Carrier && e.ship.team == Team::Player)
        .map(|e| e.transform.pos)
}

/// Seed a belt of harvestable matter sites around the fleet (deterministic;
/// see `sol_procgen::generate_resource_field`).
fn spawn_resource_field(world: &mut World) {
    for site in sol_procgen::generate_resource_field(0x5EA0_5005, 14) {
        world.spawn_resource_node(Vec3::from(site.pos), site.amount, site.radius);
    }
}

/// A small procedural rock (a displaced, flat-shaded icosahedron), instanced at
/// every resource node. Asteroids may be generated at runtime (see CLAUDE.md);
/// one shared mesh plus per-node rotation/scale gives cheap variety.
fn build_asteroid_mesh() -> CpuMesh {
    let t = (1.0 + 5f32.sqrt()) * 0.5;
    let base = [
        Vec3::new(-1.0, t, 0.0),
        Vec3::new(1.0, t, 0.0),
        Vec3::new(-1.0, -t, 0.0),
        Vec3::new(1.0, -t, 0.0),
        Vec3::new(0.0, -1.0, t),
        Vec3::new(0.0, 1.0, t),
        Vec3::new(0.0, -1.0, -t),
        Vec3::new(0.0, 1.0, -t),
        Vec3::new(t, 0.0, -1.0),
        Vec3::new(t, 0.0, 1.0),
        Vec3::new(-t, 0.0, -1.0),
        Vec3::new(-t, 0.0, 1.0),
    ];
    // Displace each vertex along its normal by a deterministic per-index hash so
    // the rock is lumpy but its shared vertices still meet (no cracks).
    let hash = |i: usize| ((i as f32 * 127.1 + 3.7).sin() * 43758.547).fract().abs();
    let verts: Vec<Vec3> = base
        .iter()
        .enumerate()
        .map(|(i, v)| v.normalize() * (0.82 + 0.34 * hash(i)))
        .collect();
    const FACES: [[usize; 3]; 20] = [
        [0, 11, 5],
        [0, 5, 1],
        [0, 1, 7],
        [0, 7, 10],
        [0, 10, 11],
        [1, 5, 9],
        [5, 11, 4],
        [11, 10, 2],
        [10, 7, 6],
        [7, 1, 8],
        [3, 9, 4],
        [3, 4, 2],
        [3, 2, 6],
        [3, 6, 8],
        [3, 8, 9],
        [4, 9, 5],
        [2, 4, 11],
        [6, 2, 10],
        [8, 6, 7],
        [9, 8, 1],
    ];
    let mut vertices = Vec::with_capacity(FACES.len() * 3);
    let mut indices = Vec::with_capacity(FACES.len() * 3);
    for f in FACES {
        let (a, b, c) = (verts[f[0]], verts[f[1]], verts[f[2]]);
        let n = (b - a).cross(c - a).normalize_or_zero();
        let base = vertices.len() as u32;
        for p in [a, b, c] {
            // Triplanar UV by the face's dominant axis (no stretch on facets).
            let an = n.abs();
            let uv = if an.x >= an.y && an.x >= an.z {
                [p.z, p.y]
            } else if an.y >= an.z {
                [p.x, p.z]
            } else {
                [p.x, p.y]
            };
            vertices.push(Vertex {
                position: p.to_array(),
                normal: n.to_array(),
                uv: [uv[0] * 0.5 + 0.5, uv[1] * 0.5 + 0.5],
            });
        }
        indices.extend_from_slice(&[base, base + 1, base + 2]);
    }
    let mut mesh = CpuMesh::new(vertices, indices);
    mesh.texture = Some(build_asteroid_texture());
    mesh
}

/// A procedural rock albedo (browns/greys with sparse darker craters). Alpha is
/// 0 everywhere so the mesh shader treats it as non-emissive (the alpha channel
/// is the emissive mask for baked ship windows); without this asteroids would
/// render fully bright.
fn build_asteroid_texture() -> CpuTexture {
    const S: u32 = 96;
    let hash = |x: i32, y: i32| -> f32 {
        let mut h = (x
            .wrapping_mul(374_761_393)
            .wrapping_add(y.wrapping_mul(668_265_263))) as u32;
        h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
        ((h ^ (h >> 16)) & 0xffff) as f32 / 65535.0
    };
    let smooth = |t: f32| t * t * (3.0 - 2.0 * t);
    let vnoise = |fx: f32, fy: f32| -> f32 {
        let (ix, iy) = (fx.floor() as i32, fy.floor() as i32);
        let (tx, ty) = (smooth(fx - ix as f32), smooth(fy - iy as f32));
        let a = hash(ix, iy);
        let b = hash(ix + 1, iy);
        let c = hash(ix, iy + 1);
        let d = hash(ix + 1, iy + 1);
        let ab = a + (b - a) * tx;
        let cd = c + (d - c) * tx;
        ab + (cd - ab) * ty
    };
    let fbm = |x: f32, y: f32| -> f32 {
        let (mut f, mut amp, mut freq, mut norm) = (0.0, 0.5, 1.0, 0.0);
        for _ in 0..4 {
            f += amp * vnoise(x * freq, y * freq);
            norm += amp;
            amp *= 0.5;
            freq *= 2.0;
        }
        f / norm
    };
    let lo = [70.0f32, 58.0, 48.0];
    let hi = [170.0f32, 154.0, 138.0];
    let mut rgba = Vec::with_capacity((S * S * 4) as usize);
    for y in 0..S {
        for x in 0..S {
            let (u, v) = (x as f32 * 0.09, y as f32 * 0.09);
            let mut t = fbm(u, v);
            // Sparse darker crater pits.
            if vnoise(u * 2.6 + 11.0, v * 2.6 + 7.0) > 0.78 {
                t *= 0.4;
            }
            let c = |i: usize| (lo[i] + (hi[i] - lo[i]) * t).clamp(0.0, 255.0) as u8;
            rgba.extend_from_slice(&[c(0), c(1), c(2), 0]);
        }
    }
    CpuTexture {
        width: S,
        height: S,
        rgba,
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
        // Placeholder: re-use the resourcer hull until the salvager's own model
        // (with the disabler beam emitter + tow rig) is authored.
        ShipClass::Salvager => include_bytes!("../../../assets/ships/resourcer/model.glb"),
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
    /// Open / close the full-screen build overlay.
    OpenBuild,
    CloseBuild,
    /// Select a class to preview in the build overlay.
    PreviewBuild(ShipClass),
    /// Queue the currently previewed class for construction (if affordable).
    BuildSelected,
    /// Toggle the single-player sim pause.
    TogglePause,
    /// Toggle camera lock-on to the selected ship (or mothership).
    ToggleFocus,
    /// Toggle the sensors-manager view.
    ToggleSensors,
    /// Set the combat stance of every selected ship from the dropdown.
    SetStance(Stance),
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
        ("sol-pause", UiCmd::TogglePause),
        ("sol-bo-pause", UiCmd::TogglePause),
        ("sol-focus", UiCmd::ToggleFocus),
        ("sol-sensors", UiCmd::ToggleSensors),
    ] {
        if let Some(el) = doc.get_element_by_id(id) {
            let cb = Closure::<dyn FnMut()>::new(move || push_ui(cmd));
            let _ = el.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
            cb.forget();
        }
    }
    // Stance dropdown: change-event pushes SetStance with the chosen value.
    if let Some(el) = doc.get_element_by_id("sol-stance") {
        if let Ok(select) = el.dyn_into::<web_sys::HtmlSelectElement>() {
            let s = select.clone();
            let cb = Closure::<dyn FnMut()>::new(move || {
                let stance = match s.value().as_str() {
                    "passive" => Stance::Passive,
                    "aggressive" => Stance::Aggressive,
                    _ => Stance::Defensive,
                };
                push_ui(UiCmd::SetStance(stance));
            });
            let _ = select.add_event_listener_with_callback("change", cb.as_ref().unchecked_ref());
            cb.forget();
        }
    }
    // Build overlay open/confirm/close.
    for (id, cmd) in [
        ("sol-build-toggle", UiCmd::OpenBuild),
        ("sol-bo-build", UiCmd::BuildSelected),
        ("sol-bo-close", UiCmd::CloseBuild),
    ] {
        if let Some(el) = doc.get_element_by_id(id) {
            let cb = Closure::<dyn FnMut()>::new(move || push_ui(cmd));
            let _ = el.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
            cb.forget();
        }
    }
    // Build-overlay class cards: clicking one previews that class.
    for (id, class) in BUILD_BUTTONS {
        if let Some(el) = doc.get_element_by_id(id) {
            let cb = Closure::<dyn FnMut()>::new(move || push_ui(UiCmd::PreviewBuild(class)));
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

/// Toggle the "Shift: A = all, S = stop" hint while Shift is held.
#[cfg(target_arch = "wasm32")]
fn show_shift_hint(show: bool) {
    if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
        if let Some(el) = doc.get_element_by_id("sol-shift-hint") {
            if show {
                let _ = el.remove_attribute("hidden");
            } else {
                let _ = el.set_attribute("hidden", "");
            }
        }
    }
}

/// Short display label for a ship class (HUD).
#[cfg(target_arch = "wasm32")]
fn class_label(class: ShipClass) -> &'static str {
    match class {
        ShipClass::Fighter => "Fighter",
        ShipClass::Bomber => "Bomber",
        ShipClass::Corvette => "Corvette",
        ShipClass::Salvager => "Salvager",
        ShipClass::Resourcer => "Resourcer",
        ShipClass::FrigateGeneral => "Frigate",
        ShipClass::FrigateMissile => "Missile Frigate",
        ShipClass::CapitalDestroyer => "Destroyer",
        ShipClass::Carrier => "Carrier",
    }
}

/// Build-menu button ids paired with the class they queue. Shared by the click
/// wiring and the per-frame affordability gating.
#[cfg(target_arch = "wasm32")]
const BUILD_BUTTONS: [(&str, ShipClass); 8] = [
    ("sol-build-fighter", ShipClass::Fighter),
    ("sol-build-bomber", ShipClass::Bomber),
    ("sol-build-corvette", ShipClass::Corvette),
    ("sol-build-salvager", ShipClass::Salvager),
    ("sol-build-resourcer", ShipClass::Resourcer),
    ("sol-build-frigate_general", ShipClass::FrigateGeneral),
    ("sol-build-frigate_missile", ShipClass::FrigateMissile),
    ("sol-build-capital_destroyer", ShipClass::CapitalDestroyer),
];

/// Reflect the pause state in the HUD: a "PAUSED" banner and the button label.
#[cfg(target_arch = "wasm32")]
fn set_pause_ui(paused: bool) {
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    if let Some(el) = doc.get_element_by_id("sol-paused") {
        if paused {
            let _ = el.remove_attribute("hidden");
        } else {
            let _ = el.set_attribute("hidden", "");
        }
    }
    let label = if paused { "Resume" } else { "Pause" };
    for id in ["sol-pause", "sol-bo-pause"] {
        if let Some(el) = doc.get_element_by_id(id) {
            el.set_text_content(Some(label));
        }
    }
}

/// Reflect the camera lock-on state on the Focus button (label + active class).
#[cfg(target_arch = "wasm32")]
fn set_focus_ui(active: bool) {
    if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
        if let Some(el) = doc.get_element_by_id("sol-focus") {
            let _ = el.class_list().toggle_with_force("active", active);
            el.set_text_content(Some(if active { "Unfocus" } else { "Focus" }));
        }
    }
}

/// Reflect the sensors-manager view state on the Sensors button (active class).
#[cfg(target_arch = "wasm32")]
fn set_sensors_ui(active: bool) {
    if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
        if let Some(el) = doc.get_element_by_id("sol-sensors") {
            let _ = el.class_list().toggle_with_force("active", active);
        }
    }
}

/// Show or hide the full-screen build overlay. While it is up, the in-world
/// controls and the mobile camera pad are hidden: there is nothing to move or
/// select in build mode.
#[cfg(target_arch = "wasm32")]
fn show_build_overlay(show: bool) {
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    if let Some(el) = doc.get_element_by_id("sol-build-overlay") {
        if show {
            let _ = el.remove_attribute("hidden");
        } else {
            let _ = el.set_attribute("hidden", "");
        }
    }
    for id in ["sol-controls", "sol-pad"] {
        if let Some(el) = doc.get_element_by_id(id) {
            if show {
                let _ = el.set_attribute("hidden", "");
            } else {
                let _ = el.remove_attribute("hidden");
            }
        }
    }
}

/// Per-frame build-overlay refresh: highlight the previewed class, dim the
/// classes the player can't afford, fill the info line, and enable/disable the
/// confirm button by affordability (matter + crew).
#[cfg(target_arch = "wasm32")]
fn update_build_overlay(world: &World, preview: ShipClass) {
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    for (id, class) in BUILD_BUTTONS {
        if let Some(el) = doc.get_element_by_id(id) {
            let cl = el.class_list();
            let _ = cl.toggle_with_force("sel", class == preview);
            let _ = cl.toggle_with_force("poor", !world.can_build(class));
        }
    }
    let bp = world.registry.get(preview);
    let kind = match preview.crew_kind() {
        CrewKind::Ops => "ops",
        CrewKind::Pilots => "pilots",
    };
    if let Some(el) = doc.get_element_by_id("sol-bo-info") {
        el.set_text_content(Some(&format!(
            "{}    cost {} MU    crew {} {}    build {}s",
            class_label(preview),
            bp.cost as i64,
            bp.crew,
            kind,
            bp.build_time as i64
        )));
    }
    if let Some(el) = doc.get_element_by_id("sol-bo-build") {
        if world.can_build(preview) {
            let _ = el.remove_attribute("disabled");
        } else {
            let _ = el.set_attribute("disabled", "");
        }
    }
    // Homeworld-style build button: a progress "slider" fill for the current
    // item, plus a yellow xN badge showing the whole queue depth at a glance.
    let (frac, depth) = match world.build_queue.first() {
        Some(b) => {
            let bt = world.registry.get(b.class).build_time.max(0.001);
            ((b.progress / bt).clamp(0.0, 1.0), world.build_queue.len())
        }
        None => (0.0, 0),
    };
    if let Some(el) = doc.get_element_by_id("sol-bo-build-fill") {
        let _ = el.set_attribute("style", &format!("width:{:.1}%", frac * 100.0));
    }
    if let Some(el) = doc.get_element_by_id("sol-bo-count") {
        if depth > 0 {
            el.set_text_content(Some(&format!("x{depth}")));
            let _ = el.remove_attribute("hidden");
        } else {
            let _ = el.set_attribute("hidden", "");
        }
    }
}

/// Update the crew HUD readout: free / capacity for ops and pilots.
#[cfg(target_arch = "wasm32")]
fn set_crew(world: &World) {
    if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
        if let Some(el) = doc.get_element_by_id("sol-crew") {
            let (cap_o, cap_p) = world.crew_capacity();
            let (free_o, free_p) = world.crew_free();
            el.set_text_content(Some(&format!(
                "Crew  ops {}/{}   pilots {}/{}",
                free_o, cap_o, free_p, cap_p
            )));
        }
    }
}

/// Update the build-queue HUD readout (current build + progress + queue depth).
#[cfg(target_arch = "wasm32")]
fn set_build_status(text: &str) {
    if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
        if let Some(el) = doc.get_element_by_id("sol-build-status") {
            el.set_text_content(Some(text));
        }
    }
}

/// Sync the Stance dropdown to the selection: `value` is the chosen option's
/// id (or "mixed" when ships disagree); `disabled` greys out the select when
/// nothing is selected.
#[cfg(target_arch = "wasm32")]
fn set_stance_ui(value: &str, disabled: bool) {
    if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
        if let Some(el) = doc.get_element_by_id("sol-stance") {
            if let Ok(sel) = el.dyn_into::<web_sys::HtmlSelectElement>() {
                sel.set_value(value);
                sel.set_disabled(disabled);
            }
        }
    }
}

/// Update the resource HUD readout: banked Matter Units (MU) plus any cargo in
/// transit on returning harvesters.
#[cfg(target_arch = "wasm32")]
fn set_resources(matter: f32, in_transit: f32) {
    if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
        if let Some(el) = doc.get_element_by_id("sol-resources") {
            let text = if in_transit > 0.5 {
                format!("MU: {}  (+{} en route)", matter as i64, in_transit as i64)
            } else {
                format!("MU: {}", matter as i64)
            };
            el.set_text_content(Some(&text));
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
