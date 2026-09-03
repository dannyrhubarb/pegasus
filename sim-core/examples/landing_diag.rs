// Landing diagnostic: re-sim a .pgrec replay blob (e.g. from a bug-report
// zip's replays/ folder) and log the landing predicate around every pad
// approach — which settle condition held or failed, tick by tick, and how
// far the registration timer got. Built for "my landing didn't register"
// reports: the first one (Hollows, 2026-09) turned out to be a lift-off
// TWO TICKS before the 0.8 s hold completed. The verifier-side twin is
// pegasus-backend's `verify_blob` example.
//
//   cargo run --release --example landing_diag -- path/to/blob.pgrec
use pegasus_sim::replay::{decompress, InputState, Recording};
use pegasus_sim::sim::{ruleset_number, Sim, FOOT_X, FOOT_Y, PHYSICS_DT};
use pegasus_sim::world::{Level, PAD_HALF_W};

fn main() {
    let path = std::env::args().nth(1).expect("usage: landing_diag <blob.pgrec>");
    let raw = std::fs::read(&path).expect("read blob");
    let data = decompress(&raw).expect("inflate");
    let (rec, build) = Recording::deserialize(&data).expect("deserialize");
    println!(
        "build id {:08x}, {} ticks ({:.1} s), {} events, {} keyframes, scoring={} terrain={}",
        build,
        rec.ticks(),
        rec.ticks() as f32 * PHYSICS_DT,
        rec.events.len(),
        rec.keyframes.len(),
        rec.level.scoring,
        rec.level.terrain.is_some(),
    );

    println!(
        "ruleset {} (hold {:.2} s, land rule {})",
        ruleset_number(&rec.params).map_or("unknown".to_string(), |n| n.to_string()),
        rec.params.pad_land_time,
        if rec.params.land_rule >= 1.0 { "both feet" } else { "centre" },
    );
    let mut sim = Sim::with_rules(Level::from_params(&rec.level), rec.params);
    let hold = rec.params.pad_land_time;
    let both_feet = rec.params.land_rule >= 1.0;
    let k0 = *rec.keyframes.first().expect("keyframe 0");
    sim.restore(&k0);

    let mut events = rec.events.iter().peekable();
    let mut input = InputState::default();
    let mut was_near = false;
    let mut was_landed = false;

    for tick in k0.tick..rec.ticks() {
        while events.peek().is_some_and(|e| e.tick <= tick) {
            input = events.next().unwrap().input;
        }
        let rep = sim.tick(input);
        let (x, y, ang) = sim.ship_pose();
        let (vx, vy) = sim.ship_vel();
        let w = sim.ship_angvel();
        let feet = y - 0.73;
        let timer = sim.land_progress() * hold;
        // Foot positions (the ruleset-2 rule samples these).
        let (c, s) = (ang.cos(), ang.sin());
        let feet_xy = [-FOOT_X, FOOT_X].map(|lx| (x + lx * c - FOOT_Y * s, y + lx * s + FOOT_Y * c));

        // Nearest pad by feet distance while horizontally near a deck.
        let near = sim
            .pads
            .iter()
            .map(|(&key, pad)| (key, pad.cx, (x - pad.cx).abs(), (feet - pad.y).abs()))
            .filter(|&(_, _, dx, _)| dx <= PAD_HALF_W + 2.0)
            .min_by(|a, b| a.3.total_cmp(&b.3));

        let t = (tick + 1) as f32 * PHYSICS_DT;
        if let Some(imp) = &rep.impact {
            println!(
                "t={:7.3} tick={:6} IMPACT dv={:.2} damage={:.1} hull={:.1}{}  at x={:.2} y={:.2}",
                t, tick, imp.dv, imp.damage, sim.hull,
                if imp.destroyed { " DESTROYED" } else { "" },
                x, y
            );
        }

        let interesting = near.as_ref().is_some_and(|&(_, _, _, dy)| dy < 1.5);
        if (interesting || was_near) && let Some((key, cx, dx, dy)) = near {
            // Mirror of the sim's landing predicate (sim.rs), per condition
            // so the log names exactly what blocked the timer.
            let pad_y = sim.pads[&key].y;
            let foot_off = |i: usize| {
                both_feet
                    && ((feet_xy[i].0 - cx).abs() > PAD_HALF_W || (feet_xy[i].1 - pad_y).abs() >= 0.3)
            };
            let fails: Vec<&str> = [
                (!both_feet && dx > PAD_HALF_W).then_some("off-deck-x"),
                (!both_feet && dy >= 0.3).then_some("feet-not-on-deck"),
                foot_off(0).then_some("left-foot-off"),
                foot_off(1).then_some("right-foot-off"),
                (ang.abs() >= 0.30).then_some("tilted"),
                (vx.abs() >= 1.0).then_some("vx"),
                (vy.abs() >= 1.0).then_some("vy"),
                (w.abs() >= 0.5).then_some("spinning"),
            ]
            .into_iter()
            .flatten()
            .collect();
            // Every tick while the hold nears registration; else 20 Hz.
            if timer > 0.7 * hold || tick % 6 == 0 || rep.impact.is_some() || rep.landed {
                println!(
                    "t={:7.3} tick={:6} pad{:?} cx={:.1} dx={:+.2} dy={:+.3} ang={:+.3} v=({:+.2},{:+.2}) w={:+.2} timer={:.3}{} {}",
                    t, tick, key, cx, dx, dy, ang, vx, vy, w, timer,
                    if rep.landed { " LANDED" } else { "" },
                    if fails.is_empty() { String::from("ok") } else { fails.join(",") },
                );
            }
            if rep.landed && !was_landed {
                println!(
                    "t={:7.3} tick={:6} >>> LANDING REGISTERED on pad {:?}; visited={:?} score={} completed={}",
                    t, tick, key, sim.visited_pads, sim.score, rep.completed
                );
            }
            was_landed = rep.landed;
        }
        was_near = interesting;
        if rep.completed {
            println!(
                "t={:7.3} tick={:6} RUN COMPLETED, time={:.1}s",
                t, tick, sim.run_ticks as f32 * PHYSICS_DT
            );
        }
        if sim.crashed {
            println!(
                "t={:7.3} tick={:6} CRASHED at x={:.2} y={:.2}; visited={:?}",
                t, tick, x, y, sim.visited_pads
            );
            break;
        }
    }
    println!(
        "end: crashed={} completed={} visited={:?} fuel={:.1} hull={:.1} max_dist={:.1}",
        sim.crashed, sim.completed, sim.visited_pads, sim.fuel, sim.hull, sim.max_dist
    );
}
