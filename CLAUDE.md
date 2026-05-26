# CLAUDE.md

Guidance for Claude (and human contributors) working in this repository.

**Sea of Lost Souls** is a 3D, Homeworld-style space RTS with a roguelike
campaign, built in **Rust + wgpu**, targeting the **web browser first** and
desktop second. The full blueprint is in [`design.md`](./design.md) - read it
before making non-trivial changes.

---

## ⚠️ Hard rule: asset generation (static vs. procedural)

> **All game models and their textures MUST be statically generated** -
> authored in the ship editor and compiled ahead of time into runtime assets.
>
> **The ONLY things generated procedurally at runtime are:**
> **the space environment** (nebulas, dust clouds, anomalies, environmental
> fields), **planets**, and **(optionally) asteroids**.

Concretely:

- **Ships, stations, and their textures → STATIC.** Author in the web editor,
  export `model.glb` + `ship.json`, compile with `sol-shipc`. **Never** generate
  ship geometry or ship textures procedurally at runtime. Ship textures are
  **baked** into the GLB; do not procedurally texture ships in shaders.
- **Space environment, planets, anomalies → PROCEDURAL** (seeded, deterministic
  for gameplay; visually elaborated on the GPU). See `sol-procgen`.
- **Asteroids → procedural is preferred, static is allowed.**

Why: ships are gameplay-critical and net-synced, so their geometry, colliders,
hardpoints, and sim data must be identical, validated, and versioned across all
peers and builds. Environments benefit from seedable variety and reactivity.
(See `design.md` §14.)

If a change would generate any ship/station model or texture at runtime, **stop
and reconsider** - it violates this rule.

---

## Project shape

This is a Cargo workspace + a standalone web editor. (Scaffolding is in
progress; consult `design.md` §12 & §18 for the intended layout.)

| Path | Purpose |
|---|---|
| `crates/sol-app` | Entry point: native bin + wasm; winit/wgpu/egui glue. |
| `crates/sol-sim` | **Deterministic** simulation (ECS, power grid, subsystems, crew, fields). No rendering, no wall-clock, no OS RNG. |
| `crates/sol-render` | wgpu renderer: render graph, volumetrics, post. Reads sim, never writes it. |
| `crates/sol-procgen` | Seeded universe generation (deterministic gameplay core + visual detail). |
| `crates/sol-net` | P2P deterministic lockstep over `matchbox` (WebRTC). |
| `crates/sol-assets` | Runtime loading of compiled `.ship` data + GLB. |
| `crates/sol-shipc` | CLI: compiles & validates authored ships → runtime assets. |
| `editor/` | Web ship editor (Three.js + TypeScript + Vite). |
| `assets/ships/<id>/` | **Authored** ship defs (`model.glb` + `ship.json`) - committed. |
| `assets/compiled/` | Build output - **git-ignored**, produced by `sol-shipc`. |
| `shaders/` | WGSL shaders. |

---

## Architectural rules (don't break these)

- **`sol-sim` must stay deterministic and pure.** No rendering, no `SystemTime`/
  wall-clock, no OS/thread RNG (use the seeded sim RNG), no unordered iteration
  that affects state. Determinism is what makes lockstep multiplayer and tests
  possible - retrofitting it later is brutal, so respect it from day one.
- **Rendering never feeds gameplay.** `sol-render` reads sim state; it must not
  write back into `sol-sim`. Visual procedural detail may be device-dependent;
  gameplay-affecting fields live in `sol-sim` and are deterministic.
- **Fixed-timestep sim, interpolated render.** Don't tie simulation steps to
  frame rate.
- **Web-first.** Anything added must work in the browser (WebGPU primary,
  WebGL2 fallback). Avoid native-only APIs in shared crates; feature-gate
  desktop-only code. Prefer single-threaded-safe designs unless we deliberately
  enable wasm threads (which require the COOP/COEP service-worker shim on Pages).
- **Authored assets are the source of truth.** Commit `model.glb` + `ship.json`;
  never commit `assets/compiled/`. `sol-shipc` is the gate - bad colliders or
  invalid ship data should fail the build.
- **Keep the `ship.json` schema in sync** between the editor (writer) and
  `sol-shipc`/`sol-assets` (readers). It is the contract between the two
  languages.

---

## Build & dev commands

> Scaffolding-dependent; update this section as crates land.

```bash
# Native game (desktop) - once sol-app exists
cargo run -p sol-app

# Web game (needs: rustup target add wasm32-unknown-unknown; cargo install trunk)
cd crates/sol-app && trunk serve            # dev server with hot reload
cd crates/sol-app && trunk build --release  # production wasm bundle

# Compile + validate ship assets
cargo run -p sol-shipc -- build assets/ships --out assets/compiled

# Web ship editor
cd editor && npm install && npm run dev     # local editor
cd editor && npm run build                  # production build

# Checks (kept green; see CI note below)
cargo fmt --all
cargo clippy --workspace --all-targets
cargo test --workspace
```

---

## CI / deployment

- **Every commit on every branch builds and deploys to GitHub Pages** -
  intentionally **no main-branch gate** (see `design.md` §17). The default
  branch is the canonical URL at the site root; other branches deploy to
  `branch/<slug>/` previews.
- Tests/lint run for signal but **must not block the deploy** (per the brief).
  Still, **keep `fmt`/`clippy`/`test` green** - don't land obviously broken code.
- `sol-shipc` runs in CI before the wasm build; invalid ship assets fail that
  step.

---

## Conventions

- **No em dashes (or en dashes), ever.** Do not use the em dash (Unicode
  U+2014) or en dash (U+2013) in any file: prose, comments, and string literals
  included. Use a spaced hyphen, comma, colon, parentheses, or two sentences;
  use a plain hyphen for numeric ranges. This rule applies repo-wide and is
  enforced by sweeping all tracked text files.
- **Formatting/lint:** `cargo fmt` + `cargo clippy` for Rust; the editor uses its
  own TS lint/format config under `editor/`.
- **Math:** use `glam` types throughout.
- **Serialization:** authoring formats are JSON (editor interop); runtime
  compiled data is binary (`bincode`/`rkyv`). Use `serde`.
- **Comments:** explain *why*, not *what*. Default to none unless a constraint or
  invariant is non-obvious (e.g., "must stay deterministic for lockstep").
- **No new model/texture procedural-generation code for ships** (see the hard
  rule above).

---

## Git workflow (this environment)

- Active development branch: **`claude/vibrant-curie-XN0TO`**. Develop and push
  there; do not push to other branches without explicit permission.
- Push with `git push -u origin <branch>`; after pushing, ensure a **draft PR**
  exists for the branch.
- Conventional, descriptive commit messages; create new commits rather than
  amending pushed history.
