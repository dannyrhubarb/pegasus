// Deterministic world generation: cave shape, vertical shafts, boulders,
// landing pads, spawn heights, and the integer-hash PRNG. Everything here is
// a pure function of position / slot index — identical on every visit, which
// is what lets the sliding collider windows load and evict freely. The
// invariants are pinned by the tests at the bottom of main.rs.

use glam::{vec2, Vec2};
use rapier2d::prelude::*;

use crate::replay::LevelParams;

pub const SEG_LEN: f32 = 3.0;
// Historical reset point, now only a world-gen clearance anchor: obstacles
// keep clear of it (like the x = 0 spawn). Respawn itself returns to SPAWN_X
// so every run shares the ghost's start line — changing this value would
// reshape obstacle placement across the whole cave.
pub const RESET_X: f32 = 64.0;

// Cave repeats exactly every PERIOD metres. All terms are integer harmonics
// of the base frequency so they all complete whole cycles together.
pub const PERIOD: f32 = 600.0;
pub const BASE: f32 = std::f32::consts::TAU / PERIOD; // 2π / 600


// ---- Levels ---------------------------------------------------------------
//
// A Level is a parameter block for the procedural generator: everything that
// shapes the WORLD lives here, so new levels are data (levels/*.level files
// fetched at runtime, with compiled-in fallbacks) rather than code. Physics
// depends on these values, so the replay header records them (LevelParams in
// src/replay.rs) and resim reconstructs the Level from the recording.
// seed = 0 reproduces the original cave bit-for-bit (legacy phases).

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scoring {
    Pads,     // +100 per first landing on each pad
    Distance, // high score = max |x| reached in a run
    Time,     // visit EVERY pad; score = completion time, lower is better
}

// Hand-drawn world geometry (the Across/Elasto Mania model): the level file
// author draws SOLID ROCK as closed polygons; the playable space is
// everything they leave open. Polygons may be concave and may overlap (an
// edge buried inside another rock mass is unreachable and harmless), so an
// enclosing map frame is authored as a few fat overlapping slabs. Pads are
// hand-placed. When a Level carries a Terrain, the whole procedural
// generator (cave curves, shafts, obstacles, pad slots) is switched off and
// every polygon edge becomes a static segment collider, loaded all at once —
// hand-drawn maps are finite, so no sliding window is needed.
#[derive(Clone, Debug, PartialEq)]
pub struct Terrain {
    pub polys: Vec<Vec<Vec2>>, // solid rock polygons, CCW (normalized on parse)
    pub pads: Vec<Vec2>,       // hand-placed pads: (deck centre x, deck top y)
    // Neutral START platform (deck centre x, deck top y): a plain deck the
    // ship spawns on — NOT a scoring/refuel pad (it's not in Sim.pads, so
    // no landing logic fires there). Keeps the launch spot out of a Time
    // level's visit list — otherwise pad 0 is a freebie under the spawn.
    // When present it defines the spawn ground (spawn_y = start.y).
    pub start: Option<Vec2>,
    pub spawn_y: f32,          // ground y under the spawn at x = 0
}

