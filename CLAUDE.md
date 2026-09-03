# Pegasus — Moon Lander

Rust + macroquad 0.4.15 + Rapier 2D game compiled to WebAssembly and served via GitHub Pages. The player pilots a ship through a procedurally-generated scrolling cave using thrust and rotation controls.

> **Keep this file current.** Update CLAUDE.md as part of every commit that changes architecture, adds a system, renames constants, fixes a gotcha, or reveals a lesson. Don't batch it up — update it while the context is fresh.

## Build & deploy
```bash
cargo build               # native dev build (quick sanity check; silent — audio is wasm-only)
cargo test --workspace    # unit tests; --workspace is required or the sim-core crate's tests are skipped
```
Deploy is automatic: any push to `main` triggers `.github/workflows/deploy.yml` which builds the WASM target and publishes to GitHub Pages. Build takes ~5–10 minutes.

### Deploy pipeline & PR previews
The site lives at **`https://pegasusmoonlander.com`** (custom domain on this
repo's GitHub Pages, issue #171 — the old
`dannyrhubarb.github.io/pegasus` origin 301-redirects there, which also
means the old origin can never serve a page again: players' old-origin
`localStorage` — settings, callsign, consent, editor drafts — is orphaned
by design, an accepted one-time loss; the WebAuthn RP ID for the planned
accounts work (#157) will be `pegasusmoonlander.com` and is PERMANENT once
passkeys ship). The custom domain lives in **Settings → Pages**, NOT a
CNAME file (Source = "GitHub Actions" ignores CNAME files); DNS is apex
A/AAAA records to the GitHub Pages fleet + `www` CNAME at the registrar.
`manifest.json`'s `start_url` must stay RELATIVE (`"./"`) — it was
`/pegasus/` once, which would 404 every PWA install on the domain root.
The published build lives on the **`gh-pages` state branch**: the `main`
build at the root, one **per-PR preview** in `pr-<n>/` (served at
`https://pegasusmoonlander.com/pr-<n>/` — works because every asset URL
in `index.html`/`manifest.json` is relative; the preview/test-APK sticky
comments ask the Pages API for `html_url`, so their links follow the
custom domain automatically). Five workflows, sharing two
composite actions (`.github/actions/build-site` = wasm build + icons + overlay
injection; `.github/actions/sync-pages-branch` = commit into `gh-pages` with a
push-retry loop for concurrent deploys):
- `deploy.yml` (**Main deploy**, push to `main`): build → sync branch **root**
  (live previews in `pr-*/` and the Android APK in `app/` are kept — the
  root replace excludes both).
- `preview-deploy.yml` (**Preview deploy**, PR opened/synchronize/reopened):
  build (overlay revision = `<head-sha>-pr-<n>`) → sync `pr-<n>/` → sticky PR
  comment (`<!-- preview-env -->` marker) with the preview URL. Skipped for
  fork PRs (read-only token).
- `preview-teardown.yml` (**Preview teardown**, PR closed): delete `pr-<n>/`,
  comment.
- `android-test-apk.yml` (**Android test APK**, OPT-IN per PR): build the
  `preview` APK → sync `pr-<n>/app/` → sticky comment (`<!-- test-apk -->`)
  with the install link. See "Android app" for the build type; the
  **`pr-<n>/app/` placement is load-bearing** — a subdir sync REPLACES its
  directory, so `sync-pages-branch` excludes a nested `app` exactly like the
  root excludes `pr-*`/`app`, otherwise the next push to the PR would delete
  the APK out from under the tester. Nesting it inside `pr-<n>/` also means
  **preview-teardown cleans it up for free**.
- `publish-pages.yml` (**Publish Pages**): the *only* workflow that calls
  `deploy-pages`. Triggered by `workflow_run` on the three above plus
  **Android release** and **Android test APK** (must match their `name:`
  strings exactly — a workflow that pushes to `gh-pages` without being listed
  here lands on the branch and is never deployed) and snapshots the whole
  `gh-pages` branch. **Its upload step sets `include-hidden-files: true`**:
  `actions/upload-pages-artifact` EXCLUDES dot-files/dirs by default
  (`tar --exclude=.[^/]*`), so anything under `.well-known/` sat on
  `gh-pages` and still 404'd (found 2026-09 on the PR #195 preview); the
  rest of the pipeline was already dot-safe (`build-site` `cp -r`,
  `sync-pages-branch`'s `cp -a` root replace).
  **Gotcha**: the auto-created `github-pages` environment only allows
  deployments from `main`, so PR-triggered workflows can't deploy directly;
  `workflow_run` workflows execute from the default branch, which passes the
  protection. Also: pushes made with `GITHUB_TOKEN` don't trigger `push`
  workflows (recursion guard), so an `on: push: branches: [gh-pages]` publisher
  would never fire — `workflow_run` is load-bearing, not a style choice.
  **Second recursion-guard bite (found 2026-08-30)**: a run that was itself
  dispatched WITH `GITHUB_TOKEN` (release-apps → android-release) never
  emits a `workflow_run` completion event either — the guard's
  workflow_dispatch exemption covers the dispatch, not the dispatched run's
  downstream events — so three Release-apps APK syncs sat on `gh-pages`
  undeployed until the next unrelated branch event. Fix: publish-pages also
  has `workflow_dispatch` (job `if` passes it through) and android-release
  dispatches it explicitly after its sync (an explicit dispatch is exempt
  again; the doubled trigger on human-dispatched runs collapses in the
  `pages` concurrency group).
  Keep **Settings → Pages → Source = "GitHub Actions"** (do *not* switch it to
  the `gh-pages` branch — that would bypass this pipeline and serve the branch
  with Jekyll defaults). The Pages API intermittently rejects deployments
  created in rapid succession ("Deployment failed, try again later." — seen
  live when 4 preview deploys landed within a minute), so the deploy step
  retries once after 30 s; a red publish run self-heals on the next branch
  event regardless.

## Project structure
- `src/main.rs` — input exports/atomics, window conf, the frame loop (input gathering + stick gating, camera, drawing, HUD, minimap, crash dialog/replay/ghost cosmetics), and unit tests
- `sim-core/` — the **`pegasus-sim` library crate** (workspace member): the whole deterministic half of the game, extracted 2026-07 so pegasus-backend can compile the IDENTICAL simulation for server-side score verification (it consumes this crate as a cargo **git dependency pinned to a `main` rev** — physics/level changes here need a backend re-pin + redeploy, see the backend repo's CLAUDE.md). **Nothing in it may depend on macroquad or any nondeterminism**; it uses `glam` (pinned to the version macroquad 0.4.15 re-exports, so `Vec2` unifies across the boundary) + `rapier2d` + `miniz_oxide`:
  - `sim-core/src/sim.rs` — **the deterministic simulation core**: `Sim` owns all Rapier state, the sliding collider windows (BTreeMaps) and ship systems (fuel/hull/score/landing/crash), advanced ONLY by `tick(InputState) -> TickReport` at `PHYSICS_DT`; plus `resim(&Recording)` and all physics constants. Same inputs + same start keyframe → bit-identical trajectory (unit-tested). **Any new gameplay force/effect must go through `tick`** — frame-level physics mutation would break replay determinism.
  - `sim-core/src/world.rs` — deterministic world generation, parameterized by a **`Level`** (see "Levels"): cave curves, shafts, obstacles, pads and `stand_y` are all `Level` methods; plus **`Terrain`** (hand-drawn polygon worlds — see "Levels"), `Rng`/`hash_u32`, the world constants (`SEG_LEN`, `RESET_X`, `PERIOD`, `V_PERIOD`, …) and `shipped_levels()` — the stem → `Level` map of the compiled-in level files the backend verifier params-checks submissions against (kept in sync with `levels/manifest.json` by a unit test)
  - `sim-core/src/replay.rs` — the hybrid `Recording` format + blob codec (see "Hybrid recording")
- `src/render.rs` — radial light shader sources, faceted wall/shaft lattice (`lattice_point`, `shaft_lattice`, `facet_shade`), `draw_flat_mesh`, and `triangulate` (ear-clip fill for hand-drawn terrain polygons)
- `src/ship_mesh.rs` — `SHIP_TRIS` / `SHIP_DETAILS` data tables extracted from the Flash SWF
- `src/audio.rs` — in-memory WAV synthesis (`wav_from_samples`, `thruster_wav`, `boom_wav`)
- `levels/` — **runtime level data**: `*.level` files (`key = value`) + `manifest.json` (menu order), fetched by `index.html` and pushed into the wasm — new levels deploy with no recompile (see "Levels")
- `fonts/` — the **vendored menu webfont**: `jetbrains-mono.woff2` (latin variable, wght 400–800) + its `OFL.txt`, loaded via `@font-face` by `index.html`/`editor.html` so every platform renders the same face (see the menu-font note under "Game menu"); in all three bundle copy lists
- `editor.html` — the **standalone level editor** (issue #89 v1, 2026-07): draws hand-drawn `.level` worlds — the same `poly`/`pad`/`start` representation The Hollows uses — on a pan/zoom canvas. Self-contained like `index.html` (no CDNs), copied by `build-site`. **Deliberately UNLINKED from the game UI** (owner decision pre-merge): it lives at its own path with no menu button and no picker row; the game only meets it through the `?custom=1` test-fly handoff. **While it stays unlinked, editor commits carry NO `Whats-new:` trailers** (the changelog must not advertise an unannounced feature — the PR #110 branch had its trailers stripped before merge; give the editor one proper entry when it's linked up for real). See "Level editor & custom drafts" under "Levels"
- `tools/gen-third-party-licenses.py` + `third-party-licenses.html` — the generated third-party attribution page served with the site and linked from the About screen; regenerate when `Cargo.lock` changes (see "License")
- `privacy.html` — standalone privacy policy served with the site (and bundled into both apps), written for the Play Store listing's required privacy-policy URL; same substance as the About screen's `#privacy-note` — keep the two in agreement when the analytics story changes
- `app-policy.json` — the **checked-in update/config policy** every client fetches at launch (`{}` = no verdicts): **the remote lever over ALREADY-INSTALLED apps and stale web tabs** — commit a `config` override to repoint old installs at a moved backend with no store release, or a `minBuild`/`minWebBuildTime` wall for a genuinely breaking change. **Reach for this whenever a backend move or compatibility break is being planned** (it exists because the #171 migration had no such lever and drained for weeks — pegasus-backend#38); see "App update policy" under "Game menu"
- `tools/gen-whats-new.py` + `tools/whats-new-backfill.json` + `tools/whats-new-overrides.json` — deploy-time generator for `whats-new.json`, the About screen's What's New changelog (see "What's new page" — **every user-facing commit needs a `Whats-new:` trailer**; the overrides file rewords already-merged entries)
- `.github/labels.json` + `tools/sync-labels.py` + `.github/workflows/labels.yml` — the **checked-in issue-label convention** (`type:` / `area:` / `status:` groups); edit the JSON, never the GitHub UI — see "Git workflow"
- `index.html` — web wrapper, safe-area insets, the **HTML game menu** (start / pause / game-over screens, level picker, settings, high scores, about — see "Game menu"), **gamepad polling**, and a **boot guard** (touch/stick input moved in-canvas — no touch handlers here any more): a small standalone `<script>` tag ahead of the bundle (script tags parse independently, so no error in the bundle/main script can kill it) that paints any script error on screen with file:line and offers a tap-to-reload if `wasm_exports` is missing 8 s after load. Keep it first and self-contained. It also pushes each reported error into a capped `window.__pegErrs` buffer (push-only — the guard never depends on anything) that the analytics module drains (see "Analytics"). It also wraps `console.error` (installed ahead of the bundle, so the wasm `console_error` import routes through it) and appends the last logged error to the banner when the error event is anonymous or attributed to the `.wasm` file — **a Rust panic reaches JS as an opaque trap** (`RuntimeError: unreachable`; iOS Safari mutes it further to a bare "Script error." with no filename, because wasm frames fail its same-origin check), and the only useful description is the panic-hook line logged just before the trap (`src/main.rs` installs `std::panic::set_hook` → `error!("{}", info)`; the *default* hook prints the useless Debug form `PanicHookInfo { payload: Any { .. }, … }`). Unhandled promise rejections get the same banner (skipped when `reason` is null). **Fully-anonymous errors (no filename AND no console.error trace) are deliberately ignored**: same-origin scripts always carry file:line and a wasm panic always logs via the hook first, so the only things that land there are Safari-injected third-party scripts — reproduced live on iOS: opening the **share sheet** runs share/action extensions' preprocessing JS in the page, and an error in any of them arrives as a muted "Script error." (this was the mystery banner of 2026-07-06, seen right after the Pegasus rename and initially blamed on it).
- `mq_js_bundle.js` — **vendored** miniquad/quad-snd JS loader (from not-fl3/miniquad-samples). Pinned in-repo so deploys don't depend on a third-party host; includes the audio backend. Update it deliberately if macroquad is upgraded. **Gotcha**: it declares globals at top level (`const canvas`, `var gl`, `wasm_exports`, `function load`, …) that share the page's global scope — redeclaring any of them in `index.html`'s inline script is a SyntaxError that silently kills the *whole* inline script (no `load()` → no wasm, page shows only the HTML chrome). Pick distinct names and check the bundle before adding top-level identifiers.

## Input sources
**Control-feel tuning**: every feel knob (PD gains, thrust gates, stick
geometry, TWR, damping), its effects, and preset recipes are documented in
`docs/control-tuning.md` — update it in the same commit as any knob change.
It also sketches the plumbing for the planned settings/controller-picker
pane (localStorage → wasm exports → atomics, with three working examples).
**`docs/touch-input.md`** is its counterpart for the touch PLUMBING: how an
event reaches the stick, and the phase-collapse trap on the way.

Four input paths feed the same physics, combined in the main loop:
- **Keyboard** (desktop): `Down` thrust, `Left`/`Right` rotate, `R` reset.
- **Mouse**: left-button held = thrust.
- **Touch** (mobile): an **in-canvas floating attitude stick**, drawn and
  read entirely in the game via macroquad's `touches()` API — no DOM element,
  no JS handlers. `simulate_mouse_with_touch(false)` at startup stops miniquad
  turning canvas touches into mouse-down (= full thrust); `touch-action: none`
  on the canvas blocks iOS scroll/zoom. The `TouchStick` gatherer + `draw_stick`
  helper live in `src/main.rs`.
  - **Attitude stick = commanded nose direction**: push up → nose up, push
    left → nose left, pull down → nose down. The "Invert stick" overlay
    checkbox (`#inv-toggle-row`, `pegasus_invert_stick`) negates the
    commanded direction (both axes — push down = nose up, like pulling back
    on a flight stick; the knob visual still follows the finger) via the
    `set_invert_stick(i32)` export → `INVERT_STICK` atomic, applied in
    `TouchStick::apply`. The game runs a **PD heading
    controller** (`HEADING_KP = 14`, `HEADING_KD = 2.2`, clamp
    `HEADING_TORQUE_MAX = 6`, applied via `add_torque`) that rotates the
    ship the **short way** to the commanded angle; deflection magnitude
    scales the torque (nudge = trim, rim = hard flip). `STICK_DZ = 0.15`
    radial dead-zone, rescaled. Manual rotation (keyboard/pad) overrides
    while held; heading RCS burns fuel proportional to commanded torque and
    puffs the matching nozzle beyond ±0.4 torque (negative torque = left
    nozzle — `fire_rcs(-1)` produces negative torque). The steer vector is
    screen convention (y down); the sim maps target angle = `atan2(-x, -y)`
    since the nose at angle `a` points `(-sin a, cos a)`.
  - **Holding the stick fires the main engine** — even dead-centre (inside
    the dead-zone there's just no heading command). One-handed flight:
    touch = burn + point, release = coast. The stick glows amber while held.
    **Gated game-side** so steering stays cheap: flicks shorter than
    `STICK_THRUST_DELAY = 0.12 s` never light the engine, thrust then ramps
    to full over `STICK_THRUST_RAMP = 0.18 s`, and a commanded flip past
    `FLIP_GATE_RAD (~92°)` keeps the engine cold (`flip_settling` latch)
    until the nose settles within `FLIP_DONE_RAD (~20°)` — the gate resets
    the ramp, so post-flip thrust also fades in. (There is no separate JET
    thrust-only button any more — stick-hold covers one-handed play.)
  - **Split controls** (two-handed scheme, `#split-toggle-row`,
    `pegasus_split_controls` → `set_split_controls` → `SPLIT_CONTROLS`,
    **on by default** since 2026-08 — turning it off restores the
    one-handed stick-hold scheme): the screen halves at the vertical
    midline — a fresh
    touch on the LEFT half spawns a floating **throttle button** under the
    finger (`ThrottleButton` + `draw_throttle` in main.rs; hold = full
    throttle, instant like the keyboard — none of the stick-hold
    flick/flip/ramp gating, which only exists because the one-handed stick
    doubles as the engine; the button rides the finger while held) and the
    RIGHT half spawns the attitude stick, which then **steers only**
    (stick-hold no longer lights the engine). The zone gates only where a
    touch LANDS — both claims run the identity-not-phase rule via
    `fresh_touch_in` (the zone-predicate form of `fresh_touch`), and a
    claimed finger is followed across the midline. The button parks
    bottom-LEFT as a translucent ghost, mirroring the stick's bottom-right
    park. Replay physics is untouched (throttle was always an analog
    channel in `InputState`), but the scheme flown rides the recording's
    **cosmetic trailer** (see "Hybrid recording") so replays render the
    widgets of the scheme that produced them: a split run's playback adds
    the half-size throttle button bottom-left (lit while the recorded
    throttle is up — under this scheme the throttle channel IS the
    button), mirroring the recorded-input stick.
  - **Floating**: while flying, a fresh touch **anywhere on screen** spawns
    the stick centred under the finger and claims that touch id — the whole
    canvas is the flight-control surface (the pause/restart buttons are HTML
    and swallow their own taps, so they never reach the canvas). The old
    lower-`STICK_ZONE` restriction was a leftover from the in-canvas
    crash-dialog buttons (now HTML) and is gone. During the dialog/replay
    `stick_active` is false and fresh touches are ignored (the out-of-flight
    UI is HTML; the old in-canvas `ui_tap` machinery is gone).
    Release parks the stick bottom-right as a translucent ghost. Positions
    are physical px (`touches()` and `screen_*()` share that space; a mouse
    press maps in via `× dpi`).
  - **"Fresh" is an id that wasn't there last frame — NEVER a
    `TouchPhase::Started` test** (`fresh_touch` / `stick_touch_lost` +
    `prev_touch_ids` in main.rs, unit-tested). macroquad keeps ONE entry per
    touch id and `touch_event` overwrites it wholesale, so a frame observes
    only the phase of whatever event arrived LAST before it ran: a
    touchstart immediately followed by a touchmove — routine on Android
    (touch sampling 120–240 Hz vs a 60 Hz frame loop) and certain whenever
    the finger is already moving as it lands — collapses into one `Moved`
    entry and `Started` is never seen at all. Claiming on the phase
    therefore dropped a large share of touchdowns, and since the claim
    could only fire on `Started` the finger stayed dead for its whole
    press: the Android tester's "only every few touches goes through …
    ignored until I release and touch again" (2026-07, diagnosed with the
    in-page touch tracer — it showed touchmoves arriving at the right
    coordinates while the stick sat parked, which exonerated the native
    shell and the WebView). Recycled ids (Chrome reuses 0 for the next
    single touch) mean `Started` still counts as fresh on its own, and the
    stick drops a claim whose id reports `Started` so the new finger
    re-centres it. **`docs/touch-input.md`** is the full write-up: the
    event chain, the collapse, the two wrong fixes that preceded it, the
    tracer reading that found it, and why the regression test is a browser
    test (`tests/touch-e2e/`, in CI) rather than an emulator one — the bug
    is a race, so an emulator can only pass by luck, while dispatching both
    events in one JS task makes the collapse certain.
- **Game controller** (BT/USB, web): `index.html` polls the **Web Gamepad API**
  each `requestAnimationFrame` and forwards to exported `set_pad_thrust(i32)` /
  `set_pad_torque(f32)` / `set_pad_reset()`. Mapping (standard layout): thrust =
  A/Cross (0), R2 (7, analog>0.3), or D-pad up (12); steer = left stick X
  (axes[0], dead-zoned/rescaled) or D-pad L/R (14/15); reset = Start (9) or
  Y/Triangle (3, edge-triggered). Polling starts on `gamepadconnected` and stops
  (releasing held inputs) if the pad drops out.

Touch is read directly via macroquad each frame; the gamepad uses `PAD_*`
atomics (JS-forwarded) so a connected-but-idle controller never stomps an
active touch. The main engine is a
**throttle (0..1)**: every current source is binary (1.0), but the plumbing
stays analog — engine force, glow, fuel burn, and exhaust particle
count/speed all scale with it. Rotation has two modes: **rate control**
(keyboard keys / pad stick → nozzle force via `fire_rcs`) and the touch
stick's **heading control** (PD to a commanded angle, pure `add_torque`);
rate control wins while actively held. `PAD_RESET` is a swap-to-consume flag
so a held reset button fires exactly once.

## Game menu (web only, HTML)
The whole out-of-flight UI is one fullscreen HTML overlay (`#menu` in
`index.html` — HTML/CSS, not drawn in-canvas, so it stays crisp at any size),
styled as a **neon vector arcade**: dark CRT base, a drifting perspective
grid (the `.grid` div, first child of `#menu`), a scanline+vignette+sweep
overlay (`.crt`, always the
last child of `#menu`, `pointer-events:none` so it never eats a tap), chunky
monospace lettering with layered `text-shadow` glow (the one webfont is
**vendored in `fonts/`** — no CDNs at runtime, the repo stays
self-contained), and **custom neon toggle switches — no native
form elements**. Palette in `:root` (`--cyan`/`--magenta`/`--amber`/
`--green`). Respects `prefers-reduced-motion`.
**Screens must never scroll sideways — `.screen` is `overflow-x: hidden`
and anything full-width needs `max-width: 100%`** (field bug, 2026-08):
`.screen`'s `overflow-y: auto` makes its unspecified `overflow-x` COMPUTE
to `auto`, so a child even 1 px wider than the padded content box turned
the whole screen into a horizontal scroller that iOS rubber-bands — the
"level picker bounces sideways" report. The culprit was `.rowlist`'s
`width: min(440px, 88vw)` vs the screen's 24 px side padding: 88vw beats
`100vw − 48px` below 400 CSS px, so iPhones up to the 393 px class
overflowed by 1–2 px while ≥ 402 px models (the owner's) fit — which is
why it reproduced only on some phones. Fixed with `max-width: 100%` on
`.rowlist`/`.announce-msg` plus the `overflow-x: hidden` guard; the
picker rows' nowrap "by <pilot>" line also got `min-width: 0` +
ellipsis so a 24-char record name shrinks instead of shoving the row
wide. Keep new wide elements under `max-width: 100%` and verify with a
headless `scrollWidth > clientWidth` check at 320–393 px widths.
**Menu animations must be compositor-only — animate `transform`/`opacity`,
never `background-position`, `top`, `box-shadow`(-ish paints on large
areas), or anything inside an SVG filter** (perf lesson, 2026-08): the menu
ran at 2–3 fps on a weak Android phone (native WebView shell) while the
WebGL game held 60, because idle menu ambience was raster-bound —
measured headless (dpr 3, 3 s idle): 5.4 s of raster tasks before vs
0.4 s after. The three offenders, each preserved visually: the grid drift
animated `background-position` (full repaint of the huge transformed
surface every frame — now the static perspective+mask lives on the
`.grid` div and its `::before` translates one 46 px grid period,
composited); the `.crt::after` sweep animated `top` (layout+paint per
frame — now `translateY(400%)` of its own 40%-height); and the ship
hero's flame flicker (9 Hz) sat under two `feGaussianBlur(18)` halo
ellipses plus a whole-SVG CSS `drop-shadow`, re-running all three blurs
every tick — the halos are now plain radial-gradient ellipses tuned to
match the blurred look, and the drop-shadow moved onto the static hull
group (`.ship-hero .hull`, a CSS `filter: url(#sh-glow) drop-shadow(…)`
chain — the CSS property overrides the group's SVG `filter` attribute, so
the chain must repeat the url).
**Menu font — bundled JetBrains Mono (2026-08)**: `fonts/jetbrains-mono.woff2`
(latin-subset VARIABLE font, wght 400–800, ~31 KB — one file covers every
weight the menu uses) leads `--mono` via `@font-face` (+ a `<head>` preload
with `crossorigin`, required on font preloads even same-origin), so every
platform renders the same face; `editor.html` declares the same face and
leads its stacks (incl. the canvas `ctx.font` strings) with it. Grew out of
the "wrong fonts on Android" report (2026-08): the stack used to end in
`"Courier New"`, which Android aliases to its slab-serif typewriter face
(Cutive Mono) — that stays OUT of the fallbacks for that reason. SIL OFL 1.1:
license text in `fonts/OFL.txt` (shipped with the site), attribution entry
+ full text on the generated third-party licenses page (the generator reads
`fonts/OFL.txt`) — fine to sell/bundle commercially, only selling the font
file BY ITSELF is disallowed. The `fonts/` dir is in all three copy lists
(build-site + both sync-web.sh, pinned by check-bundle-sync.py) and both
app shells map `.woff2` → `font/woff2` (Android's handler otherwise answers
`text/plain` for unknown extensions). Non-latin glyphs (foreign pilot
names) fall through to the platform monos per character — by design. One `.screen` is visible at a
time; the page **boots with the main menu open** (markup, not JS, so it shows
while the wasm loads):
- **scr-announce** (DORMANT — kept for future messages): a load-time
  announcement dialog ("Attention you floor scraping maggots…"); "Roger
  that" dismisses to the main menu. **The game boots straight to scr-home**
  (it holds the markup `on` class). To run a future message: set the
  `.announce-msg` text, move the `on` class from scr-home to scr-announce,
  and flip the boot state back to `"scr-announce"` in the script (the
  `overlay` init + the history boot root + `histEntries[0]`).
- **scr-home**: PEGASUS title + the **animated ship hero** (`.ship-hero`, the
  real vector ship — same geometry as `icon.svg` / `src/ship_mesh.rs` —
  inlined so it ships with `index.html`; bobs, flame flickers; hidden under
  `max-height:560px` so landscape phones keep the Fly button above the fold)
  + Fly / High scores / Settings / About. **No top-level Levels button** —
  level choice is a step inside Fly and High scores (see scr-levels).
- **scr-levels**: the **shared level picker** — level rows (a **type icon**
  on the left — derived from the level's mode keys, see the `icon` row in
  the Levels table — then
  name with the
  level file's one-line `description` beneath it, + best),
  reached two ways via `openLevelPicker(mode)` (`levelPickerMode` drives its
  title + row action): **fly mode** (from Fly, titled "SELECT LEVEL") loads
  the picked level and closes the menu to fly it; **scores mode** (from High
  scores or a board's Back, titled "HIGH SCORES") sets `scoresFile` and opens
  that level's board **without reloading the game** (viewing a board must not
  reset the flight waiting behind the menu). It's a chooser, so **no
  pre-selected highlight**. The per-row "best" is the level's **global
  all-time record** (the #1 from the board cache, refreshed by
  `prefetchGlobalBests` on every open); "—" offline/unknown. Each row
  shows "by <pilot>" under the record (both modes); **scores mode** adds
  a `›` drill-down chevron (the row opens that level's board). Row-best
  updates go through `setLevelBest` (in-place, per the rule below). **DOM-stability rule (hard-won)**: async results update the
  row text IN PLACE (`levelBestEls`) — rebuilding the rows under an
  in-flight tap retargeted the tap to whatever landed at those coordinates,
  including the Back button right below the list (= surprise exit to the
  main menu, seen in the field). The board applies the same rule by
  skipping the post-cache re-render when the fresh data is identical
  (`renderedBoardJson`). No level list (manifest fetch failed) ⇒ the picker is
  skipped and Fly / High scores go straight to their destination on the
  built-in level.
- **scr-scores**: the board for **`scoresFile`** (the level picked for
  viewing — defaults to the loaded level, decoupled from `currentLevelFile`
  so browsing another level's scores doesn't reload). The **global board**
  with Today / This week / All time period chips (**All time is the
  default** on page load) and ▶ watch buttons (see
  "Online high scores") — scores are global-only; offline builds show
  "Global scores need a connection". Back returns to the scores-mode picker.
- **scr-settings**: **Sound** (`#sound-toggle-row`, `pegasus_sound`, **off by
  default** → `set_sound_enabled` → `SOUND_ON`; off mutes the thruster loop
  and skips boom playback), **Velocity vector** (`#vel-toggle-row`), **Invert
  stick** (`#inv-toggle-row`), **Split controls** (`#split-toggle-row`,
  `pegasus_split_controls`, **on by default** → `set_split_controls` →
  `SPLIT_CONTROLS`; left-half throttle button / right-half steering stick —
  see "Input sources"), **Race best ghost** (`#ghost-toggle-row`, on by
  default), **Debug HUD** (`#debug-toggle-row`, `pegasus_debug_hud`, **off by
  default** → `set_debug_hud` → `DEBUG_HUD`; shows the telemetry text line —
  see "HUD") as styled toggles; same localStorage → export → atomic plumbing.
  Plus **Share anonymous returning-player id** (`#retid-toggle-row`) — JS-only, mirrors
  the analytics consent choice (see "Analytics"); no wasm export behind it —
  and the **Pilot name** row (`#name-row`, hidden offline) — opens the
  submit-score dialog in edit mode (see "Online high scores").
- **scr-about**: build **git revision** + **build time** (deploy-time `sed`
  of `__GIT_REVISION__` / `__BUILD_TIME__` placeholders by `build-site`;
  local fallback "dev (local build)" via `startsWith("__")`), an **App
  build** row shown only in the app shells (`#app-build-row`, the
  INSTALLED app's version — "1.0 (42)", marketing version + the CI
  run-number build — read from the shell bridges: Android
  `PegasusApp.appBuild()`, iOS the `window.__pegAppBuild` document-start
  user script; hidden on the plain website), the
  **What's new** button (`#btn-whatsnew` → **scr-whatsnew**, the changelog
  screen — see "What's new page"), the **Report a bug** button
  (`#btn-bugreport` → **scr-bugreport**: message textarea + "Save report",
  bundling the message with the last hour of the client log into a
  downloadable/shareable text file — see "Client log & bug reports" under
  "Analytics") and the
  **⟳ Reload latest build** button (`#force-reload`, same `?fresh=<ts>`
  bypass as the toast below).
- **scr-pause**: Resume / Exit to menu. **scr-gameover**: CRASHED + run
  distance + best, Fly again / Watch replay / "‹ Back" (`#btn-gomenu` —
  ends the run and opens the fly-mode level picker; home when there's no
  level list).

**Standalone launch gate (PWA jank fix, 2026-07)**: home-screen
(standalone) launches paint before WebKit delivers the safe-area insets,
so the menu laid out with its fallback padding and visibly jumped ~31 pt
down a beat into launch (same late-inset root cause as the iOS app shell
— see "iOS app" — but a PWA has no native shell to inject them). There
is **no "insets changed" DOM event; a ResizeObserver on the
`--inset-*`-sized safe-area probe divs is that callback**. A small gate
script (its own `<script>` tag between the boot guard and the bundle —
the bundle `<script src>` is the first point the parser can yield and
paint) adds `#menu.await-insets` (menu content `visibility:hidden`, dark
backdrop still painted — matches the system launch screen) on standalone
boots only, and removes it when an inset lands or after a 400 ms
fallback (devices whose insets really are 0, e.g. Android PWA).
In-browser/desktop boots are never gated (their insets are legitimately
0 — the gate would just eat the fallback delay), and a boot where the
insets are already known at parse time (the iOS shell's injection) skips
the gate entirely. Failure-safe: the class is only ever ADDED by the
gate script, so any error degrades to today's ungated boot.

During flight the only HTML is two corner buttons (`#hud-btns`, top-right):
**✕ menu** (opens scr-pause — an X, not a pause glyph, since it reads as
"leave the game view") and **⟳ restart** (same path as the R key); during a
replay they swap for a single amber **✕ exit-replay** button
(`ui_command(3)`, toggled by the 100 ms replay poll).
The menu container and the corner buttons swallow `mousedown`/`touchstart`
(`stopPropagation`, no `preventDefault` — that would kill label clicks) so
taps never reach the canvas and fire the thruster. Esc toggles the pause
screen on desktop.

### Hardware / browser back navigation
Android's back button (and browser Back / iOS edge-swipe) steps back ONE
step in the game UI instead of leaving the site. Implementation
(`index.html`, the block after the Esc handler): session history
**mirrors the logical screen stack — one entry per screen**, each tagged
`{pegasus, s: <state>, d: <depth>}`; `histPath(state)` defines every UI
state's canonical ancestor chain (`[home]`, `[home, levels]`,
`[home, levels, scores]`, `[home, game]`, `[home, game, pause]`, …;
`"game"` = no overlay: flight/wreck/replay; scr-name's path follows
`nameReturnTo`). **Why entry-per-screen instead of a single sentinel
(lesson, 2026-07)**: mobile browsers animate back with a stored
SCREENSHOT of the destination entry — with one sentinel the lone
underlying entry's screenshot (the main menu, the frame painted when the
sentinel was armed) flashed before EVERY real destination (seen in the
field: board → menu flash → picker). Entries must therefore BE their
screens; since `showScreen` flips the DOM and reconciles in the same
task (before paint), each entry's screenshot shows the screen it
represents, so the preview always matches where back lands.
On `popstate`, `uiBack()` performs the current state's own Back action by
clicking the UI's real controls (analytics logs them like taps):
generically **the screen's `.mbtn.back`** (levels / scores / settings /
about / what's-new — scr-name's Skip and **scr-gameover's "‹ Back"**
carry the class too; any new screen with an
`.mbtn.back` gets hardware back for free — so game-over's back ends the
run and lands on the fly-mode picker, matching its `histPath`
`[home, levels, gameover]`), pause → Resume, flight → pause screen (like
Esc/✕), replay → exit replay (✕); scr-consent deliberately no-ops. Afterwards `syncHistory()` (also called
at the end of `showScreen`/`closeMenu`) **reconciles** history to the new
state's path: deeper → `pushState`, lateral (the crash flow's
name → consent → game-over swaps at one depth) → `replaceState`, jumps
(UI Back buttons, exit-to-menu, fly-again) → `history.go(-k)` with the
resulting popstate swallowed (`histSwallow`) and reconciliation resumed
on arrival (`syncHistory` is a NO-OP while a traversal is in flight —
go() is async, reconciling early would double-issue it). So UI Back
buttons pop the real entries with zero per-button wiring. At depth 0
(home / announce) back leaves the site normally; a reload landing
mid-stack unwinds to a rooted announce at boot (`history.state` survives
reloads); the forward button is neutralized. **Flight guard**: in-flight,
back must PAUSE rather than exit, so once a flight frame has PAINTED
(double-rAF, `armFlightGuard`) a duplicate `"game"` entry is pushed —
back pops it (preview screenshot = the flight itself) and pause/game-over
/name/consent take its slot via replace. Verified with a scratch
Playwright suite (40 checks: entry tags per step, unwind chains, UI-back
= real pop, exit-at-root, reload self-heal, forward button, flight guard
pause⇄resume, rapid churn).

### wasm ↔ JS menu bridge (`src/main.rs`)
- `set_ui_pause(i32)` → `UI_PAUSE`: any open screen freezes the live sim —
  the physics/stick/input paths are gated on `!ui_paused` and `phys_accum`
  drains, so no catch-up burst fires on resume. **Replay playback is NOT
  gated** — a stored replay watched from the menu plays uncovered.
- `ui_command(i32)` → `UI_CMD` (swap-to-consume): 1 = reset/fly-again (joins
  the R-key reset path via `ui_do_reset`), 2 = watch the last crashed run's
  replay (only honoured in `CrashDialog`), 3 = exit the replay (only in
  `Replay`; playback freezes on its final frame and never exits by itself).
  **Race (fixed 2026-07)**: commands are consumed on the game's NEXT frame,
  so after "Fly again"/"Watch replay" closes the menu a ui-state poll tick
  can still see state 2 with no overlay and would re-open the game-over
  screen over the fresh run (pausing it — a wedge found by e2e). JS sets
  `uiCmdPending` when sending those commands and the poll's state-2 branch
  holds off until the game has actually left state 2.
- `ui_state() -> i32` / `cur_dist() -> f32`: per-frame mirrors (`UI_STATE`,
  `CUR_DIST` — exports can't read loop locals) of the mode (0 flying /
  1 wreck / 2 crash dialog or out-of-fuel game over / 3 replay) and
  `sim.max_dist`. JS polls at 200 ms:
  state 2 with no screen open ⇒ show scr-gameover; a menu-launched replay
  (state 3) returns to scr-scores when it ends without a wreck waiting.
- **Replay transport**: `set_replay_paused(i32)` / `replay_paused() -> i32`
  (shared pause state — the HTML button and the in-canvas space-bar toggle
  both drive it, so the button icon polls the game), `replay_seek(f32)`
  (bar fraction 0..1 → the exact tick, so the slider's 1000 positions are
  the only quantisation; swap-to-consume `REPLAY_SEEK` atomic with a
  NaN-bits `SEEK_NONE` sentinel), `replay_step(i32)` (steps in raw
  physics ticks — the UI sends 0.1 s = 12 per tap — auto-pausing; fetch_add
  so same-frame taps accumulate, consumed with the arrow-key steps),
  `set_replay_speed(f32)` / `replay_speed() -> f32` (playback rate; the
  button label polls the game so the in-canvas S-key cycle stays in sync),
  `replay_pos() -> f32` / `replay_len() -> f32` (per-frame mirrors:
  progress fraction + recording length in seconds). The `#replay-bar` HTML
  overlay is THREE ROWS (`.rp-row`): on top the amber ⏮ / play-pause / ⏭
  cluster dead-centre (the flanking `.rp-side` zones share `flex: 1 1 0`,
  which is what centres it — the left one is an empty spacer; every
  `.rp-btn` carries an invisible `::after` hit-area extension of +6px,
  exactly half the 12px button gap, so touch targets meet edge-to-edge
  without moving or overlapping anything) and the
  speed button right — it opens the `#rp-speed-menu` picker panel
  (absolute, anchored above the bar) rather than cycling; the `m:ss.t`
  time label on its own LEFT-ALIGNED middle row (tenths, so steps visibly
  move the clock); the full-width range slider at the bottom (the input is its
  row's full 56 px height with an oversized 32 px thumb so it's grabbable
  on touch — safe because the slider has the row to itself — and the
  visible 6 px track is drawn by the track pseudo-elements). Shows
  while `ui_state() == 3` with no menu screen open, polls at 100 ms,
  swallows mousedown/touchstart (a canvas tap skips the replay), and
  dedupes drag seeks per slider position (at most one seek per rendered
  frame reaches the sim — the atomic keeps only the latest).
- **Exit to menu** ends the run via `ui_command(1)` (reset-ended runs are
  published on the run channel but never submitted online — see "Online
  high scores"); the fresh ship waits, paused, behind the menu; Fly
  unpauses.
The in-canvas crash dialog is now just a dim + "CRASHED" + keyboard hints
(R / Enter still work — that's also the native/dev fallback); its tappable
buttons and the blob-size hint moved into the HTML game-over screen (sizes
were dropped; `fmt_size` was removed with them).

### Stale-cache reload toast
GitHub Pages caches `index.html` for ~10 min, so right after a deploy the
served page (and the `?v=` wasm cache-buster it carries) can be the previous
build. `build-site` writes `site/version.json` (`{"revision": …}`); on load,
on a 60 s interval, and on `focus`/`pageshow`/`visibilitychange` (all
throttled to one check per 30 s — the interval matters because an iOS in-app
webview that just stays open never fires any visibility event), `index.html`
fetches it with `cache: no-store` + a `?nocache=` timestamp and compares
against its baked-in revision. On mismatch `#update-toast` slides in ("New
build available — tap to reload"); tapping navigates to
`location.pathname + "?fresh=<ts>"`, which bypasses the cached HTML. Skipped
entirely in local dev (placeholder revision) and on 404 (pre-toast deploys),
and the toast swallows `mousedown` like the menu so it can't fire the
thruster. **Tap-stealer lesson (2026-07)**: the toast is fixed at z-index 60
over the menu and was "hidden" with `opacity: 0` only — an invisible
`pointer-events: all` rectangle floating exactly over the High-scores title,
so taps there "mysteriously" reloaded to the main menu whenever a newer
build existed. Hidden overlays must be `pointer-events: none` (the `.show`
state re-enables them).

### App update policy (forced-update wall + config override)
The stale-cache toast's forceful sibling (#190, born from the #171
account split): apps bake `config.json` at build time, so a backend move
or breaking change leaves old installs silently broken with no way to
reach them — and a web tab parked open for weeks is nearly as stale.
Every client checks **`app-policy.json`** at launch AND on a slow
re-check (10 min interval + on returning to the foreground, throttled
5 min — webviews fire `visibilitychange`, so resuming the app re-checks):
the SHELLS (detected via the `PegasusApp`/`__pegAppBuild` bridges) fetch
`https://pegasusmoonlander.com/app-policy.json` — the LIVE site origin,
absolute on purpose: works from any install however old (GitHub Pages
serves `access-control-allow-origin: *`, verified incl. the iOS custom
scheme's opaque origin); the WEB page fetches it RELATIVE (the same file
at the root; a preview serves an inert copy — the preview page SKIPS the
feature entirely, incl. the shared-origin localStorage cache, so the
prod policy can never leak into a staging-configured preview). The
policy is a **CHECKED-IN file: `app-policy.json` at the repo root**
(owner decision 2026-09 — it's public content served to every client, so
its history belongs in git: a policy change is a reviewed, diffable
commit that self-deploys on the push to `main`, and reverting it is a
revert). `build-site` validates and copies it (a malformed edit fails
every PR's preview deploy); it is deliberately NEVER bundled into the
app shells (WEB_ONLY in check-bundle-sync.py — fetching it live IS the
feature). Note the values usually trail the change that motivates them
by one commit: `minBuild` is a store build's CI run number and
`minWebBuildTime` a deploy instant, both assigned only when the fixed
release actually runs. All keys optional (`{}` = no verdicts):
- `minBuild` `{android, ios}`: a shell build (CI run number, the "(42)"
  in the About screen's App build) below the platform's number gets the
  **scr-update wall** — full-screen, undismissable (`walled` latch:
  `showScreen`/`closeMenu` redirect to it, hardware back no-ops — no
  `.mbtn.back`), with `message` and a store button from `storeUrls`.
  **`storeUrls` is pre-filled** (2026-09, with the install-prompt work):
  iOS = the App Store link for Apple ID 6792584910, Android = the Play
  link derived from the release `applicationId` (valid once the listing
  is live). Without an entry for the platform the wall's Update button
  is HIDDEN — a wall raised with no store link strands the player — so
  the URLs live in the file permanently, inert until a `minBuild`
  verdict actually raises the wall.
- `minWebBuildTime` (ISO instant): walls WEB pages whose deploy-baked
  `__BUILD_TIME__` predates it. The web wall's button is **Reload** (the
  `?fresh=` navigation), and the first verdict per build spends **one
  SILENT self-reload** (sessionStorage latch) before anything shows —
  the reload usually IS the fix; the latch keeps a cache-wedged client
  (the `?fresh=` lesson above) from reload-looping, and that client gets
  the visible wall instead. Dev builds (placeholder) are exempt.
- `config` `{apiBaseUrl, replayBaseUrl, wsUrl}`: **outranks the baked
  config.json IN THE SHELLS ONLY** (the online layer's `Promise.all`
  waits for both; the web page never applies it — its config.json is
  deploy-fresh at every load, so a web override could only misroute the
  live site during a shell-targeted window) — the remote fix that
  repoints old installs at a new backend without a store release; must
  be complete + https or it falls through whole. A future backend move =
  deploy the new backend, commit the override, and old installs follow
  on next launch — no weeks-long drain (pegasus-backend#38).
**A wall never interrupts a live play**: a verdict arriving mid-flight or
mid-replay latches (`wallPolicy`) and `showScreen` raises it at the next
screen — pause, crash, game over, exit; boot is menu-open, so launch
walls immediately. **FAIL OPEN, always** (the analytics/logging
philosophy): no file, bad JSON, offline, broken bridge, dev build ⇒ play
normally; walling needs an explicit well-formed "too old" verdict. The
last-seen policy is cached (`pegasus_app_policy`) so a known-stale
client stays walled offline — absence of information never walls, and a
corrected policy lifts the wall in place.

### What's new page (per-commit maintenance rule)
The About screen's **What's new** button opens **scr-whatsnew**: a
newest-first changelog of user-facing changes, each entry a short
player-language summary + localized date/time (`fmtDateTime`) + git
revision. The data is `whats-new.json`, **generated at deploy time** by
`tools/gen-whats-new.py` in `build-site` — never hand-written, because
rebase merges rewrite branch shas, so a sha pinned in a file at authoring
time would be wrong on `main`; git itself is the only truthful source of
rev/date/time. Entries are dated and sorted by **COMMITTER date** (`%cI`)
— the rebase merge stamps it with the moment the commit landed on `main`,
so the page orders by when players actually GOT each change. **Not author
date**: that survives rebases, so a long-lived branch would sort by when
its work was STARTED and sink below entries authored after it (seen live
on the first post-launch merge, 2026-07-13 — that's why this switched;
the backfill dates were rewritten to committer dates at the same time).
Two inputs, merged:
- **`Whats-new:` commit trailers — THE RULE: every commit that changes
  something a player can see or feel MUST carry a `Whats-new: <short
  player-language summary>` trailer in its commit-message trailer block.**
  One line, written for players, not commit-ese; skip it for internal
  refactors, CI, docs, test-only changes. **The note starts with a type
  prefix — `New:` (features, content, changes), `Fixed:` (bugs), or
  `Dropped:` (removals)** — e.g. "New: Replay controls auto-hide like a
  video player" (owner convention 2026-08; every pre-existing entry was
  reworded to it via the backfill + overrides files, so the whole page
  reads uniformly). **Tone (owner convention 2026-08-23): keep the note
  a little playful** — a light arcade wink where one fits ("New: The
  record ghost now photobombs your replays — every playback is a race"),
  still one line and still clear about what actually changed. Existing
  entries were left as they are; the tone applies going forward. When
  curating a branch before merge, keep the
  trailers on the commits that survive. **Gotcha (hit on the very first
  trailer)**: git parses only the LAST paragraph of a message as the
  trailer block — `Whats-new:` must sit in the same block as the
  `Co-Authored-By:` trailers with NO blank line between them; a blank
  line above it silently drops the entry. Verify with
  `git log -1 --format='%(trailers:key=Whats-new,valueonly)'`.
- `tools/whats-new-backfill.json` — hand-curated entries for the
  user-facing commits that predate the trailer convention (retro-fitted
  2026-07 from the full history). **Frozen**: their `main` shas are final
  so pinning them is safe, but never add NEW entries here — new changes
  get a trailer.

Plus one editing hook: `tools/whats-new-overrides.json` (2026-08) maps
**full merged-commit sha → replacement note, or `null` to DROP the entry**
— the only way to reword or remove an entry after its commit landed on
`main` (a rebase-merged trailer is frozen history; first used to
retroactively credit a player's suggestions, then to fold superseded
entries into their successors — e.g. the two picker-icon entries, the two
launch-shift fixes). Only
the note is replaced — rev/date still come from git — an unknown sha is
silently ignored (it can't invent or delete entries), and entries match by
prefix so
git's growing abbreviation length can't unhook one. Reword future entries
here rather than rewriting history; entries not yet merged just get their
trailer amended.

The deploy/preview/CI workflows check out with **`fetch-depth: 0` — this
is load-bearing**: on a shallow clone `git log` silently loses every
trailered commit below HEAD (the generator degrades to backfill-only
rather than failing the deploy; CI validates generation + JSON on every
PR). The screen fetches `whats-new.json` lazily on first open
(`cache: no-store`); a local dev build has no deploy step, so a missing
file shows an explanatory hint, never an error.

## Analytics (web only)

A self-contained module at the end of `index.html`'s main script
(`pegAnalytics`) batches gameplay/funnel/error events to the pegasus-backend
`POST /v1/events` endpoint. **It must never break the game**: every public
entry point is try/caught, sends are fire-and-forget, and everything
degrades to silence.

- **Enablement**: the deploy-injected `config.json`'s `apiBaseUrl`
  (missing/empty = off; ONE fetch shared with the online-highscores block —
  its `.then` calls `pegAnalytics.configure(c)`, which needs only
  `apiBaseUrl`, before the stricter scores checks). Builds are env-tagged
  `prod` / `preview` (`/pr-<n>/` path) / `dev` (placeholder revision); dev
  additionally requires `localStorage.pegasus_analytics_debug = "1"`.
  Previews DO send (tagged) — every PR exercises the pipeline; queries
  filter `env='prod'`. **Automated browsers are dropped** (`configure`
  bails, `apiBase` stays null): `navigator.webdriver` (the W3C flag
  Playwright/Puppeteer/Selenium set) OR a `HeadlessChrome` UA token —
  added 2026-07 after automated headless-Chrome traffic looping the game
  inflated plays/sessions (`linux/chrome/desktop`, always-skip submits).
  The `pegasus_analytics_debug` override keeps the e2e suite (itself
  automated) able to exercise the pipeline.
- **Privacy**: anonymous by default — a per-page-load `sessionId` in memory,
  no cookies, no IP/UA stored (backend contract). Payloads are content-free:
  enums, element ids, numbers; the only free text is error messages. The
  About screen carries the plain-language privacy note (`#privacy-note`).
- **Consent / returning-player id**: a one-time equal-weight FULL-SCREEN
  ask (`#scr-consent`), slotted into the crash flow between the
  submit-score dialog and the game-over screen (crash → [scr-name] →
  [scr-consent] → scr-gameover; `consentDue()` picks the submit dialog's
  `returnTo`) once the device has **≥ 3 plays** (`pegasus_play_count`,
  bumped per observed run_end, device-local, never sent, capped at 100) —
  never a consent wall; the game plays identically either way and both
  buttons land on scr-gameover. Accept → `pegasus_device_id` (random UUID)
  + `pegasus_analytics_consent = "1"`, and the id rides the batch envelope;
  decline → `"0"`, never re-asked while the key survives; the
  `#retid-toggle` Settings row mirrors/revokes (revoke deletes the id).
  Testing aid: the `#reset-consent` Settings button (visible while the
  Debug HUD toggle is on) forgets the answer + id and re-arms the
  per-session latch, so the next crash re-runs the flow (`pegasus_play_count`
  is left alone — a test device is already past the gate).
  **Safari ITP purges script-writable storage after 7 days of Safari use
  without a visit** — a long-lapsed device loses the key and is legitimately
  re-asked; D30-style retention undercounts iOS/Safari.
- **Events** (allowlisted backend-side; see pegasus-backend CLAUDE.md):
  `session_start` (touch/viewport bucket/dpr + the **device-mix enums**
  `os`/`browser`/`display`/`formFactor` — parsed CLIENT-side in
  `deviceInfo()`, the raw user-agent never leaves the device; handles the
  iPadOS-pretends-to-be-macOS unmask (`MacIntel` + `maxTouchPoints > 1`),
  in-app webviews (no `Safari/` token on iOS, `; wv` on Android) and
  installed-PWA detection (`display-mode: standalone` media query or
  Safari's legacy `navigator.standalone`) + **acquisition**: the
  referrer ORIGIN — `new URL(document.referrer).origin`, same-origin and
  app-opened/empty referrers skipped — plus `utm_source/medium/campaign`
  from the page URL. Origin-only is all a cross-site referrer ever carries
  now (`strict-origin-when-cross-origin` default; Safari ITP truncates
  harder), and links opened from native apps/QR codes usually have NO
  referrer — so utm-tagged links are the reliable attribution channel;
  the About privacy note discloses "which site linked here"),
  `session_end` (on every
  tab-hide via beacon — durationMs/activeMs/hiddenMs/bgCount/runs; analysis
  takes the max per session, no heartbeats), `screen_view` (in
  `showScreen`, consecutive-deduped), `ui_click` (ONE delegated listener on
  `#menu` — nearest ancestor with an id, containers skipped — plus the
  corner buttons via `onTap`, whose `preventDefault`ed touchstart never
  produces a click), `run_start`/`run_end` (wasm channel below; cause 3 = a time level completed),
  `replay_watch`, `score_submit` (the crash-flow submit dialog's outcome:
  submitted / skipped + level stem; the Settings name editor — no pending
  run — emits nothing), `error` (drained from the boot guard's
  `window.__pegErrs` buffer — the guard stays standalone, push-only, ≤10
  entries), `stale_cache` (toast shown / reload), `consent_choice`.
- **Wasm run channel** (`src/main.rs`, next to the RUN_SEQ block):
  `run_start_seq()` bumps when the armed-idle gate flips;
  `report_run_analytics` publishes cause/ticks/dist/fuel/hull mirrors +
  `run_end_seq()` — **no GHOST_MIN_SECS gate** (short runs are difficulty
  signal, flagged `short` = < 240 ticks JS-side), zero-tick runs skipped,
  payload stored before the seq bump. Called at both `report_run_end` call
  sites; the highscore channel is untouched. Polled by JS on the existing
  500 ms `collectEndedRun` cadence and once before a level switch (the run
  banks under the OLD level).
- **Transport**: in-memory queue (cap 100, drop-oldest), `(sessionId, seq)`
  is the server-side dedupe key; flush every 15 s via `fetch` (string body)
  and on `visibilitychange→hidden`/`pagehide` via `navigator.sendBeacon`.
  **Both sends are `text/plain` CORS simple requests — no preflight. Never
  "fix" this to a Blob typed `application/json`**: that forces a preflight
  sendBeacon handles inconsistently; the lambda parses the raw body and
  ignores content-type by design.

### Client log & bug reports (2026-08)
`window.pegLog` — a standalone `<script>` between the boot guard and the
launch gate (own tag for the same isolation reason; it must load BEFORE
the bundle so its console wrappers see everything the wasm logs) — keeps
a rolling ring buffer of the LAST HOUR of client activity: chain-wrapped
`console.log/info/warn/error` (the boot guard's console.error wrap stays
intact underneath), window `error`/`unhandledrejection`, a boot line,
every analytics event (`pegAnalytics.enqueue` mirrors into it — that
runs even when analytics is off/dev/webdriver, so the timeline is always
there), explicit lines at the submit POST/response and level loads, and
**every fetch()** (the wrapper is installed before the bundle, so the
wasm/level/config/board/blob requests are all covered): method + URL
(**origin-stripped — path+query only, no domains**, whatever the
request's host) + status + duration, plus the first 120 chars of the
response body on a NON-ok status only — happy-path bodies are never logged (a submit body
carries the whole replay blob). The wrapper returns the ORIGINAL
promise and its observer branch swallows its own rejection, so it can
neither alter request semantics nor spawn duplicate unhandled-rejection
noise.
Caps: 500 entries, 300 chars/line; persisted THROTTLED (10 s + pagehide/
tab-hide) to `localStorage.pegasus_log` and restored at boot, so a
report filed after a reload still carries the crash before it. Every
path try/caught — logging must never break the game (same philosophy as
analytics).

The About screen's **Report a bug** (`scr-bugreport`, histPath
`[home, about, bugreport]` — the `.mbtn.back` gives it hardware back for
free) takes a free-text message plus optional **attachments** (an
"Attach files" row → hidden `<input type=file multiple>`; picks
ACCUMULATE into `bugAttachments` — the input resets after every pick,
since a file input's own list replaces — and each listed file carries a
✕ to remove it; a DELIVERED report clears the form — message,
attachments, status — when the player leaves the screen
(`bugReportLeft` in `showScreen`/`closeMenu`), while an unsent draft
survives backing out on purpose) and builds a
**ZIP archive**: `report.txt` (build rev, app build (shells), URL, user
agent, viewport, loaded level, open screen, the `pegasus_*` settings
keys — EXCLUDING `pegasus_device_id`, the anonymous id stays on the
device, and the bulky keys: log, board cache, editor doc, custom level,
date locale, recent runs — the message, a replay/attachment index, then
the hour of log lines), `replays/<n>-<ts>-<stem>.pgrec` — the **last
five finished runs** with played-at timestamps (`recentRuns`, captured
in `collectEndedRun` BEFORE the offline and short-run gates so it works
without a backend and keeps non-scoring attempts — the wasm publishes
every armed run; persisted best-effort to `pegasus_recent_runs`, capped 5 runs /
~1.5 MB base64, always keeping the newest) — and `attachments/<name>`
(sanitized, collision-deduped). The zip comes from `buildZip`, a
dependency-free STORE-only writer (deliberate: replay blobs are already
deflated and images already compressed; CRC-32 + UTF-8-name flag,
whole-archive cap 25 MB). **Delivery is user-driven, never uploaded** —
ladder: Web Share API with the zip (mobile share sheet; AbortError =
user closed it, not a failure) → `<a download>` blob (desktop/Android
browsers) → clipboard, which can only carry `report.txt`'s text (FIRST
choice under the Android app shell, which has no share API and a
WebView that silently ignores `<a download>` without a download
manager); `#bug-status` reports which path ran. Covered by a scratch
Playwright e2e (zip structure + report content, attachments, seeded
replays, console capture, event mirror, device-id exclusion, back
navigation, reload persistence).

**Hard-won caveat (2026-07): query strings do NOT reliably bust the cache.**
An intermediary on the owner's phone served one broken `pr-59/index.html` for
2+ hours across many `?fresh=<unique>` loads (the `no-store` `version.json`
fetch was served stale too, so the toast never fired) while the same
deployment worked instantly at a never-before-seen `pr-<n>/` path. When a
preview path looks wedged on a device: do NOT trust `?fresh=` testing — open a
throwaway draft PR pinned to the suspect commit and test at its virgin
`pr-<n>/` URL instead (that bisects "bad code" vs "stale delivery" in one
step). The boot guard (see Project structure) exists because the wedged page
was a script-killing SyntaxError that also disabled the reload button and all
error reporting.

## Key constants & configuration (world/gameplay constants live in `src/world.rs` and `src/main.rs`)

| Symbol | Value | Purpose |
|--------|-------|---------|
| `SCALE` | 80.0 | World-to-pixel ratio (physics/world units only — do **not** use for rendering) |
| `SEG_LEN` | 3.0 | Cave segment length in world units |
| `HALF_WINDOW` | 80 | Segments loaded each side of ship |
| `PERIOD` | 600.0 | Cave repeat period in world units (x) |
| `V_PERIOD` | 90.0 | Vertical repeat period (y): identical cave layers stack every 90 m |
| `SHAFT_SPACING_SEGS` | 50 | Vertical shaft slot every 50 segments = 150 m (4 per `PERIOD`) |
| `SHAFT_OPEN_SEGS` | 3 | Shaft opening width: 3 segments = 9 m |
| `SHIP_SCALE` | 1.5 | Render scale multiplier applied inside the `rot` closure — makes the ship visually 1.5× larger than the raw SWF coordinates without touching `SHIP_TRIS`/`SHIP_DETAILS` |

## Levels (data-driven worlds)

A **`Level`** (src/world.rs) is a parameter block for the whole procedural
generator — all world generation is `Level` methods, so a level IS the world:

| Key | Values | Effect |
|-----|--------|--------|
| `name` | text | Cosmetic (picker label, not in replay headers) |
| `description` | text | Cosmetic one-liner shown under the name in the level picker (JS-only — the wasm parser ignores it like any unknown key) |
| `icon` | `stopwatch` / `hourglass` / `arrow` | Cosmetic picker-row TYPE glyph override (JS-only, like `description`) — normally omitted: the icon derives from the mode keys so levels of the same type share it (owner direction — the glyph classifies the game mode, not the level's personality): time-scored → stopwatch (Hollows, Dash — goal trials are clock-scored), `time_limit` → hourglass (Sprint), else double arrow (distance). Small inline SVGs in `index.html`'s `LEVEL_ICONS` (self-contained — no icon fonts), cyan for distance, **amber** for the clock modes (`AMBER_ICONS`). Only names in the map reach innerHTML — level text can't inject markup |
| `scoring` | `pads` / `distance` / `time` | Pads: +100 per first landing. Distance: score = max \|x\| reached (`Sim::max_dist`; big HUD readout, `best` beneath). Time: visit EVERY pad — the run ENDS the tick the last pad's landing registers (`Sim.completed`, `TickReport::completed`); score = completion time in seconds (`Sim.run_ticks × PHYSICS_DT`, **lower is better**), HUD shows `visited/total` + a running `TIME m:ss.t` clock at near-headline size (the clock IS the score; frozen at completion), with `BEST m:ss.t` + the "by <pilot>" record attribution beneath (`BEST_TIME`, seeded from the global all-time record like the distance BEST — see "Online high scores"). A crash/fuel-out is a DNF — no board entry. Time levels are hand-drawn (finite pad set; `terrain.pads.len()` is the total) — or procedural with a `goal_distance` finish pad (see that row): there the HUD's big line is distance progress (`837/1000 m`) and ONLY the finish pad completes (regular pads register + refuel silently, no flash) |
| `endless` | on/off | On: the cave's periodic harmonics (`cave_center`/`cave_half_width`) are replaced by hash-based **value noise** (`Level::vnoise`, smoothstep-interpolated lattice hashes) with the SAME amplitude bounds — the tunnel never wraps in x, every stretch is unique rock in both directions. The no-pinch / no-blowout guarantee carries over (unit-tested over ±34 km); C1-continuous so colliders/lattice stay seamless. Procedural only (ignored under `terrain`) |
| `shafts` | on/off | Off: `seg_in_opening` is always false (sealed cave), no shaft colliders load, minimap skips the carve |
| `obstacles` | on/off | Off: `obstacle_spec` returns None everywhere (pads then skip the boulder-overlap check) |
| `pad_spacing` | 40–2000 (clamped) | Metres between pad slots (`PAD_SPACING = 130` is the default) |
| `time_limit` | 5–1200 s (clamped) | **Hard run clock** (2026-07, The Flux Sprint): the run ENDS the tick the clock reaches the limit — `Sim.completed` fires like a time level's last pad, and the sim **parks the ship where it is** (photo finish: forces/torques reset, velocities zeroed, gravity scale 0, `prev_vel` zeroed so the park can't read as an impact dv) so the controls-dead ship can't coast into rock during the game-over grace. Stored as `Level.time_limit_ticks` (seconds ÷ `PHYSICS_DT`, exact integer cutoff — identical in live play and resim; `Sim::restore` re-seeds the completed+parked state from a keyframe's `run_ticks`, so replay seeks land on the frozen finish). Score keeps the level's own scoring — on `distance` the score is the frozen `max_dist` (`max_dist` is gated on `!completed`). HUD shows a **countdown** (`TIME m:ss.t` remaining, clock-sized, amber ≤ 10 s / red ≤ 5 s; the BEST line moves down into the attribution slot); banners/game-over read **TIME'S UP** instead of LEVEL COMPLETE (`complete_msg`, `levelHasTimeLimit` JS-side). Forces **replay format v5** (see "Hybrid recording"); the clamp keeps runs far below the backend's 60-min resim cap |
| `goal_distance` | 100–20000 m (clamped) | **Goal time trial** (2026-07, The Flux Dash): a **FINISH pad** at x = ±goal_distance (both directions — deck built like `pad_spec` over the max floor, but NEVER skipped; `Level::goal_pad_spec`), keyed in `Sim.pads` on the `GOAL_SLOT_POS`/`GOAL_SLOT_NEG` sentinels (i64::MAX / MAX−1 — the pads window tests them by true x position, same ±20 m margin as the slot window so retain/insert never churn; replicated per layer like every pad). On a `time`-scored level the finish pad's FIRST landing ends the run (`Sim.completed`; the ship is already settled on the deck, so no park is needed — unlike `time_limit`); regular pads along the way register + refuel but never flash or complete. `obstacle_spec` keeps 9 m and `pad_spec` 12 m clear of the finish; `stand_y` prefers the finish deck (inert on shipped levels, load-bearing for tests/restores onto it). Keyframe `visited` mask on goal levels: bit 0 = +x finish, bit 1 = −x (restore rebuilds visits + completed from them). Procedural only — parse zeroes it under `terrain`. Forces **replay format v5** |
| `seed` | u32 / `random` | **0 = the legacy world bit-for-bit** (zero harmonic phases, untouched slot hashes — pinned by the pre-Level unit tests still passing unchanged). Any other seed re-phases the cave harmonics and re-keys every slot hash. The half-width harmonics guarantee ≥ 2.5 m clearance for ANY phases (unit-tested), so no seed can pinch the cave shut. **`seed = random`** (`Level::random_seed`, 2026-07): the game rolls a fresh CONCRETE seed at every level load AND every restart (`with_rolled_seed` in main.rs — frame-side wall clock × counter, never 0; nondeterminism stays out of sim.rs), so each attempt flies brand-new rock. The flag is metadata only: world gen reads `seed`, `LevelParams` carries the rolled concrete value (not the flag), so replays/ghost/verification re-sim the exact world flown. The identical-re-push no-op uses `Level::same_file_as` (seed-neutralized for random levels) instead of `==`. The racing ghost is inert on such levels — a pushed ghost's recorded seed never matches the fresh world, so the adoption params equality drops it (BEST/record name still work). **One leak needed a second gate (fixed 2026-08)**: when YOU set the record, the submitted run comes back as the ghost while the wrecked sim still holds the seed it was flown on — it passes adoption, and the restart re-roll would then race it through the wrong rock; the reset block re-checks `ghost_rec` against the re-rolled params and drops the orphan (unit-tested) |
| `poly` | `x,y x,y …` | **Hand-drawn terrain** (Across / Elasto Mania model): one SOLID ROCK polygon per line (≥ 3 verts, concave OK, overlaps OK — buried edges are unreachable; winding normalized to CCW on parse). Any `poly` line puts the level in hand-drawn mode: `Level.terrain = Some(Terrain)`, procedural gen off (shafts/obstacles forced off), every edge a segment collider loaded ONCE (no sliding window — hand-drawn maps are finite; `Sim.terrain_loaded` guards the one-shot insert, fixed file order keeps Rapier handle numbering deterministic) |
| `pad` | `x,y` | Hand-placed pad (deck centre x, deck top y) — terrain levels only; keyed `(index, 0)` in `Sim.pads`, same landing/refuel/score logic |
| `start` | `x,y` | Terrain levels: a NEUTRAL start platform under the spawn — a plain high-friction deck NOT in `Sim.pads` (no visit/refuel/score fires there), so a Time level's launch spot isn't a freebie visit. Defines the spawn ground (`spawn_y = start.y`); drawn as a dimmer, light-less deck, grey on the minimap |
| `spawn_y` | f32 | Terrain levels: ground y under the spawn at x = 0 (`stand_y` returns it + 0.78); overridden by `start`'s y when a start platform is present |

`Level::parse` reads `key = value` lines (# comments; unknown keys ignored
for forward compatibility; missing keys keep `Level::demo()` defaults — the
legacy world). Shipped levels (pinned by `include_str!` tests): **The
Expanse** (no shafts, boulders), **The Glide** (no shafts, no boulders),
**The Flux** (2026-07: an **endless** procedural cave — `endless = on`
value-noise cave that never wraps, no shafts, boulders, pads every
~150 m — with `seed = random`, so every load and every restart rolls a
brand-new endless cave; no racing ghost by nature, but full
replays/boards — see the `seed` row above; it absorbed **The Rift**, the
same level with a fixed seed, retired 2026-07 as too similar), and **The
Flux Sprint** (2026-07: The Flux's endless random cave under a hard
**one-minute clock** — `time_limit = 60`, see the `time_limit` row: as far
as you can before the horn, still distance-scored/boarded in metres; full
throttle burns the tank in ~28 s, so the minute is also a fuel problem) —
all distance-scored — plus the time-scored levels: **The Flux Dash**
(2026-07: the endless random cave as a **1,000 m time trial** —
`goal_distance = 1000`, see that row: race to the gold FINISH pad at
±1000 m, landing on it ends the run, lowest time wins; the kilometre
outlasts the tank, so refuel stops on the way are part of the trade),
the (2026-08) **Sprint/Dash variants of The Expanse and The Glide**
(`expanse-sprint` / `expanse-dash` / `glide-sprint` / `glide-dash` —
the base level's exact world params plus only the mode key, pinned by a
unit test; **fixed seed 0**, so unlike the reshuffling Flux family the
racing ghost works on them; the manifest groups each world with its
variants, and `shipped_dash_finish_decks_are_landable` lints every
shipped finish deck's headroom incl. 300 rolled flux-dash seeds) and
**The
Hollows** (2026-07: the first HAND-DRAWN level — five chambers joined by
tunnels, five pads scattered through them (incl. a perch on the west
tunnel's sill) plus a neutral `start` platform in the spawn chamber,
**time-scored**: visit all five as fast as you can, the run ends on the
last pad; its geometry-lint unit test asserts every chamber / tunnel /
pad / start waypoint is open space via `Terrain::point_in_rock`).
**The Caves** (the original shafted world) was retired 2026-07 with The
Rift — its world survives as the compiled-in `Level::demo()` (`pads`
scoring), which remains the no-manifest fallback and the fixture for the
pad-scoring / world-geometry unit tests.

**Hand-drawn rendering** (`terrain` levels): each rock polygon draws as an
ear-clip-triangulated `rock_dark` fill (`render::triangulate`, cached in
main's `TerrainMesh` keyed on the RENDERED terrain — it follows `world_sim`,
so replays of other levels swap the cache) plus two faceted edge bands
extruded along the inward normal (CCW ⇒ edge dir rotated +90° points into
rock; depth 0 sits EXACTLY on the polygon edge = the collider — same
alignment rule as lattice row 0), lit by the same radial shader. **Edge
bands skip BURIED stretches** (per ~2 m band step, sampled 6 cm outside the
step midpoint via `point_in_rock`, precomputed in `TerrainMesh.exposed`):
overlapping polys are a first-class idiom (the Hollows frame; the editor's
carve splits donuts into pieces with coincident seam edges) and a lit band
on a buried stretch paints a bright seam through solid rock — seen in the
field on the first carved hole. The editor's outline pass applies the same
rule (a SELECTED poly still shows its full outline for editing). The
procedural wall/shaft lattice loop is skipped (`terrain.is_some()` break);
obstacle/shaft/pad loops self-gate via their empty maps. Minimap: dark open
space with the triangulated rock drawn on top; pad legs use a short fixed
drop (no floor curve to reach for).

**Decoupled from the wasm**: `index.html` fetches `levels/manifest.json` +
each `.level` file (cache-bypassed), feeds the level picker (scr-levels —
labels = the files' `name =` line), and pushes the picked level's
raw text into the game: `level_buf_ptr(len)` returns a wasm-side buffer, JS
writes the UTF-8 bytes via `wasm_memory.buffer`, `load_level(len)` parses it
into `PENDING_LEVEL`. The main loop applies a pending level at the next frame
boundary as a **full fresh start** (new `Sim`, new recorder, ghost dropped —
it was flown on a different world; a re-push of the identical level is a
no-op). Adding a level = add the file + list it in the manifest (removing
one is the reverse — also drop it from `shipped_levels()`, the sync test
pins the two together); the deploy copies `levels/` verbatim
(`build-site`). Selection persists in `localStorage` (`pegasus_level`).
**There is no "default level"** — Fly always goes through the picker; boot
just pre-loads the saved selection (else the first manifest level — also
the recovery path when a saved level was retired from the manifest) behind
the menu to keep its record/ghost warm. Manifest fetch failure skips the
picker and the game stays on the compiled-in `Level::demo()`.

**Level editor & custom drafts** (`editor.html` + the custom-level block in
`index.html`, 2026-07 — the issue #89 v1): a standalone editor page for
hand-drawn levels. Tools: draw-polygon (tap vertices, close on the first
vertex / CLOSE POLY / Enter — **or drag to draw FREEHAND**: pointer samples
are RDP-simplified on release with a zoom-scaled tolerance (~2.5 px, clamped
5–60 cm) so a finger-drawn blob lands as a sane vertex count, deliberately
un-snapped; a long stroke ending on the first vertex auto-closes), **carve**
(same input as draw, red preview — a finished cut becomes a **persistent,
EDITABLE cut object**: select it to move/reshape it, delete it to restore
the rock. The document is an **ordered op stack (`doc.ops`,
`{t:'rock'|'cut', p}`) where the NEWEST op wins** (owner request 2026-07,
"how do I draw rock inside a carving?"): rock over a cut is an island, a
cut through that island opens it again — `pointInRock` scans the stack in
reverse, fills composite in order on an offscreen layer (cut =
destination-out), outlines stroke only where the two sides of an edge
DISAGREE about being rock, and `compiledPolys` folds the stack into plain
polygons at export/test-fly (rock appends; a cut subtracts from everything
accumulated so far, so it only affects rock BELOW it) — the game's level
format is untouched. `normalizeDoc` migrates the older flat polys+holes
documents (all rock, then all cuts). Exported files carry the full
document in an `# editor-doc:` comment (ignored by the game's parser) so
IMPORT restores the editable stack instead of baked pieces. The bake per
cut:
`polyDifference`, a Greiner–Hormann difference:
subject walked forward outside the clip, clip walked backward inside the
subject, so boundary-crossing cuts split polys into pieces), **shape modes
FREE / RECT / CIRC** for both draw and carve (rect: drag corner-to-corner
or tap two corners; circle: drag from the centre or tap centre-then-edge,
polygonized at ~1.2 m arc steps, 12–48 verts). **Finishing anything —
closing a free polygon, applying a cut, committing a shape — auto-switches
to SELECT with the new object selected** (owner request: draw → adjust is
the natural loop). (Carve internals: a cut fully
INSIDE a poly makes a donut, which needs NO single-polygon trick — rock is
the UNION of polys and buried edges are harmless (the Hollows frame rule),
so `holeSplit` slices the donut through the hole with two half-plane
rectangle cuts and subtracts the hole from each piece, every step a
boundary-crossing `polyDifference` — exact area, only simple polygons, the
pieces' coincident split-line edges buried between rock on both sides;
verified flyable + rendering in the real game. An earlier keyhole-slit
version was replaced by this after the owner asked why one polygon at all.
The carve polygon is jittered ~1 mm pre-clip so grid-snapped
vertex-on-edge/collinear degeneracies can't occur), select (drag vertices,
tap an edge midpoint to insert one, drag/delete whole polygons, pads, the
start platform), pad/start placement, **group/lock** (owner request
2026-07, "keep them in the same relative position once we are happy": a
MULTI toggle in select mode collects elements by tapping, GROUP binds them
— tapping any member then selects the whole group and one drag moves all
members in lockstep, incl. the start pad, which re-origins on release;
LOCK on a group (or a lone element — it gets a group of one) freezes it:
selectable but not draggable/deletable until UNLOCK. Membership = op `g`
property / a third slot on pad/start arrays the exporter never reads;
group metadata in `doc.groups` — all editor-doc-only), **scale & rotate
grips** (issue #115: two amber grips off the selection bbox — scale at the
bottom-right corner, rotate at the top-right — transforming a single op or
an unlocked group uniformly about the bbox centre with a live ×/° readout;
vertices land on the 1 cm export grid, NOT the 0.5 m edit snap; grid snap
ON snaps the angle to 15° steps; pad/start POSITIONS transform — deck
width is game-fixed — and a group containing the start pad re-origins on
release), **DUPE** (copies the selected op/pad — or a whole unlocked group
into a NEW group — offset 2 m and auto-selected; a duplicated cut joins
the stack top like a fresh carve; the start platform is unique and never
copied), a **rock↔cut FLIP** on the selected op (→ CUT / → ROCK — the op
keeps its stack position; DUPE + scale-down + flip carves a narrow tunnel
following any wall's own contour, the owner's tunnel recipe),
a **MOVE toggle** in select mode (drags always move whole
elements — vertex/midpoint handles can't be grabbed, the cure for
fat-finger vertex grabs while arranging; the off-bbox scale/rotate grips
stay live), **play-feel aids**: the spawn marker is the REAL vector ship at
true scale (issue #114 — `SHIP_GLYPH`, the hero-SVG polys remapped to
~1.43 m with feet where `stand_y` parks them; a faint ring keeps it
findable at far zoom instead of inflating it), a **1:1** button zooming to
this device's in-game `view_scale` (min CSS dim < 600 → fit 19 m, capped
at desktop; else 80 CSS px/m) and a **🔒 ALL browse mode** (everything
behaves locked, drags pan — tour the level without disturbing it;
session-only), 0.5 m grid snap
(toggle), undo/redo
(snapshot stack, Ctrl+Z), import (paste any hand-drawn `.level` text —
parses the same key subset as `Level::parse`) and export (copy/download).
**The start pad IS the origin** (owner decision, 2026-07): the sim always
spawns at x = 0, so instead of moving the spawn, placing/moving/importing a
start platform re-anchors the whole level around it (`reoriginToStart` —
every x shifts so start.x = 0, the view shifts along so only the grid
appears to move); the spawn marker rides the start pad. Pointer-events
driven so mouse and touch share one path; **two fingers are always
pan/zoom** whatever the tool, taps commit on pointer-UP — in draw/carve a
one-finger drag is the freehand stroke (pan via two fingers / middle mouse
/ wheel), elsewhere it pans. **Touch ergonomics** (owner feedback,
2026-07): coarse-pointer devices draw bigger handles (`HANDLE_R`), TOUCH
input gets a fat grab radius per event (`TOUCH_PX = 28` vs 11 for mouse),
and with a big radius the NEAREST handle wins (vertices before midpoints
— first-match order grabbed the wrong vertex on dense circle polys). The
contextual DELETE / CLOSE POLY / CANCEL buttons FLOAT over the canvas
(`#ctxbar`) — in the toolbar they re-wrapped it on narrow screens every
time the selection changed, shifting the whole canvas ~40 px so the
vertex you were reaching for jumped away from your finger (found by the
touch e2e; the toolbar must never reflow mid-interaction). The status bar
obeys the same rule with a FIXED height (field bug, 2026-07: a long tool
hint wrapping on a phone grew the bar, shrank the canvas, and — with the
bitmap unresized — freehand carve strokes drew ~30 px above the finger);
a ResizeObserver on the canvas re-syncs the bitmap for any remaining
CSS-size change (dynamic browser chrome etc.). The working
document autosaves to `localStorage.pegasus_editor_doc` (survives the
test-fly round trip); a validation pass (a JS port of
`Terrain::point_in_rock` plus the backend verifier's `MAX_TERRAIN_*` caps)
checks the spawn point, pad landing pockets, degenerate polys and
time-scoring-needs-pads, surfaced live in the status bar and as a blocking
"fly anyway?" dialog. **TEST FLY** stores the exported text under
`localStorage.pegasus_custom_level` and opens `./?custom=1`; the boot
pushes the draft through the normal `pushLevel` path (pseudo-file
`custom-draft.level`, NEVER added to `levelFiles` — no picker row, no
persisted selection) and closes the menu straight into flight. **During a
draft flight the ✕ corner button returns to `editor.html`** instead of
opening the pause screen, and **so does the game-over screen's "‹ Back"**
(hardware back rides both buttons), closing the edit → fly loop from
flight and from a crash alike; the ⟳ restart button works as normal. **Drafts are
strictly local**: guards in `maybeSubmitOnline` (never submitted),
`applyGlobalRecord` (no backend fetches) and
`renderGlobalScores` ("Draft levels have no global board") keep them off
the online layer — a draft isn't a shipped level, so its board would be a
junk stem. Sharing a finished level still means adding the exported file to
`levels/` + `manifest.json` via a PR (player-hosted sharing is future work
on #89).

**Distance high score**: `Sim.max_dist` (farthest \|x\| this run, reset by
restore) raises the `BEST_DIST` atomic **only when the run ends**
(`raise_best_dist` at the three run-end sites: destroying crash, fuel-out,
alive reset — never live mid-flight, so a record-beating run keeps the
PREVIOUS best on the HUD as the number to chase; the big readout already
shows the current distance); after every level load
`applyGlobalRecord` seeds it with the level's **global all-time record**
via `set_best_dist()` (see "Online high scores"; `load_level` zeroes it so
records never leak across levels). The HUD BEST is the world record — or
the session's own best where that's higher / offline. There is **no local
persistence** (the old `pegasus_best_<file>` mirror is gone; stale keys
are cleared at boot). **Record flag** (2026-08): on Distance levels a gold
pennant is planted in-world where the current record ends — at BOTH ±BEST,
since the record is max |x| — lettered with the record holder's initial
(first char of `BEST_NAME`; blank pennant when no holder is known, "Y"
after the "by you" flip). Drawn in the pads/particles block of main's
frame loop, replicated per loaded layer like pads; the ground height comes
from `stand_y` on the LIVE level every frame (so a pad/finish deck under
the line holds the flag on its deck) — load-bearing on `seed = random`
levels, where each attempt's rock differs and the flag must stand on THIS
world's floor. A record line inside a shaft opening (no floor) is nudged
inward onto solid rock by `record_flag_x` (pure, unit-tested). Live world
only (not drawn during replays — a watched replay can be a foreign level
whose record this isn't) and procedural only (terrain levels have no floor
curve; no shipped hand-drawn level is distance-scored). The **record
progress bar** rides the same gates: a discreet slim bar along the
minimap's inside bottom edge, filling continuously as the run's `max_dist`
closes on the record, flag-gold, pulsing when matched.

**Replays**: physics depends on the level, so the recording header carries
`LevelParams` (scoring/shafts/obstacles/pad_spacing/seed — NOT the cosmetic
name; added in format v2) and `resim`/`ResimPlayer` rebuild the level from
the header (`Level::from_params`) — a replay re-runs in the world it was
flown in, unit-tested bit-exact on a non-demo level too
(`resim_reproduces_on_a_custom_level_bit_exactly`) and on an endless cave
(`resim_reproduces_on_an_endless_level_bit_exactly`). New-feature levels
carry a **flags byte + optional Terrain** in the header (**format v4** —
flag bit 0 = endless, bit 1 = terrain present, then the Terrain block incl.
the optional start platform; written only when the level is endless,
hand-drawn, OR time-scored; legacy procedural recordings still serialize
byte-identical v3 and `deserialize` accepts both, so every pre-existing
blob keeps decoding). v4 keyframes additionally append **`visited` (u64
terrain-pad bitmask) + `run_ticks`** (60 B vs v3's 48 B): `Sim::restore`
re-seeds `visited_pads`/`completed`/the frozen run clock from them, so a
replay SEEK lands on the correct x/5 count, beacon colors and completion
state instead of losing them with the rebuilt scratch sim (procedural
levels keep their old session-state semantics — mask 0).
Hand-drawn replays are thereby self-contained
(`resim_reproduces_on_a_hand_drawn_level_bit_exactly` round-trips through
serialize/deserialize). **Backend caveat**: the deployed verifier's pinned
sim-core must understand the recording format and know each shipped
level's params (THE REPIN RULE in the backend repo's CLAUDE.md). The
deployed verifier decodes **v3/v4/v5** (the current pin covers the Sprint/
Dash v5 fields, their forgery guards and every shipped level's params); a
newly shipped level verifies immediately as an unknown stem (the level
check is skipped by design, physics still fully verified) and gains its
params pinning at the next repin — EXCEPT when the level needs a NEW
recording format, whose submissions are silently discarded until the
verifier's repin decodes it (this bit The Flux Sprint/Dash at v5's
introduction). The **cosmetic trailer** (see "Hybrid recording") needs no
repin by design — but note its security rules there: presentation-only,
enum-whitelisted values (never free text — trailer content is
attacker-controlled and reaches other players' screens), and the backend
rejects malformed or oversized tails at submit
(`Recording::deserialize_with_tail` → `TailInfo`, cap
`TRAILER_MAX_BYTES`), so the ignored-trailing-bytes property can't park
arbitrary content in public replay storage.

## Rendering architecture
- **High-DPI**: `high_dpi: true` in `window_conf`. The code treats
  `screen_width()/screen_height()` as **physical pixels** and consistently
  divides thresholds by `dpi = screen_dpi_scale()` and multiplies pixel sizes
  by `dpi` (`view_scale`, `ui`, safe-area insets, star radius `(0.5*dpi).max(1)`,
  obstacle `bevel`; the minimap ship dot scales with `ui`). **Subtlety
  (measured 2026-07):** macroquad's `screen_width()` actually returns *logical*
  px (`context.screen_width / dpi_scale()`), not physical — but the two mental
  models produce identical output because `view_scale`/`ui` scale linearly with
  `sw`, so the `×dpi` factors cancel. What matters is that **everything drawn
  and every `mouse_position()` is in that one consistent space.** `dpi = 1` on
  standard displays and native.
  - **Safe-area insets are NOT ×dpi.** The `env(safe-area-inset-*)` values
    JS forwards are CSS px = the logical draw space, so `safe_top/left/bottom/
    right` are used as-is. (An earlier `×dpi` was masked by insets being ~0 in
    browser-chrome mode; it surfaced as the minimap shoved ~3× too low in
    fullscreen, where the notch inset is real.) The **left** inset is capped
    at 24. All four sides are reported (`set_safe_area(top,left,bottom,right)`);
    the **bottom** folds in the floating browser toolbar (the canvas is 100vh
    and draws under it), measured JS-side via `visualViewport` — that's what
    keeps the parked stick tappable above the URL bar.
  - **`touches()` gotcha**: unlike `mouse_position()` (which divides by
    `dpi_scale`), macroquad's `touches()` returns **raw physical px**. The
    in-canvas stick therefore divides each touch position by `dpi` before use,
    putting it in the same logical space as the drawing / `mouse_position()`.
    A steer *direction* is scale-invariant, so a missed conversion still
    steers correctly but draws the stick off-screen (that was the bug).
- **World-to-screen**: a per-frame closure `w2s` (defined inside the `loop {}`, shadows the removed module-level function) converts world coords to screen pixels using `view_scale`.
- **`view_scale`**: on small screens (`sw.min(sh)/dpi < 600` CSS px, i.e. mobile in either orientation) it is `sw.min(sh) / MOBILE_VIEW_M` (`MOBILE_VIEW_M = 19` world metres across the **smaller** screen dimension, capped at the desktop scale) — one scale for both orientations, so rotating never changes the zoom level. In landscape the smaller dimension is the height → the cave's typical full height (average ≈ 15.5 m) fits with margin; portrait keeps the same scale and simply shows more world vertically (~36 m on a tall phone). Earlier attempts for reference: a fixed factor (`SCALE * 0.38`) cropped landscape to 13 m; keying on `sh` alone fit both orientations but gave portrait a much more zoomed-in look than landscape. Desktop: `SCALE * dpi`. Controls zoom; HUD/minimap are unaffected.
- **Cave walls**: drawn as **low-poly faceted** triangle meshes — one `draw_mesh` per (layer, side) for the up-to-3 loaded cave layers (y-culled), plus one per loaded shaft wall. Each mesh is a continuous lattice of flat-shaded triangles. See "Faceted wall rendering" and "Vertical shafts" below. Per-facet base colors carry deterministic brightness jitter; radial lighting is added on top by the fragment shader.
- **Radial light shader** (`LIGHT_VERTEX` / `LIGHT_FRAGMENT` constants): a custom macroquad `Material` active only during the cave-wall and obstacle draws (`gl_use_material` / `gl_use_default_material`). Computes per-pixel radial falloff from the ship's screen position. Uniforms set each frame: `ship_pos` (vec2), `light_radius` (float), `glow` (float).
- **Shader math**: `ambient = 0.45`, quadratic falloff `t*t`, *subtle* warm orange tint `glow * falloff * 0.12` added to red (×1.0) and green (×0.4) — kept low so the cool slate rock stays blue with only a faint thruster flush. `light_radius = min(sw,sh) * 0.55 + glow * min(sw,sh) * 0.30`.
- Stars, particles, ship, HUD text, and minimap all use the default macroquad material — the radial shader does not affect them.
- **Stars**: stored as **normalized [0,1) coords** and multiplied by the *current* `sw`/`sh` each frame (then `rem_euclid`-wrapped for parallax), so the field fills the whole viewport in any orientation. (Storing absolute pixels captured the startup size and left a gap after rotating to a wider screen.)
- **UI scale `ui`** (`(sw.min(sh)/dpi / 980.0).min(1.0) * dpi`): scales fixed-size HUD/minimap. Keyed on the *smaller* dimension so a phone keeps the same HUD/minimap size across portrait/landscape — `sw` alone grew the minimap on rotation. Capped at 1.0 so desktop is unchanged.
- **Ship rendering**: the hull is the const `SHIP_TRIS` — 41 triangles **extracted from the original Flash ship** (see below) — drawn in local ship space (`+Y` = nose/forward, origin = hull centroid, full height ≈ 0.95 world units). Each facet's silver brightness is derived from its centroid height (nose lit → base shaded), **except the nose cone** (centroid `cy > TIP_Y = 0.30`) which is recoloured **red** (`tip_base` 210/50/45, same height shading applied). On top, `SHIP_DETAILS` (`[ax,ay,bx,by,cx,cy,r,g,b]`) layers the real sub-shapes — window dome, two leg-pods, central engine cup + light insert, and a small gold accent — each with its own extracted colour, plus an **added** blue accent (cockpit glass + two flank racing stripes; the original SWF lander is plain silver, verified by parsing every fill incl. mid-shape style changes and gradient stops). The **two leg-pods are recoloured red** (detected by their extracted dark-silver `0.518/0.537/0.588` → drawn as `0.784/0.188/0.169`), matching the red nose. A two-triangle orange/yellow thruster flame (scaled by `glow`, hidden when `glow ≤ 0.02`, drawn first so it sits behind the hull) completes it. All geometry goes through the `rot(lx, ly)` closure which applies `SHIP_SCALE` before calling `w2s` — so the raw SWF coordinates are unchanged, only rendered at 1.5× size.
- **Origin of the ship mesh**: the geometry is the real player ship from the original Flash game. The published SWF (`completeHS8replay.swf`, a `CWS` zlib-compressed SWF) was decompressed and its tags parsed; the ship is `DefineShape4` **character id 41** (`mcSpaceship`), a silver lander (`#999999`/`#CCCCCC`). Its vector contours were rasterised, the outer silhouette traced and RDP-simplified to a 43-pt polygon, then ear-clip triangulated to `SHIP_TRIS`. The interior detail contours (parsed with full fillStyle0/fillStyle1 tracking) were normalised into the same ship space and ear-clip triangulated to `SHIP_DETAILS`. (The source `.fla` is an OLE compound doc whose binary edge format is undocumented; the SWF shape format **is** documented, so extraction was done from the SWF.) Regeneration scripts live only in scratch (`/tmp`), not the repo.

## Rock colors (base, pre-lighting)
```rust
rock_dark = Color::from_rgba(28,  38,  58,  255)  // deep navy-slate
rock_mid  = Color::from_rgba(52,  68,  96,  255)  // mid slate-blue
rock_edge = Color::from_rgba(92,  116, 150, 255)  // lit cool edge
```
Cool slate-blue palette for the low-poly "crystal rock" look. The per-facet
brightness jitter (`facet_shade` / obstacle `facet`, ~±15%) plus the radial
shader supply all the visible variation — there is no longer a smooth bevel
gradient. (Previously a warm-brown set `80/64/50 · 118/95/72 · 150/120/88`.)

## Thrust / glow system
- `glow`: smoothed 0→1 float, exponentially approaches the throttle with factor 0.12 per frame.
- Thrust applies upward force along the ship's heading via Rapier `add_force`, scaled by the throttle (max force 8.0).
- The body carries `linear_damping(0.2)` — imperceptible at landing speeds, but it caps how much momentum piles up on long burns/free-falls.
- **Velocity vector** (opt-in, **off by default**): an arrow drawn from the ship along its momentum, length grows with speed, color = green ≤ 1 m/s (landable) / amber ≤ `CRASH_DV_SOFT` (damage-free touch) / red above (damaging); hidden under 0.25 m/s and while crashed. Toggled by the "Velocity vector" toggle in the menu's Settings screen → exported `set_show_velocity(i32)` → `SHOW_VEL` atomic; the choice persists per device in `localStorage` (`pegasus_show_vel`) and is re-applied once the WASM exports load. The Debug HUD telemetry line appends `v=…` in the same danger color (the arrow toggle and the Debug HUD are independent).
- `light_radius` and warm tint both scale with `glow`, producing the radial light effect on cave walls.

## macroquad 0.4.15 material API (verified from vendored source)
All symbols are in `macroquad::prelude::*` (already imported) — no extra imports needed:
```rust
let mat = load_material(
    ShaderSource::Glsl { vertex: VERT_SRC, fragment: FRAG_SRC },
    MaterialParams {
        uniforms: vec![
            UniformDesc::new("name", UniformType::Float1),  // or Float2, Float4, etc.
        ],
        ..Default::default()
    },
).unwrap();
// Each frame:
gl_use_material(&mat);
mat.set_uniform("name", value);
// ...draw calls...
gl_use_default_material();
```
- Vertex attributes: `position` (vec3), `texcoord` (vec2), `color0` (vec4, divide by 255 in shader), `normal` (vec4).
- Built-in uniforms injected by macroquad: `Model` (mat4), `Projection` (mat4) — do not redeclare.
- Use `#version 100` and `precision highp float` for WebGL2 compatibility.
- Pass screen-pixel position as a `varying highp vec2` from vertex to fragment; `frag_pos = position.xy` works because macroquad 2D positions are already in screen-pixel space.

## Faceted wall rendering

Cave walls are a **low-poly faceted** tessellation: each wall (ceiling = `side 0`,
floor = `side 1`) is built as **one continuous mesh of flat-shaded triangles** per
frame and drawn with a single `draw_mesh` (two calls total). Flat shading is
achieved by giving all 3 vertices of a triangle the **same** color (the GPU would
otherwise interpolate); triangles therefore use duplicated, non-shared vertices
with trivial sequential indices `(0..len)`.

### The lattice (`src/render.rs`)
- `SUBCOLS = 2` sub-columns per 3 m segment → 1.5 m facets; `COL_DX = SEG_LEN/SUBCOLS`.
- `col_x(col)` — world x of a **global** facet column. *Pure* function of the
  global column index, so adjacent segments compute their shared boundary vertex
  identically → **no seams/cracks**. The visible column range is
  `col_lo = want_left*SUBCOLS`, `col_hi = (want_right+1)*SUBCOLS`.
- `ROW_DEPTHS = [0.0, 1.0, 3.0, 6.5]` m into the rock; `N_ROWS = 4`.
- `lattice_point(&level, col, row, side)` → world `Vec2` (layer-0 space; the draw loop
  adds `layer * V_PERIOD` to y). **Row 0 is exactly on the wall edge with ZERO
  jitter** (collider-aligned — the hard rule below); deeper rows recede into the
  rock (ceiling = +y, floor = −y) with small deterministic jitter (`hash_u32` of
  col/row/side; ±0.25 m in x, depth-scaled in y).
- `facet_shade(base, col, row, side, salt)` → band base color (`row 0→rock_edge`,
  `1→rock_mid`, else `rock_dark`) × deterministic brightness in ~[0.82, 1.12].
  Shaft walls reuse it with `side` 2 (left) / 3 (right).

### Per-column emission (in the draw loop)
Runs once per loaded layer (`lay_lo..=lay_hi`, y-culled). For each visible
column (x-culled vs `margin`; **skipped entirely inside shaft openings** —
`seg_in_opening(col.div_euclid(SUBCOLS))`): for each of the `N_ROWS-1` cells,
take the 4 corner `lattice_point`s → `w2s` → **2 flat-shaded triangles**, each its
own shade (two `salt`s per cell). The cell diagonal is chosen by
`hash_u32(col ^ row*…) & 1` so the lattice doesn't read as a regular grid. After
the rows, a solid `rock_dark` quad (2 tris) — emitted with the **ceiling** side
only — closes the inter-layer rock from this layer's deepest ceiling row up to
the **next layer's** deepest floor row (world-bounded; the old screen-space
`far_up`/`far_down` fill is gone). Its cull band is padded ±15 m past the layer
lines because the wall curves reach ~13 m past them.

**Collider-alignment rule (unchanged):** the lit row-0 surface must coincide with
the Rapier segment collider. Only rows > 0 (inside the rock) may be jittered.
`w2s` inverts Y, so "into the rock" is screen-Y − for the ceiling and screen-Y +
for the floor; jitter always pushes *away* from the cave interior.

## Vertical shafts (y-wrap)

The world repeats every `V_PERIOD = 90 m` in y: identical copies of the cave
stack vertically, connected by **vertical shafts** that punch through ceiling +
floor at deterministic x positions. A shaft is a continuous vertical tunnel
crossing every layer, so climbing (or falling) one always brings you back to
"the same" cave — the vertical analogue of the `PERIOD` x-wrap. The ship's y
just grows; nothing teleports. The Debug HUD shows the current layer as
`lvl=N` (`ship_layer = round(cam_y / V_PERIOD)`).

### Placement (all pure `Level` methods, `src/world.rs`)
- `shaft_open_seg(s)` — opening start segment for slot `s`: every
  `SHAFT_SPACING_SEGS = 50` segments, anchored at `SHAFT_BASE_SEG = 35`, ±6 segs
  jitter hashed on `s mod 4` so the pattern repeats **exactly** each `PERIOD`
  (both wraps stay seamless). Openings land at x ≈ 123, 264, 387, 555 (mod 600)
  — verified clear of spawn x=0 and reset x=64.
- `seg_in_opening(idx)` — true for the 3 opening segments; there `insert_seg`
  emits **no** ceiling/floor colliders and the wall renderer skips the columns.
- `shaft_wall_x(s, side, t)` — wall x at normalized height t: two sine harmonics
  (±1.25 m, phases hashed on `s mod 4`) under an envelope that is **zero at both
  ends**, pinning the wall exactly to the opening edges. Min width ≥ 6.5 m.
- `shaft_wall_pts(s, gap, side)` — polyline (3 m steps) from layer `gap`'s
  ceiling curve to layer `gap+1`'s floor curve **at the opening-edge x**, so the
  wall's endpoints coincide with the clipped cave colliders' endpoints — the
  collider chain through a junction is gap-free by construction.
- `shaft_lattice(pts, s, i, d, side)` — facet lattice: depth col 0 = the
  polyline (collider-aligned), deeper cols recede horizontally into the rock.
  Near the ends deep cols are additionally pulled *along* the shaft (`end_pull`)
  so corner facets turn into the junction wedge instead of poking into the cave.

### Loading (`Sim::sync_window`, src/sim.rs)
- Cave segments: `BTreeMap<(layer, idx), Vec<ColliderHandle>>` — 2D sliding
  window, layers `ship_layer ± 1` × segments `want_left..=want_right`
  (`retain` + `entry().or_insert_with()`; empty Vec = opening). BTreeMap, not
  HashMap: ordered ops keep Rapier handle assignment deterministic (resim).
- Shafts: `BTreeMap<(slot, gap), Shaft>` for gaps `{ship_layer-1, ship_layer}` —
  covers everything reachable within half a period. `Shaft` stores collider
  handles + both wall polylines for rendering (pub, read by main's draw code).
- The sync runs inside `Sim::tick` (and `restore`), keyed off the TRUE body
  position, only when the ship's (segment, layer) changes — so spawn/reset
  seed the window immediately and resim performs the identical op sequence.

### Rendering
Same faceted treatment as the cave walls rotated 90° (rows along y, depth cols
into rock ±x), one mesh per wall, same light shader. A solid fill extends from
the deepest col to ~15 m past the opening edge, overlapping the inter-layer fill
(same `rock_dark`, invisible seam). Row y-cull is padded by 8 m (`end_pull`
reach). Obstacles are **skipped within 8 m of an opening** so junctions stay
flyable, and the minimap carves shafts with their true wall shape.

## Polygon obstacle system

Random convex-polygon boulders are placed deterministically along the cave so they load/unload with the same sliding window as the walls and are identical every time the player revisits a location.

### Generation
- `OBSTACLE_SPACING = 16.0 m` between slots. Each slot `k` maps to a fixed world-x position plus ±3 m jitter.
- A tiny integer-hash PRNG (`Rng` struct, seeded by slot index) drives all randomness: position jitter, size, rotation, vertex count, vertex radii.
- Slot is skipped if: within 9 m of the spawn (x = 0) or the reset point (`RESET_X` = 64), `hw < 4.5` (pinch point), within 8 m of a shaft opening (junctions stay flyable), or 1-in-6 random empty.
- Size: `max_r = (hw * 0.65).min(5.5)`, `r = rng.range(0.3, 1.0) * max_r`. Wide sections get genuine boulders (up to 5.5 m radius).
- Centre offset: `max_off = (hw - r - 1.3).max(0.0)` — guarantees ≥ 1.3 m gap to the nearer wall.

### Collider
Static Rapier `convex_hull` collider, translated and rotated to match. Hull vertices are read back from the collider for rendering so visuals exactly match the collision shape.

### Rendering
Drawn as a single `draw_mesh` per obstacle with the light shader active (same
material as the walls). Same topology as before — hull → inset ring + center fan —
but **flat-shaded** for a low-poly faceted-pebble look:

1. Compute `inset[]`: each hull vertex pulled `BEVEL = 16 px` toward the screen-space centroid. The outer `poly` ring stays the exact hull (= collider).
2. **Bevel ring** (hull → inset): 2 flat triangles per edge, base `rock_edge`/`rock_mid`.
3. **Inner fan** (inset → center): 1 flat triangle per edge, base `rock_mid`.

Each triangle is one solid color (3 identical-color verts → no GPU gradient
across a facet), emitted with sequential indices. The per-facet color =
`base × brightness × top-light gradient`, via the `facet` closure:
- **brightness**: `hash_u32(slot_key k, edge i)` → ~[0.85, 1.13]. Keyed on the
  obstacle's HashMap slot `k` (loop is `obstacles.iter()`), so facets are stable
  and do **not** flicker as the boulder rotates/moves.
- **top-light gradient**: facets whose screen centroid sits *above* the boulder
  center are brighter (`1 + clamp((center.y − tri_cy)/radius_px, −1, 1)·0.18`;
  screen-y grows downward), giving the lit-top "faceted ball" appearance.

### Minimap
The minimap is a ship-centred window that pans in **both axes** (`MM_HALF_X =
150 m`, `MM_HALF_Y = 50 m` — chosen so x and y share the same world-per-pixel
scale on the 480×160 map; ship dot and viewport rect always sit at the centre).
Cave interiors are carved per x-sample column for every layer in view; shafts
are carved with their **true wall shape** by evaluating `shaft_wall_x` /
junction curves directly (16 trapezoid steps) — the map is a genuinely
zoomed-out view of the real geometry, not a schematic. Obstacles are drawn as
their actual polygon shape (triangle fan + outline), filtered by the y window.

### Storage
`BTreeMap<(i64, i64), Obstacle>` in `Sim`, keyed by (slot index, layer) — every
layer gets an identical copy of each obstacle at `cy + layer * V_PERIOD` (the
y-wrap). Loaded/evicted by `Sim::sync_window` together with the wall window
(`k_left` / `k_right` derived from `want_left` / `want_right`, layers
`ship_layer ± 1`). Key-ordered iteration also gives boulders a stable z-order
in the renderer for free.

## Color / rendering alignment rule

**The visible rock surface must coincide with the Rapier collider line.** For
walls this means lattice **row 0 carries zero jitter** and is sampled directly
on the wall edge; only deeper rows (inside the rock) are displaced. For obstacles
the outer `poly` ring stays the exact hull. All facet displacement goes *into the
rock* (away from the cave interior), never into the cave — otherwise the visible
surface pokes past the collider and the ship appears to sink into the rock.

## Audio (web only)

Two sounds, both **synthesised in memory at startup** (`wav_from_samples` +
`thruster_wav`/`boom_wav`, driven by the deterministic `Rng`) — no asset files:
a 1 s low-passed noise loop for the engine (started muted+looped; volume set to
`glow * 0.6` each frame) and a 0.9 s darkening noise burst played on crash.
**Gated by the Sound setting** (`SOUND_ON` atomic, **off by default** →
`set_sound_enabled` export, `pegasus_sound` in localStorage): off holds the
thruster loop at volume 0 and skips every boom play. Sound stays OFF until
the player enables it in the menu's Settings screen.
The macroquad `audio` feature is **wasm-only** (`[target.'cfg(target_arch =
"wasm32")'.dependencies]` in Cargo.toml) because quad-snd needs ALSA to link on
native Linux; native builds get macroquad's dummy backend (same API, silent),
so `cargo build`/`cargo test` need no system packages. Browsers unmute the
AudioContext on the first user gesture (handled by the miniquad JS bundle).

## HUD

Cleaned up 2026-07. The top-left column, top to bottom: the **minimap**
(480×160), the **fuel & hull gauges** just below it (big fuel-drop / heart
icons; see "Fuel"), then the **primary readout** — the run **distance**
(`{max_dist} m`, or the **score** on pads levels), **big** (`100*ui`, the
best `0.36×` beneath, and under that the record attribution "by <pilot>"
at the BEST line's own size — `BEST_NAME`, see "Online high scores";
hidden when empty; pilot names render UPPERCASE everywhere they appear).
**During replay playback the record readouts — every BEST line + the
"by <pilot>" attribution — draw from a per-replay record context**
(`REPLAY_BEST`/`REPLAY_BEST_NAME`), never the live BEST/name globals:
those follow the LOADED level, and a watched replay can be a foreign
level (scores-mode board browsing never reloads the game), so reading
them under a replay showed another level's record (field bug 2026-08:
The Hollows' best under other levels' replays). The context is the
REPLAYED level's own record: `watchGlobalReplay` pushes the board's
cached all-time #1 (`set_watch_best` + `set_watch_best_name`, always —
blank on a cold cache, so a stale context never leaks) right before the
blob, and the two own-run crash-replay entry points copy the live
globals (`adopt_live_record_for_replay` — an own-run replay IS the
loaded level). Unknown (0/empty) ⇒ the BEST/"by" lines are hidden for
that replay. The rest of the readout is replay-correct by construction —
it reads `world_sim`, the replay's scratch sim. The readout is **left-aligned** to the column's left edge (`mm_ox`,
plus a `20*ui` margin) and **shrunk to the minimap width** so long numbers
stay inside the column. (It's drawn in the gauge block, after the bars are
laid out, so it can sit below them.) All HUD text/icon sizes are `× ui`, and
`ui` folds in `dpi`, so they scale correctly on high-DPI screens.

Only the **telemetry TEXT line** is behind the **Debug HUD** setting
(`DEBUG_HUD` atomic ← `set_debug_hud`, off by default): `dist=/best=/x=/lvl=/
cave-progress/FPS` + the numeric `v=` speed, at the `252*ui` baseline. The
velocity *vector arrow* is its own separate opt-in (`SHOW_VEL`), unaffected.
(Earlier this cleanup also hid the minimap behind Debug HUD, but the minimap
is gameplay, not telemetry — it stays always-on.)

## Fuel

`FUEL_MAX = 100`; the main engine burns `FUEL_BURN_MAIN = 3.5/s` at **full
throttle** (~28 s of continuous thrust; partial throttle burns proportionally),
RCS burns `FUEL_BURN_RCS = 1.2/s`. `thrusting_now` and the
RCS gates (`rcs_ok`) require `fuel > 0` — an empty tank kills engine, RCS,
particles and glow, and immediately shows "OUT OF FUEL". **Running dry
ends the run**: `FUEL_OUT_END_SECS = 2.5 s` after the tank empties —
moving or not, the final coast still earns distance for that window —
`TickReport::fuel_out` fires (a `Sim` timer — pure detection, no physics
feedback, not in `SimParams`) and main publishes the run and goes
STRAIGHT to the game-over dialog (no `CRASH_DIALOG_DELAY` handover — that
exists for the crash explosion; the banner has already been up for the
whole window). Because publish and state 2 land in the same frame, the JS
ui-state poll calls `collectEndedRun()` synchronously before opening a
screen, so the submit dialog can't miss the run (analytics cause 2 =
"fuel"). No wreck: the intact ship stays visible under the amber banner
and behind the dialog, and there's no boom. A pad catch inside the window
refuels (fuel > 0 resets the timer) and cancels the game over
(unit-tested: `out_of_fuel_at_rest_ends_the_run_but_a_pad_refuels`). The
`run_over` flag in main guards against double-publishing the run on the
follow-up reset. The banner REPLAYS too — in `Mode::Replay` it's drawn
from the scratch sim's fuel (up from the tick the tank empties until the
replayed crash, and through a fuel-out ending's freeze frame; not part of
the auto-hiding transport GUI). HUD: slim always-on gauge bar with a **fuel-drop icon** to its left,
warm-amber identity (gold > 50%, amber > 25%, red below), and the **hull
gauge** in a matching bar just beneath it (a **red heart icon** + red bar,
brighter red when critically low ≤ 25%) — top-left, directly under the
minimap and matching its width. Both bars carry a **percentage readout**
**between the icon and the bar** — sized to match the "BEST" label
(`small_fs`, both computed together up front so the gauges can reuse it),
bold via an 8-direction dark outline (green > 50% / amber > 25% / red below)
— independent of the bar's own fill color, which stays fuel-amber /
hull-red regardless. The bar itself starts after a fixed-width reservation
(sized to fit "100%") so both gauges' bars line up regardless of the actual
value. See "HUD".

## Landing pads & scoring

Flat metal pads on the cave floor at deterministic slots (`pad_spec(p)`, pure):
`PAD_SPACING = 130 m` ± 20 m jitter, deck `PAD_HALF_W = 3 m`, deck top =
max floor over the span + 0.1 (the segment collider, friction 0.9, never dips
into rock). A slot is skipped if `hw < 5`, within 8 m of a shaft opening, or a
boulder (checked via `obstacle_spec`) would overlap the deck — roughly every
other slot survives. Pads replicate per layer like obstacles
(`BTreeMap<(slot, layer), Pad>` in `Sim`, same sliding window).

**Landing** = settled on a deck (|angle| < 0.3, |v| < 1 m/s, |ω| < 0.5, feet —
0.73 below origin — within 0.3 of deck top) for `PAD_LAND_TIME = 0.8 s`. First
visit per (slot, layer) scores `PAD_POINTS = 100` (green "+100" flash); parked
ships refuel at `PAD_REFUEL_PER_S = 25/s` ("REFUELING" shown while below max).
`score` is in the HUD text line. Beacons blink green until visited, then
steady blue; the minimap draws a deck-width line (green → blue-grey).
`stand_y` prefers a pad deck over the floor, so the spawn parks on pad 0
(cx ≈ 0.4) — pad friction also stops the frictionless-floor drift at spawn.
Pads are drawn with the **default material** (readable in the dark), deck top
exactly on the collider line (alignment rule).

## Impacts, hull damage, crash & respawn

Impacts are detected inside `Sim::tick` from the **per-tick velocity
change**: `|v − prev_vel|` above a threshold means a collision impulse (an
impulse resolves within one tick; thrust/gravity move v by < 0.05 m/s per
tick) — no Rapier contact-event plumbing needed. The tick returns an
`Impact` report (with the pre-park pose/velocity — a destroying impact parks
the wreck inside the tick) that main turns into sparks/thud/shake or the
full crash flow. Damage is **graduated**, not binary:
- dv ≤ `CRASH_DV_SOFT (2.5 m/s)`: free.
- `CRASH_DV_SOFT`..`CRASH_DV_HARD (6 m/s)`: survivable scrape — hull damage
  proportional to dv (full `HULL_MAX = 100` bar exactly at HARD), a small
  spark burst (kind 3, short-lived), a quiet thud (boom sound at 0.25
  volume), and screen shake (`shake` 0..1, random ±0.12 m camera jitter
  decaying at 4/s, applied to `cam_x/cam_y` after interpolation).
- dv > `CRASH_DV_HARD`, **or a scrape that empties the hull**: destroyed.

Hull is repaired while parked on a pad (`HULL_REPAIR_PER_S = 20`, alongside
refueling — the banner reads REFUELING, or REPAIRING once fuel is full) and
restored by reset/respawn. HUD: a second slim gauge bar (red heart icon +
red bar, brighter red when critically low) directly under the fuel bar.

On destruction: 70 explosion particles (`kind 3`, ~1.1 s life), the wreck is
parked (`set_gravity_scale(0)`, velocities zeroed) so the camera holds still,
input is dead (`crashed` gates thrust/RCS and ship rendering), and a "CRASHED"
banner shows. After `CRASH_DIALOG_DELAY = 1.5 s` the **crash dialog** takes
over (see below; on web the HTML game-over screen sits on top of it);
respawn happens from its Fly-again action (or the R key,
which works from any mode) and returns to **`SPAWN_X` = 0, the original
spawn** — every run shares the ghost's start line. (`RESET_X` = 64 remains
in world.rs purely as an obstacle-clearance anchor; changing it would
reshape the cave.) `Sim::restore` snaps its internal
`prev_vel` on any teleport (otherwise the velocity jump reads as an impact);
main must still snap `prev_ship` (render interpolation) after `sim.reset`.
Spawn/reset place the ship **standing on the floor** (`stand_y(x)` = floor +
0.78, feet at 0.73): dropping it from `cave_center` reached ~5.5 m/s at
touchdown, which tripped the crash threshold and looped spawn → crash →
respawn forever.

## Crash dialog & instant replay

A `Mode` enum (`Flying` / `CrashDialog` / `Replay`) sits above the crash flag:
`Flying` covers normal play *and* the 1.5 s wreck/explosion phase; the other
two **pause physics** (the stepping loop is gated on `Flying` and drains
`phys_accum`, so no catch-up burst fires on resume; the wreck stays parked).

- **Recording**: the hybrid `Recording` (below) is the ONLY replay store —
  the dense per-step visual buffer is gone (deprecated 2026-07 once both the
  replay and the ghost became re-sim driven). Every physics step while alive
  (`crash_timer <= 0`) records the tick's input + periodic keyframes; the
  impact tick itself is captured. Reset adopts the ended recording as the
  ghost and starts a fresh one.
- **Armed-but-idle start (#76)**: after spawn/reset/level-load the run holds
  still — no `sim.tick`, no `record_tick`, `phys_accum` drained — until the
  first non-neutral `InputState` (`is_neutral()` in replay.rs: any throttle,
  rotation, or bare stick touch arms it). Keyframe 0 = the spawn state,
  tick 1 = the first commanded tick, so replays begin with action and the
  ghost's lockstep clock (`recorder.ticks()`) stays fair automatically. The
  `run_started` gate lives in the frame loop **outside** `Sim::tick` (input
  gathering is frame-level; the sim stays a pure function of the input
  stream — resim never sees the wait because it was never ticked or
  recorded).
- **Crash dialog**: in-canvas it's now only a dimmed backdrop + "CRASHED" +
  keyboard hints — on web the **HTML game-over screen** covers it and drives
  the choices through the menu bridge (`ui_command`: 1 = fly again → the
  same reset block as the R key, 2 = watch replay), while R / Enter remain
  as the native/dev fallback.
- **Playback is RE-SIM DRIVEN** (`ResimPlayer` in main.rs): WATCH REPLAY
  re-runs the hybrid recording's input events through a **scratch `Sim`**
  paced by the render clock — the machinery a replay shared from another
  device would use. The interpolated frame (fractional-tick lerp,
  `lerp_angle` for the seam-safe heading) **overrides
  `cam_x`/`cam_y`/`angle`/`ship_vx/vy` and `glow`** before the `lp`/`ld`
  closures are built, so flame/light/volume/exhaust/RCS puffs replay from
  the re-simmed state; glow is re-derived from the commanded throttle. The
  **world renders from the scratch sim** (`world_sim` binding — its collider
  windows follow the re-simmed ship; the main sim's stay parked at the
  wreck), including fuel/hull gauges, score and pad beacons, which re-earn
  themselves as the replay lands. Each 1 Hz keyframe is verified as the
  cursor passes: the overlay shows `re-simulated from inputs · drift N m`,
  and drift > `SNAP_DRIFT_M = 0.5` snaps to the keyframe (the graceful
  fallback for recordings from a different build/params — zero on the same
  binary, unit-tested). The **stick is drawn HALF SIZE, tucked into the bottom-right
  corner with tighter margins than the full-size park spot and ~168 CSS px
  up, clear of the three-row HTML replay bar** animated by the input driving the re-sim —
  knob at the recorded deflection, amber while held — so a replay shows the
  pilot's hand where the live stick sits. (A throttle meter for both live
  play and replay is a follow-up, see #67.) The destroying impact is
  re-simulated, ends the playback (boom +
  dialog). **Reaching the end does NOT exit**: playback PAUSES on the
  final frame, but the ending still ANIMATES — the destroying impact
  bursts its debris (+ boom sound) and the plume fades during a ~1.2 s
  `replay_boom_timer` grace in which particle time keeps running in
  COSMETIC time (× playback speed — a ¼× ending plays out in slow
  motion; the timer counts down in the same clock) while EMISSION stays
  off (`emit_cosmetics` — a frozen ship must not keep spraying exhaust),
  then the freeze is total on a clean frame; the ending re-fires if the
  finale is replayed after a scrub back. The hull VANISHES in the
  replayed explosion like live play (the scratch sim's `crashed` flag;
  scrubbing back rebuilds a fresh sim, so it reappears), stepping ⏭ past
  the final tick is a no-op (same-tick seeks return early — they used to
  re-seek the last interval and resurrect the plume),
  and **hitting play on the last frame restarts from the top** (the finish
  auto-pauses, so finished-and-unpaused can only mean the user pressed
  play — checked BEFORE the frame's transport commands, so a
  scrub-to-the-end while playing pauses on the finale instead of instantly
  looping). Leaving is always explicit: the **✕ corner button**
  (`ui_command(3)` — during a replay the HTML pause/restart corner buttons
  swap for it) or the R-key respawn. **Transport cosmetics**: every seek/step calls
  `rebuild_replay_particles` — re-sim the trailing `PLUME_WINDOW_TICKS`
  (60 = the 0.5 s exhaust lifetime) on a scratch player, emitting per-tick
  cosmetics — so the frame you land on (in EITHER direction) carries
  exactly the plume the ship should trail there. (Earlier iterations
  cleared on backward jumps and fed forward steps as one coarse dt lump,
  which read as a dotted clump.) Cost: a scratch seek + ≤60 ticks, a few
  ms, at most once per rendered frame during a drag. `was_finished` is
  captured BEFORE the transport commands so a seek landing exactly on the
  final tick counts as the finish transition (pause + boom) too. The only in-canvas replay UI is
  the recorded-input stick, HALF SIZE via `draw_stick`'s scale param — the
  old banner / hint line / progress bar / drift readout are gone (the
  drift check + snap still run, just undisplayed). **Auto-hide
  (YouTube-style)**: JS fades the GUI (bar + ✕) after 2.5 s of untouched
  playback and a canvas tap toggles it back (the game ignores touches in
  replay, so the canvas is free to be the show/hide surface); pausing
  (including the freeze-frame ending) SURFACES it on the transition, and
  while paused / picker-open the timer is suspended — but an explicit
  canvas tap still hides it even paused, like YouTube (forcing it visible
  continuously fought the tap toggle: show-hide flicker). A fresh replay ALWAYS starts with the GUI
  shown and the fade timer re-armed (the poll detects the transition into
  state 3 — resetting only on exit left a stale `rpHideAt` that faded the
  GUI on the first tick after > 2.5 s in the menu). Visibility is mirrored
  to the wasm (`set_replay_ui_visible` → `REPLAY_UI_VISIBLE`, reset to
  visible at the replay entry points): hidden ⇒ the stick returns to its
  FULL-SIZE parked home (`stick_park`), exactly where the live stick sits.
  WATCH REPLAY is a no-op if the recording has no ticks. **The same `Replay`
  mode also plays stored highscore replays** (see "High scores"): the ▶
  button sets `watch_rec` and playback sources from it instead of the live
  recorder, returning to `watch_return` (dialog or flight) on finish — a run
  that ended by reset (no wreck) skips the terminal boom.
  `ResimPlayer::step_one` is the shared per-tick core: `advance()` drives it
  on the wall clock for WATCH REPLAY; the ghost calls it in lockstep.
- **Transport controls (play/pause + frame-level scrubbing + speed)**:
  playback can be paused (`advance(rec, 0.0)` re-simulates nothing and
  returns the frozen interpolated frame), scrubbed/stepped to any exact
  TICK, and rate-scaled (¼×/½×/1×/2×/4× — the multiplier scales the
  wall-clock time fed to the re-sim, the tick sequence never changes, so
  determinism/drift checks are untouched; the fractional-tick interpolation
  keeps slow-mo smooth). Fast-forward note: the caller clamps the RAW frame
  time to 0.05 s before multiplying, and `advance()`'s hitch cap sits at
  0.25 s — above the 4× worst case (0.2 s = 24 ticks) — so 4× is genuinely
  4× on a 60 Hz display (unit-tested; the old 0.05 cap clipped it to ~3×).
  `ResimPlayer::seek_to_tick` reaches an arbitrary tick: physics can't run
  backwards, so a backward (or cross-keyframe forward) target goes through
  `seek_to_keyframe` — rebuild the scratch sim FRESH, restore the nearest
  keyframe at or before the target (the same op sequence as playing a
  trimmed recording from its first keyframe, input re-seeded from the event
  stream exactly like `Recording::trim` does at a cut) — then re-sims the
  remainder (< `KEYFRAME_EVERY` ticks, a few ms); a forward target inside
  the current keyframe interval just steps the live scratch sim (that IS
  ordinary playback). Tick-seeking lands on the exact state continuous
  playback reaches (unit test
  `tick_stepping_matches_continuous_playback_bit_exactly`). Transport
  steps are 0.1 s of sim (12 ticks — the bar's ⏮/⏭, hold-to-repeat at
  ~10/s ≈ a realtime shuttle; ←/→ in-canvas) and AUTO-PAUSE playback;
  the underlying seek stays tick-exact. **Cosmetic clock**: particle
  update dt runs in replay SIM time (× speed, 0 while paused, emission
  gated on dt > 0) — particle velocities are world-space, so wall-clock
  particles around a frozen/slowed ship inherit its world velocity and
  the exhaust plume streams AHEAD of it (the "thrust goes forward on
  pause" bug); the engine hum also mutes while paused. Since
  format v3 a keyframe carries the body's EXACT unit-complex rotation
  (restored via `Rotation::new_unchecked` — `Rotation::new(angle)` cost
  sub-mm round-trip drift) plus `land_timer`, so restoring an airborne
  keyframe resumes BIT-EXACTLY (unit test
  `seeking_to_a_keyframe_resumes_bit_exactly`). A keyframe captured under
  sustained contact can still diverge (Rapier warm-start caches / handle
  numbering aren't in the keyframe) — absorbed by the per-keyframe drift
  check + 0.5 m snap fallback. A seek past the end
  clamps (keyframe restores skip the terminal crash keyframe — not a
  playable start), so scrubbing to the bar's far right shows the finale. Controls: the HTML `#replay-bar` (see the bridge
  section below) or, in-canvas (native/dev fallback), space = pause,
  ←/→ = ±1 tick and S = cycle speed; the banner appends the speed
  (`REPLAY · 2x`) and `· PAUSED`. Pause, pending seeks and the speed are
  reset whenever a new replay starts (all three `Mode::Replay` entry
  points).

### Hybrid recording (`src/replay.rs`) — the single replay format
A `Recording` captures each spawn→crash run as **inputs + params +
keyframes** — both the in-game replay source AND the transport format for
replays that leave the device (sharing/ghosts/leaderboards), in memory only
for now:
- **Input change-events**: the *resolved* controls per physics step
  (`InputState`: throttle u8, rate command i8, stick vector 2×i8, stick-held),
  pushed only on change — the frame's quantized `input` is recorded by
  `record_tick` in the stepping loop.
- **Keyframes** every `KEYFRAME_EVERY = 120` ticks (1 Hz): full sim state
  (position, EXACT unit-complex rotation re/im — not an angle, see the
  `Keyframe` doc comment — velocities, fuel, hull, glow, land_timer; 48 B)
  for drift detection / transport seeking / fallback playback, plus a
  terminal keyframe at the impact (`finalize`).
- **Header**: `SimParams` — every physics constant, built by `sim_params()`
  from the (now module-level) consts `GRAVITY_Y`, `THRUST_FORCE`,
  `LINEAR_DAMPING`, `ANGULAR_DAMPING`, `RCS_FORCE`, PD gains, fuel/crash/hull
  numbers — plus `LevelParams` (the world half: scoring/shafts/obstacles/
  pad_spacing/seed) so resim rebuilds the exact world — and a
  **build id**: index.html parses the first 8 hex chars of
  the deploy revision to a u32 and pushes it via the `set_build_id` export
  (0 = local dev). Bump `REPLAY_FORMAT_VERSION` when the layout changes
  (v3: exact rotation + land_timer in keyframes; v4 = v3 + the flags
  byte/Terrain block in LevelParams and visited+run_ticks per keyframe,
  chosen PER RECORDING — only endless / hand-drawn / time-scored levels
  write v4, so v3 blobs stay valid and keep decoding; **v5** = v4 +
  `time_limit_ticks` u32 + `goal_distance` f32 right after the flags
  byte, written only by time-LIMITED or GOAL levels — The Flux Sprint /
  The Flux Dash — same per-recording rule, so v3/v4 blobs stay
  byte-identical). **No backward
  compatibility while iterating** — `deserialize` rejects pre-v3
  versions, so older server blobs stop decoding (watch/ghost pushes
  no-op gracefully); add version-tolerant reads when the game is
  released.
- **Cosmetic trailer** (2026-08 — the format's FORWARD-compatibility
  channel): optional bytes after the last keyframe, any version — magic
  `PGXT` + TLV entries (tag u8, len u16, payload). Every parser ever
  shipped reads exactly the counted sections and ignores trailing bytes
  (verified back through history — no version ever checked
  end-of-buffer), so old clients AND the pinned backend verifier decode a
  trailered blob unchanged and just present the replay without the extra
  context — no version bump, no repin. Three hard rules: entries carry
  **presentation data only** (anything the sim consumed would silently
  desync old clients' resim); readers skip unknown tags by length /
  treat malformed tails as "no trailer" (the trailer itself extends the
  same way); and values shown to other players must be **enum-whitelisted,
  never free text** — trailer content is attacker-controlled (it rides
  stored replays to other players' screens). The backend verifier
  additionally rejects malformed or oversized tails at submit
  (`deserialize_with_tail` → `TailInfo`, cap `TRAILER_MAX_BYTES` = 256 B)
  so the tail can't smuggle bulk content into public replay storage —
  game-side decoding stays lenient. Written only when an entry is
  non-default, so
  default-scheme recordings stay byte-identical to the pre-trailer
  format. Tag 1 = control scheme (`Recording.scheme`, `SCHEME_STICK` /
  `SCHEME_SPLIT` — kept current per recorded tick by the frame loop, last
  write wins on a mid-run toggle): replays render the input widgets of
  the scheme the run was flown with (see "Input sources" → Split
  controls).
  effective input re-seeded there, so it stays replayable after the cap.
  The window is `HYBRID_MAX_SECS = 60 min` (~1 MB/h worst case) — a memory
  safety net, not an expected limit: a ghost needs the run from its spawn,
  so the recording must not lose t = 0 on normal-length runs. (A trimmed
  recording's ghost appears once the live run reaches its first keyframe.)
- On destruction the blob is serialized + deflated (`compress` =
  `miniz_oxide`, the repo's only new dependency) and published via
  `report_run_end` (the old on-dialog size receipt is gone with the
  in-canvas buttons).
- `sim::resim(&Recording)` re-runs the events through a fresh `Sim` and
  reproduces the recorded keyframes **bit-exactly** (unit test
  `resim_reproduces_a_scripted_flight_bit_exactly`; `glow` is render-side
  and excluded). Guarantee is per-binary — a build/params change is what the
  header fields + keyframe fallback are for. `resim` (the batch form of
  `ResimPlayer`) and `Recording::deserialize` are the backend verifier's
  entry points: pegasus-backend re-runs every submitted replay through this
  crate (segment-wise between the 1 Hz keyframes, with drift tolerances for
  cross-platform libm differences — see its CLAUDE.md) before a score reaches
  a board.

### Determinism rules (the re-sim refactor, 2026-07)
Live play and resim must perform IDENTICAL operation sequences:
- **Fresh `Sim` per run — NEVER reuse a sim across recorded runs.** Rapier's
  contact solve is sensitive to collider HANDLE NUMBERING; a reused sim's
  handle space carries the previous run's window churn, while resim always
  runs fresh. Under sustained multi-point contact (parked on a pad) the
  differing float summation order creeps ~1e-4 m, and chaos amplifies it to
  metres at later collisions (found 2026-07 from a real replay: 13.4 m
  drift, same binary, same page load). The reset block does `sim =
  Sim::new(sim.level.clone())`; `Sim::reset()` survives for tests only. Regression test:
  `fresh_sim_per_run_with_pad_contact_resims_exactly` (its inverse — reset
  instead of recreate — reproducibly diverged).
- All forces/fuel/damage/landing run per tick inside `Sim::tick` from the
  quantized `InputState` (never from raw device floats — quantize first via
  `InputState::from_controls`, then both the sim and recorder consume it).
- Collider windows: BTreeMap (ordered ops → same Rapier handle assignment),
  keyed off the TRUE body position (not the interpolated/shaken camera),
  synced inside `tick()` only when the ship's (segment, layer) key changes.
- Impact detection is per-tick dv (thresholds unchanged — an impulse lands
  within one tick; gravity/thrust move v < 0.05 m/s per tick).
- `Date`-like nondeterminism (macroquad `gen_range`) is allowed ONLY in
  cosmetics (particles, shake); nothing in `sim.rs` may use it.

### Ghost of the BEST run (re-sim driven)
`ghost_rec` is the current level's **global-record** recording, pushed from
JS (`load_ghost_blob` — the all-time board's best replay, fetched from
CloudFront by `applyGlobalRecord`; see "Online high scores"; no ghost
offline): the game keeps the racing ghost
recording, and a `ResimPlayer` re-simulates it in **LOCKSTEP** with live
play — `while p.tick < recorder.ticks()` per live `sim.tick` (a `while`,
not an `if`, so a ghost adopted a moment after the spawn catches up in one
bounded burst), so both ships fly the same spawn clock and stay in sync
through pauses (dialog/replay freeze live ticks → the ghost freezes too).
The ghost renders as a translucent hull silhouette (`SHIP_TRIS`, no
flame/details) at `lerped_pose(alpha)` with the live interpolation alpha,
**with the ghost pilot's callsign floated just under it** (`GHOST_NAME` ←
`set_ghost_name`, pushed with the blob by `applyGlobalRecord`; distinct
from `BEST_NAME` — the ghost is the best run WITH a replay, which may not
be the #1 score, and it keeps its pilot when BEST flips to "by you"),
plus a pale-blue minimap dot; it vanishes at its own crash tick
(`finished`). Hidden while the current ship is a wreck, and during the
**armed-but-idle wait** (`run_started` gates `ghost_pose` — silhouette,
callsign and minimap dot all appear at the first control command; both
ships would otherwise sit overlapped on the spawn). Toggled by the
**"Race best ghost"** Settings toggle (`set_ghost_enabled` → `GHOST_ON`,
on by default): off drops `ghost_player` and skips `ghost_pose`. A
mismatched-level recording is ignored (the ghost must race THIS world).
Cost: one extra `Sim::tick` per physics step during flight (~2× physics,
still tiny next to rendering; the ghost sim maintains its own collider
windows around the ghost's position).

**Replays race the ghost too (2026-08)**: replay playback keeps its own
lockstep player (`replay_ghost` — a `(ResimPlayer, Recording)` pair, built
at every `Mode::Replay` entry point via `replay_ghost_player`) riding the
replay clock — one ghost tick per replayed tick, lerped with the replay
player's accumulator; transport seeks/steps/restart re-seek it through the
ghost's OWN keyframes (`replay_jumped` — a long forward jump caught up
tick-by-tick would stall the frame). Silhouette, callsign and minimap dot
all render through the same `ghost_pose`, and it stays up through the
replayed crash's freeze-frame (where the record run was when this one
ended is part of the comparison). **The ghost recording is per-replay, not
the live `ghost_rec`**: a board's ▶ watch races **the watched board's own
record** — `watchGlobalReplay` fetches the board's best entry with a
replay alongside the watch blob and pushes it via `watch_ghost_blob` +
`set_watch_ghost_name` (the callsign rides `replay_ghost_name`, never the
loaded level's `GHOST_NAME`), which is what makes the ghost work when
browsing a FOREIGN level's board (field bug 2026-08: the Hollows-loaded
session saw ghosts only on the Hollows board — the loaded level's ghost is
the wrong world for every other board). `watch_ghost_blob` has **overwrite
semantics** (a failed decode or len 0 — JS's explicit clear when the fetch
is skipped/fails — replaces the pending value with None) so a ghost from
an aborted watch can never attach to a later one; with no watch ghost
pending, the entry falls back to the loaded level's `ghost_rec` (covers
the crash-dialog replay and a cold record cache on the own level's board;
`Recording` derives Clone for this). Skipped — `replay_ghost` stays
None — when the "Race best ghost" setting is off (same `GHOST_ON` gate as
live), when the candidate ghost was flown on another world (a random-seed
level's other rolls; params equality), or when the replay **IS the record
run itself** (it would sit exactly on top of the replayed ship): detected
by `same_recording`, which compares **serialized bytes**, not structs —
the format drops fields the level's version doesn't carry (v3 keyframes
lose visited/run_ticks), so a finalized live recording and its decoded
round-tripped blob differ in memory while their blobs match
(unit-tested: `replay_ghost_races_other_runs_but_never_the_record_run_itself`).
**Watching the record itself still gets a race (2026-08)**: on a board ▶
of the record run, JS ships the RUNNER-UP as the ghost instead — the
watch-ghost pick in `watchGlobalReplay` is the first board entry whose
`replayPath` differs from the watched one (a same-path guard, not a rank
test: paths are unique per submission, so every ordinary watch still gets
the record and only the record's own watch falls through to the
next-ranked replay). A board with no second replay clears the pending
ghost as before, and the `same_recording` gate above remains the
wasm-side safety net (own-run record replays, duplicate blobs).

### High scores & watching stored replays
Scores, replays and the racing ghost are **global-only** — the local
localStorage layer (`pegasus_scores_<file>` top-5, `pegasus_best_<file>`
mirror) was removed 2026-07 (stale keys are cleared at boot; offline
builds get no boards/ghost and a session-only BEST). The wasm↔JS contract:
- **Publish**: every ended run worth keeping (destroying crash, OR a manual
  reset while still alive — "longest flights", not "longest crashes") calls
  `report_run_end` → serialize + deflate into `RUN_BLOB`, `RUN_DIST` =
  `max_dist`, and bumps `RUN_SEQ`. JS polls `run_seq()` every 500 ms
  (and once more right before a level switch, so the run banks under the
  level it was flown on) and hands the blob to the online submit flow
  (`collectEndedRun` → `maybeSubmitOnline` — its only consumer now).
  EVERY armed run is published (2026-08 — the bug-report replay buffer
  wants short and non-scoring attempts too; a time level's crash /
  fuel-out DNF and abandoned reset used to skip publishing entirely,
  which left Hollows/Dash sessions with empty report zips), with
  `run_len_ticks()` and `run_scorable()` alongside — scorable = false
  for a time level's DNF/abandon; JS applies BOTH submit gates in
  `collectEndedRun` (< 240 ticks = the old `GHOST_MIN_SECS`, and
  non-scorable), so the submit-dialog behavior is unchanged.
- **Watch**: a board's ▶ button pushes the fetched blob into the wasm
  buffer (`blob_in_ptr` + `watch_replay_blob`), which `decompress` +
  `deserialize` into `PENDING_WATCH`. The main loop plays it through the
  SAME `Replay` mode / `ResimPlayer` as the crash-dialog replay, but
  sourced from `watch_rec` instead of the live recorder — and since a
  stored blob carries its own `LevelParams` header, the scratch sim
  rebuilds the world it was flown in. Watching **pauses** the interrupted
  run (physics is frozen in non-`Flying` modes) and returns to
  `watch_return` (dialog if a wreck is waiting, else flight) on
  finish/skip, so a mid-flight "watch" resumes untouched. A run that ended
  by reset (not a crash) skips the boom at playback end.
- **Ghost**: the global record's replay is pushed back via
  `load_ghost_blob` (see above and "Online high scores").
- `decode_recording` (decompress → deserialize) rejects corrupt blobs
  (returns None, never panics) — a corrupt download can't crash
  the game. Unit-tested end-to-end
  (`stored_highscore_blob_round_trips_into_a_watchable_replay`): fly →
  serialize + deflate → decode → re-play to the end, drift 0.

### Online high scores (dannyrhubarb/pegasus-backend)
The deployed site can carry a **`config.json`** next to `index.html`:
`{"apiBaseUrl": …, "replayBaseUrl": …}` — written by `build-site` from the
**`BACKEND_CONFIG_JSON` repo variable** (the backend stack's
`FrontendConfigJson` output, pasted once into Settings → Actions →
Variables; validated at build time). No variable → no config.json → the
whole online layer is invisible (local dev, forks — no boards, no ghost,
session-only BEST). Previews read **`BACKEND_CONFIG_JSON_STAGING`**
instead (falling back to the prod variable until it's configured —
pegasus#171 account split), so a PR preview exercises the STAGING backend
and can never touch prod boards. All JS-side in `index.html`:
- **Submit is always an explicit choice**: `maybeSubmitOnline` hooks
  `collectEndedRun` — every publishable run that ended in a game over
  (destroying crash, out-of-fuel, or a completed time level; score ≥ 1 —
  metres, or seconds on a time level; `collectEndedRun`
  already drops runs < `GHOST_MIN_SECS` from the submit flow — the wasm
  publishes them for the bug-report replay buffer) is held in `pendingSubmit`, and
  the accept gate is ui-state 1/2 OR analytics cause 3 — a completion
  publishes DURING the 1.6 s "LEVEL COMPLETE" grace at state 0 (flying),
  so the state gate alone dropped it (the 500 ms collect poll consumes
  `run_seq` before state 2 shows; bug found live on The Hollows,
  2026-07-17). Reset-ended runs (cause 1) stay excluded, and
  the ui-state poll shows the SUBMIT SCORE dialog (`#scr-name`) **instead
  of** the game-over screen: submit → save name + POST
  `{level: <file stem>, name, score, replay}` to `/v1/scores` → game-over;
  skip → the run is discarded (scores are global-only — an unsubmitted
  run isn't kept anywhere). The callsign input arrives prefilled once a
  name is persisted (`pegasus_player_name`). Reset-ended runs (restart /
  exit-to-menu mid-flight) never pop UI and are therefore **not
  submitted**. **The replay is mandatory** (2026-07): the backend
  re-simulates it to verify the claimed score before the run reaches any
  board, so a run with no blob or one over the server's 512 KiB decoded cap
  (multi-hour runs only) gets no submit dialog at all (was: submitted
  score-only). A run that fails server-side verification is **silently
  discarded**: the response is shaped like success so a forger gets no
  oracle to iterate against — the run just never appears on any board.
  Nothing surfaces client-side; an honest client shouldn't produce one.
  **Verification is asynchronous backend-side (2026-08)**: the 201 comes
  back immediately and the score reaches the boards seconds later, once
  the backend's resim worker finishes (long record runs used to blow the
  old synchronous 30 s lambda budget — the timeout's 503 was swallowed by
  the old fire-and-forget fetch, which is how an honest 8 928 m Glide run
  "saved" and then never appeared; the field bug of 2026-08-26). So
  `submitScore` now RETRIES transient failures (network error / 5xx) with
  backoff — a record run is unrepeatable, and the rare double-accept from
  a lost 201 response just writes the same score twice, cosmetic — and
  after a 201 it re-runs `applyGlobalRecord` once immediately and once
  ~20 s later, so a fresh record becomes BEST + the racing ghost after
  the async verdict lands. **Own-row "verifying…"** (`pendingVerify` /
  `mergePendingRow`): the 201's runId is remembered and the board screen
  merges the submitter's own entry in at its rank — dimmed, amber
  "verifying…" instead of the date, no ▶ (the blob isn't served until
  verified) — refetching every 6 s (`schedulePendingPoll`) until the
  real row lands (matched by runId) or a 3-min window expires. STRICTLY
  LOCAL AND STRICTLY VISUAL, by design: merged into the rendered rows
  only — never `boardCache`, never the picker bests, never BEST/ghost —
  so only the submitter sees it, the shared board stays verified-only,
  and a rejected run just quietly never materialises, preserving the
  backend's silent-discard no-rejection-oracle property (don't "improve"
  this into an optimistic server-side write — that would hand forgers
  the board as a verifier oracle and show fake scores to everyone). The
  same
  screen doubles as the Settings "Pilot name" editor
  (`openNameDialog(returnTo)`). Its input `stopPropagation()`s keydown so
  typing never reaches the game.
- **Global boards**: the High-scores screen IS the global board — with no
  config.json it just shows "Global scores need a connection". Today /
  This week /
  All time period chips fetch `/v1/levels/<stem>/scores?period=…` (top 50,
  ranked server-side); each row is TWO LINES — rank / [pilot name over a
  date (`fmtDate`, the date-only twin of `fmtDateTime`, `.pcol` column;
  the time of day was dropped 2026-08 — date is enough)] / distance —
  plus a ▶ watch
  button when the entry has a replay. **Timestamp locale lesson
  (2026-07)**: language tags lie about region — a phone set to English in
  Sweden reports a bare `en-US` (verified in the field via a temporary
  Settings debug readout), so `toLocaleString(undefined, …)` rendered US
  dates. The device TIMEZONE is the one regional setting the browser does
  expose: `fmtDateTime` derives the region by reverse-mapping
  `resolvedOptions().timeZone` through `Intl.Locale getTimeZones` (a
  676-region scan, ~200 ms — deferred off boot, memoized per zone in
  `pegasus_date_locale`) and formats with the REGION'S OWN conventions —
  its dominant language via `maximize()` (`und-SE` → `sv-Latn-SE` →
  `2026-07-11 16:00`), not the UI language glued onto the region (`en-SE`
  inserts a comma that isn't the standard Swedish format); `-u-nu-latn`
  pins Latin digits. Regionless zones / old engines / a not-yet-finished
  scan fall back to a fixed ISO local format. The `#scores-list` scrolls internally (`max-height:54vh`) so
  the title/chips/Back stay fixed under a long board. **Results are cached**
  (`boardCache`, keyed `level|period`, persisted to localStorage): the
  cached board paints instantly on entry while a fresh fetch runs in the
  background; a neon ring spinner (in `#scores-wait`, a FIXED-HEIGHT slot so
  the list never shifts under a tap — shared with the **bucket-reset
  countdown** `updateResetHint` shows on Today / This week: "Today's
  board resets in 1h 8m", UTC bucket ends, re-ticked every 30 s) shows
  during any
  board/replay fetch,
  and the error line only shows when there's nothing cached to fall back
  on. **Refetched on every entry**
  (`showScreen("scr-scores")` → `renderScores`; `renderGlobalScores` no-ops
  while the screen isn't up, so banking a run doesn't spam the API). Stale
  fetches are dropped via a sequence counter. Replay blobs are **not**
  fetched here — only on a ▶ tap (`watchGlobalReplay`).
- **Watching a server replay**: ▶ fetches `<replayBaseUrl>/<replayPath>`
  (CloudFront) and pushes the bytes into `watch_replay_blob`
  (`pushBytesToWasm`); the blob's header carries its level, so it re-sims
  in the right world regardless of the selected level. The tapped entry's
  pilot rides along (`set_watch_pilot_name`, always pushed — empty clears)
  and floats under the replayed ship in AMBER (the replay accent; the
  racing ghost's label stays blue) at the same offset as the ghost's
  label — while the ships overlap on the spawn the amber label just draws
  on top; own-run crash replays clear it — you know who flew.
- **Global record → BEST + ghost** (`applyGlobalRecord`): after every
  level load (and when config.json arrives, and after a successful
  submit) the loaded level's **all-time board** is refreshed; the #1
  score raises the in-game `BEST_DIST` (the HUD BEST = the world record)
  — on a **time level** the board is fastest-first and LOWER wins, so the
  #1 instead lowers `BEST_TIME` via `set_best_time` (the time twin of
  `set_best_dist`; `raise_best_time` mirrors the "by you" flip when a
  completed run beats it) —
  and the best entry **with a replay** is fetched from CloudFront and
  pushed via `load_ghost_blob` — the racing ghost is the global record
  run, its pilot's callsign following via `set_ghost_name` (drawn under
  the in-game silhouette). The record holder's name rides along via
  `set_best_name` (the
  `blob_in_ptr` buffer, UTF-8 text) and the HUD draws "by <pilot>" under
  the BEST line — flipped to "by you" by the game when a record-beating
  run ENDS (not mid-flight — the previous record + holder stay up as the
  target while flying); skipped when the session's own best already
  exceeds the board. `ghostLoadedPath` dedupes the blob fetch; stale responses
  for a since-switched level are dropped (and the game would reject a
  wrong-level ghost anyway). Offline / empty board: silent no-op — no
  ghost, session-only BEST.
- E2E-tested headless (Playwright + a stub API server, scratch-only):
  crash → dialog → POST body → auto-post on second crash → global board →
  server replay playback. The submit threshold and dialog gating live in
  `maybeSubmitOnline`; keep them in sync with the backend's validation
  (`score > 0`, name ≤ 24 chars — the input carries `maxlength=24`).

## Multiplayer (P2P shadow race)

2-player race (2026-08, per `docs/multiplayer-p2p.md` — the design brief
with the owner's locked decisions; read it before touching this): both
players fly the SAME level (same world, same **concrete** seed) side by
side, each in their own physics world — **no ship–ship collision**. The
opponent renders like the racing ghost (translucent silhouette, magenta
vs the ghost's pale blue — both can be on screen at once — with callsign +
minimap dot), driven live by their input stream over a **WebRTC
RTCDataChannel** (true P2P; STUN → Cloudflare TURN fallback). Latency only
moves where you SEE the opponent, never your own physics. Scoring is
unchanged: each run still goes through the normal publish → submit-dialog →
backend-verification flow.

- **Feature gate**: `config.json`'s `wsUrl` (the pegasus-backend signaling
  WebSocket — see that repo's CLAUDE.md; `build-site` validates it as
  optional `wss://`). Absent ⇒ the home-screen Multiplayer button never
  shows. NOTE: after the backend deploy the `BACKEND_CONFIG_JSON` repo
  variable must be re-pasted from the new `FrontendConfigJson` output;
  the app shells fetch config.json from the live deployment and follow
  automatically.
- **wasm bridge (`src/main.rs`)**: the opponent is "a ghost whose
  recording is still being written": `RemoteFeed` accumulates received
  input change-events + 1 Hz keyframes into a growing `Recording` (via
  `record_tick`, so event dedup matches the sender exactly) and drives a
  `ResimPlayer` through it — cross-device drift is absorbed by the same
  keyframe check + `SNAP_DRIFT_M` snap as watched replays. **Built
  entirely frontend-side from sim-core's existing public API — NO sim-core
  changes, no `REPLAY_FORMAT_VERSION` bump, no backend repin needed.**
  Exports: `set_mp_active`, `mp_arm`, `set_mp_name`, `set_mp_remote_over`,
  `mp_push_remote` (BLOB_IN batches), `mp_out_take`/`mp_out_ptr` (drain
  the outgoing mirror), `mp_remote_dist`. Wire batches are
  `total_ticks(u32) + events(9 B) + keyframes(60 B)` — tick-stamped and
  self-delimiting; keyframe 0 never ships (both sides derive the spawn
  state from the shared level+seed). The remote resim is paced by the
  render clock (smooth 120 Hz motion ~1 network batch behind); big
  backlogs (tab-hide) close via a keyframe-restore seek. Unit tests:
  `remote_feed_reproduces_an_incrementally_streamed_run_bit_exactly`,
  `remote_feed_snaps_onto_a_diverged_stream`, the batch-codec round-trip.
- **`mp_arm` replaces the armed-but-idle gate for races**: the countdown's
  zero arms the run with the ship still idle, so both recorders' tick
  clocks share the start line (recording semantics otherwise identical —
  keyframe 0 = spawn, tick 1 = first tick after the gun; a race recording
  may simply lead with neutral ticks, which the verifier is fine with).
  JS resets the run (`ui_command 1`) and holds `set_ui_pause(1)` through
  the 3-2-1, so the sim is guaranteed fresh and frozen when the gun fires.
  The countdown gates only the FIRST launch after a level pick — respawns
  inside a persistent room (below) use the ordinary armed-idle gate. A
  level switch still drops the remote feed (new world).
- **Persistent rooms (2026-08)**: a run's end does NOT end the session —
  after a crash / fuel-out / completion (and the optional submit dialog)
  the player respawns straight back into the shared world, and an R / ⟳
  restart is just a quick respawn; the room lives until someone actively
  leaves (the pause screen's Exit, or a disconnect). Mechanism: the local
  reset block keeps the opponent feed, restarts the outgoing mirror at the
  fresh recording's origin and queues an 8-byte **respawn marker**
  (`mp_push_respawn_marker`, a pseudo-batch with `total = u32::MAX`)
  IN-BAND on `MP_OUT` — ordering against the batches is correct by
  construction, no JS involvement. The receiver's `mp_ingest_stream`
  replaces its `RemoteFeed` exactly at the marker (spawn keyframe = the
  local recorder's kf 0, a pure function of the shared level+seed) and
  clears the remote-over flag; `ingest_batch` independently caps a
  hostile batch's tick total so it can't spin the receiver's catch-up
  loop. On `seed = random` levels the pinned concrete seed means respawns
  keep the SAME world (`with_rolled_seed` no-ops on the pinned text) —
  the room IS its world until the host picks anew. The opponent
  silhouette stays visible through the local wreck/dialog/armed-idle wait
  (`mp_pose` gates only on `Flying | CrashDialog`); their own crash still
  hides them until their marker arrives. Unit test:
  `persistent_room_respawn_marker_resets_the_feed_bit_exactly`.
- **JS (`pegMP` in index.html, after the analytics module)**: same
  never-break-the-game rules as analytics (every entry point try/caught) —
  but deliberately NOT analytics' `navigator.webdriver` gate, so e2e
  automation can drive it. Signaling client (`create_room`/`join_room`/
  opaque `signal` relay/`leave`), RTCPeerConnection with host-as-offerer,
  candidate queueing until the SDP lands, host-driven ICE restart on
  `failed` while the socket is up. DataChannel: JSON control messages
  (`hello` incl. build id — mismatch shows a "may drift" banner, the
  keyframe snap covers it — `level`, `ready`, `start`, `run_end`) +
  binary input batches relayed verbatim to/from the wasm. Menu:
  `scr-mp` / `scr-mp-host` (room code) / `scr-mp-join` / `scr-mp-lobby`,
  all with `.mbtn.back` (hardware back for free) + `histPath` entries;
  the level picker gains an `"mp"` mode (host pick →
  `pegMP.hostPickedLevel`). There is NO results screen (persistent rooms
  replaced it — run ends are transient banners). The countdown overlay +
  the magenta `#mp-banner` notice live outside `#menu` (over the frozen
  game).
- **Seed pinning**: the host rewrites `seed = random` to a concrete roll
  before transmitting AND loads that same text itself (`pushLevel(file,
  textOverride)`), so both fly identical rock and the backend's
  any-seed-passes rule for random-seed stems verifies both runs. Exiting
  the race re-pushes the level's original text so solo play gets its
  re-rolling world back. Guests load the transmitted text even for files
  missing from their manifest (the text travels wholesale); custom drafts
  MAY be raced and keep all their local-only guards.
- **Invite links**: the host screen's Share button sends the game URL with
  `?join=<CODE>` (native share sheet via `navigator.share`, clipboard-copy
  fallback with a "Link copied!" flash). A guest landing with the param
  auto-runs the join flow once config.json's `wsUrl` arrives — prefilled
  join screen, so an expired link surfaces the normal room_not_found error
  there. The param is consumed + stripped from the URL immediately
  (`history.replaceState`, preserving the back-stack state object) so a
  reload never rejoins a dead room. Plain https on purpose: works in every
  browser and both app shells' guests-in-browser; Universal/App Links into
  the native shells were considered and deferred (needs a
  `dannyrhubarb.github.io` user-site repo for `/.well-known/` at the
  domain root, plus AASA content-type caveats on Pages).
- **Link-failure diagnosis (`scr-mp-fail`, field lesson 2026-08)**: room
  pairing can succeed while the P2P link silently never opens — seen live
  with **Apple iCloud Private Relay** on (it hides the device's address,
  so a STUN-only attempt just hangs at "connecting…"; turning it off
  fixed it). A 25 s setup watchdog (`CONNECT_FAIL_MS`, armed in
  `makePeer`, cleared on DataChannel open) plus a pre-open
  `connectionState == "failed"` both land on the diagnosis screen: the
  facts gathered during the attempt (reason, pc states, TURN available
  or STUN-only, local candidate counts by type lan/public/relay, how
  many candidates arrived from the peer) plus targeted hints — no-TURN →
  run the bootstrap, no srflx/relay → UDP blocked, zero remote → the
  other side is stuck too, and always the VPN/Private-Relay warning. A
  post-open failure keeps the existing degrade-to-solo banner instead
  (never a dialog over a live flight).
- **Run end**: the module watches the analytics run channel
  (`run_end_seq` + cause/dist/ticks mirrors — no new channel) for its own
  end, sends `run_end {cause, score}` (time levels: completion seconds,
  DNF = null), and flags the peer's via `set_mp_remote_over`. A received
  run_end is a transient banner ("X CRASHED — 1234 m" / "X FINISHED —
  0:58.3"; quick restarts pass silently) — never a blocking screen. While
  `pegMP.inRace()` the ui-state poll's wrap-up is `pegMP.respawn()` — the
  submit dialog first as usual, closed through the `"mp-respawn"`
  pseudo-target (`closeNameDialog`); the consent detour stays out of the
  room flow. A reset-cause end restarts the outgoing stream directly from
  the run watcher (no dialog on resets). Leaving: the pause screen's Exit
  calls `pegMP.leaveRoom()` BEFORE the reset (teardown first ⇒ no respawn
  marker is sent; the peer gets a clean disconnect) and restores the
  level's original text so a pinned random seed re-rolls in solo play.
  **Disconnect mid-race degrades to a normal solo run** ("OPPONENT LEFT —
  FLYING SOLO" banner, feed torn down) — the local flight is never
  blocked by network state.

- **Video & voice bubbles (2026-09, #200 step 1 — raw feed)**: opt-in
  per device (`pegasus_mp_av`, remembered; `Camera & mic` toggle on the
  host / lobby screens with a mirrored local preview, and on the pause
  screen while in a room — `#pause-av-row`, shown by `syncBubbles`), the
  selfie camera (240² @15 fps ideal, `maxBitrate` 200 kbit/s,
  `maxFramerate` 15 on the sender) + mic (echo cancellation on) ride the
  SAME `RTCPeerConnection` as the input stream as media tracks — P2P, TURN
  fallback, nothing through AWS beyond the SDP. **Transceivers are
  negotiated up front** (host `addTransceiver` audio+video before the
  first offer; the guest flips the offered m-lines to `sendrecv` in
  `onSignal` before answering), so switching a camera on/off later is
  `sender.replaceTrack` — no renegotiation, #148's offer/answer flow is
  untouched. A DataChannel `{t:"av", video, audio}` message is the truth
  for the opponent's bubble (track `mute` is only a belt-and-braces hide).
  HUD: `#mp-bubbles` (HTML, `pointer-events: none`, top-right under the
  corner buttons) — the opponent 96 px magenta with callsign, own 64 px
  cyan mirrored — shown only while in a room AND the canvas is live
  (`showScreen`/`closeMenu` call `pegMP.syncBubbles()` next to
  `syncWakeLock`; the try/catch covers pegMP's TDZ at boot). The remote
  `<video>` carries the voice track too, so it stays attached (audible)
  while hidden. Leaving the room / any teardown stops the capture but
  keeps the preference; iOS ends the capture on backgrounding — the track
  `ended` + `visibilitychange` handlers re-acquire. A failed
  `getUserMedia` (denied, no camera) flips the preference off and shows a
  banner; a combined camera+mic failure retries video-only. Shells: iOS
  `NSCameraUsageDescription` + `NSMicrophoneUsageDescription` and the
  `WKUIDelegate` media-capture callback (one system prompt per media
  type, no WebKit sheet — see "iOS app"); Android `CAMERA` /
  `RECORD_AUDIO` permissions + a `WebChromeClient.onPermissionRequest`
  that forwards the runtime prompt and grants only the bundle origin.
  Step 2 (face-tracked crop before sending) is the follow-up in #200.
## Physics notes

The body has `angular_damping(3.0)` and `linear_damping(0.2)` (see Thrust /
glow system for why the linear term exists).

**Fixed timestep**: physics steps at `PHYSICS_DT = 1/120 s` through an
accumulator in the main loop (catch-up capped at 0.05 s per frame). Each step
is one `Sim::tick(InputState)` — forces/torques are recomputed **per tick**
from the frame's quantized input (constant within a frame), so handling is
identical on 60/120/144 Hz displays *and* the sim is a pure function of the
input stream (see the determinism rules in the replay section). Rendering
interpolates the ship between the last two physics states (`prev_ship` +
`alpha = accum/PHYSICS_DT`); anything that teleports the body (reset/respawn)
must also snap `prev_ship` or the camera lerps across the jump for a frame.

The ship uses a **compound collider** of three **capsules** (stadium shapes) parented to the same rigid body, tracing the lander silhouette of the 1.5× scaled visual. Capsules are the closest primitive Rapier offers to an ellipse — they hug the rounded hull tighter than boxes and slide off rocks without corners catching. Endpoints are in scaled world units (ship-local frame):
- **Fuselage**: `capsule((0, +0.42), (0, −0.08), r=0.26)` — rounded nose down to mid-hull.
- **Left leg pod**: `capsule((−0.26, −0.30), (−0.33, −0.64), r=0.09)` — angled out to the foot.
- **Right leg pod**: `capsule((+0.26, −0.30), (+0.33, −0.64), r=0.09)` — mirror.

Each is built `ColliderBuilder::new(SharedShape::capsule(a, b, r)).restitution(0.2)` (`SharedShape`, `point!` from `rapier2d::prelude::*`). Rapier 2D has **no ellipse primitive** — capsule is the smooth-rounded alternative; for an even tighter (but faceted) fit you could use `convex_hull` of the `SHIP_TRIS` vertices, at the cost of filling the concave notch between the feet. Cave walls are `segment` colliders (zero thickness). The body has `ccd_enabled(true)`, which matters more now: a long free-fall down a vertical shaft can pass 50 m/s, far above the ~17 m/s of normal cave flight.

**RCS / attitude thrusters** (cosmetic particles, `kind 1/2`): bottom nozzles flanking the main booster vent **downward** (like a mini main thruster). Turning **left** → left nozzle at scaled-local `(−0.30, −0.71)`; turning **right** → right nozzle at `(0.30, −0.71)`. Gas exits `−Y` (downward) from both. The x positions sit in the leg nozzle (gold accent: unscaled x ≈ ±0.152–0.249 → midpoint ±0.30 scaled). Emission coords are in **scaled world units** — `lp()`/`ld()` do **not** apply `SHIP_SCALE` (only the render-time `rot` closure does), so don't multiply these by `SHIP_SCALE` (an earlier bug double-scaled them to ±0.60 and spawned the puffs outside the hull).

## iOS app (`ios/`)

A thin native wrapper that bundles the web build into an offline-capable
iOS app — the game runs unmodified in a full-screen WKWebView (same WebKit
as iOS Safari). Built and signed on a Mac with Xcode; `ios/README.md` has
the full walkthrough (free-Apple-ID device signing vs. the paid program).
Not part of the web deploy pipeline. `index.html`'s only app-awareness is
the **screen wake lock** (`syncWakeLock`, next to
`showScreen`/`closeMenu`): while the canvas is live — no menu screen up:
flight, the wreck phase, or replay playback — the page holds the screen
on (a hands-off glide or replay has no touches, so the OS screen timeout
would otherwise dim mid-run), and any open screen releases it. Three
holders, each feature-detected + try/caught: iOS = the `pegasusKeepAwake`
`WKScriptMessageHandler` (→ `UIApplication.isIdleTimerDisabled`), Android
= the `PegasusApp` `@JavascriptInterface` (→ `webView.keepScreenOn` — a
View flag, no WakeLock permission, auto-released when the window isn't
visible), and the plain website = the browser **Screen Wake Lock API**
(Safari 16.4+/Chrome; the sentinel is force-released on tab-hide and
re-acquired on the `visibilitychange` back while still wanted).
- `ios/sync-web.sh` assembles `ios/Pegasus/WebRoot/` (**gitignored** build
  product, like `pegasus.wasm`; a `.gitkeep` holds the folder for Xcode).
  It mirrors `.github/actions/build-site` — **keep them in sync when the
  site's file set changes** — with three deliberate differences: **no
  `version.json`** (the stale-cache toast is meaningless in-app; the page
  treats the 404 as feature-off), **`config.json` fetched from the live
  Pages deployment** (the `BACKEND_CONFIG_JSON` variable isn't available
  locally; unreachable ⇒ online scores off; both sync scripts try
  `pegasusmoonlander.com` first and fall back to the legacy github.io
  origin with `-L` — it 301s once the custom domain is live — until the
  domain has soaked, #171), and the injected revision
  suffixed **`-ios`** (About screen / analytics / replay build id — the
  page still env-tags these builds `prod`, so app sessions show up in
  analytics as iOS webview device-mix).
- `ios/Pegasus.xcodeproj` + `ios/Pegasus/*.swift`: WebRoot is served via a
  **custom `pegasus://` scheme handler** (`WKURLSchemeHandler`) because
  `fetch()` doesn't work on `file://` URLs and the game fetches its wasm,
  levels, manifest and config at runtime; the scheme is also the stable
  origin for localStorage. The handler **strips query strings** (`?v=`,
  `?fresh=`) and **404s missing optional files** — both load-bearing. The
  webview fills the WHOLE screen, not the safe area (the page reads
  `env(safe-area-inset-*)` itself via `viewport-fit=cover`); the **status
  bar stays visible** (light content), drawn over the game's starfield —
  the HUD/menu already lay out below the top safe-area inset; scrolling/
  bounce **toggled per page** (`syncScrollLock` in `didCommit`): OFF on
  index.html — a scrollable canvas would fight the touch stick — and ON
  for any other committed URL (the bundled licenses/LICENSE/privacy
  document pages; with it left globally off, the licenses page rendered
  as a frozen first screen — fixed 2026-08, the game's pushState entries
  carry no URL so in-game history never flips it); **back-forward
  gestures ON** (edge-swipe = the game's own
  one-screen-back history stack, same as Safari); http(s) links open in
  Safari, `target="_blank"` bundle pages (third-party licenses) load in
  place with swipe-back. The bundled document pages
  (`third-party-licenses.html` — via its generator's template —
  and `privacy.html`) carry `viewport-fit=cover` + their own `--inset-*`
  safe-area padding (same max(env, `--app-inset-*`) rule as index.html;
  the shells fill the whole screen, so without it they tucked under the
  notch/status bar — fixed 2026-08, covers Android's edge-to-edge shell
  too). A document-start `WKUserScript` injects
  `window.__pegAppBuild` ("1.0 (42)" — CFBundleShortVersionString +
  CFBundleVersion, the latter stamped with the CI run number) for the
  About screen's App build row. WebRoot ships as an Xcode **folder
  reference**, so re-running the sync + rebuilding needs no project
  edits.
- **Safe-area inset injection (launch-jank fix, 2026-07)**:
  `env(safe-area-inset-*)` reads **0 at a WKWebView's first paint** —
  WebKit propagates the insets asynchronously a couple of frames later —
  so the menu painted with its 28px fallback padding tucked under the
  Dynamic Island and then visibly jumped down ~31 pt during launch
  (diagnosed frame-by-frame from a screen recording — which turned out to
  be of the **home-screen PWA**, which shares the WebKit behavior but has
  no shell to inject anything; the web side has its own standalone-boot
  gate, see "Game menu"). App-shell fix:
  `GameViewController` defers `load()` to the first
  `viewDidLayoutSubviews` (in `viewDidLoad` the view isn't in a window
  yet, so `view.safeAreaInsets` is still zero) and injects the real
  insets as the `--app-inset-*` CSS vars via a document-start user
  script, re-pushed from `viewSafeAreaInsetsDidChange` (rotation moves
  the notch inset to another edge; the user scripts are re-registered
  too so a licenses-page navigation also boots current). `index.html`
  folds them in as **`--inset-*` in `:root`
  (`max(env(safe-area-inset-*), var(--app-inset-*, 0px))`) — every
  safe-area consumer (menu `.screen` padding, corner buttons, update
  toast, replay bar, the JS probe divs feeding `set_safe_area`) reads
  `--inset-*`, NEVER `env()` directly; keep it that way for new CSS.**
  On the plain website the `--app-inset-*` vars are unset ⇒ pure
  `env()`, behavior unchanged (verified headless: unset vars reproduce
  the old computed styles exactly). The Android shell doesn't inject
  (no jank reported there); it can adopt the same vars if ever needed.
- App icon: `icon.svg` rendered to an opaque 1024×1024 PNG in
  `Assets.xcassets` (no alpha — App Store validation rejects it);
  re-render if the SVG changes.
- **CI**: `ios-build.yml` (PRs touching `ios/` — sync + UNSIGNED
  xcodebuild, no secrets) and `ios-testflight.yml` (**manual dispatch
  ONLY** — automatic publish on `main` pushes is PAUSED since 2026-08,
  owner request; see the paused-trigger note under Android CI —
  cloud-signed archive → TestFlight;
  needs the four `APP_STORE_CONNECT_API_*`/`APPLE_TEAM_ID` repo secrets
  and the ASC app record; build number = workflow run number). Both run on
  **`macos-26` and select the newest stable Xcode** — App Store Connect
  rejects uploads built with older SDKs ("must be built with the iOS 26
  SDK", seen live 2026-07 on the macos-15 image's default Xcode 16.4).
  Both need
  the **committed shared scheme** (`xcshareddata/xcschemes/`) — xcodebuild
  on a fresh runner can't see locally auto-generated schemes — and
  `fetch-depth: 0` (What's New). `Info.plist` carries
  `ITSAppUsesNonExemptEncryption = false` so TestFlight builds skip the
  per-upload compliance question. After the upload,
  `ios/testflight-distribute.py` (ASC API, same key) waits out Apple's
  build processing, submits the build to Beta App Review and attaches it
  to the beta group named by the `TESTFLIGHT_GROUP_NAME` repo variable
  (default "Public beta") — external testers get every build hands-free;
  a group that doesn't exist yet is a soft no-op, and the group ATTACH
  retries through ASC's propagation lag (a just-processed build can 404
  on the betaGroups relationship endpoint while /v1/builds already calls
  it VALID — seen live 2026-08; exhausted retries degrade to a soft
  notice like the 422 path, the next build supersedes).
  **Cert-cap gotcha (hit live 2026-08, run 14)**: cloud signing mints a
  NEW Apple Development certificate on every run — the previous one's
  private key dies with its ephemeral runner, so the cert can never be
  reused — and the dead certs pile up in the Apple account until its
  certificate cap breaks every archive ("Your account has reached the
  maximum number of certificates" + the knock-on "No profiles for
  'se.danielfalk.pegasus' were found"). `ios/asc-cleanup-certs.py` (the
  step before Archive, same ASC key) revokes DEVELOPMENT certificates
  each run so the account stays far under the cap — best-effort, its
  failures are workflow warnings and never fail the build; distribution
  certificates are deliberately untouched. It also revokes a dev cert
  minted by a human's local Xcode — automatic signing re-creates that on
  their next build-and-run, the accepted cost. If the warnings ever say
  403 on revoke, the ASC API key's role can't manage certificates (needs
  Admin) and the pile must be cleared by hand at developer.apple.com →
  Certificates.

## Android app (`android/`)

The Android twin of `ios/`: a thin Kotlin `WebView` shell bundling the web
build, offline-capable, buildable anywhere with a JDK + Android SDK (no
Mac). `android/README.md` has the build/signing/Play walkthrough.
- `android/sync-web.sh` assembles `app/src/main/assets/webroot/`
  (**gitignored**, `.gitkeep` holds the folder) — mirrors `ios/sync-web.sh`
  and `.github/actions/build-site` (keep all three in sync); revision
  suffixed **`-android`**, no `version.json`, `config.json` from the live
  deployment.
- `MainActivity.kt` serves webroot via **`WebViewAssetLoader`** on the
  reserved `appassets.androidplatform.net` origin (fetch/localStorage need
  a real origin — same reason as the iOS scheme handler) with a CUSTOM
  path handler that answers **real 404s** for missing optional files
  (stock `AssetsPathHandler` would fall through to the network, where the
  reserved domain fails DNS and fetch ERRORS instead of 404ing). System
  back = the game's own one-screen-back history stack (the site already
  implements Android back); external links open the browser;
  `android:configChanges` keeps the activity (= the live game) alive
  across rotation; edge-to-edge with cutout `shortEdges` and **NO hidden
  system bars** — status AND navigation bars stay visible (transparent,
  over the game), matching the game-in-Chrome baseline. **Do not hide the
  nav bar with `BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE`**: any upward swipe
  near the bottom — where the stick lives — transiently reveals the bars,
  and the NEXT touch is consumed to dismiss them, so gameplay's stream of
  bottom-area swipes really does lose touches. **It was NOT, however, the
  cause of the 2026-07 "only every few touches goes through" report** — it
  was blamed for it and removed on that theory, the report survived, and
  the real cause turned out to be game-side phase collapse (see
  `docs/touch-input.md`). Keep the bars visible on the merits above; don't
  cite it as that bug's fix.
  Also: `window.insetsController` NPEs before `setContentView` on
  Android 11+ (the boot-crash the emulator smoke test caught — the pre-11
  path masked it by creating the decor as a side effect); the
  edge-to-edge call must come after. The `PegasusApp` JS interface is the
  Android half of the keep-awake bridge (see "iOS app") and also answers
  `appBuild()` (versionName + versionCode) for the About screen's App
  build row. **targetSdk/compileSdk = 36** (2026-08, Play requires
  targeting within 1 year of the latest Android release or updates are
  blocked — expect this bump roughly yearly). **AGP 9.x** (2026-08, same
  push): Kotlin is BUILT INTO AGP 9 — `org.jetbrains.kotlin.android` must
  not be applied any more (it conflicts, so there's no Kotlin version pin
  left in the repo) and the `kotlinOptions` block is gone (jvmTarget
  defaults to `compileOptions.targetCompatibility`); the Gradle floor
  moved to 9.5.0, pinned in all four android workflows
  (`gradle-version:`) — bump those pins together with AGP, they're the
  only Gradle version source (no wrapper in-repo). **Gotcha**: targeting
  36 enables predictive
  back by default, which stops delivering `onBackPressed()` — the whole
  back-button story here — so the manifest opts out via
  `android:enableOnBackInvokedCallback="false"`; a future Android release
  will drop that opt-out, at which point MainActivity must migrate to
  `OnBackInvokedDispatcher` (registered only while the WebView can go
  back, so the leave-the-app gesture keeps its predictive animation).
- **CI**: `android-build.yml` (PRs touching `android/` — debug APK built
  on ubuntu + attached as an installable artifact, no secrets) and
  `android-release.yml` (**manual dispatch ONLY** — see the paused-trigger
  note below — signed AAB + universal APK artifacts; uploads the AAB to
  the **Play internal track** when `PLAY_SERVICE_ACCOUNT_JSON` is set,
  skipped otherwise; needs the four `ANDROID_KEYSTORE_*`/`ANDROID_KEY_*`
  secrets; versionCode = workflow run number; **the first Play upload
  must be manual** — Google requirement). **Automatic publish PAUSED
  (2026-08, owner request)**: both release workflows (`android-release.yml`
  and `ios-testflight.yml`) used to also run on every `main` push that
  could reach a device; store publishing is now a deliberate manual action
  (Actions → run workflow). **`release-apps.yml` (Release apps)** is the
  one-click wrapper: dispatching it fires both store workflows (per-platform
  boolean inputs to skip one). It API-dispatches them (`gh workflow run`)
  rather than `workflow_call`ing them — DELIBERATE: a called workflow runs
  under the caller's `github.run_number`, and both apps use their own run
  number as the store build number (CFBundleVersion / versionCode must keep
  increasing), so a fresh wrapper counter would reset them; dispatch keeps
  each workflow's own counter. (GITHUB_TOKEN-created `workflow_dispatch`
  events are exempt from the no-recursive-workflows guard — unlike the
  `push` case that makes publish-pages.yml use `workflow_run`.)
  The push triggers are COMMENTED OUT in place
  in both files, hard-won filter comments included — resuming automatic
  publishing is uncommenting those blocks verbatim, nothing else. The
  preserved rationale (still binding when they return): the filters are
  `paths-ignore` (docs/tests/markdown/named-irrelevant-workflows), NOT a
  `paths` allowlist — INVERTED ON PURPOSE: the apps bundle the whole web
  build, so `ios/`+`android/` are only the SHELLS and a change to `src/`,
  `index.html`, `mq_js_bundle.js` or `levels/` changes what ships to a
  device. The old shell-only filters left both apps frozen while the
  website moved (the 2026-07 Android touch fix shipped only because its PR
  also happened to touch `android/`). Failing OPEN costs at most a wasted
  build; failing closed ships a stale app silently. A rebase merge of N
  commits is ONE push ⇒ one release run, and `concurrency` serializes
  merges landing close together. The ignore list names the individual
  irrelevant workflows rather than all of `.github/` **so each release
  workflow's OWN file stays unignored** — a fix to it is then exercised by
  the next merge, which is how `03e67a8` (a commit touching nothing but
  `android-release.yml`) was verified. **Pause side effect**: the sideload
  APK at Pages `app/pegasus.apk` and TestFlight both go stale between
  manual runs — a merged fix reaches the website automatically but reaches
  devices only when someone dispatches the release workflows.
- **`tools/check-bundle-sync.py`** (CI, 2026-07) pins the three
  hand-maintained copy lists — `build-site`, `ios/sync-web.sh`,
  `android/sync-web.sh` — against each other. Adding a file to the website
  and forgetting the shells is otherwise SILENT (the sync scripts still
  exit 0, just one file lighter — running them in CI does not catch it),
  and the apps ship incomplete. Intentional web-only files live in the
  script's `WEB_ONLY` set, each with a reason: the generated `icon-*.png`
  (nothing in a WKWebView/WebView reads `apple-touch-icon` or the web
  manifest, and generating them would make librsvg an app-build
  prerequisite) and `editor.html` (deliberately unlinked, so unreachable
  from an app shell). Release signing reads
  `PEGASUS_KEYSTORE_*` env vars in `app/build.gradle.kts`; nothing
  signing-related lives in the repo. The release also **publishes the
  signed APK to GitHub Pages** at `app/pegasus.apk` (direct-download
  sideload link): it syncs the `app/` subdir of `gh-pages` via
  `sync-pages-branch` (whose root replace keeps `app/` like `pr-*`) and
  `publish-pages.yml` triggers on this workflow's name.
- **On-demand PR test APK** (`android-test-apk.yml`, 2026-07): put a
  **`test-apk` label** on a PR and it builds one, refreshing on every push
  while the label stays on (`types: [labeled, synchronize]`, gated on the
  added label OR `labels.*.name` already containing it); manual dispatch
  takes a PR number instead. Deliberately OPT-IN — most PRs never need a
  device build. It builds the **`preview` build type**: `initWith(release)`
  plus `applicationIdSuffix = ".preview"` and the launcher label
  "Pegasus PR" (a `${appLabel}` manifest placeholder, defaulted in
  `defaultConfig`), so a tester installs it **next to** the real app instead
  of over it — no uninstall, and the real app keeps its localStorage
  (settings, pilot name, board cache). `initWith` carries the release
  signing config over, so successive PR builds upgrade in place rather than
  tripping a signature mismatch; `versionNameSuffix` comes from
  `PEGASUS_VERSION_SUFFIX` so the About screen's App build row reads
  `1.0-pr<n> (<run>)` and names the PR being tested. The workflow also sets
  `PEGASUS_REV` (sync-web.sh honours it) to the PR **head** sha — the merge
  ref the checkout sits on has a merge-commit sha that means nothing to a
  tester. Published to `pr-<n>/app/pegasus.apk`, NOT the main `app/`
  download (see "Deploy pipeline" for why that path and its teardown).
- Launcher icons rendered from `icon.svg` (adaptive foreground 108dp
  densities + legacy sizes); re-render if the SVG changes. **Render the
  adaptive foreground with Playwright (viewport = exact pixel size), NOT
  `chromium --screenshot`** — the CLI's window-size doesn't reliably match
  the layout viewport, which shipped a mispositioned foreground once
  (field screenshot: faint ship sliver top-right of the icon circle);
  verify by simulating the launcher mask (`clip-path: circle(33.3%)`).
  The in-page **touch tracer** (bottom-left overlay, only while the Debug
  HUD setting is on — standalone script at the end of `index.html`)
  counts raw touchstart/move/end/CANCEL at the window capture phase:
  built to bisect the Android-app "touches sometimes ignored" report
  (CANCEL jumping = something claims the gesture; nothing incrementing =
  events never delivered).

## Native-app install prompts (web → store apps, 2026-09)

The website advertises the store apps through the two PLATFORM-NATIVE
mechanisms — no custom banner, no JS, nothing to dismiss or persist:
- **iOS — Safari Smart App Banner**: `<meta name="apple-itunes-app"
  content="app-id=6792584910">` in `index.html`'s `<head>` (the numeric
  **Apple ID** of the App Store Connect app record — App Information →
  General; not the bundle id). Safari draws the banner itself, above the
  page, with View/Open + a close button, and renders NOTHING while the app
  isn't purchasable in the viewer's storefront — so the tag is inert until
  the App Store release, needs no gating, and lights up on its own the
  moment the app goes live. Never shows inside a frame, the simulator, an
  installed home-screen PWA or the WKWebView shell. `app-argument` is
  deliberately unset (the app has no URL handler to receive it).
- **Android — Chrome's related-app install prompt**: `manifest.json`
  carries `related_applications` (`platform: "play"`, id =
  `se.danielfalk.pegasus`, the release `applicationId` — the `.preview`
  test-APK id is NOT listed on purpose; plus an informational `itunes`
  entry Chrome ignores) and **`prefer_related_applications: true`**
  (owner decision 2026-09): Chrome offers the Play app INSTEAD of the PWA
  install. Chrome resolves the id against Play at prompt time, so
  **until the app is live on Play (production or open testing) Android
  Chrome shows NO ambient install prompt at all** — not the Play one (no
  listing) and not the PWA one (preferred away); the browser-menu "Add to
  Home screen" still works. Accepted as the pre-launch state; flip
  `prefer_related_applications` to `false` if the PWA prompt is wanted
  back in the meantime. The prompt never appears inside the WebView shell.
- **`.well-known/` — App Links + Universal Links (2026-09)**: NOT part of
  the banners; it's what makes `https://pegasusmoonlander.com/` open in the
  INSTALLED app (and, on Android, what `navigator.getInstalledRelatedApps()`
  would verify). Scope on both platforms is the site root + `/index.html`
  ONLY — previews (`pr-<n>/`), `editor.html`, the APK download and the
  document pages must keep opening in the browser (the app bundles its
  own build, so a preview link opening in it would show the wrong
  build). Same-origin navigation (the editor's `?custom=1` test-fly
  handoff) stays in the browser on both OSes by their own rules. A link's
  QUERY STRING is forwarded into the bundled `index.html` on a cold start
  (utm attribution keeps working for app opens; `?custom=1` with no
  stored draft falls through to the normal boot), while a link arriving
  in a RUNNING app is ignored — reloading would kill a run in progress.
  **#148 hook**: the multiplayer PR's invite links are `/?join=<code>`
  (auto-join on landing) — cold starts already carry them, but a WARM
  invite (app open, friend's link tapped) is the one case the no-op gets
  wrong; when #148 lands, both shells must hand the code to the page via
  evaluateJavaScript instead (TODO(#148) comments at `SceneDelegate.scene(_:continue:)`
  and `MainActivity.onCreate`), and a fragment-form invite would need the
  fragment forwarded too.
  Apex only: `www` 301s to the apex and neither OS verifies a
  redirecting host.
  - `assetlinks.json` (Digital Asset Links): release package
    `se.danielfalk.pegasus` + the SHA-256 of Play's **classical**
    app-signing certificate (Play Console → Setup → App integrity —
    Google's key under Play App Signing, NOT the upload keystore; the
    post-quantum key Play also shows can be listed alongside,
    `sha256_cert_fingerprints` is an array). Public by nature (the
    certificate, not the key — `apksigner verify --print-certs` on the
    sideload APK prints it), so it's plain repo content. App side:
    `AndroidManifest.xml`'s `android:autoVerify="true"` VIEW filter for
    the two paths; `MainActivity.onCreate` forwards `intent.data.query`.
    The `.preview` test APK's id is deliberately NOT listed, so its links
    stay in the browser. Verify on a device with
    `adb shell pm get-app-links se.danielfalk.pegasus` (`verified`).
  - `apple-app-site-association` (Universal Links): the repo copy carries
    an **`__APPLE_TEAM_ID__` placeholder** — `build-site` stamps it from
    the new `apple-team-id` input (`secrets.APPLE_TEAM_ID`, passed by
    `deploy.yml` and `preview-deploy.yml`) and REMOVES the file when the
    input is empty (forks / local builds ship no AASA). Owner preference:
    the Team ID stays a secret rather than repo content even though the
    served file is public. App side: `ios/Pegasus/Pegasus.entitlements`
    (`applinks:pegasusmoonlander.com`, wired via `CODE_SIGN_ENTITLEMENTS`
    in both build configs — the unsigned CI build ignores it);
    `SceneDelegate` forwards a cold-start `NSUserActivityTypeBrowsingWeb`
    URL's query via `GameViewController.launchQuery` and no-ops the warm
    `scene(_:continue:)`. `-allowProvisioningUpdates` in the TestFlight
    archive is expected to add the Associated Domains capability to the
    App ID by itself; if the archive ever fails with a "doesn't support
    the Associated Domains capability" profile error, enable it once by
    hand at developer.apple.com → Identifiers. Apple's CURRENT
    associated-domains doc requires only HTTPS + valid cert + no
    redirects + no file extension — the oft-quoted `application/json`
    MIME rule comes from the archived iOS 9 guide and is not in the
    current one; GitHub Pages serves the extensionless file as
    `application/octet-stream`. Verify after a `main` deploy via Apple's
    CDN (what devices actually read):
    `curl https://app-site-association.cdn-apple.com/a/v1/pegasusmoonlander.com`
    (the raw site copy is at
    `https://pegasusmoonlander.com/.well-known/apple-app-site-association`).
  - **Pipeline**: dot-dirs reach Pages only because `publish-pages.yml`
    uploads hidden files (see "Deploy pipeline & PR previews").
    `check-bundle-sync.py` lists `.well-known` as `WEB_ONLY` — nothing in
    a shell fetches it, and a copy on an app-local origin verifies nothing.
Both are browser chrome, not page content, so nothing here interacts with
the menu overlay, the canvas or the touch stick. Bundled copies of
`manifest.json`/`index.html` in the app shells carry the same fields
harmlessly (a WebView reads neither).

## License

Pegasus is **GPL-3.0-or-later** (`LICENSE`). Contributors sign on via the
CLA in `CLA.md` (agreement is a PR-description statement, not a separate
signing step — see `CONTRIBUTING.md`): they keep copyright on their own
Contributions but also grant the Maintainer relicensing rights, so the
Maintainer can offer a closed-source/commercial license to a specific third
party later without having to track down every contributor for consent.
GPL itself does **not** block commercial use or require anyone to ask
permission — it only forces derivatives that get *distributed* (which
includes serving the wasm binary to a browser) to stay open and freely
redistributable; permission-gated commercial exceptions only work because
the Maintainer, as a rights holder, can grant a separate license alongside
the public GPL one. **Caveat:** the ship mesh (`src/ship_mesh.rs`, see
"Origin of the ship mesh" below) is joint work with a friend from a 2005
Flash project — get their sign-off before offering any commercial exception
that includes it, since they hold copyright on that asset too.

**Third-party attribution**: `third-party-licenses.html` (repo root, linked
from the menu's About screen, copied into the site by `build-site` along
with `LICENSE`) lists every crate compiled into the wasm plus the vendored
miniquad JS loader and the vendored JetBrains Mono webfont (OFL-1.1, its
text read from `fonts/OFL.txt` — see the menu-font note under "Game menu"),
with real copyright notices extracted from the cargo
registry sources and one copy of each elected license text (MIT / Apache-2.0
/ Zlib / Unicode-3.0 as of 2026-07, + OFL-1.1 since 2026-08). It is **generated, not hand-edited**:
re-run `python3 tools/gen-third-party-licenses.py` (after a `cargo build`,
which populates the registry cache) whenever `Cargo.lock` changes, and
commit the refreshed page.

## Git workflow
- **Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/)
  (owner decision, 2026-09)**: `<type>(<scope>)!: <summary>` — imperative,
  lower-case, no trailing period, ≤ 72 chars. Types: `feat`, `fix`, `perf`,
  `refactor`, `docs`, `test`, `ci`, `build` (deps/toolchain/Cargo/Gradle/
  Xcode bumps), `chore` (no code-behaviour change and none of the above,
  e.g. an `app-policy.json` edit), `revert`. **Scopes are the `area:`
  label names** (see "Issue labels" below): `sim`, `levels`, `editor`,
  `ui`, `input`, `render`, `replay`, `scores`, `mp` (multiplayer),
  `analytics`, `ios`, `android`, `ci` — plus `policy` (app-policy.json)
  and `deps`; omit the scope when a change spans several. **`!` (plus a
  `BREAKING CHANGE:` footer) marks a change that alters sim results or the
  replay format** — exactly the changes that need the backend repin, so
  `git log --grep '!:'` is the repin history; pair it with the
  `status: needs-repin` label on the PR. The `Whats-new:` trailer is
  unchanged and complementary: the subject is for developers, the trailer
  is the player-language note (and stays in the trailer block, see "What's
  new page"). Example:
  ```
  fix(input): claim the stick by touch id, not TouchPhase::Started

  Android collapses touchstart+touchmove into one Moved entry, so …

  Whats-new: Fixed: The touch stick no longer ignores every few touches on Android
  Co-Authored-By: …
  ```
  PR titles use the same format (a squash merge takes the PR title as the
  subject). History before 2026-09 is free-form and stays that way —
  never rewrite it to fit.
- **Issue labels are a checked-in convention**: `.github/labels.json` is
  the source of truth, `tools/sync-labels.py` applies it (create / update /
  rename-by-alias / `--prune`, stdlib only, runnable locally with a
  `GITHUB_TOKEN`), and `.github/workflows/labels.yml` runs it on every
  `main` push touching either — so a new label is a PR editing the JSON,
  never a UI click (the workflow prunes UI-created labels). Three
  prefixed groups — exactly one `type:` per issue, any number of `area:` /
  `status:`: **`type:`** `bug` / `feature` / `idea` (design notes, not
  committed to — e.g. the video-avatar write-up #200) / `chore` / `docs` /
  `question`; **`area:`** the commit scopes above (all green — the
  dimension, not the value, carries the colour); **`status:`** `blocked`
  (waiting on something external) / `held` (parked on purpose — the
  post-1.0 level PRs) / `needs-repin` (sim results or replay format
  change; the backend must repin + deploy, see its CLAUDE.md). The GitHub
  defaults were RENAMED into the groups (rename keeps existing issue
  associations); `wontfix` / `duplicate` / `invalid` were dropped —
  GitHub's close reasons carry that. **`test-apk` stays un-prefixed**: it
  is a workflow trigger and `android-test-apk.yml` matches it by exact
  name. pegasus-backend mirrors the scheme with its own `area:` set
  (`verify` / `scores` / `analytics` / `signaling` / `infra` / `ci`).
- **Keep version pins at latest stable (owner policy, 2026-08)**: GitHub
  Actions majors, runner images, Gradle/AGP/Kotlin/SDK levels, Xcode and
  dependencies should track the latest stable release. When a newer
  stable version is discovered to be available (a deprecation notice in a
  run log, a new major spotted in passing), **proactively open a small
  housekeeping PR with the bumps** rather than waiting for something to
  force it — the point is avoiding integration debt. Check release notes
  for breaking changes before crossing majors; CI + the preview deploy
  validate the rest. **HARD EXCEPTION — sim-affecting crates**: `rapier2d`,
  `glam` (pinned to macroquad's re-export) and anything else compiled into
  `sim-core` must NEVER be bumped as routine housekeeping — a physics-crate
  bump changes simulation results, which breaks stored replays, the racing
  ghost and backend score verification, and requires the full REPIN dance
  (see the backend CLAUDE.md) as a deliberate, coordinated change.
  macroquad is pinned at 0.4.15 on purpose (vendored JS bundle matches it).
- **Commit authorship**: every commit's author should be the real human
  contributor driving the session — never `Claude <noreply@anthropic.com>`.
  Use that person's GitHub-provided private noreply address
  (`<id>+<username>@users.noreply.github.com`, found via their GitHub profile
  or existing commits/PRs from them) so their real email stays out of
  history while commits still attribute to their account. Claude is credited
  via the `Co-Authored-By: Claude <noreply@anthropic.com>` trailer instead
  (already appended per the standing commit instructions). Set it per-commit
  (`git commit --author="Name <id+username@users.noreply.github.com>"` or the
  `GIT_AUTHOR_NAME`/`GIT_AUTHOR_EMAIL` env vars) — never change global git
  config.
- **Always open a PR** after pushing a feature branch — standing instruction
  from the owner (no need to ask first). The PR also produces a phone-testable
  preview deployment at `pr-<n>/`.
- Development branch: `claude/flux-one-minute-level-c3d5zy` (current); previous: `claude/frontend-ios-app-bundle-jhhqr6`
- Merges to `main` via rebase PRs using the GitHub MCP tools (`mcp__github__create_pull_request`, `mcp__github__merge_pull_request`).
- **Curate the branch before merging.** Rebase merges land every branch
  commit on `main` verbatim, so branch noise becomes permanent history.
  Before merging, squash the branch into a sensible set of logically
  distinct commits: fold fixups/lint-fixes into the commit they fix, and
  drop add+revert pairs and dead-end experiments entirely — a revert pair
  is net-zero code but poisons `git bisect` (it can land between the two
  and test a state that was never meant to ship) and buries the real
  changes. Keep genuinely separate concerns as separate commits; curate,
  don't flatten — one-commit squashes are for one-concern PRs (or just use
  GitHub's squash merge for those). Interactive rebase isn't available
  here; use `git reset --soft $(git merge-base HEAD origin/main)` and
  re-commit in slices instead. Do it BEFORE requesting review or right
  before merge — force-pushes orphan inline review comments. (Cautionary
  example: PR #66 merged with the replay-input-widget commit AND its
  revert, both now on `main`.)
- Branch consistently diverges from main after merges — always `git fetch origin main && git rebase origin/main && git push --force-with-lease` before creating a PR to avoid merge conflicts.
- The wasm binary (`pegasus.wasm`) is **not tracked** (gitignored) — deploy builds it from source, and for local play you build it into the repo root per the README. It previously lived in git and conflicted on every rebase; don't re-add it.
