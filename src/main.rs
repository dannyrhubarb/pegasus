use macroquad::prelude::*;
use macroquad::rand::gen_range;
use rapier2d::prelude::*;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU32, Ordering};

struct Particle {
    x: f32, y: f32,
    vx: f32, vy: f32,
    life: f32,  // 1.0 = fresh, 0.0 = dead
    kind: u8,   // 0 = main thruster, 1 = left RCS, 2 = right RCS
}

static TOUCH_THRUST: AtomicU32 = AtomicU32::new(0);
static TOUCH_TORQUE: AtomicU32 = AtomicU32::new(0);
static SAFE_AREA_TOP: AtomicU32 = AtomicU32::new(0);
static SAFE_AREA_LEFT: AtomicU32 = AtomicU32::new(0);

#[unsafe(no_mangle)]
pub extern "C" fn set_touch_thrust(active: i32) {
    TOUCH_THRUST.store(active as u32, Ordering::Relaxed);
}

#[unsafe(no_mangle)]
pub extern "C" fn set_touch_torque(value: f32) {
    TOUCH_TORQUE.store(value.to_bits(), Ordering::Relaxed);
}

#[unsafe(no_mangle)]
pub extern "C" fn set_safe_area(top: f32, left: f32) {
    SAFE_AREA_TOP.store(top.to_bits(), Ordering::Relaxed);
    SAFE_AREA_LEFT.store(left.to_bits(), Ordering::Relaxed);
}

fn window_conf() -> Conf {
    Conf {
        window_title: "Rapier 2D — Cave".to_string(),
        window_width: 1440,
        window_height: 900,
        high_dpi: false,
        platform: macroquad::miniquad::conf::Platform {
            webgl_version: macroquad::miniquad::conf::WebGLVersion::WebGL2,
            ..Default::default()
        },
        ..Default::default()
    }
}

const SCALE: f32 = 80.0;
const SEG_LEN: f32 = 3.0;
// How many segments to keep loaded on each side of the ship
const HALF_WINDOW: i64 = 80;

// Vertex shader: passes screen-pixel position as a varying so the
// fragment shader can compute per-pixel distance from the ship.
const LIGHT_VERTEX: &str = r#"#version 100
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
const LIGHT_FRAGMENT: &str = r#"#version 100
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
    float warm    = glow * falloff * 0.28;

    vec4 base = color * texture2D(Texture, uv);
    gl_FragColor = vec4(
        min(base.r * l + warm,       1.0),
        min(base.g * l + warm * 0.4, 1.0),
        min(base.b * l,              1.0),
        1.0);
}"#;


// Cave repeats exactly every PERIOD metres. All terms are integer harmonics
// of the base frequency so they all complete whole cycles together.
const PERIOD: f32 = 600.0;
const BASE: f32 = std::f32::consts::TAU / PERIOD; // 2π / 600

fn cave_center(x: f32) -> f32 {
    (x * BASE).sin()       * 14.0   // 1st harmonic  — big slow sweep
    + (x * BASE * 3.0).cos() *  5.0 // 3rd harmonic  — medium curves
    + (x * BASE * 7.0).sin() *  3.0 // 7th harmonic  — tighter wiggles
}

fn cave_half_width(x: f32) -> f32 {
    6.5
    + (x * BASE * 2.0).sin()      * 2.5  // narrows / widens slowly
    + (x * BASE * 5.0).cos()      * 1.5  // medium variation
    + (x * BASE * 11.0).sin().abs() * 2.0 // pinch points (abs keeps it positive)
}

// Returns (top_a, top_b, bot_a, bot_b) for segment index i
fn seg_points(idx: i64) -> (Point<f32>, Point<f32>, Point<f32>, Point<f32>) {
    let x0 = idx as f32 * SEG_LEN;
    let x1 = x0 + SEG_LEN;
    let (cy0, hw0) = (cave_center(x0), cave_half_width(x0));
    let (cy1, hw1) = (cave_center(x1), cave_half_width(x1));
    (
        point![x0, cy0 + hw0], point![x1, cy1 + hw1],
        point![x0, cy0 - hw0], point![x1, cy1 - hw1],
    )
}

