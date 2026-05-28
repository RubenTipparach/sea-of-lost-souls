# CLAUDE.md

Guidance for Claude (and human contributors) working in this repository.

**Sea of Lost Souls** is a 3D, Homeworld-style space RTS with a roguelike
campaign, built in **Rust + wgpu**, targeting the **web browser first** and
desktop second. The full blueprint is in [`design.md`](./design.md) - read it
before making non-trivial changes.

> **`design.md` is the single source of truth.** Any core gameplay design
> change or technical/architectural change MUST be captured in `design.md` as
> part of the same change that introduces it: keep its prose in sync with the
> code and the `ship.json` schema. Working notes elsewhere (e.g. `plan-*.md`)
> are scratch; once a decision sticks, fold it into `design.md` so the blueprint
> never lags behind the build.

---

## ⚠️ Hard rule: fix-it.md must be empty before new features

While [`fix-it.md`](./fix-it.md) has any items in **Pending** or **Awaiting
validation**, no new features land. The only allowed work is:

- Implementing the remaining items on the list.
- Fixing regressions in items already on the list.
- Refactors that are needed to land items on the list.

After every push that lands fixes, **remind the user** which items moved
to "Awaiting validation" and ask them to test in the live build. Validated
items are **deleted** from `fix-it.md` (they don't accumulate as a history).
When the file is empty, normal feature work resumes.

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
- **Mobile-testable gameplay.** The target experience is desktop (mouse +
  keyboard), but every gameplay feature must *also* be exercisable in a
  phone/tablet browser, because mobile testing saves significant iteration time.
  Expect to add a lightweight DOM control overlay (HTML buttons/menus outside the
  wgpu canvas) and basic touch input (tap to select/move) that drive the *same*
  wasm-exported commands as the desktop path. Mobile is a **test harness**, not a
  polished touch UX: keep it minimal but functional, and never ship a gameplay
  feature that can only be tested on desktop.
- **Authored assets are the source of truth.** Commit `model.glb` + `ship.json`;
  never commit `assets/compiled/`. `sol-shipc` is the gate - bad colliders or
  invalid ship data should fail the build.
- **Keep the `ship.json` schema in sync** between the editor (writer) and
  `sol-shipc`/`sol-assets` (readers). It is the contract between the two
  languages.

---

## RTS prototype pitfalls (avoid these)

Hard-won failure modes common to early RTS prototypes, concentrated around
selection, movement, and the deterministic sim. Respect these when touching
`sol-sim` steering or `sol-app` input.

**Determinism (the expensive-to-fix ones)**

- Selection, camera, picking, and input mode are **local UI** in `sol-app` only;
  they must NEVER read or write `sol-sim` state, or networked peers desync.
- Player input becomes a queued `Command` applied at a fixed-step boundary, never
  a direct mutation mid-frame. Commands are the only thing lockstep syncs.
- The sim steps on a fixed `dt`; never read frame time, wall-clock, or OS RNG in
  `sol-sim`. Movement scaled by frame dt diverges and is framerate dependent.
- No iteration whose order depends on a `HashMap`/pointer address if it affects
  state. Iterate entities in stable id/index order.
- Steering/separation must read a **snapshot of all positions taken before the
  update**, so the result is order independent (ship 0's move must not change
  ship 1's input within the same step).

**Movement / steering**

- NEVER send a whole group to one identical point: they stack, jitter, and fight
  over the spot. A group move assigns each ship a distinct **formation slot**.
- Add **separation** (soft repulsion between nearby ships) so they never overlap;
  clamp the combined velocity to the ship's `maxSpeed` or it blows up.
- Implement **arrival**: ease down within a stopping radius and zero the velocity
  inside an arrive threshold, or ships orbit/oscillate around the goal forever.
- Respect `accel` (no instant velocity changes / teleporting) and `turnRateDeg`
  (rotate toward the velocity heading; never snap orientation).
- Tune separation: too strong and ships vibrate/explode apart, too weak and they
  interpenetrate. Clamp the push; let it fade once they are not overlapping.
- Avoid hard collisions that deadlock (two ships swapping one slot forever); soft
  separation plus per-ship slots sidesteps this for the prototype.
- It is a 3D game (Homeworld). Movement may resolve on a chosen altitude plane in
  the prototype, but do not bake in a 2D-only assumption.

**Selection / input**

- Distinguish a **click from a drag** (movement + time threshold) or every order
  also orbits the camera / starts a band-box.
- Picking and band-box must use a correct camera ray / unproject and must reject
  points BEHIND the camera (a naive projection wraps them onto the screen).
- Selection and control groups store stable `EntityId`s, never `Vec` indices
  (indices shift when entities despawn).
- Clicking empty space clears the selection.
- **Mobile parity:** one finger already orbits and two fingers pinch-zoom, so a
  selection is a TAP and a move is a tap on empty space (or a DOM button), all
  driving the SAME commands as desktop. Never ship a unit-management feature that
  only works with a mouse.

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

- **Every commit on every branch builds and deploys to GitHub Pages** via the
  official GitHub Actions Pages flow, intentionally with **no main-branch gate**
  (see `design.md` §17). A single live site is served at the project root; the
  most recent push wins. Pages source must be set to "GitHub Actions".
- **After each change, print the live URLs.** When you finish a change (and
  especially after pushing), output both of these so the deploy and CI are easy
  to track:
  - **GitHub Pages (live site):** https://rubentipparach.github.io/sea-of-lost-souls/
  - **GitHub Actions (CI runs):** https://github.com/RubenTipparach/sea-of-lost-souls/actions
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
