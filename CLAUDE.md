# Pegasus — Moon Lander

Rust + macroquad 0.4.15 + Rapier 2D game compiled to WebAssembly and served via GitHub Pages. The player pilots a ship through a procedurally-generated scrolling cave using thrust and rotation controls.

> **Keep this file current.** Update CLAUDE.md as part of every commit that changes architecture, adds a system, renames constants, fixes a gotcha, or reveals a lesson. Don't batch it up — update it while the context is fresh.

## Build & deploy
```bash
cargo build          # native dev build (quick sanity check; silent — audio is wasm-only)
cargo test           # unit tests for the deterministic world functions
```
Deploy is automatic: any push to `main` triggers `.github/workflows/deploy.yml` which builds the WASM target and publishes to GitHub Pages. Build takes ~5–10 minutes.

### Deploy pipeline & PR previews
The published site lives on the **`gh-pages` state branch**: the `main` build at
the root, one **per-PR preview** in `pr-<n>/` (served at
`https://<owner>.github.io/pegasus/pr-<n>/` — works because every asset URL
in `index.html`/`manifest.json` is relative). Four workflows, sharing two
composite actions (`.github/actions/build-site` = wasm build + icons + overlay
injection; `.github/actions/sync-pages-branch` = commit into `gh-pages` with a
push-retry loop for concurrent deploys):
- `deploy.yml` (**Main deploy**, push to `main`): build → sync branch **root**,
  `--exclude 'pr-*'` so live previews survive a main deploy.
- `preview-deploy.yml` (**Preview deploy**, PR opened/synchronize/reopened):
  build (overlay revision = `<head-sha>-pr-<n>`) → sync `pr-<n>/` → sticky PR
  comment (`<!-- preview-env -->` marker) with the preview URL. Skipped for
  fork PRs (read-only token).
- `preview-teardown.yml` (**Preview teardown**, PR closed): delete `pr-<n>/`,
  comment.
- `publish-pages.yml` (**Publish Pages**): the *only* workflow that calls
  `deploy-pages`. Triggered by `workflow_run` on the three above (must match
  their `name:` strings exactly) and snapshots the whole `gh-pages` branch.
  **Gotcha**: the auto-created `github-pages` environment only allows
  deployments from `main`, so PR-triggered workflows can't deploy directly;
  `workflow_run` workflows execute from the default branch, which passes the
  protection. Also: pushes made with `GITHUB_TOKEN` don't trigger `push`
  workflows (recursion guard), so an `on: push: branches: [gh-pages]` publisher
  would never fire — `workflow_run` is load-bearing, not a style choice.
  Keep **Settings → Pages → Source = "GitHub Actions"** (do *not* switch it to
  the `gh-pages` branch — that would bypass this pipeline and serve the branch
  with Jekyll defaults). The Pages API intermittently rejects deployments
  created in rapid succession ("Deployment failed, try again later." — seen
  live when 4 preview deploys landed within a minute), so the deploy step
  retries once after 30 s; a red publish run self-heals on the next branch
  event regardless.

