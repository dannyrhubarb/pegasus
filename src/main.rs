use macroquad::audio::{load_sound_from_bytes, play_sound, set_sound_volume, PlaySoundParams};
use macroquad::prelude::*;
use macroquad::rand::gen_range;
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};

mod audio;
mod render;
mod ship_mesh;

use audio::*;
use pegasus_sim::replay::{self, *};
use pegasus_sim::sim::{self, *};
use pegasus_sim::world::{self, *};
use render::*;
use ship_mesh::*;

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

// Replay playback driver: RE-SIMULATES the hybrid recording's input events
// through a scratch Sim, paced by the render clock — the exact machinery a
// replay shared from another device would use. Every keyframe the cursor
// passes is verified against the re-simmed state; drift beyond SNAP_DRIFT_M
// snaps to the keyframe (the graceful fallback for recordings from a
// different build/params — on this binary the test suite proves drift is 0).
struct ResimPlayer {
    sim: sim::Sim,
    first_tick: u32,
    end_tick: u32,
    tick: u32,       // ticks simulated so far (recording clock)
    event_idx: usize,
    kf_idx: usize,
    input: InputState,
    prev_pose: (f32, f32, f32),
    accum: f32,
    glow: f32,
    last_torque: f32,
    drift: f32,      // metres to the last verified keyframe
    snapped: bool,   // keyframe fallback engaged at least once
    finished: bool,
}

const SNAP_DRIFT_M: f32 = 0.5;

impl ResimPlayer {
    fn new(rec: &Recording) -> Option<ResimPlayer> {
        let &k0 = rec.keyframes.first()?;
        if rec.ticks() <= k0.tick {
            return None;
        }
        let mut s = sim::Sim::new(world::Level::from_params(&rec.level));
        s.restore(&k0);
        let prev_pose = s.ship_pose();
        Some(ResimPlayer {
            sim: s,
            first_tick: k0.tick,
            end_tick: rec.ticks(),
            tick: k0.tick,
            event_idx: 0,
            kf_idx: 0,
            input: InputState::default(),
            prev_pose,
            accum: 0.0,
            glow: k0.glow,
            last_torque: 0.0,
            drift: 0.0,
            snapped: false,
            finished: false,
        })
    }

    // Re-simulate exactly one tick: apply due input events, tick the scratch
    // sim, verify any keyframe passed (snap on real drift). Called from
    // advance() on the wall clock for WATCH REPLAY, and once per LIVE tick
    // for the lockstep ghost.
    fn step_one(&mut self, rec: &Recording) {
        if self.finished {
            return;
        }
        self.prev_pose = self.sim.ship_pose();
        while rec.events.get(self.event_idx).is_some_and(|e| e.tick <= self.tick) {
            self.input = rec.events[self.event_idx].input;
            self.event_idx += 1;
        }
        let rep = self.sim.tick(self.input);
        self.tick += 1;
        self.last_torque = rep.heading_torque;
        // Keyframe verification + fallback.
        while rec.keyframes.get(self.kf_idx).is_some_and(|k| k.tick <= self.tick) {
            let kf = rec.keyframes[self.kf_idx];
            if kf.tick == self.tick {
                let (x, y, _) = self.sim.ship_pose();
                self.drift = ((x - kf.x).powi(2) + (y - kf.y).powi(2)).sqrt();
                if self.drift > SNAP_DRIFT_M {
                    self.sim.restore(&kf);
                    self.snapped = true;
                }
            }
            self.kf_idx += 1;
        }
        if self.tick >= self.end_tick {
            self.finished = true;
        }
    }

    // Pose lerped between the last two re-simmed ticks. The lockstep ghost
    // passes the live interpolation alpha, so it moves in perfect sync with
    // the player ship.
    fn lerped_pose(&self, alpha: f32) -> (f32, f32, f32) {
        let (px, py, pa) = self.prev_pose;
        let (x, y, a) = self.sim.ship_pose();
        (px + (x - px) * alpha, py + (y - py) * alpha, lerp_angle(pa, a, alpha))
    }

    // Advance by real time `dt`, re-simulating whole ticks and returning the
    // interpolated visual frame (the final pose once finished). The cap
    // bounds catch-up after a hitch; it sits ABOVE the biggest deliberate
    // fast-forward step (raw frame time is pre-clamped to 0.05 s by the
    // caller before the speed multiplier, so 4× peaks at 0.2 s = 24 ticks).
    fn advance(&mut self, rec: &Recording, dt: f32) -> ReplayFrame {
        self.accum = (self.accum + dt).min(0.25);
        while self.accum >= PHYSICS_DT && !self.finished {
            self.step_one(rec);
            self.accum -= PHYSICS_DT;
        }
        // Engine glow follows the fuel-gated command, same smoothing as live.
        let throttle_fx = if self.sim.fuel > 0.0 { self.input.throttle_f32() } else { 0.0 };
        self.glow += (throttle_fx - self.glow) * 0.12;
        let rcs_live = self.sim.fuel > 0.0 && !self.sim.crashed;
        let rcs = if rcs_live && (self.input.rot < 0 || self.last_torque < -0.4) {
            -1
        } else if rcs_live && (self.input.rot > 0 || self.last_torque > 0.4) {
            1
        } else {
            0
        };
        let alpha = (self.accum / PHYSICS_DT).clamp(0.0, 1.0);
        let (px, py, pa) = self.prev_pose;
        let (x, y, a) = self.sim.ship_pose();
        let (vx, vy) = self.sim.ship_vel();
        ReplayFrame {
            x: px + (x - px) * alpha,
            y: py + (y - py) * alpha,
            angle: lerp_angle(pa, a, alpha),
            vx, vy,
            glow: self.glow,
            rcs,
        }
    }

    // Scrub the playback to keyframe `kf_idx` (clamped to the last one with
    // ticks still left to play — the terminal crash keyframe itself is not a
    // playable start). The scratch sim is rebuilt FRESH and restored from the
    // keyframe — the same operation sequence as playing a trimmed recording
    // from its first keyframe. The effective input at the keyframe is
    // re-seeded from the event stream, exactly like Recording::trim does at
    // a cut. Since format v3, keyframes carry the body's exact unit-complex
    // rotation and the land timer, so restoring an airborne keyframe resumes
    // BIT-EXACTLY (unit-tested). A keyframe captured under sustained contact
    // can still diverge (Rapier's warm-start caches and handle numbering
    // aren't in the keyframe — see the determinism rules); that's what the
    // per-keyframe drift check + 0.5 m snap fallback absorbs.
    fn seek_to_keyframe(&mut self, rec: &Recording, kf_idx: usize) {
        let mut idx = kf_idx.min(rec.keyframes.len().saturating_sub(1));
        while idx > 0 && rec.keyframes[idx].tick >= self.end_tick {
            idx -= 1;
        }
        let Some(&kf) = rec.keyframes.get(idx) else { return };
        if kf.tick >= self.end_tick {
            return; // nothing playable at or after this keyframe
        }
        let mut s = sim::Sim::new(world::Level::from_params(&rec.level));
        s.restore(&kf);
        self.prev_pose = s.ship_pose();
        self.sim = s;
        self.tick = kf.tick;
        self.event_idx = rec.events.partition_point(|e| e.tick <= kf.tick);
        self.input = self
            .event_idx
            .checked_sub(1)
            .map_or(InputState::default(), |i| rec.events[i].input);
        self.kf_idx = idx + 1;
        self.accum = 0.0;
        self.glow = kf.glow;
        self.last_torque = 0.0;
        self.drift = 0.0;
        self.snapped = false;
        self.finished = false;
    }

    // Scrub to an EXACT tick (frame-level transport). Physics can't run
    // backwards, so a backward (or cross-keyframe forward) target restores
    // the nearest keyframe at or before it and re-sims the remainder —
    // bounded by the keyframe spacing (< KEYFRAME_EVERY ticks ≈ a few ms).
    // A forward target within the current keyframe interval just steps from
    // where we are: that IS ordinary playback, no rebuild. Stepping onto
    // end_tick re-simulates the finale (finished = replay ends normally).
    fn seek_to_tick(&mut self, rec: &Recording, target: u32) {
        let target = target.clamp(self.first_tick, self.end_tick);
        if target == self.tick {
            return; // includes stepping "past" the final tick: a no-op
        }
        let kf_idx = rec
            .keyframes
            .partition_point(|k| k.tick <= target)
            .saturating_sub(1);
        if self.finished || target < self.tick || self.tick < rec.keyframes[kf_idx].tick {
            self.seek_to_keyframe(rec, kf_idx);
        }
        while self.tick < target && !self.finished {
            self.step_one(rec);
        }
        self.accum = 0.0;
    }

    fn progress(&self) -> f32 {
        (self.tick - self.first_tick) as f32 / (self.end_tick - self.first_tick).max(1) as f32
    }

    // The input currently driving the re-sim (for the replay's stick widget).
    fn current_input(&self) -> InputState {
        self.input
    }
}

// Re-derive the exhaust/RCS particle field for a replay tick: re-sim the
// trailing plume window on a scratch player and emit per-tick cosmetics
// along the way, so after a scrub or step (in EITHER direction) the plume is
// exactly what the ship "should" trail at that instant — instead of an empty
// frame (old backward behaviour) or a coarse-dt clump (old forward stepping
// integrated particles in 0.1 s lumps). Cosmetic randomness (gen_range) is
// fine here — nothing feeds back into the sim. Cost: one scratch seek plus
// ≤ PLUME_WINDOW_TICKS re-simmed ticks, a few ms, at most once per rendered
// frame during a drag.
const PLUME_WINDOW_TICKS: u32 = 60; // exhaust lives 0.5 s — the longest window needed
fn rebuild_replay_particles(rec: &Recording, target: u32, particles: &mut Vec<Particle>) {
    particles.clear();
    let Some(mut q) = ResimPlayer::new(rec) else { return };
    if target <= q.first_tick {
        return;
    }
    let start = target.saturating_sub(PLUME_WINDOW_TICKS).max(q.first_tick);
    q.seek_to_tick(rec, start);
    while q.tick < target && !q.finished {
        q.step_one(rec);
        // Integrate + decay what's already emitted by one tick (same rates
        // as the frame loop's particle pass: 0.5 s main, 0.3 s RCS).
        for pt in particles.iter_mut() {
            pt.life -= match pt.kind {
                0 => PHYSICS_DT / 0.5,
                _ => PHYSICS_DT / 0.3,
            };
            pt.x += pt.vx * PHYSICS_DT;
            pt.y += pt.vy * PHYSICS_DT;
        }
        let (x, y, a) = q.sim.ship_pose();
        let (svx, svy) = q.sim.ship_vel();
        let lp = |lx: f32, ly: f32| (x + lx * a.cos() - ly * a.sin(), y + lx * a.sin() + ly * a.cos());
        let ld = |lx: f32, ly: f32| (lx * a.cos() - ly * a.sin(), lx * a.sin() + ly * a.cos());
        let alive = q.sim.fuel > 0.0 && !q.sim.crashed;
        // Commanded throttle stands in for the render-side glow smoothing.
        let ex = if alive { q.input.throttle_f32() } else { 0.0 };
        if ex > 0.05 {
            // Live emission is ~8/frame at 60 fps → 4/tick at 120 Hz.
            for _ in 0..(4.0 * ex).ceil() as i32 {
                let spread = gen_range(-0.25f32, 0.25);
                let (px, py) = lp(spread * 0.45, -0.72);
                let speed = gen_range(4.0f32, 8.0) * (0.4 + 0.6 * ex);
                let (dvx, dvy) = ld(spread * 1.5, -speed);
                particles.push(Particle {
                    x: px, y: py, vx: svx + dvx, vy: svy + dvy, life: 1.0, kind: 0,
                });
            }
        }
        // Same nozzle mapping as the frame loop: negative torque = left.
        let rcs = if alive && (q.input.rot < 0 || q.last_torque < -0.4) {
            -1
        } else if alive && (q.input.rot > 0 || q.last_torque > 0.4) {
            1
        } else {
            0
        };
        if rcs != 0 {
            for _ in 0..2 {
                // Live is 3/frame at 60 fps → ~2/tick.
                let spread = gen_range(-0.15f32, 0.15);
                let (px, py) = lp(0.30 * rcs as f32, -0.71);
                let speed = gen_range(2.0f32, 4.0);
                let (dvx, dvy) = ld(spread, -speed);
                particles.push(Particle {
                    x: px, y: py, vx: svx + dvx, vy: svy + dvy, life: 1.0,
                    kind: if rcs < 0 { 1 } else { 2 },
                });
            }
        }
    }
    particles.retain(|pt| pt.life > 0.0);
}

// The attitude stick is now read directly from macroquad's touch API inside
// the game (see the TouchStick gatherer in the loop) — no touch atoms/exports.
// Gamepad state lives on its own atomics so a connected-but-idle controller
// never stomps touch input; the two sources are combined in the main loop.
static PAD_THRUST: AtomicU32 = AtomicU32::new(0);
static PAD_TORQUE: AtomicU32 = AtomicU32::new(0);
static PAD_RESET: AtomicU32 = AtomicU32::new(0);
static SAFE_AREA_TOP: AtomicU32 = AtomicU32::new(0);
static SAFE_AREA_LEFT: AtomicU32 = AtomicU32::new(0);
// Bottom/right insets (CSS px). Bottom folds in the floating browser
// toolbar (see index.html); used to keep the parked stick tappable.
static SAFE_AREA_BOTTOM: AtomicU32 = AtomicU32::new(0);
static SAFE_AREA_RIGHT: AtomicU32 = AtomicU32::new(0);
// Velocity-vector arrow (off by default) — toggled from the info overlay's
// checkbox, persisted in localStorage on the web side.
static SHOW_VEL: AtomicU32 = AtomicU32::new(0);
// "Invert stick": negate the commanded nose direction (push down = nose up,
// like pulling back on a flight stick). Set from the info-overlay checkbox,
// persisted in localStorage; the knob visual still follows the finger.
static INVERT_STICK: AtomicU32 = AtomicU32::new(0);

#[unsafe(no_mangle)]
pub extern "C" fn set_invert_stick(on: i32) {
    INVERT_STICK.store(on as u32, Ordering::Relaxed);
}

// Runtime level loading (levels are DATA, not code — levels/*.level files
// fetched by index.html): JS asks for a buffer with level_buf_ptr(len),
// writes the UTF-8 level text into wasm memory, then calls load_level(len).
// The parsed level is applied by the main loop at the next frame boundary
// as a full fresh-Sim restart (a level switch is a new run by definition).
static LEVEL_BUF: std::sync::Mutex<Vec<u8>> = std::sync::Mutex::new(Vec::new());
static PENDING_LEVEL: std::sync::Mutex<Option<world::Level>> = std::sync::Mutex::new(None);

#[unsafe(no_mangle)]
pub extern "C" fn level_buf_ptr(len: u32) -> *const u8 {
    let mut b = LEVEL_BUF.lock().unwrap();
    b.clear();
    b.resize(len as usize, 0);
    b.as_ptr()
}