impl Terrain {
    // Point-in-rock test (ray cast). Used by the geometry-lint unit tests to
    // assert that chambers, tunnels and pad decks are actually open space.
    pub fn point_in_rock(&self, p: Vec2) -> bool {
        self.polys.iter().any(|poly| {
            let mut inside = false;
            let n = poly.len();
            for i in 0..n {
                let (a, b) = (poly[i], poly[(i + 1) % n]);
                if (a.y > p.y) != (b.y > p.y)
                    && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x
                {
                    inside = !inside;
                }
            }
            inside
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Level {
    pub name: String,
    pub scoring: Scoring,
    pub shafts: bool,
    pub obstacles: bool,
    pub pad_spacing: f32,
    pub seed: u32,
    pub endless: bool, // no x-wrap: value-noise cave that never repeats
    pub terrain: Option<Terrain>, // Some = hand-drawn world, procedural gen off
    // Hard run clock in physics ticks (0 = none): the run ENDS the tick the
    // clock reaches the limit (`Sim.completed`, like a time level's last
    // pad) — e.g. The Flux Sprint's "as far as you can in 60 seconds". The
    // level file key is `time_limit` in SECONDS; it is stored as ticks so
    // the cutoff is exact integer arithmetic (no float-accumulation drift
    // between live play and resim). Physics-relevant → rides in
    // LevelParams (replay format v5).
    pub time_limit_ticks: u32,
    // `seed = random` in the level file: the frontend rolls a FRESH concrete
    // `seed` at every load/reset, so each attempt flies brand-new rock.
    // Metadata only — world generation reads `seed`, never this flag, and it
    // does NOT ride the replay header (LevelParams): a recording always
    // carries the concrete seed it was flown on, so resim/verification
    // rebuild that exact world (`from_params` yields `random_seed: false`).
    pub random_seed: bool,
}

impl Level {
    // The original world: pad scoring, shafts, boulders. seed 0 = legacy.
    pub fn demo() -> Level {
        Level {
            name: "The Caves".to_string(),
            scoring: Scoring::Pads,
            shafts: true,
            obstacles: true,
            pad_spacing: PAD_SPACING,
            seed: 0,
            endless: false,
            terrain: None,
            time_limit_ticks: 0,
            random_seed: false,
        }
    }

    // Parse the `key = value` level format (# comments, unknown keys ignored
    // for forward compatibility; missing keys keep demo defaults).
    // Hand-drawn terrain keys: `poly = x,y x,y …` (one solid rock polygon per
    // line, ≥ 3 vertices), `pad = x,y` (deck centre / deck top),
    // `start = x,y` (the neutral start platform — see Terrain::start) and
    // `spawn_y = <ground y at x = 0>` (overridden by `start`'s y). Any
    // `poly` line switches the level to hand-drawn mode, which forces
    // shafts/obstacles off (they are features of the procedural generator).
    pub fn parse(text: &str) -> Level {
        let mut lvl = Level::demo();
        let mut polys: Vec<Vec<Vec2>> = Vec::new();
        let mut pads: Vec<Vec2> = Vec::new();
        let mut start: Option<Vec2> = None;
        let mut spawn_y = 0.0f32;
        let pt = |s: &str| -> Option<Vec2> {
            let (x, y) = s.split_once(',')?;
            Some(vec2(x.trim().parse().ok()?, y.trim().parse().ok()?))
        };
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            let on = matches!(v, "on" | "true" | "yes" | "1");
            match k {
                "name" => lvl.name = v.to_string(),
                "scoring" => {
                    lvl.scoring = if v.eq_ignore_ascii_case("distance") {
                        Scoring::Distance
                    } else if v.eq_ignore_ascii_case("time") {
                        Scoring::Time
                    } else {
                        Scoring::Pads
                    }
                }
                "shafts" => lvl.shafts = on,
                "endless" => lvl.endless = on,
                "obstacles" => lvl.obstacles = on,
                "pad_spacing" => {
                    if let Ok(f) = v.parse::<f32>() {
                        lvl.pad_spacing = f.clamp(40.0, 2000.0);
                    }
                }
                // Seconds in the file, ticks in the Level (see the field
                // doc). Clamped well under the backend's 30-min resim cap.
                "time_limit" => {
                    if let Ok(f) = v.parse::<f32>() {
                        lvl.time_limit_ticks =
                            (f.clamp(5.0, 1200.0) / crate::sim::PHYSICS_DT).round() as u32;
                    }
                }
                "seed" => {
                    if v.eq_ignore_ascii_case("random") {
                        // The frontend rolls a concrete seed at every
                        // load/reset (see Level::random_seed); until then the
                        // demo default stays as a harmless placeholder.
                        lvl.random_seed = true;
                    } else if let Ok(s) = v.parse::<u32>() {
                        lvl.seed = s;
                    }
                }
                "poly" => {
                    let mut p: Vec<Vec2> = v.split_whitespace().filter_map(pt).collect();
                    if p.len() >= 3 {
                        // Normalize winding to CCW (positive signed area) so
                        // the renderer's "into the rock" edge normal is
                        // consistent for every polygon.
                        let area: f32 = (0..p.len())
                            .map(|i| {
                                let (a, b) = (p[i], p[(i + 1) % p.len()]);
                                a.x * b.y - b.x * a.y
                            })
                            .sum();
                        if area < 0.0 {
                            p.reverse();
                        }
                        polys.push(p);
                    }
                }
                "pad" => {
                    if let Some(p) = pt(v) {
                        pads.push(p);
                    }
                }
                "start" => start = pt(v),
                "spawn_y" => {
                    if let Ok(f) = v.parse::<f32>() {
                        spawn_y = f;
                    }
                }
                _ => {}
            }
        }
        if !polys.is_empty() {
            // The start platform, when present, IS the spawn ground.
            if let Some(sp) = start {
                spawn_y = sp.y;
            }
            lvl.terrain = Some(Terrain { polys, pads, start, spawn_y });
            lvl.shafts = false;
            lvl.obstacles = false;
        }
        lvl
    }

    // Is `self` (a freshly parsed level file) the same FILE as the currently
    // loaded level? The frontend uses this to keep an identical re-push a
    // no-op (re-picking the running level must not reset the flight). A
    // random-seed level's loaded copy carries a rolled concrete seed the
    // file doesn't have, so the comparison neutralizes the seed for those.
    pub fn same_file_as(&self, loaded: &Level) -> bool {
        if self == loaded {
            return true;
        }
        self.random_seed
            && loaded.random_seed
            && Level { seed: loaded.seed, ..self.clone() } == *loaded
    }

    // Per-harmonic phase offset derived from the seed. seed 0 = the original
    // cave (zero phases), any other seed reshapes the whole world.
    fn phase(&self, salt: u32) -> f32 {
        if self.seed == 0 {
            0.0
        } else {
            (hash_u32(self.seed ^ salt) & 0xffff) as f32 / 65535.0 * std::f32::consts::TAU
        }
    }

    // Fold the seed into a slot hash (0 = legacy value untouched).
    fn slot_seed(&self, k: u32) -> u32 {
        k ^ self.seed.wrapping_mul(0x9e37_79b9)
    }

    // The physics-relevant subset, for the replay header. The name is
    // cosmetic and doesn't survive the round trip. Hand-drawn terrain IS
    // physics (every edge is a collider), so it rides along whole — a replay
    // of a hand-drawn level is self-contained.
    pub fn to_params(&self) -> LevelParams {
        LevelParams {
            scoring: match self.scoring {
                Scoring::Pads => 0,
                Scoring::Distance => 1,
                Scoring::Time => 2,
            },
            shafts: self.shafts as u8,
            obstacles: self.obstacles as u8,
            pad_spacing: self.pad_spacing,
            seed: self.seed,
            endless: self.endless as u8,
            terrain: self.terrain.clone(),
            time_limit_ticks: self.time_limit_ticks,
        }
    }

    pub fn from_params(p: &LevelParams) -> Level {
        Level {
            name: "(replay)".to_string(),
            scoring: match p.scoring {
                1 => Scoring::Distance,
                2 => Scoring::Time,
                _ => Scoring::Pads,
            },
            // Terrain levels never run the procedural generator (parse
            // forces these off too — keep the round trip consistent).
            shafts: p.shafts != 0 && p.terrain.is_none(),
            obstacles: p.obstacles != 0 && p.terrain.is_none(),
            pad_spacing: p.pad_spacing,
            seed: p.seed,
            endless: p.endless != 0,
            terrain: p.terrain.clone(),
            time_limit_ticks: p.time_limit_ticks,
            // A replay is always of ONE concrete world — the rolled seed
            // above is that world; re-rolling would break resim.
            random_seed: false,
        }
    }
}

impl Level {
    // Signed value noise in [-1, 1]: smoothstep-interpolated hashes on an
    // integer lattice of `wavelength`-sized cells. The endless cave's
    // analogue of one harmonic term — the SAME amplitude bounds (so every
    // flyability guarantee carries over) but keyed on the cell index, so
    // the curve never repeats. C1-continuous, pure function of x.
    fn vnoise(&self, salt: u32, x: f32, wavelength: f32) -> f32 {
        let t = x / wavelength;
        let i = t.floor();
        let f = t - i;
        let h = |c: i64| {
            (hash_u32(
                (c as u32)
                    ^ self.seed.wrapping_mul(0x9e37_79b9)
                    ^ salt.wrapping_mul(0x85eb_ca6b),
            ) & 0xffff) as f32
                / 65535.0
                * 2.0
                - 1.0
        };
        let (a, b) = (h(i as i64), h(i as i64 + 1));
        a + (b - a) * (f * f * (3.0 - 2.0 * f))
    }

    pub fn cave_center(&self, x: f32) -> f32 {
        if self.endless {
            // Endless mode: no PERIOD — value-noise terms mirroring the
            // harmonic amplitudes below, hashed per cell so the cave goes
            // on forever without wrapping.
            return self.vnoise(1, x, 210.0) * 14.0  // big slow sweep
                + self.vnoise(2, x, 70.0) * 5.0     // medium curves
                + self.vnoise(3, x, 26.0) * 3.0;    // tighter wiggles
        }
        (x * BASE + self.phase(1)).sin()       * 14.0   // 1st harmonic  — big slow sweep
        + (x * BASE * 3.0 + self.phase(2)).cos() *  5.0 // 3rd harmonic  — medium curves
        + (x * BASE * 7.0 + self.phase(3)).sin() *  3.0 // 7th harmonic  — tighter wiggles
    }

    pub fn cave_half_width(&self, x: f32) -> f32 {
        if self.endless {
            // Same bounds as the harmonics: [2.5, 12.5] — no seed and no
            // stretch of x can pinch the endless cave shut either.
            return 6.5
                + self.vnoise(4, x, 130.0) * 2.5        // narrows / widens slowly
                + self.vnoise(5, x, 47.0) * 1.5         // medium variation
                + self.vnoise(6, x, 19.0).abs() * 2.0;  // pinch points (abs keeps it positive)
        }
        6.5
        + (x * BASE * 2.0 + self.phase(4)).sin()      * 2.5  // narrows / widens slowly
        + (x * BASE * 5.0 + self.phase(5)).cos()      * 1.5  // medium variation
        + (x * BASE * 11.0 + self.phase(6)).sin().abs() * 2.0 // pinch points (abs keeps it positive)
    }

    // Spawn/reset height at x: standing on the floor (feet reach 0.73 below
    // the body origin). Spawning at cave_center dropped the ship 8–9 m; the
    // ~5.5 m/s touchdown tripped the crash threshold and put spawn → crash →
    // respawn into an endless loop.
    pub fn stand_y(&self, x: f32) -> f32 {
        // Hand-drawn worlds spawn at the authored spot (x = 0, on the spawn
        // pad — hollows-style maps put pad 0 there like the procedural cave
        // does), so the ground height is a level-file key, not a curve.
        if let Some(t) = &self.terrain {
            return t.spawn_y + 0.78;
        }
        let mut ground = self.cave_center(x) - self.cave_half_width(x);
        // If a landing pad deck covers x, stand on the deck instead (its
        // friction also keeps the parked ship from drifting on sloped,
        // frictionless rock).
        let p0 = (x / self.pad_spacing).round() as i64;
        for p in p0 - 1..=p0 + 1 {
            if let Some(pad) = self.pad_spec(p)
                && (pad.cx - x).abs() <= PAD_HALF_W
            {
                ground = ground.max(pad.y);
            }
        }
        ground + 0.78
    }

    // Returns (top_a, top_b, bot_a, bot_b) for segment index i
    pub fn seg_points(&self, idx: i64) -> (Point<f32>, Point<f32>, Point<f32>, Point<f32>) {
        let x0 = idx as f32 * SEG_LEN;
        let x1 = x0 + SEG_LEN;
        let (cy0, hw0) = (self.cave_center(x0), self.cave_half_width(x0));
        let (cy1, hw1) = (self.cave_center(x1), self.cave_half_width(x1));
        (
            point![x0, cy0 + hw0], point![x1, cy1 + hw1],
            point![x0, cy0 - hw0], point![x1, cy1 - hw1],
        )
    }

    pub fn insert_seg(&self, idx: i64, layer: i64, collider_set: &mut ColliderSet) -> Vec<ColliderHandle> {
        // Shaft openings: no ceiling/floor collider where a vertical shaft
        // punches through — the shaft walls take over at the opening edges.
        if self.seg_in_opening(idx) {
            return Vec::new();
        }
        let ly = layer as f32 * V_PERIOD;
        let (ta, tb, ba, bb) = self.seg_points(idx);
        let off = |p: Point<f32>| point![p.x, p.y + ly];
        vec![
            collider_set.insert(ColliderBuilder::segment(off(ta), off(tb)).friction(0.0).build()),
            collider_set.insert(ColliderBuilder::segment(off(ba), off(bb)).friction(0.0).build()),
        ]
    }
}

// The levels that ship with the game (levels/*.level, pinned via
// include_str! so they compile into every consumer of this crate), keyed by
// the level file STEM — the id the boards/backend key everything by. This is
// what lets the server-side verifier check that a submitted replay's
// LevelParams actually belong to the board it claims: an unknown stem (a
// level added to levels/ after the backend was last built) can't be
// params-checked, so verifiers should skip that check rather than reject.
// EDITING a shipped .level file changes physics → redeploy the backend in
// the same breath, or honest submissions on that level will be rejected.
pub fn shipped_levels() -> Vec<(&'static str, Level)> {
    vec![
        ("expanse", Level::parse(include_str!("../../levels/expanse.level"))),
        ("glide", Level::parse(include_str!("../../levels/glide.level"))),
        ("flux", Level::parse(include_str!("../../levels/flux.level"))),
        ("flux-sprint", Level::parse(include_str!("../../levels/flux-sprint.level"))),
        ("hollows", Level::parse(include_str!("../../levels/hollows.level"))),
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

impl Level {
    // Start segment of the ceiling/floor opening for shaft slot `s`. Jitter
    // is keyed on s mod 4 (= slots per PERIOD) so the pattern repeats exactly
    // every period in x — the wrap stays seamless in both axes.
    pub fn shaft_open_seg(&self, s: i64) -> i64 {
        let j = (hash_u32(self.slot_seed(s.rem_euclid(4) as u32) ^ 0x51ed_270b) % 13) as i64 - 6; // ±6 segs
        s * SHAFT_SPACING_SEGS + SHAFT_BASE_SEG + j
    }

    // Does cave segment `idx` fall inside a shaft opening (→ walls removed
    // there)? Always false on levels without shafts.
    pub fn seg_in_opening(&self, idx: i64) -> bool {
        if !self.shafts {
            return false;
        }
        let s0 = (idx - SHAFT_BASE_SEG).div_euclid(SHAFT_SPACING_SEGS);
        [s0, s0 + 1].iter().any(|&s| {
            let o = self.shaft_open_seg(s);
            idx >= o && idx < o + SHAFT_OPEN_SEGS
        })
    }

    // Wall x for shaft `s` at normalized height t ∈ [0,1]; side 0 = left,
    // 1 = right. The envelope pins both ends exactly to the opening edges so
    // the wall meets the clipped ceiling/floor colliders with no gap; in
    // between it wiggles up to ±1.25 m (opening is 9 m wide → the shaft
    // always stays ≥ 6.5 m flyable).
    pub fn shaft_wall_x(&self, s: i64, side: u8, t: f32) -> f32 {
        let o = self.shaft_open_seg(s);
        let edge = (if side == 0 { o } else { o + SHAFT_OPEN_SEGS }) as f32 * SEG_LEN;
        let h = hash_u32(self.slot_seed(s.rem_euclid(4) as u32) ^ ((side as u32) << 8) ^ 0xabc0_ffee);
        let tau = std::f32::consts::TAU;
        let p1 = (h & 0xffff) as f32 / 65535.0 * tau;
        let p2 = ((h >> 16) & 0xffff) as f32 / 65535.0 * tau;
        let env = (t.min(1.0 - t) / 0.18).clamp(0.0, 1.0);
        edge + env * ((t * tau * 2.0 + p1).sin() * 0.9 + (t * tau * 5.0 + p2).sin() * 0.35)
    }

    // Wall polyline for the shaft connecting layer `gap`'s ceiling to layer
    // `gap + 1`'s floor. Endpoints lie exactly on the two cave wall curves at
    // the opening edges, so colliders and row-0 facets chain seamlessly
    // through both junctions. The shape is identical for every gap (mod
    // V_PERIOD) — the wrap.
    pub fn shaft_wall_pts(&self, s: i64, gap: i64, side: u8) -> Vec<Vec2> {
        let o = self.shaft_open_seg(s);
        let xe = (if side == 0 { o } else { o + SHAFT_OPEN_SEGS }) as f32 * SEG_LEN;
        let y_bot = gap as f32 * V_PERIOD + self.cave_center(xe) + self.cave_half_width(xe);
        let y_top = (gap + 1) as f32 * V_PERIOD + self.cave_center(xe) - self.cave_half_width(xe);
        let n = ((y_top - y_bot) / SHAFT_STEP).ceil().max(1.0) as usize;
        (0..=n)
            .map(|i| {
                let t = i as f32 / n as f32;
                vec2(self.shaft_wall_x(s, side, t), y_bot + (y_top - y_bot) * t)
            })
            .collect()
    }
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
    // Not an Iterator: this can never end and must never be fused/adapted —
    // world-gen call sites consume a fixed, order-sensitive number of draws.
    #[allow(clippy::should_implement_trait)]
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

impl Level {
    pub fn obstacle_spec(&self, k: i64) -> Option<ObstacleSpec> {
        if !self.obstacles {
            return None;
        }
        let mut rng = Rng::new(self.slot_seed(k as u32));

        let cx = k as f32 * OBSTACLE_SPACING + rng.range(-3.0, 3.0);

        // Keep the spawn and reset areas clear so neither drops the ship onto a rock.
        if cx.abs() < 9.0 || (cx - RESET_X).abs() < 9.0 {
            return None;
        }

        let cy_wall = self.cave_center(cx);
        let hw = self.cave_half_width(cx);

        // Skip pinch points — no room for an obstacle plus a passable gap.
        if hw < 4.5 {
            return None;
        }

        // Skip slots near a vertical shaft opening so the junction crossings
        // (where the player has to maneuver vertically) stay flyable.
        if self.shafts {
            let seg = (cx / SEG_LEN).floor() as i64;
            let s0 = (seg - SHAFT_BASE_SEG).div_euclid(SHAFT_SPACING_SEGS);
            for s in [s0, s0 + 1] {
                let o = self.shaft_open_seg(s);
                let (ox0, ox1) = (o as f32 * SEG_LEN, (o + SHAFT_OPEN_SEGS) as f32 * SEG_LEN);
                if cx > ox0 - 8.0 && cx < ox1 + 8.0 {
                    return None;
                }
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

impl Level {
    pub fn pad_spec(&self, p: i64) -> Option<PadSpec> {
        let mut rng = Rng::new(self.slot_seed(p as u32) ^ 0x50AD_5EED);
        let cx = p as f32 * self.pad_spacing + rng.range(-20.0, 20.0);

        // Need headroom to come down vertically.
        if self.cave_half_width(cx) < 5.0 {
            return None;
        }

        // Keep clear of shaft openings (same 8 m rule as obstacles).
        if self.shafts {
            let seg = (cx / SEG_LEN).floor() as i64;
            let s0 = (seg - SHAFT_BASE_SEG).div_euclid(SHAFT_SPACING_SEGS);
            for s in [s0, s0 + 1] {
                let o = self.shaft_open_seg(s);
                let (ox0, ox1) = (o as f32 * SEG_LEN, (o + SHAFT_OPEN_SEGS) as f32 * SEG_LEN);
                if cx > ox0 - 8.0 && cx < ox1 + 8.0 {
                    return None;
                }
            }
        }

        // Don't overlap a boulder: check the obstacle slots whose jitter range
        // could reach the deck. (obstacle_spec is None on boulder-free levels,
        // so those levels keep every otherwise-valid pad slot.)
        let k0 = (cx / OBSTACLE_SPACING).round() as i64;
        for k in k0 - 1..=k0 + 1 {
            if let Some(ob) = self.obstacle_spec(k) {
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
            y = y.max(self.cave_center(x) - self.cave_half_width(x));
        }
        Some(PadSpec { cx, y: y + 0.1 })
    }
}
