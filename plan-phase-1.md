# Phase 1 Plan - Fleet Spine and Economy Slice

The goal of Phase 1 is the first genuinely *playable* slice: a deterministic
fixed-step sim driving a fleet of statically authored ships that you can
**select**, **move** in true 3D, feed with a **resourcer** that gathers salvage,
and grow by **producing** new hulls at a carrier. It runs in the browser and
deploys to Pages on every push.

This maps onto `design.md` as: complete the **M0 spine** (camera, selection,
move-disk, sim/render interpolation) and pull a thin slice of **M4 economy**
(resource gathering + production) forward, using the **ship taxonomy from
[design.md] §4**. Combat, real subsystem/power simulation, capture, AI, and
multiplayer are explicitly **out of scope** here (M1, M2, M5).

---

## Guardrails (non-negotiable)

Pulled from `CLAUDE.md` and `design.md`. Every stage must respect these.

- **Determinism.** `sol-sim` stays pure: no rendering, no wall-clock, no OS/thread
  RNG, no unordered iteration that affects state. Fixed **30 Hz** step; the
  renderer interpolates. All gameplay randomness flows from the seeded sim RNG.
- **Orders are commands.** Player input becomes `Command` values applied at tick
  boundaries, not direct mutations. This is the lockstep seam (design §12-13) even
  though Phase 1 is single-player. **Selection is local UI state** and lives in
  `sol-app`, never in the sim.
- **Static assets.** All 8 ships are authored **offline** (committed `model.glb` +
  `ship.json`) and compiled by `sol-shipc`. **Never** generate ship geometry or
  textures at runtime. Salvage fields and environment are procedural (allowed).
- **Rendering reads the sim, never writes it.**
- **Web-first.** WebGPU primary, WebGL2 fallback; everything must run in the
  browser, not just native.
- **House rules.** No em or en dashes anywhere. Keep `fmt`/`clippy`/`test` green.
  Keep the `ship.json` schema in sync across editor, `sol-assets`, and `sol-shipc`.

---

## Current state (what already exists)

- `sol-sim`: pure `World` with `Body { Transform, Velocity, Ship{mass} }`,
  `spawn`, `step(dt)` (explicit Euler), seeded `Rng`. No ids, teams, orders,
  steering, or resources yet. (`crates/sol-sim/src/lib.rs`)
- `sol-render`: real wgpu renderer, orbit camera, instanced draw of **one** mesh.
  No multi-mesh, no gizmo/line pass, no egui (TODO at `lib.rs:414`).
- `sol-app`: winit + orbit camera (RMB orbit, wheel zoom). Renders a single static
  instance; **does not step a sim**. Web uses a placeholder mesh (GLB is not yet
  bundled into wasm). (`crates/sol-app/src/lib.rs`)
- `sol-assets`: full `ShipDef` serde types mirroring `ship.json`.
- `sol-shipc`: `gen-test-ship` (offline authors the placeholder GLB) and `build`
  (validates GLB nodes + power-graph connectivity, emits `.ship` + `.glb`).
- `editor/`: Three.js authoring tool with `shipDef.ts` (the shared schema).
- `assets/ships/test_interceptor/`: one authored ship.

---

## The ship suite (8 static ships)

Authored offline as placeholder hulls now (distinct silhouettes and scale per
class via an extended `sol-shipc gen`), replaced with real art from the editor
later. Hardpoints are authored for schema completeness even though weapons do not
fire in Phase 1. IDs are snake_case to match `test_interceptor`.

| id | Taxonomy (§4) | Role | Mass (t) | Max spd | Turn (deg/s) | Build: salvage / crew / s | Special |
|---|---|---|---|---|---|---|---|
| `fighter` | Strike craft | Anti-strike, harass | 18 | 140 | 120 | 30 / 2 / 12 | very nimble |
| `bomber` | Strike craft | Anti-capital torpedoes | 30 | 110 | 80 | 50 / 3 / 18 | strike, less nimble |
| `corvette` | Light line | Escort, point defense | 220 | 85 | 55 | 120 / 6 / 28 | bridges strike/frigate |
| `resourcer` | Utility | Salvage collection | 140 | 60 | 40 | 80 / 3 / 22 | `cargo.capacity` 200 |
| `frigate_general` | Frigate | Workhorse line combatant | 4000 | 45 | 22 | 300 / 18 / 50 | backbone |
| `frigate_missile` | Frigate (special) | Standoff missile variant | 4200 | 42 | 20 | 340 / 18 / 55 | missile hardpoints |
| `capital_destroyer` | Capital | Heavy hitter | 16000 | 28 | 10 | 900 / 60 / 110 | wide turns |
| `carrier` | Ark / mothership | Command + production | 40000 | 18 | 6 | prebuilt (not buildable) | `production.bays` 2; loss = game over |

