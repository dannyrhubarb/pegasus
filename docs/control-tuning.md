# Control-feel tuning guide

Every knob that shapes how the ship *feels* to fly, where it lives, what
happens when you turn it, and some ready-made recipes. Written for future
tuning sessions and as the checklist for an eventual in-game settings pane.

Physics-affecting constants (PD gains, forces, damping, fuel, crash
thresholds) live in `src/sim.rs`; input-generation knobs (stick gating,
dead-zones) stay in `src/main.rs`. JS constants are in the inline script in
`index.html`.

**Control loop rate (2026-07 re-sim refactor):** forces, the PD controller
and fuel burn are now recomputed **every physics tick (120 Hz)** from a
quantized per-tick `InputState`, instead of once per render frame with the
forces persisting across substeps. Handling is therefore identical on every
display refresh rate (previously a 60 Hz display held each torque twice as
long as a 120 Hz one), and the damper acts on the freshest spin rate — the
same `HEADING_KD` stops fractionally crisper than before. Inputs are
quantized (throttle u8, stick i8×2) before BOTH the sim and the replay
recorder see them, so recorded runs re-simulate bit-exactly; tuning knobs
are unaffected, but any new control path must route through `InputState` or
replays will silently desync from live play.

## 1. The touch scheme in one paragraph

Holding the floating stick fires the main engine and its direction commands
the nose direction; a PD controller (spring + damper) torques the ship the
short way to that angle. Two gates keep steering cheap: a *flick grace*
(short touches never thrust) and a *flip settle* (commanding a >92° turn
keeps the engine cold until the nose is nearly there). Holding the stick is
the only touch thrust control (the old JET button is gone). Keyboard/gamepad
use direct rate rotation and override the PD controller while held.

## 2. Heading controller (how the nose chases the stick)

| Knob | Now | What it is | Turn it up | Turn it down |
|------|-----|------------|------------|--------------|
| `HEADING_KP` | 14.0 | Spring: torque per radian of error | Snappier lock-on; past ~KD²·something it overshoots and rings | Lazy, floaty pointing; nose lags the stick |
| `HEADING_KD` | 2.2 | Damper: torque against spin rate | Crisper stop, no wobble; too high = sluggish approach to target | Overshoot + oscillation around the target ("compass needle") |
| `HEADING_TORQUE_MAX` | 6.0 | Torque ceiling — **this sets the 180°-flip time** (the spring saturates it for most of a big swing) | Faster flips (6.0 ≈ 0.45 s for 180°; 3.5 was ≈ 1 s) | Slower, more deliberate flips |
| `angular_damping` (body builder, `main()`) | 3.0 | Rapier's passive spin decay — affects *all* rotation incl. keyboard | Ship stops spinning on its own quickly | Momentum-y rotation; free spins persist |

Rules of thumb: raise `KP` and `KD` together (roughly `KD ∝ √KP`) or you
trade lag for wobble. `TORQUE_MAX` is the honest "flip speed" dial; `KP`
mostly affects the last ~30° of the approach.

Deflection scaling: commanded torque is multiplied by stick deflection
(0..1 after dead-zone), so rim = full authority, small nudge = trim. That
mapping is linear — squaring it (`steer_mag * steer_mag`) would give a
finer trim band at the cost of a "dead" feel mid-stick.

## 3. Stick-thrust gating (one-handed feel)

| Knob | Now | What it is | Turn it up | Turn it down |
|------|-----|------------|------------|--------------|
| `STICK_THRUST_DELAY` | 0.12 s | Flick grace: contact shorter than this never lights the engine | Steering flicks always free, but engine feels laggy on purpose-holds | Accidental micro-burns on every flick |
| `STICK_THRUST_RAMP` | 0.18 s | 0→full throttle fade after the delay | Softer, "spool-up" engine; gentler hover corrections | Punchy instant engine (0 = old binary feel) |
| `FLIP_GATE_RAD` | 1.6 (~92°) | Error that counts as a "big flip" → engine goes cold | Only near-reversals gate thrust; diagonal turns burn through | Even modest turns cut the engine (feels stuttery) |
| `FLIP_DONE_RAD` | 0.35 (~20°) | Error below which the flip is "settled" → engine relights (via the ramp) | Earlier relight — thrust while still swinging (smears the burn direction) | Later, more precise relight; feels hesitant if < ~8° |

The gate resets the ramp, so a post-flip burn always fades in. If flips get
faster (higher `TORQUE_MAX`), consider tightening `FLIP_DONE_RAD` so the
relight doesn't happen mid-swing.

## 4. Stick geometry (Rust, `src/main.rs` — the stick is in-canvas now)

