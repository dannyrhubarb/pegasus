# Touch input: the plumbing, and the trap in it

How a finger on the glass becomes a command to the ship, and the one place
that chain lies to you. Written up after a bug that cost two wrong fixes
before it was understood (2026-07).

Related: `docs/control-tuning.md` covers how the stick *feels* once a touch
is claimed — gains, gates, dead-zones. This file is about whether the touch
is seen at all.

## The chain

```
finger
  → browser touch event            (touchstart / touchmove / touchend / touchcancel)
  → mq_js_bundle.js                canvas listener → wasm_exports.touch(phase, id, x, y)
  → miniquad                       → macroquad's Context::touch_event
  → macroquad `touches()`          HashMap<u64, Touch>, read once per frame
  → TouchStick (src/main.rs)       claims an id, tracks it, produces a steer vector
  → InputState                     quantized, fed to Sim::tick and the recorder
```

Two properties of that chain matter more than anything else in it.

**Positions arrive in raw physical pixels.** `touches()` does *not* divide by
the DPI scale, unlike `mouse_position()`. The gather divides by `dpi` before
anything else touches the value, putting it in the same logical space as
`screen_width()` and all drawing. A steer *direction* is scale-invariant, so
a missed conversion still steers correctly and only shows up as the stick
being drawn off-screen.

**A frame sees one entry per touch id, carrying only the phase of the last
event that arrived.** This is the trap, and it has its own section.

## The trap: phase collapse

macroquad stores touches like this (`macroquad-0.4.15/src/lib.rs`):

```rust
fn touch_event(&mut self, phase: TouchPhase, id: u64, x: f32, y: f32) {
    let context = get_context();
    context.touches.insert(id, input::Touch { id, phase: phase.into(), position: Vec2::new(x, y) });
    // …
}
```

`insert` **overwrites the whole entry, phase included**. Events arrive
between frames, whenever the browser dispatches them; the frame loop reads
the map once. So the phase a frame observes is not "what happened to this
touch since last frame" — it is "whatever happened *last*".

A `touchstart` immediately followed by a `touchmove` therefore reaches the
game as a single entry phased `Moved`. **`Started` is never observed at
all.** Nothing is dropped and no event is lost; the phase is simply not a
record of the touch's history.

That pairing is not exotic. Android samples touch at 120–240 Hz against a
60 Hz frame loop, so a finger that is moving even slightly as it lands emits
its first `touchmove` within a few milliseconds — comfortably inside the same
~16 ms gap as the `touchstart`. Which is to say: it happens most of the time,
for exactly the gesture a player makes when they mean to steer.

The end of a touch has the mirror-image case. Browsers recycle touch ids —
Chrome hands identifier `0` back to the next single touch — so a
`touchend`+`touchstart` pair landing in one gap leaves a *new* finger sitting
on a *familiar* id, phased `Started`.

### The rule

> **Freshness is identity, not phase. A touch id that was not on screen last
> frame is a new finger, whatever phase it reports.**
>
> `Started` still counts as fresh on its own, to catch the recycled-id case.
> And a claimed id that reports `Started` is a *different finger* — drop the
> claim so the new one re-centres the stick.

`fresh_touch` and `stick_touch_lost` in `src/main.rs` are that rule, kept as
pure functions so they can be unit-tested; `prev_touch_ids` in the frame loop
is the memory they need.

Split controls (the Settings toggle) add a second claimant: the left-half
floating throttle button. Both claims run the same rule through
`fresh_touch_in` (the zone-predicate form of `fresh_touch`) — the zone only
decides where a touch *lands*; once claimed, a finger is followed by
identity wherever it moves, across the midline included. The follow/release
logic (`stick_touch_lost`) is shared verbatim, so the phase-collapse and
recycled-id cases behave identically for both controls.

## The bug this came from

The stick claimed a touch only on `TouchPhase::Started`:

