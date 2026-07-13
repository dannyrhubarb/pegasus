// The deterministic simulation core: everything that decides where the ship
// goes lives here, advanced exclusively in fixed PHYSICS_DT ticks driven by
// a quantized per-tick InputState. The renderer/main loop supplies inputs
// and reads state; it never touches forces, fuel, damage or colliders.
//
// Determinism contract: given the same initial Keyframe and the same
// sequence of InputState per tick, `Sim` reproduces the same trajectory
// bit-for-bit (same binary). Everything that feeds physics is tick-driven:
// - forces/torques are recomputed and applied every tick from the input
//   (previously they were set once per render frame and persisted across
//   substeps — the sim outcome depended on display refresh rate);
// - fuel burn, hull damage, landing timers and pad refuel/repair advance by
//   PHYSICS_DT, not frame time;
// - the collider sliding windows are keyed off the TRUE body position (the
//   render camera adds interpolation + screen shake) and are stored in
//   BTreeMaps so insertion/removal order — and therefore Rapier handle
//   assignment and solver iteration order — is identical across runs;
// - window syncs happen inside tick(), only when the ship's (segment,
//   layer) changes, so live play and resim perform the identical operation
//   sequence at the identical ticks.
// `resim()` re-runs a hybrid Recording through a fresh Sim, which is what
// makes recorded runs verifiable and shareable as pure input streams.

use macroquad::prelude::Vec2;
use rapier2d::prelude::*;
use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};

use crate::replay::{InputState, Keyframe, Recording, SimParams};
use crate::world::*;

pub const PHYSICS_DT: f32 = 1.0 / 120.0;
// Where every run starts AND where reset/respawn returns: a shared start
// line is what lets the last-run ghost race you (it stands on pad 0).
pub const SPAWN_X: f32 = 0.0;
// How many segments to keep loaded on each side of the ship.
pub const HALF_WINDOW: i64 = 80;
pub const GRAVITY_Y: f32 = -1.62;
pub const THRUST_FORCE: f32 = 8.0; // main engine at full throttle
pub const LINEAR_DAMPING: f32 = 0.2;
pub const ANGULAR_DAMPING: f32 = 3.0;
// Side RCS booster force, applied at the nozzle (x-lever ~0.30 → roughly
// ±1.0 of torque).
pub const RCS_FORCE: f32 = 3.3;
// Touch heading control: PD to the commanded nose direction (see
// docs/control-tuning.md). Now applied per physics tick (120 Hz).
pub const HEADING_KP: f32 = 14.0;
pub const HEADING_KD: f32 = 2.2;
pub const HEADING_TORQUE_MAX: f32 = 6.0;
// Impact grading (per-tick velocity change; a collision impulse lands
// within one tick, while gravity/thrust move v by < 0.05 m/s per tick).
pub const CRASH_DV_SOFT: f32 = 2.5;
pub const CRASH_DV_HARD: f32 = 6.0;
pub const HULL_MAX: f32 = 100.0;
pub const HULL_REPAIR_PER_S: f32 = 20.0;
pub const FUEL_MAX: f32 = 100.0;
pub const FUEL_BURN_MAIN: f32 = 3.5; // units/s at full throttle
pub const FUEL_BURN_RCS: f32 = 1.2;  // units/s while an RCS nozzle fires
// The run ends this long after the tank empties, moving or not
// (`TickReport::fuel_out`; main turns it into the game-over flow so the
// run reaches the submit dialog instead of stranding the player until a
// manual reset). A pad catch inside the window refuels (fuel > 0 resets
// the timer) and cancels it. Detection only: no force depends on it, so
// it isn't part of SimParams.
pub const FUEL_OUT_END_SECS: f32 = 2.5;

