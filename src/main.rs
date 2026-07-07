use macroquad::audio::{load_sound_from_bytes, play_sound, set_sound_volume, PlaySoundParams};
use macroquad::prelude::*;
use macroquad::rand::gen_range;
use rapier2d::prelude::*;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU32, Ordering};

mod audio;
mod render;
mod replay;
mod ship_mesh;
mod world;

use audio::*;
use render::*;
use replay::*;
use ship_mesh::*;
use world::*;

struct Particle {
    x: f32, y: f32,
    vx: f32, vy: f32,
    life: f32,  // 1.0 = fresh, 0.0 = dead
    kind: u8,   // 0 = main thruster, 1 = left RCS, 2 = right RCS
}

// Top-level game mode. Flying covers normal play AND the brief wreck/
// explosion phase (crash_timer > 0); the dialog and replay pause physics.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Flying,
    CrashDialog,
    Replay,
}

// One recorded physics step for the instant replay: the ship pose plus the
// visual state (velocity for the HUD/exhaust, engine glow, RCS puff side)
// needed to re-render the flight. This is VISUAL-state playback — the
// planned cross-platform-deterministic Rapier replay (record inputs, re-run
// the sim) can replace the recording source behind the same dialog/playback
// UI later.
#[derive(Clone, Copy)]
struct ReplayFrame {
    x: f32,
    y: f32,
    angle: f32,
    vx: f32,
    vy: f32,
    glow: f32,
    rcs: i8, // -1 = left nozzle puffing, +1 = right, 0 = none
}

// Shortest-path angle interpolation (replay playback samples between steps).
fn lerp_angle(a: f32, b: f32, t: f32) -> f32 {
    let mut d = b - a;
    if d > std::f32::consts::PI { d -= std::f32::consts::TAU; }
    if d < -std::f32::consts::PI { d += std::f32::consts::TAU; }
    a + d * t
}

// Touch throttle is ANALOG (f32 bits, 0..1): the on-screen JET button sends
// 1.0/0.0 today, but the export stays analog. The stick supplies a STEER
// VECTOR (f32 bits each, screen convention: x right, y down, magnitude ≤ 1):
// its direction is the commanded nose direction, (0,0) = released.
static TOUCH_THRUST: AtomicU32 = AtomicU32::new(0);
static TOUCH_STEER_X: AtomicU32 = AtomicU32::new(0);
static TOUCH_STEER_Y: AtomicU32 = AtomicU32::new(0);
// Stick contact (0/1): holding the stick fires the main engine too, but
// through the flick-grace / flip-settle gating in the main loop — hence a
// separate flag instead of driving TOUCH_THRUST directly.
static TOUCH_STICK_HELD: AtomicU32 = AtomicU32::new(0);
// Gamepad state lives on its own atomics (not the touch ones) so a connected-but-
// idle controller never stomps an active touch input, and vice versa. The two
// sources are combined in the main loop.
static PAD_THRUST: AtomicU32 = AtomicU32::new(0);
static PAD_TORQUE: AtomicU32 = AtomicU32::new(0);
static PAD_RESET: AtomicU32 = AtomicU32::new(0);
static SAFE_AREA_TOP: AtomicU32 = AtomicU32::new(0);
static SAFE_AREA_LEFT: AtomicU32 = AtomicU32::new(0);
// Velocity-vector arrow (off by default) — toggled from the info overlay's
// checkbox, persisted in localStorage on the web side.
static SHOW_VEL: AtomicU32 = AtomicU32::new(0);

#[unsafe(no_mangle)]
pub extern "C" fn set_touch_thrust(value: f32) {
    TOUCH_THRUST.store(value.to_bits(), Ordering::Relaxed);
}

#[unsafe(no_mangle)]
pub extern "C" fn set_touch_steer(x: f32, y: f32) {
    TOUCH_STEER_X.store(x.to_bits(), Ordering::Relaxed);
    TOUCH_STEER_Y.store(y.to_bits(), Ordering::Relaxed);
}

#[unsafe(no_mangle)]
pub extern "C" fn set_touch_stick_held(held: i32) {
    TOUCH_STICK_HELD.store(held as u32, Ordering::Relaxed);
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
pub extern "C" fn set_show_velocity(on: i32) {
    SHOW_VEL.store(on as u32, Ordering::Relaxed);
}

// Deploy git revision (first 8 hex chars parsed to a u32 by index.html),
// stamped into serialized replay blobs so a future re-sim/verifier knows
// which build flew the run. 0 = local dev build.
static BUILD_ID: AtomicU32 = AtomicU32::new(0);

#[unsafe(no_mangle)]
pub extern "C" fn set_build_id(id: u32) {
    BUILD_ID.store(id, Ordering::Relaxed);
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
        high_dpi: true,
        platform: macroquad::miniquad::conf::Platform {
            webgl_version: macroquad::miniquad::conf::WebGLVersion::WebGL2,
            ..Default::default()
        },
        ..Default::default()
    }
}

const SCALE: f32 = 80.0;
// How many segments to keep loaded on each side of the ship
const HALF_WINDOW: i64 = 80;
// Physics-shaping constants, hoisted so the replay params header serializes
// the exact values the sim runs with (see sim_params()).
const GRAVITY_Y: f32 = -1.62;
const THRUST_FORCE: f32 = 8.0;     // main engine at full throttle
const LINEAR_DAMPING: f32 = 0.2;
const ANGULAR_DAMPING: f32 = 3.0;
// Side RCS booster force, applied at the nozzle (see the controls section);
// tuned so the ~0.30 x-lever yields roughly ±1.0 of torque.
const RCS_FORCE: f32 = 3.3;
// Render scale for the ship mesh relative to the raw SWF coordinates
const SHIP_SCALE: f32 = 1.5;
// Physics runs on a fixed timestep (accumulator in the main loop) so handling
// is identical on every display refresh rate; rendering interpolates the ship
// between the last two steps.
const PHYSICS_DT: f32 = 1.0 / 120.0;
// Impacts are read from the frame-to-frame velocity change (collision
// impulse); gravity/thrust change v by < 0.3 m/s per frame, so anything above
// CRASH_DV_SOFT is a real hit. Damage is graduated: below SOFT is free,
// SOFT..HARD scrapes the hull proportionally (full bar at HARD), and HARD —
// or a scrape that empties the hull — destroys the ship.
const CRASH_DV_SOFT: f32 = 2.5;
const CRASH_DV_HARD: f32 = 6.0;
// Seconds from crash to the crash dialog (fly again / watch replay) — long
// enough for the explosion to play out with the camera held still.
const CRASH_DIALOG_DELAY: f32 = 1.5;
// Instant replay: how many seconds of flight are retained (recorded once per
// physics step). Sized to cover a whole spawn→crash run; runs longer than
// this keep only their tail. 5 min ≈ 36k visual frames ≈ 1 MB in RAM. The
// hybrid recording (src/replay.rs) trims to the same horizon.
const REPLAY_MAX_SECS: f32 = 300.0;
// Hull integrity: scraped off by survivable impacts, restored while parked on
// a pad (alongside refueling) and by reset/respawn.
const HULL_MAX: f32 = 100.0;
const HULL_REPAIR_PER_S: f32 = 20.0;
// Touch heading control: the stick commands a nose DIRECTION; a PD controller
// torques the ship toward it (shortest way). Authority scales with deflection.
// Tuned snappy: strong spring (KP), high torque ceiling, damping raised with
// them so the nose stops crisply instead of ringing. The torque ceiling is
// what sets the 180°-flip time (the spring saturates it for most of the
// swing) — 6.0 flips roughly twice as fast as the earlier 3.5.
const HEADING_KP: f32 = 14.0;
const HEADING_KD: f32 = 2.2;
const HEADING_TORQUE_MAX: f32 = 6.0;
// Stick-hold engine gating (one-handed scheme): a quick flick shorter than
// DELAY never lights the engine, thrust then ramps to full over RAMP, and a
// commanded flip past FLIP_GATE keeps the engine cold until the nose settles
// within FLIP_DONE of the target (steer first, burn once pointed).
const STICK_THRUST_DELAY: f32 = 0.12;
const STICK_THRUST_RAMP: f32 = 0.18;
const FLIP_GATE_RAD: f32 = 1.6;  // ~92°
const FLIP_DONE_RAD: f32 = 0.35; // ~20°
// Fuel: the main engine burns a full tank in ~28 s of continuous thrust; the
// RCS sips. An empty tank kills engine and RCS until reset/respawn refills it.
const FUEL_MAX: f32 = 100.0;
const FUEL_BURN_MAIN: f32 = 3.5; // units/s while the main engine fires
const FUEL_BURN_RCS: f32 = 1.2;  // units/s while an RCS nozzle fires