All numbers are **initial intent** and get tuned in Stage 10. The crew figure is
the headcount a ship needs to operate (allocated from the roster, not spent):
`fighter` and `bomber` draw **Pilots**, every other class draws **Ops**.

---

## Schema additions (the contract, bumped to v2)

These fields are added once, in Stage 1, and kept in sync across
`editor/src/shipDef.ts`, `crates/sol-assets/src/lib.rs`, and the `sol-shipc`
validator. `schemaVersion` goes `1 -> 2`; `test_interceptor` is migrated.

- `mobility: { maxSpeed, accel, turnRateDeg }` - required; drives steering.
- `build: { salvage, time }` - required; salvage cost and seconds to produce.
- `cargo: { capacity }` - optional; salvage units a collector can carry.
- `production: { bays }` - optional; concurrent build bays (carrier).

The crew a ship needs is the existing `crew.min`; its **category** (ops vs
pilots) is derived from class (strike craft need pilots, everything else needs
ops). Building allocates that crew from the team roster rather than spending it.

Validation (in `sol-shipc`): `maxSpeed/accel/turnRateDeg > 0`, build costs and
times non-negative and finite, optional blocks well-formed. Floats are `f32`
(same ops native and wasm) for determinism.

---

## Crew as logistics (target design vs Phase 1 scope)

**Target design** (build toward, mostly M2/M4): crew are the rarest resource,
earned through noble deeds (prestige) across the campaign, and act as a logistics
system. They split into **ship operations** and **pilots**. Pilots gain XP and
improve; if killed they are replaced slowly via prestige or by retraining ops
into pilots (less effective). Too few crew forces you to pull crew off other
ships, degrading their system efficiency, repair rate, and performance. Low
morale can make crews leave between missions.

**Phase 1 scope** (the foundation only): a team **roster** with two pools, `Ops`
and `Pilots`, each tracking free vs allocated headcount. A ship needs `crew.min`
of its category to operate; **building allocates** that crew from the free pool
and **blocks** if the pool is short (with a clear HUD message). Deferred to later
milestones: prestige rewards, pilot XP and casualties, retraining, morale-driven
departures, and the reallocation-degradation model (which depends on the
subsystem/power sim in M2).

---

# Part A - Foundations

## Stage 1 - Schema v2: gameplay stats on ships

- Goal: the ship contract can express movement and economy.
- Tasks:
  - [ ] Add `mobility`, `build`, optional `cargo`, optional `production` to the
        TS schema, Rust `ShipDef`, and `defaultShipDef()`.
  - [ ] Bump `schemaVersion` to 2; migrate `test_interceptor/ship.json`.
  - [ ] Extend `sol-shipc` validation for the new fields with sane ranges.
- Touches: `editor/src/shipDef.ts`, `crates/sol-assets/src/lib.rs`,
  `crates/sol-shipc/src/build.rs`, `assets/ships/test_interceptor/ship.json`.
- DoD: `cargo test` round-trips v2; `sol-shipc build assets/ships` passes; editor
  exports v2.

## Stage 2 - Sim core: entities, teams, registry, fixed step, commands

- Goal: a deterministic world that holds a fleet and advances by synchronized
  commands.
- Tasks:
  - [ ] Add stable `EntityId` (incrementing for now; generational free-list lands
        with combat/despawn in M1), `Team`, and `ShipClass`.
  - [ ] Replace the bare `bodies: Vec<Body>` with an entity store (stable index
        order) carrying `Transform`, `Velocity`, `Ship` (mass + class + team +
        blueprint ref). Keep `prev_transform` for render interpolation.
  - [ ] `ShipRegistry`: load compiled blueprints (static stats) keyed by id.
  - [ ] `Command` enum (`MoveTo`, `Stop`, `Harvest`, `Build`, `SetRally`, ...) and
        a per-tick command queue applied at step boundaries, exposed through
        wasm-callable entry points so desktop input and the mobile DOM panel issue
        the *same* commands.
  - [ ] Fixed-timestep accumulator (30 Hz) in `sol-app`; render interpolates
        between `prev` and `curr`.
- Touches: `crates/sol-sim/src/lib.rs` (+ new modules), `crates/sol-app/src/lib.rs`.
- Notes: this is the determinism backbone. No wall-clock in sim; the accumulator
  lives in the app and feeds the sim fixed `dt`.
