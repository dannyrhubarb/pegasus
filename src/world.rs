// Deterministic world generation: cave shape, vertical shafts, boulders,
// landing pads, spawn heights, and the integer-hash PRNG. Everything here is
// a pure function of position / slot index — identical on every visit, which
// is what lets the sliding collider windows load and evict freely. The
// invariants are pinned by the tests at the bottom of main.rs.

use macroquad::prelude::*;
use rapier2d::prelude::*;

pub const SEG_LEN: f32 = 3.0;
// Where [R] / gamepad reset drops the ship. Obstacles keep clear of this point
// (like the x = 0 spawn) so a reset never lands on a rock.
pub const RESET_X: f32 = 64.0;

// Cave repeats exactly every PERIOD metres. All terms are integer harmonics
// of the base frequency so they all complete whole cycles together.
pub const PERIOD: f32 = 600.0;
pub const BASE: f32 = std::f32::consts::TAU / PERIOD; // 2π / 600

pub fn cave_center(x: f32) -> f32 {
    (x * BASE).sin()       * 14.0   // 1st harmonic  — big slow sweep
    + (x * BASE * 3.0).cos() *  5.0 // 3rd harmonic  — medium curves
    + (x * BASE * 7.0).sin() *  3.0 // 7th harmonic  — tighter wiggles
}

pub fn cave_half_width(x: f32) -> f32 {
    6.5
    + (x * BASE * 2.0).sin()      * 2.5  // narrows / widens slowly
    + (x * BASE * 5.0).cos()      * 1.5  // medium variation
    + (x * BASE * 11.0).sin().abs() * 2.0 // pinch points (abs keeps it positive)
}

// Spawn/reset height at x: standing on the floor (feet reach 0.73 below the
// body origin). Spawning at cave_center dropped the ship 8–9 m; the ~5.5 m/s
// touchdown tripped CRASH_DV and put spawn → crash → respawn into an endless
// loop.
pub fn stand_y(x: f32) -> f32 {
    let mut ground = cave_center(x) - cave_half_width(x);
    // If a landing pad deck covers x, stand on the deck instead (its friction
    // also keeps the parked ship from drifting on sloped, frictionless rock).
    let p0 = (x / PAD_SPACING).round() as i64;
    for p in p0 - 1..=p0 + 1 {
        if let Some(pad) = pad_spec(p)
            && (pad.cx - x).abs() <= PAD_HALF_W
        {
            ground = ground.max(pad.y);
        }
    }
    ground + 0.78
}

// Returns (top_a, top_b, bot_a, bot_b) for segment index i
pub fn seg_points(idx: i64) -> (Point<f32>, Point<f32>, Point<f32>, Point<f32>) {
    let x0 = idx as f32 * SEG_LEN;
    let x1 = x0 + SEG_LEN;
    let (cy0, hw0) = (cave_center(x0), cave_half_width(x0));
    let (cy1, hw1) = (cave_center(x1), cave_half_width(x1));
    (
        point![x0, cy0 + hw0], point![x1, cy1 + hw1],
        point![x0, cy0 - hw0], point![x1, cy1 - hw1],
    )
}

pub fn insert_seg(idx: i64, layer: i64, collider_set: &mut ColliderSet) -> Vec<ColliderHandle> {
    // Shaft openings: no ceiling/floor collider where a vertical shaft punches
    // through — the shaft's own walls take over exactly at the opening edges.
    if seg_in_opening(idx) {
        return Vec::new();
    }
    let ly = layer as f32 * V_PERIOD;
    let (ta, tb, ba, bb) = seg_points(idx);
    let off = |p: Point<f32>| point![p.x, p.y + ly];
    vec![
        collider_set.insert(ColliderBuilder::segment(off(ta), off(tb)).friction(0.0).build()),
        collider_set.insert(ColliderBuilder::segment(off(ba), off(bb)).friction(0.0).build()),
    ]
}

// ---- Vertical shafts ------------------------------------------------------
//
// The world also repeats every V_PERIOD metres in y: identical copies of the
// horizontal cave are stacked vertically, and vertical shafts punch through
// ceiling + floor at deterministic x positions, connecting each cave layer to
// the (identical) one above/below. Climbing a shaft therefore "wraps around"
// back into the same cave, exactly like flying PERIOD metres wraps in x.

pub const V_PERIOD: f32 = 90.0;         // vertical repeat distance (m)
pub const SHAFT_SPACING_SEGS: i64 = 50; // one shaft slot every 150 m (4 per PERIOD)
pub const SHAFT_BASE_SEG: i64 = 35;     // slot anchor; keeps openings clear of x = 0 / 64
pub const SHAFT_OPEN_SEGS: i64 = 3;     // opening width: 3 segments = 9 m
pub const SHAFT_STEP: f32 = 3.0;        // shaft wall segment length (m)

