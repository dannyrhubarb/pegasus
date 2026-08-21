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
// pre-v3 versions, so old stored blobs simply stop decoding (decode returns
// None — high-score watch/ghost buttons no-op, nothing crashes).
// v4: the LevelParams block gains a flags byte (bit 0 = endless cave,
// bit 1 = hand-drawn Terrain present) followed by the Terrain block (rock
// polygons, pads, optional start platform, spawn ground) when bit 1 is set,
// and every keyframe appends visited(u64) + run_ticks(u32) — see the
// Keyframe doc. Written ONLY when the
// level uses a post-v3 feature (endless, terrain, or time scoring) — a
// legacy procedural recording still serializes as byte-identical v3, so
// every existing blob keeps decoding and the backend keeps verifying legacy
// submissions without a re-pin. (Runs on the NEW levels DO need the backend
// re-pinned to a sim-core rev that understands v4.)
// v5: the LevelParams block gains time_limit_ticks (u32) + goal_distance
// (f32), right after the flags byte — the hard run clock of time-LIMITED
// levels (The Flux Sprint) and the finish-pad distance of GOAL levels
// (The Flux Dash). Same per-recording choice as v4: only a level using
// one of them writes v5, so v3/v4 blobs stay byte-identical and keep
// decoding.
pub const REPLAY_FORMAT_VERSION: u16 = 3;
pub const REPLAY_FORMAT_VERSION_EXT: u16 = 4;
pub const REPLAY_FORMAT_VERSION_V5: u16 = 5;

// Cosmetic TRAILER appended after the keyframes (any version): magic "PGXT"
// followed by TLV entries (tag u8, len u16 LE, payload). This is the
// format's FORWARD-compatibility channel: every parser ever shipped reads
// exactly the counted sections and ignores trailing bytes, so an old client
// (and the pinned backend verifier) decodes a trailered blob unchanged and
// simply presents the replay without the extra context — no version bump,
// no repin. Two hard rules keep that true:
// - Entries carry PRESENTATION data only. Nothing the sim consumes may ever
//   live here — an old client's resim would silently ignore it and desync.
// - New readers skip unknown tags by their length and treat any malformed
//   tail as "no trailer", so the trailer extends the same way.
// The trailer is written only when an entry has a non-default value, so a
// default-scheme recording stays byte-identical to the pre-trailer format.
// Security rules (the trailer is attacker-controlled content that reaches
// OTHER players' screens via stored replays): values presented to users
// must be enum/whitelist-validated — never free text — and the backend
// verifier rejects blobs whose tail is not a well-formed trailer within
// TRAILER_MAX_BYTES (see TailInfo), so the ignored-trailing-bytes property
// can't be used to smuggle arbitrary content into public replay storage.
pub const REPLAY_TRAILER_MAGIC: [u8; 4] = *b"PGXT";
// Trailer tag 1: the control scheme the run was flown with (u8 payload,
// 0 = one-handed stick-hold, 1 = split controls). Replays render the input
// widgets of the scheme that produced them: split runs draw the left-hand
// throttle button (lit while the recorded throttle is up) next to the
// steering stick.
pub const TRAILER_TAG_SCHEME: u8 = 1;
pub const SCHEME_STICK: u8 = 0;
pub const SCHEME_SPLIT: u8 = 1;

// Submit-validation cap on the tail (magic + all entries), enforced by the
// backend verifier — never by game-side decoding. A legitimate trailer
// today is 8 B, so this leaves generous headroom for future tags while
// keeping the tail useless as a channel for smuggling bulk content into
// replay storage.
pub const TRAILER_MAX_BYTES: usize = 256;

