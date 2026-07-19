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

// --- Hand-drawn terrain (polygon worlds) -----------------------------------

// Ear-clip triangulation for a simple polygon (concave allowed, CCW winding —
// Level::parse normalizes). Cosmetic only: the colliders are the polygon
// edges themselves, this just fills the rock for drawing (cached per level in
// the main loop, not recomputed per frame). O(n²), fine for hand-drawn maps.
pub fn triangulate(poly: &[Vec2]) -> Vec<[Vec2; 3]> {
    let n = poly.len();
    if n < 3 {
        return Vec::new();
    }
    let cross = |a: Vec2, b: Vec2, c: Vec2| (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
    let mut idx: Vec<usize> = (0..n).collect();
    let mut tris = Vec::with_capacity(n - 2);
    while idx.len() > 3 {
        let m = idx.len();
        let mut clipped = false;
        for i in 0..m {
            let (pa, pb, pc) = (poly[idx[(i + m - 1) % m]], poly[idx[i]], poly[idx[(i + 1) % m]]);
            // Convex corner (CCW) …
            if cross(pa, pb, pc) <= 1e-6 {
                continue;
            }
            // … containing no other remaining vertex = an ear.
            let ear = idx.iter().all(|&j| {
                let q = poly[j];
                q == pa || q == pb || q == pc
                    || cross(pa, pb, q) < 0.0
                    || cross(pb, pc, q) < 0.0
                    || cross(pc, pa, q) < 0.0
            });
            if ear {
                tris.push([pa, pb, pc]);
                idx.remove(i);
                clipped = true;
                break;
            }
        }
        if !clipped {
            // Degenerate input (self-touching / collinear run): fall back to
            // a fan so we always terminate — worst case some overdraw.
            for i in 1..idx.len() - 1 {
                tris.push([poly[idx[0]], poly[idx[i]], poly[idx[i + 1]]]);
            }
            return tris;
        }
    }
    tris.push([poly[idx[0]], poly[idx[1]], poly[idx[2]]]);
    tris
}

// Sutherland–Hodgman clip of a convex-or-triangle polygon against an
// axis-aligned rect. The minimap uses this to clip terrain triangles to its
// window IN WORLD SPACE — clamping the mapped vertices instead (the old
// approach) dragged far-outside vertices onto the box edge and WARPED the
// triangle's interior: invisible on axis-aligned frame slabs, but the
// editor's carve pieces and islands are full of long sliver triangles with
// distant vertices, which smeared rock across carved space (field report,
// 2026-07). Returns 0..=7 vertices; fan-triangulate the result.
pub fn clip_poly_rect(input: &[Vec2], lo: Vec2, hi: Vec2) -> Vec<Vec2> {
    let mut poly: Vec<Vec2> = input.to_vec();
    for side in 0..4 {
        if poly.is_empty() {
            break;
        }
        let inside = |p: Vec2| match side {
            0 => p.x >= lo.x,
            1 => p.x <= hi.x,
            2 => p.y >= lo.y,
            _ => p.y <= hi.y,
        };
        let mut out = Vec::with_capacity(poly.len() + 2);
        for i in 0..poly.len() {
            let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
            let (ia, ib) = (inside(a), inside(b));
            if ia {
                out.push(a);
            }
            if ia != ib {
                let t = match side {
                    0 => (lo.x - a.x) / (b.x - a.x),
                    1 => (hi.x - a.x) / (b.x - a.x),
                    2 => (lo.y - a.y) / (b.y - a.y),
                    _ => (hi.y - a.y) / (b.y - a.y),
                };
                out.push(a + (b - a) * t);
            }
        }
        poly = out;
    }
    poly
}

#[cfg(test)]
mod clip_tests {
    use super::*;

    fn in_poly(p: Vec2, poly: &[Vec2]) -> bool {
        let mut inside = false;
        for i in 0..poly.len() {
            let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
            if (a.y > p.y) != (b.y > p.y)
                && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
                inside = !inside;
            }
        }
        inside
    }
    fn in_tri(p: Vec2, t: &[Vec2; 3]) -> bool {
        let c = |a: Vec2, b: Vec2| (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
        let (d0, d1, d2) = (c(t[0], t[1]), c(t[1], t[2]), c(t[2], t[0]));
        !((d0 < 0.0 || d1 < 0.0 || d2 < 0.0) && (d0 > 0.0 || d1 > 0.0 || d2 > 0.0))
    }

    // The editor's carve pipeline splits a cut slab into C-shaped pieces
    // whose seam edges leave several collinear vertices on one line — real
    // output that broke ear clipping in the field (minimap showed rock
    // across the carved chamber). Coverage oracle: a grid point is inside
    // some triangle IFF it is inside the polygon.
    #[test]
    fn triangulation_covers_carve_pieces_exactly() {
        let pieces: Vec<Vec<Vec2>> = vec![
            vec![vec2(-2.5, 37.87), vec2(-2.5, 80.0), vec2(-80.0, 80.0),
                 vec2(-80.0, -70.0), vec2(-2.5, -70.0), vec2(-2.5, -27.92),
                 vec2(-40.0, -30.0), vec2(-60.0, 5.0), vec2(-45.0, 40.0)],
            vec![vec2(-2.5, -27.92), vec2(-2.5, -70.0), vec2(80.0, -70.0),
                 vec2(80.0, 80.0), vec2(-2.5, 80.0), vec2(-2.5, 37.87),
                 vec2(55.0, 35.0), vec2(50.0, -25.0)],
            vec![vec2(1.0, 11.94), vec2(1.0, 19.8), vec2(-10.0, 22.0),
                 vec2(-28.0, 8.0), vec2(-20.0, -5.0), vec2(1.0, -8.0),
                 vec2(1.0, -1.22), vec2(-10.0, 0.0), vec2(-5.0, 13.0)],
            vec![vec2(1.0, -1.22), vec2(1.0, -8.0), vec2(15.0, -10.0),
                 vec2(25.0, 15.0), vec2(1.0, 19.8), vec2(1.0, 11.94),
                 vec2(12.0, 10.0), vec2(8.0, -2.0)],
        ];
        for (pi, raw) in pieces.iter().enumerate() {
            // Same normalization as Level::parse: winding forced CCW.
            let mut poly = raw.clone();
            let area: f32 = (0..poly.len())
                .map(|i| {
                    let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
                    a.x * b.y - b.x * a.y
                })
                .sum();
            if area < 0.0 {
                poly.reverse();
            }
            let tris = triangulate(&poly);
            let mut bad = 0;
            for gx in 0..60 {
                for gy in 0..60 {
                    let p = vec2(
                        -82.0 + gx as f32 * (164.0 / 60.0) + 0.0137,
                        -72.0 + gy as f32 * (154.0 / 60.0) + 0.0071,
                    );
                    let want = in_poly(p, &poly);
                    let got = tris.iter().any(|t| in_tri(p, t));
                    if want != got {
                        bad += 1;
                    }
                }
            }
            assert_eq!(bad, 0, "piece {} mis-triangulated at {} grid points", pi, bad);
        }
    }

    fn area(p: &[Vec2]) -> f32 {
        let mut a = 0.0;
        for i in 0..p.len() {
            let (q, r) = (p[i], p[(i + 1) % p.len()]);
            a += q.x * r.y - r.x * q.y;
        }
        (a / 2.0).abs()
    }

    #[test]
    fn fully_inside_triangle_is_unchanged() {
        let t = [vec2(1.0, 1.0), vec2(3.0, 1.0), vec2(2.0, 2.0)];
        let c = clip_poly_rect(&t, vec2(0.0, 0.0), vec2(4.0, 4.0));
        assert_eq!(c, t.to_vec());
    }

    #[test]
    fn fully_outside_triangle_vanishes() {
        let t = [vec2(10.0, 10.0), vec2(12.0, 10.0), vec2(11.0, 12.0)];
        assert!(clip_poly_rect(&t, vec2(0.0, 0.0), vec2(4.0, 4.0)).len() < 3);
    }

    #[test]
    fn far_vertex_is_clipped_not_warped() {
        // A sliver triangle with one vertex far outside: the clipped area
        // must equal the exact intersection area — the old vertex-clamp
        // produced a different (warped) shape.
        let t = [vec2(0.0, 0.0), vec2(4.0, 0.0), vec2(2.0, 100.0)];
        let c = clip_poly_rect(&t, vec2(0.0, 0.0), vec2(4.0, 4.0));
        assert!(c.len() >= 3);
        // Exact intersection: trapezoid between y=0 and y=4 of the triangle.
        // Width at y: lerp from 4 at y=0 toward 0 at y=100 => 4*(1-y/100).
        let expect = (4.0 + 4.0 * (1.0 - 4.0 / 100.0)) / 2.0 * 4.0;
        assert!((area(&c) - expect).abs() < 1e-3, "area {} vs {}", area(&c), expect);
    }
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
