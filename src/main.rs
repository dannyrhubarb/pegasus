use macroquad::prelude::*;
use macroquad::rand::gen_range;
use rapier2d::prelude::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};

struct Particle {
    x: f32, y: f32,
    vx: f32, vy: f32,
    life: f32,  // 1.0 = fresh, 0.0 = dead
    kind: u8,   // 0 = main thruster, 1 = left RCS, 2 = right RCS
}

static TOUCH_THRUST: AtomicU32 = AtomicU32::new(0);
static TOUCH_TORQUE: AtomicU32 = AtomicU32::new(0);
// Gamepad state lives on its own atomics (not the touch ones) so a connected-but-
// idle controller never stomps an active touch input, and vice versa. The two
// sources are combined in the main loop.
static PAD_THRUST: AtomicU32 = AtomicU32::new(0);
static PAD_TORQUE: AtomicU32 = AtomicU32::new(0);
static PAD_RESET: AtomicU32 = AtomicU32::new(0);
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

// --- Bluetooth / USB game controller bridge (Web Gamepad API, see index.html) ---
#[unsafe(no_mangle)]
pub extern "C" fn set_pad_thrust(active: i32) {
    PAD_THRUST.store(active as u32, Ordering::Relaxed);
}

#[unsafe(no_mangle)]
pub extern "C" fn set_pad_torque(value: f32) {
    PAD_TORQUE.store(value.to_bits(), Ordering::Relaxed);
}

// Edge-triggered reset (Start / Y button). JS sets the flag on a fresh press;
// the loop consumes it with a swap so it fires exactly once.
#[unsafe(no_mangle)]
pub extern "C" fn set_pad_reset() {
    PAD_RESET.store(1, Ordering::Relaxed);
}

#[unsafe(no_mangle)]
pub extern "C" fn set_safe_area(top: f32, left: f32) {
    SAFE_AREA_TOP.store(top.to_bits(), Ordering::Relaxed);
    SAFE_AREA_LEFT.store(left.to_bits(), Ordering::Relaxed);
}