// What followed the counted sections (events + keyframes) of a blob —
// reported by `deserialize_with_tail` so the backend verifier can gate
// storage hygiene without changing how anything PLAYS a replay. `strict`
// means the tail is empty, or a well-formed trailer (magic + TLV entries)
// tiling exactly to the end of the buffer. The game itself stays lenient —
// a malformed tail just reads as "no trailer" — but the verifier rejects
// non-strict or oversized tails at submit time, because stored blobs are
// re-served verbatim via CloudFront and a free-form tail would let a
// submission park arbitrary bytes in public storage.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TailInfo {
    pub len: usize,   // bytes after the last counted section
    pub strict: bool, // empty, or a well-formed trailer tiling exactly to the end
}

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
    // v4-only game-logic state (serialized only in the extended format; a
    // v3 read leaves visited = 0 and run_ticks = tick):
    // - visited: hand-drawn pad-visit bitmask (bit i = terrain pad i), so a
    //   replay SEEK restores the x/5 counter, beacon colors and the
    //   completed flag instead of losing them with the rebuilt scratch sim.
    //   Procedural levels keep 0 — their (slot, layer) keys don't bitmask.
    // - run_ticks: the run clock, which FREEZES at completion and therefore
    //   can lag the keyframe tick (the post-completion grace keeps ticking).
    pub visited: u64,
    pub run_ticks: u32,
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
// physics (the cave/obstacle/pad generator parameters — or, for hand-drawn
// levels, the full Terrain), so resim rebuilds the exact world the run was
// flown in. The level NAME is cosmetic and deliberately not part of this —
// two levels with equal params are the same world. Conversions to/from
// `world::Level` live in world.rs.
#[derive(Clone, PartialEq, Debug)]
pub struct LevelParams {
    pub scoring: u8, // 0 = pads, 1 = distance, 2 = time (run ends at all pads visited)
    pub shafts: u8,
    pub obstacles: u8,
    pub pad_spacing: f32,
    pub seed: u32,
    pub endless: u8, // no x-wrap: value-noise cave (format v4)
    pub terrain: Option<crate::world::Terrain>, // Some = hand-drawn world (format v4)
    pub time_limit_ticks: u32, // hard run clock, 0 = none (format v5)
    pub goal_distance: f32,    // finish-pad distance, 0 = none (format v5)
}

impl LevelParams {
    // The lowest format version whose layout can express this level — the
    // per-recording version choice that keeps every older blob decoding.
    fn format_version(&self) -> u16 {
        if self.time_limit_ticks != 0 || self.goal_distance != 0.0 {
            REPLAY_FORMAT_VERSION_V5
        } else if self.endless != 0 || self.terrain.is_some() || self.scoring >= 2 {
            REPLAY_FORMAT_VERSION_EXT
        } else {
            REPLAY_FORMAT_VERSION
        }
    }
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
    // Control scheme the run was flown with (SCHEME_*) — presentation only,
    // rides the cosmetic trailer, never consumed by the sim. The recorder
    // keeps it current per recorded tick (last write wins on a mid-run
    // toggle); a blob without a trailer reads as SCHEME_STICK.
    pub scheme: u8,
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
            scheme: SCHEME_STICK,
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
    //         [v4/v5 only, right after seed: flags(1) — bit 0 endless,
    //          bit 1 terrain present; v5 only: time_limit_ticks(4) +
    //          goal_distance(4); if flags bit 1: terrain — n_polys(2),
    //          per poly n_verts(2) + verts(2×4 each); n_pads(2) +
    //          pads(2×4 each); has_start(1) [+ start 2×4]; spawn_y(4)]
    //         ticks(4) n_events(4) n_keyframes(4)      = 93 B (v3, no extras)
    // Event:  tick(4) throttle(1) rot(1) steer_x(1) steer_y(1) held(1) = 9 B
    // Keyframe: tick(4) + 11×f32                          = 48 B (v3)
    //           v4/v5 append visited(8) + run_ticks(4)    = 60 B
    // Trailer (optional, any version, AFTER the last keyframe): "PGXT" +
    //           TLV entries (tag u8, len u16, payload) — cosmetic data old
    //           parsers never reach (see REPLAY_TRAILER_MAGIC). Written
    //           only when an entry is non-default.
    // The version is picked per recording: a legacy procedural level writes
    // the byte-identical v3 layout; endless / terrain / time-scored levels
    // write v4; time-limited levels write v5.