// The exact simulation constants this build runs with, serialized into every
// replay blob so a recording can be re-run under the rules it was flown with.
pub fn sim_params() -> SimParams {
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

// The ship's state at a spawn/reset point: standing on the floor (or pad 0
// at the origin), upright, still, tanks full.
pub fn spawn_keyframe(level: &Level, x: f32) -> Keyframe {
    Keyframe {
        tick: 0,
        x,
        y: level.stand_y(x),
        rot_re: 1.0, // upright: unit complex for angle 0
        rot_im: 0.0,
        vx: 0.0,
        vy: 0.0,
        angvel: 0.0,
        fuel: FUEL_MAX,
        hull: HULL_MAX,
        glow: 0.0,
        land_timer: 0.0,
    }
}

pub struct Shaft {
    pub handles: Vec<ColliderHandle>,
    pub walls: [Vec<Vec2>; 2], // left / right wall polylines, world space
}

pub struct Obstacle {
    pub handle: ColliderHandle,
    pub cx: f32,
    pub cy: f32,
    pub rot: f32,
    pub verts: Vec<Vec2>, // hull vertices (local space), read back from the collider
}

pub struct Pad {
    pub handle: ColliderHandle,
    pub cx: f32,
    pub y: f32, // deck top (collider line), layer offset applied
}

// What one tick did, for the frame loop's cosmetics (sparks, sounds, shake,
// score flash, RCS puffs). Nothing in here feeds back into the sim.
#[derive(Default)]
pub struct TickReport {
    pub impact: Option<Impact>,
    pub landed: bool,         // settled on a pad past PAD_LAND_TIME
    pub scored: bool,         // first visit registered this tick
    pub heading_torque: f32,  // PD torque applied (for nozzle puffs)
    pub fuel_out: bool,       // stranded dry past FUEL_OUT_END_SECS: run over
}

// An impact this tick (dv above CRASH_DV_SOFT). Carries the post-impact
// pose/velocity because a destroying impact parks the wreck immediately —
// by the time the frame loop sees the report, the body is zeroed. The
// heading is the exact unit-complex rotation so the terminal keyframe built
// from it stays bit-faithful.
#[derive(Clone, Copy)]
pub struct Impact {
    pub dv: f32,
    pub damage: f32,
    pub destroyed: bool,
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
    pub rot_re: f32,
    pub rot_im: f32,
    pub angvel: f32,
}

pub struct Sim {
    bodies: RigidBodySet,
    colliders: ColliderSet,
    physics_pipeline: PhysicsPipeline,
    island_manager: IslandManager,
    broad_phase: DefaultBroadPhase,
    narrow_phase: NarrowPhase,
    impulse_joints: ImpulseJointSet,
    multibody_joints: MultibodyJointSet,
    ccd_solver: CCDSolver,
    query_pipeline: QueryPipeline,
    integration_params: IntegrationParameters,
    gravity: Vector<f32>,
    ship: RigidBodyHandle,

    // Sliding collider windows. BTreeMap (not HashMap): iteration order in
    // the sync's retain/insert loops determines Rapier handle assignment,
    // which must be identical across runs for bit-exact resim.
    cave: BTreeMap<(i64, i64), Vec<ColliderHandle>>,
    pub shafts: BTreeMap<(i64, i64), Shaft>,
    pub obstacles: BTreeMap<(i64, i64), Obstacle>,
    pub pads: BTreeMap<(i64, i64), Pad>,
    synced_at: Option<(i64, i64)>, // (segment, layer) of the last window sync

    // The world this sim generates around the ship. Immutable for the sim's
    // lifetime — switching level means a fresh Sim (same rule as a new run).
    pub level: Level,

    // Ship systems (all tick-driven).
    pub fuel: f32,
    pub hull: f32,
    pub score: u32,
    pub max_dist: f32, // farthest |x| this run (the Distance-scoring metric)
    pub visited_pads: BTreeSet<(i64, i64)>,
    pub crashed: bool,
    land_timer: f32,
    fuel_out_timer: f32,
    prev_vel: (f32, f32),
}

impl Sim {
    pub fn new(level: Level) -> Sim {
        let mut bodies = RigidBodySet::new();
        let mut colliders = ColliderSet::new();

        let body = RigidBodyBuilder::dynamic()
            .translation(vector![0.0, level.stand_y(0.0)])
            .angular_damping(ANGULAR_DAMPING)
            // A whisper of drag: imperceptible at landing speeds but it caps
            // how much momentum can pile up on a long burn or free-fall.
            .linear_damping(LINEAR_DAMPING)
            .ccd_enabled(true)
            .build();
        let ship = bodies.insert(body);
        // Compound collider of three capsules tracing the 1.5× scaled lander
        // (see CLAUDE.md "Physics notes"). Endpoints in scaled world units.
        colliders.insert_with_parent(
            ColliderBuilder::new(SharedShape::capsule(
                point![0.0, 0.42], point![0.0, -0.08], 0.26))
                .restitution(0.2).build(),
            ship, &mut bodies,
        );
        colliders.insert_with_parent(
            ColliderBuilder::new(SharedShape::capsule(
                point![-0.26, -0.30], point![-0.33, -0.64], 0.09))
                .restitution(0.2).build(),
            ship, &mut bodies,
        );
        colliders.insert_with_parent(
            ColliderBuilder::new(SharedShape::capsule(
                point![0.26, -0.30], point![0.33, -0.64], 0.09))
                .restitution(0.2).build(),
            ship, &mut bodies,
        );

        let mut sim = Sim {
            bodies,
            colliders,
            physics_pipeline: PhysicsPipeline::new(),
            island_manager: IslandManager::new(),
            broad_phase: DefaultBroadPhase::new(),
            narrow_phase: NarrowPhase::new(),
            impulse_joints: ImpulseJointSet::new(),
            multibody_joints: MultibodyJointSet::new(),
            ccd_solver: CCDSolver::new(),
            query_pipeline: QueryPipeline::new(),
            integration_params: IntegrationParameters {
                dt: PHYSICS_DT,
                num_solver_iterations: std::num::NonZeroUsize::new(8).unwrap(),
                ..Default::default()
            },
            gravity: vector![0.0, GRAVITY_Y],
            ship,
            cave: BTreeMap::new(),
            shafts: BTreeMap::new(),
            obstacles: BTreeMap::new(),
            pads: BTreeMap::new(),
            synced_at: None,
            level,
            fuel: FUEL_MAX,
            hull: HULL_MAX,
            score: 0,
            max_dist: 0.0,
            visited_pads: BTreeSet::new(),
            crashed: false,
            land_timer: 0.0,
            fuel_out_timer: 0.0,
            prev_vel: (0.0, 0.0),
        };
        // Seed the collider window at the spawn so even the very first tick
        // has ground under the ship.
        let kf = spawn_keyframe(&sim.level, SPAWN_X);
        sim.restore(&kf);
        sim
    }

    // Place the ship in the state a Keyframe describes. Used by reset (spawn
    // keyframe) and by resim (a recording's first keyframe). Score/visited
    // pads are session state, not run state — deliberately untouched.
    pub fn restore(&mut self, kf: &Keyframe) {
        let rb = self.bodies.get_mut(self.ship).unwrap();
        rb.set_gravity_scale(1.0, true);
        rb.set_translation(vector![kf.x, kf.y], true);
        // new_unchecked, NOT Rotation::new / from_complex: the keyframe holds
        // the body's original unit complex verbatim, and any re-normalisation
        // or angle round-trip would change its bits (= sub-mm restore drift).
        rb.set_rotation(
            Rotation::new_unchecked(rapier2d::na::Complex::new(kf.rot_re, kf.rot_im)),
            true,
        );
        rb.set_linvel(vector![kf.vx, kf.vy], true);
        rb.set_angvel(kf.angvel, true);
        self.fuel = kf.fuel;
        self.hull = kf.hull;
        self.crashed = false;
        self.land_timer = kf.land_timer;
        self.fuel_out_timer = 0.0;
        self.max_dist = kf.x.abs();
        self.prev_vel = (kf.vx, kf.vy);
        self.sync_window(Self::window_key(kf.x, kf.y));
    }

    // Teleport this sim back to a spawn. WARNING: do NOT use this to start a
    // new RECORDED run — a reused sim's collider-handle space differs from
    // the fresh sim a replay uses, and Rapier's contact solve is sensitive
    // to handle numbering (see the fresh-sim regression test). The game
    // creates a fresh Sim per run instead; this is for tests/tools.
    #[allow(dead_code)]
    pub fn reset(&mut self, x: f32) {
        let kf = spawn_keyframe(&self.level, x);
        self.restore(&kf);
    }

    pub fn ship_pose(&self) -> (f32, f32, f32) {
        let b = &self.bodies[self.ship];
        (b.translation().x, b.translation().y, b.rotation().angle())
    }

    pub fn ship_vel(&self) -> (f32, f32) {
        let v = self.bodies[self.ship].linvel();
        (v.x, v.y)
    }

    pub fn ship_angvel(&self) -> f32 {
        self.bodies[self.ship].angvel()
    }

    pub fn keyframe(&self, tick: u32, glow: f32) -> Keyframe {
        let b = &self.bodies[self.ship];
        let rot = *b.rotation();
        let (vx, vy) = self.ship_vel();
        Keyframe {
            tick,
            x: b.translation().x,
            y: b.translation().y,
            rot_re: rot.re, // exact unit-complex heading, not an angle —
            rot_im: rot.im, // see the Keyframe doc comment in replay.rs
            vx, vy,
            angvel: self.ship_angvel(),
            fuel: self.fuel,
            hull: self.hull,
            glow,
            land_timer: self.land_timer,
        }
    }

    // Advance the world by one PHYSICS_DT under `input`.
    pub fn tick(&mut self, input: InputState) -> TickReport {
        // Slide the collider windows when the ship's (segment, layer)
        // changed — keyed off the true body position so live play and resim
        // perform identical window ops at identical ticks.
        {
            let (bx, by, _) = self.ship_pose();
            let key = Self::window_key(bx, by);
            if self.synced_at != Some(key) {
                self.sync_window(key);
            }
        }

        let mut report = TickReport::default();

        if !self.crashed {
            // Inputs are COMMANDS; the fuel gate lives here so an empty tank
            // behaves identically in live play and resim.
            let rcs_ok = self.fuel > 0.0;
            let throttle = if rcs_ok { input.throttle_f32() } else { 0.0 };
            let rot = if rcs_ok { input.rot } else { 0 };
            let (steer_x, steer_y) = input.steer_f32();
            let steer_mag = (steer_x * steer_x + steer_y * steer_y).sqrt().min(1.0);

            let rb = self.bodies.get_mut(self.ship).unwrap();
            rb.reset_forces(true);
            rb.reset_torques(true);
            let a = rb.rotation().angle();

            if throttle > 0.0 {
                let f = THRUST_FORCE * throttle;
                rb.add_force(vector![-a.sin() * f, a.cos() * f], true);
            }

            // Manual rate rotation: fire a side RCS booster at the nozzle
            // (off-center, gas out −Y local) so the ship pivots about where
            // the boosters actually push. Left nozzle (rot < 0) at scaled-
            // local (−0.30, −0.71), right mirrored.
            if rot != 0 {
                let side = rot.signum() as f32;
                let (lx, ly) = (0.30 * side, -0.71);
                let px = rb.translation().x + lx * a.cos() - ly * a.sin();
                let py = rb.translation().y + lx * a.sin() + ly * a.cos();
                let (fx, fy) = (-RCS_FORCE * a.sin(), RCS_FORCE * a.cos());
                rb.add_force_at_point(vector![fx, fy], point![px, py], true);
            }

            // Touch heading control: PD to the commanded nose direction,
            // shortest way around, authority scaled by deflection. Manual
            // rotation wins while held. Runs per tick (120 Hz), so damping
            // acts on the freshest angular velocity.
            let mut heading_torque = 0.0f32;
            if rcs_ok && steer_mag > 0.0 && rot == 0 {
                let target = (-steer_x).atan2(-steer_y);
                let mut err = target - a;
                if err > std::f32::consts::PI { err -= std::f32::consts::TAU; }
                if err < -std::f32::consts::PI { err += std::f32::consts::TAU; }
                heading_torque = (err * HEADING_KP - rb.angvel() * HEADING_KD)
                    .clamp(-HEADING_TORQUE_MAX, HEADING_TORQUE_MAX) * steer_mag;
                rb.add_torque(heading_torque, true);
            }
            report.heading_torque = heading_torque;

            // Fuel burn for whatever fired this tick.
            if throttle > 0.0 {
                self.fuel -= FUEL_BURN_MAIN * throttle * PHYSICS_DT;
            }
            if rot != 0 {
                self.fuel -= FUEL_BURN_RCS * PHYSICS_DT;
            } else if heading_torque != 0.0 {
                self.fuel -=
                    FUEL_BURN_RCS * (heading_torque.abs() / HEADING_TORQUE_MAX) * PHYSICS_DT;
            }
            self.fuel = self.fuel.max(0.0);
        }

        self.physics_pipeline.step(
            &self.gravity,
            &self.integration_params,
            &mut self.island_manager,
            &mut self.broad_phase,
            &mut self.narrow_phase,
            &mut self.bodies,
            &mut self.colliders,
            &mut self.impulse_joints,
            &mut self.multibody_joints,
            &mut self.ccd_solver,
            Some(&mut self.query_pipeline),
            &(),
            &(),
        );

        let (x, y, _) = self.ship_pose();
        let (vx, vy) = self.ship_vel();

        if !self.crashed {
            // Distance-scoring metric (harmless to track on every level).
            self.max_dist = self.max_dist.max(x.abs());
        }

        if !self.crashed {
            // Impact = per-tick velocity jump (a collision impulse resolves
            // within one tick; gravity/thrust move v by < 0.05 m/s per tick).
            let (dvx, dvy) = (vx - self.prev_vel.0, vy - self.prev_vel.1);
            let dv = (dvx * dvx + dvy * dvy).sqrt();
            if dv > CRASH_DV_SOFT {
                let damage = (dv - CRASH_DV_SOFT) / (CRASH_DV_HARD - CRASH_DV_SOFT) * HULL_MAX;
                self.hull -= damage;
                let destroyed = dv > CRASH_DV_HARD || self.hull <= 0.0;
                let rot = *self.bodies[self.ship].rotation();
                report.impact = Some(Impact {
                    dv, damage, destroyed, x, y, vx, vy,
                    rot_re: rot.re, rot_im: rot.im,
                    angvel: self.ship_angvel(),
                });
                if destroyed {
                    self.hull = 0.0;
                    self.crashed = true;
                    // Park the wreck where it died so the camera holds still.
                    let rb = self.bodies.get_mut(self.ship).unwrap();
                    rb.set_linvel(vector![0.0, 0.0], true);
                    rb.set_angvel(0.0, true);
                    rb.set_gravity_scale(0.0, true);
                }
            }
        }

        if !self.crashed {
            // Landing: settled on a pad deck (slow, upright, feet on the
            // deck) for PAD_LAND_TIME. First visit scores; parked ships
            // refuel and repair.
            let b = &self.bodies[self.ship];
            let settled = b.rotation().angle().abs() < 0.30
                && vx.abs() < 1.0
                && vy.abs() < 1.0
                && b.angvel().abs() < 0.5;
            let on_pad = settled
                .then(|| {
                    let feet = y - 0.73;
                    self.pads.iter().find_map(|(&key, pad)| {
                        ((x - pad.cx).abs() <= PAD_HALF_W && (feet - pad.y).abs() < 0.3)
                            .then_some(key)
                    })
                })
                .flatten();
            if let Some(key) = on_pad {
                self.land_timer += PHYSICS_DT;
                if self.land_timer >= PAD_LAND_TIME {
                    // First visits always register (beacons turn blue), but
                    // they only pay points on Pads-scoring levels — on
                    // Distance levels the score IS max |x|.
                    if self.visited_pads.insert(key) && self.level.scoring == Scoring::Pads {
                        self.score += PAD_POINTS;
                        report.scored = true;
                    }
                    self.fuel = (self.fuel + PAD_REFUEL_PER_S * PHYSICS_DT).min(FUEL_MAX);
                    self.hull = (self.hull + HULL_REPAIR_PER_S * PHYSICS_DT).min(HULL_MAX);
                    report.landed = true;
                }
            } else {
                self.land_timer = 0.0;
            }

            // Out-of-fuel game over: the run ends FUEL_OUT_END_SECS after
            // the tank empties — moving or not (the final coast still earns
            // distance for that window). A pad catch refuels (the refuel
            // block above runs first, so fuel > 0 clears the timer this
            // same tick). Pure detection — nothing feeds back into the
            // physics, so replay determinism is untouched.
            if self.fuel <= 0.0 {
                self.fuel_out_timer += PHYSICS_DT;
            } else {
                self.fuel_out_timer = 0.0;
            }
            report.fuel_out = self.fuel_out_timer >= FUEL_OUT_END_SECS;
        } else {
            self.land_timer = 0.0;
        }

        self.prev_vel = (vx, vy);
        report
    }

    fn window_key(x: f32, y: f32) -> (i64, i64) {
        ((x / SEG_LEN).floor() as i64, (y / V_PERIOD).round() as i64)
    }

    // Slide all four collider windows around (ship_seg, ship_layer). Every
    // loop below iterates in key order (BTreeMap / ordered ranges), so the
    // sequence of Rapier insert/remove ops is deterministic.
    fn sync_window(&mut self, key: (i64, i64)) {
        let (ship_seg, ship_layer) = key;
        let want_left = ship_seg - HALF_WINDOW;
        let want_right = ship_seg + HALF_WINDOW;
        let (lay_lo, lay_hi) = (ship_layer - 1, ship_layer + 1);

        // Cave wall segments (2D window: segments × layers).
        let level = &self.level;
        let (colliders, island_manager, bodies) =
            (&mut self.colliders, &mut self.island_manager, &mut self.bodies);
        self.cave.retain(|&(layer, idx), handles| {
            if layer < lay_lo || layer > lay_hi || idx < want_left || idx > want_right {
                for h in handles.drain(..) {
                    colliders.remove(h, island_manager, bodies, false);
                }
                false
            } else {
                true
            }
        });
        for layer in lay_lo..=lay_hi {
            for idx in want_left..=want_right {
                self.cave
                    .entry((layer, idx))
                    .or_insert_with(|| level.insert_seg(idx, layer, colliders));
            }
        }

        // Vertical shafts for the gaps below/above the ship's layer.
        let s_lo = want_left.div_euclid(SHAFT_SPACING_SEGS) - 1;
        let s_hi = want_right.div_euclid(SHAFT_SPACING_SEGS) + 1;
        self.shafts.retain(|&(s, gap), sh| {
            if s < s_lo || s > s_hi || gap < ship_layer - 1 || gap > ship_layer {
                for h in sh.handles.drain(..) {
                    colliders.remove(h, island_manager, bodies, false);
                }
                false
            } else {
                true
            }
        });
        for s in s_lo..=s_hi {
            // Levels without shafts leave no openings in the cave walls, so
            // the shaft wall colliders (which would sit sealed inside solid
            // rock) are skipped entirely — the map just stays empty.
            if !level.shafts {
                break;
            }
            for gap in [ship_layer - 1, ship_layer] {
                let Entry::Vacant(e) = self.shafts.entry((s, gap)) else { continue };
                let walls = [level.shaft_wall_pts(s, gap, 0), level.shaft_wall_pts(s, gap, 1)];
                let mut handles = Vec::new();
                for pts in &walls {
                    for w in pts.windows(2) {
                        handles.push(colliders.insert(
                            ColliderBuilder::segment(
                                point![w[0].x, w[0].y],
                                point![w[1].x, w[1].y],
                            )
                            .friction(0.0)
                            .build(),
                        ));
                    }
                }
                e.insert(Shaft { handles, walls });
            }
        }

        // Obstacles (slot window mirrors the wall window; ±3 m jitter pad).
        let win_left_x = want_left as f32 * SEG_LEN;
        let win_right_x = (want_right + 1) as f32 * SEG_LEN;
        let k_left = ((win_left_x - 3.0) / OBSTACLE_SPACING).floor() as i64;
        let k_right = ((win_right_x + 3.0) / OBSTACLE_SPACING).ceil() as i64;
        self.obstacles.retain(|&(k, layer), ob| {
            if k < k_left || k > k_right || layer < lay_lo || layer > lay_hi {
                colliders.remove(ob.handle, island_manager, bodies, false);
                false
            } else {
                true
            }
        });
        for layer in lay_lo..=lay_hi {
            for k in k_left..=k_right {
                let Entry::Vacant(e) = self.obstacles.entry((k, layer)) else { continue };
                let Some(spec) = level.obstacle_spec(k) else { continue };
                let Some(builder) = ColliderBuilder::convex_hull(&spec.pts) else { continue };
                let cy = spec.cy + layer as f32 * V_PERIOD;
                let handle = colliders.insert(
                    builder
                        .translation(vector![spec.cx, cy])
                        .rotation(spec.rot)
                        .friction(0.6)
                        .restitution(0.2)
                        .build(),
                );
                // Read the hull back so rendering matches the collider.
                let verts = colliders[handle]
                    .shape()
                    .as_convex_polygon()
                    .map(|cp| cp.points().iter().map(|p| Vec2::new(p.x, p.y)).collect())
                    .unwrap_or_else(|| spec.pts.iter().map(|p| Vec2::new(p.x, p.y)).collect());
                e.insert(Obstacle { handle, cx: spec.cx, cy, rot: spec.rot, verts });
            }
        }

        // Landing pads (±20 m position jitter).
        let p_left = ((win_left_x - 20.0) / level.pad_spacing).floor() as i64;
        let p_right = ((win_right_x + 20.0) / level.pad_spacing).ceil() as i64;
        self.pads.retain(|&(p, layer), pad| {
            if p < p_left || p > p_right || layer < lay_lo || layer > lay_hi {
                colliders.remove(pad.handle, island_manager, bodies, false);
                false
            } else {
                true
            }
        });
        for layer in lay_lo..=lay_hi {
            for p in p_left..=p_right {
                let Entry::Vacant(e) = self.pads.entry((p, layer)) else { continue };
                let Some(spec) = level.pad_spec(p) else { continue };
                let y = spec.y + layer as f32 * V_PERIOD;
                // High friction, no restitution: settle, don't skate.
                let handle = colliders.insert(
                    ColliderBuilder::segment(
                        point![spec.cx - PAD_HALF_W, y],
                        point![spec.cx + PAD_HALF_W, y],
                    )
                    .friction(0.9)
                    .build(),
                );
                e.insert(Pad { handle, cx: spec.cx, y });
            }
        }

        self.synced_at = Some(key);
    }
}

// The batch form of what main's ResimPlayer does incrementally for playback.
// Not called by the game loop — it's the verification entry point for when
// blobs leave the device (a server re-running a submitted run), and the
// anchor of the determinism tests below.

#[allow(dead_code)]
// Re-run a hybrid Recording through a fresh Sim: restore its first keyframe,
// feed the input events tick by tick, and emit keyframes on the same cadence
// the recorder used. With an unchanged binary and params this reproduces the
// recorded keyframes bit-for-bit (glow excepted — it's a render-side
// smoothing; resim substitutes the commanded throttle).
pub fn resim(rec: &Recording) -> Vec<Keyframe> {
    let mut sim = Sim::new(Level::from_params(&rec.level));
    let Some(&k0) = rec.keyframes.first() else { return Vec::new() };
    sim.restore(&k0);
    let mut out = vec![k0];
    let mut events = rec.events.iter().peekable();
    let mut input = InputState::default();
    for tick in k0.tick..rec.ticks() {
        while events.peek().is_some_and(|e| e.tick <= tick) {
            input = events.next().unwrap().input;
        }
        let rep = sim.tick(input);
        let done = tick + 1;
        if let Some(imp) = rep.impact.filter(|i| i.destroyed) {
            out.push(Keyframe {
                tick: done,
                x: imp.x, y: imp.y, rot_re: imp.rot_re, rot_im: imp.rot_im,
                vx: imp.vx, vy: imp.vy, angvel: imp.angvel,
                fuel: sim.fuel, hull: sim.hull,
                glow: input.throttle_f32(),
                land_timer: 0.0, // a destroying tick always zeroes it
            });
            break;
        }
        if done.is_multiple_of(crate::replay::KEYFRAME_EVERY) {
            out.push(sim.keyframe(done, input.throttle_f32()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replay::{Recording, KEYFRAME_EVERY};

    // A varied flight: full burn up, coast, catch, steer with the stick,
    // burn while rotating, then coast until (probably) meeting the rock.
    // The test makes no assumption about whether it survives — whatever
    // happens must resim identically.
    fn script(tick: u32) -> InputState {
        match tick {
            0..=119 => InputState::from_controls(1.0, 0, 0.0, 0.0, false),
            120..=359 => InputState::default(),
            360..=479 => InputState::from_controls(1.0, 0, 0.0, 0.0, false),
            480..=719 => InputState::from_controls(0.4, 0, 0.7, -0.4, true),
            720..=899 => InputState::from_controls(0.8, -1, 0.0, 0.0, false),
            _ => InputState::default(),
        }
    }

    fn record_scripted_flight(level: Level, ticks: u32) -> (Recording, Vec<Keyframe>) {
        let mut sim = Sim::new(level.clone());
        let mut rec = Recording::new(sim_params(), level.to_params(), u32::MAX);
        rec.push_keyframe(sim.keyframe(0, 0.0));
        for t in 0..ticks {
            let input = script(t);
            let rep = sim.tick(input);
            let due = rec.record_tick(input);
            if let Some(imp) = rep.impact.filter(|i| i.destroyed) {
                rec.finalize(Keyframe {
                    tick: rec.ticks(),
                    x: imp.x, y: imp.y, rot_re: imp.rot_re, rot_im: imp.rot_im,
                    vx: imp.vx, vy: imp.vy, angvel: imp.angvel,
                    fuel: sim.fuel, hull: sim.hull,
                    glow: input.throttle_f32(),
                    land_timer: 0.0,
                });
                break;
            }
            if due {
                rec.push_keyframe(sim.keyframe(rec.ticks(), input.throttle_f32()));
            }
        }
        let kfs = rec.keyframes.clone();
        (rec, kfs)
    }

    #[test]
    fn out_of_fuel_at_rest_ends_the_run_but_a_pad_refuels() {
        // Stranded dry away from any pad (RESET_X is the guaranteed
        // obstacle-free stand spot): fuel_out must fire, exactly at the
        // FUEL_OUT_END_SECS deadline (not sooner).
        let level = Level::demo();
        let mut sim = Sim::new(level.clone());
        sim.restore(&spawn_keyframe(&level, crate::world::RESET_X));
        sim.fuel = 0.0;
        let mut fired_at = None;
        for t in 0..(6.0 / PHYSICS_DT) as u32 {
            if sim.tick(InputState::default()).fuel_out {
                fired_at = Some(t);
                break;
            }
        }
        let fired_at = fired_at.expect("a dry ship must end the run");
        assert!(
            fired_at as f32 * PHYSICS_DT >= FUEL_OUT_END_SECS - 0.1,
            "fired after {} ticks — before the deadline",
            fired_at
        );

        // Parked dry ON a pad (the spawn stands on pad 0): the refuel wins
        // — fuel_out must never fire.
        let mut sim = Sim::new(level);
        sim.fuel = 0.0;
        for _ in 0..(6.0 / PHYSICS_DT) as u32 {
            assert!(!sim.tick(InputState::default()).fuel_out,
                "a pad-parked ship must refuel, not game-over");
        }
        assert!(sim.fuel > 0.0, "the pad must have refueled the parked ship");
    }

    fn assert_physics_eq(a: &Keyframe, b: &Keyframe) {
        // Bit-exact on every physics field; glow is render-side and excluded.
        assert_eq!(a.tick, b.tick);
        assert_eq!(a.x.to_bits(), b.x.to_bits(), "x differs at tick {}", a.tick);
        assert_eq!(a.y.to_bits(), b.y.to_bits(), "y differs at tick {}", a.tick);
        assert_eq!(a.rot_re.to_bits(), b.rot_re.to_bits(), "rot_re differs at tick {}", a.tick);
        assert_eq!(a.rot_im.to_bits(), b.rot_im.to_bits(), "rot_im differs at tick {}", a.tick);
        assert_eq!(a.vx.to_bits(), b.vx.to_bits(), "vx differs at tick {}", a.tick);
        assert_eq!(a.vy.to_bits(), b.vy.to_bits(), "vy differs at tick {}", a.tick);
        assert_eq!(a.angvel.to_bits(), b.angvel.to_bits(), "angvel differs at tick {}", a.tick);
        assert_eq!(a.fuel.to_bits(), b.fuel.to_bits(), "fuel differs at tick {}", a.tick);
        assert_eq!(a.hull.to_bits(), b.hull.to_bits(), "hull differs at tick {}", a.tick);
        assert_eq!(
            a.land_timer.to_bits(), b.land_timer.to_bits(),
            "land_timer differs at tick {}", a.tick
        );
    }

    // Regression test for the replay-drift bug (2026-07). Rapier's contact
    // solve depends on collider HANDLE NUMBERING: a sim reused across runs
    // (reset instead of recreated) carries the previous run's handle space,
    // while resim always runs on a fresh sim — and under sustained pad
    // contact the differing float summation order diverged (reproduced:
    // max 1e-4 m creep, first at kf tick 840, amplified to metres by chaos
    // at later collisions). The game therefore creates a FRESH Sim per run;
    // this test mimics exactly that (prior run on a separate sim, recording
    // on a fresh one, sustained pad contact) and must stay bit-exact.
    #[test]
    fn fresh_sim_per_run_with_pad_contact_resims_exactly() {
        // Previous run happens on its own sim (dropped, like the fixed game).
        let mut prior = Sim::new(Level::demo());
        for _ in 0..1500 {
            prior.tick(InputState::from_controls(1.0, 1, 0.0, 0.0, false));
            if prior.crashed { break; }
        }
        drop(prior);
        let mut sim = Sim::new(Level::demo());
        // Recorded run: sit parked on pad 0 (multi-contact), hop, land, sit.
        let script = |t: u32| match t {
            0..=239 => InputState::default(),                                  // parked
            240..=299 => InputState::from_controls(0.5, 0, 0.0, 0.0, false),   // hop
            _ => InputState::default(),                                        // fall + land + sit
        };
        let mut rec = Recording::new(sim_params(), Level::demo().to_params(), u32::MAX);
        rec.push_keyframe(sim.keyframe(0, 0.0));
        for t in 0..(8 * KEYFRAME_EVERY) {
            let input = script(t);
            let rep = sim.tick(input);
            let due = rec.record_tick(input);
            if let Some(imp) = rep.impact.filter(|i| i.destroyed) {
                rec.finalize(Keyframe {
                    tick: rec.ticks(), x: imp.x, y: imp.y,
                    rot_re: imp.rot_re, rot_im: imp.rot_im,
                    vx: imp.vx, vy: imp.vy, angvel: imp.angvel,
                    fuel: sim.fuel, hull: sim.hull, glow: input.throttle_f32(),
                    land_timer: 0.0,
                });
                break;
            }
            if due {
                rec.push_keyframe(sim.keyframe(rec.ticks(), input.throttle_f32()));
            }
        }
        let live = rec.keyframes.clone();
        let resimmed = resim(&rec);
        let mut max_drift = 0.0f32;
        let mut first: Option<u32> = None;
        for (a, b) in live.iter().zip(&resimmed) {
            let d = ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt();
            if d > 1e-6 && first.is_none() { first = Some(a.tick); }
            if d > max_drift { max_drift = d; }
        }
        assert!(max_drift < 1e-6,
            "fresh-sim run vs resim diverges: max {max_drift} m, first at kf tick {first:?}");
    }

    #[test]
    fn resim_reproduces_a_scripted_flight_bit_exactly() {
        // 12 s of varied flight (or up to the crash). The recording's
        // keyframes and a fresh resim of its input events must agree on
        // every physics field, bit for bit — the determinism contract that
        // makes shared replays and verified ghosts possible.
        let (rec, live_kfs) = record_scripted_flight(Level::demo(), 12 * KEYFRAME_EVERY);
        assert!(live_kfs.len() >= 3, "flight too short to be a meaningful test");
        let resimmed = resim(&rec);
        assert_eq!(resimmed.len(), live_kfs.len());
        for (a, b) in live_kfs.iter().zip(&resimmed) {
            assert_physics_eq(a, b);
        }
    }

    #[test]
    fn resim_reproduces_on_a_custom_level_bit_exactly() {
        // The determinism contract must hold on NON-demo levels too: the
        // level params ride in the recording header, and resim rebuilds the
        // identical world from them (different seed, no shafts, tighter
        // pads) before replaying the inputs.
        let level = Level::parse(
            "name = T\nscoring = distance\nshafts = off\nobstacles = on\npad_spacing = 90\nseed = 3",
        );
        let (rec, live_kfs) = record_scripted_flight(level, 12 * KEYFRAME_EVERY);
        assert!(live_kfs.len() >= 3, "flight too short to be a meaningful test");
        let resimmed = resim(&rec);
        assert_eq!(resimmed.len(), live_kfs.len());
        for (a, b) in live_kfs.iter().zip(&resimmed) {
            assert_physics_eq(a, b);
        }
    }

    #[test]
    fn spawn_has_ground_under_the_ship() {
        // The window syncs inside restore/tick, so even tick 0 collides:
        // an idle ship must still be standing (not fallen through) after 2 s.
        let mut sim = Sim::new(Level::demo());
        for _ in 0..240 {
            sim.tick(InputState::default());
        }
        let (_, y, _) = sim.ship_pose();
        let (_, vy) = sim.ship_vel();
        assert!((y - Level::demo().stand_y(0.0)).abs() < 0.5, "ship sank or bounced: y={y}");
        assert!(vy.abs() < 0.2, "ship still moving vertically: vy={vy}");
        assert!(!sim.crashed);
    }

    #[test]
    fn empty_tank_kills_thrust_and_rcs() {
        // Mid-air with a whisker of fuel (NOT on the spawn pad — parked
        // ships refuel, which is exactly what this test must not trigger).
        let lvl = Level::demo();
        let mut sim = Sim::new(lvl.clone());
        let mut kf = spawn_keyframe(&lvl, 30.0);
        kf.y = lvl.cave_center(30.0);
        kf.fuel = 0.05;
        sim.restore(&kf);
        let burn = InputState::from_controls(1.0, 1, 0.0, 0.0, false);
        for _ in 0..120 {
            sim.tick(burn);
        }
        assert_eq!(sim.fuel, 0.0);
        // With the tank dry the engine is dead: the ship is falling.
        let (_, vy) = sim.ship_vel();
        assert!(vy < 0.0, "ship not falling on an empty tank: vy={vy}");
    }

    #[test]
    fn parked_on_spawn_pad_scores_and_refuels() {
        // stand_y(0) parks the ship on pad 0; sitting still past
        // PAD_LAND_TIME must register the visit and start refueling.
        let mut sim = Sim::new(Level::demo());
        sim.fuel = 50.0;
        let mut scored = false;
        let mut landed = false;
        for _ in 0..(2.0 / PHYSICS_DT) as u32 {
            let rep = sim.tick(InputState::default());
            scored |= rep.scored;
            landed |= rep.landed;
        }
        assert!(scored, "first visit never scored");
        assert!(landed, "never registered as landed");
        assert_eq!(sim.score, PAD_POINTS);
        assert!(sim.fuel > 50.0, "no refuel happened");
    }
}