fn insert_seg(
    idx: i64,
    collider_set: &mut ColliderSet,
) -> (ColliderHandle, ColliderHandle) {
    let (ta, tb, ba, bb) = seg_points(idx);
    let top = collider_set.insert(ColliderBuilder::segment(ta, tb).friction(0.5).build());
    let bot = collider_set.insert(ColliderBuilder::segment(ba, bb).friction(0.5).build());
    (top, bot)
}

// ---- Random polygon obstacles -------------------------------------------
//
// Obstacles are placed deterministically along the cave so they stay put as
// the player flies back and forth, and so they load/unload with the same
// sliding window as the walls. Each obstacle slot `k` maps to a fixed
// position and a fixed random convex polygon, derived purely from `k`.

// Average spacing between obstacle slots, in metres.
const OBSTACLE_SPACING: f32 = 16.0;

// Tiny deterministic PRNG (integer hash). Seeded per obstacle slot so the
// same slot always produces the same obstacle, independent of when it loads.
struct Rng(u32);

fn hash_u32(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x
}

impl Rng {
    fn new(seed: u32) -> Self {
        Rng(hash_u32(seed ^ 0x9e37_79b9))
    }
    fn next(&mut self) -> u32 {
        self.0 = hash_u32(self.0);
        self.0
    }
    fn unit(&mut self) -> f32 {
        (self.next() >> 8) as f32 / (1u32 << 24) as f32
    }
    fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.unit()
    }
    fn range_int(&mut self, a: i32, b: i32) -> i32 {
        a + (self.next() % (b - a + 1) as u32) as i32
    }
}

// Deterministic spec for obstacle slot `k`. Returns None where the cave is
// too narrow (or too close to the spawn point) to fit a fair obstacle.
struct ObstacleSpec {
    cx: f32,
    cy: f32,
    rot: f32,
    pts: Vec<Point<f32>>, // local-space candidate vertices for the convex hull
}