// The exact simulation constants this build runs with, serialized into every
// replay blob so a recording can be re-run under the rules it was flown with.
fn sim_params() -> SimParams {
    SimParams {
        dt: PHYSICS_DT,
        gravity_y: GRAVITY_Y,
        thrust_force: THRUST_FORCE,
        linear_damping: LINEAR_DAMPING,
        angular_damping: ANGULAR_DAMPING,
        rcs_force: RCS_FORCE,
        heading_kp: HEADING_KP,
        heading_kd: HEADING_KD,
        heading_torque_max: HEADING_TORQUE_MAX,
        fuel_max: FUEL_MAX,
        fuel_burn_main: FUEL_BURN_MAIN,
        fuel_burn_rcs: FUEL_BURN_RCS,
        crash_dv_soft: CRASH_DV_SOFT,
        crash_dv_hard: CRASH_DV_HARD,
        hull_max: HULL_MAX,
    }
}

#[macroquad::main(window_conf)]
async fn main() {
    // Panic hook: log the human-readable panic ("panicked at src/…:line: msg")
    // through miniquad's console_error import. The default hook prints the
    // opaque Debug form (`PanicHookInfo { payload: Any { .. }, … }`), and the
    // JS error event that follows the trap is muted to a bare "Script error."
    // on iOS Safari — this line is what the boot guard's console.error mirror
    // shows on screen, so keep it a single message.
    std::panic::set_hook(Box::new(|info| error!("{}", info)));

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

    // Loaded landing pads, keyed by (slot index, layer) like obstacles.
    struct Pad {
        handle: ColliderHandle,
        cx: f32,
        y: f32, // deck top (collider line), layer offset applied
    }
    let mut pads: HashMap<(i64, i64), Pad> = HashMap::new();

    let spawn_pad = |p: i64, layer: i64, collider_set: &mut ColliderSet,
                         pads: &mut HashMap<(i64, i64), Pad>| {
        let Some(spec) = pad_spec(p) else { return };
        let y = spec.y + layer as f32 * V_PERIOD;
        // High friction, no restitution: the ship should settle, not skate.
        let handle = collider_set.insert(
            ColliderBuilder::segment(
                point![spec.cx - PAD_HALF_W, y],
                point![spec.cx + PAD_HALF_W, y],
            )
            .friction(0.9)
            .build(),
        );
        pads.insert((p, layer), Pad { handle, cx: spec.cx, y });
    };

    // Ship starts at cave centre
    let box_body = RigidBodyBuilder::dynamic()
        .translation(vector![0.0, stand_y(0.0)])
        .angular_damping(ANGULAR_DAMPING)
        // A whisper of drag: imperceptible at landing speeds but it caps how
        // much momentum can pile up on a long burn or free-fall.
        .linear_damping(LINEAR_DAMPING)
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

    let gravity = vector![0.0, GRAVITY_Y];
    let integration_params = IntegrationParameters {
        dt: PHYSICS_DT,
        num_solver_iterations: std::num::NonZeroUsize::new(8).unwrap(),
        ..Default::default()
    };
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

    // Procedural sounds, generated in memory (see thruster_wav / boom_wav).
    // The engine rumble runs as a muted loop whose volume follows `glow`.
    let thruster_snd = load_sound_from_bytes(&thruster_wav()).await.ok();
    let boom_snd = load_sound_from_bytes(&boom_wav()).await.ok();
    if let Some(s) = &thruster_snd {
        play_sound(s, PlaySoundParams { looped: true, volume: 0.0 });
    }

    let mut phys_accum = 0.0f32;
    // Ship state at the previous physics step, for render interpolation.
    let mut prev_ship = (0.0f32, stand_y(0.0), 0.0f32); // x, y, angle
    // Crash state: velocity last frame (impact = big dv) and the wreck timer
    // (> 0 → crashed: input dead, ship hidden, respawn when it hits 0).
    let mut prev_vel = (0.0f32, 0.0f32);
    let mut crash_timer = 0.0f32;
    let mut mode = Mode::Flying;
    // Instant-replay ring buffer (one frame per physics step) and the
    // playback cursor, in fractional frame indices.
    let replay_cap = (REPLAY_MAX_SECS / PHYSICS_DT) as usize;
    let mut replay_buf: VecDeque<ReplayFrame> = VecDeque::with_capacity(replay_cap);
    let mut replay_t = 0.0f32;
    let mut last_rcs: i8 = 0; // RCS puff side of the previous frame, recorded per step

    // Hybrid recording (src/replay.rs): the shareable spawn→crash replay —
    // input change-events + 1 Hz keyframes + params header. In memory only
    // for now; serialized + deflated at the crash to measure what shipping
    // it would cost (shown on the WATCH REPLAY button).
    let spawn_keyframe = |x: f32| Keyframe {
        tick: 0, x, y: stand_y(x), angle: 0.0, vx: 0.0, vy: 0.0,
        angvel: 0.0, fuel: FUEL_MAX, hull: HULL_MAX, glow: 0.0,
    };
    let mut recorder = Recording::new(sim_params(), (REPLAY_MAX_SECS / PHYSICS_DT) as u32);
    recorder.push_keyframe(spawn_keyframe(0.0));
    let mut last_input = InputState::default(); // controls in effect, quantized per frame
    let mut blob_sizes: Option<(usize, usize)> = None; // (raw, deflated) at last crash

    // Debris burst at (x, y) — fired at the real crash and again when the
    // replay reaches its end.
    let boom_burst = |x: f32, y: f32, particles: &mut Vec<Particle>| {
        for _ in 0..70 {
            let ang = gen_range(0.0f32, std::f32::consts::TAU);
            let spd = gen_range(1.0f32, 9.0);
            particles.push(Particle {
                x: x + gen_range(-0.3f32, 0.3),
                y: y + gen_range(-0.3f32, 0.3),
                vx: ang.cos() * spd,
                vy: ang.sin() * spd + 1.5,
                life: gen_range(0.5f32, 1.0),
                kind: 3,
            });
        }
    };
    let mut fuel = FUEL_MAX;
    let mut hull = HULL_MAX;
    let mut shake = 0.0f32; // impact screen-shake intensity, 0..1, decays fast
    let mut stick_thrust_t = 0.0f32; // seconds the stick-hold engine has been eligible
    let mut flip_settling = false;   // big flip commanded: engine cold until nose settles
    // Landing pads: score, first-visit set, and how long the ship has been
    // settled on the current pad (landing counts at PAD_LAND_TIME).
    let mut score = 0u32;
    let mut visited_pads: HashSet<(i64, i64)> = HashSet::new();
    let mut land_timer = 0.0f32;
    let mut pad_msg_timer = 0.0f32; // "+100" flash after a first landing

    loop {
        // Fixed-timestep accumulator: the cap bounds catch-up work after a
        // hitch (same role as the old per-frame dt cap of 0.05 s). Physics
        // only runs while flying — the crash dialog and the replay pause the
        // sim (the wreck is parked anyway) and drain the accumulator so no
        // catch-up burst fires on resume.
        if mode == Mode::Flying {
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
                // Record the step for the instant replay (stops at the moment
                // of destruction — crash_timer flips positive the same frame,
                // after this loop, so the impact itself IS captured).
                if crash_timer <= 0.0 {
                    if replay_buf.len() >= replay_cap {
                        replay_buf.pop_front();
                    }
                    let b = &rigid_body_set[box_handle];
                    replay_buf.push_back(ReplayFrame {
                        x: b.translation().x,
                        y: b.translation().y,
                        angle: b.rotation().angle(),
                        vx: b.linvel().x,
                        vy: b.linvel().y,
                        glow,
                        rcs: last_rcs,
                    });
                    // Hybrid recording: the input in effect for this step
                    // (an event only when it changed) + periodic keyframes.
                    if recorder.record_tick(last_input) {
                        recorder.push_keyframe(Keyframe {
                            tick: recorder.ticks(),
                            x: b.translation().x,
                            y: b.translation().y,
                            angle: b.rotation().angle(),
                            vx: b.linvel().x,
                            vy: b.linvel().y,
                            angvel: b.angvel(),
                            fuel, hull, glow,
                        });
                    }
                }
            }
        } else {
            phys_accum = 0.0;
        }

        // --- Impact / crash detection ---
        // A collision shows up as a large velocity change across the frame's
        // physics steps. Graduated: dv in SOFT..HARD scrapes the hull
        // (proportionally, full bar at HARD); dv past HARD — or a scrape the
        // hull can't absorb — destroys the ship.
        if mode == Mode::Flying {
            let (wx, wy, vx, vy) = {
                let b = &rigid_body_set[box_handle];
                (b.translation().x, b.translation().y, b.linvel().x, b.linvel().y)
            };
            if crash_timer <= 0.0 {
                let (dvx, dvy) = (vx - prev_vel.0, vy - prev_vel.1);
                let dv = (dvx * dvx + dvy * dvy).sqrt();
                if dv > CRASH_DV_SOFT {
                    let damage =
                        (dv - CRASH_DV_SOFT) / (CRASH_DV_HARD - CRASH_DV_SOFT) * HULL_MAX;
                    hull -= damage;
                    shake = (shake + damage / HULL_MAX + 0.25).min(1.0);
                    if dv > CRASH_DV_HARD || hull <= 0.0 {
                        hull = 0.0;
                        crash_timer = CRASH_DIALOG_DELAY;
                        // Debris burst at the crash site.
                        boom_burst(wx, wy, &mut particles);
                        if let Some(s) = &boom_snd {
                            play_sound(s, PlaySoundParams { looped: false, volume: 0.9 });
                        }
                        // Freeze the shareable recording at the impact (the
                        // body still holds the post-impact pose/velocity —
                        // the wreck is parked just below) and measure what
                        // shipping it would cost, raw and deflated.
                        {
                            let b = &rigid_body_set[box_handle];
                            recorder.finalize(Keyframe {
                                tick: recorder.ticks(),
                                x: wx, y: wy,
                                angle: b.rotation().angle(),
                                vx, vy,
                                angvel: b.angvel(),
                                fuel, hull, glow,
                            });
                        }
                        let blob = recorder.serialize(BUILD_ID.load(Ordering::Relaxed));
                        blob_sizes = Some((blob.len(), compress(&blob).len()));
                        // Park the wreck where it died so the camera holds still.
                        let rb = rigid_body_set.get_mut(box_handle).unwrap();
                        rb.set_linvel(vector![0.0, 0.0], true);
                        rb.set_angvel(0.0, true);
                        rb.set_gravity_scale(0.0, true);
                    } else {
                        // Survivable scrape: a spray of sparks + a quiet thud,
                        // scaled to the damage taken. The ship keeps flying.
                        for _ in 0..(6 + (damage * 0.3) as i32) {
                            let ang = gen_range(0.0f32, std::f32::consts::TAU);
                            let spd = gen_range(0.8f32, 4.0);
                            particles.push(Particle {
                                x: wx + gen_range(-0.25f32, 0.25),
                                y: wy + gen_range(-0.25f32, 0.25),
                                vx: vx * 0.3 + ang.cos() * spd,
                                vy: vy * 0.3 + ang.sin() * spd + 1.0,
                                life: gen_range(0.25f32, 0.55),
                                kind: 3,
                            });
                        }
                        if let Some(s) = &boom_snd {
                            play_sound(s, PlaySoundParams { looped: false, volume: 0.25 });
                        }
                    }
                }
            }
            prev_vel = (vx, vy);
        }
        // Wreck timer → once the explosion has played out, hand over to the
        // crash dialog (fly again / watch replay). Respawn happens from there.
        if crash_timer > 0.0 {
            crash_timer -= get_frame_time();
            if crash_timer <= 0.0 {
                crash_timer = 0.0;
                mode = Mode::CrashDialog;
            }
        }
        let crashed = crash_timer > 0.0;

        let sh = screen_height();
        let sw = screen_width();
        // With high_dpi enabled, sw/sh are PHYSICAL pixels (CSS px × device
        // pixel ratio). All breakpoints and fixed sizes below were tuned in
        // CSS pixels, so thresholds compare against sw/sh ÷ dpi and pixel
        // sizes multiply by it. dpi = 1 on standard displays / native builds.
        let dpi = screen_dpi_scale();

        // Mobile zoom: ONE scale for both orientations, keyed on the smaller
        // screen dimension — rotating the phone never changes the zoom level,
        // the world just extends further along the long axis. The smaller
        // dimension always spans MOBILE_VIEW_M metres; in landscape that
        // dimension is the height, so the cave's typical full height (average
        // half-width ~7.8 m → ~15.5 m of cave) fits with margin, and portrait
        // shows the same-sized world with more of it visible vertically.
        // Desktop keeps the fixed SCALE zoom; the cap stops a small desktop
        // window from zooming in past it.
        const MOBILE_VIEW_M: f32 = 19.0; // world metres across the smaller screen dimension
        let view_scale = if sw.min(sh) / dpi < 600.0 {
            (sw.min(sh) / MOBILE_VIEW_M).min(SCALE * dpi)
        } else {
            SCALE * dpi
        };
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
        let ui = (sw.min(sh) / dpi / 980.0).min(1.0) * dpi;

        // Safe-area insets (notch / status bar), supplied by JS via env(safe-area-inset-*)
        // in CSS pixels → converted to physical pixels here. The top inset is
        // honoured in full (the notch/island sits at the top in portrait); the
        // LEFT inset is capped — in landscape the island sits mid-edge, not in
        // the top corner, and the full ~47-59 px inset shoved the minimap far
        // into the screen when only the rounded bezel corner actually matters.
        let safe_top = f32::from_bits(SAFE_AREA_TOP.load(Ordering::Relaxed)) * dpi;
        let safe_left = f32::from_bits(SAFE_AREA_LEFT.load(Ordering::Relaxed)).min(24.0) * dpi;

        let (mut cam_x, mut cam_y, mut angle, mut ship_vx, mut ship_vy) = {
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

        // Impact screen shake: brief random camera jitter that decays in a
        // few tenths of a second. ±0.12 m at full intensity — enough to feel,
        // far too small to disturb the sliding windows keyed off cam_x.
        if shake > 0.0 {
            cam_x += gen_range(-1.0f32, 1.0) * shake * 0.12;
            cam_y += gen_range(-1.0f32, 1.0) * shake * 0.12;
            shake = (shake - 4.0 * get_frame_time()).max(0.0);
        }

        // Replay playback: advance the cursor and override the camera/pose
        // (and velocity, for the HUD and exhaust) with the recorded flight.
        // The sliding windows below key off cam_x/cam_y, so the world loads
        // around the replayed ship automatically. Ends by replaying the
        // explosion at the crash site, then returns to the dialog.
        let mut replay_frame: Option<ReplayFrame> = None;
        if mode == Mode::Replay {
            replay_t += get_frame_time() / PHYSICS_DT;
            let last = replay_buf.len() - 1; // entry is gated on len >= 2
            if replay_t >= last as f32 {
                let f = replay_buf[last];
                boom_burst(f.x, f.y, &mut particles);
                if let Some(s) = &boom_snd {
                    play_sound(s, PlaySoundParams { looped: false, volume: 0.9 });
                }
                mode = Mode::CrashDialog;
            } else {
                let i = replay_t.floor() as usize;
                let t = replay_t - i as f32;
                let (a, b) = (replay_buf[i], replay_buf[i + 1]);
                replay_frame = Some(ReplayFrame {
                    x: a.x + (b.x - a.x) * t,
                    y: a.y + (b.y - a.y) * t,
                    angle: lerp_angle(a.angle, b.angle, t),
                    vx: a.vx + (b.vx - a.vx) * t,
                    vy: a.vy + (b.vy - a.vy) * t,
                    glow: a.glow + (b.glow - a.glow) * t,
                    rcs: a.rcs,
                });
            }
        }
        if let Some(f) = replay_frame {
            cam_x = f.x;
            cam_y = f.y;
            angle = f.angle;
            ship_vx = f.vx;
            ship_vy = f.vy;
        }

        // Local-to-world helpers (position and direction)
        let lp = |lx: f32, ly: f32| -> (f32, f32) {
            (cam_x + lx * angle.cos() - ly * angle.sin(),
             cam_y + lx * angle.sin() + ly * angle.cos())
        };
        let ld = |lx: f32, ly: f32| -> (f32, f32) {
            (lx * angle.cos() - ly * angle.sin(),
             lx * angle.sin() + ly * angle.cos())
        };

        // Main-engine throttle (0..1), read early so lighting can use it.
        // JET button / keyboard / mouse / pad = immediate full power. HOLDING
        // the attitude stick also thrusts, but gated so steering stays cheap:
        // a flick shorter than STICK_THRUST_DELAY never lights the engine,
        // thrust then ramps in over STICK_THRUST_RAMP, and a commanded flip
        // past FLIP_GATE_RAD keeps it cold until the nose settles within
        // FLIP_DONE_RAD (the gate resets the ramp, so post-flip thrust also
        // fades in). Dead while crashed or dry.
        let stick_held = TOUCH_STICK_HELD.load(Ordering::Relaxed) != 0;
        let steer_x = f32::from_bits(TOUCH_STEER_X.load(Ordering::Relaxed));
        let steer_y = f32::from_bits(TOUCH_STEER_Y.load(Ordering::Relaxed));
        let steer_mag = (steer_x * steer_x + steer_y * steer_y).sqrt().min(1.0);
        // Heading error to the commanded nose direction (0 when centred).
        // Physics already stepped this frame, so this is the same angle the
        // heading controller below acts on.
        let heading_err = if steer_mag > 0.0 {
            let target = (-steer_x).atan2(-steer_y);
            let mut e = target - rigid_body_set[box_handle].rotation().angle();
            if e > std::f32::consts::PI { e -= std::f32::consts::TAU; }
            if e < -std::f32::consts::PI { e += std::f32::consts::TAU; }
            e
        } else {
            0.0
        };
        if stick_held && heading_err.abs() > FLIP_GATE_RAD {
            flip_settling = true;
        }
        if !stick_held || heading_err.abs() < FLIP_DONE_RAD {
            flip_settling = false;
        }
        if stick_held && !flip_settling {
            stick_thrust_t += get_frame_time();
        } else {
            stick_thrust_t = 0.0;
        }
        let stick_throttle =
            ((stick_thrust_t - STICK_THRUST_DELAY) / STICK_THRUST_RAMP).clamp(0.0, 1.0);
        let mut throttle = f32::from_bits(TOUCH_THRUST.load(Ordering::Relaxed))
            .clamp(0.0, 1.0)
            .max(stick_throttle);
        if is_mouse_button_down(MouseButton::Left)
            || is_key_down(KeyCode::Down)
            || PAD_THRUST.load(Ordering::Relaxed) != 0
        {
            throttle = 1.0;
        }
        if crashed || mode != Mode::Flying || fuel <= 0.0 {
            throttle = 0.0;
        }
        let thrusting_now = throttle > 0.0;
        glow += (throttle - glow) * 0.12;
        // During playback the engine visuals/sound follow the recording.
        if let Some(f) = replay_frame {
            glow = f.glow;
        }
        if let Some(s) = &thruster_snd {
            set_sound_volume(s, glow * 0.6);
        }

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

        // --- Slide the pad window (same shape; ±20 m position jitter) ---
        let p_left  = ((win_left_x  - 20.0) / PAD_SPACING).floor() as i64;
        let p_right = ((win_right_x + 20.0) / PAD_SPACING).ceil()  as i64;
        pads.retain(|&(p, layer), pad| {
            if p < p_left || p > p_right || layer < lay_lo || layer > lay_hi {
                collider_set.remove(pad.handle, &mut island_manager, &mut rigid_body_set, false);
                false
            } else {
                true
            }
        });
        for layer in lay_lo..=lay_hi {
            for p in p_left..=p_right {
                if !pads.contains_key(&(p, layer)) {
                    spawn_pad(p, layer, &mut collider_set, &mut pads);
                }
            }
        }

        // --- Landing detection ---
        // A landing = resting on a pad deck: slow, upright, feet on the deck
        // (leg capsules bottom out 0.73 below the body origin), held for
        // PAD_LAND_TIME. First visit scores; parked ships refuel.
        let frame_dt = get_frame_time();
        let mut on_pad: Option<(i64, i64)> = None;
        if mode == Mode::Flying && !crashed {
            let b = &rigid_body_set[box_handle];
            let (bx, by) = (b.translation().x, b.translation().y);
            let v = b.linvel();
            let settled = b.rotation().angle().abs() < 0.30
                && v.x.abs() < 1.0
                && v.y.abs() < 1.0
                && b.angvel().abs() < 0.5;
            if settled {
                on_pad = pads.iter().find_map(|(&key, pad)| {
                    let feet = by - 0.73;
                    ((bx - pad.cx).abs() <= PAD_HALF_W && (feet - pad.y).abs() < 0.3)
                        .then_some(key)
                });
            }
        }
        let landed = if let Some(key) = on_pad {
            land_timer += frame_dt;
            if land_timer >= PAD_LAND_TIME {
                if visited_pads.insert(key) {
                    score += PAD_POINTS;
                    pad_msg_timer = 1.8;
                }
                fuel = (fuel + PAD_REFUEL_PER_S * frame_dt).min(FUEL_MAX);
                hull = (hull + HULL_REPAIR_PER_S * frame_dt).min(HULL_MAX);
                true
            } else {
                false
            }
        } else {
            land_timer = 0.0;
            false
        };
        pad_msg_timer = (pad_msg_timer - frame_dt).max(0.0);

        // --- Draw ---
        clear_background(Color::from_rgba(8, 8, 18, 255));

        // Stars
        for &(sx, sy) in &stars {
            let px = (sx * sw - cam_x * view_scale * 0.05).rem_euclid(sw);
            let py = (sy * sh + cam_y * view_scale * 0.05).rem_euclid(sh);
            draw_circle(px, py, (0.5 * dpi).max(1.0), Color::from_rgba(200, 200, 255, 150));
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
        let bevel = 16.0 * dpi; // obstacle bevel width, tuned in CSS px
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
                *p + d * (bevel / len).min(0.5)
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

        // Landing pads — man-made metal, drawn with the default material so
        // the deck and beacons stay readable in the dark. Deck top = the
        // collider line (alignment rule); legs drop to the floor curve.
        for (&pad_key, pad) in pads.iter() {
            let pad_layer = pad_key.1;
            let top_mid = w2s(pad.cx, pad.y, sh, cam_x, cam_y);
            if top_mid.x < -margin || top_mid.x > sw + margin
                || top_mid.y < -100.0 || top_mid.y > sh + 100.0 {
                continue;
            }
            let a = w2s(pad.cx - PAD_HALF_W, pad.y, sh, cam_x, cam_y);
            let b = w2s(pad.cx + PAD_HALF_W, pad.y, sh, cam_x, cam_y);
            let deck_h = 0.22 * view_scale;
            let deck = Color::from_rgba(96, 106, 122, 255);
            draw_triangle(a, b, vec2(b.x, b.y + deck_h), deck);
            draw_triangle(a, vec2(b.x, b.y + deck_h), vec2(a.x, a.y + deck_h), deck);
            draw_line(a.x, a.y, b.x, b.y, 2.0 * dpi, Color::from_rgba(190, 200, 218, 255));
            // Legs at ±(hw − 0.5), from under the deck down to the floor.
            for side in [-1.0f32, 1.0] {
                let lx = pad.cx + side * (PAD_HALF_W - 0.5);
                let ground = pad_layer as f32 * V_PERIOD + cave_center(lx) - cave_half_width(lx);
                let top = w2s(lx, pad.y, sh, cam_x, cam_y);
                let bot = w2s(lx, ground.min(pad.y), sh, cam_x, cam_y);
                draw_line(top.x, top.y + deck_h, bot.x, bot.y, 3.0 * dpi,
                    Color::from_rgba(60, 68, 82, 255));
            }
            // Beacons: blinking green until first landing, then steady blue.
            let visited = visited_pads.contains(&pad_key);
            let bc = if visited {
                Color::from_rgba(110, 140, 200, 200)
            } else if (get_time() * 5.0).sin() > 0.0 {
                Color::from_rgba(80, 240, 120, 255)
            } else {
                Color::from_rgba(30, 90, 50, 255)
            };
            for side in [-1.0f32, 1.0] {
                let p = w2s(pad.cx + side * (PAD_HALF_W - 0.15), pad.y + 0.12, sh, cam_x, cam_y);
                draw_circle(p.x, p.y, 2.5 * dpi, bc);
            }
        }

        // Particles
        for p in &particles {
            let s = w2s(p.x, p.y, sh, cam_x, cam_y);
            let a = (p.life * 255.0) as u8;
            let radius = p.life * match p.kind { 0 => 5.0, 3 => 9.0, _ => 3.0 };
            let color = match p.kind {
                0 => Color::from_rgba(255, (120.0 + p.life * 100.0) as u8, 20, a), // orange flame
                3 => Color::from_rgba(255, (60.0 + p.life * 180.0) as u8, (p.life * 80.0) as u8, a), // explosion
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

        // The ship renders while flying (unless it's a wreck) and during the
        // replay (where cam/angle/glow carry the recorded pose); the crash
        // dialog shows no ship — it was just destroyed.
        let ship_visible = match mode {
            Mode::Flying => !crashed,
            Mode::Replay => true,
            Mode::CrashDialog => false,
        };

        // Thruster flame drawn first (behind the hull), out of the engine base
        // at local -Y. Scales with `glow`.
        if ship_visible && glow > 0.02 {
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
        for t in SHIP_TRIS.iter().filter(|_| ship_visible) {
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
        for d in SHIP_DETAILS.iter().filter(|_| ship_visible) {
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

        // Speed danger color, shared by the HUD readout and the (optional)
        // velocity arrow: green = landable, amber = damage-free touch, red =
        // damaging.
        let speed = (ship_vx * ship_vx + ship_vy * ship_vy).sqrt();
        let speed_col = if speed <= 1.0 {
            Color::from_rgba(110, 225, 130, 235)
        } else if speed <= CRASH_DV_SOFT {
            Color::from_rgba(235, 190, 70, 235)
        } else {
            Color::from_rgba(240, 85, 60, 235)
        };
        // Velocity vector (opt-in via the info overlay): an arrow from the
        // ship showing where momentum is carrying it, length grows with
        // speed; near-hover shows nothing.
        if SHOW_VEL.load(Ordering::Relaxed) != 0 && ship_visible && speed > 0.25 {
            let dir = vec2(ship_vx, -ship_vy) / speed; // w2s inverts y
            let ship_scr = vec2(sw / 2.0, sh / 2.0);   // camera is ship-centred
            let p0 = ship_scr + dir * (0.85 * view_scale); // start clear of the hull
            let len = ((14.0 + speed * 13.0) * ui).min(120.0 * ui);
            let p1 = p0 + dir * len;
            let perp = vec2(-dir.y, dir.x);
            draw_line(p0.x, p0.y, p1.x, p1.y, 3.0 * ui, speed_col);
            draw_triangle(p1 + dir * (9.0 * ui), p1 + perp * (5.0 * ui),
                p1 - perp * (5.0 * ui), speed_col);
        }

        smooth_fps += (get_fps() as f32 - smooth_fps) * 0.05;
        let cave_x = cam_x.rem_euclid(PERIOD);
        let hud_fs = 36.0 * ui;
        let hud_y = safe_top + 252.0 * ui; // below the fuel + hull gauges
        let hud = format!("score={}  x={:.0}  lvl={}  {:.0}m/{}m   [R] reset   FPS: {:.0}", score, cam_x, ship_layer, cave_x, PERIOD as i32, smooth_fps);
        draw_text(&hud, safe_left + 10.0 * ui, hud_y, hud_fs, WHITE);
        // Speed readout in the same danger color as the velocity arrow.
        let hud_w = measure_text(&hud, None, hud_fs as u16, 1.0).width;
        draw_text(format!("  v={speed:.1}"),
            safe_left + 10.0 * ui + hud_w, hud_y, hud_fs, speed_col);

        // Crash dialog / replay overlay / status banners. `do_reset` is
        // consumed by the reset block below (same path as the R key).
        let mut do_reset = false;
        if mode == Mode::CrashDialog {
            // Dim the scene so the dialog reads over the cave.
            draw_rectangle(0.0, 0.0, sw, sh, Color::from_rgba(0, 0, 0, 130));
            let msg = "CRASHED";
            let fs = 96.0 * ui;
            let dims = measure_text(msg, None, fs as u16, 1.0);
            draw_text(msg, (sw - dims.width) / 2.0, sh * 0.30, fs,
                Color::from_rgba(255, 90, 60, 255));

            // Two buttons side by side, kept ABOVE the touch-stick zone: the
            // JS stick handler swallows canvas touches in the lower 45% of
            // the viewport before miniquad sees them, so buttons there would
            // be untappable on mobile.
            let bw = 300.0 * ui;
            let bh = 84.0 * ui;
            let gap = 28.0 * ui;
            let by = sh * 0.36;
            let (mx, my) = mouse_position();
            let click = is_mouse_button_pressed(MouseButton::Left);
            let button = |x: f32, label: &str, hint: &str| -> bool {
                let hover = mx >= x && mx <= x + bw && my >= by && my <= by + bh;
                let bg = if hover {
                    Color::from_rgba(60, 80, 120, 235)
                } else {
                    Color::from_rgba(28, 38, 58, 235)
                };
                draw_rectangle(x, by, bw, bh, bg);
                draw_rectangle_lines(x, by, bw, bh, 2.0 * dpi,
                    Color::from_rgba(190, 200, 218, 255));
                let lfs = 34.0 * ui;
                let d = measure_text(label, None, lfs as u16, 1.0);
                draw_text(label, x + (bw - d.width) / 2.0, by + bh * 0.46, lfs, WHITE);
                let hfs = 22.0 * ui;
                let hd = measure_text(hint, None, hfs as u16, 1.0);
                draw_text(hint, x + (bw - hd.width) / 2.0, by + bh * 0.82, hfs,
                    Color::from_rgba(170, 180, 200, 255));
                hover && click
            };
            if button(sw / 2.0 - bw - gap / 2.0, "FLY AGAIN", "[R]") {
                do_reset = true;
            }
            // Hint shows what shipping this run's hybrid replay blob would
            // cost: serialized size raw → deflated.
            let replay_hint = match blob_sizes {
                Some((raw, packed)) => {
                    format!("[ENTER] · {} → {}", fmt_size(raw), fmt_size(packed))
                }
                None => "[ENTER]".to_string(),
            };
            let replay_clicked = button(sw / 2.0 + gap / 2.0, "WATCH REPLAY", &replay_hint);
            if (replay_clicked || is_key_pressed(KeyCode::Enter)) && replay_buf.len() >= 2 {
                mode = Mode::Replay;
                replay_t = 0.0;
            }
        } else if mode == Mode::Replay {
            // Pulsing banner + progress bar; any click/tap (above the stick
            // zone) skips back to the dialog, R skips straight to a respawn.
            let pulse = 180.0 + (get_time() * 4.0).sin() as f32 * 60.0;
            let msg = "REPLAY";
            let fs = 48.0 * ui;
            let dims = measure_text(msg, None, fs as u16, 1.0);
            let msg_y = sh * 0.16;
            draw_text(msg, (sw - dims.width) / 2.0, msg_y, fs,
                Color::from_rgba(255, 220, 120, pulse as u8));
            let hint = "tap to skip";
            let hfs = 24.0 * ui;
            let hd = measure_text(hint, None, hfs as u16, 1.0);
            draw_text(hint, (sw - hd.width) / 2.0, msg_y + 34.0 * ui, hfs,
                Color::from_rgba(200, 205, 220, 160));
            let pw = sw * 0.30;
            let frac = (replay_t / (replay_buf.len().max(2) - 1) as f32).clamp(0.0, 1.0);
            let px = (sw - pw) / 2.0;
            let py = msg_y + 48.0 * ui;
            draw_rectangle(px, py, pw, 4.0 * ui, Color::from_rgba(255, 255, 255, 60));
            draw_rectangle(px, py, pw * frac, 4.0 * ui, Color::from_rgba(255, 220, 120, 200));
            if is_mouse_button_pressed(MouseButton::Left) {
                mode = Mode::CrashDialog;
            }
        } else if crashed {
            let msg = "CRASHED";
            let fs = 96.0 * ui;
            let dims = measure_text(msg, None, fs as u16, 1.0);
            draw_text(msg, (sw - dims.width) / 2.0, sh * 0.42, fs,
                Color::from_rgba(255, 90, 60, 255));
        } else if fuel <= 0.0 {
            let msg = "OUT OF FUEL — [R] RESET";
            let fs = 48.0 * ui;
            let dims = measure_text(msg, None, fs as u16, 1.0);
            draw_text(msg, (sw - dims.width) / 2.0, sh * 0.42, fs,
                Color::from_rgba(255, 180, 60, 255));
        } else if pad_msg_timer > 0.0 {
            let msg = format!("+{PAD_POINTS}");
            let fs = 64.0 * ui;
            let dims = measure_text(&msg, None, fs as u16, 1.0);
            let alpha = (pad_msg_timer / 1.8 * 255.0) as u8;
            draw_text(&msg, (sw - dims.width) / 2.0, sh * 0.38, fs,
                Color::from_rgba(120, 255, 160, alpha));
        } else if landed && (fuel < FUEL_MAX || hull < HULL_MAX) {
            let msg = if fuel < FUEL_MAX { "REFUELING" } else { "REPAIRING" };
            let fs = 36.0 * ui;
            let dims = measure_text(msg, None, fs as u16, 1.0);
            draw_text(msg, (sw - dims.width) / 2.0, sh * 0.38, fs,
                Color::from_rgba(120, 220, 160, 200));
        }

        // Controls
        let rb = rigid_body_set.get_mut(box_handle).unwrap();
        rb.reset_forces(true);
        rb.reset_torques(true);
        if thrusting_now {
            let a = rb.rotation().angle();
            let f = THRUST_FORCE * throttle;
            rb.add_force(vector![-a.sin() * f, a.cos() * f], true);
        }
        // Manual rate rotation: keyboard keys and the gamepad's analog stick.
        let pad_torque = f32::from_bits(PAD_TORQUE.load(Ordering::Relaxed)).clamp(-1.0, 1.0);
        let rcs_ok = mode == Mode::Flying && !crashed && fuel > 0.0;
        let rotating_left  = rcs_ok && (is_key_down(KeyCode::Left)  || pad_torque < -0.1);
        let rotating_right = rcs_ok && (is_key_down(KeyCode::Right) || pad_torque >  0.1);
        // Rotate by firing a side RCS booster: apply the force *at the nozzle*
        // (off-center) instead of a pure couple, so the ship pivots about where
        // the boosters actually push. Nozzles exhaust downward (-Y local) → the
        // reaction force is +Y local at x = ∓0.30: left nozzle → clockwise,
        // right nozzle → counter-clockwise. RCS_FORCE (module const) is tuned
        // so the x-lever (~0.30) yields roughly the previous ±1.0 pure torque.
        let fire_rcs = |rb: &mut RigidBody, side: f32, mag: f32| {
            let (px, py) = lp(0.30 * side, -0.71);
            let (fx, fy) = ld(0.0, mag);
            rb.add_force_at_point(vector![fx, fy], point![px, py], true);
        };
        if rotating_left {
            fire_rcs(rb, -1.0, RCS_FORCE);
        } else if rotating_right {
            fire_rcs(rb, 1.0, RCS_FORCE);
        }

        // Touch heading control: the stick's direction is the commanded nose
        // direction; a PD controller torques the ship toward it the short way
        // around, with authority scaled by deflection so a small nudge trims
        // and full deflection flips hard. Manual rotation (keys/pad) wins
        // while held. Applied as a pure torque (convention-safe: err > 0
        // always means "target is counter-clockwise of the nose in world
        // space", and positive torque rotates counter-clockwise).
        // steer_mag / heading_err come from the throttle block above (screen
        // y grows downward, so stick (x, y) commands world nose (x, -y); the
        // nose at angle a points (-sin a, cos a) → target = atan2(-x, -y)).
        let mut heading_torque = 0.0f32;
        if rcs_ok && steer_mag > 0.0 && !rotating_left && !rotating_right {
            heading_torque = (heading_err * HEADING_KP - rb.angvel() * HEADING_KD)
                .clamp(-HEADING_TORQUE_MAX, HEADING_TORQUE_MAX) * steer_mag;
            rb.add_torque(heading_torque, true);
        }

        // --- Particle emission ---
        let dt = get_frame_time();

        // Which RCS nozzle is puffing: live flags while flying, the recorded
        // side during playback. `last_rcs` feeds the replay recorder.
        let (puff_left, puff_right) = if let Some(f) = replay_frame {
            (f.rcs < 0, f.rcs > 0)
        } else {
            (rotating_left || heading_torque < -0.4,
             rotating_right || heading_torque > 0.4)
        };
        if mode == Mode::Flying {
            last_rcs = if puff_left { -1 } else if puff_right { 1 } else { 0 };
            // Resolved controls for the hybrid recorder (quantized). These
            // drive the physics steps of the NEXT frame, which is exactly
            // when record_tick stores them.
            last_input = InputState {
                throttle: (throttle * 255.0).round() as u8,
                rot: if rotating_left { -1 } else if rotating_right { 1 } else { 0 },
                steer_x: (steer_x.clamp(-1.0, 1.0) * 127.0).round() as i8,
                steer_y: (steer_y.clamp(-1.0, 1.0) * 127.0).round() as i8,
                stick_held: stick_held as u8,
            };
        }

        // Burn fuel for whatever fired this frame (main burn scales with throttle).
        if thrusting_now {
            fuel -= FUEL_BURN_MAIN * throttle * dt;
        }
        if rotating_left || rotating_right {
            fuel -= FUEL_BURN_RCS * dt;
        } else if heading_torque != 0.0 {
            // Heading control sips proportionally to the torque it commands.
            fuel -= FUEL_BURN_RCS * (heading_torque.abs() / HEADING_TORQUE_MAX) * dt;
        }
        fuel = fuel.max(0.0);

        // Main thruster: exhaust exits local -Y (out the bottom), up to 8
        // particles/frame — count and exhaust speed scale with the throttle.
        // During playback the recorded glow stands in for the throttle so the
        // replayed burn trails exhaust too (fuel burn above stays live-only).
        let exhaust = match replay_frame {
            Some(f) if f.glow > 0.05 => f.glow,
            Some(_) => 0.0,
            None => throttle,
        };
        if exhaust > 0.0 {
            for _ in 0..(8.0 * exhaust).ceil() as i32 {
                let spread = gen_range(-0.25f32, 0.25);
                let (px, py) = lp(spread * 0.45, -0.72);
                let speed = gen_range(4.0f32, 8.0) * (0.4 + 0.6 * exhaust);
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
        // Heading control maps by torque sign: fire_rcs(-1) produces negative
        // torque, so negative heading torque puffs the LEFT nozzle and
        // positive the RIGHT (threshold keeps small trim corrections silent).
        // Coords are in scaled world units — lp() does NOT apply SHIP_SCALE.
        if puff_left {
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
        if puff_right {
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
        let decay_boom = dt / 1.1;  // explosion debris lives ~1.1s
        for p in &mut particles {
            let decay = match p.kind { 0 => decay_main, 3 => decay_boom, _ => decay_rcs };
            p.life -= decay;
            p.x += p.vx * dt;
            p.y += p.vy * dt;
        }
        particles.retain(|p| p.life > 0.0);

        // Reset / respawn: R key, gamepad Start/Y, or the dialog's FLY AGAIN
        // button. Also the escape hatch out of the dialog and the replay.
        if is_key_pressed(KeyCode::R) || PAD_RESET.swap(0, Ordering::Relaxed) != 0 || do_reset {
            let rb = rigid_body_set.get_mut(box_handle).unwrap();
            rb.set_gravity_scale(1.0, true); // in case we reset out of a crash
            rb.set_translation(vector![RESET_X, stand_y(RESET_X)], true);
            rb.set_linvel(vector![0.0, 0.0], true);
            rb.set_angvel(0.0, true);
            rb.set_rotation(Rotation::new(0.0), true);
            // Snap the interpolation + crash state too, or the camera lerps
            // across the teleport and the velocity jump reads as an impact.
            prev_ship = (RESET_X, stand_y(RESET_X), 0.0);
            prev_vel = (0.0, 0.0);
            crash_timer = 0.0;
            fuel = FUEL_MAX;
            hull = HULL_MAX;
            shake = 0.0;
            mode = Mode::Flying;
            // A fresh attempt records a fresh replay (both formats).
            replay_buf.clear();
            replay_t = 0.0;
            last_rcs = 0;
            glow = 0.0;
            recorder = Recording::new(sim_params(), (REPLAY_MAX_SECS / PHYSICS_DT) as u32);
            recorder.push_keyframe(spawn_keyframe(RESET_X));
            last_input = InputState::default();
            blob_sizes = None;
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

        // Pad markers on the minimap: bright line at deck height, green until
        // visited, blue-grey after.
        for (&pad_key, pad) in pads.iter() {
            if (pad.cx - cam_x).abs() > MM_HALF_X + 5.0 || (pad.y - cam_y).abs() > MM_HALF_Y + 5.0 {
                continue;
            }
            let y = to_mm_y(pad.y).clamp(mm_oy, mm_oy + mm_h);
            let x0 = to_mm_x(pad.cx - PAD_HALF_W).clamp(mm_ox, mm_ox + mm_w);
            let x1 = to_mm_x(pad.cx + PAD_HALF_W).clamp(mm_ox, mm_ox + mm_w);
            let c = if visited_pads.contains(&pad_key) {
                Color::from_rgba(110, 140, 200, 220)
            } else {
                Color::from_rgba(90, 240, 130, 255)
            };
            draw_line(x0, y, x1, y, 2.0 * dpi, c);
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
        draw_circle(mm_cx, mm_cy, 3.0 * ui, YELLOW);

        // Border
        draw_rectangle_lines(mm_ox, mm_oy, mm_w, mm_h, 1.0, Color::from_rgba(255, 255, 255, 120));

        // Fuel gauge — slim bar directly under the minimap.
        let fg_y = mm_oy + mm_h + 8.0 * ui;
        let fg_h = 14.0 * ui;
        let frac = fuel / FUEL_MAX;
        let fg_col = if frac > 0.5 {
            Color::from_rgba(90, 200, 120, 255)
        } else if frac > 0.25 {
            Color::from_rgba(230, 180, 60, 255)
        } else {
            Color::from_rgba(220, 70, 50, 255)
        };
        draw_rectangle(mm_ox, fg_y, mm_w, fg_h, mm_dark);
        draw_rectangle(mm_ox, fg_y, mm_w * frac, fg_h, fg_col);
        draw_rectangle_lines(mm_ox, fg_y, mm_w, fg_h, 1.0, Color::from_rgba(255, 255, 255, 120));

        // Hull gauge — same slim bar directly under the fuel gauge.
        let hg_y = fg_y + fg_h + 6.0 * ui;
        let hfrac = hull / HULL_MAX;
        let hg_col = if hfrac > 0.5 {
            Color::from_rgba(150, 175, 215, 255)
        } else if hfrac > 0.25 {
            Color::from_rgba(230, 180, 60, 255)
        } else {
            Color::from_rgba(220, 70, 50, 255)
        };
        draw_rectangle(mm_ox, hg_y, mm_w, fg_h, mm_dark);
        draw_rectangle(mm_ox, hg_y, mm_w * hfrac, fg_h, hg_col);
        draw_rectangle_lines(mm_ox, hg_y, mm_w, fg_h, 1.0, Color::from_rgba(255, 255, 255, 120));

        next_frame().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The whole world is pure functions of position/slot index. These tests
    // pin the invariants the rendering and collision code rely on.

    #[test]
    fn cave_repeats_every_period() {
        for i in 0..200 {
            let x = i as f32 * 3.7 - 300.0;
            assert!((cave_center(x + PERIOD) - cave_center(x)).abs() < 1e-3);
            assert!((cave_half_width(x + PERIOD) - cave_half_width(x)).abs() < 1e-3);
        }
    }

    #[test]
    fn cave_never_pinches_shut() {
        for i in 0..6000 {
            let x = i as f32 * 0.1;
            assert!(cave_half_width(x) > 1.0, "cave too narrow at x={x}");
        }
    }

    #[test]
    fn lattice_row0_is_exactly_on_the_collider_line() {
        // The hard rendering rule: row 0 must coincide with the wall edge that
        // the segment colliders are built from — bit-exact, no jitter.
        for col in -500..500 {
            let x = col_x(col);
            let top = lattice_point(col, 0, 0);
            let bot = lattice_point(col, 0, 1);
            assert_eq!(top, vec2(x, cave_center(x) + cave_half_width(x)));
            assert_eq!(bot, vec2(x, cave_center(x) - cave_half_width(x)));
        }
    }

    #[test]
    fn lattice_jitter_only_goes_into_the_rock() {
        for col in -200..200 {
            for row in 1..N_ROWS {
                let top = lattice_point(col, row, 0);
                let bot = lattice_point(col, row, 1);
                let edge_top = lattice_point(col, 0, 0).y;
                let edge_bot = lattice_point(col, 0, 1).y;
                assert!(top.y > edge_top, "ceiling row {row} pokes into the cave");
                assert!(bot.y < edge_bot, "floor row {row} pokes into the cave");
            }
        }
    }

    #[test]
    fn obstacles_are_deterministic() {
        for k in -50..50 {
            let (a, b) = (obstacle_spec(k), obstacle_spec(k));
            match (a, b) {
                (None, None) => {}
                (Some(a), Some(b)) => {
                    assert_eq!(a.cx, b.cx);
                    assert_eq!(a.cy, b.cy);
                    assert_eq!(a.rot, b.rot);
                    assert_eq!(a.pts, b.pts);
                }
                _ => panic!("slot {k} not deterministic"),
            }
        }
    }

    #[test]
    fn obstacles_keep_clear_of_spawn_reset_and_walls() {
        for k in -200..200 {
            let Some(spec) = obstacle_spec(k) else { continue };
            assert!(spec.cx.abs() >= 9.0, "slot {k} too close to spawn");
            assert!((spec.cx - RESET_X).abs() >= 9.0, "slot {k} too close to reset");
            // Documented invariant: >= 1.3 m gap to the nearer wall.
            let max_r = spec
                .pts
                .iter()
                .map(|p| (p.x * p.x + p.y * p.y).sqrt())
                .fold(0.0f32, f32::max);
            let off = (spec.cy - cave_center(spec.cx)).abs();
            let hw = cave_half_width(spec.cx);
            assert!(
                off + max_r <= hw - 1.3 + 1e-3,
                "slot {k}: gap {} < 1.3 m",
                hw - off - max_r
            );
        }
    }

    #[test]
    fn shaft_pattern_repeats_every_period() {
        // 4 slots per PERIOD (50 segs * 3 m * 4 = 600 m): slot s+4 must be the
        // exact translate of slot s so the x-wrap stays seamless.
        for s in -8..8 {
            assert_eq!(
                shaft_open_seg(s + 4),
                shaft_open_seg(s) + 4 * SHAFT_SPACING_SEGS
            );
            for side in [0u8, 1] {
                for i in 0..=10 {
                    let t = i as f32 / 10.0;
                    let dx = shaft_wall_x(s + 4, side, t) - shaft_wall_x(s, side, t);
                    assert!((dx - PERIOD).abs() < 1e-3);
                }
            }
        }
    }

    #[test]
    fn shaft_openings_match_wall_gaps() {
        for s in -8..8 {
            let o = shaft_open_seg(s);
            assert!(!seg_in_opening(o - 1));
            for idx in o..o + SHAFT_OPEN_SEGS {
                assert!(seg_in_opening(idx), "segment {idx} of shaft {s} has a wall");
            }
            assert!(!seg_in_opening(o + SHAFT_OPEN_SEGS));
        }
    }

    #[test]
    fn shafts_stay_flyable() {
        // Opening is 9 m; the wiggle envelope must leave >= 6.5 m of width.
        for s in 0..4 {
            for i in 0..=100 {
                let t = i as f32 / 100.0;
                let width = shaft_wall_x(s, 1, t) - shaft_wall_x(s, 0, t);
                assert!(width >= 6.5 - 1e-3, "shaft {s} narrows to {width} at t={t}");
            }
        }
    }

    #[test]
    fn rng_is_deterministic_and_in_range() {
        let (mut a, mut b) = (Rng::new(42), Rng::new(42));
        for _ in 0..1000 {
            let u = a.unit();
            assert_eq!(u, b.unit());
            assert!((0.0..1.0).contains(&u));
        }
        let mut r = Rng::new(7);
        for _ in 0..1000 {
            let v = r.range_int(3, 9);
            assert!((3..=9).contains(&v));
        }
    }

    #[test]
    fn lerp_angle_takes_the_short_way() {
        use std::f32::consts::PI;
        // Plain case: no wrap involved.
        assert!((lerp_angle(0.0, 1.0, 0.5) - 0.5).abs() < 1e-6);
        // Across the ±π seam: 170° → -170° should pass through 180°, not 0°.
        let a = 170.0f32.to_radians();
        let b = -170.0f32.to_radians();
        let mid = lerp_angle(a, b, 0.5);
        assert!((mid.abs() - PI).abs() < 1e-5, "went the long way: {mid}");
        // Endpoints are exact.
        assert!((lerp_angle(a, b, 0.0) - a).abs() < 1e-6);
        assert!((lerp_angle(a, b, 1.0) - (a + (b - a + std::f32::consts::TAU))).abs() < 1e-5);
    }

    #[test]
    fn wav_header_is_well_formed() {
        let wav = wav_from_samples(&[0i16; 100], AUDIO_RATE);
        assert_eq!(wav.len(), 44 + 200);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..16], b"WAVEfmt ");
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 200);
    }

    #[test]
    fn pads_are_deterministic_and_sit_on_the_floor() {
        for p in -100..100 {
            let Some(pad) = pad_spec(p) else { continue };
            let again = pad_spec(p).unwrap();
            assert_eq!((pad.cx, pad.y), (again.cx, again.y), "pad {p} not deterministic");
            // Deck top must clear the floor across the whole span (collider
            // never dips into rock) without floating absurdly high.
            for i in 0..=20 {
                let x = pad.cx - PAD_HALF_W + i as f32 * (PAD_HALF_W / 10.0);
                let floor = cave_center(x) - cave_half_width(x);
                assert!(pad.y >= floor, "pad {p} deck below floor at x={x}");
                assert!(pad.y - floor < 4.0, "pad {p} floats {}m high", pad.y - floor);
            }
        }
    }

    #[test]
    fn pads_keep_clear_of_shafts_and_boulders() {
        for p in -100..100 {
            let Some(pad) = pad_spec(p) else { continue };
            // Not inside a shaft opening column (8 m rule).
            let seg = (pad.cx / SEG_LEN).floor() as i64;
            let s0 = (seg - SHAFT_BASE_SEG).div_euclid(SHAFT_SPACING_SEGS);
            for s in [s0, s0 + 1] {
                let o = shaft_open_seg(s);
                let (ox0, ox1) = (o as f32 * SEG_LEN, (o + SHAFT_OPEN_SEGS) as f32 * SEG_LEN);
                assert!(
                    pad.cx <= ox0 - 8.0 || pad.cx >= ox1 + 8.0,
                    "pad {p} inside shaft clearance"
                );
            }
            // No boulder overlapping the deck.
            let k0 = (pad.cx / OBSTACLE_SPACING).round() as i64;
            for k in k0 - 1..=k0 + 1 {
                if let Some(ob) = obstacle_spec(k) {
                    let r = ob.pts.iter()
                        .map(|q| (q.x * q.x + q.y * q.y).sqrt())
                        .fold(0.0f32, f32::max);
                    assert!(
                        (ob.cx - pad.cx).abs() >= r + PAD_HALF_W + 1.0 - 1e-3,
                        "pad {p} overlaps boulder {k}"
                    );
                }
            }
        }
    }

    #[test]
    fn spawn_and_reset_stand_on_solid_ground() {
        for x in [0.0f32, RESET_X] {
            let y = stand_y(x);
            let floor = cave_center(x) - cave_half_width(x);
            // Feet (0.73 below origin) rest on or just above floor/deck level,
            // never below the floor.
            assert!(y - 0.73 >= floor - 1e-3);
            assert!(y - 0.73 <= floor + 1.5, "spawn at x={x} would drop and crash");
        }
    }
}
