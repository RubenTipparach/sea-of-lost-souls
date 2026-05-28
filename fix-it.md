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

- [ ] **(1) Starting fleet = 3 fighters each side.** Player and enemy both
  spawn with only 3 fighters; the rest of the military builds up over time
  from harvesting.
- [ ] **(2) Enemy farther away.** Move the enemy strike group and mothership
  significantly farther from the player.
- [ ] **(3) Carriers carry light weapons.** Carriers should be able to fight
  back (small turret or two).
- [ ] **(4) Auto-harvest at start.** Player resourcers auto-assigned to the
  nearest live node from spawn, same as the enemy commander already does.
- [ ] **(5) Slower harvest rate.** Lower the per-second harvest rate.
- [ ] **(6) Half ship speed.** Global tuning - every ship moves at roughly
  half its current speed.
- [ ] **(7) No fighter patrols at spawn.** Fighters spawn in formation at
  the start, no orbiting circles.

## AI

- [ ] **(8) Fighters defend nearby allies.** Any fighter (and other armed
  ships) should break off to engage an enemy that's attacking a friendly
  within sensor / SOS range.
- [ ] **(9) All ships default to Defensive awareness.** Any ship near a
  fight engages by default; spell out the rule so it's consistent across
  classes.
- [ ] **(10) Enemy wave cycle (5-minute increments).**
  - First wave is gated for 5 minutes; during the build-up phase the
    commander harvests + grows the force.
  - When the timer fires, the wave attacks the player.
  - If the wave loses >=50% of its initial size, the survivors **retreat
    home** and the cycle restarts (another 5-minute build-up).
  - Each successive wave sends an **ever-increasing** force.

## Controls

- [ ] **(11) Space = Sensors toggle.**
- [ ] **(12) P = Pause.**

## Rendering

- [ ] **(13) Grid dots.** Color **white**; positions **integer-locked**
  (snap to `n * 10` in world units); only **near the camera pivot** so the
  grid feels infinite as the camera moves.
- [ ] **(14) Lock FPS at 60.**
- [ ] **(15) Nebula no-clip.** Nebula follows the camera (skybox-style) and
  the camera far plane is doubled.
- [ ] **(16) Stars fixed distance from camera** (skybox-style).
- [ ] **(17) Build overlay has no nebula** behind the previewed ship.
- [ ] **(18) Resource patches as orange dots in Sensors view.**
- [ ] **(19) Resource dust puff bigger.**

---

## Awaiting validation

(items move here once implemented and pushed; user deletes them after
testing the live build)
