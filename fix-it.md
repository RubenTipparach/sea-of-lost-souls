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

(none)

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
  **clicking a ship row queues one** of that class (and previews it); each row
  shows its own progress-fill "slider" + yellow **xN** queued badge. The bottom
  Build button still queues the previewed class. Enemy keeps its serial queue.
