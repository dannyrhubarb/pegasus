# rapier-test — Cave Flyer

Rust + macroquad 0.4.15 + Rapier 2D cave-flying game compiled to WebAssembly and served via GitHub Pages. The player pilots a ship through a procedurally-generated scrolling cave using thrust and rotation controls.

> **Keep this file current.** Update CLAUDE.md as part of every commit that changes architecture, adds a system, renames constants, fixes a gotcha, or reveals a lesson. Don't batch it up — update it while the context is fresh.

## Build & deploy
```bash
cargo build          # native dev build (quick sanity check)
```
Deploy is automatic: any push to `main` triggers the GitHub Actions workflow `.github/workflows/deploy.yml` which builds the WASM target and publishes to GitHub Pages. Build takes ~5–10 minutes.

## Project structure
- `src/main.rs` — entire game (single file): physics, rendering, cave generation, HUD, minimap, touch controls
- `index.html` — web wrapper, touch event forwarding, safe-area insets

## Key constants & configuration (`src/main.rs`)

| Symbol | Value | Purpose |
|--------|-------|---------|
| `SCALE` | 80.0 | World-to-pixel ratio (physics/world units only — do **not** use for rendering) |
| `SEG_LEN` | 3.0 | Cave segment length in world units |
| `HALF_WINDOW` | 80 | Segments loaded each side of ship |
| `PERIOD` | 600.0 | Cave repeat period in world units |
| `SHIP_SCALE` | 1.5 | Render scale multiplier applied inside the `rot` closure — makes the ship visually 1.5× larger than the raw SWF coordinates without touching `SHIP_TRIS`/`SHIP_DETAILS` |

## Rendering architecture
- **World-to-screen**: a per-frame closure `w2s` (defined inside the `loop {}`, shadows the removed module-level function) converts world coords to screen pixels using `view_scale`.
- **`view_scale`**: `SCALE * 0.38` on narrow screens (`sw < 600px`, i.e. mobile portrait), `SCALE` on desktop. Controls zoom; HUD/minimap are unaffected.
- **Cave walls**: drawn as **low-poly faceted** triangle meshes — two `draw_mesh` calls per frame (one ceiling, one floor), each a continuous lattice of flat-shaded triangles. See "Faceted wall rendering" below. Per-facet base colors carry deterministic brightness jitter; radial lighting is added on top by the fragment shader.
- **Radial light shader** (`LIGHT_VERTEX` / `LIGHT_FRAGMENT` constants): a custom macroquad `Material` active only during the cave-wall and obstacle draws (`gl_use_material` / `gl_use_default_material`). Computes per-pixel radial falloff from the ship's screen position. Uniforms set each frame: `ship_pos` (vec2), `light_radius` (float), `glow` (float).
- **Shader math**: `ambient = 0.45`, quadratic falloff `t*t`, *subtle* warm orange tint `glow * falloff * 0.12` added to red (×1.0) and green (×0.4) — kept low so the cool slate rock stays blue with only a faint thruster flush. `light_radius = min(sw,sh) * 0.55 + glow * min(sw,sh) * 0.30`.
- Stars, particles, ship, HUD text, and minimap all use the default macroquad material — the radial shader does not affect them.
- **Ship rendering**: the hull is the const `SHIP_TRIS` — 41 triangles **extracted from the original Flash ship** (see below) — drawn in local ship space (`+Y` = nose/forward, origin = hull centroid, full height ≈ 0.95 world units). Each facet's silver brightness is derived from its centroid height (nose lit → base shaded). On top, `SHIP_DETAILS` (`[ax,ay,bx,by,cx,cy,r,g,b]`) layers the real sub-shapes — window dome, two darker leg-pods, central engine cup + light insert, and a small gold accent — each with its own extracted colour, plus an **added** blue accent (cockpit glass + two flank racing stripes; the original SWF lander is plain silver, verified by parsing every fill incl. mid-shape style changes and gradient stops). A two-triangle orange/yellow thruster flame (scaled by `glow`, hidden when `glow ≤ 0.02`, drawn first so it sits behind the hull) completes it. All geometry goes through the `rot(lx, ly)` closure which applies `SHIP_SCALE` before calling `w2s` — so the raw SWF coordinates are unchanged, only rendered at 1.5× size.
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
- `SUBCOLS = 3` sub-columns per 3 m segment → ~1 m facets; `COL_DX = SEG_LEN/SUBCOLS`.
- `col_x(col)` — world x of a **global** facet column. *Pure* function of the
  global column index, so adjacent segments compute their shared boundary vertex
  identically → **no seams/cracks**. The visible column range comes from the
  `cave` deque (`col_lo = front.idx*SUBCOLS`, `col_hi = (back.idx+1)*SUBCOLS`).
- `ROW_DEPTHS = [0.0, 0.45, 1.1, 2.2]` m into the rock; `N_ROWS = 4`.
- `lattice_point(col, row, side)` → world `Vec2`. **Row 0 is exactly on the wall
  edge with ZERO jitter** (collider-aligned — the hard rule below); deeper rows
  recede into the rock (ceiling = +y, floor = −y) with small deterministic jitter
  (`hash_u32` of col/row/side; ±0.25 m in x, depth-scaled in y).
