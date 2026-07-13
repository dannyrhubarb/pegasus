// Hybrid replay recording: INPUTS + PARAMS + KEYFRAMES, covering the last
// spawn→crash run. This is the storage/transport format for replays that
// will leave the device (sharing, ghosts, leaderboards); the dense per-step
// visual buffer in main.rs remains the *playback* path until the
// deterministic re-sim refactor lands.
//
// - Input change-events at physics-tick resolution: the resolved control
//   values (throttle, rate command, stick vector) recorded only when they
//   change — near-constant inputs cost almost nothing.
// - State keyframes every KEYFRAME_EVERY ticks (1 Hz): full sim state for
//   drift detection, seeking, and graceful fallback playback when a replay
//   was recorded under different params / an older build.
// - A params header (every physics-affecting constant) + build id, so a
//   future re-sim can run the recording under the rules it was flown with
//   and detect when it can't.
//
// The recording is trimmed to a max window at keyframe boundaries; the
// effective input at the new start is re-seeded so the retained window is
// always replayable from its first keyframe.

// One keyframe per second of sim time (physics runs at 120 Hz).
pub const KEYFRAME_EVERY: u32 = 120;
pub const REPLAY_MAGIC: [u8; 4] = *b"PGRP";
// v3: keyframes carry the EXACT rotation (unit-complex re/im, not an angle —
// the atan2→Rotation::new round-trip cost sub-mm drift on keyframe restore)
// plus land_timer (pad-settle progress, it gates refuel/repair timing).
// No backward compatibility while the game is iterating: deserialize rejects
// other versions, so pre-v3 stored blobs simply stop decoding (decode returns
// None — high-score watch/ghost buttons no-op, nothing crashes).
pub const REPLAY_FORMAT_VERSION: u16 = 3;

// Resolved control values in effect for a physics step, quantized for
// storage. "Resolved" = after the input-combining logic (stick ramp gates,
// source priority), so a future re-sim replays exactly what drove the
// forces, not the raw device state.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct InputState {
    pub throttle: u8,   // main engine 0..255 = 0..1
    pub rot: i8,        // manual rate command: -1 = left, +1 = right
    pub steer_x: i8,    // touch-stick vector × 127 (screen convention)
    pub steer_y: i8,
    pub stick_held: u8, // 0/1 (drives the thrust-gating state machine)
}

impl InputState {
    // Quantize the frame's resolved controls. The LIVE sim consumes the
    // dequantized values of exactly this struct (not the raw floats), so a
    // resim of the recorded stream sees bit-identical inputs.
    pub fn from_controls(throttle: f32, rot: i8, steer_x: f32, steer_y: f32, held: bool) -> Self {
        InputState {
            throttle: (throttle.clamp(0.0, 1.0) * 255.0).round() as u8,
            rot,
            steer_x: (steer_x.clamp(-1.0, 1.0) * 127.0).round() as i8,
            steer_y: (steer_y.clamp(-1.0, 1.0) * 127.0).round() as i8,
            stick_held: held as u8,
        }
    }

    pub fn throttle_f32(&self) -> f32 {
        self.throttle as f32 / 255.0
    }

    // No command at all — not even a stick touch. The main loop holds a
    // freshly spawned run armed-but-idle until the first non-neutral input
    // (the run clock starts at the pilot's first action, not at spawn).
    pub fn is_neutral(&self) -> bool {
        *self == InputState::default()
    }

    pub fn steer_f32(&self) -> (f32, f32) {
        (self.steer_x as f32 / 127.0, self.steer_y as f32 / 127.0)
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct InputEvent {
    pub tick: u32, // input takes effect from this tick's step
    pub input: InputState,
}

// Full simulation state at a tick — enough to resume/verify a re-sim.
// The heading is stored as the body's exact unit-complex rotation (re, im),
// NOT an angle: restoring `Rotation::new(atan2(im, re))` doesn't reproduce
// the original bits, and that sub-mm seed compounds through the integrator
// (measured ~6e-4 m over 3 s of steered flight). With the raw components a
// keyframe restore is bit-exact.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Keyframe {
    pub tick: u32,
    pub x: f32,
    pub y: f32,
    pub rot_re: f32,
    pub rot_im: f32,
    pub vx: f32,
    pub vy: f32,
    pub angvel: f32,
    pub fuel: f32,
    pub hull: f32,
    pub glow: f32,
    pub land_timer: f32, // pad-settle progress — it gates refuel/repair timing
}

// Every constant that shapes the simulation, so a replay re-runs under the
// rules it was recorded with. Serialized in field order below — extending
// this struct means bumping REPLAY_FORMAT_VERSION.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SimParams {
    pub dt: f32,
    pub gravity_y: f32,
    pub thrust_force: f32,
    pub linear_damping: f32,
    pub angular_damping: f32,
    pub rcs_force: f32,
    pub heading_kp: f32,
    pub heading_kd: f32,
    pub heading_torque_max: f32,
    pub fuel_max: f32,
    pub fuel_burn_main: f32,
    pub fuel_burn_rcs: f32,
    pub crash_dv_soft: f32,
    pub crash_dv_hard: f32,
    pub hull_max: f32,
}

