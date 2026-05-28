# fix-it.md

Active punch list. Per [CLAUDE.md](./CLAUDE.md), **no new features land while
this file has any pending or unvalidated items**. Each item is in one of two
states:

- **Pending** - not implemented yet.
- **Awaiting validation** - implemented, pushed; the user needs to test it
  in the live build and confirm. Once confirmed, the item is **deleted** from
  this file (validated items don't accumulate here).

After every push that lands fixes, the assistant must remind the user which
items moved to "Awaiting validation" and ask for validation.

To validate: open https://rubentipparach.github.io/sea-of-lost-souls/, try
the behavior, then tell me which items pass. Items that pass get deleted.
Items that fail get moved back to Pending with a note on what's still wrong.

---

## Sim / balance

(items 1-7 implemented; see "Awaiting validation" below)

## AI

(items 8-10 implemented; see "Awaiting validation" below)

## Controls

(items 11-12 implemented; see "Awaiting validation" below)

## Rendering

(items 13-19 implemented; see "Awaiting validation" below)

---

## Awaiting validation

- [ ] **(1) Starting fleet = 3 fighters each side.** Each team spawns with
  Carrier + 1 Resourcer + 3 Fighters; no extra strike group.
- [ ] **(2) Enemy farther away.** Enemy mothership moved from `(220, 0, -180)`
  to `(420, 0, -340)` (~440 units from origin vs ~280).
- [ ] **(3) Carriers carry light weapons.** Carrier blueprint now has a
  light Ballistic turret (range 30, dmg 8, cooldown 0.9s). Carrier no longer
  spawns Passive (Defensive default - fires in range, doesn't pursue).
- [ ] **(4) Auto-harvest at start.** New `auto_assign_idle_harvesters` runs
  each step for both teams; any idle Resourcer is assigned the nearest live
  node automatically. The existing manual Gather command path still works
  (player Gather orders take precedence).
- [ ] **(5) Slower harvest rate.** `HARVEST_RATE` lowered from 60.0/s to
  25.0/s.
- [ ] **(6) Half ship speed.** Every blueprint's max_speed + accel halved
  (Fighter 16->8, Bomber 12->6, Corvette 10->5, etc.).
- [ ] **(7) No fighter patrols at spawn.** Player + enemy each spawn 3
  Fighters in a wedge formation behind the carrier (no orbiting circle).
- [ ] **(8) Fighters defend nearby allies.** SOS radius bumped from 35 ->
  90 so any armed ally within real "nearby" distance breaks off to engage
  an attacker. Fighters were already eligible; the larger radius makes it
  reliable.
- [ ] **(9) All ships default to Defensive awareness.** Together with (8),
  Defensive ships auto-engage anything in weapon range AND respond to SOS
  out to 90 units. Carriers join the awareness pool (no longer Passive).
- [ ] **(10) Enemy wave cycle (5-minute increments).** New
  `enemy_wave_cooldown` (init 300s) gates wave formation. Each wave records
  its `initial_size`; when survivors drop to <=50%, the wave **routs**
  (clears attack_target, orders survivors home to the enemy mothership) and
  the cooldown resets. Successful kills also reset the cooldown. The mother-
  ship is excluded from waves (anchors home defense).
- [ ] **(11) Space = Sensors toggle.** Was Pause; now triggers Sensors view.
  V still works as an alias.
- [ ] **(12) P = Pause.** New hotkey. The Pause button on the HUD still
  works too.
- [ ] **(13) Grid dots.** `build_grid` is now per-frame around the camera
  pivot; positions snap to nearest 10; color is white with a soft fade out
  toward the rim. `Renderer::set_grid` re-uploads cheaply each frame.
- [ ] **(14) Lock FPS at 60.** `RedrawRequested` schedules the next loop
  iteration ~16.67ms out via `ControlFlow::WaitUntil`. On web this is a
  no-op (browser vsync wins); on >60Hz native it caps the frame rate.
- [ ] **(15) Nebula no-clip.** Camera `z_far` doubled from 1000 -> 2000.
  Nebula vertices are translated by `camera.focus` in the bg shader via a
  new `bg_offset` field on the camera uniform, so the nebula rides the
  camera pivot like a skybox and never visibly clips.
- [ ] **(16) Stars fixed distance from camera.** Backdrop stars use the
  same `bg_offset` (new `vs_main` in `stars.wgsl`). World-space particles
  (harvest dust, hit sparks) use a new `vs_particle` entry point and the
  matching `particle_pipeline` so they stay in world space and don't drift
  with the camera.
- [ ] **(17) Build overlay has no nebula.** New
  `Renderer::set_background_visible(bool)` toggles the nebula + backdrop
  star draws. Set false each frame while `build_open` is true; grid still
  renders so the preview has a reference plane.
- [ ] **(18) Resource patches as orange dots in Sensors view.** Sensors
  pass now pushes an orange blip at every live `resource_node` (sized by
  the node's footprint), alongside the existing ship blips.
- [ ] **(19) Resource dust puff bigger.** Mining motes: 10 -> 18 sprites,
  ~3x sprite size, brighter, wider jitter. Hauler trail also enlarged.