| Knob | Now | What it is | Notes |
|------|-----|------------|-------|
| `STICK_TRAVEL` | 60 (CSS px) | Deflection for full authority | Bigger = finer control, more thumb travel |
| `STICK_DZ` | 0.15 | Radial dead-zone (fraction of travel, rescaled) | Below it: thrust-only hold, no heading command. Bigger = easier "just burn straight"; smaller = twitchier |
| `STICK_ZONE` | 0.55 | Touches below this fraction of viewport height grab the stick | Lower value = bigger grab area (0.55 → lower 45%) |
| `STICK_RADIUS` / `STICK_KNOB_R` | 85 / 32 (CSS px) | Visual only — input math uses `STICK_TRAVEL` | Keep `RADIUS ≈ TRAVEL + KNOB_R` so visuals match feel |

(Values are CSS px, multiplied by `dpi` at draw/hit-test time — same rule as
every other pixel constant.)

## 5. Engine & airframe (the "how heavy does it feel" set)

| Knob | Now | Where | What it does |
|------|-----|-------|--------------|
| Main engine force | `8.0` | inline in the Controls section (`let f = 8.0 * throttle`) | TWR dial. Ship mass ≈ 0.65, lunar gravity 1.62 → weight ≈ 1.05 → **TWR ≈ 7.5** (very hot; Apollo was ~3). Lower toward 5–6 for a calmer game |
| `linear_damping` | 0.2 | body builder | Invisible speed ceiling; higher = momentum bleeds off faster (arcade), 0 = pure Newton |
| Gravity | −1.62 | `main()` | The Moon. Turn up for punishing, down for floaty |
| `RCS_FORCE` | 3.3 | Controls section | Keyboard/pad rate-rotation strength only (touch uses the PD controller) |
| Glow smoothing | 0.12/frame | main loop | Cosmetic: how fast flame/light/sound follow the throttle |

## 6. Consequences (difficulty rather than feel, but they interact)

`CRASH_DV_SOFT = 2.5` (free-hit ceiling), `CRASH_DV_HARD = 6.0` (instant
death), `HULL_MAX = 100` (soft-hit budget; damage is linear between SOFT
and HARD), `HULL_REPAIR_PER_S = 20`, `FUEL_BURN_MAIN = 3.5/s` at full
throttle, `FUEL_BURN_RCS = 1.2/s` (heading control burns proportional to
commanded torque). A hotter engine (higher force) doesn't burn more fuel —
burn is per-second of throttle, so TWR changes also change effective range.

## 7. Recipes

**Docile trainer** — forgiving, slow, hard to die:
`force 8→5`, `HEADING_TORQUE_MAX 6→4`, `CRASH_DV_SOFT 2.5→3.5`,
`linear_damping 0.2→0.35`.

**Twitch racer** — for when the cave feels too easy:
`HEADING_KP 14→18`, `HEADING_KD 2.2→2.6`, `HEADING_TORQUE_MAX 6→9`,
`STICK_THRUST_DELAY 0.12→0.08`, `STICK_THRUST_RAMP 0.18→0.08`.

**Heavy freighter** — deliberate, shuttle-like:
`force 8→6`, `HEADING_TORQUE_MAX 6→2.5`, `HEADING_KP 14→8`,
`STICK_THRUST_RAMP 0.18→0.4`, `FLIP_DONE_RAD 0.35→0.2`.

**Purist / no assists** — closest to the original Flash game on touch:
not a knob set — route the stick back to rate control (torque = stick x,
like the keyboard path). Keep this in mind as a *scheme*, not a tuning, for
the controller-picker below.

## 8. Toward a settings / controller pane

The plumbing pattern already exists, with two working examples
(velocity-vector and invert-stick toggles) in `index.html`:

1. Checkbox/slider in the info overlay (`stopPropagation`, **no**
   `preventDefault` — that kills checkbox clicks).
2. Persist in `localStorage` (`pegasus_*` keys).
3. Forward to the game via a wasm export (`set_*`) + an atomic, with a short
   `setInterval` retry until `wasm_exports` is ready (see `applyVelSetting` /
   `applyInvSetting`). Now that the stick is in-canvas, input-remapping
   settings (like invert) go through this same export path rather than living
   in JS.

For live tuning without rebuilds, add one export per knob group, e.g.
`set_heading_gains(kp, kd, tmax)` and `set_stick_gates(delay, ramp, gate,
done)`, storing into f32 atomics read where the constants are used today.
A "controller preset" is then just a named bundle of these values plus a
scheme flag (heading-control vs rate-control) — the pane sets one preset,
`localStorage` remembers it, and experimenting becomes sliders instead of
recompiles. Keep the constants in `main.rs` as the defaults the atomics
initialise from, so native builds and a wiped localStorage behave
identically.

> When a knob's value or meaning changes, update this file and the matching
> CLAUDE.md sections in the same commit.
