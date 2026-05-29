# Sea of Lost Souls - Design Document

> A 3D, Homeworld-style space RTS with roguelike campaign structure, built in
> Rust + wgpu, targeting the **web browser first** and desktop second.
>
> You command not a hero unit but a *fleet intelligence*. The camera is your
> body. The mothership is your heart. The void is full of derelicts and the
> crews who died aboard them - salvage them, capture them, and learn their
> secrets before the sea swallows you too.

This is a living design document. It captures intent, the technical plan, and
the open questions. It is the source of truth for *what we are building and
why*. Implementation details that change often live in code and `CLAUDE.md`.

---

## Table of Contents

1. [Vision & Pillars](#1-vision--pillars)
2. [Game Overview](#2-game-overview)
3. [Core Loop & Roguelike Campaign](#3-core-loop--roguelike-campaign)
4. [Fleet, Ships & Scarcity](#4-fleet-ships--scarcity)
5. [Crew, Capture & Research](#5-crew-capture--research)
6. [Ship Simulation Model](#6-ship-simulation-model)
7. [The Procedural Universe](#7-the-procedural-universe)
8. [Reactive Environments](#8-reactive-environments)
9. [Camera & Controls](#9-camera--controls)
10. [UI / HUD](#10-ui--hud)
11. [Rendering Architecture (wgpu)](#11-rendering-architecture-wgpu)
12. [Technical Architecture](#12-technical-architecture)
13. [Multiplayer (Peer-to-Peer)](#13-multiplayer-peer-to-peer)
14. [Asset Strategy: Static vs Procedural](#14-asset-strategy-static-vs-procedural)
15. [The Ship Asset Pipeline](#15-the-ship-asset-pipeline)
16. [Web-Based Ship Editor](#16-web-based-ship-editor)
17. [Build & Deployment Pipeline](#17-build--deployment-pipeline)
18. [Repository Structure](#18-repository-structure)
19. [Vertical Slice (Milestone 0)](#19-vertical-slice-milestone-0)
20. [Roadmap](#20-roadmap)
21. [Risks & Open Questions](#21-risks--open-questions)
22. [Glossary](#22-glossary)

---

## 1. Vision & Pillars

**Sea of Lost Souls** is a tactical, fully-3D space RTS in the spirit of
*Homeworld*: a small, persistent fleet maneuvering through a vast, dark,
volumetric battlespace where position, orientation, and momentum matter. We
wrap that classic feel in a **roguelike campaign**: scarce resources,
permadeath of the run, and meta-progression earned by mastering the ships and
technologies you pry from the dead.

### Design Pillars

| Pillar | What it means | What it rules out |
|---|---|---|
| **The fleet is precious** | Few ships, expensive to build, painful to lose. Every hull matters. | Disposable spam armies; build-order races. |
| **Space is a 3D place, not a plane** | Full volumetric movement, the famous Homeworld move-disk, attacks from above/below. | 2D-on-a-plane RTS with a fixed top-down camera. |
| **You command, you don't pilot** | The player is fleet intelligence; the camera is its sensorium. Orders, not twitch. | Direct flight-sim piloting of a single ship. |
| **The universe pushes back** | Environments are simulated and reactive - they respond to your tactics and become hazards or tools. | Static skyboxes and inert "decoration" hazards. |
| **Salvage over manufacture** | Capturing and mastering enemy ships is the smart economy; building is the desperate one. **Salvagers** are the star: a tier-one specialist hull that disables, grapples, and drags enemy ships home. Captured hulls can be acclimated and crewed, or **liquidated** for **research points** that unlock advanced frigates and capitals. | Pure factory economies with infinite reinforcements. |
| **Web-first, no install** | It runs in a browser tab at a URL, on every commit. | Desktop-only, heavyweight installers as the primary path. |

### Theme

The title is literal. The battlespace is a graveyard - drifting derelicts,
ghost-quiet hulls adrift in nebulas, crews lost to the dark. Your fleet sails
this sea, and the central tension is *moral and mechanical entanglement with
the dead*: you survive by taking what they left behind, but every captured ship
carries an unfamiliar crew, alien systems, and risk.

---

## 2. Game Overview

- **Genre:** Real-time tactics / RTS, single- and multiplayer, roguelike
  campaign meta-structure.
- **Perspective:** Free 3D command camera (Homeworld "sensors sphere" model).
- **Scale:** Squadron-to-small-fleet. Tens of ships on screen, not thousands.
  Scarcity is a feature, and it keeps the per-ship simulation rich and the
  netcode tractable.
- **Platforms:** Web browser (WebGPU, WebGL2 fallback) **first**; native
  desktop (Windows/Linux/macOS via the same wgpu codebase) **second**.
- **Engine:** Custom Rust engine on **wgpu**, borrowing mature ecosystem crates
  for non-rendering concerns (ECS, math, physics queries, GUI, networking).
  See [§12](#12-technical-architecture).

### The "Camera as Character" conceit

In *Homeworld* the player is a disembodied fleet command bound to a persistent
mothership. We make that literal and mechanical:

- The **camera is the player's presence** in the world - a sensor point of view
  that exists *in* the simulation, not floating above it.
- What the camera/fleet can *see* is governed by the [sensors model](#6-ship-simulation-model):
  fog of war, nebula occlusion, active vs. passive detection. The camera is not
  omniscient.
- The **mothership (the "Ark")** is the anchor of a run. It is where your
  command intelligence "lives," where ships are built, where research happens,
  and where captured crews are interned and re-trained. **Lose the Ark and the
  run ends.** Building ships exposes it (see [§4](#4-fleet-ships--scarcity)).

This conceit lets us unify three things players usually treat separately -
*camera, fog of war, and the run's win/lose condition* - into one coherent
fiction.

---

## 3. Core Loop & Roguelike Campaign

A **run** is a voyage across a sector of the Sea, structured as a branching
node map (think *FTL* / *Slay the Spire* topology, but each node is a 3D
tactical encounter).

```mermaid
flowchart LR
    A[Start: Ark + starter fleet] --> B{Sector Map}
    B -->|jump| C[Encounter Node]
    C --> D[Tactical Battle / Salvage / Event]
    D --> E[Resolve: MU, salvaged ships, crew, damage]
    E --> F{Ark survived?}
    F -->|no| G[Run ends - bank meta-progress]
    F -->|yes| B
    B -->|reach the deep| H[Sector Boss / Anomaly]
    H --> I[Next sector or victory]
    G --> J[Meta: persistent research unlocks]
    I --> J
    J --> A
```

### Within a run (scarce, tense, permadeath)

- **Resources** are scarce and carried between encounters: *Matter Units* (raw
  matter for repair/build), *Power Cells* (deployable energy reserves),
  *Crew* (the rarest resource - people don't respawn), and *Intel* (unlocks
  research within the run).
- **Permadeath** at the run level: destroyed ships and dead crew are gone for
  the run. The Ark's destruction ends the run.
- **Choices compound:** repair vs. build vs. capture vs. flee. Every encounter
  leaves you weaker or richer, and the map forces you deeper.

### Mission objectives (per encounter)

Every tactical encounter (an "encounter node" in the run map) has:

- **Main objective** - the win condition. The default is **destroy the enemy
  mothership**; variants will land over time: defend a friendly station, capture
  a key ship intact, escort a convoy, raid a supply depot, survive a fixed
  duration.
- **Bonus objective** - an optional secondary, e.g. "lose no harvester,"
  "capture two enemy frigates," "kill the enemy commander unit," "complete
  under N minutes." Completing it pays out extra **research points** on top of
  the base reward.
- **Rewards** - **Matter Units** carried back to the Ark (the in-run currency,
  banked into the player's MU pool) and **research points** banked toward the
  meta-progression tree (between runs). Research points come from two sources:
  the base + bonus objective payouts, and **liquidating** enemy ships captured
  by **Salvagers** during the encounter ([§5](#5-crew-capture--research)). Cap-
  turing more, and choosing which captures to liquidate vs keep, is the main
  knob the player has on the research-point economy.

Failing the main objective ends the encounter (and usually the run, when the
Ark is the failed target).

### Between runs (meta-progression)

- A **persistent research tree** ([§5](#5-crew-capture--research)) unlocks
  across runs: new starting loadouts (the **specialized hull classes**, locked
  at run start until unlocked), the *ability* to operate certain captured alien
  hull classes, crew doctrines, Ark modules.
- Players spend **research points** earned from mission rewards (main + bonus
  objectives). Each unlock is an *access* gate (you can now build / crew /
  research this thing), not a flat buff.
- Meta-progression is about **capability and knowledge, not raw power** - you
  unlock *understanding* of exotic tech, not stat boosts. This keeps each run a
  test of tactics, not a grind wall.

---

## 4. Fleet, Ships & Scarcity

Ships are the heart of the fantasy and the economy. They are **few, costly, and
individually simulated** ([§6](#6-ship-simulation-model)).

### Ship classes (initial taxonomy)

| Class | Role | Notes |
|---|---|---|
| **Ark (Mothership)** | Command, production, research, crew berthing | One per run. Its loss = game over. Slow, well-defended, but vulnerable while building. |
| **Strike craft** (fighters/bombers) | Fast harassment, anti-strike-craft | Cheap-ish, crewed by few, attritable but still not free. |
| **Frigates** | Workhorse line combatants; specialized variants (ion, missile, support, **boarding**) | The backbone. Boarding frigates enable capture. |
| **Capital ships** | Heavy hitters, rich subsystem trees | Expensive, slow, devastating; prime capture targets. |
| **Utility** (collectors, sensors, repair) | Economy & support | Matter Units collection, forward sensor pickets. |

### Building is the *desperate* economy

Building at the Ark is intentionally painful:

- **Costly** in Matter Units *and* Crew (a new hull needs people to run it).
- **Slow** - construction takes real encounter time; you can't build mid-fight
  and expect it to arrive.
- **Dangerous** - an active construction bay forces the Ark to lower defenses /
  divert reactor power (an explicit power trade-off in the [energy model](#6-ship-simulation-model)),
  making it **vulnerable**. Enemies that detect a building Ark will press the
  advantage.

### Capturing is the *smart* economy

The cheaper path to a bigger fleet is **taking** ships
([§5](#5-crew-capture--research)) - but it is risky, situational, and gated by
research and crew acclimation. This is the central economic tension: *manufacture
exposes your heart; salvage entangles you with the dead.*

---

## 5. Crew, Capture & Research

### Crew

Crew are the **rarest resource** and a first-class simulated entity per ship:

- Ships require a minimum crew to operate subsystems; below it, systems run
  degraded or offline.
- Crew take **casualties** from hull breaches, life-support failure, fire, and
  boarding actions ([§6](#6-ship-simulation-model)).
- Crew carry **proficiencies** (e.g., *Human Reactor Eng.*, *Vaered Ion
  Doctrine*). Proficiency with a ship's tech determines how much of its
  potential you can unlock.

**Crew are a logistics roster, split into two categories:**

- **Ship operations** crew run reactors, subsystems, and damage control; the
  proficiency model above applies to them.
- **Pilots** fly strike craft (fighters, bombers). Pilots gain **experience**
  and improve over a campaign; when killed they are replaced only slowly, either
  by recruiting with **prestige** earned through noble deeds, or by
  **retraining** operations crew into pilots (cheaper, but sloppier and less
  effective).

Crew are **allocated, not spent**. There is rarely enough to fully staff every
hull, so the player constantly triages: pulling crew off one ship to operate
another **degrades** the donor's system efficiency, repair rate, and overall
performance. Between missions, **low morale** can make crews leave entirely.
Because crew are earned mainly through prestige (noble deeds across the
campaign), they are the tightest constraint in the fleet economy.

**Allocation is automatic by default.** Crew flows from the pool to ships
automatically (e.g. a new build draws its complement on construction); a manual
allocation mode is an opt-in **setting** for players who want to micro who staffs
what. Either way, **the carrier (Ark) is the crew reservoir.**

**Prototype crew model (current build).** Two pools, `Ops` and `Pilots`, are
held by the carrier and **derived from fleet composition** (deterministic, in the
checksum): free crew = carrier capacity minus the requirement of every ship and
queued build. The starting carrier provides **2000 ops + 100 pilots**. Per-class
requirements: fighter 1 pilot, bomber 2 pilots; corvette 10 ops, resourcer 3 ops,
frigate (general/missile) 100 ops, capital destroyer 400 ops; the carrier
requires none (it provides). A build is allowed only when both matter **and**
free crew of the right kind suffice; queued builds reserve their crew up front so
production can't over-commit the roster.

### Capturing a ship (the Salvager loop)

**Salvagers are the keystone class** of the game's economy. They are unarmed
utility hulls unlocked right after Corvettes: slow, tough, fitted with a
**disabler beam** that drops the target's hull to a non-lethal floor without
killing it, and a **grapple/tow rig** that drags the disabled hull back to the
Ark. The player uses salvagers to capture enemy ships *before* the rest of the
fleet kills them, turning combat into a resource-extraction exercise.

```mermaid
flowchart TD
    A[Target enemy ship] --> B[Combat ships soften it<br/>hull falls below the disabler's effective threshold]
    B --> C[Salvager fires the disabler beam<br/>damage ramps inverse to target hull %]
    C --> D[Hull clamped to a non-lethal floor; ship is **Disabled**:<br/>no movement, no firing, ignored by combat AI]
    D --> E[Salvager grapples and tows the dead hull back to the Ark]
    E --> F[On arrival, hull enters the **Pending decisions** queue]
    F --> G{Player chooses, per hull}
    G -->|Keep| H[Allocate crew; if alien tech, requires faction proficiency]
    G -->|Liquidate| I[Strip for research points in the tech tree]
```

**Disabler beam (mechanic).** The disabler does scaling hull damage that ramps
**inversely** with the target's current hull fraction. A full-hull ship barely
feels it; a wounded ship comes apart. Concretely: at 100% hull the disabler
applies a small fraction of base damage, at 50% it does base damage, at 0% it
does near-double. **Hull is clamped to a 5% floor** when struck by a disabler,
and the target is flagged **Disabled** the moment it hits the floor. The
gameplay loop: hit it with combat ships first, *then* swap to salvagers for the
finisher. The disabler itself cannot kill, but other ships can - so the player
must back off their own combat fire once the salvager moves in.

**Disabled state.** A disabled ship has:
- `max_speed` and `accel` forced to zero (it drifts only on residual momentum
  + separation).
- No weapon firing (skipped in the combat pass).
- Skipped by auto-targeting and SOS rallies; commander wave-conscription
  ignores it. (Player-commanded fire still works, if they want to scuttle a
  capture.)
- No flee, no retaliate.
- Eligible for tow by any friendly salvager.

**Tow.** Once a salvager reaches a disabled hull, it grapples and pulls the
hull back to the Ark at half its own max speed. The tow can be broken (the
salvager dying mid-tow drops the hull where it is). Multiple salvagers can
work the battlefield in parallel, but a hull can only be towed by one at a
time.

**Pending decisions queue.** On contact with the Ark the towed hull leaves the
battle and joins a **Pending decisions** queue. The HUD shows `N awaiting
decision`; clicking the badge opens the queue panel where the player commits
**Keep** or **Liquidate** per hull. The decision can be deferred between
missions; pending hulls survive across encounters until resolved.

**Keep vs Liquidate (the core economy).**

- **Keep.** Allocate crew from the carrier's pool (the hull won't operate
  without it). The ship enters the fleet at reduced capability: unknown
  subsystems locked, exotic weapons unusable until understood. Crew aboard
  **acclimate** over time, raising **proficiency** (capped by what's unlocked
  in the research tree). A captured **alien** hull cannot be crewed at all
  until the player has researched the matching **xenotech doctrine** for that
  faction; a freshly captured alien ship therefore sits in the queue as
  *keep-locked* until that branch is bought. **Crew aboard captured alien
  ships also drip alien research points slowly over time** as they figure the
  systems out hands-on, but at a much lower rate than liquidating.
- **Liquidate.** Strip the hull for **research points** at the Ark. Bigger
  and rarer hulls yield more points: a frigate is worth more than a corvette
  is worth more than a fighter, with **alien-tech multipliers** on top.
  Liquidating an alien hull pays into both the **general** research pool and
  the **alien-faction proficiency** branch. Liquidation is the fast lane to
  unlocking xenotech doctrines.

> Design tension: the best captures are also the best to keep. Liquidating is
> a *concession* (you couldn't crew this hull yet, or you need the tier
> unlock more than the asset). Until you've unlocked alien doctrines, every
> alien capture is a **forced liquidation** - which is exactly how you fund
> the first doctrine in the first place.

### Research tree (branches)

Research spans **within-run** (Intel unlocks tactical capability this run) and
**meta** (persistent across runs). Proposed branches:

- **Xenotech mastery** - per-civilization branches (each captured faction's
  hull classes, weapons, and subsystems). Unlocks the *ability* to fully operate
  their ships.
- **Reactor & Power** - better energy throughput, capacitor tech, conduit
  resilience.
- **Doctrine** - crew training: boarding, damage-control, gunnery, sensors.
- **Ark modules** - production speed, additional bays, defensive upgrades,
  research throughput.
- **Survey** - better sensors, anomaly analysis, safer navigation of hazards.

> Design rule: meta-progression unlocks **knowledge and access**, not flat
> stat inflation. The fun is "now I can crew a Vaered dreadnought," not "+10%
> damage."

### Alien factions: the Vaered (first fast follower)

The first alien faction to land after the capture loop ships is the
**Vaered**, an ion-tech civilization that survived in the deep currents of
the Sea long before the player's people arrived. They are the design proving
ground for the **xenotech proficiency** mechanic: their ships are obviously
foreign on first contact, and capturing one is a milestone moment.

**Theme & aesthetic.** Crystalline angular hulls, cyan/blue energy bloom,
slow languid maneuvers, exterior antennae instead of visible weapons. Their
livery uses pale lattice patterns; impacts on Vaered shields shed prismatic
spray. They feel **older** than the player's fleet, not newer.

**Doctrine.** Ion + electromagnetic. Vaered weapons **shred shields** far
faster than they damage hull, and their own ships carry **regenerating ion
shields** that brush off incoming kinetic fire but melt under sustained
human ballistics once the shield is down. Players who learn to fight Vaered
quickly figure out: drop the shield fast (mass fire) or never (skim past
them).

**Ship classes (initial roster, fast follower).**

| Class | Role | Signature trait |
|---|---|---|
| **Vaered Skiff** | strike craft | small ion lance; very fast, paper hull |
| **Vaered Lance** | corvette-equivalent | focused ion lance that can drop a player frigate's shield in one volley |
| **Vaered Ion Frigate** | long-range artillery | the existing example ship; standoff ion bolts |
| **Vaered Carrier** | mothership | crystalline core; the mission boss when Vaered show up |

A first Vaered encounter is balanced as a **research-points farm**: the
player can't crew their hulls yet, so every capture is a forced liquidation
into the **Vaered Doctrine** branch. After unlocking Doctrine I, captured
Vaered ships become *crewable* with a low proficiency cap; later Doctrine
tiers raise the cap and unlock ion weapons + ion shields on human hulls
through **cross-tech retrofit** research.

**Proficiency (the new dimension on crew).** Each crew member carries a
proficiency rating per faction-tech they've worked with: `Human` baseline
plus `Vaered`, and (later factions) `Sho`, `Kethari`, etc. A human crew
freshly assigned to a Vaered hull starts at floor proficiency. **They gain
proficiency over time aboard**, asymptotically approaching the cap allowed
by the player's current Vaered Doctrine tier. Higher proficiency unlocks
more of the ship's subsystems and reduces the unfamiliarity penalty on its
stats. A side-effect of crew working an alien hull: a slow **research-point
drip** into that faction's proficiency branch, so keeping captures
contributes (slowly) to unlocking the rest of the tree.

**Why this is the fast follower.** The capture loop ships first; the moment
it works, the Vaered enter the encounter pool so the keep-or-liquidate
decision actually has stakes. Until then, every capture is a generic human
hull and the decision is purely "do I want this ship or research points?"
The Vaered add the third axis (and the alien-doctrine gate) that makes the
loop a real game.

### Mission-driven progression: from basics to specialized fleet

A new player starts a run with the **bare minimum**: the **Ark / mothership**
itself, **harvesters** (Resourcer class) to mine Matter Units from asteroid
fields, and **fighters** as the generalist combat hull. Every other ship class
is **locked** until the player unlocks it in the research tree using **research
points** earned from missions ([§3](#mission-objectives-per-encounter)).

Indicative unlock chain (subject to balance):

| Tier | Unlock | Why you want it |
|---|---|---|
| Start | **Mothership**, **Harvester**, **Fighter** | generalist baseline; the run viable but vanilla |
| 1 | **Bomber** | heavy unguided ordnance against capital-grade hulls |
| 1 | **Corvette** | tougher gun line for swarms of enemy strike craft |
| 1 | **Salvager** | the keystone unlock: capture enemy hulls for keep-or-liquidate; converts kills into research points |
| 2 | **Missile frigate** | long-range standoff and homing kills on capitals |
| 2 | **General frigate** | line ship, screening, anti-frigate work |
| 3 | **Capital destroyer** | big-gun anchor for mothership engagements |
| 3 | **Xenotech doctrines** | crew enough captured alien ships to use them |

Tier-2 and tier-3 unlocks (the frigates, capital, and doctrines) lean heavily
on **liquidated research points**, so the Salvager is the gateway the player
must pass through to reach the rest of the tree at any pace. Skipping
Salvagers is possible but punishes the player with a much slower meta-loop
fueled only by base mission rewards.

Each tier becomes accessible once enough research points have been banked; the
player chooses the *order* of unlocks (Bomber-first vs Corvette-first changes
what the next mission's fleet composition can answer). The unlock graph is
deliberately wider than deep so each playthrough's tech path can feel
different.

The mission ramp pairs with this: early enemy strike groups should be
fighter-counterable, mid-game missions field corvettes and frigates that
*demand* the player has unlocked their counters, and the final missions field
capital + carrier engagements that only a fully-stocked tech tree can answer.
Until those higher tiers exist in code, the prototype demo uses the full ship
suite at run start; the tier-gating lands with the campaign meta-loop.

---

## 6. Ship Simulation Model

Every non-trivial ship is a **simulated machine**, not a bag of stats. This is
where the depth lives. The model has four coupled layers: **power**,
**subsystems**, **structure/hitboxes**, and **crew**.

### 6.1 Reactor & the power grid

A ship is a small **power network**:

```
 [Reactor] --conduit--> [Bus] --+--> [Capacitor] --> [Weapons]
                                |--> [Engines]
                                |--> [Shields]
                                |--> [Sensors]
                                |--> [Life Support]
```

- The **reactor** produces power (a rate, MW) each simulation tick.
- Power flows along **conduits** (edges with capacity) to a bus and out to
  **subsystems** (nodes that consume power).
- **Capacitors** buffer energy for bursty loads (weapon volleys).
- The player sets **allocation priorities** across subsystems - the core
  tactical economy of a single ship (evoking bridge-sim games like *FTL* /
  Star Trek bridge management, layered onto an RTS). Overdrive engines for a
  flanking run and your shields sag; charge weapons for an alpha strike and
  sensors dim.
- **Damage rewires the network:** a hit that severs a conduit or kills the
  reactor cuts power downstream. This makes *where* you hit a ship matter.

> The power grid is a small graph solved each tick (clamp flows to conduit
> capacity, distribute by priority, drain/charge capacitors). It is fully
> deterministic and lives in the simulation crate (`sol-sim`) - critical for
> [lockstep multiplayer](#13-multiplayer-peer-to-peer).

### 6.2 Subsystems

Each subsystem is a component placed at a **hardpoint** (a position + collider
inside the hull) authored in the [ship editor](#16-web-based-ship-editor).

| Subsystem | Function | Power behavior | If damaged/destroyed |
|---|---|---|---|
| **Reactor** | Generates power | Source | Power output falls; destruction can chain-explode. |
| **Engines** | Thrust + maneuver | Heavy draw; scales with throttle | Sluggish or dead in the water; can't flee. |
| **Shields** | Regenerating barrier (faceted; see below) | Heavy draw; regen scales with power | Facings drop; hull exposed. |
| **Weapons** (per hardpoint) | Damage output | Draw from capacitor per shot | That mount goes silent. |
| **Sensors** | Detection range/resolution | Moderate; active mode draws more | Blind; fog closes in around the ship. |
| **Life Support** | Keeps crew alive | Light but critical | Crew attrition begins; clock to casualties. |
| **Bridge / Command** | Crew control & coordination | Light | Ship loses orders/coordination; easier to capture. |

### 6.3 Structure, hitboxes & positional damage

- Each ship has an **outer hull collider** (convex hull or compound shape)
  authored in the editor, used for hit detection and selection.
- Subsystems have **internal positions + colliders**. A shot that defeats
  shields and penetrates the hull can damage a *specific* subsystem based on
  impact location and penetration - enabling targeted tactics (kill their
  engines to prevent escape; kill weapons to safely board).
- **Shields are faceted/directional** (e.g., 6 facings). Flanking and
  positioning matter: hit the weak facing.
- **Hull integrity** is tracked per **section**; sections breach, venting
  atmosphere (life-support / crew consequences).

> Collision uses `parry3d` shape queries (raycasts for weapons, overlap tests
> for proximity/boarding) over a custom broad-phase (spatial grid / BVH sized
> to the encounter). See [§12](#12-technical-architecture).

### 6.4 Crew (inside the machine)

- Crew occupy the ship and **operate subsystems**; minimum crew thresholds gate
  function.
- **Casualties** from breaches, fire, life-support loss, and **boarding**
  (attacker vs. defender resolution inside the hull over time).
- **Damage control:** crew can be tasked to repair subsystems, fight fires, or
  repel boarders - a within-ship micro-economy of attention.

### 6.5 Sensors, emissions & fog of war

- **Active sensors:** long range, high resolution - but **broadcast your
  position** (emissions detectable by enemies). 
- **Passive sensors:** stealthy, shorter range, lower resolution.
- The environment modulates sensing: **nebulas occlude**, **ion storms jam**,
  **pulsars wash out** passive sensors with periodic sweeps. This ties the
  sensor model directly to the [reactive universe](#8-reactive-environments).
- Fog of war is the player's *actual* knowledge - consistent with the
  [camera-as-character](#the-camera-as-character-conceit) conceit.

### 6.6 Flight & movement model

Homeworld ships are **not** free-floating Newtonian bodies and **not**
rigid-body physics props - they fly with an *arcade* model: a top speed, finite
acceleration, a turn rate, and banking into turns. They arrive and stop; they
don't drift forever. We adopt the same, scaled by **mass class**:

- **State:** position, orientation (quaternion), linear & angular velocity.
- **Steering:** an *arrive* behavior toward the order target (decelerate into
  the point), constrained by per-class max speed / acceleration / turn-rate.
  Strike craft are nimble; capital ships are sluggish and wide-turning.
- **Formation flocking:** formation slots define desired offsets; ships seek
  their slot with separation so they hold shape without jostling.
- **Banking & roll** are *visual* (render-only) flourishes on the deterministic
  motion; they never feed back into the sim.

Movement integration runs in `sol-sim` at the **fixed timestep**
(deterministic); the renderer interpolates between ticks for smoothness.

### 6.7 Collision model (and the Homeworld lineage)

Observably, Homeworld ships **mostly avoid** each other and only
**occasionally collide** - and when they do, they don't ricochet like billiard
balls. Across the series the behavior matured from HW1's looser
avoidance/overlap (capital ships could foul each other, and ramming dealt
damage) to HW2's tighter formation-flying and avoidance. We want that *feel*:
graceful avoidance first, gentle (and damaging) contact second - never bouncy
rigid-body chaos.

Our approach is **avoidance-first kinematic motion with soft contact response**:

1. **Avoidance (primary):** a *separation* steering term keeps ships spaced;
   local avoidance steers around obstacles (other hulls, asteroids) along the
   path. Most "collisions" are prevented before they happen - this is what sells
   the Homeworld look.
2. **Broad phase:** a spatial structure (uniform grid or BVH, e.g. parry's
   `Qbvh`) sized to the encounter finds candidate overlaps cheaply.
3. **Narrow phase:** `parry3d` shape queries against the authored convex
   hulls/compounds ([§6.3](#63-structure-hitboxes--positional-damage)) produce
   contacts.
4. **Soft response (no restitution):** on real contact we apply a **gentle
   separation impulse + velocity damping**, scaled by **mass** - fighters get
   shoved aside, capital ships barely budge, and nothing bounces. Relative
   momentum at impact deals **ram damage** (and can be a deliberate tactic).
5. **Continuous collision detection (CCD):** fast ships (and all projectiles)
   are swept along their path each tick so they can't tunnel through thin hulls
   - essential for both fairness and determinism.

> **Library choice: `parry3d`, not `rapier3d`.** We want collision *queries* and
> a custom kinematic response, not a dynamics solver. `rapier` (forces,
> restitution, joints) would fight us by making things bounce and by adding
> solver nondeterminism. `parry3d` gives us raycasts, shape-casts, contact
> generation, convex decomposition, and the BVH - exactly what a space RTS needs
> - while we own the motion and response rules. (Tumbling *wreckage/debris*, if
> we add it, is a **render-only, non-deterministic** flourish that may use a
> throwaway dynamics pass which never touches `sol-sim`.)

### 6.8 Ballistics & projectiles

Per the brief, **every bullet is physical but deterministic** - no abstract
"dice-roll" hit chances on ballistic weapons.

- **Projectiles are entities** in `sol-sim` with position + velocity, integrated
  at the fixed timestep (semi-implicit Euler). They have travel time, can be
  dodged, and can miss.
- **Firing solution:** when a mount fires at a moving target it computes a
  deterministic **lead/intercept** (solve for the interception point given
  projectile speed and target velocity). Slow guns must lead fast ships; fast
  ships can juke slow rounds - emergent, skill-expressive combat.
- **Hit detection = swept query (CCD):** each tick, ray/shape-cast the
  projectile's segment of travel against candidate colliders via `parry3d`. No
  tunneling, frame-rate-independent, deterministic.
- **Weapon archetypes:**
  - *Kinetic / pulse* - discrete projectiles as above.
  - *Beam* - an instantaneous ray (still a deterministic `parry3d` raycast);
    continuous power draw.
  - *Missile / torpedo* - a **guided** projectile: deterministic
    pursuit/proportional-navigation steering toward the target, with fuel/turn
    limits (so it can be outmaneuvered or flared).
- **Impact resolution (deterministic, positional):** at the hit point we
  resolve, in order - **shield facing** → **hull section** → **subsystem** (by
  which internal collider the penetrating shot struck;
  [§6.3](#63-structure-hitboxes--positional-damage)). This is what makes "shoot
  their engines" real and ties weapons to the
  [capture loop](#5-crew-capture--research).
- **Gravity / fields:** projectile integration reads the deterministic
  environmental fields ([§8](#8-reactive-environments)) - gravity wells curve
  trajectories; magnetar/ion fields perturb or disrupt - all on the coarse CPU
  sim so every peer agrees.

### 6.9 Determinism in the physics sim (non-negotiable for multiplayer)

[Lockstep multiplayer](#13-multiplayer-peer-to-peer) requires that movement,
collision, and ballistics produce **bit-identical** results on every peer -
including **wasm vs. native**. The physics layer is the hardest part of that
promise, so we design for it from day one:

- **Fixed timestep**, fixed integration scheme, no wall-clock anywhere in
  `sol-sim`.
- **Deterministic iteration order:** ships, contacts, and projectiles are
  processed in a **canonical order** (stable entity ids / sorted contact keys),
  so resolution never depends on hash-map or thread ordering.
- **Controlled floating point:** no fast-math/FMA contraction surprises; route
  transcendentals (intercept math, trig) through the **`libm`** crate so
  `sin/cos/sqrt` match across platforms; prefer scalar paths over autovectorized
  ones where results could differ.
- **Fixed-point escape hatch:** if cross-platform `f32` proves too divergent for
  the most sensitive quantities (positions/velocities), move *those* to
  fixed-point while keeping the rest in float.
- **Seeded RNG only:** any scatter/spread is drawn from the sim's deterministic
  PRNG, never the OS.
- **Desync detection:** the sim state (all bodies + projectiles) is
  **checksummed every K ticks**; peers compare and halt+report on divergence
  ([§13](#13-multiplayer-peer-to-peer)).
- **Determinism test harness (build it early):** replay an identical command
  stream on native and on wasm, compare per-tick checksums, and fail CI if they
  drift. Catching divergence the day it is introduced is the only sane way to
  keep lockstep alive.

> This is *why* `parry3d` is used purely for queries and we own
> integration/response: it keeps every floating-point operation that affects
> gameplay inside `sol-sim`, where we can control it.

---

## 7. The Procedural Universe

The battlespace is **procedurally generated** per encounter (seeded for
reproducibility and multiplayer sync). It must feel like a *place* - vast,
volumetric, and dangerous - not a backdrop. (Ships are the exception: they are
**static, authored assets** - see [§14](#14-asset-strategy-static-vs-procedural).)

### Environment elements

| Element | Generation approach | Gameplay role |
|---|---|---|
| **Voxel nebulas** | Sparse volumetric density field (3D noise / brick grid), raymarched | Sensor occlusion, concealment, fuel for ion storms; some are flammable/charged. |
| **Dust clouds** | Lighter-density volumetric fields | Soft cover, visual depth, mild sensor scatter. |
| **3D asteroids** | Procedural meshes (displaced icospheres / marching cubes on a voxel field), instanced fields | Cover, collision hazards, Matter Units sources, line-of-sight blockers. |
| **Planets** | Procedural sphere shaders (noise terrain, atmosphere scattering) | Backdrops, gravity, mission anchors; rarely entered. |
| **Gravity wells** | Scalar/vector field over the encounter | Bend trajectories, slingshots, traps; affect missile/strike-craft paths. |
| **Pulsars** | Rotating volumetric beam + periodic EM sweep | Timed sensor/comms disruption; rhythmic hazard windows. |
| **Supernovae** | Expanding volumetric shockwave + radiation field | Map-changing event; a moving wall of death/opportunity. |
| **Magnetars** | Strong EM field viz | Disrupt shields/electronics; weapon and sensor penalties. |
| **Black holes** | Screen-space gravitational lensing + accretion volumetrics + strong gravity well | Terrain you route around; instant loss if crossed; tactical anchor. |

### Determinism & seeding

- Each encounter is generated from a **seed** (derived from the run seed + node
  id). Same seed ⇒ same universe on every peer - required for
  [lockstep multiplayer](#13-multiplayer-peer-to-peer) and for reproducible
  bug reports.
- **Two representations** of the procedural world:
  - **Gameplay/deterministic** - coarse fields and collider geometry on the CPU
    in `sol-sim` (gravity, sensor occlusion, hazard intensity). Drives
    simulation and must be identical across peers.
  - **Visual** - high-resolution volumetrics on the GPU in `sol-render`,
    derived from the same seed but allowed to be device-dependent (it never
    feeds back into gameplay). See [§11](#11-rendering-architecture-wgpu).

---

## 8. Reactive Environments

The universe is not just procedural - it is **simulated and reactive**. Player
and AI tactics perturb environmental fields, which feed back into both visuals
and gameplay. This is a headline feature.

### The canonical example: the ion storm

1. A nebula region carries a baseline **ion charge** field.
2. Firing **energy weapons** (and certain subsystem events) **injects charge**
   into the local field.
3. Past a threshold the region **ignites into an ion storm**: heightened, more
   dangerous conditions - sensor jamming, shield disruption, arc damage,
   chaotic lighting.
4. The storm is now a **shared hazard and a weapon**: bait an enemy into a
   nebula, provoke the storm with your own fire, and let it savage their
   shields while you fight on kinetics.

### How it works (architecture)

- Environmental fields (ion charge, radiation, heat, gravity) are **coarse 3D
  grids** in `sol-sim`, advanced each tick by simple, **deterministic** update
  rules (diffusion, decay, thresholds) plus **injections** from gameplay events.
- Gameplay reads these fields (e.g., sensor range *= occlusion factor; shields
  take arc damage in active storm cells).
- `sol-render` reads the *same* field state to drive **visual intensity**
  (denser, angrier volumetrics; lightning; color shifts) at high resolution.
- Because the gameplay field is coarse and deterministic, **all peers see the
  same storm evolve** from the same actions - reactivity is multiplayer-safe.

> Generalization: any "anomaly" can expose an injectable/decaying field with
> threshold behaviors. Supernova shockwaves, magnetar surges, and pulsar sweeps
> all reuse this field-simulation substrate. Build it once, reuse everywhere.

---

## 9. Camera & Controls

The control scheme is the soul of the Homeworld feel. Two systems define it:
the **command camera** and **3D order issuing** (the move-disk), plus classic
**box selection**.

### 9.1 The command camera (Homeworld "sensors sphere")

**How Homeworld actually does it (reference).** The camera is a *focus-orbit*
camera - never a free-fly/FPS camera. Its state is `(focus point, distance,
yaw, pitch)` with **world-up preserved** (the horizon never rolls) and pitch
clamped. Everything orbits a focus:

- **Focus** is re-anchored with `F` (focus on selection), `Home` (focus on the
  Mothership/flagship), cycled with `Page Up`/`Page Down` (next/previous focus),
  and `Num 0` (last combat event). Once focused, the camera *tracks* that target
  as it moves.
- **Orbit / tumble:** hold the **right mouse button and drag** to rotate around
  the focus; hold **`Alt`** to lock onto and orbit a specific entity under the
  cursor (the usual way you fly the view around a battle).
- **Zoom & camera height:** the **mouse wheel** dollies in/out. Zooming fully
  out opens the **Sensors Manager** - a flattened, abstracted tactical sphere of
  the whole battlespace where you can still select and issue orders.
- **Pan** the focus with the arrow keys (forward/back/left/right) and
  `Insert`/`Delete` (up/down).

> Exact bindings differ across Homeworld 1, 2, and Remastered and are all
> rebindable; the *model* - focus-orbit, sensors view, focus cycling - is the
> constant, and that is what we implement.

**Our model** adopts this directly:

- State = focus point + distance + yaw + pitch; world-up locked; pitch clamped
  just shy of the poles to avoid gimbal flip.
- Orbit (RMB-drag, `Alt` to re-lock), zoom (wheel), pan focus (keys/edge),
  focus-on-selection (`F`), focus-on-Ark (`Home`), cycle focus.
- Far zoom → **sensors view** (icons/vectors, readable at fleet scale; orders
  still issuable).
- Momentum-eased and framerate-independent - the camera *is* the player, so it
  must feel like an extension of intent. The focus tracks moving selections.

### 9.2 Issuing 3D orders - the move-disk

Moving in true 3D with a 2D mouse is *the* Homeworld problem, solved by the
**movement disc**. The exact Homeworld flow:

1. With ships selected, press **`M`** (Move) - a horizontal **disc** (the "blue
   circle") appears, centered on the selection at its current altitude.
2. **Move the mouse** to slide the destination across the disc - this sets the
   **X/Z** position (and how far).
3. To set **altitude (Y)**: **hold `Shift` and drag the mouse up/down** - a
   vertical guide line lifts the destination above/below the disc plane.
4. **Click** to confirm. Ships path to the 3D point, preserving formation.

A plain **right-click** also issues a context order - *move* to empty space,
*attack* on an enemy - but the `M`-disc is the precise 3D tool. We implement
both: RMB context-orders for speed, and the `M`-disc (`Shift`+vertical for
altitude) for deliberate positioning, drawing a clear gizmo (disc, altitude
pole, destination ghost, formation preview).

Supporting order tools (Homeworld defaults shown):
- **Attack-move** (`Ctrl+A`), **Waypoints** (`W`), **Stop** (`S`),
  **Guard** (`G`), **Dock** (`D`), **Hyperspace/jump** (`J`).
- **Formation & facing** on arrival; **formations** (Delta, Broad, Wall, X,
  Claw, Sphere, …).
- **Tactics stance:** *Evasive / Neutral / Aggressive* - changes how units
  engage and reposition.
- **Power presets** (our addition): per-selection energy allocation ("engines
  hot," "weapons free," "rig for silent running").

### 9.3 Selection

- **Single select:** left-click a ship.
- **Band-box (marquee):** left-click-drag a screen rectangle; selects friendlies
  whose footprint intersects it (frustum test against hull colliders for 3D
  correctness). *Prototype:* the test is against each ship's projected center
  (the full footprint/frustum test lands with colliders).
- **Mobile band-box:** hold the on-screen **Box Select** button and drag a
  second finger to sweep the rectangle (one finger alone still orbits); it
  drives the same selection path as the desktop drag.
- **Select all of type:** double-click a ship → all of that type on screen.
- **Control / hotkey groups:** `Ctrl+1…9` to bind, `1…9` to recall (double-tap
  to focus the group).
- **Modifiers:** `Shift` add, `Alt`/`Ctrl` subtract (configurable).
- **From the sensors view:** selection and orders work at fleet scale too.

**Selection & order gizmos (prototype, drawn world-to-screen per [§10](#10-ui--hud)):**
each selected ship gets a wireframe **ground circle** on the `y=0` plane plus a
vertical **elevation pole** to the hull (reading X/Z and altitude above/below the
plane), a **dashed destination line** while it has a move order, and a floating
**health bar**. A group move defaults to the class-segregated **military parade**
(carrier front-center, capitals trailing, frigates flanking, fighters/bombers on
the wings, resourcers rear) when the mothership is in the selection, and falls
back to a grid otherwise; `C` (or the HUD button) cycles Parade / Grid / Line.

### 9.4 Attack & combat orders

- **Attack:** select, right-click an enemy (or the `Attack` command). Multiple
  groups can **focus-fire** one target.
- **Attack-move** (`Ctrl+A`): advance to a point, engaging anything hostile en
  route.
- **Guard / escort** a ship or volume; **patrol** via waypoints.
- **Subsystem targeting (Homeworld 2):** against capital ships you can target
  specific **subsystems** - engines, turrets, production, sensors. This maps
  directly onto our
  [positional-damage model](#63-structure-hitboxes--positional-damage): kill
  engines to prevent escape, kill weapons to move in safely and **board**
  ([§5](#5-crew-capture--research)).
- **Capture order:** send boarding frigates at a sufficiently disabled target
  ([§5](#5-crew-capture--research)).

### 9.5 Input bindings (initial)

| Action | Default | Notes |
|---|---|---|
| Orbit camera | RMB drag (`Alt` = lock to entity) | Yaw/pitch around focus |
| Zoom / camera height | Mouse wheel | Far out → sensors view |
| Pan focus | Arrows / edge-scroll; `Ins`/`Del` up-down | Move focus point |
| Focus on selection | `F` | Re-anchor + track |
| Focus on Ark | `Home` | The mothership |
| Cycle focus | `PgUp`/`PgDn`; `Num0` last event | |
| Select / band-box | LMB / LMB-drag | Click / marquee |
| Select all of type | Double-click | On-screen |
| Move (disc) | `M`, then mouse; `Shift`+vertical = altitude | The 3D move tool |
| Context order | RMB | Move on empty / attack on enemy |
| Cycle formation | `C` | Prototype cycles Parade / Grid / Line (Delta/Broad/Wall/X … later); also a HUD button for touch |
| Attack-move | `Ctrl+A` | |
| Waypoints / Stop / Guard / Dock / Jump | `W` / `S` / `G` / `D` / `J` | |
| Control group set/recall | `Ctrl+N` / `N` | |
| Tactics stance | hotkey cycle | Evasive/Neutral/Aggressive |
| Power preset | `1`-`4` on hotbar | Per-selection allocation |

> Bindings are data-driven and fully rebindable; defaults mirror Homeworld
> Remastered where applicable. Controllers are out of scope for the slice, but
> the input layer must not hard-code mouse assumptions, and **touch is a required
> test harness** (see [§9.6](#96-mobile-test-harness-input-parity)).

**Prototype bindings (current build).** The table above is the Homeworld-faithful
target. The prototype wires a smaller, hands-on scheme and will migrate toward
the target as the order set lands:

| Action | Current binding |
|---|---|
| Pan focus (XZ) | **WASD** / arrow keys / screen-edge scroll; mobile **joystick** |
| Elevation (focus Y) | **`Q`** down / **`E`** up; mobile **elevation slider** |
| Orbit / zoom | RMB drag / wheel (desktop); one-finger drag / pinch (touch) |
| Select / band-box | LMB click / LMB-drag; mobile tap / hold **Box Select** + drag |
| Move (context order) | RMB on empty space; mobile tap on empty space |
| Attack a target | RMB a hostile ship; mobile tap a hostile (with ships selected) |
| Set stance | the **Stance dropdown** (Passive / Defensive / Aggressive, or **Mixed** when the selection disagrees), or **`N`** to cycle |
| Cycle formation | **`C`** or the **Form:** HUD button |
| Select all (player) | **`Shift+A`** or the **Select All** button |
| Stop | **`Shift+S`** or the **Stop** button |
| Clear selection | **`Esc`** or the **Clear** button |
| Open build overlay | **Build** HUD button (Esc / **Close** to exit); build mode hides the in-world controls + camera pad |
| Focus camera on ship | **`F`** or the **Focus** button: locks onto the selected ship (or the mothership); any pan unlocks |
| Sensors-manager view | **`V`** or the **Sensors** button: dims the scene, draws the tactical grid + sensor spheres |
| Pause / resume | **`Space`** or the **Pause** button (works any time, incl. over the build overlay) |

**Pause is single-player, local, and active.** It stops *advancing* the
fixed-step sim (no movement, harvest, or build progress; the camera and UI keep
working), but **orders still take effect immediately**: you can pause, queue
builds and moves (matter debits and the build queue updates right away), then
resume to watch them play out. Mechanically, pause keeps applying queued commands
each frame (`World::apply_commands`) without calling `World::step`. It does not
pause "the world" for anyone else; lockstep multiplayer will need a synchronized
pause command instead.

**Build overlay (Homeworld-style).** The previewed ship is a slow auto-spinning
turntable; drag it (mouse or finger, on the empty centre of the overlay) to
inspect it from any angle, and after a short idle it eases back into the spin.
The previewed ship sits on a blue light-to-dark **horizon gradient** backdrop
(the nebula is hidden in build mode). Build mode hides the in-world
movement/selection controls and the mobile camera pad (there is nothing to move
there).

**Parallel build lanes.** Production runs in **four parallel lanes by hull
size** (`BuildLane`): **Small** (Fighter, Bomber), **Medium** (Corvette,
Salvager, Resourcer), **Large** (general/missile Frigate), **Capital**
(Destroyer). Each lane advances and completes its own front item independently
in `World::step`, so a stream of fighters never stalls a frigate. The build
menu is the **ship list, segregated into those four sections**; **clicking a
ship row queues one** of that class into its lane (and previews it). Each row
carries its own state: a progress "slider" fill while that class is the lane's
active build, and a yellow **xN** badge counting how many of that class are
queued. Matter is still debited up front and crew is reserved across all lanes
(a queued build can't over-commit the roster). The enemy commander keeps a
single serial queue (it is AI pacing, not a player-facing UX).

**Camera focus lock.** `F` (or the **Focus** button) anchors the camera on the
selected ship, or the mothership when nothing is selected, and tracks it each
frame; orbit and zoom still work, so you circle the locked ship. Any pan input
(`WASD`/arrows, edge-scroll, or the mobile joystick) releases the lock, matching
the target table's focus-on-selection role for `F`.

**Sensors, fog of war, and enemy AI (current build).** A first slice of the
[sensors model](#65-sensors-emissions--fog-of-war) and the AI is in: each ship
class carries a `sensor_range` (a `Blueprint` field, deterministic), an enemy
strike group (`Team::Enemy`) spawns inbound, and a **movement-only** enemy AI
runs in `World::step` (deterministic: it reads each step's start positions,
writes only orders, iterates in stable id order). Each enemy steers toward the
nearest player ship inside its sensor range, and advances on the mothership when
it has no direct contact. **Fog of war** is enforced in the view: an enemy is
drawn only while inside some player ship's sensor sphere (a local-UI computation
that never feeds the sim). The **Sensors-manager view** (`V` / the **Sensors**
button) pulls the camera out to a battlefield overview (restoring the prior
framing on exit), dims the 3D scene with a full-screen quad drawn beneath the
world-space gizmos (`Renderer::set_scene_dim`), draws a tactical grid, and shows
each player ship's sensor range as a translucent **sensor-field sphere**
(`Renderer::set_sensor_spheres`). Those spheres are rendered inverted (front
faces culled, far shell shown) with depth write, so overlapping ranges merge into
one solid translucent color instead of stacking alpha. Ships collapse into
**blips** the way the Homeworld sensors manager does: strike craft and frigates
become a team-colored blip sized by class (a ring at altitude plus a pole to the
plane), while the **mothership and capital ships keep their model** as readable
anchors.

**Combat (current build).** A first slice of [ballistics](#68-ballistics--projectiles)
is in, all in the deterministic sim. Each armed class carries a `Weapon` (a
`Blueprint` field) of one of three archetypes: **Ballistic** (fast unguided
rounds), **Bomb** (slow, heavy, short-ranged, on bombers), and **Missile**
(guided, homing with a turn limit, on missile frigates); resourcers and the
carrier are unarmed. **Projectiles are entities** in `World` integrated at the
fixed step: unguided shots fly on a deterministic **lead/intercept** solution,
missiles steer toward their target, and hits use a **swept segment-vs-sphere**
test (CCD, no tunneling). Every ship **auto-engages** the nearest hostile in
weapon range and fires on cooldown; the enemy AI now closes to a weapon-range
standoff instead of ramming. Players can also issue a manual **attack order**
(RMB or tap a hostile): the selected ships pursue that target to weapon range and
focus fire (`Command::Attack`), shown by a red lead line and reticle. Larger
ships carry a **shield** (`max_shield` + recharge after a no-hit delay) that
absorbs damage before the hull, **modulated by facing** ([§6.8](#68-ballistics--projectiles)):
a front hit is fully blocked while a rear hit bypasses the shield entirely, so
flanking matters. Overflow (and all damage to unshielded strike craft) hits the
**hull**, and a ship at zero hull despawns. Every ship carries a **combat
stance** (`Command::SetStance`; HUD dropdown for the selection, `N` to cycle):
**Passive** never auto-engages (only fires on a direct attack order); **Defensive**
auto-engages hostiles in weapon range, **broadcasts SOS** so nearby allies adopt
the attacker as their target, and **returns to its anchor** (the last move-order
point, or the spot where stance was set) when the fight ends; **Aggressive**
auto-acquires out to sensor range and chases targets down. Mother ships and
harvesters spawn **Passive** by default so they never wander off to fight; the
rest spawn Defensive. When a non-combat ship is hit it **flees toward the
nearest armed ally**; when an armed Defensive/Aggressive ship is hit without a
commanded target, it **retaliates against the attacker** (so an enemy combat
ship pulled off a non-combat target the moment a real threat opens fire on it).

**Enemy commander AI (current build).** The sim runs a two-level AI: the
per-ship stance behavior above, plus a strategic **commander** that owns the
enemy team's economy and coordinates raids. The enemy team has its own
`enemy_matter` pool and `enemy_build_queue`; each step the commander:
**dispatches** idle harvesters to the nearest live resource node; **queues a
build** (rotating Fighter / Corvette / Bomber) when the line is idle, matter
allows, and the armed-enemy count is below `ENEMY_FORCE_CAP`; **forms a raid
wave** by pointing every idle, healthy armed enemy at the player mothership
once `WAVE_SIZE_MIN` are free (reinforcements join while the wave's target
lives, and the wave dissolves on target death); and **orders low-hull combat
ships back** to the enemy mothership to recover. SOS now works for both teams,
so enemy escorts respond to attacks on their own harvesters. An enemy whose
current target is a non-combat ship **switches** to the attacker the moment a
player combat ship opens fire on it. Player-commanded attack orders stay
locked. (Hull sections and subsystem disable
from [§6.3](#63-structure-hitboxes--positional-damage) are still deferred; facing
is the first positional step.) Combat VFX read from a deterministic event
stream the sim emits each step (`World::drain_combat_events`): the renderer
turns those into **muzzle flashes** on fire, an orange spark on a hull hit, a
**blue expanding shield ripple** on a shield hit, projectile tracers + glints in
flight, and an expanding burst when a ship dies. Health bars now appear over any
damaged ship (blue shield over red hull), not just selected ones.

Because `WASD` pans here, the target table's `W/S/D` order keys (waypoint / stop /
dock) are not yet bound; `Stop` and `Select All` move to `Shift+S` / `Shift+A`
(holding `Shift` shows an on-screen hint).

### 9.6 Mobile test harness (input parity)

The target experience is desktop (mouse + keyboard), but **every gameplay
feature must also be exercisable in a phone/tablet browser**, because mobile
testing massively shortens iteration. Mobile is a **test harness, not a polished
touch UX**: keep it minimal but always functional.

- A lightweight **DOM control overlay** (HTML buttons/menus outside the wgpu
  canvas) plus basic **touch input** (tap to select/move) drive the *same*
  wasm-exported commands as the desktop path; there is no parallel gameplay
  logic.
- The move-disk altitude gesture stays desktop-only for now; mobile substitutes
  a simple altitude control.
- On-device debugging uses **eruda** (a mobile DevTools console) injected in the
  game's `index.html`; it loads only on touch devices, or on any device when the
  URL carries `?eruda`.
- Gameplay on touch is **landscape-only**: in portrait the game shows a button
  that, on tap (the required user gesture), enters fullscreen and locks the
  orientation to landscape (best-effort; some browsers, e.g. iOS, ignore the
  lock).
- **Rule:** never ship a gameplay feature that can only be tested on desktop.

---

## 10. UI / HUD

The brief calls for a **simple, immediate-mode GUI** layered on wgpu. We use
**`egui`** via **`egui-wgpu`** (+ `egui-winit`):

- Immediate mode → trivial to iterate, no retained-widget bookkeeping, excellent
  for tools and debug overlays.
- First-class **web + desktop** support, renders in our wgpu pass.
- Same toolkit powers in-game HUD **and** debug/dev panels (entity inspector,
  field visualizers, netcode stats).

### HUD surfaces (slice → full)

- **Selection panel:** selected ships, health, **power-allocation bars**, crew,
  subsystem status.
- **Command hotbar:** orders + power presets.
- **Tactical overlay:** velocity vectors, move-disk gizmo, sensor ranges,
  hazard fields (toggleable), threat indicators.
- **Sensors/minimap (3D-aware):** a readable representation of the volumetric
  battlespace and contacts.
- **Run/Resource bar:** Matter Units, Power Cells, Crew, Intel; Ark status &
  build queue.
- **Dev overlays (debug builds):** ECS inspector, field heatmaps, frame/sim
  timing, network desync checksums.

> 2D in-world labels (ship names, health pips, order lines) are drawn as part of
> the tactical overlay, projected from world to screen. Keep the HUD legible at
> both close and sensors-view zoom.

---

## 11. Rendering Architecture (wgpu)

**Web-first** dictates the rendering plan. We target **WebGPU** as the primary
backend (Chrome/Edge stable; Firefox and Safari shipping support through
2024-2025) with a **reduced-quality WebGL2 fallback** that `wgpu` provides
transparently for raster, plus graceful feature-detection for the parts that
need compute.

### Backend tiers

| Tier | Backend | Volumetrics / fields | Notes |
|---|---|---|---|
| **High** | WebGPU (web), Vulkan/Metal/DX12 (native) | Compute-driven volumetrics & field updates, full effects | Primary target. |
| **Fallback** | WebGL2 | Fragment-shader raymarch at low res, simpler procedural visuals, **no compute** (visual field updates approximated; gameplay fields always on CPU) | Keep it playable, not pretty. |

### Render graph (high tier)

```
1. Depth pre-pass            (opaque ships, asteroids, planets)
2. Opaque G-buffer / forward (PBR ships [static GLB], asteroids, planets)
3. Volumetric pass           (half-res raymarch: nebulas, dust, anomaly fx;
                              reads density/field textures; temporal reproject)
4. Transparent & particles   (engine trails, weapon fx, debris)
5. Composite + post          (volumetric upscale/composite, bloom,
                              black-hole lensing, tonemap, color grade)
6. UI overlay                (egui + tactical overlay)
```

### Key techniques

- **Volumetric nebulas/dust:** raymarch a sparse density field (3D textures /
  brick grid). **Half-resolution** volumetric buffer + **temporal
  reprojection** + bilateral upscale to stay within web GPU budgets. LOD by
  distance and by camera zoom (sensors view simplifies).
- **Procedural meshes (asteroids):** generated on the GPU/CPU at encounter load,
  **instanced** across asteroid fields. (Asteroids are a permitted procedural
  exception - see [§14](#14-asset-strategy-static-vs-procedural).)
- **Planets:** sphere shaders - noise terrain, atmospheric scattering - runtime.
- **Anomalies:** black-hole **lensing** as a screen-space post effect over an
  accretion volumetric; pulsar beams and supernova shells as animated
  volumetrics driven by the [reactive fields](#8-reactive-environments).
- **Ships:** static **GLB** meshes, **PBR** materials with **statically baked
  textures** (hard project rule, [§14](#14-asset-strategy-static-vs-procedural)),
  GPU instancing for same-class hulls.
- **Determinism boundary:** rendering never feeds gameplay. Visual field
  resolution/timing may differ per device; gameplay reads only the coarse
  deterministic CPU fields.

> Performance budget is a first-class constraint. The volumetric pass is the
> biggest risk on web; it gets an explicit, tunable budget (resolution scale,
> step count, max active anomaly fields) exposed in dev settings.

---

## 12. Technical Architecture

### Language & crate stack

| Concern | Choice | Why |
|---|---|---|
| Language | **Rust** | Brief; safety + wasm story. |
| GPU | **wgpu** | Brief; one API for WebGPU + native, WebGL2 fallback. |
| Windowing/input | **winit** | De-facto standard; web + desktop. |
| Math | **glam** | SIMD, the gamedev standard. |
| ECS | **bevy_ecs** (standalone) | Scales to the rich subsystem/crew/field sim; systems + scheduling + change detection. (`hecs` is a lighter alternative if we want minimalism.) |
| GUI | **egui** + `egui-wgpu` + `egui-winit` | Immediate mode; web + desktop; HUD + tools. |
| Collision | **parry3d** | Raycasts, shape queries, convex hulls; no forced rigid-body sim. |
| Net transport | **matchbox** (WebRTC) | True browser P2P; native too. See [§13](#13-multiplayer-peer-to-peer). |
| Serialization | **serde** (+ `bincode`/`rkyv` for compiled runtime data, JSON for authoring) | Authoring is JSON (editor interop); runtime is fast binary. |
| Determinism helpers | `libm` for fixed math; fixed-timestep loop | Cross-platform determinism for lockstep. |
| wasm build | **trunk** | Bundles wasm + assets + `index.html`; great DX. |

> **Why custom-wgpu and not Bevy (the engine)?** The brief asks for wgpu
> specifically, and reactive volumetric rendering is a core pillar we want to
> own. So we hand-roll the **renderer** while borrowing mature crates for
> everything that isn't rendering (including `bevy_ecs` as a *library*). If the
> renderer becomes a sink, full Bevy is the documented fallback - but that is a
> deliberate, later decision, not the default.

### Crate layout (Cargo workspace)

```
sol-app      → entry point (native bin + wasm); winit + wgpu + egui wiring,
               game states, input, glue. The only crate that knows about both
               render and sim.
sol-sim      → DETERMINISTIC simulation: ECS world, fixed-timestep tick,
               power grid, subsystems, crew, collision queries, env fields,
               procedural-gameplay generation. NO rendering, NO wall-clock,
               NO OS RNG. This crate is the multiplayer source of truth.
sol-render   → wgpu renderer: render graph, volumetrics, instancing, post,
               procedural VISUALS. Reads sim state; never writes it.
sol-procgen  → seeded generators shared by sim (gameplay geometry/fields) and
               render (visual detail). Deterministic core + visual elaboration.
sol-net      → P2P lockstep over matchbox: command collection, turn scheduling,
               input delay, desync detection (state checksums).
sol-assets   → runtime loading of COMPILED ship data (.ship binary) + GLB
               meshes/colliders; asset registry.
sol-shipc    → CLI tool: compiles authored ship JSON + GLB into runtime assets,
               validates colliders & sim data. Runs in CI. See [§15].
```

Dependency direction: `sol-app` → {`sol-sim`, `sol-render`, `sol-net`,
`sol-assets`}; `sol-render`/`sol-sim` → `sol-procgen`; `sol-sim` depends on
**nothing graphical**. Keeping `sol-sim` pure is what makes lockstep and tests
possible.

### Simulation loop

- **Fixed timestep** (e.g., 30 Hz sim) decoupled from render framerate;
  render **interpolates** between the last two sim states for smoothness.
- All gameplay randomness flows from a **seeded, deterministic RNG** owned by
  `sol-sim`. No `SystemTime`, no thread-RNG, no unordered iteration in the sim.
- This loop is identical in single-player and multiplayer; multiplayer simply
  feeds it [synchronized commands](#13-multiplayer-peer-to-peer).

---

## 13. Multiplayer (Peer-to-Peer)

> **Status: draft / forward-looking.** Single-player and the
> [vertical slice](#19-vertical-slice-milestone-0) come first. But because
> determinism is *architectural*, we design the sim for it from day one - retro-
> fitting determinism later is brutal.

### Model: deterministic lockstep

RTS with few units → the classic, proven approach is **deterministic lockstep**
(the *Age of Empires* model), not state replication or per-entity rollback:

- Every peer runs the **identical** `sol-sim` on the **same seed**.
- Peers exchange only **commands** (orders), not world state - tiny bandwidth,
  scales with players not entities.
- Commands are scheduled **N turns in the future** (input delay, e.g.,
  100-250 ms of "turns") so all peers have every command before simulating that
  turn. The sim advances a turn only when all peers' commands for it are in.

```mermaid
sequenceDiagram
    participant P1 as Peer 1
    participant P2 as Peer 2
    Note over P1,P2: Turn T: collect local orders
    P1->>P2: commands(T) + checksum(T-k)
    P2->>P1: commands(T) + checksum(T-k)
    Note over P1,P2: When all commands(T) received → simulate turn T
    Note over P1,P2: Compare checksums → detect desync early
```

### Transport: WebRTC via `matchbox`

- **`matchbox`** gives browser-to-browser **WebRTC data channels** (and works
  natively), with a lightweight **signaling server** (`matchbox_server`) used
  only to introduce peers - gameplay traffic is peer-to-peer.
- Unreliable-ordered channels for command turns (with our own small
  retransmit/ack for the turn protocol); reliable channel for lobby/handshake.
- For small skirmishes, **`ggrs`** (GGPO-style rollback) over matchbox is a
  documented alternative; but for RTS-scale entity counts we default to
  **delay-based lockstep**, which avoids re-simulating thousands of entities.

### The hard part: determinism

Lockstep lives or dies on **bit-identical simulation across machines** -
including **wasm vs. native** and different GPUs/CPUs:

- **Fixed timestep**, fixed iteration order (stable entity ordering), seeded RNG.
- **Controlled floating point:** avoid fast-math; route transcendentals through
  the **`libm`** crate so `sin/cos/sqrt` match across platforms; beware FMA
  contraction differences. Consider **fixed-point** for the most sensitive
  quantities (positions/velocities) if f32 proves too divergent.
- **No nondeterminism in `sol-sim`:** no wall-clock, no hash-map iteration order,
  no floating-point from the renderer crossing back in.
- **Desync detection:** each peer hashes a **state checksum** every K turns and
  exchanges it; mismatch → halt, report the turn, and dump state for diffing.
- **Recovery:** on desync, fall back to a one-time **state resync** from an
  agreed peer (designated by lowest peer-id) - a pragmatic safety net for a P2P
  game without a server.

### Security caveats (be honest)

- **P2P lockstep has no authority**, so it is vulnerable to **"maphack"-style**
  information cheats (a modified client can ignore fog of war) - a known,
  accepted RTS trade-off. We mitigate *state-altering* cheats because divergence
  trips the checksum, but we cannot fully prevent passive information cheats
  without a server.
- A later **optional dedicated/host-authoritative mode** is the answer for
  ranked/competitive play; the lockstep core is the LAN/friends/co-op default.

---

## 14. Asset Strategy: Static vs Procedural

This is a **hard project rule** (mirrored in `CLAUDE.md`):

> **All game models and their textures MUST be statically generated
> (authored + compiled ahead of time).**
> **Exceptions - generated procedurally at runtime - are: the space
> environment (nebulas, dust, anomalies, fields), planets, and (optionally)
> asteroids.**

| Asset | Static (authored + compiled) | Procedural (runtime) |
|---|---|---|
| Ships & ship textures | ✅ **required** | ❌ never |
| Ship subsystems / hardpoints / colliders | ✅ **required** | ❌ |
| Stations / structures (treated as ships) | ✅ | ❌ |
| Nebulas, dust clouds, env fields | ❌ | ✅ |
| Planets | ❌ | ✅ |
| Asteroids | allowed either way | ✅ (preferred) |
| Anomalies (pulsar/supernova/black hole/magnetar) | ❌ | ✅ |

**Why this split:**
- **Ships are gameplay-critical and net-synced.** Their geometry, colliders,
  hardpoints, and sim data must be *identical, validated, and versioned* across
  all peers and builds. Authored+compiled assets give us that guarantee; runtime
  procedural ship generation would threaten determinism and balance.
- **Environments benefit from variety and reactivity.** Procedural nebulas and
  anomalies give endless, seedable, *reactive* battlespaces - and they don't
  need to be bit-identical *visually*, only their coarse gameplay fields do
  (which are generated deterministically from the seed in `sol-sim`).

---

## 15. The Ship Asset Pipeline

Ships are **authored** (in the [web editor](#16-web-based-ship-editor)) and
**compiled** (by `sol-shipc`) into runtime assets. The pipeline is the bridge
between the human-friendly editor and the deterministic engine.

### Model format decision: **GLB** (not OBJ)

We use **glTF 2.0 binary (`.glb`)** for ship meshes. Rationale:

| | **GLB (chosen)** | OBJ |
|---|---|---|
| Single file | ✅ mesh + materials + textures + hierarchy + animation in one binary | ❌ `.obj` + `.mtl` + loose textures |
| PBR materials | ✅ native PBR | ❌ MTL is ad-hoc, no real PBR |
| Scene graph / nodes | ✅ (perfect for hardpoint/subsystem anchors) | ❌ flat |
| Animation / skinning | ✅ (turrets, bays, future) | ❌ |
| Web tooling | ✅ Three.js `GLTFLoader`/`GLTFExporter` | ⚠️ limited |
| Rust loading | ✅ `gltf` crate | ⚠️ basic |
| Compactness/parse | ✅ binary, fast | ❌ verbose text |

The editor **exports GLB**; **glTF node names/extras** carry hardpoint and
subsystem anchors so geometry and sim data stay spatially aligned.

### Authoring artifacts (committed to the repo)

For each ship, under `assets/ships/<ship_id>/`:

- `model.glb` - geometry + PBR materials + **baked static textures** + named
  anchor nodes (hardpoints, subsystem mounts, shield-facing origins).
- `ship.json` - the **ship definition**: metadata, subsystems, hardpoints,
  colliders, power network, crew requirements, balance stats. (Authoring format
  = JSON for editor interop.)

### Ship definition (authoring schema, abridged)

```jsonc
{
  "id": "vaered_frigate_ion",
  "name": "Vaered Ion Frigate",
  "faction": "vaered",
  "class": "frigate",
  "model": "model.glb",
  "mass": 4200,
  "crew": { "min": 18, "optimal": 30 },

  "hull": { "sections": ["bow", "port", "stbd", "stern", "core"],
            "integrity": 1200 },

  "colliders": [                         // authored in-editor; validated by shipc
    { "type": "convexHull", "node": "hull_collision" },
    { "type": "box", "node": "engine_mount_collider" }
  ],

  "shields": { "facings": 6, "capacity": 800, "regen": 40 },

  "powerGrid": {
    "reactor": { "node": "reactor_core", "output": 100 },
    "capacitors": [ { "node": "cap_fwd", "capacity": 120 } ],
    "conduits": [ { "from": "reactor_core", "to": "bus_main", "capacity": 120 } ],
    "subsystems": [
      { "id": "engines", "node": "engine_mount",  "draw": 35, "priority": 2 },
      { "id": "shields", "node": "shield_emitter","draw": 30, "priority": 3 },
      { "id": "sensors", "node": "sensor_array",  "draw": 10, "priority": 1 },
      { "id": "lifeSupport","node": "life_support","draw": 6, "priority": 0 }
    ]
  },

  "hardpoints": [
    { "id": "ion_beam_1", "node": "hp_ion_1", "mount": "turret",
      "weapon": "ion_beam_mk2", "arc_deg": 220, "drawPerShot": 25 }
  ]
}
```

### Compilation (`sol-shipc`, runs in CI)

```mermaid
flowchart LR
    A[assets/ships/*/model.glb] --> C[sol-shipc]
    B[assets/ships/*/ship.json] --> C
    C --> V{Validate}
    V -->|colliders watertight/convex,<br/>nodes exist, power graph valid,<br/>crew/draw sane| OK[OK]
    V -->|fail| ERR[Build fails - bad asset]
    OK --> D[(compiled/ship_id.ship<br/>binary: geometry refs + colliders<br/>+ sim data, content-hashed)]
    OK --> E[(compiled/ship_id.glb<br/>optimized/validated mesh)]
```

`sol-shipc`:
- **Validates** colliders (watertight/convex, performs convex decomposition if
  needed), checks that every referenced glTF node exists, that the power graph
  is connected and sane, and that crew/draw numbers are within bounds.
- **Emits** a compact **runtime binary** (`bincode`/`rkyv`) bundling collider
  shapes + power network + hardpoints + crew data, **content-hashed** for cache-
  busting and version pinning, plus an optimized/validated `.glb`.
- Is the **gate that keeps bad ships out of the build** - and, crucially, keeps
  ship data **identical across all peers** for lockstep.

> The compiled outputs live in `assets/compiled/` (git-ignored) and are produced
> fresh by CI on every build, so the authored JSON+GLB are the only source of
> truth in version control.

---

## 16. Web-Based Ship Editor

A standalone **browser tool** (Three.js / WebGL + TypeScript) for authoring
ships: geometry import/arrangement, **collider configuration**, hardpoint/
subsystem placement, and sim-data editing. It is deliberately **separate** from
the Rust game (different language, different cadence) but shares the **`ship.json`
schema** ([§15](#15-the-ship-asset-pipeline)) as the contract between them.

### Why Three.js here (and wgpu in-game)

- The editor needs rich, off-the-shelf 3D-editing UX (gizmos, transform
  controls, GLTF import/export) where Three.js is mature and fast to build in.
- The game needs custom volumetrics and a deterministic core where wgpu/Rust
  wins. Different tools for different jobs; the **`ship.json` + GLB contract**
  keeps them aligned.

### Editor features

- **Import** a base mesh (GLB/OBJ) or assemble from primitive/library parts.
- **Hardpoints & subsystems:** place named anchor nodes (turrets, reactor,
  engines, sensors, shields, life support, bridge) with transforms; these become
  glTF node names/extras.
- **Collider authoring:** build the hull collider (convex hull from mesh, or
  primitives), plus per-subsystem colliders; **visualize** them; flag
  non-convex/non-watertight geometry *before* it reaches `sol-shipc`.
- **Power network editor:** wire reactor → bus → capacitors → subsystems; set
  capacities, draws, priorities; live-validate connectivity.
- **Sim data:** crew requirements, shield facings/capacity, hull sections,
  balance stats.
- **Live preview:** orbit the ship, toggle colliders/hardpoints/power overlay,
  sanity-check facings and arcs.
- **Export:** writes `model.glb` (via `GLTFExporter`) + `ship.json` ready to
  commit under `assets/ships/<id>/`. (Optionally a "download bundle" + a
  paste-into-repo flow.)

### Deployment

The editor is **built and deployed alongside the game** by the same CI
([§17](#17-build--deployment-pipeline)), served at `/<branch>/editor/` (and the
default branch at `/editor/`). Designers get a live editor URL on every commit,
no install.

---

## 17. Build & Deployment Pipeline

**Continuous delivery, no gates.** Per the brief: **every commit on any branch
builds and deploys to GitHub Pages**, with **no main-branch gate**. We deploy
via the official **GitHub Actions Pages** flow (Pages source = "GitHub
Actions"), which serves a **single live site**: the most recent push wins. This
mirrors the pattern used by the sibling `rts-engine-c` project. Each push is one
run: a build job, then a deploy job, so nothing competes with the deploying
push run.

### What gets built

1. **Asset compile:** `sol-shipc` over `assets/ships/**`, output to
   `assets/compiled/`.
2. **Game (wasm):** `trunk build --release` of `sol-app`, producing the web
   bundle (wasm + JS glue + assets) in `crates/sol-app/dist`.
3. **Ship editor:** `vite build` of `editor/` (Three.js/TS) to `editor/dist`.
4. **Assemble + upload:** game at the site root, editor under `editor/`,
   uploaded as a single Pages artifact.

### Deploy layout (GitHub Pages)

The whole site is published at the project root and replaced on every push:

```
https://<user>.github.io/<repo>/
  index.html             the wasm game
  sol-app-<hash>.js       wasm-bindgen glue + the wasm module
  editor/                 the web ship editor
```

- **Single shared deployment.** Every push (any branch) rebuilds and replaces
  the live site; the latest commit wins. This honors "build and deploy on every
  commit, no gate" while keeping one canonical URL.
- **Relative asset paths.** `trunk` uses `public_url = "./"` and the editor uses
  Vite `base: "./"`, so the bundle loads correctly under the project-site path
  `/<repo>/`.
- **Official Actions flow.** A `build` job uploads the assembled site via
  `actions/upload-pages-artifact`; a `deploy` job publishes it with
  `actions/deploy-pages` to the `github-pages` environment.

> Trade-off: the official Pages-Actions deploy publishes one artifact as the
> whole site, so we do not keep separate `branch/<slug>/` preview URLs. If we
> later want previews, branch-based serving (`peaceiris/actions-gh-pages` to a
> `gh-pages` branch) is the alternative, at the cost of leaving "Actions" mode.

### Workflow sketch (`.github/workflows/deploy.yml`)

```yaml
name: build-and-deploy
on:
  push:
    branches: ["**"]        # every branch, every commit, no gates
  workflow_dispatch:
permissions:
  contents: read
  pages: write
  id-token: write
concurrency:
  # Keyed by ref: only a newer push to the SAME ref cancels an older in-flight
  # run. Guards overlapping pushes; one run is always build then deploy.
  group: pages-${{ github.ref }}
  cancel-in-progress: true
jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with: { targets: wasm32-unknown-unknown }
      - uses: Swatinem/rust-cache@v2
      - uses: jetli/trunk-action@v0.5.0
      - uses: actions/setup-node@v4
        with: { node-version: 20 }
      - run: cargo run -p sol-shipc -- build assets/ships --out assets/compiled
      - run: cd crates/sol-app && trunk build --release
      - run: cd editor && npm ci && npm run build
      - run: |
          mkdir -p site_build/editor
          cp -r crates/sol-app/dist/. site_build/
          cp -r editor/dist/. site_build/editor/
      - uses: actions/upload-pages-artifact@v3
        with: { path: site_build }
  deploy:
    needs: build
    runs-on: ubuntu-latest
    environment:
      name: github-pages
      url: ${{ steps.deployment.outputs.page_url }}
    steps:
      - id: deployment
        uses: actions/deploy-pages@v4
```

### Setup & caveats

- **Pages source must be "GitHub Actions"** (Settings, Pages, Build and
  deployment, Source: GitHub Actions). Nothing serves until this is set.
- **Environment branch rules.** If a non-default branch is blocked ("Branch not
  allowed to deploy to github-pages"), allow it under Settings, Environments,
  `github-pages`, Deployment branches.
- **WebGPU + threads:** enabling wasm threads needs **COOP/COEP** for
  `SharedArrayBuffer`; GitHub Pages cannot set headers, so we would ship the
  `coi-serviceworker` shim. Single-threaded web needs none of this.
- **WebGPU availability:** feature-detect and show a friendly message (with the
  WebGL2 fallback) on browsers without WebGPU.
- **Cache busting:** `trunk` hashes wasm/asset filenames; compiled `.ship`
  assets are content-hashed by `sol-shipc`.
- **Build speed:** `Swatinem/rust-cache` keeps every-commit builds fast.

> CI builds and deploys on every commit, intentionally without correctness
> gates. We may run tests/lint as a separate, non-blocking job for signal, but
> they never stop a deploy, per the brief.

---

## 18. Repository Structure

```
sea-of-lost-souls/
├── Cargo.toml                  # workspace manifest
├── rust-toolchain.toml         # pinned toolchain (+ wasm32 target)
├── design.md                   # this document
├── CLAUDE.md                   # agent/contributor rules (incl. asset rule)
├── README.md
│
├── crates/
│   ├── sol-app/                # bin: native + wasm entry (winit/wgpu/egui)
│   │   ├── index.html          # trunk entry for web
│   │   └── src/
│   ├── sol-sim/                # deterministic simulation (ECS, sim, fields)
│   ├── sol-render/             # wgpu renderer, volumetrics, post
│   ├── sol-procgen/            # seeded universe generation (sim + visual)
│   ├── sol-net/                # P2P lockstep over matchbox
│   ├── sol-assets/             # runtime asset loading (.ship + glb)
│   └── sol-shipc/              # CLI: compile + validate ship assets
│
├── editor/                     # web ship editor (Three.js + TS + Vite)
│   ├── package.json
│   └── src/
│
├── assets/
│   ├── ships/                  # AUTHORED ship defs (committed)
│   │   └── <ship_id>/
│   │       ├── model.glb
│   │       └── ship.json
│   └── compiled/               # build output (gitignored)
│
├── shaders/                    # wgsl shaders (volumetrics, pbr, post)
│
└── .github/
    └── workflows/
        └── deploy.yml          # build + deploy every commit, every branch
```

---

## 19. Vertical Slice (Milestone 0)

The first **simple slice** - deliberately small, proving the spine end-to-end.
Three deliverables, all live on GitHub Pages from the first commit:

### A. In-engine tactical slice (`sol-app` + `sol-sim` + `sol-render`)

- A handful of **placeholder ships** (a couple of compiled GLB hulls) drifting
  in a 3D void with a starfield + one simple procedural dust cloud.
- **Homeworld camera:** orbit / zoom / pan / focus-on-selection
  ([§9.1](#91-the-command-camera-homeworld-sensors-sphere)).
- **Box selection** of ships + click/double-click + control groups
  ([§9.3](#93-selection)).
- **Move-disk 3D orders** ([§9.2](#92-issuing-3d-orders--the-move-disk)): select,
  set X/Z on the disk, drag for altitude, ships move there with simple
  Newtonian-ish steering on the **fixed-timestep sim** with render
  interpolation.
- **Matter Units gathering (prototype economy):** a seeded belt of **procedural
  asteroids** (displaced icospheres, instanced; the permitted procedural case,
  [§14](#14-asset-strategy-static-vs-procedural)) acts as matter sources. With a
  **resourcer** selected, clicking an asteroid sends it to harvest; it fills its
  cargo, returns to the mothership to deposit, and the banked **Matter Units** counter
  rises. All gather/harvest/deposit state lives in the deterministic sim (node
  amounts and the matter pool are in the checksum); asteroid positions/amounts
  are seeded so they are identical across peers.
- **Production (prototype build menu):** a full-screen **Build** overlay (opened
  from the HUD) lists the ship classes down a sidebar, grouped into the four
  **size-class sections** (Small / Medium / Large / Capital), and shows the
  most-recently-clicked one as a slowly rotating **3D preview** on a blue
  horizon-gradient backdrop (the wgpu canvas renders it behind the overlay's
  transparent centre while in-world input is suspended). A build needs both
  **matter and free crew** ([§5](#5-crew-capture--research)); unaffordable
  options are greyed. **Clicking a ship row queues one** of that class; matter
  is debited up front (a `Build` command) and the build drops into that class's
  **lane**. The four lanes accrue build time **in parallel** each fixed step,
  and on completion the ship spawns next to the carrier at a spread angle. Each
  row shows its own progress fill + queued **xN** count. The lanes and per-item
  progress are deterministic sim state (in the checksum); the menu, like all
  controls, drives the same command path on desktop and touch. A HUD readout
  shows banked matter, the crew pools, and how many lanes are building. This
  closes the gather -> bank -> build economy loop
  ([§3](#3-core-loop--roguelike-campaign)).
- **Faction livery:** hulls are baked neutral grey with a livery mask in the
  texture alpha; the renderer multiplies a per-instance faction color onto those
  texels only (player blue, enemy red, resourcers forced yellow), so one hull
  serves any faction without re-baking.
- Minimal **egui HUD:** selection list + a stub power/health panel
  ([§10](#10-ui--hud)).
- *(No combat, no real subsystems yet - movement, selection, camera, and the
  rendering/sim/interpolation spine.)*

### B. Web ship editor MVP (`editor/`)

- Import/preview a GLB, place a **hull convex-hull collider** + a couple of
  **hardpoint/subsystem anchors**, and **export** `model.glb` + `ship.json`
  ([§16](#16-web-based-ship-editor)).
- Just enough to author the placeholder ships the slice consumes - closing the
  loop **editor → pipeline → game**.

### C. The pipeline itself (`sol-shipc` + CI)

- `sol-shipc` compiles the authored ships and **fails on invalid colliders**.
- GitHub Actions **builds + deploys game and editor to Pages on every commit,
  any branch** ([§17](#17-build--deployment-pipeline)).

**Slice definition of done:** open the canonical Pages URL → orbit the camera →
box-select ships → issue a 3D move order and watch them go; open `/editor/` →
tweak a collider → export → (commit) → see it live. The whole spine, thin but
complete.

---

## 20. Roadmap

> Milestones are capability slices, not dates. Each builds on a working spine.

| Milestone | Theme | Headline deliverables |
|---|---|---|
| **M0** | **Spine** | Camera + box-select + move-disk; editor MVP; CI/CD to Pages. ([§19](#19-vertical-slice-milestone-0)) |
| **M1** | **Combat & ships** | Weapons/hardpoints, shields, hitboxes & positional damage, basic AI; richer HUD. |
| **M2** | **Ship simulation** | Reactor/power grid, subsystems, crew, damage control, capture-by-boarding. |
| **M3** | **Living universe** | Procedural nebulas/asteroids/planets/anomalies; **reactive fields** (ion storm). |
| **M4** | **Roguelike campaign** | Sector map, runs, scarcity economy, research tree, meta-progression, the Ark. |
| **M5** | **Multiplayer** | Deterministic lockstep over matchbox; desync detection; co-op/skirmish. |
| **M6** | **Polish & desktop** | Performance passes, native desktop builds, audio, UX, accessibility. |

---

## 21. Risks & Open Questions

### Top risks

1. **Cross-platform determinism for lockstep** (esp. wasm vs. native float
   behavior). *Mitigation:* keep `sol-sim` pure & ordered, `libm` for
   transcendentals, consider fixed-point for positions, checksum desync
   detection + resync fallback. Validate **early** with a determinism test
   harness (run the same command stream on native + wasm, compare checksums).
2. **Volumetric rendering performance on web.** *Mitigation:* half-res +
   temporal reprojection, strict tunable budgets, sensors-view simplification,
   WebGL2 fallback that degrades gracefully.
3. **WebGPU availability/variance across browsers.** *Mitigation:* feature
   detection, WebGL2 fallback, clear messaging.
4. **Scope.** This is a large design. *Mitigation:* the M0 spine is tiny and
   honest; each milestone is independently valuable; depth (sim) is layered onto
   a working spine, never blocking it.
5. **Fun of the energy/power micro at RTS scale.** Managing per-ship power for a
   fleet could be tedious. *Mitigation:* **power presets** and good defaults/AI;
   deep control is opt-in, not mandatory.

### Open questions (to resolve as we build)

- **ECS:** start with `bevy_ecs` (richer) or `hecs` (simpler) for the slice?
  *(Leaning `bevy_ecs` for the sim's eventual complexity; revisit at M0.)*
- **Netcode flavor:** confirm **delay-based lockstep** vs. `ggrs` rollback once
  we know typical entity counts. *(Leaning delay-based for RTS scale.)*
- **Determinism strategy:** how far can controlled **f32** take us before we
  need **fixed-point** for sim-critical state?
- **Editor distribution of authored assets:** export-and-commit by hand vs. an
  in-editor "open PR / write to repo" flow?
- **Web threading:** single-threaded web (simpler, no COOP/COEP) vs. threaded
  (faster, needs the service-worker shim)?
- **Default branch policy:** is the canonical Pages URL always the default
  branch, or a dedicated `release` branch? *(Doc assumes default branch.)*

---

## 22. Glossary

| Term | Meaning |
|---|---|
| **Ark** | The mothership; command/production/research anchor of a run. Its loss ends the run. |
| **MU / Matter Units** | The in-run resource currency, mined from asteroid fields by harvesters and spent on builds at the Ark. |
| **Research points** | Meta-progression currency. Awarded by missions (main + bonus objectives) **and by liquidating captured ships**; spent between runs in the research tree. |
| **Mission** | A single tactical encounter with a **main objective** (default: destroy enemy mothership) and an optional **bonus objective** that pays extra research points. |
| **Salvager** | Lightly armored utility hull unlocked at tier 1, after Corvettes. Disables and tows enemy ships back to the Ark for the **keep-or-liquidate** decision. The star class of the game's economy. |
| **Disabler beam** | Salvager's primary weapon. Damage ramps **inversely** with the target's hull fraction (weak on full hulls, strong on wounded ones). Cannot kill: clamps target hull to a 5% floor and flags the ship **Disabled**. |
| **Disabled** | Special ship state: zero movement, no firing, ignored by auto-target / SOS / wave conscription. Eligible for tow. |
| **Tow** | A salvager grappling a Disabled hull and pulling it home at half its own max speed. |
| **Pending decisions** | Per-Ark queue of towed captures awaiting Keep or Liquidate. Survives across encounters until resolved. |
| **Liquidation** | Stripping a captured hull at the Ark to convert it into research points. Alien hulls pay into both the general pool and the faction's xenotech-doctrine branch. |
| **Proficiency** | Per-faction-tech crew rating (`Human`, `Vaered`, ...). Determines how much of a captured hull's potential its crew can unlock. Rises asymptotically with time aboard, capped by the relevant Doctrine tier. |
| **Vaered** | The first alien faction (fast follower to the capture loop). Crystalline, ion-based; their hulls shred shields fast and tank kinetics until their own ion shield drops. The first xenotech doctrine branch. |
| **Move-disk** | Homeworld-style 3D move tool: set X/Z on a plane, drag vertically for altitude. |
| **Sensors view** | Far-zoom abstracted tactical representation of the battlespace. |
| **Lockstep** | Multiplayer model where all peers deterministically simulate the same state from shared commands. |
| **Desync** | Divergence of peers' simulations; detected via state checksums. |
| **Reactive field** | A coarse, deterministic environmental grid (ion charge, gravity, radiation) that gameplay perturbs and reads. |
| **Hardpoint** | A named anchor on a ship hull for a weapon/subsystem mount, authored in the editor. |
| **`sol-sim`** | The deterministic simulation crate - multiplayer source of truth, no rendering. |
| **`sol-shipc`** | The CLI that compiles & validates authored ships into runtime assets. |
| **Acclimation** | Crew gaining proficiency with a captured ship's exotic tech over time + research. |

---

*End of document. This is a draft blueprint meant to evolve - propose changes
via PR. Keep `CLAUDE.md` in sync with any change to the static-asset rule or the
build/deploy contract.*