- DoD: app spawns ships from blueprints and steps at 30 Hz with interpolated
  render; a headless test asserts identical state checksums for identical
  seed + command stream.

## Stage 3 - Ship suite: author 8 static ships

- Goal: eight committed, compilable ships.
- Tasks:
  - [ ] Extend `sol-shipc gen` into a parametric per-class placeholder hull
        generator (distinct silhouette and scale), run **offline**, output
        committed `model.glb` per ship.
  - [ ] Author `ship.json` per class with the Stage 1 fields and class stats from
        the table above (hardpoints included, not yet firing).
  - [ ] `sol-shipc build assets/ships --out assets/compiled` validates all 8.
- Touches: `crates/sol-shipc/src/{gen,geometry,glb}.rs`,
  `assets/ships/<8 ids>/{model.glb,ship.json}`.
- Notes: offline authoring of committed static assets satisfies the hard rule;
  the editor remains the path to real art (design §19B, in parallel).
- DoD: all 8 directories committed; CI `sol-shipc` step green.

## Stage 4 - Rendering: multi-mesh, web assets, team color

- Goal: see the whole fleet, on web and native, colored by team.
- Tasks:
  - [ ] Mesh registry keyed by ship id; group instances by mesh; one
        `draw_indexed` per mesh.
  - [ ] Per-instance tint (player vs neutral/enemy) via an added instance attribute
        and a small shader tweak (`shaders/mesh.wgsl`).
  - [ ] Web asset loading: embed the compiled placeholder GLBs with
        `include_bytes!` so native and wasm load identically (revisit an async
        registry when the asset count grows).
- Touches: `crates/sol-render/src/{lib.rs,mesh.rs}`, `shaders/mesh.wgsl`,
  `crates/sol-app/src/lib.rs`.
- DoD: all 8 ship types render in the browser with distinct shapes and team
  colors.

## Stage 5 - HUD shell: egui in the render pass

- Goal: a place for readouts and menus (prerequisite for Stages 8-9).
- Tasks:
  - [ ] Wire `egui` via `egui-wgpu` + `egui-winit` into the frame (the TODO at
        `sol-render/src/lib.rs:414`).
  - [ ] Stub panels: resource readout (Salvage, Crew) and a selection list.
- Touches: `crates/sol-render/src/lib.rs`, `crates/sol-app/src/lib.rs`, Cargo deps.
- Notes: confirm egui versions line up with the pinned `wgpu`.
- DoD: an egui overlay draws over the scene on web and native without breaking the
  mesh pass.

---

# Part B - The four systems

## Stage 6 - Fleet selection

- Goal: pick ships the way an RTS expects.
- Tasks:
  - [ ] Single click (ray vs bounding sphere), band-box marquee (project positions
        to screen, test rect), double-click select-all-of-type on screen.
  - [ ] Control groups `Ctrl+1..9` set / `1..9` recall; `Shift` add, `Ctrl`
        subtract.
  - [ ] Selection visuals: a ring/bracket per selected ship (instanced ring mesh
        or gizmo pass).
- Touches: `crates/sol-app/src/lib.rs` (input + selection state),
  `crates/sol-render` (selection visuals).
- Notes: selection is **local** app state, never in `sol-sim`.
- DoD: box-select a mixed group, bind and recall a control group, see selection
  rings.

## Stage 7 - Movement: orders, steering, formations

- Goal: command ships in true 3D and watch them arrive in formation.
- Tasks:
  - [ ] Sim: `MoveTo` order with an **arrive** steering behavior (accelerate, then
        decelerate into the point) clamped to blueprint `maxSpeed`/`accel`/
        `turnRateDeg`; turn toward velocity; separation so ships do not stack.
  - [ ] Formation: simple offset slots for a group move.
  - [ ] Input: RMB context-move first (raycast to the selection-altitude plane for
        X/Z), then the `M`-disc tool (`M` shows the disc, mouse sets X/Z,
        `Shift`+drag sets altitude, click confirms).
  - [ ] Gizmos: move-disc, altitude pole, destination ghost.
- Touches: `crates/sol-sim` (orders + steering modules),
  `crates/sol-app` (input + gizmo build), `crates/sol-render` (line/disc pass).
- Notes: steering runs in the fixed step; banking/roll are render-only later.
- DoD: select, issue a 3D move, ships arrive holding formation; a headless test
  shows identical paths for identical seed + orders.

## Stage 8 - Resource gathering

