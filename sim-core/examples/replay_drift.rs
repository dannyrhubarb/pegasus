// Replay drift check: re-sim every .pgrec blob in a directory (or the
// blobs given) on THIS build and report how far the re-simulated
// trajectory strays from the RECORDED keyframes. Built for physics-engine
// bumps (CLAUDE.md "Physics engines"): it is how the 2026-09 rapier 0.31
// trial was measured — 0 of 69 live board runs bit-exact, 24 outside the
// verifier tolerances — and the regression gate for the legacy engine
// (the legacy report must not change at all when engine.rs is touched).
//
//   cargo run --release --example replay_drift -- path/to/blobs/
//   cargo run --release --example replay_drift -- a.pgrec b.pgrec
//
// Two views per blob:
//   batch  — one uninterrupted resim from keyframe 0 with NO re-anchoring
//            (what the game's ResimPlayer does between snaps): keyframes
//            reproduced bit-exactly, first divergence, max position drift,
//            and whether the recorded ending (crash tick) reproduces.
//   verify — a port of pegasus-backend verify.rs's segment loop: one sim,
//            compare at every claimed keyframe (impact-aware on the
//            terminal one), re-anchor on the claim, stop at a destroying
//            impact; then the drift verdict. The score bound needs the
//            claimed score, which a bare blob doesn't carry — an optional
//            `scores.txt` next to the blobs ("<file> <claimed score>" per
//            line, e.g. pulled from a board) enables it.
use std::collections::HashMap;
use std::path::PathBuf;

use pegasus_sim::replay::{decompress, InputState, Keyframe, Recording};
use pegasus_sim::sim::{ruleset_number, Sim, PHYSICS_DT};
use pegasus_sim::world::Level;

// Mirrors of pegasus-backend verify.rs's tolerances (keep in sync).
const POS_TOL: f32 = 0.5;
const VEL_TOL: f32 = 1.0;
const FUEL_TOL: f32 = 0.5;
const HULL_TOL: f32 = 5.0;
const SCORE_TOL_M: f64 = 1.0;
const TIME_TOL_S: f64 = 0.25;

fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

fn drift_exceeds(actual: &Keyframe, claimed: &Keyframe) -> Option<&'static str> {
    if dist((actual.x, actual.y), (claimed.x, claimed.y)) > POS_TOL {
        return Some("position");
    }
    if dist((actual.vx, actual.vy), (claimed.vx, claimed.vy)) > VEL_TOL {
        return Some("velocity");
    }
    if (actual.fuel - claimed.fuel).abs() > FUEL_TOL {
        return Some("fuel");
    }
    if (actual.hull - claimed.hull).abs() > HULL_TOL {
        return Some("hull");
    }
    None
}

fn bit_exact(a: &Keyframe, b: &Keyframe) -> bool {
    [a.x, a.y, a.vx, a.vy, a.rot_re, a.rot_im, a.fuel, a.hull]
        .iter()
        .zip([b.x, b.y, b.vx, b.vy, b.rot_re, b.rot_im, b.fuel, b.hull])
        .all(|(p, q)| p.to_bits() == q.to_bits())
}

// The keyframe the sim is at right now — from the Impact report on a
// destroying tick (the terminal keyframe carries the pre-park state; same
// construction as sim::resim and the verifier).
fn actual_kf(sim: &Sim, done: u32, destroyed: Option<pegasus_sim::sim::Impact>) -> Keyframe {
    match destroyed {
        Some(imp) => Keyframe {
            tick: done,
            x: imp.x, y: imp.y, rot_re: imp.rot_re, rot_im: imp.rot_im,
            vx: imp.vx, vy: imp.vy, angvel: imp.angvel,
            fuel: sim.fuel, hull: sim.hull,
            glow: 0.0, land_timer: 0.0,
            visited: sim.visited_mask(), run_ticks: sim.run_ticks,
        },
        None => sim.keyframe(done, 0.0),
    }
}

struct Report {
    exact: usize,
    first_div: Option<u32>,
    batch_max: f32,
    batch_end: String,
    ver_max_pos: f32,
    ver_max_vel: f32,
    verdict: String,
}