// The world-shaping half of the header: everything a Level contributes to
// physics (the cave/obstacle/pad generator parameters), so resim rebuilds
// the exact world the run was flown in. The level NAME is cosmetic and
// deliberately not part of this — two levels with equal params are the same
// world. Conversions to/from `world::Level` live in world.rs.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct LevelParams {
    pub scoring: u8, // 0 = pads, 1 = distance (display-only, kept for context)
    pub shafts: u8,
    pub obstacles: u8,
    pub pad_spacing: f32,
    pub seed: u32,
}

impl SimParams {
    const N_FIELDS: usize = 15;

    fn to_array(self) -> [f32; Self::N_FIELDS] {
        [
            self.dt, self.gravity_y, self.thrust_force, self.linear_damping,
            self.angular_damping, self.rcs_force, self.heading_kp, self.heading_kd,
            self.heading_torque_max, self.fuel_max, self.fuel_burn_main,
            self.fuel_burn_rcs, self.crash_dv_soft, self.crash_dv_hard, self.hull_max,
        ]
    }

    fn from_array(a: [f32; Self::N_FIELDS]) -> Self {
        SimParams {
            dt: a[0], gravity_y: a[1], thrust_force: a[2], linear_damping: a[3],
            angular_damping: a[4], rcs_force: a[5], heading_kp: a[6], heading_kd: a[7],
            heading_torque_max: a[8], fuel_max: a[9], fuel_burn_main: a[10],
            fuel_burn_rcs: a[11], crash_dv_soft: a[12], crash_dv_hard: a[13],
            hull_max: a[14],
        }
    }
}

pub struct Recording {
    pub params: SimParams,
    pub level: LevelParams,
    pub events: Vec<InputEvent>,
    pub keyframes: Vec<Keyframe>,
    ticks: u32,             // physics steps recorded so far
    last_input: Option<InputState>,
    max_ticks: u32,         // retention window (trimmed at keyframe boundaries)
}

impl Recording {
    pub fn new(params: SimParams, level: LevelParams, max_ticks: u32) -> Self {
        Recording {
            params,
            level,
            events: Vec::new(),
            keyframes: Vec::new(),
            ticks: 0,
            last_input: None,
            max_ticks,
        }
    }

    pub fn ticks(&self) -> u32 {
        self.ticks
    }

    // Record one physics step under `input`. Pushes an event only when the
    // input changed. Returns true when a keyframe is due (call push_keyframe
    // with the post-step state).
    pub fn record_tick(&mut self, input: InputState) -> bool {
        if self.last_input != Some(input) {
            self.events.push(InputEvent { tick: self.ticks, input });
            self.last_input = Some(input);
        }
        self.ticks += 1;
        self.ticks.is_multiple_of(KEYFRAME_EVERY)
    }

    pub fn push_keyframe(&mut self, kf: Keyframe) {
        self.keyframes.push(kf);
        self.trim();
    }

    // Terminal keyframe at the crash: lets a verifier check the final state
    // without simulating past the last periodic keyframe. Skipped if a
    // keyframe for this tick already exists.
    pub fn finalize(&mut self, kf: Keyframe) {
        if self.keyframes.last().map(|k| k.tick) != Some(kf.tick) {
            self.keyframes.push(kf);
        }
    }