## Project structure
- `src/main.rs` — input exports/atomics, window conf, the frame loop (input gathering + stick gating, camera, drawing, HUD, minimap, crash dialog/replay/ghost cosmetics), and unit tests
- `src/sim.rs` — **the deterministic simulation core**: `Sim` owns all Rapier state, the sliding collider windows (BTreeMaps) and ship systems (fuel/hull/score/landing/crash), advanced ONLY by `tick(InputState) -> TickReport` at `PHYSICS_DT`; plus `resim(&Recording)` and all physics constants. Same inputs + same start keyframe → bit-identical trajectory (unit-tested). **Any new gameplay force/effect must go through `tick`** — frame-level physics mutation would break replay determinism.
- `src/world.rs` — deterministic world generation: cave curves, shafts, obstacles, pads, `stand_y`, `Rng`/`hash_u32`, and their constants (`SEG_LEN`, `RESET_X`, `PERIOD`, `V_PERIOD`, …)
- `src/render.rs` — radial light shader sources, faceted wall/shaft lattice (`lattice_point`, `shaft_lattice`, `facet_shade`), `draw_flat_mesh`
- `src/ship_mesh.rs` — `SHIP_TRIS` / `SHIP_DETAILS` data tables extracted from the Flash SWF
- `src/audio.rs` — in-memory WAV synthesis (`wav_from_samples`, `thruster_wav`, `boom_wav`)
- `index.html` — web wrapper, safe-area insets, settings checkboxes, **info overlay**, **gamepad polling**, and a **boot guard** (touch/stick input moved in-canvas — no touch handlers here any more): a small standalone `<script>` tag ahead of the bundle (script tags parse independently, so no error in the bundle/main script can kill it) that paints any script error on screen with file:line and offers a tap-to-reload if `wasm_exports` is missing 8 s after load. Keep it first and self-contained. It also wraps `console.error` (installed ahead of the bundle, so the wasm `console_error` import routes through it) and appends the last logged error to the banner when the error event is anonymous or attributed to the `.wasm` file — **a Rust panic reaches JS as an opaque trap** (`RuntimeError: unreachable`; iOS Safari mutes it further to a bare "Script error." with no filename, because wasm frames fail its same-origin check), and the only useful description is the panic-hook line logged just before the trap (`src/main.rs` installs `std::panic::set_hook` → `error!("{}", info)`; the *default* hook prints the useless Debug form `PanicHookInfo { payload: Any { .. }, … }`). Unhandled promise rejections get the same banner (skipped when `reason` is null). **Fully-anonymous errors (no filename AND no console.error trace) are deliberately ignored**: same-origin scripts always carry file:line and a wasm panic always logs via the hook first, so the only things that land there are Safari-injected third-party scripts — reproduced live on iOS: opening the **share sheet** runs share/action extensions' preprocessing JS in the page, and an error in any of them arrives as a muted "Script error." (this was the mystery banner of 2026-07-06, seen right after the Pegasus rename and initially blamed on it).
- `mq_js_bundle.js` — **vendored** miniquad/quad-snd JS loader (from not-fl3/miniquad-samples). Pinned in-repo so deploys don't depend on a third-party host; includes the audio backend. Update it deliberately if macroquad is upgraded. **Gotcha**: it declares globals at top level (`const canvas`, `var gl`, `wasm_exports`, `function load`, …) that share the page's global scope — redeclaring any of them in `index.html`'s inline script is a SyntaxError that silently kills the *whole* inline script (no `load()` → no wasm, page shows only the HTML chrome). Pick distinct names and check the bundle before adding top-level identifiers.

## Input sources
**Control-feel tuning**: every feel knob (PD gains, thrust gates, stick
geometry, TWR, damping), its effects, and preset recipes are documented in
`docs/control-tuning.md` — update it in the same commit as any knob change.
It also sketches the plumbing for the planned settings/controller-picker
pane (localStorage → wasm exports → atomics, with three working examples).

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
  - **Floating**: a touch below `STICK_ZONE = 0.55` of the viewport height
    (only while flying) spawns the stick centred under the finger and claims
    that touch id; other fresh touches become `ui_tap`s for the crash-dialog
    / replay-skip hit-testing. Release parks the stick bottom-right as a
    translucent ghost. Positions are physical px (`touches()` and
    `screen_*()` share that space; a mouse press maps in via `× dpi`).
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

## Info overlay (web only)
A fixed top-right "i" button (`#info-btn`) opens a fullscreen `#info-overlay`
(HTML/CSS, not drawn in-canvas — so it stays crisp/readable at any size). It
shows the build's **git revision** and **build time**. Both are injected at
deploy time: `index.html` ships with the literal placeholders `__GIT_REVISION__`
and `__BUILD_TIME__`, and the `Assemble site` step in
`.github/workflows/deploy.yml` runs `sed` to replace them with
`$(git rev-parse --short HEAD)` and `$(date -u +"%Y-%m-%d %H:%M UTC")`
respectively. Opened locally without that substitution, the JS falls back to
"dev (local build)" for each (detected via `startsWith("__")`). Button/overlay handlers `stopPropagation` on `mousedown` so a desktop
click doesn't bleed through to the canvas and fire the thruster.