- Goal: a resourcer turns salvage in space into a team stockpile.
- Tasks:
  - [ ] Procedural salvage fields/wrecks as sim entities with a salvage amount
        (seeded, deterministic placement; this is environment, allowed).
  - [ ] Resourcer state machine: `ToField -> Mining -> ToDepot -> Depositing`,
        filling `cargo.capacity`, depositing at the carrier.
  - [ ] Team `Salvage` stockpile in the sim (deterministic counter).
  - [ ] `Harvest` command + simple auto-assign to the nearest field.
- Touches: `crates/sol-sim` (economy + collector modules), `crates/sol-procgen`
  (field placement), `crates/sol-app` + HUD readout.
- DoD: order a resourcer to harvest, watch cargo fill, deposit, and the team
  Salvage counter climb in the HUD.

## Stage 9 - Production

- Goal: spend salvage and crew to build new ships at the carrier.
- Tasks:
  - [ ] Carrier production queue with `production.bays` concurrency.
  - [ ] `Build` command: if the team has enough `Salvage` and free crew of the
        ship's category, enqueue and allocate the crew; progress over
        `build.time`; on completion spawn near the carrier and join the fleet.
  - [ ] Team crew **roster** (`Ops` + `Pilots`, free vs allocated); building
        blocks when the needed pool is short.
  - [ ] Build HUD: buildable classes with costs, queue display, resource readout.
- Touches: `crates/sol-sim` (production module), `crates/sol-app` + egui build menu.
- Notes: the design's "building lowers defenses / diverts power" trade-off is
  deferred; Phase 1 uses cost + time + queue only.
- DoD: select the carrier, build a fighter, see Salvage drop and a Pilot get
  allocated from the roster, the timer run, and the new ship appear and be
  selectable (on desktop and from the mobile panel).

---

# Part C - Slice

## Stage 10 - Integration, determinism tests, tuning

- Goal: prove the whole spine end to end and lock determinism.
- Tasks:
  - [ ] A starting scenario: one carrier, one resourcer, a salvage field, a few
        fighters; the loop is harvest -> build -> select -> move.
  - [ ] Headless determinism harness: same seed + command log produces identical
        end-state checksums over many ticks.
  - [ ] Tune mobility and economy numbers for feel; pass `fmt`/`clippy`/`test`.
- Touches: workspace-wide; a small scenario in `sol-app`, tests in `sol-sim`.
- DoD: on the Pages URL you can harvest salvage, build a ship, box-select the
  fleet, and issue a 3D move order, all on the deterministic 30 Hz sim.

---

## Resolved decisions

1. **Sim core: hand-rolled deterministic store.** Extend the Vec-based world with
   stable EntityIds and typed component storage; explicit ordering keeps lockstep
   determinism simple. `bevy_ecs` is deferred until combat/despawn needs it.
2. **Web asset delivery: embed.** Compiled placeholder GLBs are embedded with
   `include_bytes!` so native and wasm load identically; no fetch layer yet.
3. **Crew: a logistics roster, not a currency.** See the Crew as logistics
   section.
4. **Ship authoring: offline gen now.** Author the 8 placeholder hulls with an
   extended `sol-shipc gen` (committed static assets); the editor authors real art
   later.
5. **Mobile: a minimal DOM test panel.** HTML buttons/menus plus tap input drive
   the same commands as desktop. See the Mobile test harness section.

## Mobile test harness (cross-cutting)

Per the CLAUDE.md rule, every system below must be operable on a phone, so the
harness is built incrementally alongside Stages 6-9, not bolted on at the end.

- A DOM control overlay in `crates/sol-app/index.html`: HTML buttons/menus
  (select all, cycle selection, move-to-tap, stop, build <class>, focus carrier)
  positioned outside the wgpu canvas.
- Buttons call the wasm-exported command entry points from Stage 2, so they share
  one code path with desktop input (no parallel game logic).
- Touch input on the canvas: tap to select, tap to move (the move-disc altitude
  gesture stays desktop-only for now; mobile uses a simple altitude slider).
- Definition of done for each system: it can be exercised end to end from a phone
  browser using only the DOM panel and taps.

## Sequencing notes

Stages 1 and 2 are the spine and unblock everything. Stage 3 (ships) can proceed
in parallel with 4-5 once the schema (Stage 1) lands. Stages 6 and 7
(selection, movement) are the M0 payoff. Stages 8 and 9 (the economy) depend on
the sim core (2), ships (3), and HUD (5). Stage 10 ties it together.

## Out of scope for Phase 1 (deferred)

Weapons and combat, shields/positional damage, real reactor/power-grid and
subsystem simulation, crew-as-individuals and boarding/capture, enemy AI,
multiplayer/lockstep transport, real ship art and textures, planets/nebulas/
anomalies and reactive fields. These are M1, M2, M3, and M5.
