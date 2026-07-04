# Pegasus — Moon Lander

Rust + macroquad 0.4.15 + Rapier 2D game compiled to WebAssembly and served via GitHub Pages. The player pilots a ship through a procedurally-generated scrolling cave using thrust and rotation controls.

> **Keep this file current.** Update CLAUDE.md as part of every commit that changes architecture, adds a system, renames constants, fixes a gotcha, or reveals a lesson. Don't batch it up — update it while the context is fresh.

## Build & deploy
```bash
cargo build          # native dev build (quick sanity check)
```
Deploy is automatic: any push to `main` triggers the GitHub Actions workflow `.github/workflows/deploy.yml` which builds the WASM target and publishes to GitHub Pages. Build takes ~5–10 minutes.

## Project structure
- `src/main.rs` — entire game (single file): physics, rendering, cave generation, HUD, minimap, touch controls
- `index.html` — web wrapper, touch event forwarding, safe-area insets, **info overlay**, **gamepad polling**

## Input sources
Four input paths feed the same physics, combined in the main loop:
- **Keyboard** (desktop): `Down` thrust, `Left`/`Right` rotate, `R` reset.
- **Mouse**: left-button held = thrust.
- **Touch** (mobile): on-screen THRUST button + analog torque slider, forwarded
  from `index.html` via exported `set_touch_thrust(i32)` / `set_touch_torque(f32)`.
- **Game controller** (BT/USB, web): `index.html` polls the **Web Gamepad API**
  each `requestAnimationFrame` and forwards to exported `set_pad_thrust(i32)` /
  `set_pad_torque(f32)` / `set_pad_reset()`. Mapping (standard layout): thrust =
  A/Cross (0), R2 (7, analog>0.3), or D-pad up (12); steer = left stick X
  (axes[0], dead-zoned/rescaled) or D-pad L/R (14/15); reset = Start (9) or
  Y/Triangle (3, edge-triggered). Polling starts on `gamepadconnected` and stops
  (releasing held inputs) if the pad drops out.

Touch and gamepad use **separate atomics** (`TOUCH_*` vs `PAD_*`) so a
connected-but-idle source never stomps the other; thrust is OR'd and analog
torque is summed-then-clamped to ±1. `PAD_RESET` is a swap-to-consume flag so a
held reset button fires exactly once.

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

## Key constants & configuration (`src/main.rs`)

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
- **World-to-screen**: a per-frame closure `w2s` (defined inside the `loop {}`, shadows the removed module-level function) converts world coords to screen pixels using `view_scale`.
- **`view_scale`**: `SCALE * 0.38` on small screens (`sw.min(sh) < 600px`, i.e. mobile in either orientation), `SCALE` on desktop. Controls zoom; HUD/minimap are unaffected. Keyed on the *smaller* dimension (not `sw`) so a phone keeps the same zoom across portrait/landscape — using `sw` alone made landscape flip to desktop zoom and appear zoomed-in.
- **Cave walls**: drawn as **low-poly faceted** triangle meshes — one `draw_mesh` per (layer, side) for the up-to-3 loaded cave layers (y-culled), plus one per loaded shaft wall. Each mesh is a continuous lattice of flat-shaded triangles. See "Faceted wall rendering" and "Vertical shafts" below. Per-facet base colors carry deterministic brightness jitter; radial lighting is added on top by the fragment shader.
- **Radial light shader** (`LIGHT_VERTEX` / `LIGHT_FRAGMENT` constants): a custom macroquad `Material` active only during the cave-wall and obstacle draws (`gl_use_material` / `gl_use_default_material`). Computes per-pixel radial falloff from the ship's screen position. Uniforms set each frame: `ship_pos` (vec2), `light_radius` (float), `glow` (float).
- **Shader math**: `ambient = 0.45`, quadratic falloff `t*t`, *subtle* warm orange tint `glow * falloff * 0.12` added to red (×1.0) and green (×0.4) — kept low so the cool slate rock stays blue with only a faint thruster flush. `light_radius = min(sw,sh) * 0.55 + glow * min(sw,sh) * 0.30`.
- Stars, particles, ship, HUD text, and minimap all use the default macroquad material — the radial shader does not affect them.
- **Stars**: stored as **normalized [0,1) coords** and multiplied by the *current* `sw`/`sh` each frame (then `rem_euclid`-wrapped for parallax), so the field fills the whole viewport in any orientation. (Storing absolute pixels captured the startup size and left a gap after rotating to a wider screen.)
- **UI scale `ui`** (`(sw.min(sh) / 980.0).min(1.0)`): scales fixed-size HUD/minimap. Keyed on the *smaller* dimension so a phone keeps the same HUD/minimap size across portrait/landscape — `sw` alone grew the minimap on rotation. Capped at 1.0 so desktop is unchanged.
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
- `glow`: smoothed 0→1 float, exponentially approaches thrust input with factor 0.12 per frame.
- Thrust applies upward force along the ship's heading via Rapier `add_force`.
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

### The lattice (module-level, near `hash_u32`)
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

### Placement (all pure functions, near `insert_seg`)
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

### Loading (main loop)
- Cave segments: `HashMap<(layer, idx), Vec<ColliderHandle>>` — 2D sliding
  window, layers `ship_layer ± 1` × segments `want_left..=want_right`
  (`retain` + `entry().or_insert_with()` each frame; empty Vec = opening).
- Shafts: `HashMap<(slot, gap), Shaft>` for gaps `{ship_layer-1, ship_layer}` —
  covers everything reachable within half a period. `Shaft` stores collider
  handles + both wall polylines for rendering.