// Start segment of the ceiling/floor opening for shaft slot `s`. Jitter is
// keyed on s mod 4 (= slots per PERIOD) so the pattern repeats exactly every
// period in x — the wrap stays seamless in both axes.
pub fn shaft_open_seg(s: i64) -> i64 {
    let j = (hash_u32(s.rem_euclid(4) as u32 ^ 0x51ed_270b) % 13) as i64 - 6; // ±6 segs
    s * SHAFT_SPACING_SEGS + SHAFT_BASE_SEG + j
}

// Does cave segment `idx` fall inside a shaft opening (→ walls removed there)?
pub fn seg_in_opening(idx: i64) -> bool {
    let s0 = (idx - SHAFT_BASE_SEG).div_euclid(SHAFT_SPACING_SEGS);
    [s0, s0 + 1].iter().any(|&s| {
        let o = shaft_open_seg(s);
        idx >= o && idx < o + SHAFT_OPEN_SEGS
    })
}

// Wall x for shaft `s` at normalized height t ∈ [0,1]; side 0 = left, 1 = right.
// The envelope pins both ends exactly to the opening edges so the wall meets
// the clipped ceiling/floor colliders with no gap; in between it wiggles up to
// ±1.25 m (opening is 9 m wide → the shaft always stays ≥ 6.5 m flyable).
pub fn shaft_wall_x(s: i64, side: u8, t: f32) -> f32 {
    let o = shaft_open_seg(s);
    let edge = (if side == 0 { o } else { o + SHAFT_OPEN_SEGS }) as f32 * SEG_LEN;
    let h = hash_u32(s.rem_euclid(4) as u32 ^ ((side as u32) << 8) ^ 0xabc0_ffee);
    let tau = std::f32::consts::TAU;
    let p1 = (h & 0xffff) as f32 / 65535.0 * tau;
    let p2 = ((h >> 16) & 0xffff) as f32 / 65535.0 * tau;
    let env = (t.min(1.0 - t) / 0.18).clamp(0.0, 1.0);
    edge + env * ((t * tau * 2.0 + p1).sin() * 0.9 + (t * tau * 5.0 + p2).sin() * 0.35)
}

// Wall polyline for the shaft connecting layer `gap`'s ceiling to layer
// `gap + 1`'s floor. Endpoints lie exactly on the two cave wall curves at the
// opening edges, so colliders and row-0 facets chain seamlessly through both
// junctions. The shape is identical for every gap (mod V_PERIOD) — the wrap.
pub fn shaft_wall_pts(s: i64, gap: i64, side: u8) -> Vec<Vec2> {
    let o = shaft_open_seg(s);
    let xe = (if side == 0 { o } else { o + SHAFT_OPEN_SEGS }) as f32 * SEG_LEN;
    let y_bot = gap as f32 * V_PERIOD + cave_center(xe) + cave_half_width(xe);
    let y_top = (gap + 1) as f32 * V_PERIOD + cave_center(xe) - cave_half_width(xe);
    let n = ((y_top - y_bot) / SHAFT_STEP).ceil().max(1.0) as usize;
    (0..=n)
        .map(|i| {
            let t = i as f32 / n as f32;
            vec2(shaft_wall_x(s, side, t), y_bot + (y_top - y_bot) * t)
        })
        .collect()
}

// ---- Random polygon obstacles -------------------------------------------
//
// Obstacles are placed deterministically along the cave so they stay put as
// the player flies back and forth, and so they load/unload with the same
// sliding window as the walls. Each obstacle slot `k` maps to a fixed
// position and a fixed random convex polygon, derived purely from `k`.

// Average spacing between obstacle slots, in metres.
pub const OBSTACLE_SPACING: f32 = 16.0;

// Tiny deterministic PRNG (integer hash). Seeded per obstacle slot so the
// same slot always produces the same obstacle, independent of when it loads.
pub struct Rng(u32);

pub fn hash_u32(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x
}

impl Rng {
    pub fn new(seed: u32) -> Self {
        Rng(hash_u32(seed ^ 0x9e37_79b9))
    }
    pub fn next(&mut self) -> u32 {
        self.0 = hash_u32(self.0);
        self.0
    }
    pub fn unit(&mut self) -> f32 {
        (self.next() >> 8) as f32 / (1u32 << 24) as f32
    }
    pub fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.unit()
    }
    pub fn range_int(&mut self, a: i32, b: i32) -> i32 {
        a + (self.next() % (b - a + 1) as u32) as i32
    }
}

// Deterministic spec for obstacle slot `k`. Returns None where the cave is
// too narrow (or too close to the spawn point) to fit a fair obstacle.
pub struct ObstacleSpec {
    pub cx: f32,
    pub cy: f32,
    pub rot: f32,
    pub pts: Vec<Point<f32>>, // local-space candidate vertices for the convex hull
}