    // Drop history older than max_ticks, cutting at a keyframe so the
    // retained window is replayable from its first keyframe. The input in
    // effect at the cut is re-seeded as an event at the cut tick.
    fn trim(&mut self) {
        let Some(&Keyframe { tick: newest, .. }) = self.keyframes.last() else { return };
        let cutoff = newest.saturating_sub(self.max_ticks);
        let start = match self.keyframes.iter().find(|k| k.tick >= cutoff) {
            Some(k) if k.tick > self.keyframes[0].tick => k.tick,
            _ => return, // window already starts at the first keyframe
        };
        self.keyframes.retain(|k| k.tick >= start);
        // Input in effect at the cut = last event at or before it.
        let effective = self
            .events
            .iter()
            .take_while(|e| e.tick <= start)
            .last()
            .map(|e| e.input);
        self.events.retain(|e| e.tick >= start);
        if let Some(input) = effective
            && self.events.first().map(|e| e.tick) != Some(start)
        {
            self.events.insert(0, InputEvent { tick: start, input });
        }
    }

    // --- Serialization (little-endian) ---
    // Header: magic(4) version(2) build_id(4) params(15×4)
    //         level: scoring(1) shafts(1) obstacles(1) pad_spacing(4) seed(4)
    //         ticks(4) n_events(4) n_keyframes(4)                 = 93 B
    // Event:  tick(4) throttle(1) rot(1) steer_x(1) steer_y(1) held(1) = 9 B
    // Keyframe: tick(4) + 11×f32                                      = 48 B

    pub fn serialize(&self, build_id: u32) -> Vec<u8> {
        let mut out = Vec::with_capacity(93 + self.events.len() * 9 + self.keyframes.len() * 48);
        out.extend_from_slice(&REPLAY_MAGIC);
        out.extend_from_slice(&REPLAY_FORMAT_VERSION.to_le_bytes());
        out.extend_from_slice(&build_id.to_le_bytes());
        for f in self.params.to_array() {
            out.extend_from_slice(&f.to_le_bytes());
        }
        out.push(self.level.scoring);
        out.push(self.level.shafts);
        out.push(self.level.obstacles);
        out.extend_from_slice(&self.level.pad_spacing.to_le_bytes());
        out.extend_from_slice(&self.level.seed.to_le_bytes());
        out.extend_from_slice(&self.ticks.to_le_bytes());
        out.extend_from_slice(&(self.events.len() as u32).to_le_bytes());
        out.extend_from_slice(&(self.keyframes.len() as u32).to_le_bytes());
        for e in &self.events {
            out.extend_from_slice(&e.tick.to_le_bytes());
            out.push(e.input.throttle);
            out.push(e.input.rot as u8);
            out.push(e.input.steer_x as u8);
            out.push(e.input.steer_y as u8);
            out.push(e.input.stick_held);
        }
        for k in &self.keyframes {
            out.extend_from_slice(&k.tick.to_le_bytes());
            for f in [k.x, k.y, k.rot_re, k.rot_im, k.vx, k.vy, k.angvel,
                      k.fuel, k.hull, k.glow, k.land_timer] {
                out.extend_from_slice(&f.to_le_bytes());
            }
        }
        out
    }