- There is no startup seeding — the first frame's physics tick runs with no
  colliders (harmless) and the window sync fills everything before drawing.

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
`HashMap<(i64, i64), Obstacle>` keyed by (slot index, layer) — every layer gets
an identical copy of each obstacle at `cy + layer * V_PERIOD` (the y-wrap).
Load/evict each frame in sync with the wall window (`k_left` / `k_right` derived
from `want_left` / `want_right`, layers `ship_layer ± 1`).

## Color / rendering alignment rule

**The visible rock surface must coincide with the Rapier collider line.** For
walls this means lattice **row 0 carries zero jitter** and is sampled directly
on the wall edge; only deeper rows (inside the rock) are displaced. For obstacles
the outer `poly` ring stays the exact hull. All facet displacement goes *into the
rock* (away from the cave interior), never into the cave — otherwise the visible
surface pokes past the collider and the ship appears to sink into the rock.

## Fuel

`FUEL_MAX = 100`; the main engine burns `FUEL_BURN_MAIN = 3.5/s` (~28 s of
continuous thrust), RCS burns `FUEL_BURN_RCS = 1.2/s`. `thrusting_now` and the
RCS gates (`rcs_ok`) require `fuel > 0` — an empty tank kills engine, RCS,
particles and glow, and shows "OUT OF FUEL — [R] RESET" (reset and respawn
refill). HUD: slim gauge bar directly under the minimap (green > 50%, amber
> 25%, red below); the HUD text line moved down to `232*ui` baseline to clear
it.

## Crash & respawn

Impacts are detected from the **frame-to-frame velocity change**: after the
physics substeps, `|v − prev_vel| > CRASH_DV (5 m/s)` means a collision impulse
(thrust/gravity change v by < 0.3 m/s per frame) — no Rapier contact-event
plumbing needed. On crash: 70 explosion particles (`kind 3`, ~1.1 s life), the
wreck is parked (`set_gravity_scale(0)`, velocities zeroed) so the camera holds
still, input is dead (`crashed` gates thrust/RCS and ship rendering), and a
"CRASHED" banner shows. After `CRASH_RESPAWN = 1.5 s` the ship respawns at
`RESET_X` (gravity restored). Anything that teleports or zeroes velocity
(reset, respawn) must also snap `prev_vel` — otherwise the velocity jump reads
as an impact — and `prev_ship` (render interpolation).

## Physics notes

**Fixed timestep**: physics steps at `PHYSICS_DT = 1/120 s` through an
accumulator in the main loop (catch-up capped at 0.05 s per frame), so handling
is identical on 60/120/144 Hz displays. Rendering interpolates the ship between
the last two physics states (`prev_ship` + `alpha = accum/PHYSICS_DT`); anything
that teleports the body (reset/respawn) must also snap `prev_ship` or the camera
lerps across the jump for a frame. Forces set in the controls section persist
across all substeps of the following frame (reset each frame via `reset_forces`).

The ship uses a **compound collider** of three **capsules** (stadium shapes) parented to the same rigid body, tracing the lander silhouette of the 1.5× scaled visual. Capsules are the closest primitive Rapier offers to an ellipse — they hug the rounded hull tighter than boxes and slide off rocks without corners catching. Endpoints are in scaled world units (ship-local frame):
- **Fuselage**: `capsule((0, +0.42), (0, −0.08), r=0.26)` — rounded nose down to mid-hull.
- **Left leg pod**: `capsule((−0.26, −0.30), (−0.33, −0.64), r=0.09)` — angled out to the foot.
- **Right leg pod**: `capsule((+0.26, −0.30), (+0.33, −0.64), r=0.09)` — mirror.

Each is built `ColliderBuilder::new(SharedShape::capsule(a, b, r)).restitution(0.2)` (`SharedShape`, `point!` from `rapier2d::prelude::*`). Rapier 2D has **no ellipse primitive** — capsule is the smooth-rounded alternative; for an even tighter (but faceted) fit you could use `convex_hull` of the `SHIP_TRIS` vertices, at the cost of filling the concave notch between the feet. Cave walls are `segment` colliders (zero thickness). The body has `ccd_enabled(true)`, which matters more now: a long free-fall down a vertical shaft can pass 50 m/s, far above the ~17 m/s of normal cave flight.

**RCS / attitude thrusters** (cosmetic particles, `kind 1/2`): bottom nozzles flanking the main booster vent **downward** (like a mini main thruster). Turning **left** → left nozzle at scaled-local `(−0.30, −0.71)`; turning **right** → right nozzle at `(0.30, −0.71)`. Gas exits `−Y` (downward) from both. The x positions sit in the leg nozzle (gold accent: unscaled x ≈ ±0.152–0.249 → midpoint ±0.30 scaled). Emission coords are in **scaled world units** — `lp()`/`ld()` do **not** apply `SHIP_SCALE` (only the render-time `rot` closure does), so don't multiply these by `SHIP_SCALE` (an earlier bug double-scaled them to ±0.60 and spawned the puffs outside the hull).

## Git workflow
- Development branch: `claude/vertical-wrapping-caves-csv79r` (current); previous: `claude/vector-spaceship-extraction-njnuoq`
- Merges to `main` via rebase PRs using the GitHub MCP tools (`mcp__github__create_pull_request`, `mcp__github__merge_pull_request`).
- Branch consistently diverges from main after merges — always `git fetch origin main && git rebase origin/main && git push --force-with-lease` before creating a PR to avoid merge conflicts.
- The wasm binary (`rapier-test.wasm`) is **not tracked** (gitignored) — deploy builds it from source, and for local play you build it into the repo root per the README. It previously lived in git and conflicted on every rebase; don't re-add it.