The overlay also hosts the settings checkboxes — **Velocity vector**
(`#vel-toggle-row`) and **Invert stick** (`#inv-toggle-row`); both
`stopPropagation` but *no* `preventDefault`, which
would kill the checkbox click — and a **⟳ Reload latest build** button (`#force-reload`) —
the manual cache-bypass: same `?fresh=<ts>` navigation as the toast below,
for when you don't want to wait for detection.

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
and the toast swallows `mousedown` like the info button so it can't fire the
thruster.

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
- **Velocity vector** (opt-in, **off by default**): an arrow drawn from the ship along its momentum, length grows with speed, color = green ≤ 1 m/s (landable) / amber ≤ `CRASH_DV_SOFT` (damage-free touch) / red above (damaging); hidden under 0.25 m/s and while crashed. Toggled by the "Velocity vector" checkbox in the info overlay → exported `set_show_velocity(i32)` → `SHOW_VEL` atomic; the choice persists per device in `localStorage` (`pegasus_show_vel`) and is re-applied once the WASM exports load. The HUD line always appends `v=…` in the same danger color regardless of the toggle.
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
- `lattice_point(col, row, side)` → world `Vec2` (layer-0 space; the draw loop
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
just grows; nothing teleports. HUD shows the current layer as `lvl=N`
(`ship_layer = round(cam_y / V_PERIOD)`).

### Placement (all pure functions, `src/world.rs`)
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
The macroquad `audio` feature is **wasm-only** (`[target.'cfg(target_arch =
"wasm32")'.dependencies]` in Cargo.toml) because quad-snd needs ALSA to link on
native Linux; native builds get macroquad's dummy backend (same API, silent),
so `cargo build`/`cargo test` need no system packages. Browsers unmute the
AudioContext on the first user gesture (handled by the miniquad JS bundle).

## Fuel

`FUEL_MAX = 100`; the main engine burns `FUEL_BURN_MAIN = 3.5/s` at **full
throttle** (~28 s of continuous thrust; partial throttle burns proportionally),
RCS burns `FUEL_BURN_RCS = 1.2/s`. `thrusting_now` and the
RCS gates (`rcs_ok`) require `fuel > 0` — an empty tank kills engine, RCS,
particles and glow, and shows "OUT OF FUEL — [R] RESET" (reset and respawn
refill). HUD: slim gauge bar directly under the minimap (green > 50%, amber
> 25%, red below), with the **hull gauge** in a matching bar just beneath it;
the HUD text line sits at the `252*ui` baseline to clear both.

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
restored by reset/respawn. HUD: a second slim gauge bar (blue-grey → amber →
red) directly under the fuel bar.

On destruction: 70 explosion particles (`kind 3`, ~1.1 s life), the wreck is
parked (`set_gravity_scale(0)`, velocities zeroed) so the camera holds still,
input is dead (`crashed` gates thrust/RCS and ship rendering), and a "CRASHED"
banner shows. After `CRASH_DIALOG_DELAY = 1.5 s` the **crash dialog** takes
over (see below); respawn happens from its FLY AGAIN action (or the R key,
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
- **Crash dialog**: dimmed backdrop, in-canvas buttons **FLY AGAIN [R]** /
  **WATCH REPLAY [ENTER]** drawn at `sh*0.36`. Hit-testing uses `ui_tap` (a
  fresh touch OR a mouse press, physical px) — the in-canvas stick only
  claims touches *while flying*, so during the dialog the whole screen is
  tappable (the old "keep buttons above the lower 45%" rule is gone with the
  JS stick handler). FLY AGAIN sets `do_reset`, consumed by the same reset
  block as the R key. WATCH REPLAY is a no-op unless the recording has ticks.
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
  binary, unit-tested). The **stick is drawn at its normal parked home**
  (`stick_park`, bottom-right) animated by the input driving the re-sim —
  knob at the recorded deflection, amber while held — so a replay shows the
  pilot's hand where the live stick sits. (A throttle meter for both live
  play and replay is a follow-up, see #67.) The destroying impact is
  re-simulated, ends the playback (boom +
  dialog); a `ui_tap` skips back to the dialog, R skips straight to respawn.
  WATCH REPLAY is a no-op if the recording has no ticks.
  `ResimPlayer::step_one` is the shared per-tick core: `advance()` drives it
  on the wall clock for WATCH REPLAY; the ghost calls it in lockstep.

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
  (pose, velocities, fuel, hull, glow) for future drift detection / seeking /
  fallback playback, plus a terminal keyframe at the impact (`finalize`).
- **Header**: `SimParams` — every physics constant, built by `sim_params()`
  from the (now module-level) consts `GRAVITY_Y`, `THRUST_FORCE`,
  `LINEAR_DAMPING`, `ANGULAR_DAMPING`, `RCS_FORCE`, PD gains, fuel/crash/hull
  numbers — and a **build id**: index.html parses the first 8 hex chars of
  the deploy revision to a u32 and pushes it via the `set_build_id` export
  (0 = local dev). Bump `REPLAY_FORMAT_VERSION` when the layout changes.
- Trimming keeps the retained window **starting at a keyframe** with the
  effective input re-seeded there, so it stays replayable after the cap.
  The window is `HYBRID_MAX_SECS = 60 min` (~1 MB/h worst case) — a memory
  safety net, not an expected limit: a ghost needs the run from its spawn,
  so the recording must not lose t = 0 on normal-length runs. (A trimmed
  recording's ghost appears once the live run reaches its first keyframe.)
- On destruction the blob is serialized (+ `compress` = `miniz_oxide` deflate,
  the repo's only new dependency) and the WATCH REPLAY button hint shows both
  sizes: `[ENTER] · <raw> → <deflated>`.
- `sim::resim(&Recording)` re-runs the events through a fresh `Sim` and
  reproduces the recorded keyframes **bit-exactly** (unit test
  `resim_reproduces_a_scripted_flight_bit_exactly`; `glow` is render-side
  and excluded). Guarantee is per-binary — a build/params change is what the
  header fields + keyframe fallback are for. `resim` (the batch form of
  `ResimPlayer`) and `Recording::deserialize` are `#[allow(dead_code)]`
  until blobs leave the device (server-side verification).

### Determinism rules (the re-sim refactor, 2026-07)
Live play and resim must perform IDENTICAL operation sequences:
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

### Ghost of the last run (re-sim driven)
On reset the ended run's `Recording` (if ≥ `GHOST_MIN_SECS = 2 s` of ticks)
becomes `ghost_rec`, and a second `ResimPlayer` re-simulates it in
**LOCKSTEP** with live play: exactly one `step_one` per live `sim.tick`
(gated `p.tick < recorder.ticks()`), so both ships fly the same spawn clock
and stay in sync through pauses (dialog/replay freeze live ticks → the
ghost freezes too). The ghost renders as a translucent hull silhouette
(`SHIP_TRIS`, no flame/details) at `lerped_pose(alpha)` with the live
interpolation alpha, plus a pale-blue minimap dot; it vanishes at its own
crash tick (`finished`). Hidden while the current ship is a wreck. Cost:
one extra `Sim::tick` per physics step during flight (~2× physics, still
tiny next to rendering; the ghost sim maintains its own collider windows
around the ghost's position).

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

## Git workflow
- **Always open a PR** after pushing a feature branch — standing instruction
  from the owner (no need to ask first). The PR also produces a phone-testable
  preview deployment at `pr-<n>/`.
- Development branch: `claude/crash-replay-dialog-qmvtn8` (current); previous: `claude/replicate-pr-review-deployment-gjw4x0`
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
