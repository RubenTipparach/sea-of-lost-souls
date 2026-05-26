//! `sol-app` - the entry point that wires winit + `sol-render` together and
//! drives the Homeworld-style orbit camera. Native opens a window; wasm grabs a
//! canvas (see [`start`]). The test interceptor mesh is loaded from its GLB
//! (with a code-generated placeholder fallback if loading fails).

#![forbid(unsafe_code)]

use std::sync::Arc;

use glam::{Mat4, Vec3};
use sol_render::{CpuMesh, GpuMesh, MeshInstance, OrbitCamera, RenderOutcome, Renderer, Vertex};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
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
}

impl Default for CameraController {
    fn default() -> Self {
        Self {
            orbiting: false,
            last_cursor: None,
            orbit_speed: 0.005,
            zoom_speed: 0.1,
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

    fn on_cursor_moved(&mut self, camera: &mut OrbitCamera, x: f64, y: f64) {
        if self.orbiting {
            if let Some((px, py)) = self.last_cursor {
                let dx = (x - px) as f32;
                let dy = (y - py) as f32;
                camera.yaw -= dx * self.orbit_speed;
                camera.pitch =
                    (camera.pitch + dy * self.orbit_speed).clamp(-PITCH_LIMIT, PITCH_LIMIT);
            }
        }
        self.last_cursor = Some((x, y));
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
    mesh: GpuMesh,
}

/// Async-init result slot, shared with the spawned setup future on web.
#[cfg(target_arch = "wasm32")]
type PendingGraphics = std::rc::Rc<std::cell::RefCell<Option<Graphics>>>;

/// Application state. Built lazily on `resumed` (the winit 0.30 lifecycle point
/// where a window/surface can be created on all platforms incl. web).
#[derive(Default)]
struct App {
    window: Option<Arc<Window>>,
    graphics: Option<Graphics>,
    camera: OrbitCamera,
    controller: CameraController,
    /// On web, adapter/device acquisition is async; the result lands here.
    #[cfg(target_arch = "wasm32")]
    pending: Option<PendingGraphics>,
}

impl App {
    /// Native: build the renderer synchronously (pollster blocks on the async
    /// adapter/device request).
    #[cfg(not(target_arch = "wasm32"))]
    fn init_graphics(&mut self, window: Arc<Window>) {
        let size = window.inner_size();
        let (w, h) = (size.width.max(1), size.height.max(1));

        let renderer = match Renderer::new_blocking(Arc::clone(&window), w, h) {
            Ok(r) => r,
            Err(e) => {
                log::error!("failed to create renderer: {e:#}");
                return;
            }
        };
        let cpu_mesh = load_ship_mesh();
        let mesh = renderer.upload_mesh(&cpu_mesh, "test_interceptor");

        self.camera.focus = Vec3::ZERO;
        self.camera.distance = 8.0;
        self.graphics = Some(Graphics { renderer, mesh });
        self.window = Some(window);
    }

    /// Web: spawn an async task to build the renderer; the result is dropped
    /// into a shared cell and picked up on the next redraw.
    #[cfg(target_arch = "wasm32")]
    fn init_graphics(&mut self, window: Arc<Window>) {
        self.camera.focus = Vec3::ZERO;
        self.camera.distance = 8.0;
        self.window = Some(Arc::clone(&window));

        let slot: PendingGraphics = std::rc::Rc::new(std::cell::RefCell::new(None));
        self.pending = Some(std::rc::Rc::clone(&slot));

        let (w, h) = web_drawable_size().unwrap_or_else(|| {
            let s = window.inner_size();
            (s.width.max(1), s.height.max(1))
        });
        wasm_bindgen_futures::spawn_local(async move {
            match Renderer::new(Arc::clone(&window), w, h).await {
                Ok(renderer) => {
                    let mesh = renderer.upload_mesh(&load_ship_mesh(), "test_interceptor");
                    *slot.borrow_mut() = Some(Graphics { renderer, mesh });
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

        let Some(gfx) = self.graphics.as_mut() else {
            // Graphics not ready yet (web async init); keep polling.
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        };
        let instances = [MeshInstance::new(Mat4::IDENTITY)];
        match gfx.renderer.render(&self.camera, &gfx.mesh, &instances) {
            RenderOutcome::NeedsReconfigure => {
                if let Some(window) = &self.window {
                    let size = window.inner_size();
                    gfx.renderer.resize(size.width, size.height);
                }
            }
            RenderOutcome::Presented | RenderOutcome::Skipped => {}
        }
        // Keep animating (orbit is interactive; request continuous frames).
        if let Some(window) = &self.window {
            window.request_redraw();
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
                self.controller.on_mouse_button(button, state);
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.controller
                    .on_cursor_moved(&mut self.camera, position.x, position.y);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                self.controller.on_scroll(&mut self.camera, delta);
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
    let mut app = App::default();
    event_loop.run_app(&mut app).expect("run event loop");
}

/// Load the test interceptor mesh, preferring the authored GLB and falling back
/// to a code-generated placeholder so the app always runs.
fn load_ship_mesh() -> CpuMesh {
    // On native, try the on-disk GLB. (On web, assets are bundled differently;
    // for the scaffold we use the placeholder there - see the wasm note.)
    #[cfg(not(target_arch = "wasm32"))]
    {
        for path in [
            "assets/ships/test_interceptor/model.glb",
            "assets/compiled/test_interceptor.glb",
        ] {
            if std::path::Path::new(path).exists() {
                match CpuMesh::from_gltf_file(path) {
                    Ok(m) => {
                        log::info!("loaded ship mesh from {path}");
                        return m;
                    }
                    Err(e) => log::warn!("failed to load {path}: {e:#}; trying next"),
                }
            }
        }
        log::warn!("no GLB found; using placeholder ship mesh");
    }
    placeholder_ship_mesh()
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

/// wasm entry point. Trunk calls this on load. Sets up panic/log hooks, appends
/// a canvas to the document body, and starts the app.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
    let _ = console_log::init_with_level(log::Level::Info);

    // winit creates the canvas on web via the Window; append it to <body>.
    // The actual canvas wiring happens inside winit 0.30's web backend; here we
    // just ensure a document exists, then run.
    run();
}
