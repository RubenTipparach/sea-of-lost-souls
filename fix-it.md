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

## Pending

- [ ] **(33) Integrated desktop UI (in-canvas, not DOM).** The build menu and
  the various functional buttons (Sensors, Build toggle, Pause, Focus,
  stance dropdown, MU / crew / queue readouts, selection panels) need to be
  rendered **inside the wgpu canvas** (egui via `egui_wgpu`, or equivalent
  integrated UI), not as HTML/DOM elements. Per the updated CLAUDE.md rule:
  the existing DOM overlay (`#sol-controls`, `#sol-build-overlay`, etc.)
  becomes the **mobile-only test harness**; desktop builds hide it and use
  the integrated UI; both surfaces drive the same wasm-exported commands.
  This is a multi-PR migration: scaffold egui_wgpu, port one panel at a
  time (start with the build menu), keep tests green, then switch the desktop
  default. Not gated on items below validating - it's a separate track.

## Awaiting validation

- [ ] **(3) Carriers carry light weapons.** Carrier blueprint has a light
  Ballistic turret (range 30, dmg 8, cooldown 0.9s) and spawns Defensive.
  (User: low priority, verify later.)
- [ ] **(17) Build menu backdrop = blue gradient horizon.** New
  `Renderer::set_build_backdrop(Some((top, bottom)))` draws a full-screen
  vertical gradient (light blue at top -> near-black at bottom) behind the
  scene; the build-preview render path turns it on and hides the nebula, so
  the previewed ship sits on a horizon gradient. Cleared in the normal path.
- [ ] **(21) Formation followers yield to big ships.** Separation is now
  size-weighted: the push on a ship from a neighbor scales by
  `2 * r_neighbor / (r_self + r_neighbor)`. Equal sizes -> 1.0 (unchanged);
  a fighter near the carrier gets shoved hard, the carrier barely budges, so
  big hulls aren't blocked by their own escorts in parade / group moves.
- [ ] **(22) Enemy carrier blip on the strategic view.** Sensors view now
  always draws a bright-red blip on every enemy carrier, even with no scout
  in range, so the enemy base is always locatable. Other enemy ships still
  appear only when within a player ship's sensor range.
- [ ] **(23) Smooth camera transitions.** Added `cam_target_{distance,pitch,
  focus}` + `ease_camera` (exp smoothing, tau 0.12s). Sensors zoom and focus
  lock-on set the targets and glide; orbit / wheel / pinch / pan write the
  camera directly and resync the targets so they stay instant.
- [ ] **(24) Parallel build system by ship-class lane + per-ship queue UI.**
  Sim: `World.build_queue` replaced by `build_lanes: [Vec<BuildItem>; 4]`
  (Small / Medium / Large / Capital via `ShipClass::build_lane`); all four
  lanes advance + complete concurrently each step; crew reservation + checksum
  sum across lanes. UI: the build overlay sidebar is now four labeled sections;
  each row shows its own progress-fill "slider" + yellow **xN** queued badge.
  Enemy keeps its serial queue. (Click flow updated by items 27, 30, 31 below.)
- [ ] **(25) Resources visible in the build menu.** Three chips at the top
  of the sidebar - **MU / Crew / Pilots** - refreshed each frame from
  `World::crew_capacity` + `crew_free`. (Layout reorganized per item 32.)
- [ ] **(32) Per-section capacity in the build menu.** Each size-class
  header (Small / Medium / Large / Capital) shows `can build N` based on the
  cheapest class in that section, accounting for both MU and free crew of
  the right kind. Hidden when zero so the header doesn't add noise mid-fight.
  Backed by a new `World::capacity_for(class) -> u32` helper.
- [ ] **(26) No grid dots in the build preview.** The build-preview render
  path calls `set_grid(&[])` so only the gradient backdrop shows.
  **Regression fixed:** `Renderer::set_grid` previously panicked on the
  empty-slice case (`buffer slices can not be empty` -> winit RefCell unwind
  -> build menu frozen). Now an empty grid just sets `grid_count = 0` and
  the draw is gated on it.
- [ ] **(27) Right-click a row to cancel + refund.** New
  `Command::CancelBuild { class }` removes the most recently queued item of
  that class from its lane and refunds its MU; row `contextmenu` listener
  fires `CancelBuildRow` and `event.preventDefault()`s the browser menu.
- [ ] **(28) Bottom HUD hidden in build mode.** `show_build_overlay`
  toggles a `body.sol-build-open` class; CSS hides `#sol-controls` plus the
  MU / queue / pending / crew corner readouts while it's set. The Build
  toggle button stays on the main HUD (you need it to re-open later).
- [ ] **(29) Global Build / Pause buttons removed.** The overlay's bottom
  bar is just the info text + Close now. Queueing happens via row clicks;
  pausing is per-lane (item 30).
- [ ] **(30) Per-lane Pause / Resume control.** New
  `Command::ToggleLanePause { lane }` + `World.build_lane_paused[4]`; the
  production pass skips paused lanes (queue preserved). Each row carries a
  `.bo-row-pause` chip (`||`) that flips to play (`>`) when the lane is
  paused; click stops propagation so the row's left-click doesn't also fire.
  Info line appends `[lane paused]` when the previewed class's lane is.
- [ ] **(31) Two-click build flow.** `UiCmd::ClickBuildRow(class)`: if the
  row is already the previewed class, queue one; otherwise just select
  (preview) it. Picking a different class is a pure re-select.
