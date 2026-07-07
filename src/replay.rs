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
pub const REPLAY_FORMAT_VERSION: u16 = 1;

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

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct InputEvent {
    pub tick: u32, // input takes effect from this tick's step
    pub input: InputState,
}

// Full simulation state at a tick — enough to resume/verify a re-sim.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Keyframe {
    pub tick: u32,
    pub x: f32,
    pub y: f32,
    pub angle: f32,
    pub vx: f32,
    pub vy: f32,
    pub angvel: f32,
    pub fuel: f32,
    pub hull: f32,
    pub glow: f32,
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

    // Read side (from_array/deserialize/Reader) is only exercised by tests
    // today; it becomes live code the moment replays leave the device.
    #[allow(dead_code)]
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
    pub events: Vec<InputEvent>,
    pub keyframes: Vec<Keyframe>,
    ticks: u32,             // physics steps recorded so far
    last_input: Option<InputState>,
    max_ticks: u32,         // retention window (trimmed at keyframe boundaries)
}

impl Recording {
    pub fn new(params: SimParams, max_ticks: u32) -> Self {
        Recording {
            params,
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
        self.ticks % KEYFRAME_EVERY == 0
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
        if let Some(input) = effective {
            if self.events.first().map(|e| e.tick) != Some(start) {
                self.events.insert(0, InputEvent { tick: start, input });
            }
        }
    }

    // --- Serialization (little-endian) ---
    // Header: magic(4) version(2) build_id(4) params(15×4) ticks(4)
    //         n_events(4) n_keyframes(4)                          = 82 B
    // Event:  tick(4) throttle(1) rot(1) steer_x(1) steer_y(1) held(1) = 9 B
    // Keyframe: tick(4) + 9×f32                                       = 40 B

    pub fn serialize(&self, build_id: u32) -> Vec<u8> {
        let mut out = Vec::with_capacity(82 + self.events.len() * 9 + self.keyframes.len() * 40);
        out.extend_from_slice(&REPLAY_MAGIC);
        out.extend_from_slice(&REPLAY_FORMAT_VERSION.to_le_bytes());
        out.extend_from_slice(&build_id.to_le_bytes());
        for f in self.params.to_array() {
            out.extend_from_slice(&f.to_le_bytes());
        }
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
            for f in [k.x, k.y, k.angle, k.vx, k.vy, k.angvel, k.fuel, k.hull, k.glow] {
                out.extend_from_slice(&f.to_le_bytes());
            }
        }
        out
    }

    #[allow(dead_code)]
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
        let ticks = r.u32()?;
        let n_events = r.u32()? as usize;
        let n_keyframes = r.u32()? as usize;
        let mut events = Vec::with_capacity(n_events);
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
        let mut keyframes = Vec::with_capacity(n_keyframes);
        for _ in 0..n_keyframes {
            let tick = r.u32()?;
            let mut f = [0f32; 9];
            for v in &mut f {
                *v = r.f32()?;
            }
            keyframes.push(Keyframe {
                tick,
                x: f[0], y: f[1], angle: f[2], vx: f[3], vy: f[4],
                angvel: f[5], fuel: f[6], hull: f[7], glow: f[8],
            });
        }
        let last_input = events.last().map(|e| e.input);
        Ok((
            Recording { params, events, keyframes, ticks, last_input, max_ticks: u32::MAX },
            build_id,
        ))
    }
}

#[allow(dead_code)]
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

#[allow(dead_code)]
impl<'a> Reader<'a> {
    fn bytes(&mut self, n: usize) -> Result<&'a [u8], &'static str> {
        let s = self.data.get(self.pos..self.pos + n).ok_or("truncated")?;
        self.pos += n;
        Ok(s)
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

// Deflate, level 8 — what the replay blob would ship over the wire as.
pub fn compress(data: &[u8]) -> Vec<u8> {
    miniz_oxide::deflate::compress_to_vec(data, 8)
}

// "512 B" / "3.4 KB" for the dialog button.
pub fn fmt_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else {
        format!("{:.1} KB", bytes as f32 / 1024.0)
    }
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

    fn kf(tick: u32) -> Keyframe {
        Keyframe {
            tick,
            x: tick as f32 * 0.1, y: 5.0, angle: 0.3, vx: 1.0, vy: -0.5,
            angvel: 0.0, fuel: 90.0, hull: 100.0, glow: 0.7,
        }
    }

    #[test]
    fn events_are_deduplicated() {
        let mut rec = Recording::new(params(), u32::MAX);
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
        let mut rec = Recording::new(params(), u32::MAX);
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
        let mut rec = Recording::new(params(), 2 * KEYFRAME_EVERY);
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
        let mut rec = Recording::new(params(), u32::MAX);
        rec.push_keyframe(kf(0));
        rec.finalize(kf(0));
        assert_eq!(rec.keyframes.len(), 1);
        rec.finalize(kf(7));
        assert_eq!(rec.keyframes.len(), 2);
    }

    #[test]
    fn serialize_roundtrips() {
        let mut rec = Recording::new(params(), u32::MAX);
        rec.push_keyframe(kf(0));
        let burn = InputState { throttle: 255, rot: -1, steer_x: 40, steer_y: -90, stick_held: 1 };
        for _ in 0..KEYFRAME_EVERY {
            if rec.record_tick(burn) {
                rec.push_keyframe(kf(rec.ticks()));
            }
        }
        let blob = rec.serialize(0xdeadbeef);
        assert_eq!(blob.len(), 82 + rec.events.len() * 9 + rec.keyframes.len() * 40);
        let (back, build_id) = Recording::deserialize(&blob).expect("deserialize");
        assert_eq!(build_id, 0xdeadbeef);
        assert_eq!(back.params, rec.params);
        assert_eq!(back.ticks(), rec.ticks());
        assert_eq!(back.events, rec.events);
        assert_eq!(back.keyframes, rec.keyframes);
    }

    #[test]
    fn compression_shrinks_a_real_recording() {
        // A recording with periodic keyframes and sparse events should
        // deflate well (repetitive structure), and must decompress intact.
        let mut rec = Recording::new(params(), u32::MAX);
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

    #[test]
    fn fmt_size_picks_sane_units() {
        assert_eq!(fmt_size(512), "512 B");
        assert_eq!(fmt_size(1536), "1.5 KB");
    }
}