- `facet_shade(base, col, row, side, salt)` → band base color (`row 0→rock_edge`,
  `1→rock_mid`, else `rock_dark`) × deterministic brightness in ~[0.82, 1.12].

### Per-column emission (in the draw loop)
For each visible column (x-culled vs `margin`): for each of the `N_ROWS-1` cells,
take the 4 corner `lattice_point`s → `w2s` → **2 flat-shaded triangles**, each its
own shade (two `salt`s per cell). The cell diagonal is chosen by
`hash_u32(col ^ row*…) & 1` so the lattice doesn't read as a regular grid. After
the rows, a solid `rock_dark` quad (2 tris) fills from the deepest row out to
`far_up`/`far_down`.

**Collider-alignment rule (unchanged):** the lit row-0 surface must coincide with
the Rapier segment collider. Only rows > 0 (inside the rock) may be jittered.
`w2s` inverts Y, so "into the rock" is screen-Y − for the ceiling and screen-Y +
for the floor; jitter always pushes *away* from the cave interior.

## Polygon obstacle system

Random convex-polygon boulders are placed deterministically along the cave so they load/unload with the same sliding window as the walls and are identical every time the player revisits a location.

### Generation
- `OBSTACLE_SPACING = 16.0 m` between slots. Each slot `k` maps to a fixed world-x position plus ±3 m jitter.
- A tiny integer-hash PRNG (`Rng` struct, seeded by slot index) drives all randomness: position jitter, size, rotation, vertex count, vertex radii.
- Slot is skipped if: `cx.abs() < 9.0` (spawn-clear zone), `hw < 4.5` (pinch point), or 1-in-6 random empty.
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
Obstacles are drawn on the minimap as their actual polygon shape (triangle fan + outline) projected into minimap space, not as dots.

### Storage
`HashMap<i64, Obstacle>` keyed by slot index. Load/evict each frame in sync with the wall window (`k_left` / `k_right` derived from `want_left` / `want_right`).

## Color / rendering alignment rule

**The visible rock surface must coincide with the Rapier collider line.** For
walls this means lattice **row 0 carries zero jitter** and is sampled directly
on the wall edge; only deeper rows (inside the rock) are displaced. For obstacles
the outer `poly` ring stays the exact hull. All facet displacement goes *into the
rock* (away from the cave interior), never into the cave — otherwise the visible
surface pokes past the collider and the ship appears to sink into the rock.

## Physics notes

The ship uses a **compound collider** of three **capsules** (stadium shapes) parented to the same rigid body, tracing the lander silhouette of the 1.5× scaled visual. Capsules are the closest primitive Rapier offers to an ellipse — they hug the rounded hull tighter than boxes and slide off rocks without corners catching. Endpoints are in scaled world units (ship-local frame):
- **Fuselage**: `capsule((0, +0.42), (0, −0.08), r=0.26)` — rounded nose down to mid-hull.
- **Left leg pod**: `capsule((−0.26, −0.30), (−0.33, −0.64), r=0.09)` — angled out to the foot.
- **Right leg pod**: `capsule((+0.26, −0.30), (+0.33, −0.64), r=0.09)` — mirror.

Each is built `ColliderBuilder::new(SharedShape::capsule(a, b, r)).restitution(0.2)` (`SharedShape`, `point!` from `rapier2d::prelude::*`). Rapier 2D has **no ellipse primitive** — capsule is the smooth-rounded alternative; for an even tighter (but faceted) fit you could use `convex_hull` of the `SHIP_TRIS` vertices, at the cost of filling the concave notch between the feet. Cave walls are `segment` colliders (zero thickness). The ship (max ~17 m/s under normal thrust) never tunnels through walls — CCD is not necessary.

**RCS / attitude thrusters** (cosmetic particles, `kind 1/2`): a nose-mounted nozzle vents sideways to swing the ship. Turning **left** → right nozzle at scaled-local `(0.27, 0.20)` fires gas out `+X`; turning **right** → left nozzle at `(−0.27, 0.20)` fires gas out `−X`. Emission coords are in **scaled world units** — `lp()`/`ld()` do **not** apply `SHIP_SCALE` (only the render-time `rot` closure does), so don't multiply these by `SHIP_SCALE` (an earlier bug double-scaled them to ±0.60 and spawned the puffs outside the hull).

## Git workflow
- Development branch: `claude/vector-spaceship-extraction-njnuoq` (current); previous: `claude/walls-obstacles-appearance-qj1rpp`
- Merges to `main` via rebase PRs using the GitHub MCP tools (`mcp__github__create_pull_request`, `mcp__github__merge_pull_request`).
- Branch consistently diverges from main after merges — always `git fetch origin main && git rebase origin/main && git push --force-with-lease` before creating a PR to avoid merge conflicts.
- The wasm binary (`rapier-test.wasm`) conflicts on every rebase — always resolve by rebuilding from source: `cargo build --release --target wasm32-unknown-unknown && cp target/wasm32-unknown-unknown/release/rapier-test.wasm rapier-test.wasm`, then `git add rapier-test.wasm` before `git rebase --continue`.