#[unsafe(no_mangle)]
pub extern "C" fn load_level(len: u32) {
    let b = LEVEL_BUF.lock().unwrap();
    let end = (len as usize).min(b.len());
    if let Ok(text) = std::str::from_utf8(&b[..end]) {
        *PENDING_LEVEL.lock().unwrap() = Some(world::Level::parse(text));
        BEST_DIST.store(0, Ordering::Relaxed);
        BEST_NAME.lock().unwrap().clear();
        GHOST_NAME.lock().unwrap().clear();
    }
}

// Best distance on the current level (the Distance-scoring high score).
// The game raises it live as the run passes it; JS seeds it with the
// level's global all-time record after each level (re)load — load_level
// clears it so records never leak across levels.
static BEST_DIST: AtomicU32 = AtomicU32::new(0);

#[unsafe(no_mangle)]
pub extern "C" fn set_best_dist(v: f32) {
    BEST_DIST.store(v.max(0.0).to_bits(), Ordering::Relaxed);
}

#[unsafe(no_mangle)]
pub extern "C" fn get_best_dist() -> f32 {
    f32::from_bits(BEST_DIST.load(Ordering::Relaxed))
}

// The record holder shown under the HUD BEST line ("by <pilot>"). Pushed by
// JS alongside set_best_dist (same blob_in_ptr buffer-write pattern as the
// replay blobs, UTF-8 text instead of a blob); becomes "you" the moment the
// live run beats the seeded record; cleared with BEST_DIST on level load.
// Empty = no line drawn (offline, empty board, pads scoring).
static BEST_NAME: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

#[unsafe(no_mangle)]
pub extern "C" fn set_best_name(len: u32) {
    let b = BLOB_IN.lock().unwrap();
    let end = (len as usize).min(b.len());
    *BEST_NAME.lock().unwrap() = String::from_utf8_lossy(&b[..end]).into_owned();
}

// The ghost pilot's callsign, floated under the racing silhouette. Pushed by
// JS right after a successful load_ghost_blob — it can differ from BEST_NAME
// (the ghost is the best run WITH a replay, which may not be the #1 score,
// and BEST_NAME flips to "you" when the record falls while the ghost keeps
// its pilot). Cleared on level load with the ghost itself.
static GHOST_NAME: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

#[unsafe(no_mangle)]
pub extern "C" fn set_ghost_name(len: u32) {
    let b = BLOB_IN.lock().unwrap();
    let end = (len as usize).min(b.len());
    *GHOST_NAME.lock().unwrap() = String::from_utf8_lossy(&b[..end]).into_owned();
}

// --- Highscores & best-run ghost (the global board is the store) ---
// When a run ends (destroying crash, or a manual reset while alive), the
// game publishes it here: deflated recording blob + final |x| distance + a
// bumped sequence number. JS polls run_seq(), pulls the blob on change, and
// offers it to the online submit flow (the pegasus-backend global board) —
// the sole consumer since the local localStorage store was removed. The
// board pushes back: the global record raises BEST_DIST (set_best_dist)
// and its replay becomes the racing ghost (load_ghost_blob).
static RUN_BLOB: std::sync::Mutex<Vec<u8>> = std::sync::Mutex::new(Vec::new());
static RUN_DIST: AtomicU32 = AtomicU32::new(0);
static RUN_SEQ: AtomicU32 = AtomicU32::new(0);

#[unsafe(no_mangle)]
pub extern "C" fn run_seq() -> u32 {
    RUN_SEQ.load(Ordering::Relaxed)
}
#[unsafe(no_mangle)]
pub extern "C" fn run_dist() -> f32 {
    f32::from_bits(RUN_DIST.load(Ordering::Relaxed))
}
#[unsafe(no_mangle)]
pub extern "C" fn run_blob_len() -> i32 {
    RUN_BLOB.lock().unwrap().len() as i32
}
#[unsafe(no_mangle)]
pub extern "C" fn run_blob_ptr() -> *const u8 {
    RUN_BLOB.lock().unwrap().as_ptr()
}

// Publish an ended run for the JS submit flow. Blink-and-gone attempts
// (< GHOST_MIN_SECS) aren't worth a leaderboard slot.
fn report_run_end(rec: &Recording, dist: f32) {
    if rec.ticks() < (GHOST_MIN_SECS / PHYSICS_DT) as u32 {
        return;
    }
    let packed = compress(&rec.serialize(BUILD_ID.load(Ordering::Relaxed)));
    *RUN_BLOB.lock().unwrap() = packed;
    RUN_DIST.store(dist.to_bits(), Ordering::Relaxed);
    RUN_SEQ.fetch_add(1, Ordering::Relaxed);
}

// --- Analytics run channel (JS polls it alongside run_seq) ---
// Separate from the highscore publish above: analytics wants EVERY ended
// run — short runs are difficulty signal — so there is no GHOST_MIN_SECS
// gate here, and the highscore path stays untouched. Counters, not flags,
// so a run that starts and ends within one JS poll window still counts.
static RUN_START_SEQ: AtomicU32 = AtomicU32::new(0);
static RUN_END_SEQ: AtomicU32 = AtomicU32::new(0);
static RUN_END_CAUSE: AtomicU32 = AtomicU32::new(0); // 0 crash / 1 reset / 2 fuel
static RUN_END_TICKS: AtomicU32 = AtomicU32::new(0);
static RUN_END_DIST: AtomicU32 = AtomicU32::new(0); // f32 bits
static RUN_END_FUEL: AtomicU32 = AtomicU32::new(0); // f32 bits
static RUN_END_HULL: AtomicU32 = AtomicU32::new(0); // f32 bits

#[unsafe(no_mangle)]
pub extern "C" fn run_start_seq() -> u32 {
    RUN_START_SEQ.load(Ordering::Relaxed)
}
#[unsafe(no_mangle)]
pub extern "C" fn run_end_seq() -> u32 {
    RUN_END_SEQ.load(Ordering::Relaxed)
}
#[unsafe(no_mangle)]
pub extern "C" fn run_end_cause() -> i32 {
    RUN_END_CAUSE.load(Ordering::Relaxed) as i32
}
#[unsafe(no_mangle)]
pub extern "C" fn run_end_ticks() -> u32 {
    RUN_END_TICKS.load(Ordering::Relaxed)
}
#[unsafe(no_mangle)]
pub extern "C" fn run_end_dist() -> f32 {
    f32::from_bits(RUN_END_DIST.load(Ordering::Relaxed))
}
#[unsafe(no_mangle)]
pub extern "C" fn run_end_fuel() -> f32 {
    f32::from_bits(RUN_END_FUEL.load(Ordering::Relaxed))
}
#[unsafe(no_mangle)]
pub extern "C" fn run_end_hull() -> f32 {
    f32::from_bits(RUN_END_HULL.load(Ordering::Relaxed))
}

/// Publish an ended run for analytics. Payload stored first, seq bumped
/// last, so the payload is complete when JS sees the new seq.
fn report_run_analytics(cause: u32, ticks: u32, dist: f32, fuel: f32, hull: f32) {
    if ticks == 0 {
        return; // a reset from the armed-idle state ended nothing
    }
    RUN_END_CAUSE.store(cause, Ordering::Relaxed);
    RUN_END_TICKS.store(ticks, Ordering::Relaxed);
    RUN_END_DIST.store(dist.to_bits(), Ordering::Relaxed);
    RUN_END_FUEL.store(fuel.to_bits(), Ordering::Relaxed);
    RUN_END_HULL.store(hull.to_bits(), Ordering::Relaxed);
    RUN_END_SEQ.fetch_add(1, Ordering::Relaxed);
}

// Blobs coming BACK from the global board: watch a fetched replay, or race
// the record run as the ghost. Same buffer-write pattern as load_level: JS
// asks for a buffer (blob_in_ptr), writes the deflated blob, then calls the
// consumer; the main loop picks the decoded Recording up at the next frame
// boundary. Both consumers return 1 on a successful decode, 0 otherwise (a
// corrupt download must not panic the game).
static BLOB_IN: std::sync::Mutex<Vec<u8>> = std::sync::Mutex::new(Vec::new());
static PENDING_WATCH: std::sync::Mutex<Option<Recording>> = std::sync::Mutex::new(None);
static PENDING_GHOST: std::sync::Mutex<Option<Recording>> = std::sync::Mutex::new(None);

#[unsafe(no_mangle)]
pub extern "C" fn blob_in_ptr(len: u32) -> *const u8 {
    let mut b = BLOB_IN.lock().unwrap();
    b.clear();
    b.resize(len as usize, 0);
    b.as_ptr()
}

// Deflated bytes -> Recording (the watch_replay_blob / load_ghost_blob
// decode path). Pure so the round trip is unit-testable.
fn decode_recording(packed: &[u8]) -> Option<Recording> {
    let raw = replay::decompress(packed)?;
    Recording::deserialize(&raw).ok().map(|(rec, _build_id)| rec)
}

fn decode_blob_in(len: u32) -> Option<Recording> {
    let b = BLOB_IN.lock().unwrap();
    let end = (len as usize).min(b.len());
    decode_recording(&b[..end])
}

