// Rendering support: the radial light shader, the low-poly faceted lattice
// for cave and shaft walls, and the flat-shaded-mesh helper. The lattice is a
// pure function of GLOBAL column indices (no seams) and row 0 must stay
// exactly on the collider line — see CLAUDE.md "Faceted wall rendering".

use pegasus_sim::world::*;
use macroquad::prelude::*;

// Vertex shader: passes screen-pixel position as a varying so the
// fragment shader can compute per-pixel distance from the ship.
pub const LIGHT_VERTEX: &str = r#"#version 100
attribute vec3 position;
attribute vec2 texcoord;
attribute vec4 color0;

varying lowp vec2 uv;
varying lowp vec4 color;
varying highp vec2 frag_pos;

uniform mat4 Model;
uniform mat4 Projection;

void main() {
    gl_Position = Projection * Model * vec4(position, 1);
    color = color0 / 255.0;
    uv = texcoord;
    frag_pos = position.xy;
}"#;

// Fragment shader: true per-pixel radial falloff from the ship.
// Eliminates the vertical "column" that Gouraud shading produces over
// the large fill quads.
pub const LIGHT_FRAGMENT: &str = r#"#version 100
precision highp float;

varying vec2 uv;
varying vec4 color;
varying vec2 frag_pos;

uniform sampler2D Texture;
uniform vec2  ship_pos;
uniform float light_radius;
uniform float glow;

void main() {
    float dist    = distance(frag_pos, ship_pos);
    float t       = clamp(1.0 - dist / light_radius, 0.0, 1.0);
    float falloff = t * t;
    float ambient = 0.45;
    float l       = min(ambient + (1.0 - ambient) * falloff, 1.0);
    float warm    = glow * falloff * 0.12;

    vec4 base = color * texture2D(Texture, uv);
    gl_FragColor = vec4(
        min(base.r * l + warm,       1.0),
        min(base.g * l + warm * 0.4, 1.0),
        min(base.b * l,              1.0),
        1.0);
}"#;

// --- Low-poly faceted wall lattice ---------------------------------------
// The cave walls are rendered as a grid of flat-shaded triangles ("facets").
// Geometry is a pure function of a GLOBAL column index so the shared boundary
// between adjacent segments is computed identically on both sides — no cracks.

pub const SUBCOLS: i64 = 2;                       // sub-columns per 3 m segment → ~1.5 m facets
pub const COL_DX: f32 = SEG_LEN / SUBCOLS as f32; // world width of one facet column
pub const ROW_DEPTHS: [f32; 4] = [0.0, 1.0, 3.0, 6.5]; // metres into rock; row 0 on the edge
pub const N_ROWS: usize = 4;

// World x for a global facet column. Pure function → identical on both sides
// of any segment boundary, so adjacent strips share an exact x.
pub fn col_x(col: i64) -> f32 {
    col as f32 * COL_DX
}

// World-space lattice point for (col, row, side). Row 0 sits EXACTLY on the
// wall edge (collider-aligned, no jitter); deeper rows recede into the rock
// with small deterministic jitter for the faceted look.
// side 0 = ceiling (rock is +y), side 1 = floor (rock is -y).
pub fn lattice_point(level: &Level, col: i64, row: usize, side: u8) -> Vec2 {
    let x = col_x(col);
    let edge_y = if side == 0 {
        level.cave_center(x) + level.cave_half_width(x)
    } else {
        level.cave_center(x) - level.cave_half_width(x)
    };
    if row == 0 {
        return vec2(x, edge_y); // locked to the collider line
    }
    let depth = ROW_DEPTHS[row];
    let dir = if side == 0 { 1.0 } else { -1.0 };
    let h = hash_u32(
        (col as u32).wrapping_mul(73856093)
            ^ (row as u32).wrapping_mul(19349663)
            ^ (side as u32).wrapping_mul(83492791),
    );
    let jx = ((h & 0xffff) as f32 / 65535.0 - 0.5) * (COL_DX * 0.5); // ±0.25 m
    let jy = (((h >> 16) & 0xffff) as f32 / 65535.0 - 0.5) * (depth * 0.35);
    vec2(x + jx, edge_y + dir * (depth + jy))
}

// Draw a flat-shaded triangle soup (sequential indices, no shared vertices).
// Indices are u16, so a single mesh must stay under 65 536 vertices — the
// debug_assert catches it loudly if facet density ever grows past that.
pub fn draw_flat_mesh(vertices: Vec<Vertex>) {
    if vertices.is_empty() {
        return;
    }
    debug_assert!(vertices.len() <= u16::MAX as usize, "mesh exceeds u16 index range");
    let indices: Vec<u16> = (0..vertices.len() as u16).collect();
    draw_mesh(&Mesh { vertices, indices, texture: None });
}

// Flat-shade color for a wall facet: a band base color (by row) modulated by a
// deterministic per-facet brightness so each triangle reads as a distinct facet.
pub fn facet_shade(base: Color, col: i64, row: usize, side: u8, salt: u32) -> Color {
    let h = hash_u32(
        (col as u32).wrapping_mul(2246822519)
            ^ (row as u32).wrapping_mul(3266489917)
            ^ (side as u32)
            ^ salt,
    );
    // Wider contrast on deeper (darker) rows so facets stay readable in shadow.
    let (lo, hi) = match row { 0 => (0.82, 1.12), 1 => (0.65, 1.25), _ => (0.45, 1.40) };
    let b = lo + (h & 0xffff) as f32 / 65535.0 * (hi - lo);
    Color::new(
        (base.r * b).min(1.0),
        (base.g * b).min(1.0),
        (base.b * b).min(1.0),
        1.0,
    )
}

// Facet lattice point for shaft walls (vertical analogue of lattice_point):
// depth col 0 sits exactly on the wall polyline (collider-aligned); deeper
// cols recede horizontally into the rock with deterministic jitter. Near the
// two ends the deep cols are additionally pulled along the shaft toward the
// junction rock, so corner facets turn diagonally into the corner wedge
// instead of poking past the cave wall line into the cave interior.
pub fn shaft_lattice(pts: &[Vec2], s: i64, i: usize, d: usize, side: u8) -> Vec2 {
    let p = pts[i];
    if d == 0 {
        return p;
    }
    let depth = ROW_DEPTHS[d];
    let dir = if side == 0 { -1.0 } else { 1.0 };
    let h = hash_u32(
        (s as u32).wrapping_mul(0x9e37_79b9)
            ^ (i as u32).wrapping_mul(73856093)
            ^ (d as u32).wrapping_mul(19349663)
            ^ (side as u32 + 7).wrapping_mul(83492791),
    );
    let jy = ((h & 0xffff) as f32 / 65535.0 - 0.5) * (SHAFT_STEP * 0.5);
    let jx = (((h >> 16) & 0xffff) as f32 / 65535.0 - 0.5) * (depth * 0.35);
    let e = i.min(pts.len() - 1 - i) as f32;
    let end_pull = (1.0 - e / 3.0).max(0.0) * depth;
    let end_dir = if i * 2 < pts.len() { 1.0 } else { -1.0 }; // up at bottom end, down at top
    vec2(p.x + dir * (depth + jx), p.y + jy + end_dir * end_pull)
}