    pub fn serialize(&self, build_id: u32) -> Vec<u8> {
        let mut out = Vec::with_capacity(93 + self.events.len() * 9 + self.keyframes.len() * 48);
        out.extend_from_slice(&REPLAY_MAGIC);
        let version = self.level.format_version();
        out.extend_from_slice(&version.to_le_bytes());
        out.extend_from_slice(&build_id.to_le_bytes());
        for f in self.params.to_array() {
            out.extend_from_slice(&f.to_le_bytes());
        }
        out.push(self.level.scoring);
        out.push(self.level.shafts);
        out.push(self.level.obstacles);
        out.extend_from_slice(&self.level.pad_spacing.to_le_bytes());
        out.extend_from_slice(&self.level.seed.to_le_bytes());
        if version != REPLAY_FORMAT_VERSION {
            let flags = (self.level.endless & 1)
                | if self.level.terrain.is_some() { 2 } else { 0 };
            out.push(flags);
        }
        if version == REPLAY_FORMAT_VERSION_V5 {
            out.extend_from_slice(&self.level.time_limit_ticks.to_le_bytes());
            out.extend_from_slice(&self.level.goal_distance.to_le_bytes());
        }
        if let Some(t) = &self.level.terrain {
            out.extend_from_slice(&(t.polys.len() as u16).to_le_bytes());
            for poly in &t.polys {
                out.extend_from_slice(&(poly.len() as u16).to_le_bytes());
                for p in poly {
                    out.extend_from_slice(&p.x.to_le_bytes());
                    out.extend_from_slice(&p.y.to_le_bytes());
                }
            }
            out.extend_from_slice(&(t.pads.len() as u16).to_le_bytes());
            for p in &t.pads {
                out.extend_from_slice(&p.x.to_le_bytes());
                out.extend_from_slice(&p.y.to_le_bytes());
            }
            out.push(t.start.is_some() as u8);
            if let Some(sp) = &t.start {
                out.extend_from_slice(&sp.x.to_le_bytes());
                out.extend_from_slice(&sp.y.to_le_bytes());
            }
            out.extend_from_slice(&t.spawn_y.to_le_bytes());
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
            for f in [k.x, k.y, k.rot_re, k.rot_im, k.vx, k.vy, k.angvel,
                      k.fuel, k.hull, k.glow, k.land_timer] {
                out.extend_from_slice(&f.to_le_bytes());
            }
            if version != REPLAY_FORMAT_VERSION {
                out.extend_from_slice(&k.visited.to_le_bytes());
                out.extend_from_slice(&k.run_ticks.to_le_bytes());
            }
        }
        // Cosmetic trailer — only when there is a non-default value to
        // carry, so a default-scheme recording stays byte-identical to the
        // pre-trailer format.
        if self.scheme != SCHEME_STICK {
            out.extend_from_slice(&REPLAY_TRAILER_MAGIC);
            out.push(TRAILER_TAG_SCHEME);
            out.extend_from_slice(&1u16.to_le_bytes());
            out.push(self.scheme);
        }
        out
    }