#[unsafe(no_mangle)]
pub extern "C" fn watch_replay_blob(len: u32) -> i32 {
    match decode_blob_in(len) {
        Some(rec) => {
            *PENDING_WATCH.lock().unwrap() = Some(rec);
            1
        }
        None => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn load_ghost_blob(len: u32) -> i32 {
    match decode_blob_in(len) {
        Some(rec) => {
            *PENDING_GHOST.lock().unwrap() = Some(rec);
            1
        }
        None => 0,
    }
}

// --- HTML game-menu bridge (index.html owns the menu/pause/game-over UI) ---
// The web wrapper drives the game with set_ui_pause + ui_command and observes
// it through ui_state / cur_dist. UI_STATE and CUR_DIST are per-frame mirrors
// written by the main loop — exports can't read loop locals.
static UI_PAUSE: AtomicU32 = AtomicU32::new(0);
static UI_CMD: AtomicU32 = AtomicU32::new(0);
static UI_STATE: AtomicU32 = AtomicU32::new(0);
static CUR_DIST: AtomicU32 = AtomicU32::new(0);

/// Freeze/unfreeze the live sim while an HTML overlay (menu, pause view) is
/// up. Replay playback is NOT gated — a stored replay watched from the menu
/// keeps playing behind no overlay.
#[unsafe(no_mangle)]
pub extern "C" fn set_ui_pause(on: i32) {
    UI_PAUSE.store(on as u32, Ordering::Relaxed);
}

/// One-shot commands from the HTML UI (swap-to-consume, like PAD_RESET):
/// 1 = reset / fly again, 2 = watch the last crashed run's replay,
/// 3 = exit the replay (playback freezes on its final frame and never
/// exits by itself — the ✕ button sends this).
#[unsafe(no_mangle)]
pub extern "C" fn ui_command(cmd: i32) {
    UI_CMD.store(cmd as u32, Ordering::Relaxed);
}

/// What the game is doing, for the JS overlay state machine:
/// 0 = flying, 1 = wreck (explosion pause), 2 = crash dialog, 3 = replay.
#[unsafe(no_mangle)]
pub extern "C" fn ui_state() -> i32 {
    UI_STATE.load(Ordering::Relaxed) as i32
}

// --- Replay transport (the HTML replay bar + in-canvas keys) ---
// Pause and keyframe-scrub controls for Replay mode. REPLAY_SEEK carries the
// requested bar position as f32 fraction bits (swap-to-consume; SEEK_NONE
// sentinel = no request — it's a NaN bit pattern no real fraction produces).
// REPLAY_POS/REPLAY_LEN are per-frame mirrors for the JS bar, like UI_STATE.
const SEEK_NONE: u32 = u32::MAX;
static REPLAY_PAUSED: AtomicU32 = AtomicU32::new(0);
static REPLAY_SEEK: AtomicU32 = AtomicU32::new(SEEK_NONE);
// Relative keyframe steps (the bar's ⏮/⏭ buttons). fetch_add so taps landing
// within one frame accumulate; the loop consumes with swap(0).
static REPLAY_STEP: AtomicI32 = AtomicI32::new(0);
// Playback speed multiplier (f32 bits, 1.0 = realtime). The bar's speed
// button cycles ¼×..4×; slow-mo and fast-forward just scale the wall-clock
// time fed to the re-sim — the tick sequence itself never changes.
static REPLAY_SPEED: AtomicU32 = AtomicU32::new(1.0f32.to_bits());
static REPLAY_POS: AtomicU32 = AtomicU32::new(0);
static REPLAY_LEN: AtomicU32 = AtomicU32::new(0);
// Whether the HTML replay GUI (bar + ✕) is currently shown. JS owns the
// YouTube-style auto-hide (fade while playing, tap to bring back) and
// mirrors it here so the recorded-input stick can swap between its
// half-size spot above the bar and its full-size parked home.
static REPLAY_UI_VISIBLE: AtomicU32 = AtomicU32::new(1);

const REPLAY_SPEEDS: [f32; 5] = [0.25, 0.5, 1.0, 2.0, 4.0];

/// Pause / resume replay playback (the bar's play/pause button). Cleared by
/// the game whenever a new replay starts.
#[unsafe(no_mangle)]
pub extern "C" fn set_replay_paused(on: i32) {
    REPLAY_PAUSED.store(on as u32, Ordering::Relaxed);
}

/// Current pause state, so the JS button tracks the in-canvas space-bar
/// toggle too.
#[unsafe(no_mangle)]
pub extern "C" fn replay_paused() -> i32 {
    REPLAY_PAUSED.load(Ordering::Relaxed) as i32
}

/// Scrub the replay to `frac` (0..1 of the recording); the game snaps it to
/// the nearest keyframe at or before that point and re-sims from there.
#[unsafe(no_mangle)]
pub extern "C" fn replay_seek(frac: f32) {
    REPLAY_SEEK.store(frac.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
}

/// Step the replay by physics ticks (the bar's ⏮/⏭ buttons send 0.1 s
/// worth — 12 ticks — per tap and hold-to-repeat JS-side; the in-canvas
/// arrow keys step the same 0.1 s). Stepping auto-pauses playback;
/// fetch_add so a burst of taps within one frame accumulates.
#[unsafe(no_mangle)]
pub extern "C" fn replay_step(delta: i32) {
    REPLAY_STEP.fetch_add(delta, Ordering::Relaxed);
}

/// Playback speed (the bar's speed button / the in-canvas S key). Clamped to
/// what the advance() catch-up cap can actually sustain (see the cap note in
/// ResimPlayer::advance); reset to 1× whenever a new replay starts.
#[unsafe(no_mangle)]
pub extern "C" fn set_replay_speed(speed: f32) {
    REPLAY_SPEED.store(speed.clamp(0.05, 5.0).to_bits(), Ordering::Relaxed);
}

/// The HTML replay GUI's visibility (JS auto-hides it YouTube-style while
/// playing untouched; a canvas tap brings it back). Hidden ⇒ the in-canvas
/// recorded-input stick returns to its full-size parked home.
#[unsafe(no_mangle)]
pub extern "C" fn set_replay_ui_visible(on: i32) {
    REPLAY_UI_VISIBLE.store(on as u32, Ordering::Relaxed);
}

/// Current speed, so the JS button label tracks the in-canvas S-key cycle.
#[unsafe(no_mangle)]
pub extern "C" fn replay_speed() -> f32 {
    f32::from_bits(REPLAY_SPEED.load(Ordering::Relaxed))
}

/// Playback position as a fraction 0..1 (bar knob) and total length in
/// seconds (time label). Valid while ui_state() == 3.
#[unsafe(no_mangle)]
pub extern "C" fn replay_pos() -> f32 {
    f32::from_bits(REPLAY_POS.load(Ordering::Relaxed))
}

#[unsafe(no_mangle)]
pub extern "C" fn replay_len() -> f32 {
    f32::from_bits(REPLAY_LEN.load(Ordering::Relaxed))
}

/// The current run's distance (farthest |x| reached), for the game-over
/// screen. Still valid during the crash dialog — the sim isn't recreated
/// until the respawn.
#[unsafe(no_mangle)]
pub extern "C" fn cur_dist() -> f32 {
    f32::from_bits(CUR_DIST.load(Ordering::Relaxed))
}

// "Race best ghost" toggle (settings checkbox, on by default): whether
// the top-highscore run re-simulates alongside live play.
static GHOST_ON: AtomicU32 = AtomicU32::new(1);

#[unsafe(no_mangle)]
pub extern "C" fn set_ghost_enabled(on: i32) {
    GHOST_ON.store(on as u32, Ordering::Relaxed);
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

// Sound (settings toggle, OFF by default): the engine rumble loop + the
// crash boom. Off keeps the thruster loop muted and skips boom playback.
static SOUND_ON: AtomicU32 = AtomicU32::new(0);

#[unsafe(no_mangle)]
pub extern "C" fn set_sound_enabled(on: i32) {
    SOUND_ON.store(on as u32, Ordering::Relaxed);
}

// "Debug HUD" (settings toggle, OFF by default): the developer telemetry
// TEXT line only — x / layer / cave-position / FPS / speed. The minimap and
// the fuel & hull gauges are always on; the default HUD is those plus the big
// distance (or score) readout.
static DEBUG_HUD: AtomicU32 = AtomicU32::new(0);

#[unsafe(no_mangle)]
pub extern "C" fn set_debug_hud(on: i32) {
    DEBUG_HUD.store(on as u32, Ordering::Relaxed);
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
pub extern "C" fn set_safe_area(top: f32, left: f32, bottom: f32, right: f32) {
    SAFE_AREA_TOP.store(top.to_bits(), Ordering::Relaxed);
    SAFE_AREA_LEFT.store(left.to_bits(), Ordering::Relaxed);
    SAFE_AREA_BOTTOM.store(bottom.to_bits(), Ordering::Relaxed);
    SAFE_AREA_RIGHT.store(right.to_bits(), Ordering::Relaxed);
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
// Render scale for the ship mesh relative to the raw SWF coordinates
const SHIP_SCALE: f32 = 1.5;
// All physics-shaping constants (PHYSICS_DT, thrust/RCS forces, crash
// thresholds, fuel/hull numbers, PD gains, window size) live in src/sim.rs —
// the deterministic simulation core — and are re-exported via `use sim::*`.
// Seconds from crash to the crash dialog (fly again / watch replay) — long
// enough for the explosion to play out with the camera held still.
const CRASH_DIALOG_DELAY: f32 = 1.5;
// The hybrid recording is the ONLY replay store (~1 MB/hour worst case) and
// a ghost needs the run from its spawn (t = 0 is the shared start line), so
// its window is a memory safety net, not an expected limit — only a
// parked-for-hours session ever hits it.
const HYBRID_MAX_SECS: f32 = 3600.0;
// A run shorter than this isn't published to the highscore channel —
// R-spam shouldn't reach the submit dialog.
const GHOST_MIN_SECS: f32 = 2.0;
// Stick-hold engine gating (one-handed scheme): a quick flick shorter than
// DELAY never lights the engine, thrust then ramps to full over RAMP, and a
// commanded flip past FLIP_GATE keeps the engine cold until the nose settles
// within FLIP_DONE of the target (steer first, burn once pointed).
const STICK_THRUST_DELAY: f32 = 0.12;
const STICK_THRUST_RAMP: f32 = 0.18;
const FLIP_GATE_RAD: f32 = 1.6;  // ~92°
const FLIP_DONE_RAD: f32 = 0.35; // ~20°

// --- In-canvas floating attitude stick (ported from index.html) ---
// All positions/sizes are in LOGICAL px — the space `screen_width()` and
// `mouse_position()` report (both = raw / dpi_scale). NOTE: macroquad's
// `touches()` returns RAW physical px (it does NOT divide by dpi like
// mouse_position does), so touch positions are divided by dpi in the gather
// before reaching this struct. While flying, a touch ANYWHERE spawns the
// stick under the finger; release parks it bottom-right. Deflection
// direction = commanded nose direction (screen convention, y down); STICK_DZ
// is a radial dead-zone rescaled so the rim still reaches 1. Holding the
// stick — even centred — lights the main engine through the flick/flip gating.
const STICK_TRAVEL: f32 = 60.0;  // logical px from centre = full deflection
const STICK_DZ: f32 = 0.15;      // radial dead-zone (rescaled)
const STICK_RADIUS: f32 = 85.0;  // logical px, ring radius (matches the old 170px element)
const STICK_KNOB_R: f32 = 32.0;  // logical px, knob radius

// The floating stick's live state, tracked across frames (all logical px).
// `id` = the claimed touch (None = parked).
struct TouchStick {
    id: Option<u64>,
    center: Vec2,     // where the finger landed
    knob: Vec2,       // knob offset from centre, clamped to travel
    steer: Vec2,      // commanded nose vector (screen convention), 0 = centred
    held: bool,
}

impl TouchStick {
    fn new() -> Self {
        TouchStick { id: None, center: Vec2::ZERO, knob: Vec2::ZERO, steer: Vec2::ZERO, held: false }
    }

    fn release(&mut self) {
        self.id = None;
        self.knob = Vec2::ZERO;
        self.steer = Vec2::ZERO;
        self.held = false;
    }

    // Recompute knob offset + steer vector from a finger position (logical px).
    fn apply(&mut self, pos: Vec2, invert: bool) {
        let mut d = pos - self.center;
        let len = d.length();
        if len > STICK_TRAVEL {
            d *= STICK_TRAVEL / len;
        }
        self.knob = d;
        let m = (len / STICK_TRAVEL).min(1.0);
        let eff = if m < STICK_DZ { 0.0 } else { (m - STICK_DZ) / (1.0 - STICK_DZ) };
        self.steer = if eff > 0.0 && len > 0.0 {
            let dir = d / d.length();
            let flip = if invert { -1.0 } else { 1.0 };
            dir * eff * flip
        } else {
            Vec2::ZERO
        };
    }
}

// Draw the floating stick (logical px), matching the original HTML element:
// a soft translucent filled base disc, faint ▲◀▶▼ hint arrows, a ring, and a
// big soft knob. Parked (not held) the whole thing is dimmed to ~0.45; held
// it goes full-opacity with amber ring + knob (engine lit). All alphas mirror
// the old CSS (`background .08`, `border .35→.7`, `knob .45→.85`).
// `scale` shrinks the whole widget (the replay's recorded-input display
// draws at 0.5); the knob offset is scaled here too, so callers always pass
// full-size deflections.
fn draw_stick(center: Vec2, knob: Vec2, held: bool, scale: f32) {
    let (mul, accent) = if held { (1.0, (255u8, 200u8, 0u8)) } else { (0.45, (255, 255, 255)) };
    let a = |alpha: f32| (alpha * mul * 255.0) as u8;
    let radius = STICK_RADIUS * scale;
    let knob = knob * scale;
    // draw_circle/_lines default to 20 sides, which reads polygonal at this
    // radius (~255 physical px), so the ring/base/knob use draw_poly with a
    // high side count for a smooth curve.
    const SIDES: u8 = 64;
    // Soft filled base — this disc is what makes it read round, not hollow.
    draw_poly(center.x, center.y, SIDES, radius, 0.0, Color::from_rgba(255, 255, 255, a(0.08)));
    // Directional hint arrows (static, faint), 12 px in from the rim.
    let hint = Color::from_rgba(255, 255, 255, a(0.30));
    let s = 9.0 * scale; // arrow half-size
    let r = radius - 14.0 * scale;
    let tri = |tip: Vec2, base_a: Vec2, base_b: Vec2| draw_triangle(tip, base_a, base_b, hint);
    tri(vec2(center.x, center.y - r), vec2(center.x - s, center.y - r + s), vec2(center.x + s, center.y - r + s)); // up
    tri(vec2(center.x, center.y + r), vec2(center.x - s, center.y + r - s), vec2(center.x + s, center.y + r - s)); // down
    tri(vec2(center.x - r, center.y), vec2(center.x - r + s, center.y - s), vec2(center.x - r + s, center.y + s)); // left
    tri(vec2(center.x + r, center.y), vec2(center.x + r - s, center.y - s), vec2(center.x + r - s, center.y + s)); // right
    // Ring border.
    draw_poly_lines(center.x, center.y, SIDES, radius, 0.0, 2.5 * scale,
        Color::from_rgba(accent.0, accent.1, accent.2, a(if held { 0.7 } else { 0.35 })));
    // Big soft knob.
    draw_poly(center.x + knob.x, center.y + knob.y, SIDES, STICK_KNOB_R * scale, 0.0,
        Color::from_rgba(accent.0, accent.1, accent.2, a(if held { 0.85 } else { 0.45 })));
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

    // Touch is a first-class input now (the attitude stick lives in-canvas),
    // so stop miniquad synthesizing mouse events from touches — otherwise a
    // canvas tap would fire mouse-down = full thrust.
    simulate_mouse_with_touch(false);

    // The deterministic simulation core: Rapier world, ship, sliding collider
    // windows, fuel/hull/score — everything tick-driven (src/sim.rs). The
    // loop below only supplies per-tick InputStates and reads state back.
    // The startup level: the built-in demo world. index.html pushes the
    // player's chosen level (fetched from levels/) as soon as the exports
    // are live; the loop below applies it as a fresh start.
    let mut sim = Sim::new(Level::demo());

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
    let mut prev_ship = (0.0f32, sim.level.stand_y(0.0), 0.0f32); // x, y, angle
    // Wreck timer (> 0 → crashed: input dead, ship hidden; hands over to the
    // crash dialog when it hits 0). Impact detection itself lives in the sim.
    let mut crash_timer = 0.0f32;
    // Armed-but-idle (#76): a freshly spawned run holds still — no sim.tick,
    // no record_tick — until the first non-neutral input, so every recording
    // (and the ghost's lockstep clock) starts at the pilot's first action,
    // not with the ship sitting at spawn. Keyframe 0 (pushed at recorder
    // creation) is the spawn state; tick 1 is the first commanded tick.
    let mut run_started = false;
    // The current run already ended (out-of-fuel game over): its blob is
    // published and the crash-dialog handover is pending/showing. Guards the
    // reset block's ended-alive publish so the same run isn't reported twice.
    let mut run_over = false;
    let mut mode = Mode::Flying;
    // Instant-replay ring buffer (one frame per physics step) and the
    // playback cursor, in fractional frame indices.
    // WATCH REPLAY playback: a scratch Sim re-simulating the hybrid
    // recording's inputs on the wall clock.
    let mut replay_player: Option<ResimPlayer> = None;

    // Hybrid recording (src/replay.rs): the shareable spawn→crash replay —
    // input change-events + 1 Hz keyframes + params header. In memory only
    // for now; serialized + deflated at the crash to measure what shipping
    // it would cost (shown on the WATCH REPLAY button).
    let mut recorder = Recording::new(sim_params(), sim.level.to_params(),
        (HYBRID_MAX_SECS / PHYSICS_DT) as u32);
    recorder.push_keyframe(sim.keyframe(0, 0.0));

    // Ghost of the BEST run: JS pushes the current level's top-highscore
    // recording (load_ghost_blob) and it is RE-SIMULATED in lockstep with
    // live play — one ghost tick per live tick, same spawn clock — drawn as
    // a translucent silhouette. Toggled by the "Race best ghost" checkbox
    // (GHOST_ON); recreated from ghost_rec at every spawn.
    let mut ghost_rec: Option<Recording> = None;
    let mut ghost_player: Option<ResimPlayer> = None;

    // An externally-loaded replay being watched (a highscore's ▶ button →
    // watch_replay_blob). While Some, the Replay mode plays THIS recording
    // instead of the live recorder, and exiting returns to `watch_return`
    // (the dialog if a wreck is waiting, otherwise flight — physics was
    // paused throughout, so the interrupted run resumes untouched).
    let mut watch_rec: Option<Recording> = None;
    let mut watch_return = Mode::Flying;

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
    let mut shake = 0.0f32; // impact screen-shake intensity, 0..1, decays fast
    // Replay-ending grace: after playback pauses on the final frame, particle
    // time keeps running this long so the crash debris bursts and the plume
    // fades out — THEN the freeze is total. Emission stays off throughout
    // (paused), so nothing new streams from the frozen ship.
    let mut replay_boom_timer = 0.0f32;
    let mut stick_thrust_t = 0.0f32; // seconds the stick-hold engine has been eligible
    let mut flip_settling = false;   // big flip commanded: engine cold until nose settles
    let mut pad_msg_timer = 0.0f32; // "+100" flash after a first landing
    let mut stick = TouchStick::new();

    loop {
        // Apply a level pushed from JS (startup restore or the overlay
        // picker): a full fresh start in the new world. The ghost is
        // dropped — it was flown on a different level.
        if let Some(lvl) = PENDING_LEVEL.lock().unwrap().take()
            && lvl != sim.level
        {
            sim = Sim::new(lvl);
            prev_ship = (SPAWN_X, sim.level.stand_y(SPAWN_X), 0.0);
            crash_timer = 0.0;
            shake = 0.0;
            phys_accum = 0.0;
            mode = Mode::Flying;
            run_started = false;
            run_over = false;
            recorder = Recording::new(sim_params(), sim.level.to_params(),
                (HYBRID_MAX_SECS / PHYSICS_DT) as u32);
            recorder.push_keyframe(sim.keyframe(0, 0.0));
            ghost_rec = None;
            ghost_player = None;
            replay_player = None;
            watch_rec = None;
            glow = 0.0;
        }

        // Best-run ghost pushed from JS (the level's global-record replay).
        // Wrong-level recordings are dropped — the ghost must race THIS
        // world. Adopted for future spawns; if the live run is still young
        // the player also starts now and the lockstep loop below catches it
        // up within one frame (bounded burst), otherwise the racing ghost
        // (if any) keeps going and the new one appears at the next spawn.
        if let Some(g) = PENDING_GHOST.lock().unwrap().take()
            && g.level == sim.level.to_params()
        {
            let young = recorder.ticks() <= (30.0 / PHYSICS_DT) as u32;
            ghost_rec = Some(g);
            if GHOST_ON.load(Ordering::Relaxed) != 0 && young && mode == Mode::Flying {
                ghost_player = ghost_rec.as_ref().and_then(ResimPlayer::new);
            }
        }

        // A stored replay to watch (highscore ▶). Pauses whatever was
        // happening — the sim freezes, so the interrupted run resumes when
        // the replay ends or is skipped.
        if let Some(rec) = PENDING_WATCH.lock().unwrap().take()
            && let Some(p) = ResimPlayer::new(&rec)
        {
            watch_return = if sim.crashed { Mode::CrashDialog } else { Mode::Flying };
            crash_timer = 0.0; // skip any remaining wreck pause
            watch_rec = Some(rec);
            replay_player = Some(p);
            mode = Mode::Replay;
            REPLAY_PAUSED.store(0, Ordering::Relaxed);
            REPLAY_STEP.store(0, Ordering::Relaxed);
            REPLAY_SEEK.store(SEEK_NONE, Ordering::Relaxed);
            REPLAY_SPEED.store(1.0f32.to_bits(), Ordering::Relaxed);
            REPLAY_UI_VISIBLE.store(1, Ordering::Relaxed);
        }

        // One-shot HTML-UI commands (menu buttons). Reset flows through the
        // same path as the R key below; watch-replay only makes sense while
        // the crash dialog is waiting.
        let mut ui_do_reset = false;
        match UI_CMD.swap(0, Ordering::Relaxed) {
            1 => ui_do_reset = true,
            2 if mode == Mode::CrashDialog => {
                if let Some(p) = ResimPlayer::new(&recorder) {
                    replay_player = Some(p);
                    mode = Mode::Replay;
                    REPLAY_PAUSED.store(0, Ordering::Relaxed);
                    REPLAY_STEP.store(0, Ordering::Relaxed);
                    REPLAY_SEEK.store(SEEK_NONE, Ordering::Relaxed);
                    REPLAY_SPEED.store(1.0f32.to_bits(), Ordering::Relaxed);
                    REPLAY_UI_VISIBLE.store(1, Ordering::Relaxed);
                }
            }
            3 if mode == Mode::Replay => {
                // The ✕ button: playback freezes on its final frame instead
                // of exiting, so leaving the replay is always this explicit
                // command (or the R-key respawn).
                replay_player = None;
                particles.clear();
                replay_boom_timer = 0.0;
                mode = if watch_rec.take().is_some() { watch_return } else { Mode::CrashDialog };
            }
            _ => {}
        }
        // HTML overlay up (menu / pause view): freeze the live sim. Replay
        // playback is deliberately not gated — a stored replay watched from
        // the menu plays with no overlay covering it.
        let ui_paused = UI_PAUSE.load(Ordering::Relaxed) != 0;

        // --- Gather this frame's touch input (mobile) ---
        // Device sampling plus the stick-hold gating machine. This is input
        // GENERATION (allowed to be frame-based); the quantized InputState it
        // produces is what the sim consumes per tick AND what the recorder
        // stores, so live play and resim see bit-identical inputs.
        //
        // The floating stick claims the first fresh touch anywhere on screen
        // while flying (the whole canvas is the flight-control surface — the
        // pause/restart buttons and the replay bar are HTML and swallow
        // their own taps); during the dialog/replay `stick_active` is false
        // and fresh touches are ignored (the out-of-flight UI is HTML).
        // `touches()` reports RAW physical px, so divide by dpi to reach the
        // LOGICAL space that `screen_*()`, `mouse_position()` and all
        // drawing use.
        let touch_dpi = screen_dpi_scale();
        let tpos = |t: &Touch| t.position / touch_dpi;
        let invert = INVERT_STICK.load(Ordering::Relaxed) != 0;
        let stick_active = matches!(mode, Mode::Flying) && crash_timer <= 0.0 && !ui_paused;
        // Keep following / release the claimed stick touch.
        if let Some(id) = stick.id {
            match touches().iter().find(|t| t.id == id) {
                Some(t) if !matches!(t.phase, TouchPhase::Ended | TouchPhase::Cancelled) => {
                    stick.apply(tpos(t), invert);
                }
                _ => stick.release(),
            }
        }
        if !stick_active {
            stick.release();
        }
        for t in touches() {
            if t.phase != TouchPhase::Started {
                continue;
            }
            if stick_active && stick.id.is_none() {
                let p = tpos(&t);
                stick.id = Some(t.id);
                stick.center = p;
                stick.held = true;
                stick.apply(p, invert);
            }
        }

        let stick_held = stick.held;
        let (steer_x, steer_y) = (stick.steer.x, stick.steer.y);
        let steer_mag = (steer_x * steer_x + steer_y * steer_y).sqrt().min(1.0);
        // Heading error to the commanded nose direction (0 when centred),
        // for the flip gate. Uses the true body angle.
        let heading_err = if steer_mag > 0.0 {
            let target = (-steer_x).atan2(-steer_y);
            let mut e = target - sim.ship_pose().2;
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
        let mut throttle_cmd = stick_throttle;
        if is_mouse_button_down(MouseButton::Left)
            || is_key_down(KeyCode::Down)
            || PAD_THRUST.load(Ordering::Relaxed) != 0
        {
            throttle_cmd = 1.0;
        }
        // Manual rate rotation: keyboard keys and the gamepad's analog stick.
        let pad_torque = f32::from_bits(PAD_TORQUE.load(Ordering::Relaxed)).clamp(-1.0, 1.0);
        let rot: i8 = if is_key_down(KeyCode::Left) || pad_torque < -0.1 {
            -1
        } else if is_key_down(KeyCode::Right) || pad_torque > 0.1 {
            1
        } else {
            0
        };
        // Inputs are commands — the fuel gate lives in the sim. A dead ship
        // (or a paused mode) commands nothing.
        let input = if mode == Mode::Flying && crash_timer <= 0.0 && !ui_paused {
            InputState::from_controls(throttle_cmd, rot, steer_x, steer_y, stick_held)
        } else {
            InputState::default()
        };

        // --- Fixed timestep ---
        // The cap bounds catch-up work after a hitch. Physics only runs
        // while flying — the crash dialog and the replay pause the sim (the
        // wreck is parked anyway) and drain the accumulator so no catch-up
        // burst fires on resume. Tick events are aggregated for the
        // frame-level cosmetics below.
        let mut frame_impact: Option<Impact> = None;
        let mut frame_landed = false;
        let mut frame_scored = false;
        let mut frame_fuel_out = false;
        let mut frame_heading_torque = 0.0f32;
        // First input starts the run clock. The gate lives HERE, outside
        // Sim::tick — input gathering is frame-level, and the sim must stay
        // a pure function of the input stream it is actually fed (see the
        // determinism rules): resim never sees the armed-idle wait because
        // it was never ticked or recorded.
        if !run_started && !input.is_neutral() {
            run_started = true;
            RUN_START_SEQ.fetch_add(1, Ordering::Relaxed);
        }
        if mode == Mode::Flying && !ui_paused && run_started {
            phys_accum = (phys_accum + get_frame_time()).min(0.05);
            while phys_accum >= PHYSICS_DT {
                prev_ship = sim.ship_pose();
                let was_crashed = sim.crashed;
                let rep = sim.tick(input);
                phys_accum -= PHYSICS_DT;
                if was_crashed {
                    continue; // parked wreck: nothing to record or report
                }
                let destroyed = rep.impact.as_ref().is_some_and(|i| i.destroyed);
                // Hybrid recording: the input in effect for this step (an
                // event only when it changed) + periodic keyframes. The
                // destruction tick skips its periodic keyframe — the
                // terminal one below carries the impact state instead.
                if recorder.record_tick(input) && !destroyed {
                    recorder.push_keyframe(sim.keyframe(recorder.ticks(), glow));
                }
                // Ghost: re-simulate the best run in LOCKSTEP — one ghost
                // tick per live tick, so both ships fly the same spawn
                // clock. The `while` also absorbs a ghost adopted a moment
                // after the spawn (bounded catch-up burst, see the adoption
                // block above). (A trimmed recording starts at tick > 0;
                // the ghost then waits for the live run to reach it.)
                if let (Some(p), Some(r)) = (ghost_player.as_mut(), ghost_rec.as_ref()) {
                    while !p.finished && p.tick < recorder.ticks() {
                        p.step_one(r);
                    }
                }
                frame_heading_torque = rep.heading_torque;
                frame_landed = rep.landed;
                frame_scored |= rep.scored;
                frame_fuel_out |= rep.fuel_out;
                if let Some(imp) = rep.impact {
                    let replace = frame_impact
                        .as_ref()
                        .is_none_or(|o| imp.destroyed || (!o.destroyed && imp.dv > o.dv));
                    if replace {
                        frame_impact = Some(imp);
                    }
                }
            }
        } else {
            phys_accum = 0.0;
        }

        // --- Crash flow & impact cosmetics (from the ticks' reports) ---
        // Detection and hull bookkeeping happen inside sim.tick(); this only
        // turns the strongest impact of the frame into effects and, on
        // destruction, freezes the shareable recording.
        if let Some(imp) = &frame_impact {
            shake = (shake + imp.damage / HULL_MAX + 0.25).min(1.0);
            if imp.destroyed {
                crash_timer = CRASH_DIALOG_DELAY;
                // Debris burst at the crash site.
                boom_burst(imp.x, imp.y, &mut particles);
                if SOUND_ON.load(Ordering::Relaxed) != 0 && let Some(s) = &boom_snd {
                    play_sound(s, PlaySoundParams { looped: false, volume: 0.9 });
                }
                // Terminal keyframe with the impact pose/velocity (the wreck
                // is already parked and zeroed).
                recorder.finalize(Keyframe {
                    tick: recorder.ticks(),
                    x: imp.x, y: imp.y, rot_re: imp.rot_re, rot_im: imp.rot_im,
                    vx: imp.vx, vy: imp.vy, angvel: imp.angvel,
                    fuel: sim.fuel, hull: sim.hull, glow,
                    land_timer: 0.0, // a destroying tick always zeroes it
                });
                // Publish for the online submit flow and the analytics
                // channel (which also takes short runs) — unless the run
                // already ended (a destroying impact during the out-of-fuel
                // handover wait must not publish the same run twice).
                if !run_over {
                    run_over = true;
                    report_run_end(&recorder, sim.max_dist);
                    report_run_analytics(0, recorder.ticks(), sim.max_dist, sim.fuel, sim.hull);
                }
            } else {
                // Survivable scrape: a spray of sparks + a quiet thud,
                // scaled to the damage taken. The ship keeps flying.
                for _ in 0..(6 + (imp.damage * 0.3) as i32) {
                    let ang = gen_range(0.0f32, std::f32::consts::TAU);
                    let spd = gen_range(0.8f32, 4.0);
                    particles.push(Particle {
                        x: imp.x + gen_range(-0.25f32, 0.25),
                        y: imp.y + gen_range(-0.25f32, 0.25),
                        vx: imp.vx * 0.3 + ang.cos() * spd,
                        vy: imp.vy * 0.3 + ang.sin() * spd + 1.0,
                        life: gen_range(0.25f32, 0.55),
                        kind: 3,
                    });
                }
                if SOUND_ON.load(Ordering::Relaxed) != 0 && let Some(s) = &boom_snd {
                    play_sound(s, PlaySoundParams { looped: false, volume: 0.25 });
                }
            }
        }
        // Out of fuel past the deadline: the run is over. Publish it and go
        // STRAIGHT to the game-over dialog — the "OUT OF FUEL" banner has
        // already been up since the tank emptied, so unlike a crash there's
        // no explosion to play out and no handover delay (the JS ui-state
        // poll collects the just-ended run synchronously when it sees
        // state 2, so the submit dialog can't miss it). No wreck: the sim
        // isn't crashed, the intact ship stays visible, and there's no boom.
        if frame_fuel_out && mode == Mode::Flying && crash_timer <= 0.0
            && !sim.crashed && !run_over
        {
            run_over = true;
            report_run_end(&recorder, sim.max_dist);
            report_run_analytics(2, recorder.ticks(), sim.max_dist, sim.fuel, sim.hull);
            mode = Mode::CrashDialog;
        }
        // Wreck timer → once the explosion has played out, hand over to the
        // crash dialog (fly again / watch replay). Respawn happens from there.
        if crash_timer > 0.0 && !ui_paused {
            crash_timer -= get_frame_time();
            if crash_timer <= 0.0 {
                crash_timer = 0.0;
                mode = Mode::CrashDialog;
            }
        }
        let crashed = crash_timer > 0.0;

        // Mirror the mode + run distance for the HTML overlay state machine
        // (the JS side polls ui_state / cur_dist — see index.html).
        UI_STATE.store(
            match mode {
                Mode::Flying if crashed => 1,
                Mode::Flying => 0,
                Mode::CrashDialog => 2,
                Mode::Replay => 3,
            },
            Ordering::Relaxed,
        );
        CUR_DIST.store(sim.max_dist.to_bits(), Ordering::Relaxed);

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

        // Safe-area insets (notch / status bar / home indicator / browser
        // toolbar), supplied by JS via env(safe-area-inset-*) in CSS px.
        // CSS px == the LOGICAL space everything is drawn in (macroquad's
        // screen_width() is physical/dpi), so these are used AS-IS — NOT
        // ×dpi. (The old ×dpi was a latent bug masked by insets being ~0 in
        // browser-chrome mode; it surfaced as a minimap shoved 3× too low in
        // fullscreen, where the notch inset is real.) The top inset is
        // honoured in full; the LEFT inset is capped — in landscape the
        // island sits mid-edge, not in the corner, and the full ~47-59 px
        // inset shoved the minimap far in when only the bezel corner matters.
        let safe_top = f32::from_bits(SAFE_AREA_TOP.load(Ordering::Relaxed));
        let safe_left = f32::from_bits(SAFE_AREA_LEFT.load(Ordering::Relaxed)).min(24.0);
        let safe_bottom = f32::from_bits(SAFE_AREA_BOTTOM.load(Ordering::Relaxed));
        let safe_right = f32::from_bits(SAFE_AREA_RIGHT.load(Ordering::Relaxed));

        let (mut cam_x, mut cam_y, mut angle, mut ship_vx, mut ship_vy) = {
            let (bx, by, ba) = sim.ship_pose();
            let (vx, vy) = sim.ship_vel();
            // Interpolate between the last two physics steps so rendering
            // stays smooth when the frame rate and PHYSICS_DT don't divide
            // evenly (e.g. 144 Hz display over a 120 Hz simulation).
            let alpha = (phys_accum / PHYSICS_DT).clamp(0.0, 1.0);
            let (px, py, pa) = prev_ship;
            (
                px + (bx - px) * alpha,
                py + (by - py) * alpha,
                lerp_angle(pa, ba, alpha),
                vx, vy,
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

        // Replay playback: RE-SIMULATED from the hybrid recording — the
        // scratch Sim re-runs the input events in real time and the camera/
        // pose/velocity are overridden with its state. Its own collider
        // windows follow the re-simmed ship, so the world (and collisions)
        // are genuinely recomputed, not played back. Ends by re-simulating
        // the destroying impact, then returns to the dialog.
        let mut replay_frame: Option<ReplayFrame> = None;
        if mode == Mode::Replay {
            // Playback source: an externally-loaded highscore replay when
            // one is active, else the live recorder (crash-dialog replay).
            let play_rec = watch_rec.as_ref().unwrap_or(&recorder);
            if let Some(p) = replay_player.as_mut() {
                // Hitting play on the last frame restarts from the top. The
                // finish auto-pauses (below), so entering a frame finished
                // AND unpaused can only mean the user pressed play/space on
                // the final frame. Checked BEFORE this frame's transport
                // commands so a scrub-to-the-end while playing pauses on the
                // finale (via the transition below) instead of instantly
                // looping.
                if p.finished && REPLAY_PAUSED.load(Ordering::Relaxed) == 0 {
                    p.seek_to_tick(play_rec, p.first_tick);
                    particles.clear();
                    replay_boom_timer = 0.0;
                }
                // Captured BEFORE the transport commands so a seek/step that
                // lands exactly on the final tick counts as the transition
                // into finished (pause + boom below), same as playing there.
                let was_finished = p.finished;
                // Transport controls: play/pause, keyframe scrubbing and
                // playback speed, from the HTML replay bar (atomics) or the
                // keyboard (space = pause, ←/→ = one keyframe, S = cycle
                // speed — the native/dev fallback).
                if is_key_pressed(KeyCode::Space) {
                    REPLAY_PAUSED.fetch_xor(1, Ordering::Relaxed);
                }
                if is_key_pressed(KeyCode::S) {
                    let cur = f32::from_bits(REPLAY_SPEED.load(Ordering::Relaxed));
                    let i = REPLAY_SPEEDS
                        .iter()
                        .position(|s| (s - cur).abs() < 0.01)
                        .unwrap_or(2);
                    let next = REPLAY_SPEEDS[(i + 1) % REPLAY_SPEEDS.len()];
                    REPLAY_SPEED.store(next.to_bits(), Ordering::Relaxed);
                }
                let seek_bits = REPLAY_SEEK.swap(SEEK_NONE, Ordering::Relaxed);
                // Transport steps are 0.1 s of sim time (12 ticks); the
                // seek engine underneath stays tick-exact. REPLAY_STEP
                // arrives from JS already in ticks (the bar sends 0.1 s
                // worth per tap).
                let step_unit = (0.1 / PHYSICS_DT).round() as i64;
                let step_ticks = (is_key_pressed(KeyCode::Right) as i64
                    - is_key_pressed(KeyCode::Left) as i64) * step_unit
                    + REPLAY_STEP.swap(0, Ordering::Relaxed) as i64;
                if seek_bits != SEEK_NONE {
                    // Bar position → exact tick (frame-level scrubbing; the
                    // slider's 1000 positions are the only quantisation).
                    let span = (p.end_tick - p.first_tick) as f32;
                    let target = p.first_tick
                        + (f32::from_bits(seek_bits).clamp(0.0, 1.0) * span).round() as u32;
                    let before = p.tick;
                    p.seek_to_tick(play_rec, target);
                    // Rebuild the plume the ship "should" trail at the
                    // landing tick (the camera teleports, so the old
                    // particles are wrong in both position and time).
                    if p.tick != before {
                        rebuild_replay_particles(play_rec, p.tick, &mut particles);
                        replay_boom_timer = 0.0;
                    }
                } else if step_ticks != 0 {
                    // Steps (⏮/⏭ buttons, ←/→ keys) auto-pause: a 0.1 s
                    // step during playback would barely register.
                    REPLAY_PAUSED.store(1, Ordering::Relaxed);
                    let before = p.tick;
                    let target = (p.tick as i64 + step_ticks).max(0) as u32;
                    p.seek_to_tick(play_rec, target);
                    if p.tick != before {
                        rebuild_replay_particles(play_rec, p.tick, &mut particles);
                        replay_boom_timer = 0.0;
                    }
                }
                // Paused playback still renders: advance(0) re-simulates
                // nothing and returns the frozen interpolated frame. The
                // speed multiplier scales the wall-clock time fed to the
                // re-sim (raw frame time clamped first, so a browser hitch
                // at 4× can't burst past advance()'s catch-up cap).
                let paused = REPLAY_PAUSED.load(Ordering::Relaxed) != 0;
                let speed = f32::from_bits(REPLAY_SPEED.load(Ordering::Relaxed));
                let dt = if paused { 0.0 } else { get_frame_time().min(0.05) * speed };
                let f = p.advance(play_rec, dt);
                REPLAY_POS.store(p.progress().to_bits(), Ordering::Relaxed);
                REPLAY_LEN.store(
                    (((p.end_tick - p.first_tick) as f32) * PHYSICS_DT).to_bits(),
                    Ordering::Relaxed,
                );
                // Reaching the end does NOT exit: playback PAUSES on the
                // final frame — play restarts from the top, scrubbing keeps
                // working, the ✕ button / ui_command(3) / R key leave. The
                // ending still ANIMATES: the crash debris bursts and the
                // plume fades during the replay_boom_timer grace (particle
                // time keeps running while emission stays off), then the
                // freeze is total on a clean frame. Re-fires if the finale
                // is replayed after a scrub back; runs that ended by manual
                // reset just fade their plume silently.
                if p.finished && !was_finished {
                    REPLAY_PAUSED.store(1, Ordering::Relaxed);
                    replay_boom_timer = 1.2;
                    if p.sim.crashed {
                        boom_burst(f.x, f.y, &mut particles);
                        if SOUND_ON.load(Ordering::Relaxed) != 0 && let Some(s) = &boom_snd {
                            play_sound(s, PlaySoundParams { looped: false, volume: 0.9 });
                        }
                    }
                }
                replay_frame = Some(f);
            } else {
                // No player (every entry point builds one, so this is just a
                // safety net): fall back out of the mode.
                mode = if watch_rec.take().is_some() { watch_return } else { Mode::CrashDialog };
            }
        }
        if let Some(f) = replay_frame {
            cam_x = f.x;
            cam_y = f.y;
            angle = f.angle;
            ship_vx = f.vx;
            ship_vy = f.vy;
        }

        // Ghost of the last run: the lockstep re-sim's pose, lerped with the
        // SAME alpha as the live ship so both move in sync. None once the
        // ghost reaches its crash (it "dies" there), before a trimmed
        // recording's first keyframe, outside live flight, or during the
        // armed-but-idle wait (`run_started` — both ships would sit
        // overlapped on the spawn until the first control command).
        let ghost_pose: Option<(f32, f32, f32)> = match (&ghost_player, mode) {
            (Some(p), Mode::Flying)
                if GHOST_ON.load(Ordering::Relaxed) != 0
                    && run_started
                    && !crashed
                    && !p.finished
                    && recorder.ticks() >= p.first_tick =>
            {
                Some(p.lerped_pose((phys_accum / PHYSICS_DT).clamp(0.0, 1.0)))
            }
            _ => None,
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

        // Engine glow follows the fuel-gated command (the command itself was
        // gathered at the top of the frame; the sim applies the same gate,
        // so visuals track what the engine actually does). During playback
        // the recording overrides it.
        let throttle_fx = if sim.fuel > 0.0 { input.throttle_f32() } else { 0.0 };
        glow += (throttle_fx - glow) * 0.12;
        if let Some(f) = replay_frame {
            glow = f.glow;
        }
        // A paused replay freezes cosmetic time too (see the particle clock
        // below); the engine hum mutes with it.
        let replay_paused_now =
            mode == Mode::Replay && REPLAY_PAUSED.load(Ordering::Relaxed) != 0;
        if let Some(s) = &thruster_snd {
            let vol = if SOUND_ON.load(Ordering::Relaxed) != 0 && !replay_paused_now {
                glow * 0.6
            } else {
                0.0
            };
            set_sound_volume(s, vol);
        }

        // Column/layer ranges for the wall meshes below. Rendering derives
        // its own ranges from the camera (which tracks the replayed pose in
        // playback); the collider windows are the sim's business and follow
        // the true body position inside sim.tick().
        let ship_seg = (cam_x / SEG_LEN).floor() as i64;
        let want_left  = ship_seg - HALF_WINDOW;
        let want_right = ship_seg + HALF_WINDOW;
        let ship_layer = (cam_y / V_PERIOD).round() as i64;
        let (lay_lo, lay_hi) = (ship_layer - 1, ship_layer + 1);

        // Landing state for the banners: detection/refuel/score happen per
        // tick in the sim; here we just flash the "+100" on a first visit.
        let landed = frame_landed;
        if frame_scored {
            pad_msg_timer = 1.8;
        }
        pad_msg_timer = (pad_msg_timer - get_frame_time()).max(0.0);

        // During replay playback the world (walls, pads, obstacles — and the
        // run's fuel/hull/score below) renders from the SCRATCH sim: its
        // windows and level follow the re-simmed run, while the main sim's
        // stay parked at the wreck.
        let world_sim = replay_player.as_ref().map_or(&sim, |p| &p.sim);

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
                    if world_sim.level.seg_in_opening(col.div_euclid(SUBCOLS)) {
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
                            let w00 = lattice_point(&world_sim.level, col,     row,     side);
                            let w10 = lattice_point(&world_sim.level, col + 1, row,     side);
                            let w11 = lattice_point(&world_sim.level, col + 1, row + 1, side);
                            let w01 = lattice_point(&world_sim.level, col,     row + 1, side);
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
                        let wd0 = lattice_point(&world_sim.level, col,     N_ROWS - 1, 0);
                        let wd1 = lattice_point(&world_sim.level, col + 1, N_ROWS - 1, 0);
                        let wu0 = lattice_point(&world_sim.level, col,     N_ROWS - 1, 1);
                        let wu1 = lattice_point(&world_sim.level, col + 1, N_ROWS - 1, 1);
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
        for (&(s, _gap), shaft) in world_sim.shafts.iter() {
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
        // BTreeMap iteration is key-ordered, so adjacent overlapping boulders
        // keep a stable z-order as the window slides.
        for (&(k, _layer), ob) in world_sim.obstacles.iter() {
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
        for (&pad_key, pad) in world_sim.pads.iter() {
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
                let ground = pad_layer as f32 * V_PERIOD
                    + world_sim.level.cave_center(lx) - world_sim.level.cave_half_width(lx);
                let top = w2s(lx, pad.y, sh, cam_x, cam_y);
                let bot = w2s(lx, ground.min(pad.y), sh, cam_x, cam_y);
                draw_line(top.x, top.y + deck_h, bot.x, bot.y, 3.0 * dpi,
                    Color::from_rgba(60, 68, 82, 255));
            }
            // Beacons: blinking green until first landing, then steady blue.
            let visited = world_sim.visited_pads.contains(&pad_key);
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

        // Ghost of the last run — a translucent silhouette racing the same
        // spawn clock, drawn behind the player ship. No flame/particles: a
        // quiet presence, not a second ship fighting for attention.
        if let Some((gx, gy, ga)) = ghost_pose {
            let gs = w2s(gx, gy, sh, cam_x, cam_y);
            if gs.x > -120.0 && gs.x < sw + 120.0 && gs.y > -120.0 && gs.y < sh + 120.0 {
                let g_rot = |lx: f32, ly: f32| -> Vec2 {
                    let sx = lx * SHIP_SCALE;
                    let sy = ly * SHIP_SCALE;
                    w2s(
                        gx + sx * ga.cos() - sy * ga.sin(),
                        gy + sx * ga.sin() + sy * ga.cos(),
                        sh, cam_x, cam_y,
                    )
                };
                let gc = Color::from_rgba(150, 190, 255, 70);
                for t in SHIP_TRIS.iter() {
                    draw_triangle(g_rot(t[0], t[1]), g_rot(t[2], t[3]), g_rot(t[4], t[5]), gc);
                }
                // The ghost pilot's callsign floats just under the
                // silhouette (unrotated; empty = no label, e.g. offline).
                let name = GHOST_NAME.lock().unwrap();
                if !name.is_empty() {
                    // Names render uppercase everywhere (boards, picker, HUD).
                    let label = name.to_uppercase();
                    let fs = 20.0 * ui;
                    let dim = measure_text(&label, None, fs as u16, 1.0);
                    draw_text(&label, gs.x - dim.width / 2.0,
                        gs.y + 1.05 * view_scale + fs,
                        fs, Color::from_rgba(150, 190, 255, 150));
                }
            }
        }

        // The ship renders while flying (unless it's a wreck) and during the
        // replay (where cam/angle/glow carry the recorded pose); the crash
        // dialog shows no ship — it was just destroyed.
        let ship_visible = match mode {
            Mode::Flying => !crashed,
            // The hull vanishes in the replayed explosion, like live play
            // (the scratch sim's crashed flag is set exactly at the
            // destroying tick; scrubbing back rebuilds a fresh sim, so the
            // ship reappears automatically).
            Mode::Replay => !replay_player.as_ref().is_some_and(|p| p.sim.crashed),
            // An out-of-fuel game over has no wreck (sim.crashed is false):
            // the intact ship stays visible behind the dialog. Only a real
            // destruction hides it.
            Mode::CrashDialog => !sim.crashed,
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
        let debug_hud = DEBUG_HUD.load(Ordering::Relaxed) != 0;
        // Distance levels: the score IS the farthest |x| reached; raise the
        // per-level best live (seeded with the global record by JS).
        if sim.level.scoring == Scoring::Distance
            && sim.max_dist > f32::from_bits(BEST_DIST.load(Ordering::Relaxed))
        {
            BEST_DIST.store(sim.max_dist.to_bits(), Ordering::Relaxed);
            // The seeded record just fell — the holder is now the pilot.
            let mut name = BEST_NAME.lock().unwrap();
            if name.as_str() != "you" {
                *name = "you".to_string();
            }
        }

        // The prominent distance / score readout is drawn LATER — below the
        // minimap + fuel + hull gauges (see the gauge block) — so it needs
        // their laid-out y; it can't go here (before the minimap section).

        // Full telemetry line — Debug HUD only. Position/layer/cave-progress,
        // FPS, and the numeric speed readout (in the shared danger colour).
        if debug_hud {
            let hud_fs = 30.0 * ui;
            let hud_y = safe_top + 252.0 * ui; // below the fuel + hull gauges
            let hud = match world_sim.level.scoring {
                Scoring::Distance => format!(
                    "dist={:.0}m  best={:.0}m  x={:.0}  lvl={}   [R] reset   FPS: {:.0}",
                    world_sim.max_dist, get_best_dist(), cam_x, ship_layer, smooth_fps),
                Scoring::Pads => format!(
                    "score={}  x={:.0}  lvl={}  {:.0}m/{}m   [R] reset   FPS: {:.0}",
                    world_sim.score, cam_x, ship_layer, cave_x, PERIOD as i32, smooth_fps),
            };
            draw_text(&hud, safe_left + 10.0 * ui, hud_y, hud_fs, WHITE);
            let hud_w = measure_text(&hud, None, hud_fs as u16, 1.0).width;
            draw_text(format!("  v={speed:.1}"),
                safe_left + 10.0 * ui + hud_w, hud_y, hud_fs, speed_col);
        }

        // In-canvas floating attitude stick (mobile). Spawned under the
        // finger while held (amber = engine lit); parked bottom-right as a
        // translucent ghost otherwise. Only while flying — the dialog/replay
        // draw their own UI. Parked position uses the safe-area insets we
        // already have (approx bottom/right from the top/left-derived margins).
        // Parked stick home (bottom-right), clear of the home indicator /
        // browser toolbar via the bottom+right safe insets. Also where the
        // replay draws the stick, animated by the recorded input.
        let stick_park = vec2(
            sw - safe_right - STICK_RADIUS - 24.0,
            sh - safe_bottom - STICK_RADIUS - 28.0,
        );
        if matches!(mode, Mode::Flying) && !crashed && !ui_paused {
            if stick.id.is_some() {
                draw_stick(stick.center, stick.knob, stick.held, 1.0);
            } else {
                draw_stick(stick_park, Vec2::ZERO, false, 1.0);
            }
        }

        // Crash dialog / replay overlay / status banners. `ui_do_reset` (the
        // HTML UI's fly-again/restart command) is consumed by the reset block
        // below, same path as the R key.
        if mode == Mode::CrashDialog {
            // On web the HTML game-over screen (index.html) covers this and
            // drives the choices via ui_command; what's drawn here is the
            // dim + keyboard fallback for native/dev builds.
            draw_rectangle(0.0, 0.0, sw, sh, Color::from_rgba(0, 0, 0, 130));
            let (msg, col) = if sim.crashed {
                ("CRASHED", Color::from_rgba(255, 90, 60, 255))
            } else {
                ("OUT OF FUEL", Color::from_rgba(255, 180, 60, 255))
            };
            let fs = 96.0 * ui;
            let dims = measure_text(msg, None, fs as u16, 1.0);
            draw_text(msg, (sw - dims.width) / 2.0, sh * 0.30, fs, col);
            let hint = "[R] fly again   ·   [ENTER] watch replay";
            let hfs = 26.0 * ui;
            let hd = measure_text(hint, None, hfs as u16, 1.0);
            draw_text(hint, (sw - hd.width) / 2.0, sh * 0.38, hfs,
                Color::from_rgba(170, 180, 200, 255));
            if is_key_pressed(KeyCode::Enter) {
                // Re-simulate the hybrid recording from its first keyframe.
                if let Some(p) = ResimPlayer::new(&recorder) {
                    replay_player = Some(p);
                    mode = Mode::Replay;
                    REPLAY_PAUSED.store(0, Ordering::Relaxed);
                    REPLAY_STEP.store(0, Ordering::Relaxed);
                    REPLAY_SEEK.store(SEEK_NONE, Ordering::Relaxed);
                    REPLAY_SPEED.store(1.0f32.to_bits(), Ordering::Relaxed);
                    REPLAY_UI_VISIBLE.store(1, Ordering::Relaxed);
                }
            }
        } else if mode == Mode::Replay {
            // The transport UI is the HTML replay bar (index.html); the only
            // in-canvas element is the recorded-input stick — HALF SIZE,
            // raised above the bar — animated by the input driving the
            // re-sim (knob at the recorded deflection, amber while held), so
            // a replay shows the pilot's hand. Keys remain as the native/dev
            // fallback: space pause, ←/→ step, S speed, R respawn. (The
            // per-keyframe drift check + snap still run; they're just no
            // longer displayed. Throttle meter: see #67.)
            let inp = replay_player.as_ref().map(|p| p.current_input()).unwrap_or_default();
            let (isx, isy) = inp.steer_f32();
            // The OUT OF FUEL banner replays too — from the SCRATCH sim's
            // fuel (the state the replay is showing), exactly like live
            // play: up from the tick the tank empties until the (replayed)
            // crash, and through a fuel-out ending's freeze frame. Not part
            // of the auto-hiding transport GUI — it's game state.
            if replay_player.as_ref().is_some_and(|p| p.sim.fuel <= 0.0 && !p.sim.crashed) {
                let msg = "OUT OF FUEL";
                let fs = 48.0 * ui;
                let dims = measure_text(msg, None, fs as u16, 1.0);
                draw_text(msg, (sw - dims.width) / 2.0, sh * 0.42, fs,
                    Color::from_rgba(255, 180, 60, 255));
            }
            if REPLAY_UI_VISIBLE.load(Ordering::Relaxed) != 0 {
                // Half size, tucked into the corner with tighter margins
                // than the full-size park spot, and clear of the HTML replay
                // bar (~154 CSS px incl. its bottom offset; logical px ==
                // CSS px).
                let r = STICK_RADIUS * 0.5;
                let replay_stick_home = vec2(
                    sw - safe_right - r - 12.0,
                    sh - safe_bottom - r - 168.0,
                );
                draw_stick(replay_stick_home, vec2(isx, isy) * STICK_TRAVEL,
                    inp.stick_held != 0, 0.5);
            } else {
                // GUI auto-hidden: the stick takes back its full-size parked
                // home, exactly where the live stick sits.
                draw_stick(stick_park, vec2(isx, isy) * STICK_TRAVEL,
                    inp.stick_held != 0, 1.0);
            }
        } else if crashed {
            let msg = "CRASHED";
            let fs = 96.0 * ui;
            let dims = measure_text(msg, None, fs as u16, 1.0);
            draw_text(msg, (sw - dims.width) / 2.0, sh * 0.42, fs,
                Color::from_rgba(255, 90, 60, 255));
        } else if sim.fuel <= 0.0 {
            // Shown the moment the tank empties; FUEL_OUT_END_SECS later the
            // run ends (game over) — the last coast still counts.
            let msg = "OUT OF FUEL";
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
        } else if landed && (sim.fuel < FUEL_MAX || sim.hull < HULL_MAX) {
            let msg = if sim.fuel < FUEL_MAX { "REFUELING" } else { "REPAIRING" };
            let fs = 36.0 * ui;
            let dims = measure_text(msg, None, fs as u16, 1.0);
            draw_text(msg, (sw - dims.width) / 2.0, sh * 0.38, fs,
                Color::from_rgba(120, 220, 160, 200));
        }

        // --- Particle emission ---
        // (Forces, fuel burn and the heading controller all run per tick in
        // the sim; this section is purely cosmetic.)
        //
        // Cosmetic clock: during replay playback particles advance in SIM
        // time — scaled by the playback rate, frozen while paused — because
        // particle velocities are world-space: with the camera pinned to a
        // frozen/slowed ship, wall-clock particles inherit the ship's world
        // velocity and the exhaust plume streams AHEAD of it (seen live as
        // "thrust goes forward" when pausing mid-burn). Emission below is
        // gated on dt > 0 so a paused frame doesn't pile particles at the
        // nozzle.
        let dt = if mode == Mode::Replay {
            let cosmetic = get_frame_time().min(0.05)
                * f32::from_bits(REPLAY_SPEED.load(Ordering::Relaxed));
            if replay_paused_now {
                // The ending grace: debris/plume keep animating briefly
                // after the finish auto-pause (see the replay_boom_timer
                // note above) — in COSMETIC time, so the explosion respects
                // the playback speed — then time truly stops.
                if replay_boom_timer > 0.0 { cosmetic } else { 0.0 }
            } else {
                cosmetic
            }
        } else {
            get_frame_time()
        };
        // Counts down in the same clock the particles advance by, so a ¼×
        // ending gets its full slow-motion play-out.
        replay_boom_timer = (replay_boom_timer - dt).max(0.0);
        // Emission needs cosmetic time AND live playback — during the ending
        // grace the frozen ship must not keep spraying exhaust.
        let emit_cosmetics = dt > 0.0 && !replay_paused_now;

        // Which RCS nozzle is puffing: live command + the sim's last applied
        // heading torque while flying, the recorded side during playback.
        let (puff_left, puff_right) = if let Some(f) = replay_frame {
            (f.rcs < 0, f.rcs > 0)
        } else {
            let rcs_live = sim.fuel > 0.0 && mode == Mode::Flying && !crashed;
            (rcs_live && (input.rot < 0 || frame_heading_torque < -0.4),
             rcs_live && (input.rot > 0 || frame_heading_torque > 0.4))
        };

        // Main thruster: exhaust exits local -Y (out the bottom), up to 8
        // particles/frame — count and exhaust speed scale with the throttle.
        // During playback the recorded glow stands in for the throttle so the
        // replayed burn trails exhaust too (fuel burn above stays live-only).
        let exhaust = match replay_frame {
            Some(f) if f.glow > 0.05 => f.glow,
            Some(_) => 0.0,
            None => throttle_fx,
        };
        if emit_cosmetics && exhaust > 0.0 {
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
        if emit_cosmetics && puff_left {
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
        if emit_cosmetics && puff_right {
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
        // Respawn returns to the ORIGINAL spawn (not RESET_X): every run
        // starts from the same place, which is what lets the ghost race you.
        if is_key_pressed(KeyCode::R) || PAD_RESET.swap(0, Ordering::Relaxed) != 0 || ui_do_reset {
            // FRESH Sim per run — never reuse the world across recorded runs.
            // Rapier's contact solve depends on collider handle numbering: a
            // reused sim's handle space carries the previous run's history,
            // while a replay's sim is fresh, and under sustained multi-point
            // contact (parked on a pad) the differing float summation order
            // diverges → chaos amplifies → metres of replay drift (found
            // 2026-07 from a real downloaded replay). A fresh sim makes live
            // and resim identical operation sequences by construction.
            // Runs ended by a manual reset while alive go to the highscore
            // store too ("longest flights", not "longest crashes") — a
            // crashed run was already published at the impact.
            let ended_alive = !sim.crashed && !run_over;
            let ended_dist = sim.max_dist;
            let (ended_fuel, ended_hull) = (sim.fuel, sim.hull);
            sim = Sim::new(sim.level.clone());
            // Snap the interpolation too, or the camera lerps across the
            // teleport for a frame.
            prev_ship = (SPAWN_X, sim.level.stand_y(SPAWN_X), 0.0);
            crash_timer = 0.0;
            shake = 0.0;
            mode = Mode::Flying;
            run_started = false;
            run_over = false;
            let ended = std::mem::replace(
                &mut recorder,
                Recording::new(sim_params(), sim.level.to_params(),
                    (HYBRID_MAX_SECS / PHYSICS_DT) as u32),
            );
            if ended_alive {
                // A crashed or fuel-out run was already published (both
                // channels) when it ended — only alive resets report here.
                report_run_end(&ended, ended_dist);
                report_run_analytics(1, ended.ticks(), ended_dist, ended_fuel, ended_hull);
            }
            // The ghost re-simulates the BEST run (the global record,
            // pushed from JS) from its first keyframe, in lockstep with
            // the new run.
            ghost_player = if GHOST_ON.load(Ordering::Relaxed) != 0 {
                ghost_rec.as_ref().and_then(ResimPlayer::new)
            } else {
                None
            };
            replay_player = None;
            watch_rec = None;
            glow = 0.0;
            recorder.push_keyframe(sim.keyframe(0, 0.0));
        }

        // --- Minimap + gauges (top-left) ---
        // Re-bound after the dialog/reset mutations above; during playback
        // this is the scratch sim (map follows the re-simmed run).
        let world_sim = replay_player.as_ref().map_or(&sim, |p| &p.sim);
        let mm_w = 480.0f32 * ui;
        let mm_h = 160.0f32 * ui;
        let mm_ox = safe_left + 10.0f32 * ui;
        let mm_oy = safe_top + 10.0f32 * ui;
        let mm_dark = Color::from_rgba(8, 8, 18, 220);

        // Minimap — always on (ship centred; pans in BOTH axes). A scoped
        // block so its helper locals (to_mm_x/y, MM_SHAFT_STEPS, …) don't leak
        // into the rest of the frame. The fuel & hull gauges are drawn just
        // below it; only the telemetry TEXT line is gated on the Debug HUD.
        {
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
                let c  = world_sim.level.cave_center(x);
                let hw = world_sim.level.cave_half_width(x);
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
                if !world_sim.level.shafts {
                    break;
                }
                let lv = &world_sim.level;
                let o = lv.shaft_open_seg(s);
                let (xl, xr) = (o as f32 * SEG_LEN, (o + SHAFT_OPEN_SEGS) as f32 * SEG_LEN);
                if xr < cam_x - MM_HALF_X - 2.0 || xl > cam_x + MM_HALF_X + 2.0 {
                    continue;
                }
                // Per-side junction offsets within a layer (same as shaft_wall_pts).
                let (jbl, jtl) = (lv.cave_center(xl) + lv.cave_half_width(xl), lv.cave_center(xl) - lv.cave_half_width(xl));
                let (jbr, jtr) = (lv.cave_center(xr) + lv.cave_half_width(xr), lv.cave_center(xr) - lv.cave_half_width(xr));
                for gap in gap_lo..=gap_hi {
                    let (gy0, gy1) = (gap as f32 * V_PERIOD, (gap + 1) as f32 * V_PERIOD);
                    let mm_pt = |side: u8, t: f32| -> Vec2 {
                        let (y0, y1) = if side == 0 { (gy0 + jbl, gy1 + jtl) } else { (gy0 + jbr, gy1 + jtr) };
                        vec2(
                            to_mm_x(world_sim.level.shaft_wall_x(s, side, t)).clamp(mm_ox, mm_ox + mm_w),
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
            for (&pad_key, pad) in world_sim.pads.iter() {
                if (pad.cx - cam_x).abs() > MM_HALF_X + 5.0 || (pad.y - cam_y).abs() > MM_HALF_Y + 5.0 {
                    continue;
                }
                let y = to_mm_y(pad.y).clamp(mm_oy, mm_oy + mm_h);
                let x0 = to_mm_x(pad.cx - PAD_HALF_W).clamp(mm_ox, mm_ox + mm_w);
                let x1 = to_mm_x(pad.cx + PAD_HALF_W).clamp(mm_ox, mm_ox + mm_w);
                let c = if world_sim.visited_pads.contains(&pad_key) {
                    Color::from_rgba(110, 140, 200, 220)
                } else {
                    Color::from_rgba(90, 240, 130, 255)
                };
                draw_line(x0, y, x1, y, 2.0 * dpi, c);
            }

            // Obstacle shapes on the minimap — actual polygon, not just a dot.
            // All loaded layers; the y window filters to what's actually in view.
            for ob in world_sim.obstacles.values() {
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

            // Ghost dot — the last run's position, when it's inside the window.
            if let Some((gx, gy, _)) = ghost_pose
                && (gx - cam_x).abs() < MM_HALF_X
                && (gy - cam_y).abs() < MM_HALF_Y
            {
                draw_circle(to_mm_x(gx), to_mm_y(gy), 2.5 * ui,
                    Color::from_rgba(150, 190, 255, 180));
            }

            // Border
            draw_rectangle_lines(mm_ox, mm_oy, mm_w, mm_h, 1.0, Color::from_rgba(255, 255, 255, 120));
        }

        // Prominent distance / score readout text + sizing, computed here (but
        // drawn further below, once the gauges are laid out) so `small_fs` —
        // the "BEST …" size — is available to the gauge percentage labels too.
        let (big, small) = match world_sim.level.scoring {
            Scoring::Distance => (
                format!("{:.0} m", world_sim.max_dist),
                format!("BEST {:.0} m", get_best_dist()),
            ),
            Scoring::Pads => (format!("{}", world_sim.score), "SCORE".to_string()),
        };
        // Left-aligned to the HUD column's left edge (`mm_ox`) plus a margin,
        // shrunk to the minimap width so a long number stays inside the column.
        let ro_margin = 20.0 * ui;
        let ro_x = mm_ox + ro_margin;
        let mut big_fs = 100.0 * ui;
        let big_dim = measure_text(&big, None, big_fs as u16, 1.0);
        if big_dim.width > mm_w - ro_margin {
            big_fs *= (mm_w - ro_margin) / big_dim.width;
        }
        let small_fs = big_fs * 0.36; // gauge percentages match this size

        // Fuel & hull gauges — two slim bars directly under the minimap, each
        // with a little vector icon to its left (a fuel drop, a heart) and a
        // bold percentage readout BETWEEN the icon and the bar (same size as
        // the "BEST" label). Distinct identities: fuel = warm amber (→ red
        // when low); hull = classic health green → amber → red.
        let gw = mm_w;
        let fg_h = 18.0 * ui;
        let ir = 14.0 * ui; // icon radius (big, unmissable)
        let bar_gap = 12.0 * ui; // space between the two bars
        let white120 = Color::from_rgba(255, 255, 255, 120);
        // Fuel drop: pointed top, rounded bottom.
        let draw_drop = |cx: f32, cy: f32, col: Color| {
            draw_triangle(vec2(cx, cy - ir), vec2(cx - ir * 0.72, cy + ir * 0.12),
                vec2(cx + ir * 0.72, cy + ir * 0.12), col);
            draw_circle(cx, cy + ir * 0.28, ir * 0.72, col);
        };
        // Heart: two lobes + a downward point.
        let draw_heart = |cx: f32, cy: f32, col: Color| {
            draw_circle(cx - ir * 0.42, cy - ir * 0.22, ir * 0.5, col);
            draw_circle(cx + ir * 0.42, cy - ir * 0.22, ir * 0.5, col);
            draw_triangle(vec2(cx - ir * 0.9, cy), vec2(cx + ir * 0.9, cy),
                vec2(cx, cy + ir * 0.9), col);
        };

        // Icon column, then the percentage (reserved to the widest case —
        // "100%" — so the bar starts at the same x for both gauges regardless
        // of the actual value), then the bar fills the rest of the column.
        let icon_cx = mm_ox + ir + 6.0 * ui;
        let pct_x = icon_cx + ir + 10.0 * ui;
        let pct_col = |frac: f32| -> Color {
            if frac > 0.5 {
                Color::from_rgba(120, 230, 140, 255)
            } else if frac > 0.25 {
                Color::from_rgba(255, 210, 70, 255)
            } else {
                Color::from_rgba(255, 70, 60, 255)
            }
        };
        let pct_max_w = measure_text("100%", None, small_fs as u16, 1.0).width;
        let bar_x = pct_x + pct_max_w + 10.0 * ui;
        let bar_w = gw - (bar_x - mm_ox);
        // Bold look via an 8-direction dark outline (scaled with `ui` so it
        // stays proportional at any resolution) filling in the strokes, then
        // the colored glyph on top — legible over the cave/space background,
        // not just over a bar. (An earlier version faked bold by drawing a
        // second copy offset sideways; at this outline weight it just read as
        // a smeared extra digit, so the outline alone carries the boldness.)
        let draw_pct = |frac: f32, bar_y: f32| {
            let label = format!("{}%", (frac * 100.0).round() as i32);
            // Exact vertical centering on the bar's mid-line: draw_text(x, Y)
            // rasterizes into Rect::new(x, Y - offset_y, w, h) (macroquad
            // TextDimensions docs), so solve Y from that rect's vertical
            // centre = the bar's centre — no font-metric guesswork.
            let dim = measure_text(&label, None, small_fs as u16, 1.0);
            let ty = bar_y + fg_h * 0.5 + dim.offset_y - dim.height * 0.5;
            let col = pct_col(frac);
            let sh = Color::from_rgba(0, 0, 0, 220);
            let o = 1.6 * ui;
            for dx in [-o, 0.0, o] {
                for dy in [-o, 0.0, o] {
                    if dx != 0.0 || dy != 0.0 {
                        draw_text(&label, pct_x + dx, ty + dy, small_fs, sh);
                    }
                }
            }
            draw_text(&label, pct_x, ty, small_fs, col);
        };

        // Fuel gauge (warm amber identity, red when low).
        let fg_y = mm_oy + mm_h + 8.0 * ui;
        let frac = world_sim.fuel / FUEL_MAX;
        let fg_col = if frac > 0.5 {
            Color::from_rgba(250, 190, 70, 255)
        } else if frac > 0.25 {
            Color::from_rgba(232, 150, 45, 255)
        } else {
            Color::from_rgba(225, 70, 45, 255)
        };
        draw_rectangle(bar_x, fg_y, bar_w, fg_h, mm_dark);
        draw_rectangle(bar_x, fg_y, bar_w * frac, fg_h, fg_col);
        draw_rectangle_lines(bar_x, fg_y, bar_w, fg_h, 1.0, white120);
        draw_drop(icon_cx, fg_y + fg_h * 0.5, fg_col);
        draw_pct(frac, fg_y);

        // Hull gauge — red health identity (heart + bar); brighter red when
        // critically low. The bar length still shows how much is left.
        let hg_y = fg_y + fg_h + bar_gap;
        let hfrac = world_sim.hull / HULL_MAX;
        let hg_col = if hfrac > 0.25 {
            Color::from_rgba(220, 65, 55, 255)
        } else {
            Color::from_rgba(255, 45, 35, 255)
        };
        draw_rectangle(bar_x, hg_y, bar_w, fg_h, mm_dark);
        draw_rectangle(bar_x, hg_y, bar_w * hfrac, fg_h, hg_col);
        draw_rectangle_lines(bar_x, hg_y, bar_w, fg_h, 1.0, white120);
        draw_heart(icon_cx, hg_y + fg_h * 0.5, hg_col);
        draw_pct(hfrac, hg_y);

        // Prominent distance / score readout — directly BELOW the gauges,
        // left-aligned under the minimap column (see sizing above).
        let ro_y = hg_y + fg_h + 14.0 * ui + big_fs;
        draw_text(&big, ro_x, ro_y, big_fs, WHITE);
        draw_text(&small, ro_x, ro_y + small_fs + 4.0 * ui,
            small_fs, Color::from_rgba(150, 180, 220, 220));
        // Record attribution under the BEST line ("by <pilot>" — or "by you"
        // once the record falls); empty name = no line (offline, pads).
        if world_sim.level.scoring == Scoring::Distance {
            let name = BEST_NAME.lock().unwrap();
            if !name.is_empty() {
                // Names render uppercase everywhere (boards, picker, ghost).
                let by = format!("by {}", name.to_uppercase());
                let mut by_fs = small_fs * 0.78;
                let by_dim = measure_text(&by, None, by_fs as u16, 1.0);
                if by_dim.width > mm_w - ro_margin {
                    by_fs *= (mm_w - ro_margin) / by_dim.width;
                }
                draw_text(&by, ro_x, ro_y + small_fs + 4.0 * ui + by_fs + 4.0 * ui,
                    by_fs, Color::from_rgba(130, 155, 190, 200));
            }
        }

        next_frame().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_analytics_publishes_payload_and_skips_zero_tick_runs() {
        let seq0 = run_end_seq();
        // A reset from the armed-idle state ended nothing — no publish.
        report_run_analytics(1, 0, 5.0, 50.0, 50.0);
        assert_eq!(run_end_seq(), seq0);
        // A real run publishes the full payload before bumping the seq.
        report_run_analytics(1, 240, 123.5, 61.25, 88.0);
        assert_eq!(run_end_seq(), seq0 + 1);
        assert_eq!(run_end_cause(), 1);
        assert_eq!(run_end_ticks(), 240);
        assert_eq!(run_end_dist(), 123.5);
        assert_eq!(run_end_fuel(), 61.25);
        assert_eq!(run_end_hull(), 88.0);
    }

    // The whole world is pure functions of (level, position/slot index).
    // These tests pin the invariants the rendering and collision code rely
    // on, evaluated on the DEMO level (seed 0 = the legacy cave, shafts and
    // boulders on) — the wrappers below keep the call sites readable. Local
    // fns shadow the glob-imported names, so e.g. `lattice_point` here is
    // the demo-level curried form of render::lattice_point.
    fn lvl() -> Level { Level::demo() }
    fn cave_center(x: f32) -> f32 { lvl().cave_center(x) }
    fn cave_half_width(x: f32) -> f32 { lvl().cave_half_width(x) }
    fn stand_y(x: f32) -> f32 { lvl().stand_y(x) }
    fn shaft_open_seg(s: i64) -> i64 { lvl().shaft_open_seg(s) }
    fn seg_in_opening(idx: i64) -> bool { lvl().seg_in_opening(idx) }
    fn shaft_wall_x(s: i64, side: u8, t: f32) -> f32 { lvl().shaft_wall_x(s, side, t) }
    fn obstacle_spec(k: i64) -> Option<ObstacleSpec> { lvl().obstacle_spec(k) }
    fn pad_spec(p: i64) -> Option<PadSpec> { lvl().pad_spec(p) }
    fn lattice_point(col: i64, row: usize, side: u8) -> Vec2 {
        render::lattice_point(&lvl(), col, row, side)
    }

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
    fn resim_player_replays_a_recording_to_the_end_without_drift() {
        // Record a short flight directly through the sim (burn, then coast),
        // then drive the playback player at a fake 60 Hz frame clock: it
        // must reach the recording's end with zero keyframe drift and no
        // fallback snaps on this binary.
        let mut s = Sim::new(lvl());
        let mut rec = Recording::new(sim_params(), lvl().to_params(), u32::MAX);
        rec.push_keyframe(s.keyframe(0, 0.0));
        for t in 0..600u32 {
            let input = if t < 150 {
                InputState::from_controls(0.8, 0, 0.0, 0.0, false)
            } else {
                InputState::default()
            };
            let rep = s.tick(input);
            let due = rec.record_tick(input);
            if let Some(imp) = rep.impact.filter(|i| i.destroyed) {
                rec.finalize(Keyframe {
                    tick: rec.ticks(),
                    x: imp.x, y: imp.y, rot_re: imp.rot_re, rot_im: imp.rot_im,
                    vx: imp.vx, vy: imp.vy, angvel: imp.angvel,
                    fuel: s.fuel, hull: s.hull, glow: 0.0,
                    land_timer: 0.0,
                });
                break;
            }
            if due {
                rec.push_keyframe(s.keyframe(rec.ticks(), 0.0));
            }
        }
        let mut p = ResimPlayer::new(&rec).expect("player from recording");
        let mut guard = 0;
        while !p.finished && guard < 10_000 {
            p.advance(&rec, 1.0 / 60.0);
            guard += 1;
        }
        assert!(p.finished, "player never finished");
        assert_eq!(p.tick, rec.ticks());
        assert!(p.drift < 1e-4, "keyframe drift {} m", p.drift);
        assert!(!p.snapped, "fallback snap engaged on the same binary");
    }

    // DIAGNOSTIC: faithfully mimic the LIVE loop — input computed once per
    // frame, a variable number of physics ticks per frame via the accumulator
    // (with the 0.05 s cap), each tick recorded — then play the recording back
    // through ResimPlayer at a DIFFERENT variable frame clock. Reproduces the
    // real replay conditions the simpler tests skip.
    #[test]
    fn frame_batched_recording_replays_without_snapping() {
        // A wandering flight that keeps steering (lots of heading commands).
        let script = |tick: u32| -> InputState {
            let f = tick as f32 * 0.02;
            let sx = (f.sin()) * 0.8;
            let sy = -(f * 0.7).cos() * 0.6;
            InputState::from_controls(0.7, 0, sx, sy, true)
        };
        let mut sim = Sim::new(lvl());
        let mut rec = Recording::new(sim_params(), lvl().to_params(), u32::MAX);
        rec.push_keyframe(sim.keyframe(0, 0.0));
        // Variable frame times cycling 40–90 fps, driving the accumulator.
        let frame_dts = [0.011f32, 0.025, 0.016, 0.009, 0.05, 0.02, 0.014];
        let mut accum = 0.0f32;
        let mut done_ticks = 0u32;
        'frames: for fi in 0..2000usize {
            accum = (accum + frame_dts[fi % frame_dts.len()]).min(0.05);
            // Input generated ONCE per frame, from the current tick count.
            let input = script(done_ticks);
            while accum >= PHYSICS_DT {
                let rep = sim.tick(input);
                accum -= PHYSICS_DT;
                let due = rec.record_tick(input);
                done_ticks += 1;
                if rep.impact.filter(|i| i.destroyed).is_some() {
                    rec.finalize(sim.keyframe(rec.ticks(), input.throttle_f32()));
                    break 'frames;
                }
                if due {
                    rec.push_keyframe(sim.keyframe(rec.ticks(), input.throttle_f32()));
                }
                if done_ticks >= 10 * KEYFRAME_EVERY { break 'frames; }
            }
        }
        // Play back at a different variable frame clock.
        let mut p = ResimPlayer::new(&rec).expect("player");
        let play_dts = [0.016f32, 0.033, 0.008, 0.02, 0.045];
        let mut pj = 0usize;
        let mut guard = 0;
        while !p.finished && guard < 100_000 {
            p.advance(&rec, play_dts[pj % play_dts.len()]);
            pj += 1; guard += 1;
        }
        assert!(p.finished, "player never finished (tick {} / {})", p.tick, rec.ticks());
        assert!(p.drift < 1e-3, "keyframe drift {} m", p.drift);
        assert!(!p.snapped, "SNAP engaged — recording diverges from replay");
    }

    #[test]
    fn fast_forward_steps_many_ticks_per_frame() {
        // 4× playback on a 60 Hz display feeds ~0.067 s per frame (0.2 s
        // worst case after the caller's 0.05 s raw-frame clamp) — advance()'s
        // hitch cap must pass deliberate fast-forward through, not clip it
        // back to realtime.
        let mut s = Sim::new(lvl());
        let mut rec = Recording::new(sim_params(), lvl().to_params(), u32::MAX);
        rec.push_keyframe(s.keyframe(0, 0.0));
        let burn = InputState::from_controls(0.8, 0, 0.0, 0.0, false);
        for _ in 0..KEYFRAME_EVERY {
            s.tick(burn);
            if rec.record_tick(burn) {
                rec.push_keyframe(s.keyframe(rec.ticks(), 0.0));
            }
        }
        let mut p = ResimPlayer::new(&rec).expect("player");
        p.advance(&rec, 0.05 * 4.0); // one worst-case frame at 4×
        // 0.2 s ≈ 24 ticks (float residue can leave the last one pending);
        // the old 0.05 s cap would have clipped this to 6.
        assert!(p.tick >= 23, "fast-forward clipped: {} ticks", p.tick);
    }

    // A recorded CONTACT-FREE flight (climb off the pad, then hover with a
    // gentle steer wobble; the guards prove it never even scrapes): the
    // shared fixture for the transport-seek tests, where zero drift is only
    // provable without contact — contact solving depends on Rapier
    // warm-start caches and handle numbering, which a keyframe can't carry
    // (that case is what the 0.5 m snap fallback absorbs).
    fn contact_free_recording() -> Recording {
        let script = |tick: u32| -> InputState {
            if tick < 60 {
                return InputState::from_controls(0.6, 0, 0.0, 0.0, false); // lift off
            }
            // Hover (TWR ≈ 7.5 → hover throttle ≈ 0.13) with a small steer
            // wobble so the PD heading controller stays busy.
            let throttle = if (tick / 90).is_multiple_of(2) { 0.16 } else { 0.10 };
            let f = tick as f32 * 0.01;
            InputState::from_controls(throttle, 0, f.sin() * 0.15, -0.9, true)
        };
        let mut sim = Sim::new(lvl());
        let mut rec = Recording::new(sim_params(), lvl().to_params(), u32::MAX);
        rec.push_keyframe(sim.keyframe(0, 0.0));
        for t in 0..6 * KEYFRAME_EVERY {
            let input = script(t);
            let rep = sim.tick(input);
            let due = rec.record_tick(input);
            assert!(rep.impact.is_none(), "test flight hit rock at tick {t}");
            if due {
                rec.push_keyframe(sim.keyframe(rec.ticks(), input.throttle_f32()));
            }
        }
        assert_eq!(sim.hull, HULL_MAX, "test flight scraped — not contact-free");
        assert!(rec.keyframes.len() >= 5, "flight too short: {} keyframes", rec.keyframes.len());
        rec
    }

    // Bit-compare every physics field of two sim states (via keyframes;
    // glow is render-side and passed as 0 for both).
    fn assert_state_bits_eq(a: &Keyframe, b: &Keyframe, what: &str) {
        for (fa, fb, name) in [
            (a.x, b.x, "x"), (a.y, b.y, "y"),
            (a.rot_re, b.rot_re, "rot_re"), (a.rot_im, b.rot_im, "rot_im"),
            (a.vx, b.vx, "vx"), (a.vy, b.vy, "vy"),
            (a.angvel, b.angvel, "angvel"), (a.fuel, b.fuel, "fuel"),
            (a.hull, b.hull, "hull"), (a.land_timer, b.land_timer, "land_timer"),
        ] {
            assert_eq!(fa.to_bits(), fb.to_bits(), "{name} differs after {what}");
        }
    }

    #[test]
    fn tick_stepping_matches_continuous_playback_bit_exactly() {
        // Frame-level transport: seeking to an arbitrary TICK must land on
        // exactly the state continuous playback reaches there. Forward it
        // restores the keyframe before the target and re-sims the remainder
        // (or just steps when already inside the interval); one tick BACK
        // does the rebuild + re-sim dance. All bit-exact on an airborne
        // flight.
        let rec = contact_free_recording();
        let t_mid = KEYFRAME_EVERY + 45; // mid-interval, not on a keyframe

        // Continuous reference: a player stepped straight to t_mid.
        let mut reference = ResimPlayer::new(&rec).expect("player");
        while reference.tick < t_mid {
            reference.step_one(&rec);
        }

        // Forward tick-seek from a fresh player.
        let mut p = ResimPlayer::new(&rec).expect("player");
        p.seek_to_tick(&rec, t_mid);
        assert_eq!(p.tick, t_mid);
        assert_state_bits_eq(
            &p.sim.keyframe(0, 0.0),
            &reference.sim.keyframe(0, 0.0),
            "a forward tick-seek",
        );

        // One tick back from t_mid, vs a reference stepped to t_mid - 1.
        let mut back_ref = ResimPlayer::new(&rec).expect("player");
        while back_ref.tick < t_mid - 1 {
            back_ref.step_one(&rec);
        }
        p.seek_to_tick(&rec, t_mid - 1);
        assert_eq!(p.tick, t_mid - 1);
        assert_state_bits_eq(
            &p.sim.keyframe(0, 0.0),
            &back_ref.sim.keyframe(0, 0.0),
            "a single-tick back-step",
        );

        // Stepping forward from mid-interval continues the SAME sim (no
        // rebuild): still identical to the continuous reference.
        p.seek_to_tick(&rec, t_mid + 7);
        while reference.tick < t_mid + 7 {
            reference.step_one(&rec);
        }
        assert_state_bits_eq(
            &p.sim.keyframe(0, 0.0),
            &reference.sim.keyframe(0, 0.0),
            "an in-interval forward step",
        );
    }

    #[test]
    fn seeking_to_a_keyframe_resumes_bit_exactly() {
        // Scrub: a fresh player seeks FORWARD to keyframe 3, and a second
        // plays partway and seeks BACK to keyframe 1. v3 keyframes carry the
        // exact rotation + land timer, so both must re-sim the airborne
        // remainder with ZERO keyframe drift and never engage the snap
        // fallback.
        let rec = contact_free_recording();

        // Forward scrub from a fresh player.
        let mut p = ResimPlayer::new(&rec).expect("player");
        p.seek_to_keyframe(&rec, 3);
        assert_eq!(p.tick, rec.keyframes[3].tick);
        while !p.finished {
            p.step_one(&rec);
        }
        assert_eq!(p.tick, rec.ticks());
        assert_eq!(p.drift, 0.0, "forward-seek drift");
        assert!(!p.snapped, "fallback snap engaged after a forward seek");

        // Backward scrub after playing partway past keyframe 2.
        let mut p = ResimPlayer::new(&rec).expect("player");
        for _ in 0..(2 * KEYFRAME_EVERY + 40) {
            p.step_one(&rec);
        }
        p.seek_to_keyframe(&rec, 1);
        assert_eq!(p.tick, rec.keyframes[1].tick);
        while !p.finished {
            p.step_one(&rec);
        }
        assert_eq!(p.drift, 0.0, "backward-seek drift");
        assert!(!p.snapped, "fallback snap engaged after a backward seek");

        // Seeking past the end clamps to the last keyframe that still has
        // ticks to play — a scrub to the bar's far right shows the finale
        // instead of doing nothing.
        let mut p = ResimPlayer::new(&rec).expect("player");
        p.seek_to_keyframe(&rec, rec.keyframes.len() + 5);
        assert!(p.tick < rec.ticks());
        assert!(!p.finished);
        while !p.finished {
            p.step_one(&rec);
        }
        assert_eq!(p.tick, rec.ticks());
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

    // --- Level system -------------------------------------------------------

    #[test]
    fn shipped_level_files_parse_to_the_intended_worlds() {
        // include_str! pins the ACTUAL data files that deploy, so a typo in
        // levels/*.level fails the suite instead of silently falling back to
        // demo defaults.
        let caves = Level::parse(include_str!("../levels/caves.level"));
        // Distance-scored since 2026-07 like the other shipped levels (the
        // compiled-in demo default stays pads for the pad-scoring tests).
        assert_eq!(caves, Level {
            name: "The Caves".to_string(),
            scoring: Scoring::Distance,
            ..Level::demo()
        });

        let expanse = Level::parse(include_str!("../levels/expanse.level"));
        assert_eq!(expanse.name, "The Expanse");
        assert_eq!(expanse.scoring, Scoring::Distance);
        assert!(!expanse.shafts);
        assert!(expanse.obstacles);
        assert_eq!(expanse.seed, 0, "expanse must reuse the legacy cave shape");

        let glide = Level::parse(include_str!("../levels/glide.level"));
        assert_eq!(glide.name, "The Glide");
        assert_eq!(glide.scoring, Scoring::Distance);
        assert!(!glide.shafts);
        assert!(!glide.obstacles);
    }

    #[test]
    fn shipped_levels_map_stays_in_sync_with_the_manifest() {
        // The backend verifier params-checks submissions against
        // world::shipped_levels() — a level listed in the manifest but
        // missing there silently loses that check (unknown stems are
        // accepted un-params-checked by design).
        let manifest = include_str!("../levels/manifest.json");
        let shipped = pegasus_sim::world::shipped_levels();
        for (stem, lvl) in &shipped {
            assert!(
                manifest.contains(&format!("\"{stem}.level\"")),
                "shipped_levels entry {stem} is not in levels/manifest.json"
            );
            assert!(!lvl.name.is_empty());
        }
        assert_eq!(
            manifest.matches(".level").count(),
            shipped.len(),
            "levels/manifest.json and world::shipped_levels() are out of sync"
        );
    }

    #[test]
    fn level_parse_is_forgiving_and_clamps() {
        // Unknown keys and junk lines are ignored (forward compatibility),
        // missing keys keep demo defaults, pad_spacing clamps to a sane band.
        let l = Level::parse("name = X\nfuture_knob = 12\n# comment\nnot a kv line\npad_spacing = 5");
        assert_eq!(l.name, "X");
        assert_eq!(l.pad_spacing, 40.0);
        assert!(l.shafts && l.obstacles);
        assert_eq!(Level::parse(""), Level::demo());
    }

    #[test]
    fn level_params_round_trip_through_the_replay_header() {
        let lvl = Level::parse(include_str!("../levels/expanse.level"));
        let back = Level::from_params(&lvl.to_params());
        // Everything physics-relevant survives; only the cosmetic name doesn't.
        assert_eq!(Level { name: lvl.name.clone(), ..back }, lvl);
    }

    #[test]
    fn shaftless_level_seals_the_cave_and_keeps_refuel_pads() {
        let expanse = Level::parse(include_str!("../levels/expanse.level"));
        for idx in -400..400 {
            assert!(!expanse.seg_in_opening(idx), "wall opening at segment {idx}");
        }
        // No shaft colliders load either.
        let mut sim = Sim::new(expanse.clone());
        for _ in 0..120 {
            sim.tick(InputState::default());
        }
        assert!(sim.shafts.is_empty());
        // Refueling pads still appear regularly — at least as many slots
        // survive as on the demo level (the shaft-clearance skip is gone).
        let count = |l: &Level| (-20..=20).filter(|&p| l.pad_spec(p).is_some()).count();
        let (n_demo, n_exp) = (count(&lvl()), count(&expanse));
        assert!(n_exp >= n_demo && n_demo >= 10,
            "pads too sparse: demo {n_demo}, expanse {n_exp} of 41 slots");
    }

    #[test]
    fn boulderless_level_has_no_obstacles_but_keeps_pads() {
        let lvl = Level::parse(include_str!("../levels/glide.level"));
        for k in -300..300 {
            assert!(lvl.obstacle_spec(k).is_none(), "boulder at slot {k}");
        }
        let mut sim = Sim::new(lvl);
        for _ in 0..120 {
            sim.tick(InputState::default());
        }
        assert!(sim.obstacles.is_empty());
        assert!(!sim.pads.is_empty(), "pads must still load");
    }

    #[test]
    fn seed_reshapes_the_world_and_never_pinches_it_shut() {
        let demo = lvl();
        for seed in [1u32, 7, 123456] {
            let seeded = Level { seed, ..lvl() };
            assert_ne!(
                seeded.cave_center(100.0).to_bits(),
                demo.cave_center(100.0).to_bits(),
                "seed {seed} did not reshape the cave"
            );
            // The harmonic amplitudes guarantee half-width ≥ 2.5 for ANY
            // phases; pin the fairness floor against regressions.
            for i in 0..3000 {
                let x = i as f32 * 0.21;
                assert!(seeded.cave_half_width(x) > 1.0, "seed {seed} pinches at x={x}");
            }
        }
    }

    #[test]
    fn stored_highscore_blob_round_trips_into_a_watchable_replay() {
        // Fly a scripted run on The Glide, serialize + deflate it (exactly
        // the blob shape the backend stores and CloudFront serves), then
        // decode it the way the watch_replay_blob export does and re-play
        // it to the end — the decoded recording must carry its own level
        // and be fully playable.
        let level = Level::parse(include_str!("../levels/glide.level"));
        let mut sim = Sim::new(level.clone());
        let mut rec = Recording::new(sim_params(), level.to_params(), u32::MAX);
        rec.push_keyframe(sim.keyframe(0, 0.0));
        for t in 0..600u32 {
            let input = if t < 240 {
                InputState::from_controls(1.0, 0, 0.0, 0.0, false)
            } else {
                InputState::default()
            };
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
        let packed = compress(&rec.serialize(7));
        let decoded = decode_recording(&packed).expect("stored blob must decode");
        assert_eq!(decoded.level, level.to_params());
        assert_eq!(decoded.ticks(), rec.ticks());
        let mut p = ResimPlayer::new(&decoded).expect("decoded replay must be playable");
        while !p.finished {
            p.step_one(&decoded);
        }
        assert_eq!(p.tick, decoded.ticks());
        assert!(!p.snapped, "same-binary playback of a stored blob must not drift");

        // Corrupt data must be rejected, not panic.
        assert!(decode_recording(&packed[..packed.len() / 2]).is_none());
        assert!(decode_recording(b"garbage").is_none());
    }

    #[test]
    fn distance_level_pays_no_pad_points_and_tracks_max_dist() {
        // Park on the spawn pad past PAD_LAND_TIME: the visit registers (the
        // beacon turns blue) but the score stays 0 — on Distance levels the
        // score is max |x|, mirrored in Sim::max_dist.
        let lvl = Level::parse(include_str!("../levels/expanse.level"));
        let mut sim = Sim::new(lvl);
        for _ in 0..(2.0 / PHYSICS_DT) as u32 {
            sim.tick(InputState::default());
        }
        assert!(!sim.visited_pads.is_empty(), "pad visit never registered");
        assert_eq!(sim.score, 0, "distance level must not pay pad points");
        let (x, _, _) = sim.ship_pose();
        assert!(sim.max_dist >= x.abs());
    }
}