fn check(rec: &Recording, claimed_score: Option<f64>) -> Report {
    let kfs = &rec.keyframes;
    let last = kfs.last().unwrap();
    let recorded_crash = last.tick == rec.ticks() && last.hull <= 0.0;
    let time_scored = rec.level.scoring == 2;

    // ---- batch: no re-anchoring ----
    let mut sim = Sim::with_rules(Level::from_params(&rec.level), rec.params);
    sim.restore(&kfs[0]);
    let mut events = rec.events.iter().peekable();
    let mut input = InputState::default();
    let (mut exact, mut first_div, mut batch_max, mut next_kf, mut destroyed_at) =
        (1usize, None, 0f32, 1, None);
    for tick in kfs[0].tick..rec.ticks() {
        while events.peek().is_some_and(|e| e.tick <= tick) {
            input = events.next().unwrap().input;
        }
        let rep = sim.tick(input);
        let done = tick + 1;
        let destroyed = rep.impact.filter(|i| i.destroyed);
        if let Some(claimed) = kfs.get(next_kf).filter(|k| k.tick == done) {
            let actual = actual_kf(&sim, done, destroyed);
            batch_max = batch_max.max(dist((actual.x, actual.y), (claimed.x, claimed.y)));
            if bit_exact(&actual, claimed) {
                exact += 1;
            } else if first_div.is_none() {
                first_div = Some(done);
            }
            next_kf += 1;
        }
        if destroyed.is_some() {
            destroyed_at = Some(done);
            break;
        }
    }
    let batch_end = format!(
        "{} @{} → {}",
        if recorded_crash { "crash" } else { "alive" },
        last.tick,
        match destroyed_at {
            Some(d) => format!("crash @{d}"),
            None => format!("alive dist={:.1} done={}", sim.max_dist, sim.completed),
        }
    );

    // ---- verifier port ----
    let mut sim = Sim::with_rules(Level::from_params(&rec.level), rec.params);
    sim.restore(&kfs[0]);
    let mut best = f64::from(kfs[0].x.abs());
    let mut completed_at: Option<u32> = None;
    let mut events = rec.events.iter().peekable();
    let mut input = InputState::default();
    let (mut next_kf, mut ver_max_pos, mut ver_max_vel, mut resim_crash) = (1, 0f32, 0f32, None);
    let mut verdict = String::new();
    for tick in 0..rec.ticks() {
        while events.peek().is_some_and(|e| e.tick <= tick) {
            input = events.next().unwrap().input;
        }
        let rep = sim.tick(input);
        best = best.max(f64::from(sim.max_dist));
        let done = tick + 1;
        if rep.completed && completed_at.is_none() {
            completed_at = Some(done);
        }
        let destroyed = rep.impact.filter(|i| i.destroyed);
        if let Some(claimed) = kfs.get(next_kf).filter(|k| k.tick == done) {
            let actual = actual_kf(&sim, done, destroyed);
            ver_max_pos = ver_max_pos.max(dist((actual.x, actual.y), (claimed.x, claimed.y)));
            ver_max_vel = ver_max_vel.max(dist((actual.vx, actual.vy), (claimed.vx, claimed.vy)));
            if let Some(what) = drift_exceeds(&actual, claimed) {
                verdict = format!("REJECT drift({what}) @{done}");
                break;
            }
            next_kf += 1;
            if destroyed.is_none() {
                sim.restore(claimed);
                best = best.max(f64::from(claimed.x.abs()));
            }
        }
        if destroyed.is_some() {
            resim_crash = Some(done);
            break;
        }
    }
    if verdict.is_empty() {
        verdict = if time_scored {
            match completed_at {
                None => "REJECT not-completed".into(),
                Some(at) => {
                    let resimmed = f64::from(at) * f64::from(PHYSICS_DT);
                    match claimed_score {
                        Some(c) if c < resimmed - TIME_TOL_S => {
                            format!("REJECT time claimed {c:.2} < resim {resimmed:.2}")
                        }
                        _ => format!("ok (time {resimmed:.2})"),
                    }
                }
            }
        } else {
            match claimed_score {
                Some(c) if c > best + SCORE_TOL_M => {
                    format!("REJECT score claimed {c:.1} > resim {best:.1}")
                }
                _ => format!("ok (best {best:.1})"),
            }
        };
        if let Some(d) = resim_crash.filter(|&d| !(recorded_crash && d == last.tick)) {
            verdict.push_str(&format!(
                " [resim crashed @{d}, recorded {}]",
                if recorded_crash { format!("crash @{}", last.tick) } else { "alive".into() }
            ));
        }
    }
    Report { exact, first_div, batch_max, batch_end, ver_max_pos, ver_max_vel, verdict }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: replay_drift <dir | blob.pgrec ...>");
        std::process::exit(2);
    }
    let mut paths: Vec<PathBuf> = Vec::new();
    let mut scores: HashMap<String, f64> = HashMap::new();
    for a in &args {
        let p = PathBuf::from(a);
        if p.is_dir() {
            for e in std::fs::read_dir(&p).unwrap().flatten() {
                if e.path().extension().is_some_and(|x| x == "pgrec") {
                    paths.push(e.path());
                }
            }
            if let Ok(s) = std::fs::read_to_string(p.join("scores.txt")) {
                for l in s.lines() {
                    let mut it = l.split_whitespace();
                    if let (Some(n), Some(v)) = (it.next(), it.next().and_then(|v| v.parse().ok())) {
                        scores.insert(n.to_string(), v);
                    }
                }
            }
        } else {
            paths.push(p);
        }
    }
    paths.sort();
    println!(
        "{:<28} {:>2} {:>4} {:>5} {:>6} {:>8} | {:>7} {:>7}  verifier verdict ‖ batch ending",
        "blob", "rs", "kfs", "exact", "1stdiv", "batchmax", "vmaxpos", "vmaxvel",
    );
    let (mut n, mut n_exact, mut n_ok) = (0, 0, 0);
    for path in &paths {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let raw = std::fs::read(path).unwrap();
        let Some(data) = decompress(&raw) else { println!("{name:<28} inflate failed"); continue };
        let (rec, _) = match Recording::deserialize(&data) {
            Ok(r) => r,
            Err(e) => { println!("{name:<28} deserialize failed: {e}"); continue }
        };
        n += 1;
        let r = check(&rec, scores.get(&name).copied());
        if r.exact == rec.keyframes.len() { n_exact += 1; }
        if r.verdict.starts_with("ok") && !r.verdict.contains("[resim crashed") { n_ok += 1; }
        println!(
            "{:<28} {:>2} {:>4} {:>5} {:>6} {:>8.4} | {:>7.4} {:>7.4}  {}  ‖ batch: {}",
            name,
            ruleset_number(&rec.params).map_or("?".into(), |n| n.to_string()),
            rec.keyframes.len(),
            r.exact,
            r.first_div.map_or("-".to_string(), |t| format!("{:.0}s", t as f32 * PHYSICS_DT)),
            r.batch_max,
            r.ver_max_pos,
            r.ver_max_vel,
            r.verdict,
            r.batch_end,
        );
    }
    println!(
        "\n{n} blobs: {n_exact} bit-exact at every keyframe (no re-anchor), \
         {n_ok} accepted by the verifier model with the recorded ending"
    );
}