fn obstacle_spec(k: i64) -> Option<ObstacleSpec> {
    let mut rng = Rng::new(k as u32);

    let cx = k as f32 * OBSTACLE_SPACING + rng.range(-3.0, 3.0);

    // Keep the spawn area clear so a reset never drops the ship onto a rock.
    if cx.abs() < 9.0 {
        return None;
    }

    let cy_wall = cave_center(cx);
    let hw = cave_half_width(cx);

    // Skip pinch points — no room for an obstacle plus a passable gap.
    if hw < 4.5 {
        return None;
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

#[macroquad::main(window_conf)]
async fn main() {
    let mut rigid_body_set = RigidBodySet::new();
    let mut collider_set = ColliderSet::new();

    // Sliding window: deque of (segment_index, top_handle, bot_handle)
    let mut cave: VecDeque<(i64, ColliderHandle, ColliderHandle)> = VecDeque::new();

    // Seed the initial window around x=0
    for idx in -HALF_WINDOW..=HALF_WINDOW {
        let (top, bot) = insert_seg(idx, &mut collider_set);
        cave.push_back((idx, top, bot));
    }

    // Loaded obstacles, keyed by their slot index. Each carries its collider
    // handle plus the hull vertices (local space) used for rendering.
    struct Obstacle {
        handle: ColliderHandle,
        cx: f32,
        cy: f32,
        rot: f32,
        verts: Vec<Vec2>,
    }
    let mut obstacles: HashMap<i64, Obstacle> = HashMap::new();

    // Insert the obstacle for slot `k` (if any) into the world + render map.
    let spawn_obstacle = |k: i64, collider_set: &mut ColliderSet,
                              obstacles: &mut HashMap<i64, Obstacle>| {
        let Some(spec) = obstacle_spec(k) else { return };
        let Some(builder) = ColliderBuilder::convex_hull(&spec.pts) else { return };
        let handle = collider_set.insert(
            builder
                .translation(vector![spec.cx, spec.cy])
                .rotation(spec.rot)
                .friction(0.6)
                .restitution(0.2)
                .build(),
        );
        // Read the actual hull vertices back so rendering matches the collider.
        let verts = collider_set[handle]
            .shape()
            .as_convex_polygon()
            .map(|cp| cp.points().iter().map(|p| vec2(p.x, p.y)).collect())
            .unwrap_or_else(|| spec.pts.iter().map(|p| vec2(p.x, p.y)).collect());
        obstacles.insert(k, Obstacle {
            handle,
            cx: spec.cx,
            cy: spec.cy,
            rot: spec.rot,
            verts,
        });
    };

    // Ship starts at cave centre
    let box_body = RigidBodyBuilder::dynamic()
        .translation(vector![0.0, cave_center(0.0)])
        .angular_damping(3.0)
        .build();
    let box_handle = rigid_body_set.insert(box_body);
    collider_set.insert_with_parent(
        ColliderBuilder::cuboid(0.5, 0.5).restitution(0.2).build(),
        box_handle,
        &mut rigid_body_set,
    );

    let gravity = vector![0.0, -1.62];
    let mut integration_params = IntegrationParameters::default();
    let mut physics_pipeline = PhysicsPipeline::new();
    let mut island_manager = IslandManager::new();
    let mut broad_phase = DefaultBroadPhase::new();
    let mut narrow_phase = NarrowPhase::new();
    let mut impulse_joint_set = ImpulseJointSet::new();
    let mut multibody_joint_set = MultibodyJointSet::new();
    let mut ccd_solver = CCDSolver::new();
    let mut query_pipeline = QueryPipeline::new();

    let stars: Vec<(f32, f32)> = (0..200).map(|i| {
        let t = i as f32 * 2.399f32;
        (
            ((t * 17.3).sin() * 0.5 + 0.5) * screen_width(),
            ((t * 11.7).cos() * 0.5 + 0.5) * screen_height(),
        )
    }).collect();

    let mut particles: Vec<Particle> = Vec::with_capacity(512);
    let mut smooth_fps = 60.0f32;

    // Pre-compute Y extents over one full period for minimap scaling
    const MM_SAMPLES: usize = 300;
    let (mm_world_y_min, mm_world_y_max) = (0..MM_SAMPLES).fold(
        (f32::INFINITY, f32::NEG_INFINITY),
        |(lo, hi), i| {
            let x  = i as f32 * PERIOD / MM_SAMPLES as f32;
            let cy = cave_center(x);
            let hw = cave_half_width(x);
            (lo.min(cy - hw - 3.0), hi.max(cy + hw + 3.0))
        },
    );
    const MM_HALF_X: f32 = 150.0; // world metres shown each side of ship

    let rock_dark = Color::from_rgba(80,  64,  50,  255);
    let rock_mid  = Color::from_rgba(118, 95,  72,  255);
    let rock_edge = Color::from_rgba(150, 120, 88,  255);

    // Obstacles use the same rock palette as the walls.
    let obs_fill = rock_dark;
    let obs_edge = rock_edge;

    let mut glow = 0.0f32; // 0 = idle, 1 = full thrust

    let light_material = load_material(
        ShaderSource::Glsl { vertex: LIGHT_VERTEX, fragment: LIGHT_FRAGMENT },
        MaterialParams {
            uniforms: vec![
                UniformDesc::new("ship_pos",     UniformType::Float2),
                UniformDesc::new("light_radius", UniformType::Float1),
                UniformDesc::new("glow",         UniformType::Float1),
            ],
            ..Default::default()
        },
    ).expect("cave light shader");

    loop {
        integration_params.dt = get_frame_time().min(0.05);
        physics_pipeline.step(
            &gravity,
            &integration_params,
            &mut island_manager,
            &mut broad_phase,
            &mut narrow_phase,
            &mut rigid_body_set,
            &mut collider_set,
            &mut impulse_joint_set,
            &mut multibody_joint_set,
            &mut ccd_solver,
            Some(&mut query_pipeline),
            &(),
            &(),
        );

        let sh = screen_height();
        let sw = screen_width();

        // Zoom out on narrow screens so more of the cave fits (HUD/minimap are unaffected).
        let view_scale = if sw < 600.0 { SCALE * 0.38 } else { SCALE };
        // Shadow the module-level w2s so all render calls below use view_scale automatically.
        let w2s = |x: f32, y: f32, sh: f32, cam_x: f32, cam_y: f32| -> Vec2 {
            vec2(
                (x - cam_x) * view_scale + sw / 2.0,
                sh / 2.0 - (y - cam_y) * view_scale,
            )
        };

        // UI scale: HUD/minimap were tuned for a ~980px logical width. With the
        // device-width viewport, narrow screens report their true width, so scale
        // fixed-size UI down proportionally (capped at 1.0 so desktop is unchanged).
        let ui = (sw / 980.0).min(1.0);

        // Safe-area insets (notch / status bar), supplied by JS via env(safe-area-inset-*).
        // Keeps the top-left HUD clear of the notch in both portrait (top) and landscape (left).
        let safe_top = f32::from_bits(SAFE_AREA_TOP.load(Ordering::Relaxed));
        let safe_left = f32::from_bits(SAFE_AREA_LEFT.load(Ordering::Relaxed));

        let (cam_x, cam_y, angle, ship_vx, ship_vy) = {
            let body = &rigid_body_set[box_handle];
            let p = body.translation();
            let v = body.linvel();
            (p.x, p.y, body.rotation().angle(), v.x, v.y)
        };

        // Local-to-world helpers (position and direction)
        let lp = |lx: f32, ly: f32| -> (f32, f32) {
            (cam_x + lx * angle.cos() - ly * angle.sin(),
             cam_y + lx * angle.sin() + ly * angle.cos())
        };
        let ld = |lx: f32, ly: f32| -> (f32, f32) {
            (lx * angle.cos() - ly * angle.sin(),
             lx * angle.sin() + ly * angle.cos())
        };

        // Read thrust state early so lighting can use it
        let thrusting_now = is_mouse_button_down(MouseButton::Left)
            || is_key_down(KeyCode::Down)
            || TOUCH_THRUST.load(Ordering::Relaxed) != 0;
        glow += (if thrusting_now { 1.0 } else { 0.0 } - glow) * 0.12;

        // --- Slide the cave window ---
        let ship_seg = (cam_x / SEG_LEN).floor() as i64;
        let want_left  = ship_seg - HALF_WINDOW;
        let want_right = ship_seg + HALF_WINDOW;

        // Evict segments that are too far left
        while cave.front().map_or(false, |&(idx, ..)| idx < want_left) {
            if let Some((_, top, bot)) = cave.pop_front() {
                collider_set.remove(top, &mut island_manager, &mut rigid_body_set, false);
                collider_set.remove(bot, &mut island_manager, &mut rigid_body_set, false);
            }
        }
        // Evict segments that are too far right
        while cave.back().map_or(false, |&(idx, ..)| idx > want_right) {
            if let Some((_, top, bot)) = cave.pop_back() {
                collider_set.remove(top, &mut island_manager, &mut rigid_body_set, false);
                collider_set.remove(bot, &mut island_manager, &mut rigid_body_set, false);
            }
        }
        // Extend left
        while cave.front().map_or(want_left, |&(idx, ..)| idx) > want_left {
            let new_idx = cave.front().map_or(want_left, |&(idx, ..)| idx) - 1;
            let (top, bot) = insert_seg(new_idx, &mut collider_set);
            cave.push_front((new_idx, top, bot));
        }
        // Extend right
        while cave.back().map_or(want_right - 1, |&(idx, ..)| idx) < want_right {
            let new_idx = cave.back().map_or(want_right, |&(idx, ..)| idx) + 1;
            let (top, bot) = insert_seg(new_idx, &mut collider_set);
            cave.push_back((new_idx, top, bot));
        }

        // --- Slide the obstacle window (mirrors the wall window) ---
        let win_left_x  = want_left as f32 * SEG_LEN;
        let win_right_x = (want_right + 1) as f32 * SEG_LEN;
        // Slot index covers position jitter (±3 m) with a margin.
        let k_left  = ((win_left_x  - 3.0) / OBSTACLE_SPACING).floor() as i64;
        let k_right = ((win_right_x + 3.0) / OBSTACLE_SPACING).ceil()  as i64;

        // Evict obstacles whose slot fell outside the window.
        obstacles.retain(|&k, ob| {
            if k < k_left || k > k_right {
                collider_set.remove(ob.handle, &mut island_manager, &mut rigid_body_set, false);
                false
            } else {
                true
            }
        });
        // Load any newly-in-range obstacles.
        for k in k_left..=k_right {
            if !obstacles.contains_key(&k) {
                spawn_obstacle(k, &mut collider_set, &mut obstacles);
            }
        }

        // --- Draw ---
        clear_background(Color::from_rgba(8, 8, 18, 255));

        // Stars
        for &(sx, sy) in &stars {
            let px = (sx - cam_x * view_scale * 0.05).rem_euclid(sw);
            let py = (sy + cam_y * view_scale * 0.05).rem_euclid(sh);
            draw_circle(px, py, 1.0, Color::from_rgba(200, 200, 255, 150));
        }

        // Cave walls
        let far_up   = -sh * 2.0;
        let far_down =  sh * 3.0;
        let margin = sw + view_scale * 4.0;
        let ship_screen = vec2(sw / 2.0, sh / 2.0);
        let base_dim = sw.min(sh);
        let light_radius = base_dim * 0.55 + glow * base_dim * 0.30;

        // Indices for two quads stacked: face-edge, face-mid, fill-to-infinity
        // Each quad = 2 triangles = 6 indices, layout:
        //   0--1
        //   |\ |
        //   | \|
        //   3--2
        let quad_idx: [u16; 6] = [0, 1, 2, 0, 2, 3];
        let wall_indices: Vec<u16> = (0u16..3).flat_map(|q| quad_idx.map(|i| i + q * 4)).collect();

        let v = |p: Vec2, c: Color| -> Vertex {
            Vertex { position: vec3(p.x, p.y, 0.0), uv: vec2(0., 0.), color: c.into(), normal: vec4(0., 0., 1., 0.) }
        };

        // Bind per-pixel radial-light shader for all cave wall draws.
        gl_use_material(&light_material);
        light_material.set_uniform("ship_pos",     ship_screen);
        light_material.set_uniform("light_radius", light_radius);
        light_material.set_uniform("glow",         glow);

        for &(idx, ..) in &cave {
            let (ta, tb, ba, bb) = seg_points(idx);
            let t0 = w2s(ta.x, ta.y, sh, cam_x, cam_y);
            let t1 = w2s(tb.x, tb.y, sh, cam_x, cam_y);

            if t0.x.min(t1.x) > sw + margin || t0.x.max(t1.x) < -margin {
                continue;
            }

            let b0 = w2s(ba.x, ba.y, sh, cam_x, cam_y);
            let b1 = w2s(bb.x, bb.y, sh, cam_x, cam_y);

            // Top wall: three stacked quads (edge → mid → dark fill).
            // All vertices carry raw base colors; the shader applies radial lighting.
            draw_mesh(&Mesh {
                vertices: vec![
                    // quad 0 — bright lit edge face
                    v(t0,                        rock_edge),
                    v(t1,                        rock_edge),
                    v(vec2(t1.x, t1.y + 6.0),   rock_mid),
                    v(vec2(t0.x, t0.y + 6.0),   rock_mid),
                    // quad 1 — mid band
                    v(vec2(t0.x, t0.y + 6.0),   rock_mid),
                    v(vec2(t1.x, t1.y + 6.0),   rock_mid),
                    v(vec2(t1.x, t1.y + 14.0),  rock_dark),
                    v(vec2(t0.x, t0.y + 14.0),  rock_dark),
                    // quad 2 — rock fill (ceiling surface to far off-screen)
                    v(t0,                        rock_dark),
                    v(t1,                        rock_dark),
                    v(vec2(t1.x, far_up),        rock_dark),
                    v(vec2(t0.x, far_up),        rock_dark),
                ],
                indices: wall_indices.clone(),
                texture: None,
            });

            // Bottom wall: three non-overlapping quads, y increases downward
            draw_mesh(&Mesh {
                vertices: vec![
                    // quad 0 — dark→mid band
                    v(vec2(b0.x, b0.y - 14.0), rock_dark),  // TL
                    v(vec2(b1.x, b1.y - 14.0), rock_dark),  // TR
                    v(vec2(b1.x, b1.y -  6.0), rock_mid),   // BR
                    v(vec2(b0.x, b0.y -  6.0), rock_mid),   // BL
                    // quad 1 — edge highlight
                    v(vec2(b0.x, b0.y -  6.0), rock_mid),   // TL
                    v(vec2(b1.x, b1.y -  6.0), rock_mid),   // TR
                    v(b1,                       rock_edge),  // BR
                    v(b0,                       rock_edge),  // BL
                    // quad 2 — rock fill (floor surface to far off-screen)
                    v(b0,                       rock_dark),  // TL
                    v(b1,                       rock_dark),  // TR
                    v(vec2(b1.x, far_down),     rock_dark),  // BR
                    v(vec2(b0.x, far_down),     rock_dark),  // BL
                ],
                indices: wall_indices.clone(),
                texture: None,
            });
        }

        // Obstacles — filled, lit by the same radial shader as the walls.
        // Compute screen-space polygons once and reuse them for the outline.
        let mut obs_screens: Vec<(Vec2, Vec<Vec2>)> = Vec::with_capacity(obstacles.len());
        for ob in obstacles.values() {
            let (c, s) = (ob.rot.cos(), ob.rot.sin());
            let poly: Vec<Vec2> = ob.verts.iter().map(|p| {
                let wx = ob.cx + p.x * c - p.y * s;
                let wy = ob.cy + p.x * s + p.y * c;
                w2s(wx, wy, sh, cam_x, cam_y)
            }).collect();
            let center = w2s(ob.cx, ob.cy, sh, cam_x, cam_y);

            // Cull obstacles fully off-screen.
            let (mut minx, mut maxx) = (f32::INFINITY, f32::NEG_INFINITY);
            for p in &poly { minx = minx.min(p.x); maxx = maxx.max(p.x); }
            if maxx < -margin || minx > sw + margin {
                continue;
            }

            let n = poly.len();
            let mut verts = Vec::with_capacity(n + 1);
            verts.push(v(center, obs_fill));
            for p in &poly { verts.push(v(*p, obs_edge)); }
            let mut indices = Vec::with_capacity(n * 3);
            for i in 0..n as u16 {
                indices.push(0);
                indices.push(1 + i);
                indices.push(1 + (i + 1) % n as u16);
            }
            draw_mesh(&Mesh { vertices: verts, indices, texture: None });

            obs_screens.push((center, poly));
        }

        gl_use_default_material();

        // Crisp outline on top of each obstacle (default material).
        for (_, poly) in &obs_screens {
            let n = poly.len();
            for i in 0..n {
                let a = poly[i];
                let b = poly[(i + 1) % n];
                draw_line(a.x, a.y, b.x, b.y, 1.5, obs_edge);
            }
        }

        // Particles
        for p in &particles {
            let s = w2s(p.x, p.y, sh, cam_x, cam_y);
            let a = (p.life * 255.0) as u8;
            let radius = p.life * if p.kind == 0 { 5.0 } else { 3.0 };
            let color = match p.kind {
                0 => Color::from_rgba(255, (120.0 + p.life * 100.0) as u8, 20, a), // orange flame
                _ => Color::from_rgba(100, 180, 255, a),                             // blue RCS
            };
            draw_circle(s.x, s.y, radius, color);
        }

        // Ship
        let sc = w2s(cam_x, cam_y, sh, cam_x, cam_y);
        draw_rectangle_ex(sc.x, sc.y, view_scale, view_scale, DrawRectangleParams {
            offset: vec2(0.5, 0.5),
            rotation: -angle,
            color: RED,
        });
        let rot = |lx: f32, ly: f32| -> Vec2 {
            w2s(
                cam_x + lx * angle.cos() - ly * angle.sin(),
                cam_y + lx * angle.sin() + ly * angle.cos(),
                sh, cam_x, cam_y,
            )
        };
        draw_triangle(rot(0.0, -0.65), rot(-0.25, -0.45), rot(0.25, -0.45), YELLOW);

        smooth_fps += (get_fps() as f32 - smooth_fps) * 0.05;
        let cave_x = cam_x.rem_euclid(PERIOD);
        draw_text(
            &format!("x={:.0}  {:.0}m/{}m   [R] reset   FPS: {:.0}", cam_x, cave_x, PERIOD as i32, smooth_fps),
            safe_left + 10.0 * ui, safe_top + 206.0 * ui, 36.0 * ui, WHITE,
        );

        // Controls
        let rb = rigid_body_set.get_mut(box_handle).unwrap();
        rb.reset_forces(true);
        rb.reset_torques(true);
        if thrusting_now {
            let a = rb.rotation().angle();
            rb.add_force(vector![-a.sin() * 8.0, a.cos() * 8.0], true);
        }
        let touch_torque = f32::from_bits(TOUCH_TORQUE.load(Ordering::Relaxed));
        let rotating_left  = is_key_down(KeyCode::Left)  || touch_torque < -0.1;
        let rotating_right = is_key_down(KeyCode::Right) || touch_torque >  0.1;
        if rotating_left {
            rb.add_torque(-1.0, true);
        } else if rotating_right {
            rb.add_torque(1.0, true);
        } else {
            rb.add_torque(touch_torque, true);
        }

        // --- Particle emission ---
        let dt = get_frame_time();

        // Main thruster: exhaust exits local -Y (out the bottom), 8 particles/frame
        if thrusting_now {
            for _ in 0..8 {
                let spread = gen_range(-0.25f32, 0.25);
                let (px, py) = lp(spread * 0.3, -0.55);
                let speed = gen_range(4.0f32, 8.0);
                let (dvx, dvy) = ld(spread * 1.5, -speed);
                particles.push(Particle {
                    x: px, y: py,
                    vx: ship_vx + dvx, vy: ship_vy + dvy,
                    life: 1.0, kind: 0,
                });
            }
        }

        // Side RCS thrusters: emit from the side opposite to rotation
        // rotating_left (clockwise) → right-side thruster fires, exhaust exits local +X
        if rotating_left {
            for _ in 0..3 {
                let spread = gen_range(-0.15f32, 0.15);
                let (px, py) = lp(-0.45, -0.55);
                let speed = gen_range(2.0f32, 4.0);
                let (dvx, dvy) = ld(spread, -speed);
                particles.push(Particle {
                    x: px, y: py,
                    vx: ship_vx + dvx, vy: ship_vy + dvy,
                    life: 1.0, kind: 1,
                });
            }
        }
        if rotating_right {
            for _ in 0..3 {
                let spread = gen_range(-0.15f32, 0.15);
                let (px, py) = lp(0.45, -0.55);
                let speed = gen_range(2.0f32, 4.0);
                let (dvx, dvy) = ld(spread, -speed);
                particles.push(Particle {
                    x: px, y: py,
                    vx: ship_vx + dvx, vy: ship_vy + dvy,
                    life: 1.0, kind: 2,
                });
            }
        }

        // Update particles
        let decay_main = dt / 0.5;  // main thruster lives ~0.5s
        let decay_rcs  = dt / 0.3;  // RCS lives ~0.3s
        for p in &mut particles {
            let decay = if p.kind == 0 { decay_main } else { decay_rcs };
            p.life -= decay;
            p.x += p.vx * dt;
            p.y += p.vy * dt;
        }
        particles.retain(|p| p.life > 0.0);

        if is_key_pressed(KeyCode::R) {
            let rb = rigid_body_set.get_mut(box_handle).unwrap();
            rb.set_translation(vector![0.0, cave_center(0.0)], true);
            rb.set_linvel(vector![0.0, 0.0], true);
            rb.set_angvel(0.0, true);
            rb.set_rotation(Rotation::new(0.0), true);
        }

        // --- Minimap (ship always centred) ---
        let mm_w = 480.0f32 * ui;
        let mm_h = 160.0f32 * ui;
        let mm_ox = safe_left + 10.0f32 * ui;
        let mm_oy = safe_top + 10.0f32 * ui;
        let mm_y_range = mm_world_y_max - mm_world_y_min;

        // World → minimap: X is relative to ship, Y uses global extents
        let to_mm_y = |wy: f32| -> f32 {
            mm_oy + mm_h - (wy - mm_world_y_min) / mm_y_range * mm_h
        };

        // Fill with rock, carve cave interior columns sampled around ship
        draw_rectangle(mm_ox, mm_oy, mm_w, mm_h, rock_mid);
        let col_w = mm_w / MM_SAMPLES as f32 + 0.5;
        for i in 0..MM_SAMPLES {
            let x     = cam_x - MM_HALF_X + (i as f32 + 0.5) * (2.0 * MM_HALF_X) / MM_SAMPLES as f32;
            let top   = cave_center(x) + cave_half_width(x);
            let bot   = cave_center(x) - cave_half_width(x);
            let col_x = mm_ox + i as f32 / MM_SAMPLES as f32 * mm_w;
            let top_s = to_mm_y(top).clamp(mm_oy, mm_oy + mm_h);
            let bot_s = to_mm_y(bot).clamp(mm_oy, mm_oy + mm_h);
            draw_rectangle(col_x, top_s, col_w, bot_s - top_s, Color::from_rgba(8, 8, 18, 220));
        }

        // Obstacle shapes on the minimap — actual polygon, not just a dot.
        let to_mm_x = |wx: f32| -> f32 {
            mm_ox + (wx - cam_x + MM_HALF_X) / (2.0 * MM_HALF_X) * mm_w
        };
        for ob in obstacles.values() {
            if (ob.cx - cam_x).abs() > MM_HALF_X + 6.0 {
                continue;
            }
            let (c, s) = (ob.rot.cos(), ob.rot.sin());
            let mm_poly: Vec<Vec2> = ob.verts.iter().map(|p| {
                let wx = ob.cx + p.x * c - p.y * s;
                let wy = ob.cy + p.x * s + p.y * c;
                vec2(
                    to_mm_x(wx).clamp(mm_ox, mm_ox + mm_w),
                    to_mm_y(wy).clamp(mm_oy, mm_oy + mm_h),
                )
            }).collect();
            let mc = vec2(to_mm_x(ob.cx), to_mm_y(ob.cy).clamp(mm_oy, mm_oy + mm_h));
            let n = mm_poly.len();
            for i in 0..n {
                draw_triangle(mc, mm_poly[i], mm_poly[(i + 1) % n], obs_fill);
            }
            for i in 0..n {
                draw_line(mm_poly[i].x, mm_poly[i].y,
                          mm_poly[(i + 1) % n].x, mm_poly[(i + 1) % n].y,
                          1.0, obs_edge);
            }
        }

        // Viewport rectangle — always centred horizontally, Y follows ship
        let vp_hw   = sw / (2.0 * view_scale);
        let vp_hh   = sh / (2.0 * view_scale);
        let vp_mm_hw = vp_hw / MM_HALF_X * (mm_w / 2.0);
        let vp_cx   = mm_ox + mm_w / 2.0;
        let vp_t    = to_mm_y(cam_y + vp_hh).clamp(mm_oy, mm_oy + mm_h);
        let vp_b    = to_mm_y(cam_y - vp_hh).clamp(mm_oy, mm_oy + mm_h);
        draw_rectangle_lines(vp_cx - vp_mm_hw, vp_t, 2.0 * vp_mm_hw, vp_b - vp_t, 1.0,
            Color::from_rgba(255, 255, 255, 180));

        // Ship dot — always at horizontal centre
        draw_circle(vp_cx, to_mm_y(cam_y), 3.0, YELLOW);

        // Border
        draw_rectangle_lines(mm_ox, mm_oy, mm_w, mm_h, 1.0, Color::from_rgba(255, 255, 255, 120));

        next_frame().await;
    }
}