```rust
for t in touches() {
    if t.phase != TouchPhase::Started { continue; }
    if stick_active && stick.id.is_none() { /* claim */ }
}
```

Every touchdown whose `Started` was collapsed was ignored. Worse, because the
claim could *only* ever fire on `Started`, there was no recovery: the finger
stayed dead for its entire press, however long the player held or dragged it.
Lifting off and touching down again re-rolled the dice.

An Android tester reported it as **"only every few touches goes through …
some touch interactions are just ignored until I release and touch again"** —
which is a precise description of a race, once you know what to listen for.

### Two wrong turns worth remembering

Both were plausible, both were about the native shell, and both were wrong:

1. **The navigation bar.** `BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE` really
   does eat a touch: an upward swipe near the bottom — where the stick lives
   — reveals the bars transiently, and the next tap is consumed dismissing
   them. Removing it was a genuine improvement and is still in place. It was
   not this bug.
2. **WebView gesture claiming.** Long-press selection and overscroll can also
   steal an in-progress gesture, so those were disabled too. Also reasonable,
   also not this bug.

The lesson is not "don't guess" — those were sane hypotheses. It is that
**neither was tested before shipping**, and a fix that ships without evidence
buys nothing but a round trip through a human tester.

### What actually found it

An in-page touch tracer (bottom-left, gated behind the Debug HUD setting,
standalone script at the end of `index.html`) counting raw
touchstart/move/end/**cancel** at the window capture phase. The reading taken
at the moment touch died:

```
ts 45   tm 412   te 44
CANCEL 0   pcancel 0
touchmove @280,733 n=1
```

One live finger (`ts − te = 1`, `n=1`), touchmoves streaming at coordinates
sitting **on** the stick, zero cancels — and the stick rendered parked. That
single reading eliminates, in one shot: the native shell, the WebView, any
invisible tap-stealing overlay, and coordinate/DPI errors. The events were
arriving, correctly, and the game was not acting on them. Everything after
that was reading `touches()` and the macroquad source.

Keep the tracer. Counters at the capture phase are cheap and they answer the
only question that matters first: *does the event reach the page at all?*

## Regression tests

**`fresh_touch` / `stick_touch_lost` unit tests** (`src/main.rs`) pin the rule
directly and deterministically: a brand-new id phased `Moved` must be
claimed, a known id must not be re-claimed, a recycled id phased `Started`
must be, and lifted/cancelled/vanished touches must release.

**`tests/touch-e2e/run.mjs`** drives the real wasm build in headless Chromium
and asserts the whole chain end to end. Two cases:

| case | what it dispatches | before the fix | after |
|---|---|---|---|
| control | `touchstart` alone | arms the run | arms the run |
| regression | `touchstart` + `touchmove`, same task | **ignored** | arms the run |

The control exists to tell a real regression from a broken harness: if both
cases fail, the test is wrong, not the game. Verified to discriminate — the
regression case fails, and the script exits 1, when the claim rule is
reverted to the phase test.

The observable is `run_start_seq()`, an export the analytics already use: the
run is armed-but-idle until the first non-neutral input, and a bare stick
touch is non-neutral, so the counter bumping *is* "the game accepted the
touch". No test-only surface in the shipped binary.

### Why this is not an emulator test

The obvious instinct is to reproduce it on a real Android emulator. Don't —
for this bug it is the wrong instrument.

The bug is a race between event delivery and the frame loop. On a device you
cannot make `touchstart` and `touchmove` land in the same frame gap on
demand; you can only hope they do. A passing emulator run would mean "the
events happened to arrive in different frames this time", which is exactly
the false confidence that let this ship in the first place.

Dispatching both events synchronously in one JS task removes the hope: the
browser *cannot* run a frame between two statements, so the collapse is
guaranteed. That is what makes the test deterministic, and it is why it lives
in a browser rather than on a phone.

The emulator smoke test (`android-smoke.yml`) is still worth having — it
catches boot crashes, which is a job an emulator is genuinely good at.