fn window_conf() -> Conf {
    Conf {
        window_title: "Pegasus — Moon Lander".to_string(),
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
// Render scale for the ship mesh relative to the raw SWF coordinates
const SHIP_SCALE: f32 = 1.5;
// Where [R] / gamepad reset drops the ship. Obstacles keep clear of this point
// (like the x = 0 spawn) so a reset never lands on a rock.
const RESET_X: f32 = 64.0;
// Physics runs on a fixed timestep (accumulator in the main loop) so handling
// is identical on every display refresh rate; rendering interpolates the ship
// between the last two steps.
const PHYSICS_DT: f32 = 1.0 / 120.0;

// Ship hull mesh: 41 triangles extracted from the original Flash SWF
// (mcSpaceship, character id 41 in completeHS8replay.swf), ear-clip triangulated
// from the rasterised vector silhouette. Local ship space: +Y = nose/forward,
// origin at hull centroid; full height ≈ 0.95 world units.
const SHIP_TRIS: [[f32; 6]; 41] = [
    [0.000,0.472, -0.022,0.472, -0.047,0.460],
    [0.000,0.472, -0.047,0.460, -0.084,0.421],
    [0.000,0.472, -0.084,0.421, -0.114,0.369],
    [0.000,0.472, -0.114,0.369, -0.143,0.287],
    [0.003,0.475, 0.000,0.472, -0.143,0.287],
    [0.003,0.475, -0.143,0.287, -0.168,0.183],
    [0.003,0.475, -0.168,0.183, -0.178,0.087],
    [0.003,0.475, -0.178,0.087, -0.181,-0.079],
    [0.003,0.475, -0.181,-0.079, -0.171,-0.171],
    [-0.188,-0.181, -0.208,-0.178, -0.237,-0.200],
    [-0.188,-0.181, -0.237,-0.200, -0.260,-0.245],
    [-0.171,-0.171, -0.188,-0.181, -0.260,-0.245],
    [-0.171,-0.171, -0.260,-0.245, -0.270,-0.292],
    [-0.171,-0.171, -0.270,-0.292, -0.270,-0.379],
    [-0.171,-0.171, -0.270,-0.379, -0.250,-0.443],
    [-0.171,-0.171, -0.250,-0.443, -0.250,-0.475],
    [0.003,0.475, -0.171,-0.171, -0.250,-0.475],
    [-0.250,-0.475, -0.153,-0.475, -0.153,-0.448],
    [0.003,0.475, -0.250,-0.475, -0.153,-0.448],
    [0.003,0.475, -0.153,-0.448, -0.114,-0.386],
    [0.003,0.475, -0.114,-0.386, -0.082,-0.440],
    [0.003,0.475, -0.082,-0.440, -0.082,-0.475],
    [0.003,0.475, -0.082,-0.475, 0.082,-0.475],
    [0.003,0.475, 0.082,-0.475, 0.082,-0.448],
    [0.003,0.475, 0.082,-0.448, 0.121,-0.391],
    [0.003,0.475, 0.121,-0.391, 0.151,-0.448],
    [0.151,-0.448, 0.151,-0.475, 0.247,-0.475],
    [0.003,0.475, 0.151,-0.448, 0.247,-0.475],
    [0.247,-0.475, 0.270,-0.364, 0.267,-0.287],
    [0.247,-0.475, 0.267,-0.287, 0.237,-0.200],
    [0.247,-0.475, 0.237,-0.200, 0.208,-0.178],
    [0.247,-0.475, 0.208,-0.178, 0.188,-0.181],
    [0.247,-0.475, 0.188,-0.181, 0.176,-0.171],
    [0.003,0.475, 0.247,-0.475, 0.176,-0.171],
    [0.003,0.475, 0.176,-0.171, 0.185,-0.050],
    [0.003,0.475, 0.185,-0.050, 0.183,0.072],
    [0.003,0.475, 0.183,0.072, 0.171,0.183],
    [0.003,0.475, 0.171,0.183, 0.148,0.287],
    [0.003,0.475, 0.148,0.287, 0.119,0.369],
    [0.003,0.475, 0.119,0.369, 0.089,0.421],
    [0.089,0.421, 0.050,0.460, 0.003,0.475],
];

// Ship detail + accent overlays. Window, leg-pods, engine cup, and gold accent are
// extracted from the SWF; the blue cockpit glass and two flank racing stripes are an
// added accent (the original lander art is plain silver). [ax,ay,bx,by,cx,cy,r,g,b].
const SHIP_DETAILS: [[f32; 9]; 86] = [
    [0.189,-0.181, 0.139,-0.425, 0.146,-0.445, 0.518,0.537,0.588],
    [0.189,-0.181, 0.146,-0.445, 0.251,-0.445, 0.518,0.537,0.588],
    [0.189,-0.181, 0.251,-0.445, 0.257,-0.430, 0.518,0.537,0.588],
    [0.189,-0.181, 0.257,-0.430, 0.262,-0.413, 0.518,0.537,0.588],
    [0.189,-0.181, 0.262,-0.413, 0.266,-0.396, 0.518,0.537,0.588],
    [0.189,-0.181, 0.266,-0.396, 0.268,-0.377, 0.518,0.537,0.588],
    [0.189,-0.181, 0.268,-0.377, 0.270,-0.358, 0.518,0.537,0.588],
    [0.189,-0.181, 0.270,-0.358, 0.271,-0.337, 0.518,0.537,0.588],
    [0.189,-0.181, 0.271,-0.337, 0.270,-0.316, 0.518,0.537,0.588],
    [0.189,-0.181, 0.270,-0.316, 0.268,-0.296, 0.518,0.537,0.588],
    [0.189,-0.181, 0.268,-0.296, 0.265,-0.276, 0.518,0.537,0.588],
    [0.189,-0.181, 0.265,-0.276, 0.261,-0.258, 0.518,0.537,0.588],
    [0.189,-0.181, 0.261,-0.258, 0.256,-0.241, 0.518,0.537,0.588],
    [0.189,-0.181, 0.256,-0.241, 0.249,-0.225, 0.518,0.537,0.588],
    [0.189,-0.181, 0.249,-0.225, 0.242,-0.211, 0.518,0.537,0.588],
    [0.189,-0.181, 0.242,-0.211, 0.234,-0.200, 0.518,0.537,0.588],
    [0.189,-0.181, 0.234,-0.200, 0.226,-0.191, 0.518,0.537,0.588],
    [0.189,-0.181, 0.226,-0.191, 0.217,-0.184, 0.518,0.537,0.588],
    [0.189,-0.181, 0.217,-0.184, 0.208,-0.180, 0.518,0.537,0.588],
    [0.208,-0.180, 0.198,-0.179, 0.189,-0.181, 0.518,0.537,0.588],
    [-0.138,-0.425, -0.189,-0.181, -0.198,-0.179, 0.518,0.537,0.588],
    [-0.138,-0.425, -0.198,-0.179, -0.208,-0.180, 0.518,0.537,0.588],
    [-0.138,-0.425, -0.208,-0.180, -0.217,-0.184, 0.518,0.537,0.588],
    [-0.138,-0.425, -0.217,-0.184, -0.226,-0.191, 0.518,0.537,0.588],
    [-0.138,-0.425, -0.226,-0.191, -0.234,-0.200, 0.518,0.537,0.588],
    [-0.138,-0.425, -0.234,-0.200, -0.242,-0.211, 0.518,0.537,0.588],
    [-0.138,-0.425, -0.242,-0.211, -0.249,-0.225, 0.518,0.537,0.588],
    [-0.138,-0.425, -0.249,-0.225, -0.256,-0.241, 0.518,0.537,0.588],
    [-0.138,-0.425, -0.256,-0.241, -0.261,-0.258, 0.518,0.537,0.588],
    [-0.138,-0.425, -0.261,-0.258, -0.265,-0.276, 0.518,0.537,0.588],
    [-0.138,-0.425, -0.265,-0.276, -0.268,-0.296, 0.518,0.537,0.588],
    [-0.138,-0.425, -0.268,-0.296, -0.270,-0.316, 0.518,0.537,0.588],
    [-0.138,-0.425, -0.270,-0.316, -0.271,-0.337, 0.518,0.537,0.588],
    [-0.138,-0.425, -0.271,-0.337, -0.270,-0.358, 0.518,0.537,0.588],
    [-0.138,-0.425, -0.270,-0.358, -0.268,-0.377, 0.518,0.537,0.588],
    [-0.138,-0.425, -0.268,-0.377, -0.266,-0.396, 0.518,0.537,0.588],
    [-0.138,-0.425, -0.266,-0.396, -0.262,-0.413, 0.518,0.537,0.588],
    [-0.138,-0.425, -0.262,-0.413, -0.257,-0.430, 0.518,0.537,0.588],
    [-0.138,-0.425, -0.257,-0.430, -0.250,-0.445, 0.518,0.537,0.588],
    [-0.250,-0.445, -0.146,-0.445, -0.138,-0.425, 0.518,0.537,0.588],
    [0.119,-0.385, -0.117,-0.379, -0.111,-0.392, 0.471,0.490,0.541],
    [0.119,-0.385, -0.111,-0.392, -0.105,-0.404, 0.471,0.490,0.541],
    [0.119,-0.385, -0.105,-0.404, -0.098,-0.416, 0.471,0.490,0.541],
    [0.119,-0.385, -0.098,-0.416, -0.092,-0.426, 0.471,0.490,0.541],
    [0.119,-0.385, -0.092,-0.426, -0.086,-0.436, 0.471,0.490,0.541],
    [0.119,-0.385, -0.086,-0.436, -0.079,-0.445, 0.471,0.490,0.541],
    [0.119,-0.385, -0.079,-0.445, 0.084,-0.445, 0.471,0.490,0.541],
    [0.119,-0.385, 0.084,-0.445, 0.090,-0.437, 0.471,0.490,0.541],
    [0.119,-0.385, 0.090,-0.437, 0.096,-0.428, 0.471,0.490,0.541],
    [0.119,-0.385, 0.096,-0.428, 0.102,-0.418, 0.471,0.490,0.541],
    [0.119,-0.385, 0.102,-0.418, 0.108,-0.408, 0.471,0.490,0.541],
    [0.108,-0.408, 0.113,-0.397, 0.119,-0.385, 0.471,0.490,0.541],
    [0.083,-0.475, 0.083,-0.445, -0.081,-0.445, 0.776,0.792,0.831],
    [-0.081,-0.445, -0.081,-0.475, 0.083,-0.475, 0.776,0.792,0.831],
    [-0.249,-0.445, -0.249,-0.475, -0.152,-0.475, 0.706,0.588,0.204],
    [-0.152,-0.475, -0.152,-0.445, -0.249,-0.445, 0.706,0.588,0.204],
    [0.047,0.244, 0.040,0.256, 0.033,0.266, 0.227,0.510,0.784],
    [0.047,0.244, 0.033,0.266, 0.026,0.273, 0.227,0.510,0.784],
    [0.047,0.244, 0.026,0.273, 0.019,0.279, 0.227,0.510,0.784],
    [0.047,0.244, 0.019,0.279, 0.011,0.282, 0.227,0.510,0.784],
    [0.047,0.244, 0.011,0.282, 0.002,0.283, 0.227,0.510,0.784],
    [0.047,0.244, 0.002,0.283, -0.006,0.282, 0.227,0.510,0.784],
    [0.047,0.244, -0.006,0.282, -0.014,0.279, 0.227,0.510,0.784],
    [0.047,0.244, -0.014,0.279, -0.021,0.273, 0.227,0.510,0.784],
    [0.047,0.244, -0.021,0.273, -0.028,0.266, 0.227,0.510,0.784],
    [0.047,0.244, -0.028,0.266, -0.035,0.256, 0.227,0.510,0.784],
    [0.047,0.244, -0.035,0.256, -0.041,0.244, 0.227,0.510,0.784],
    [0.047,0.244, -0.041,0.244, -0.047,0.230, 0.227,0.510,0.784],
    [0.047,0.244, -0.047,0.230, -0.052,0.216, 0.227,0.510,0.784],
    [0.047,0.244, -0.052,0.216, -0.055,0.200, 0.227,0.510,0.784],
    [0.047,0.244, -0.055,0.200, -0.058,0.184, 0.227,0.510,0.784],
    [0.047,0.244, -0.058,0.184, -0.059,0.166, 0.227,0.510,0.784],
    [0.047,0.244, -0.059,0.166, -0.060,0.148, 0.227,0.510,0.784],
    [0.047,0.244, -0.060,0.148, -0.059,0.125, 0.227,0.510,0.784],
    [0.047,0.244, -0.059,0.125, 0.064,0.125, 0.227,0.510,0.784],
    [0.047,0.244, 0.064,0.125, 0.065,0.148, 0.227,0.510,0.784],
    [0.047,0.244, 0.065,0.148, 0.064,0.166, 0.227,0.510,0.784],
    [0.047,0.244, 0.064,0.166, 0.063,0.184, 0.227,0.510,0.784],
    [0.047,0.244, 0.063,0.184, 0.060,0.200, 0.227,0.510,0.784],
    [0.047,0.244, 0.060,0.200, 0.057,0.216, 0.227,0.510,0.784],
    [0.057,0.216, 0.052,0.230, 0.047,0.244, 0.227,0.510,0.784],
    [0.000,0.270, 0.028,0.205, -0.028,0.205, 0.490,0.776,0.925],
    [-0.150,0.130, -0.118,0.130, -0.140,-0.150, 0.188,0.486,0.816],
    [-0.150,0.130, -0.140,-0.150, -0.172,-0.150, 0.188,0.486,0.816],
    [0.118,0.130, 0.150,0.130, 0.172,-0.150, 0.188,0.486,0.816],
    [0.118,0.130, 0.172,-0.150, 0.140,-0.150, 0.188,0.486,0.816],
];

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
    float warm    = glow * falloff * 0.12;

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

fn insert_seg(idx: i64, layer: i64, collider_set: &mut ColliderSet) -> Vec<ColliderHandle> {
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

const V_PERIOD: f32 = 90.0;         // vertical repeat distance (m)
const SHAFT_SPACING_SEGS: i64 = 50; // one shaft slot every 150 m (4 per PERIOD)
const SHAFT_BASE_SEG: i64 = 35;     // slot anchor; keeps openings clear of x = 0 / 64
const SHAFT_OPEN_SEGS: i64 = 3;     // opening width: 3 segments = 9 m
const SHAFT_STEP: f32 = 3.0;        // shaft wall segment length (m)

// Start segment of the ceiling/floor opening for shaft slot `s`. Jitter is
// keyed on s mod 4 (= slots per PERIOD) so the pattern repeats exactly every
// period in x — the wrap stays seamless in both axes.
fn shaft_open_seg(s: i64) -> i64 {
    let j = (hash_u32(s.rem_euclid(4) as u32 ^ 0x51ed_270b) % 13) as i64 - 6; // ±6 segs
    s * SHAFT_SPACING_SEGS + SHAFT_BASE_SEG + j
}

// Does cave segment `idx` fall inside a shaft opening (→ walls removed there)?
fn seg_in_opening(idx: i64) -> bool {
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
fn shaft_wall_x(s: i64, side: u8, t: f32) -> f32 {
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
fn shaft_wall_pts(s: i64, gap: i64, side: u8) -> Vec<Vec2> {
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

// Facet lattice point for shaft walls (vertical analogue of lattice_point):
// depth col 0 sits exactly on the wall polyline (collider-aligned); deeper
// cols recede horizontally into the rock with deterministic jitter. Near the
// two ends the deep cols are additionally pulled along the shaft toward the
// junction rock, so corner facets turn diagonally into the corner wedge
// instead of poking past the cave wall line into the cave interior.
fn shaft_lattice(pts: &[Vec2], s: i64, i: usize, d: usize, side: u8) -> Vec2 {
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

// --- Low-poly faceted wall lattice ---------------------------------------
// The cave walls are rendered as a grid of flat-shaded triangles ("facets").
// Geometry is a pure function of a GLOBAL column index so the shared boundary
// between adjacent segments is computed identically on both sides — no cracks.

const SUBCOLS: i64 = 2;                       // sub-columns per 3 m segment → ~1.5 m facets
const COL_DX: f32 = SEG_LEN / SUBCOLS as f32; // world width of one facet column
const ROW_DEPTHS: [f32; 4] = [0.0, 1.0, 3.0, 6.5]; // metres into rock; row 0 on the edge
const N_ROWS: usize = 4;

// World x for a global facet column. Pure function → identical on both sides
// of any segment boundary, so adjacent strips share an exact x.
fn col_x(col: i64) -> f32 {
    col as f32 * COL_DX
}

// World-space lattice point for (col, row, side). Row 0 sits EXACTLY on the
// wall edge (collider-aligned, no jitter); deeper rows recede into the rock
// with small deterministic jitter for the faceted look.
// side 0 = ceiling (rock is +y), side 1 = floor (rock is -y).
fn lattice_point(col: i64, row: usize, side: u8) -> Vec2 {
    let x = col_x(col);
    let edge_y = if side == 0 {
        cave_center(x) + cave_half_width(x)
    } else {
        cave_center(x) - cave_half_width(x)
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
fn draw_flat_mesh(vertices: Vec<Vertex>) {
    if vertices.is_empty() {
        return;
    }
    debug_assert!(vertices.len() <= u16::MAX as usize, "mesh exceeds u16 index range");
    let indices: Vec<u16> = (0..vertices.len() as u16).collect();
    draw_mesh(&Mesh { vertices, indices, texture: None });
}

// Flat-shade color for a wall facet: a band base color (by row) modulated by a
// deterministic per-facet brightness so each triangle reads as a distinct facet.
fn facet_shade(base: Color, col: i64, row: usize, side: u8, salt: u32) -> Color {
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

#[macroquad::main(window_conf)]
async fn main() {
    let mut rigid_body_set = RigidBodySet::new();
    let mut collider_set = ColliderSet::new();

    // Sliding 2D window of cave wall colliders, keyed by (layer, segment idx).
    // Value holds that segment's colliders (empty where a shaft opening removes
    // both walls). Filled by the window-sync code on the first frame.
    let mut cave: HashMap<(i64, i64), Vec<ColliderHandle>> = HashMap::new();

    // Loaded vertical shafts, keyed by (slot, gap): the shaft for slot `s`
    // connecting layer `gap`'s ceiling to layer `gap + 1`'s floor.
    struct Shaft {
        handles: Vec<ColliderHandle>,
        walls: [Vec<Vec2>; 2], // left / right wall polylines, world space
    }
    let mut shafts: HashMap<(i64, i64), Shaft> = HashMap::new();

    let spawn_shaft = |s: i64, gap: i64, collider_set: &mut ColliderSet,
                           shafts: &mut HashMap<(i64, i64), Shaft>| {
        let walls = [shaft_wall_pts(s, gap, 0), shaft_wall_pts(s, gap, 1)];
        let mut handles = Vec::new();
        for pts in &walls {
            for w in pts.windows(2) {
                handles.push(collider_set.insert(
                    ColliderBuilder::segment(point![w[0].x, w[0].y], point![w[1].x, w[1].y])
                        .friction(0.0)
                        .build(),
                ));
            }
        }
        shafts.insert((s, gap), Shaft { handles, walls });
    };

    // Loaded obstacles, keyed by (slot index, layer). Each carries its collider
    // handle plus the hull vertices (local space) used for rendering.
    struct Obstacle {
        handle: ColliderHandle,
        cx: f32,
        cy: f32,
        rot: f32,
        verts: Vec<Vec2>,
    }
    let mut obstacles: HashMap<(i64, i64), Obstacle> = HashMap::new();

    // Insert the obstacle for slot `k` in cave layer `layer` (if any).
    let spawn_obstacle = |k: i64, layer: i64, collider_set: &mut ColliderSet,
                              obstacles: &mut HashMap<(i64, i64), Obstacle>| {
        let Some(spec) = obstacle_spec(k) else { return };
        let Some(builder) = ColliderBuilder::convex_hull(&spec.pts) else { return };
        let cy = spec.cy + layer as f32 * V_PERIOD;
        let handle = collider_set.insert(
            builder
                .translation(vector![spec.cx, cy])
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
        obstacles.insert((k, layer), Obstacle {
            handle,
            cx: spec.cx,
            cy,
            rot: spec.rot,
            verts,
        });
    };

    // Ship starts at cave centre
    let box_body = RigidBodyBuilder::dynamic()
        .translation(vector![0.0, cave_center(0.0)])
        .angular_damping(3.0)
        .ccd_enabled(true)
        .build();
    let box_handle = rigid_body_set.insert(box_body);
    // Compound collider of three capsules (stadium shapes) tracing the 1.5× scaled
    // lander: a rounded fuselage + two splayed leg-pods. Capsules are the closest
    // primitive Rapier offers to an ellipse, so they hug the rounded hull tighter
    // than boxes and slide off rocks without corners catching. Endpoints are in
    // scaled world units (ship-local frame).
    // Fuselage: vertical capsule, rounded nose to mid-hull.
    collider_set.insert_with_parent(
        ColliderBuilder::new(SharedShape::capsule(
            point![0.0, 0.42], point![0.0, -0.08], 0.26))
            .restitution(0.2).build(),
        box_handle, &mut rigid_body_set,
    );
    // Left leg pod: capsule angled out to the foot.
    collider_set.insert_with_parent(
        ColliderBuilder::new(SharedShape::capsule(
            point![-0.26, -0.30], point![-0.33, -0.64], 0.09))
            .restitution(0.2).build(),
        box_handle, &mut rigid_body_set,
    );
    // Right leg pod, mirrored.
    collider_set.insert_with_parent(
        ColliderBuilder::new(SharedShape::capsule(
            point![0.26, -0.30], point![0.33, -0.64], 0.09))
            .restitution(0.2).build(),
        box_handle, &mut rigid_body_set,
    );

    let gravity = vector![0.0, -1.62];
    let mut integration_params = IntegrationParameters::default();
    integration_params.num_solver_iterations = std::num::NonZeroUsize::new(8).unwrap();
    let mut physics_pipeline = PhysicsPipeline::new();
    let mut island_manager = IslandManager::new();
    let mut broad_phase = DefaultBroadPhase::new();
    let mut narrow_phase = NarrowPhase::new();
    let mut impulse_joint_set = ImpulseJointSet::new();
    let mut multibody_joint_set = MultibodyJointSet::new();
    let mut ccd_solver = CCDSolver::new();
    let mut query_pipeline = QueryPipeline::new();

    // Normalized [0,1) star field — scaled to the current screen size each frame so
    // it fills the whole viewport in any orientation. (Storing absolute pixel coords
    // captured the startup size, leaving a gap after rotating to a wider screen.)
    let stars: Vec<(f32, f32)> = (0..200).map(|i| {
        let t = i as f32 * 2.399f32;
        (
            (t * 17.3).sin() * 0.5 + 0.5,
            (t * 11.7).cos() * 0.5 + 0.5,
        )
    }).collect();

    let mut particles: Vec<Particle> = Vec::with_capacity(512);
    let mut smooth_fps = 60.0f32;

    // Minimap window: world metres shown around the ship. MM_HALF_Y keeps the
    // same world-per-pixel scale as x (map is 480×160 px → 3:1, 300×100 m).
    const MM_SAMPLES: usize = 300;
    const MM_HALF_X: f32 = 150.0;
    const MM_HALF_Y: f32 = 50.0;

    let rock_dark = Color::from_rgba(28,  38,  58,  255); // deep navy-slate
    let rock_mid  = Color::from_rgba(52,  68,  96,  255); // mid slate-blue
    let rock_edge = Color::from_rgba(92,  116, 150, 255); // lit cool edge

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

    integration_params.dt = PHYSICS_DT;
    let mut phys_accum = 0.0f32;
    // Ship state at the previous physics step, for render interpolation.
    let mut prev_ship = (0.0f32, cave_center(0.0), 0.0f32); // x, y, angle

    loop {
        // Fixed-timestep accumulator: the cap bounds catch-up work after a
        // hitch (same role as the old per-frame dt cap of 0.05 s).
        phys_accum = (phys_accum + get_frame_time()).min(0.05);
        while phys_accum >= PHYSICS_DT {
            {
                let body = &rigid_body_set[box_handle];
                prev_ship = (body.translation().x, body.translation().y, body.rotation().angle());
            }
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
            phys_accum -= PHYSICS_DT;
        }

        let sh = screen_height();
        let sw = screen_width();

        // Zoom out on small (mobile) screens so more of the cave fits (HUD/minimap are
        // unaffected). Keyed on the *smaller* screen dimension so a phone keeps the same
        // zoom in portrait and landscape — a phone's narrow side stays small in both
        // orientations, whereas `sw` alone would flip to desktop zoom on rotation.
        let view_scale = if sw.min(sh) < 600.0 { SCALE * 0.38 } else { SCALE };
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
        // Keyed on the *smaller* dimension so a phone keeps the same HUD/minimap size
        // across portrait/landscape — `sw` alone grew the minimap on rotation.
        let ui = (sw.min(sh) / 980.0).min(1.0);

        // Safe-area insets (notch / status bar), supplied by JS via env(safe-area-inset-*).
        // Keeps the top-left HUD clear of the notch in both portrait (top) and landscape (left).
        let safe_top = f32::from_bits(SAFE_AREA_TOP.load(Ordering::Relaxed));
        let safe_left = f32::from_bits(SAFE_AREA_LEFT.load(Ordering::Relaxed));

        let (cam_x, cam_y, angle, ship_vx, ship_vy) = {
            let body = &rigid_body_set[box_handle];
            let p = body.translation();
            let v = body.linvel();
            // Interpolate between the last two physics steps so rendering
            // stays smooth when the frame rate and PHYSICS_DT don't divide
            // evenly (e.g. 144 Hz display over a 120 Hz simulation).
            let alpha = (phys_accum / PHYSICS_DT).clamp(0.0, 1.0);
            let (px, py, pa) = prev_ship;
            let mut da = body.rotation().angle() - pa;
            if da > std::f32::consts::PI { da -= std::f32::consts::TAU; }
            if da < -std::f32::consts::PI { da += std::f32::consts::TAU; }
            (
                px + (p.x - px) * alpha,
                py + (p.y - py) * alpha,
                pa + da * alpha,
                v.x, v.y,
            )
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
            || TOUCH_THRUST.load(Ordering::Relaxed) != 0
            || PAD_THRUST.load(Ordering::Relaxed) != 0;
        glow += (if thrusting_now { 1.0 } else { 0.0 } - glow) * 0.12;

        // --- Slide the cave window (2D: segments in x, layers in y) ---
        let ship_seg = (cam_x / SEG_LEN).floor() as i64;
        let want_left  = ship_seg - HALF_WINDOW;
        let want_right = ship_seg + HALF_WINDOW;
        let ship_layer = (cam_y / V_PERIOD).round() as i64;
        let (lay_lo, lay_hi) = (ship_layer - 1, ship_layer + 1);

        cave.retain(|&(layer, idx), handles| {
            if layer < lay_lo || layer > lay_hi || idx < want_left || idx > want_right {
                for h in handles.drain(..) {
                    collider_set.remove(h, &mut island_manager, &mut rigid_body_set, false);
                }
                false
            } else {
                true
            }
        });
        for layer in lay_lo..=lay_hi {
            for idx in want_left..=want_right {
                cave.entry((layer, idx))
                    .or_insert_with(|| insert_seg(idx, layer, &mut collider_set));
            }
        }

        // --- Slide the shaft window ---
        // Shafts for the gap below and above the ship's layer cover everything
        // reachable within half a vertical period.
        let s_lo = want_left.div_euclid(SHAFT_SPACING_SEGS) - 1;
        let s_hi = want_right.div_euclid(SHAFT_SPACING_SEGS) + 1;
        shafts.retain(|&(s, gap), sh| {
            if s < s_lo || s > s_hi || gap < ship_layer - 1 || gap > ship_layer {
                for h in sh.handles.drain(..) {
                    collider_set.remove(h, &mut island_manager, &mut rigid_body_set, false);
                }
                false
            } else {
                true
            }
        });
        for s in s_lo..=s_hi {
            for gap in [ship_layer - 1, ship_layer] {
                if !shafts.contains_key(&(s, gap)) {
                    spawn_shaft(s, gap, &mut collider_set, &mut shafts);
                }
            }
        }

        // --- Slide the obstacle window (mirrors the wall window) ---
        let win_left_x  = want_left as f32 * SEG_LEN;
        let win_right_x = (want_right + 1) as f32 * SEG_LEN;
        // Slot index covers position jitter (±3 m) with a margin.
        let k_left  = ((win_left_x  - 3.0) / OBSTACLE_SPACING).floor() as i64;
        let k_right = ((win_right_x + 3.0) / OBSTACLE_SPACING).ceil()  as i64;

        // Evict obstacles whose slot or layer fell outside the window.
        obstacles.retain(|&(k, layer), ob| {
            if k < k_left || k > k_right || layer < lay_lo || layer > lay_hi {
                collider_set.remove(ob.handle, &mut island_manager, &mut rigid_body_set, false);
                false
            } else {
                true
            }
        });
        // Load any newly-in-range obstacles.
        for layer in lay_lo..=lay_hi {
            for k in k_left..=k_right {
                if !obstacles.contains_key(&(k, layer)) {
                    spawn_obstacle(k, layer, &mut collider_set, &mut obstacles);
                }
            }
        }

        // --- Draw ---
        clear_background(Color::from_rgba(8, 8, 18, 255));

        // Stars
        for &(sx, sy) in &stars {
            let px = (sx * sw - cam_x * view_scale * 0.05).rem_euclid(sw);
            let py = (sy * sh + cam_y * view_scale * 0.05).rem_euclid(sh);
            draw_circle(px, py, 1.0, Color::from_rgba(200, 200, 255, 150));
        }

        // Cave walls. Cull pad: 4 m of world keeps jittered deep-row facets from
        // popping at the screen edge without tessellating a whole extra screen.
        let margin = view_scale * 4.0;
        let ship_screen = vec2(sw / 2.0, sh / 2.0);
        let base_dim = sw.min(sh);
        let light_radius = base_dim * 0.55 + glow * base_dim * 0.30;

        let v = |p: Vec2, c: Color| -> Vertex {
            Vertex { position: vec3(p.x, p.y, 0.0), uv: vec2(0., 0.), color: c.into(), normal: vec4(0., 0., 1., 0.) }
        };

        // Bind per-pixel radial-light shader for all cave wall draws.
        gl_use_material(&light_material);
        light_material.set_uniform("ship_pos",     ship_screen);
        light_material.set_uniform("light_radius", light_radius);
        light_material.set_uniform("glow",         glow);

        // Faceted cave walls, one layer per V_PERIOD. Each wall (ceiling = side 0,
        // floor = side 1) is one continuous mesh of flat-shaded triangles spanning
        // all visible columns. Lattice positions are pure functions of the GLOBAL
        // column index, so adjacent segments share exact boundary vertices (no
        // cracks); row 0 sits on the wall line (= the collider) so the lit surface
        // stays aligned. Columns inside shaft openings are skipped — the shaft
        // rendering below covers that rock. The rock between two stacked layers
        // is closed by a world-bounded fill emitted with each layer's ceiling
        // (deepest ceiling row up to the NEXT layer's deepest floor row).
        let col_lo = want_left * SUBCOLS;
        let col_hi = (want_right + 1) * SUBCOLS;

        for layer in lay_lo..=lay_hi {
            let ly = layer as f32 * V_PERIOD;
            // Vertical culling: facet bands live within ±45 m of the layer line;
            // the inter-layer fill spans [ly, ly + V_PERIOD].
            let facets_visible = {
                let top = w2s(0.0, ly + 45.0, sh, cam_x, cam_y).y;
                let bot = w2s(0.0, ly - 45.0, sh, cam_x, cam_y).y;
                bot > -100.0 && top < sh + 100.0
            };
            // The fill quad can reach ~13 m past the layer lines where the
            // ceiling/floor curves dip, so the band is padded by 15 m.
            let fill_visible = {
                let top = w2s(0.0, ly + V_PERIOD + 15.0, sh, cam_x, cam_y).y;
                let bot = w2s(0.0, ly - 15.0, sh, cam_x, cam_y).y;
                bot > -100.0 && top < sh + 100.0
            };
            if !facets_visible && !fill_visible {
                continue;
            }
            for side in [0u8, 1u8] {
                if side == 1 && !facets_visible {
                    continue;
                }
                let mut verts: Vec<Vertex> = Vec::new();
                for col in col_lo..col_hi {
                    // Shaft opening: no wall here (the shaft's rock covers it).
                    if seg_in_opening(col.div_euclid(SUBCOLS)) {
                        continue;
                    }
                    // Cull columns fully off-screen in x.
                    let sx0 = w2s(col_x(col),     0.0, sh, cam_x, cam_y).x;
                    let sx1 = w2s(col_x(col + 1), 0.0, sh, cam_x, cam_y).x;
                    if sx0.min(sx1) > sw + margin || sx0.max(sx1) < -margin {
                        continue;
                    }

                    // Facet rows: each cell is two flat-shaded triangles.
                    if facets_visible {
                        for row in 0..N_ROWS - 1 {
                            let w00 = lattice_point(col,     row,     side);
                            let w10 = lattice_point(col + 1, row,     side);
                            let w11 = lattice_point(col + 1, row + 1, side);
                            let w01 = lattice_point(col,     row + 1, side);
                            let s00 = w2s(w00.x, w00.y + ly, sh, cam_x, cam_y);
                            let s10 = w2s(w10.x, w10.y + ly, sh, cam_x, cam_y);
                            let s11 = w2s(w11.x, w11.y + ly, sh, cam_x, cam_y);
                            let s01 = w2s(w01.x, w01.y + ly, sh, cam_x, cam_y);

                            let base = match row { 0 => rock_edge, 1 => rock_mid, _ => rock_dark };
                            let ca = facet_shade(base, col, row, side, 0);
                            let cb = facet_shade(base, col, row, side, 0x5bd1_e995);

                            // Hashed diagonal so the lattice doesn't read as a regular grid.
                            if hash_u32(col as u32 ^ (row as u32).wrapping_mul(2654435761)) & 1 == 0 {
                                verts.push(v(s00, ca)); verts.push(v(s10, ca)); verts.push(v(s11, ca));
                                verts.push(v(s00, cb)); verts.push(v(s11, cb)); verts.push(v(s01, cb));
                            } else {
                                verts.push(v(s00, ca)); verts.push(v(s10, ca)); verts.push(v(s01, ca));
                                verts.push(v(s10, cb)); verts.push(v(s11, cb)); verts.push(v(s01, cb));
                            }
                        }
                    }

                    // Solid dark fill closing the rock between this layer's
                    // ceiling and the next layer's floor (shared lattice points
                    // with both facet bands → no cracks).
                    if side == 0 && fill_visible {
                        let wd0 = lattice_point(col,     N_ROWS - 1, 0);
                        let wd1 = lattice_point(col + 1, N_ROWS - 1, 0);
                        let wu0 = lattice_point(col,     N_ROWS - 1, 1);
                        let wu1 = lattice_point(col + 1, N_ROWS - 1, 1);
                        let sd0 = w2s(wd0.x, wd0.y + ly, sh, cam_x, cam_y);
                        let sd1 = w2s(wd1.x, wd1.y + ly, sh, cam_x, cam_y);
                        let su0 = w2s(wu0.x, wu0.y + ly + V_PERIOD, sh, cam_x, cam_y);
                        let su1 = w2s(wu1.x, wu1.y + ly + V_PERIOD, sh, cam_x, cam_y);
                        verts.push(v(sd0, rock_dark)); verts.push(v(sd1, rock_dark)); verts.push(v(su1, rock_dark));
                        verts.push(v(sd0, rock_dark)); verts.push(v(su1, rock_dark)); verts.push(v(su0, rock_dark));
                    }
                }

                draw_flat_mesh(verts);
            }
        }

        // Vertical shaft walls — same faceted treatment rotated 90°: depth cols
        // recede horizontally into the rock, rows run along y. Col 0 sits exactly
        // on the wall polyline (= the colliders), and a solid fill extends past
        // the deepest col to blend into the inter-layer rock fill.
        for (&(s, _gap), shaft) in shafts.iter() {
            for side in [0u8, 1u8] {
                let pts = &shaft.walls[side as usize];
                let dir = if side == 0 { -1.0f32 } else { 1.0 };
                let edge_x = pts[0].x;
                // Cull walls fully off-screen in x (facets + fill span ~16 m).
                let sx0 = w2s(edge_x - 16.0, 0.0, sh, cam_x, cam_y).x;
                let sx1 = w2s(edge_x + 16.0, 0.0, sh, cam_x, cam_y).x;
                if sx0.min(sx1) > sw + margin || sx0.max(sx1) < -margin {
                    continue;
                }
                let fill_x = edge_x + dir * 15.0;
                let mut verts: Vec<Vertex> = Vec::new();
                for i in 0..pts.len() - 1 {
                    // Cull rows fully off-screen in y. Corner facets near the
                    // shaft ends are pulled up to ROW_DEPTHS along the shaft,
                    // so pad by ~8 m worth of pixels.
                    let pad = 8.0 * view_scale;
                    let sy0 = w2s(0.0, pts[i].y,     sh, cam_x, cam_y).y;
                    let sy1 = w2s(0.0, pts[i + 1].y, sh, cam_x, cam_y).y;
                    if sy0.max(sy1) < -pad || sy0.min(sy1) > sh + pad {
                        continue;
                    }
                    let key = s.wrapping_mul(4096) ^ i as i64;
                    for d in 0..N_ROWS - 1 {
                        let w00 = shaft_lattice(pts, s, i,     d,     side);
                        let w10 = shaft_lattice(pts, s, i + 1, d,     side);
                        let w11 = shaft_lattice(pts, s, i + 1, d + 1, side);
                        let w01 = shaft_lattice(pts, s, i,     d + 1, side);
                        let s00 = w2s(w00.x, w00.y, sh, cam_x, cam_y);
                        let s10 = w2s(w10.x, w10.y, sh, cam_x, cam_y);
                        let s11 = w2s(w11.x, w11.y, sh, cam_x, cam_y);
                        let s01 = w2s(w01.x, w01.y, sh, cam_x, cam_y);

                        let base = match d { 0 => rock_edge, 1 => rock_mid, _ => rock_dark };
                        let ca = facet_shade(base, key, d, 2 + side, 0);
                        let cb = facet_shade(base, key, d, 2 + side, 0x5bd1_e995);

                        if hash_u32(key as u32 ^ (d as u32).wrapping_mul(2654435761)) & 1 == 0 {
                            verts.push(v(s00, ca)); verts.push(v(s10, ca)); verts.push(v(s11, ca));
                            verts.push(v(s00, cb)); verts.push(v(s11, cb)); verts.push(v(s01, cb));
                        } else {
                            verts.push(v(s00, ca)); verts.push(v(s10, ca)); verts.push(v(s01, ca));
                            verts.push(v(s10, cb)); verts.push(v(s11, cb)); verts.push(v(s01, cb));
                        }
                    }

                    // Solid fill from the deepest col out into the rock.
                    let wd0 = shaft_lattice(pts, s, i,     N_ROWS - 1, side);
                    let wd1 = shaft_lattice(pts, s, i + 1, N_ROWS - 1, side);
                    let sd0 = w2s(wd0.x, wd0.y, sh, cam_x, cam_y);
                    let sd1 = w2s(wd1.x, wd1.y, sh, cam_x, cam_y);
                    let f0 = w2s(fill_x, wd0.y, sh, cam_x, cam_y);
                    let f1 = w2s(fill_x, wd1.y, sh, cam_x, cam_y);
                    verts.push(v(sd0, rock_dark)); verts.push(v(sd1, rock_dark)); verts.push(v(f1, rock_dark));
                    verts.push(v(sd0, rock_dark)); verts.push(v(f1, rock_dark)); verts.push(v(f0, rock_dark));
                }

                draw_flat_mesh(verts);
            }
        }

        // Obstacles — faceted pebbles lit by the same radial shader as the walls.
        // Same hull→inset ring + center fan topology as before (outer ring = the
        // exact hull = collider), but each triangle is FLAT-shaded with a
        // deterministic per-facet brightness plus a fake top-light gradient, so
        // boulders read as low-poly rocks with brighter tops.
        const BEVEL: f32 = 16.0;
        // Sorted keys, not HashMap order: adjacent boulders can overlap, and
        // map iteration order changes as the window slides, which would flip
        // their z-order mid-flight.
        let mut obstacle_keys: Vec<(i64, i64)> = obstacles.keys().copied().collect();
        obstacle_keys.sort_unstable();
        for &(k, layer) in &obstacle_keys {
            let ob = &obstacles[&(k, layer)];
            let (c, s) = (ob.rot.cos(), ob.rot.sin());
            let poly: Vec<Vec2> = ob.verts.iter().map(|p| {
                let wx = ob.cx + p.x * c - p.y * s;
                let wy = ob.cy + p.x * s + p.y * c;
                w2s(wx, wy, sh, cam_x, cam_y)
            }).collect();
            let center = w2s(ob.cx, ob.cy, sh, cam_x, cam_y);

            // Cull obstacles fully off-screen (other layers' copies are ~V_PERIOD
            // away in y, so the y check drops nearly all of them).
            let (mut minx, mut maxx) = (f32::INFINITY, f32::NEG_INFINITY);
            let (mut miny, mut maxy) = (f32::INFINITY, f32::NEG_INFINITY);
            for p in &poly {
                minx = minx.min(p.x); maxx = maxx.max(p.x);
                miny = miny.min(p.y); maxy = maxy.max(p.y);
            }
            if maxx < -margin || minx > sw + margin || maxy < -margin || miny > sh + margin {
                continue;
            }

            let n = poly.len();

            // Inset polygon: each vertex pulled BEVEL px toward the centroid.
            let inset: Vec<Vec2> = poly.iter().map(|p| {
                let d = center - *p;
                let len = d.length();
                *p + d * (BEVEL / len).min(0.5)
            }).collect();

            // Screen radius for normalising the top-light gradient.
            let radius_px = poly.iter()
                .map(|p| (center - *p).length())
                .fold(1.0f32, f32::max);

            // Flat-shade a facet: base colour × stable per-facet brightness
            // (keyed on the obstacle slot + edge, so it never flickers as the
            // boulder rotates) × top-light gradient (higher on screen = brighter).
            let facet = |base: Color, edge: usize, salt: u32, tri_cy: f32| -> Color {
                let h = hash_u32((k as u32).wrapping_mul(2654435761) ^ (edge as u32) ^ salt);
                let bj = 0.85 + (h & 0xffff) as f32 / 65535.0 * 0.28;
                let g = 1.0 + ((center.y - tri_cy) / radius_px).clamp(-1.0, 1.0) * 0.18;
                let b = bj * g;
                Color::new((base.r * b).min(1.0), (base.g * b).min(1.0), (base.b * b).min(1.0), 1.0)
            };

            let mut verts: Vec<Vertex> = Vec::with_capacity(n * 9);
            for i in 0..n {
                let j = (i + 1) % n;
                // Bevel ring — two flat-shaded triangles per edge.
                let ring_cy = (poly[i].y + poly[j].y + inset[j].y + inset[i].y) * 0.25;
                let c_edge = facet(rock_edge, i, 0, ring_cy);
                let c_mid  = facet(rock_mid,  i, 0x9e37_79b9, ring_cy);
                verts.push(v(poly[i], c_edge)); verts.push(v(poly[j], c_edge)); verts.push(v(inset[j], c_edge));
                verts.push(v(poly[i], c_mid));  verts.push(v(inset[j], c_mid)); verts.push(v(inset[i], c_mid));
                // Inner fan triangle.
                let fan_cy = (inset[i].y + inset[j].y + center.y) / 3.0;
                let c_fan = facet(rock_mid, i, 0x85eb_ca6b, fan_cy);
                verts.push(v(center, c_fan)); verts.push(v(inset[i], c_fan)); verts.push(v(inset[j], c_fan));
            }
            draw_flat_mesh(verts);
        }

        gl_use_default_material();

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

        // Ship — vector spaceship
        let rot = |lx: f32, ly: f32| -> Vec2 {
            let sx = lx * SHIP_SCALE;
            let sy = ly * SHIP_SCALE;
            w2s(
                cam_x + sx * angle.cos() - sy * angle.sin(),
                cam_y + sx * angle.sin() + sy * angle.cos(),
                sh, cam_x, cam_y,
            )
        };

        // Thruster flame drawn first (behind the hull), out of the engine base
        // at local -Y. Scales with `glow`.
        if glow > 0.02 {
            let base = -0.475;
            let fw = 0.10 + glow * 0.05;
            let ft = glow * 0.36;
            let fa = (glow * 220.0) as u8;
            draw_triangle(
                rot(0.0, base - ft), rot(-fw, base + 0.03), rot(fw, base + 0.03),
                Color::from_rgba(255, (110.0 + glow * 110.0) as u8, 30, fa),
            );
            draw_triangle(
                rot(0.0, base - ft * 0.55), rot(-fw * 0.5, base + 0.03), rot(fw * 0.5, base + 0.03),
                Color::from_rgba(255, 232, 120, (fa as f32 * 0.7) as u8),
            );
        }

        // Hull: faceted silver mesh extracted from the original Flash ship.
        // Per-facet brightness from centroid height (nose lit, base shaded).
        // The nose cone (centroid above TIP_Y) is recoloured red.
        let hull_base = (168.0_f32, 174.0_f32, 188.0_f32); // silver (#CCCCCC family)
        let tip_base  = (210.0_f32, 50.0_f32,  45.0_f32);  // red nose cone
        const TIP_Y: f32 = 0.30;
        for t in SHIP_TRIS.iter() {
            let cy = (t[1] + t[3] + t[5]) / 3.0;
            let s = (0.84 + (cy + 0.475) / 0.95 * 0.34).min(1.25);
            let base = if cy > TIP_Y { tip_base } else { hull_base };
            let col = Color::new(
                (base.0 * s / 255.0).min(1.0),
                (base.1 * s / 255.0).min(1.0),
                (base.2 * s / 255.0).min(1.0),
                1.0,
            );
            draw_triangle(rot(t[0], t[1]), rot(t[2], t[3]), rot(t[4], t[5]), col);
        }
        // Detail overlays (window, leg-pods, engine cup, gold accent) — exact
        // sub-shapes from the original ship, drawn on top of the hull. The two
        // leg-pods (extracted dark-silver 0.518/0.537/0.588) are recoloured red.
        for d in SHIP_DETAILS.iter() {
            let is_leg = (d[6] - 0.518).abs() < 0.001
                && (d[7] - 0.537).abs() < 0.001
                && (d[8] - 0.588).abs() < 0.001;
            let col = if is_leg {
                Color::new(0.784, 0.188, 0.169, 1.0) // red legs
            } else {
                Color::new(d[6], d[7], d[8], 1.0)
            };
            draw_triangle(rot(d[0], d[1]), rot(d[2], d[3]), rot(d[4], d[5]), col);
        }

        smooth_fps += (get_fps() as f32 - smooth_fps) * 0.05;
        let cave_x = cam_x.rem_euclid(PERIOD);
        draw_text(
            &format!("x={:.0}  lvl={}  {:.0}m/{}m   [R] reset   FPS: {:.0}", cam_x, ship_layer, cave_x, PERIOD as i32, smooth_fps),
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
        // Analog steering: touch slider + gamepad stick share one value. They
        // live on separate atomics so neither stomps the other; sum and clamp
        // (you won't push both at once, and clamping keeps |torque| ≤ 1).
        let touch_torque = (f32::from_bits(TOUCH_TORQUE.load(Ordering::Relaxed))
            + f32::from_bits(PAD_TORQUE.load(Ordering::Relaxed)))
            .clamp(-1.0, 1.0);
        let rotating_left  = is_key_down(KeyCode::Left)  || touch_torque < -0.1;
        let rotating_right = is_key_down(KeyCode::Right) || touch_torque >  0.1;
        // Rotate by firing a side RCS booster: apply the force *at the nozzle*
        // (off-center) instead of a pure couple, so the ship pivots about where
        // the boosters actually push. Nozzles exhaust downward (-Y local) → the
        // reaction force is +Y local at x = ∓0.30: left nozzle → clockwise,
        // right nozzle → counter-clockwise. RCS_FORCE is tuned so the x-lever
        // (~0.30) yields roughly the previous ±1.0 pure torque.
        const RCS_FORCE: f32 = 3.3;
        let fire_rcs = |rb: &mut RigidBody, side: f32, mag: f32| {
            let (px, py) = lp(0.30 * side, -0.71);
            let (fx, fy) = ld(0.0, mag);
            rb.add_force_at_point(vector![fx, fy], point![px, py], true);
        };
        if rotating_left {
            fire_rcs(rb, -1.0, RCS_FORCE);
        } else if rotating_right {
            fire_rcs(rb, 1.0, RCS_FORCE);
        } else if touch_torque != 0.0 {
            fire_rcs(rb, touch_torque.signum(), RCS_FORCE * touch_torque.abs());
        }

        // --- Particle emission ---
        let dt = get_frame_time();

        // Main thruster: exhaust exits local -Y (out the bottom), 8 particles/frame
        if thrusting_now {
            for _ in 0..8 {
                let spread = gen_range(-0.25f32, 0.25);
                let (px, py) = lp(spread * 0.45, -0.72);
                let speed = gen_range(4.0f32, 8.0);
                let (dvx, dvy) = ld(spread * 1.5, -speed);
                particles.push(Particle {
                    x: px, y: py,
                    vx: ship_vx + dvx, vy: ship_vy + dvy,
                    life: 1.0, kind: 0,
                });
            }
        }

        // Side RCS thrusters (cosmetic): bottom nozzles flanking the main booster vent
        // downward (out the bottom, like a mini main thruster) to swing the ship.
        // Turning left → LEFT nozzle fires; turning right → RIGHT nozzle fires.
        // Coords are in scaled world units — lp() does NOT apply SHIP_SCALE.
        if rotating_left {
            for _ in 0..3 {
                let spread = gen_range(-0.15f32, 0.15);
                let (px, py) = lp(-0.30, -0.71);   // left leg nozzle (gold accent, scaled)
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
                let (px, py) = lp(0.30, -0.71);    // right leg nozzle (gold accent, scaled)
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

        if is_key_pressed(KeyCode::R) || PAD_RESET.swap(0, Ordering::Relaxed) != 0 {
            let rb = rigid_body_set.get_mut(box_handle).unwrap();
            rb.set_translation(vector![RESET_X, cave_center(RESET_X)], true);
            rb.set_linvel(vector![0.0, 0.0], true);
            rb.set_angvel(0.0, true);
            rb.set_rotation(Rotation::new(0.0), true);
            // Snap the interpolation state too, or the camera lerps across the
            // teleport for one frame.
            prev_ship = (RESET_X, cave_center(RESET_X), 0.0);
        }

        // --- Minimap (ship always centred; pans in BOTH axes) ---
        let mm_w = 480.0f32 * ui;
        let mm_h = 160.0f32 * ui;
        let mm_ox = safe_left + 10.0f32 * ui;
        let mm_oy = safe_top + 10.0f32 * ui;
        let mm_dark = Color::from_rgba(8, 8, 18, 220);

        // World → minimap, both axes relative to the ship.
        let to_mm_x = |wx: f32| -> f32 {
            mm_ox + (wx - cam_x + MM_HALF_X) / (2.0 * MM_HALF_X) * mm_w
        };
        let to_mm_y = |wy: f32| -> f32 {
            mm_oy + mm_h - (wy - cam_y + MM_HALF_Y) / (2.0 * MM_HALF_Y) * mm_h
        };

        // Fill with rock, then carve the cave interior of every layer in view,
        // sampled in columns around the ship.
        draw_rectangle(mm_ox, mm_oy, mm_w, mm_h, rock_mid);
        let col_w = mm_w / MM_SAMPLES as f32 + 0.5;
        for i in 0..MM_SAMPLES {
            let x     = cam_x - MM_HALF_X + (i as f32 + 0.5) * (2.0 * MM_HALF_X) / MM_SAMPLES as f32;
            let col_x = mm_ox + i as f32 / MM_SAMPLES as f32 * mm_w;
            let c  = cave_center(x);
            let hw = cave_half_width(x);
            for layer in (ship_layer - 1)..=(ship_layer + 1) {
                let ly = layer as f32 * V_PERIOD;
                let top_s = to_mm_y(ly + c + hw).clamp(mm_oy, mm_oy + mm_h);
                let bot_s = to_mm_y(ly + c - hw).clamp(mm_oy, mm_oy + mm_h);
                if bot_s > top_s {
                    draw_rectangle(col_x, top_s, col_w, bot_s - top_s, mm_dark);
                }
            }
        }

        // Carve the vertical shafts with their true wall shape — evaluated from
        // the same pure functions as the world geometry, so the minimap is a
        // genuinely zoomed-out view (wiggles, junction heights and all).
        const MM_SHAFT_STEPS: usize = 16;
        let slot_w = SHAFT_SPACING_SEGS as f32 * SEG_LEN;
        let s_mm_lo = ((cam_x - MM_HALF_X) / slot_w).floor() as i64 - 1;
        let s_mm_hi = ((cam_x + MM_HALF_X) / slot_w).ceil()  as i64 + 1;
        let gap_lo = ((cam_y - MM_HALF_Y - 10.0) / V_PERIOD).floor() as i64;
        let gap_hi = ((cam_y + MM_HALF_Y + 10.0) / V_PERIOD).floor() as i64;
        for s in s_mm_lo..=s_mm_hi {
            let o = shaft_open_seg(s);
            let (xl, xr) = (o as f32 * SEG_LEN, (o + SHAFT_OPEN_SEGS) as f32 * SEG_LEN);
            if xr < cam_x - MM_HALF_X - 2.0 || xl > cam_x + MM_HALF_X + 2.0 {
                continue;
            }
            // Per-side junction offsets within a layer (same as shaft_wall_pts).
            let (jbl, jtl) = (cave_center(xl) + cave_half_width(xl), cave_center(xl) - cave_half_width(xl));
            let (jbr, jtr) = (cave_center(xr) + cave_half_width(xr), cave_center(xr) - cave_half_width(xr));
            for gap in gap_lo..=gap_hi {
                let (gy0, gy1) = (gap as f32 * V_PERIOD, (gap + 1) as f32 * V_PERIOD);
                let mm_pt = |side: u8, t: f32| -> Vec2 {
                    let (y0, y1) = if side == 0 { (gy0 + jbl, gy1 + jtl) } else { (gy0 + jbr, gy1 + jtr) };
                    vec2(
                        to_mm_x(shaft_wall_x(s, side, t)).clamp(mm_ox, mm_ox + mm_w),
                        to_mm_y(y0 + (y1 - y0) * t).clamp(mm_oy, mm_oy + mm_h),
                    )
                };
                for k in 0..MM_SHAFT_STEPS {
                    let t0 = k as f32 / MM_SHAFT_STEPS as f32;
                    let t1 = (k + 1) as f32 / MM_SHAFT_STEPS as f32;
                    let a = mm_pt(0, t0);
                    let b = mm_pt(1, t0);
                    let c = mm_pt(1, t1);
                    let d = mm_pt(0, t1);
                    // Cells fully clamped to the top/bottom edge are degenerate
                    // (zero height) and draw nothing — no need to cull.
                    draw_triangle(a, b, c, mm_dark);
                    draw_triangle(a, c, d, mm_dark);
                }
            }
        }

        // Obstacle shapes on the minimap — actual polygon, not just a dot.
        // All loaded layers; the y window filters to what's actually in view.
        for ob in obstacles.values() {
            if (ob.cx - cam_x).abs() > MM_HALF_X + 6.0 || (ob.cy - cam_y).abs() > MM_HALF_Y + 6.0 {
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

        // Viewport rectangle — ship-centred in both axes, like the map itself
        let vp_hw   = sw / (2.0 * view_scale);
        let vp_hh   = sh / (2.0 * view_scale);
        let vp_mm_hw = vp_hw / MM_HALF_X * (mm_w / 2.0);
        let vp_mm_hh = vp_hh / MM_HALF_Y * (mm_h / 2.0);
        let (mm_cx, mm_cy) = (mm_ox + mm_w / 2.0, mm_oy + mm_h / 2.0);
        draw_rectangle_lines(mm_cx - vp_mm_hw, mm_cy - vp_mm_hh,
            2.0 * vp_mm_hw, 2.0 * vp_mm_hh, 1.0,
            Color::from_rgba(255, 255, 255, 180));

        // Ship dot — map centre
        draw_circle(mm_cx, mm_cy, 3.0, YELLOW);

        // Border
        draw_rectangle_lines(mm_ox, mm_oy, mm_w, mm_h, 1.0, Color::from_rgba(255, 255, 255, 120));

        next_frame().await;
    }
}