    pub fn deserialize(data: &[u8]) -> Result<(Recording, u32), &'static str> {
        Self::deserialize_with_tail(data).map(|(rec, build_id, _)| (rec, build_id))
    }

    // `deserialize` plus a report on the bytes AFTER the counted sections
    // (see TailInfo) — the backend verifier's entry point, so it can reject
    // malformed/oversized tails at submit time. Decoding itself treats the
    // tail identically to `deserialize` (leniently).
    pub fn deserialize_with_tail(
        data: &[u8],
    ) -> Result<(Recording, u32, TailInfo), &'static str> {
        let mut r = Reader { data, pos: 0 };
        if r.bytes(4)? != REPLAY_MAGIC {
            return Err("bad magic");
        }
        let version = r.u16()?;
        if !matches!(
            version,
            REPLAY_FORMAT_VERSION | REPLAY_FORMAT_VERSION_EXT | REPLAY_FORMAT_VERSION_V5
        ) {
            return Err("unsupported version");
        }
        let build_id = r.u32()?;
        let mut a = [0f32; SimParams::N_FIELDS];
        for f in &mut a {
            *f = r.f32()?;
        }
        let params = SimParams::from_array(a);
        let mut level = LevelParams {
            scoring: r.u8()?,
            shafts: r.u8()?,
            obstacles: r.u8()?,
            pad_spacing: r.f32()?,
            seed: r.u32()?,
            endless: 0,
            terrain: None,
            time_limit_ticks: 0,
            goal_distance: 0.0,
        };
        if version != REPLAY_FORMAT_VERSION {
            let flags = r.u8()?;
            level.endless = flags & 1;
            if version == REPLAY_FORMAT_VERSION_V5 {
                level.time_limit_ticks = r.u32()?;
                level.goal_distance = r.f32()?;
            }
            if flags & 2 != 0 {
                // Same hostile-count rule as events/keyframes below: cap
                // every Vec::with_capacity by what the remaining bytes
                // could hold.
                let n_polys = r.u16()? as usize;
                let mut polys = Vec::with_capacity(n_polys.min(r.remaining() / 2));
                for _ in 0..n_polys {
                    let n_verts = r.u16()? as usize;
                    let mut poly = Vec::with_capacity(n_verts.min(r.remaining() / 8));
                    for _ in 0..n_verts {
                        poly.push(glam::vec2(r.f32()?, r.f32()?));
                    }
                    polys.push(poly);
                }
                let n_pads = r.u16()? as usize;
                let mut pads = Vec::with_capacity(n_pads.min(r.remaining() / 8));
                for _ in 0..n_pads {
                    pads.push(glam::vec2(r.f32()?, r.f32()?));
                }
                let start = (r.u8()? != 0)
                    .then(|| Ok::<_, &'static str>(glam::vec2(r.f32()?, r.f32()?)))
                    .transpose()?;
                let spawn_y = r.f32()?;
                level.terrain =
                    Some(crate::world::Terrain { polys, pads, start, spawn_y });
            }
        }
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
        let kf_size = if version == REPLAY_FORMAT_VERSION { 48 } else { 60 };
        let mut keyframes = Vec::with_capacity(n_keyframes.min(r.remaining() / kf_size));
        for _ in 0..n_keyframes {
            let tick = r.u32()?;
            let mut f = [0f32; 11];
            for v in &mut f {
                *v = r.f32()?;
            }
            let (visited, run_ticks) = if version == REPLAY_FORMAT_VERSION {
                (0, tick) // pre-extension state: the clock never froze
            } else {
                (r.u64()?, r.u32()?)
            };
            keyframes.push(Keyframe {
                tick,
                x: f[0], y: f[1], rot_re: f[2], rot_im: f[3], vx: f[4], vy: f[5],
                angvel: f[6], fuel: f[7], hull: f[8], glow: f[9], land_timer: f[10],
                visited, run_ticks,
            });
        }
        // Cosmetic trailer (see REPLAY_TRAILER_MAGIC). Parsed LENIENTLY for
        // decoding — it carries only presentation data, so any malformation
        // (short magic, truncated entry) just means "no trailer", never an
        // error: a blob an old client decodes must never fail here. The
        // walk additionally reports the tail's strictness (TailInfo) for
        // the backend verifier's storage-hygiene gate: strict = empty, or
        // magic + well-formed TLV entries tiling exactly to the buffer end.
        let tail_len = r.remaining();
        let mut scheme = SCHEME_STICK;
        let mut tail_strict = tail_len == 0;
        if tail_len >= 4 && r.bytes(4).is_ok_and(|b| b == REPLAY_TRAILER_MAGIC) {
            tail_strict = true;
            while r.remaining() > 0 {
                let (Ok(tag), Ok(len)) = (r.u8(), r.u16()) else {
                    tail_strict = false;
                    break;
                };
                let Ok(payload) = r.bytes(len as usize) else {
                    tail_strict = false;
                    break;
                };
                if tag == TRAILER_TAG_SCHEME && payload.len() == 1 {
                    scheme = payload[0];
                }
                // Unknown tags: skipped by their length (trailer extension).
            }
        }
        let last_input = events.last().map(|e| e.input);
        Ok((
            Recording {
                params, level, events, keyframes, scheme, ticks, last_input,
                max_ticks: u32::MAX,
            },
            build_id,
            TailInfo { len: tail_len, strict: tail_strict },
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
    fn u64(&mut self) -> Result<u64, &'static str> {
        Ok(u64::from_le_bytes(self.bytes(8)?.try_into().unwrap()))
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
        LevelParams {
            scoring: 0, shafts: 1, obstacles: 1, pad_spacing: 130.0, seed: 0,
            endless: 0, terrain: None, time_limit_ticks: 0, goal_distance: 0.0,
        }
    }

    fn kf(tick: u32) -> Keyframe {
        Keyframe {
            tick,
            x: tick as f32 * 0.1, y: 5.0, rot_re: 0.955, rot_im: 0.296,
            vx: 1.0, vy: -0.5, angvel: 0.0, fuel: 90.0, hull: 100.0,
            glow: 0.7, land_timer: 0.25,
            // v3-lossless values (a v3 read yields exactly these); the v4
            // test overrides them to prove the extended fields round-trip.
            visited: 0, run_ticks: tick,
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
    fn endless_and_time_levels_roundtrip_as_v4() {
        // The v4 flags byte: an endless (no-wrap) cave and a time-scored
        // level must both force the extended format and round-trip exactly.
        let lp = LevelParams { endless: 1, scoring: 2, ..lparams() };
        let mut rec = Recording::new(params(), lp.clone(), u32::MAX);
        rec.push_keyframe(kf(0));
        let blob = rec.serialize(3);
        assert_eq!(u16::from_le_bytes([blob[4], blob[5]]), REPLAY_FORMAT_VERSION_EXT);
        let (back, _) = Recording::deserialize(&blob).expect("deserialize v4");
        assert_eq!(back.level, lp);
    }

    #[test]
    fn goal_levels_roundtrip_as_v5() {
        // A finish-pad distance forces the v5 layout too (The Flux Dash:
        // endless + time-scored + goal) and must round-trip exactly.
        let lp = LevelParams { endless: 1, scoring: 2, goal_distance: 1000.0, ..lparams() };
        let mut rec = Recording::new(params(), lp.clone(), u32::MAX);
        rec.push_keyframe(kf(0));
        let blob = rec.serialize(4);
        assert_eq!(u16::from_le_bytes([blob[4], blob[5]]), REPLAY_FORMAT_VERSION_V5);
        let (back, _) = Recording::deserialize(&blob).expect("deserialize v5 goal");
        assert_eq!(back.level, lp);
    }

    #[test]
    fn time_limited_levels_roundtrip_as_v5() {
        // A hard run clock forces the v5 layout (v4 has no slot for it) and
        // must round-trip exactly — including alongside the endless flag,
        // The Flux Sprint's actual combination.
        let lp = LevelParams { endless: 1, scoring: 1, time_limit_ticks: 7200, ..lparams() };
        let mut rec = Recording::new(params(), lp.clone(), u32::MAX);
        rec.push_keyframe(kf(0));
        for _ in 0..KEYFRAME_EVERY {
            if rec.record_tick(InputState { throttle: 255, ..Default::default() }) {
                let mut k = kf(rec.ticks());
                k.run_ticks = rec.ticks().min(7200); // the frozen-clock shape
                rec.push_keyframe(k);
            }
        }
        let blob = rec.serialize(9);
        assert_eq!(u16::from_le_bytes([blob[4], blob[5]]), REPLAY_FORMAT_VERSION_V5);
        let (back, build_id) = Recording::deserialize(&blob).expect("deserialize v5");
        assert_eq!(build_id, 9);
        assert_eq!(back.level, lp);
        assert_eq!(back.events, rec.events);
        assert_eq!(back.keyframes, rec.keyframes);
    }

    #[test]
    fn terrain_levels_roundtrip_as_v4_and_procedural_stays_v3() {
        // A legacy procedural recording must serialize byte-compatible v3
        // (existing stored blobs and the deployed backend keep working); a
        // hand-drawn level rides its full Terrain in a v4 blob and
        // round-trips exactly.
        let mut rec = Recording::new(params(), lparams(), u32::MAX);
        rec.push_keyframe(kf(0));
        let blob = rec.serialize(1);
        assert_eq!(u16::from_le_bytes([blob[4], blob[5]]), REPLAY_FORMAT_VERSION);

        let terrain = crate::world::Terrain {
            polys: vec![
                vec![glam::vec2(-10.0, -5.0), glam::vec2(10.0, -5.0), glam::vec2(0.0, -1.0)],
                vec![
                    glam::vec2(-8.0, 8.0), glam::vec2(8.0, 8.0),
                    glam::vec2(8.0, 12.0), glam::vec2(-8.0, 12.0),
                ],
            ],
            pads: vec![glam::vec2(0.0, -0.5), glam::vec2(5.0, 9.0)],
            start: Some(glam::vec2(-3.0, -0.5)),
            spawn_y: -0.5,
        };
        let lp = LevelParams { terrain: Some(terrain), ..lparams() };
        let mut rec = Recording::new(params(), lp.clone(), u32::MAX);
        rec.push_keyframe(kf(0));
        for _ in 0..KEYFRAME_EVERY {
            if rec.record_tick(InputState { throttle: 200, ..Default::default() }) {
                // Non-trivial game-logic state: only v4 carries it.
                let mut k = kf(rec.ticks());
                k.visited = 0b10101;
                k.run_ticks = rec.ticks() - 7;
                rec.push_keyframe(k);
            }
        }
        let blob = rec.serialize(7);
        assert_eq!(u16::from_le_bytes([blob[4], blob[5]]), REPLAY_FORMAT_VERSION_EXT);
        let (back, build_id) = Recording::deserialize(&blob).expect("deserialize v4");
        assert_eq!(build_id, 7);
        assert_eq!(back.level, lp);
        assert_eq!(back.events, rec.events);
        assert_eq!(back.keyframes, rec.keyframes);
    }

    #[test]
    fn split_scheme_rides_a_trailer_old_parsers_never_reach() {
        let mut rec = Recording::new(params(), lparams(), u32::MAX);
        rec.scheme = SCHEME_SPLIT;
        rec.push_keyframe(kf(0));
        for _ in 0..KEYFRAME_EVERY {
            let input = InputState { throttle: 255, stick_held: 1, ..Default::default() };
            if rec.record_tick(input) {
                rec.push_keyframe(kf(rec.ticks()));
            }
        }
        let blob = rec.serialize(2);
        // The trailer must not force a version bump — still a plain v3 blob.
        assert_eq!(u16::from_le_bytes([blob[4], blob[5]]), REPLAY_FORMAT_VERSION);
        let (back, _) = Recording::deserialize(&blob).expect("deserialize");
        assert_eq!(back.scheme, SCHEME_SPLIT);
        assert_eq!(back.events, rec.events);
        assert_eq!(back.keyframes, rec.keyframes);
        // An old parser stops after the counted sections and never sees the
        // trailer: chopping it off must decode to the same recording minus
        // the scheme — which is exactly what a pre-trailer client does with
        // the full blob (it reads the counted bytes and returns).
        let counted = blob.len() - 8; // magic 4 + tag 1 + len 2 + payload 1
        let (old, _) = Recording::deserialize(&blob[..counted]).expect("counted sections");
        assert_eq!(old.scheme, SCHEME_STICK);
        assert_eq!(old.events, back.events);
        assert_eq!(old.keyframes, back.keyframes);
    }

    #[test]
    fn default_scheme_writes_no_trailer_and_malformed_trailers_are_lenient() {
        // A default-scheme recording stays byte-identical to the
        // pre-trailer format (nothing appended).
        let mut rec = Recording::new(params(), lparams(), u32::MAX);
        rec.push_keyframe(kf(0));
        let blob = rec.serialize(0);
        assert_eq!(blob.len(), 93 + rec.events.len() * 9 + rec.keyframes.len() * 48);
        let (back, _) = Recording::deserialize(&blob).expect("no trailer");
        assert_eq!(back.scheme, SCHEME_STICK);
        // Unknown tags are skipped by their length so the scheme entry
        // after one still lands (trailer extensibility).
        let mut b = blob.clone();
        b.extend_from_slice(&REPLAY_TRAILER_MAGIC);
        b.push(99);
        b.extend_from_slice(&1u16.to_le_bytes());
        b.push(7);
        b.push(TRAILER_TAG_SCHEME);
        b.extend_from_slice(&1u16.to_le_bytes());
        b.push(SCHEME_SPLIT);
        let (back, _) = Recording::deserialize(&b).expect("unknown tag skipped");
        assert_eq!(back.scheme, SCHEME_SPLIT);
        // Malformed tails never error — a truncated magic and an entry
        // whose length overruns the buffer both read as "no trailer".
        let mut b = blob.clone();
        b.extend_from_slice(b"PG");
        assert_eq!(Recording::deserialize(&b).expect("short tail").0.scheme, SCHEME_STICK);
        let mut b = blob.clone();
        b.extend_from_slice(&REPLAY_TRAILER_MAGIC);
        b.push(TRAILER_TAG_SCHEME);
        b.extend_from_slice(&500u16.to_le_bytes());
        assert_eq!(Recording::deserialize(&b).expect("overrun entry").0.scheme, SCHEME_STICK);
    }

    #[test]
    fn tail_info_reports_strictness_for_the_verifier() {
        let mut rec = Recording::new(params(), lparams(), u32::MAX);
        rec.push_keyframe(kf(0));
        let clean = rec.serialize(0);
        let tail = |b: &[u8]| Recording::deserialize_with_tail(b).expect("decodes").2;
        // No tail at all: strict.
        assert_eq!(tail(&clean), TailInfo { len: 0, strict: true });
        // A well-formed trailer tiling exactly to the end: strict, and the
        // legitimate scheme trailer is 8 B — comfortably under the cap.
        rec.scheme = SCHEME_SPLIT;
        let trailered = rec.serialize(0);
        let info = tail(&trailered);
        assert_eq!(info, TailInfo { len: 8, strict: true });
        assert!(info.len <= TRAILER_MAX_BYTES);
        // Free-form junk: not strict.
        let mut b = clean.clone();
        b.extend_from_slice(b"arbitrary smuggled bytes");
        assert!(!tail(&b).strict);
        // Junk AFTER a valid trailer entry breaks strictness too (the walk
        // must reach exactly the end of the buffer).
        let mut b = trailered.clone();
        b.push(0xff);
        assert!(!tail(&b).strict);
        // An entry whose length overruns the buffer: not strict.
        let mut b = clean.clone();
        b.extend_from_slice(&REPLAY_TRAILER_MAGIC);
        b.push(TRAILER_TAG_SCHEME);
        b.extend_from_slice(&500u16.to_le_bytes());
        assert!(!tail(&b).strict);
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
