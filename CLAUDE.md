# rapier-test — Cave Flyer

Rust + macroquad 0.4.15 + Rapier 2D cave-flying game compiled to WebAssembly and served via GitHub Pages. The player pilots a ship through a procedurally-generated scrolling cave using thrust and rotation controls.

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

## Rendering architecture
- **World-to-screen**: a per-frame closure `w2s` (defined inside the `loop {}`, shadows the removed module-level function) converts world coords to screen pixels using `view_scale`.
- **`view_scale`**: `SCALE * 0.38` on narrow screens (`sw < 600px`, i.e. mobile portrait), `SCALE` on desktop. Controls zoom; HUD/minimap are unaffected.
- **Cave walls**: drawn as `draw_mesh` calls (3 stacked quads per segment: edge band, mid band, fill-to-infinity). Use raw base rock colors — lighting is entirely in the fragment shader.
- **Radial light shader** (`LIGHT_VERTEX` / `LIGHT_FRAGMENT` constants): a custom macroquad `Material` active only during the cave-wall draw loop (`gl_use_material` / `gl_use_default_material`). Computes per-pixel radial falloff from the ship's screen position. Uniforms set each frame: `ship_pos` (vec2), `light_radius` (float), `glow` (float).
- **Shader math**: `ambient = 0.45`, quadratic falloff `t*t`, warm orange tint `glow * falloff * 0.28` added to red (×1.0) and green (×0.4). `light_radius = min(sw,sh) * 0.55 + glow * min(sw,sh) * 0.30`.
- Stars, particles, ship, HUD text, and minimap all use the default macroquad material — the radial shader does not affect them.

## Rock colors (base, pre-lighting)
```rust
rock_dark = Color::from_rgba(80,  64,  50,  255)
rock_mid  = Color::from_rgba(118, 95,  72,  255)
rock_edge = Color::from_rgba(150, 120, 88,  255)
```
These were deliberately lightened (from the original ~35/60/90) so walls are clearly visible at ambient lighting without relying on a high ambient floor.

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

## Git workflow
- Development branch: `claude/radial-light-thrust-shader-uuqt21`
- Merges to `main` via squash PRs using the GitHub MCP tools (`mcp__github__create_pull_request`, `mcp__github__merge_pull_request`).
- Branch consistently diverges from main after merges — always `git fetch origin main && git rebase origin/main && git push --force-with-lease` before creating a PR to avoid merge conflicts.