    pub fn deserialize(data: &[u8]) -> Result<(Recording, u32), &'static str> {
        let mut r = Reader { data, pos: 0 };
        if r.bytes(4)? != REPLAY_MAGIC {
            return Err("bad magic");
        }
        if r.u16()? != REPLAY_FORMAT_VERSION {
            return Err("unsupported version");
        }
        let build_id = r.u32()?;
        let mut a = [0f32; SimParams::N_FIELDS];
        for f in &mut a {
            *f = r.f32()?;
        }
        let params = SimParams::from_array(a);
        let level = LevelParams {
            scoring: r.u8()?,
            shafts: r.u8()?,
            obstacles: r.u8()?,
            pad_spacing: r.f32()?,
            seed: r.u32()?,
        };
        let ticks = r.u32()?;
        let n_events = r.u32()? as usize;
        let n_keyframes = r.u32()? as usize;
        // Never trust header counts for allocation: blobs cross trust
        // boundaries (the backend verifier parses player uploads), and a
        // hostile count of 4 billion would OOM on with_capacity before the
        // reads below ever failed with "truncated". Cap by what the
        // remaining bytes could actually hold.
        let mut events = Vec::with_capacity(n_events.min(r.remaining() / 9));
        for _ in 0..n_events {
            events.push(InputEvent {
                tick: r.u32()?,
                input: InputState {
                    throttle: r.u8()?,
                    rot: r.u8()? as i8,
                    steer_x: r.u8()? as i8,
                    steer_y: r.u8()? as i8,
                    stick_held: r.u8()?,
                },
            });
        }
        let mut keyframes = Vec::with_capacity(n_keyframes.min(r.remaining() / 48));
        for _ in 0..n_keyframes {
            let tick = r.u32()?;
            let mut f = [0f32; 11];
            for v in &mut f {
                *v = r.f32()?;
            }
            keyframes.push(Keyframe {
                tick,
                x: f[0], y: f[1], rot_re: f[2], rot_im: f[3], vx: f[4], vy: f[5],
                angvel: f[6], fuel: f[7], hull: f[8], glow: f[9], land_timer: f[10],
            });
        }
        let last_input = events.last().map(|e| e.input);
        Ok((
            Recording { params, level, events, keyframes, ticks, last_input, max_ticks: u32::MAX },
            build_id,
        ))
    }
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn bytes(&mut self, n: usize) -> Result<&'a [u8], &'static str> {
        let s = self.data.get(self.pos..self.pos + n).ok_or("truncated")?;
        self.pos += n;
        Ok(s)
    }
    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }
    fn u8(&mut self) -> Result<u8, &'static str> {
        Ok(self.bytes(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, &'static str> {
        Ok(u16::from_le_bytes(self.bytes(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32, &'static str> {
        Ok(u32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }
    fn f32(&mut self) -> Result<f32, &'static str> {
        Ok(f32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }
}

// Deflate, level 8 — the form replay blobs ship/store as (base64'd in the
// submit POST body; raw bytes from CloudFront).
pub fn compress(data: &[u8]) -> Vec<u8> {
    miniz_oxide::deflate::compress_to_vec(data, 8)
}

// Inverse of compress(); None on corrupt input (a mangled downloaded blob
// must not panic the game).
pub fn decompress(data: &[u8]) -> Option<Vec<u8>> {
    miniz_oxide::inflate::decompress_to_vec(data).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> SimParams {
        SimParams {
            dt: 1.0 / 120.0, gravity_y: -1.62, thrust_force: 8.0,
            linear_damping: 0.2, angular_damping: 3.0, rcs_force: 3.3,
            heading_kp: 14.0, heading_kd: 2.2, heading_torque_max: 6.0,
            fuel_max: 100.0, fuel_burn_main: 3.5, fuel_burn_rcs: 1.2,
            crash_dv_soft: 2.5, crash_dv_hard: 6.0, hull_max: 100.0,
        }
    }

    fn lparams() -> LevelParams {
        LevelParams { scoring: 0, shafts: 1, obstacles: 1, pad_spacing: 130.0, seed: 0 }
    }

    fn kf(tick: u32) -> Keyframe {
        Keyframe {
            tick,
            x: tick as f32 * 0.1, y: 5.0, rot_re: 0.955, rot_im: 0.296,
            vx: 1.0, vy: -0.5, angvel: 0.0, fuel: 90.0, hull: 100.0,
            glow: 0.7, land_timer: 0.25,
        }
    }

    #[test]
    fn neutral_means_no_command_at_all() {
        assert!(InputState::default().is_neutral());
        // Each command on its own must arm the run — including a bare stick
        // touch (held, dead-centre: no throttle yet, no steer vector).
        assert!(!InputState { throttle: 1, ..Default::default() }.is_neutral());
        assert!(!InputState { rot: -1, ..Default::default() }.is_neutral());
        assert!(!InputState { steer_x: 3, ..Default::default() }.is_neutral());
        assert!(!InputState { steer_y: -3, ..Default::default() }.is_neutral());
        assert!(!InputState { stick_held: 1, ..Default::default() }.is_neutral());
    }

    #[test]
    fn events_are_deduplicated() {
        let mut rec = Recording::new(params(), lparams(), u32::MAX);
        let idle = InputState::default();
        let burn = InputState { throttle: 255, ..Default::default() };
        for _ in 0..50 {
            rec.record_tick(idle);
        }
        for _ in 0..50 {
            rec.record_tick(burn);
        }
        for _ in 0..50 {
            rec.record_tick(idle);
        }
        assert_eq!(rec.ticks(), 150);
        let ticks: Vec<u32> = rec.events.iter().map(|e| e.tick).collect();
        assert_eq!(ticks, vec![0, 50, 100]);
    }

    #[test]
    fn keyframes_come_due_every_second() {
        let mut rec = Recording::new(params(), lparams(), u32::MAX);
        rec.push_keyframe(kf(0));
        let mut due_at = Vec::new();
        for _ in 0..KEYFRAME_EVERY * 3 {
            if rec.record_tick(InputState::default()) {
                due_at.push(rec.ticks());
                rec.push_keyframe(kf(rec.ticks()));
            }
        }
        assert_eq!(due_at, vec![KEYFRAME_EVERY, KEYFRAME_EVERY * 2, KEYFRAME_EVERY * 3]);
    }

    #[test]
    fn trim_keeps_a_replayable_window() {
        // Window of 2 s; record 5 s with an input change early on. After the
        // trim the window must start at a keyframe and carry a re-seeded
        // event stating the input in effect there.
        let burn = InputState { throttle: 255, ..Default::default() };
        let mut rec = Recording::new(params(), lparams(), 2 * KEYFRAME_EVERY);
        rec.push_keyframe(kf(0));
        for _ in 0..5 * KEYFRAME_EVERY {
            if rec.record_tick(burn) {
                rec.push_keyframe(kf(rec.ticks()));
            }
        }
        let first_kf = rec.keyframes[0].tick;
        assert_eq!(first_kf, 3 * KEYFRAME_EVERY); // 600 - 240 window
        assert_eq!(rec.events[0].tick, first_kf, "input not re-seeded at cut");
        assert_eq!(rec.events[0].input, burn);
        assert_eq!(rec.events.len(), 1);
    }

    #[test]
    fn finalize_skips_duplicate_tick() {
        let mut rec = Recording::new(params(), lparams(), u32::MAX);
        rec.push_keyframe(kf(0));
        rec.finalize(kf(0));
        assert_eq!(rec.keyframes.len(), 1);
        rec.finalize(kf(7));
        assert_eq!(rec.keyframes.len(), 2);
    }

    #[test]
    fn serialize_roundtrips() {
        let mut rec = Recording::new(params(), lparams(), u32::MAX);
        rec.push_keyframe(kf(0));
        let burn = InputState { throttle: 255, rot: -1, steer_x: 40, steer_y: -90, stick_held: 1 };
        for _ in 0..KEYFRAME_EVERY {
            if rec.record_tick(burn) {
                rec.push_keyframe(kf(rec.ticks()));
            }
        }
        let blob = rec.serialize(0xdeadbeef);
        assert_eq!(blob.len(), 93 + rec.events.len() * 9 + rec.keyframes.len() * 48);
        let (back, build_id) = Recording::deserialize(&blob).expect("deserialize");
        assert_eq!(build_id, 0xdeadbeef);
        assert_eq!(back.params, rec.params);
        assert_eq!(back.level, rec.level);
        assert_eq!(back.ticks(), rec.ticks());
        assert_eq!(back.events, rec.events);
        assert_eq!(back.keyframes, rec.keyframes);
    }

    #[test]
    fn hostile_header_counts_fail_without_allocating() {
        // A header claiming 4 billion events must fail with "truncated",
        // not OOM in Vec::with_capacity before the reads can fail — the
        // backend verifier feeds player uploads through this parser.
        let mut rec = Recording::new(params(), lparams(), u32::MAX);
        rec.push_keyframe(kf(0));
        let mut blob = rec.serialize(0);
        // n_events sits at header offset 85 (see the layout comment above
        // serialize): magic 4 + version 2 + build 4 + params 60 + level 11
        // + ticks 4.
        blob[85..89].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(Recording::deserialize(&blob).err(), Some("truncated"));
    }

    #[test]
    fn compression_shrinks_a_real_recording() {
        // A recording with periodic keyframes and sparse events should
        // deflate well (repetitive structure), and must decompress intact.
        let mut rec = Recording::new(params(), lparams(), u32::MAX);
        rec.push_keyframe(kf(0));
        for i in 0..60 * KEYFRAME_EVERY {
            let input = InputState {
                throttle: if (i / 300) % 2 == 0 { 255 } else { 0 },
                ..Default::default()
            };
            if rec.record_tick(input) {
                rec.push_keyframe(kf(rec.ticks()));
            }
        }
        let blob = rec.serialize(1);
        let packed = compress(&blob);
        assert!(packed.len() < blob.len() / 2, "{} vs {}", packed.len(), blob.len());
        let back = miniz_oxide::inflate::decompress_to_vec(&packed).expect("inflate");
        assert_eq!(back, blob);
    }

}