pub fn obstacle_spec(k: i64) -> Option<ObstacleSpec> {
    let mut rng = Rng::new(k as u32);

    let cx = k as f32 * OBSTACLE_SPACING + rng.range(-3.0, 3.0);

    // Keep the spawn and reset areas clear so neither drops the ship onto a rock.
    if cx.abs() < 9.0 || (cx - RESET_X).abs() < 9.0 {
        return None;
    }

    let cy_wall = cave_center(cx);
    let hw = cave_half_width(cx);

    // Skip pinch points — no room for an obstacle plus a passable gap.
    if hw < 4.5 {
        return None;
    }

    // Skip slots near a vertical shaft opening so the junction crossings
    // (where the player has to maneuver vertically) stay flyable.
    let seg = (cx / SEG_LEN).floor() as i64;
    let s0 = (seg - SHAFT_BASE_SEG).div_euclid(SHAFT_SPACING_SEGS);
    for s in [s0, s0 + 1] {
        let o = shaft_open_seg(s);
        let (ox0, ox1) = (o as f32 * SEG_LEN, (o + SHAFT_OPEN_SEGS) as f32 * SEG_LEN);
        if cx > ox0 - 8.0 && cx < ox1 + 8.0 {
            return None;
        }
    }

    // Roughly 1 in 6 slots is empty, for uneven, natural-feeling spacing.
    if rng.range_int(0, 5) == 0 {
        return None;
    }

    // Obstacle size. Boulders up to 5.5 m radius appear in the widest
    // sections; the cap scales with local half-width so a gap always fits.
    let max_r = (hw * 0.65).min(5.5);
    let r = rng.range(0.3, 1.0) * max_r;

    // Centre offset, leaving at least ~1.3 m clearance to the nearer wall so
    // there is always a flyable gap on at least one side.
    let max_off = (hw - r - 1.3).max(0.0);
    let cy = cy_wall + rng.range(-max_off, max_off);

    // Build a lumpy convex polygon: vertices at sorted angles, varying radius.
    let n = rng.range_int(6, 9);
    let mut pts = Vec::with_capacity(n as usize);
    for i in 0..n {
        let base = i as f32 / n as f32 * std::f32::consts::TAU;
        let ang = base + rng.range(-0.25, 0.25);
        let rad = r * rng.range(0.6, 1.0);
        pts.push(point![rad * ang.cos(), rad * ang.sin()]);
    }

    Some(ObstacleSpec {
        cx,
        cy,
        rot: rng.range(0.0, std::f32::consts::TAU),
        pts,
    })
}

// ---- Landing pads ---------------------------------------------------------
//
// Flat metal pads sit on the cave floor at deterministic x positions. Landing
// gently (slow, upright, settled for PAD_LAND_TIME) refuels the ship and, on
// the first visit to a pad, scores PAD_POINTS. Like obstacles, pads are pure
// functions of their slot index and replicate on every layer (the y-wrap).

pub const PAD_SPACING: f32 = 130.0;    // metres between pad slots (plus ±20 jitter)
pub const PAD_HALF_W: f32 = 3.0;       // deck half-width
pub const PAD_POINTS: u32 = 100;       // score for a first landing
pub const PAD_LAND_TIME: f32 = 0.8;    // seconds settled before a landing counts
pub const PAD_REFUEL_PER_S: f32 = 25.0; // fuel per second while parked on a pad

pub struct PadSpec {
    pub cx: f32,
    pub y: f32, // deck top = collider line
}

pub fn pad_spec(p: i64) -> Option<PadSpec> {
    let mut rng = Rng::new((p as u32) ^ 0x50AD_5EED);
    let cx = p as f32 * PAD_SPACING + rng.range(-20.0, 20.0);

    // Need headroom to come down vertically.
    if cave_half_width(cx) < 5.0 {
        return None;
    }

    // Keep clear of shaft openings (same 8 m rule as obstacles).
    let seg = (cx / SEG_LEN).floor() as i64;
    let s0 = (seg - SHAFT_BASE_SEG).div_euclid(SHAFT_SPACING_SEGS);
    for s in [s0, s0 + 1] {
        let o = shaft_open_seg(s);
        let (ox0, ox1) = (o as f32 * SEG_LEN, (o + SHAFT_OPEN_SEGS) as f32 * SEG_LEN);
        if cx > ox0 - 8.0 && cx < ox1 + 8.0 {
            return None;
        }
    }

    // Don't overlap a boulder: check the obstacle slots whose jitter range
    // could reach the deck.
    let k0 = (cx / OBSTACLE_SPACING).round() as i64;
    for k in k0 - 1..=k0 + 1 {
        if let Some(ob) = obstacle_spec(k) {
            let r = ob
                .pts
                .iter()
                .map(|q| (q.x * q.x + q.y * q.y).sqrt())
                .fold(0.0f32, f32::max);
            if (ob.cx - cx).abs() < r + PAD_HALF_W + 1.0 {
                return None;
            }
        }
    }

    // Deck sits just above the highest floor point under the span, so the
    // collider never dips into the rock.
    let mut y = f32::NEG_INFINITY;
    for i in 0..=12 {
        let x = cx - PAD_HALF_W + i as f32 * (PAD_HALF_W / 6.0);
        y = y.max(cave_center(x) - cave_half_width(x));
    }
    Some(PadSpec { cx, y: y + 0.1 })
}
