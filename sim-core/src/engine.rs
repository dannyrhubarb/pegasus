// The physics ENGINES behind `Sim` — two Rapier versions compiled side by
// side, selected per recording by the ruleset's `engine` field.
//
// Why two: a physics-crate bump changes simulation results (a different
// contact solver rounds differently, and chaos amplifies that through
// every collision), so a replay flown on one Rapier version does not
// re-simulate bit-exactly on another — measured 2026-09 on the way to
// rapier 0.35: 0 of 69 stored board runs reproduced their keyframes on the
// new engine, and 24 of them fell outside the backend verifier's drift
// tolerances. Since stored replays, the racing ghost and score
// verification all rest on bit-exact resim, the engine a run was flown on
// is part of its ruleset (`SimParams::engine`, format v6 extension field
// 6) and `Sim` builds the matching engine from the header:
//   - `Legacy` = rapier 0.23.1 (`rapier_legacy` in Cargo.toml, pinned
//     EXACTLY) — what rulesets 1 and 2 were recorded with. FROZEN: the
//     code in `legacy` below is the pre-engine `Sim` internals moved
//     verbatim (same builder calls, same insert/remove sequence), which is
//     what keeps every pre-existing blob resimming bit-for-bit — never
//     touch it to "clean up".
//   - `Modern` = rapier 0.35 (`rapier2d`) — ruleset 3 onwards.
// A future Rapier bump is a NEW variant + a new ruleset naming it (and a
// LOGIC_VERSION bump so older clients refuse those replays cleanly), never
// a replacement of an existing one. Both versions cost their code size in
// the wasm (measured unoptimised: 1.23 MB with 0.23 alone, 1.43 MB with
// 0.35 alone, 1.90 MB with both); that is the price of keeping history
// replayable.
//
// The interface below is the whole surface `Sim` needs: one dynamic ship
// body (three capsules), static segment/convex-hull colliders addressed by
// engine-neutral `ColHandle`s, per-tick forces and one `step`. Positions
// cross the boundary as plain f32 pairs / glam 0.27 `Vec2` (the crate's
// public math type, pinned to macroquad's re-export) so neither Rapier's
// math types (nalgebra in 0.23, glam 0.33 via glamx in 0.35) leak out.
//
// Determinism note: BOTH engines must be driven with the identical op
// sequence live and in resim (see the rules in sim.rs); the abstraction
// adds no state of its own, so that property is unchanged.

use glam::Vec2;

use crate::replay::SimParams;

// Which Rapier a ruleset runs on. `SimParams::engine` is a float like every
// other extension field: 0 = Legacy (the ruleset-1 neutral default, per
// the v6 append contract), 1 = Modern. Anything above 1 is an engine this
// build does not ship — the header's `min_logic` will have refused the
// blob before we get here; picking Modern is the lenient fallback for a
// decoder that got this far.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Engine {
    Legacy,
    Modern,
}

impl Engine {
    pub fn of(rules: &SimParams) -> Engine {
        if rules.engine >= 1.0 { Engine::Modern } else { Engine::Legacy }
    }
}

// The ship body as `Sim` wants it built: standing at (0, y0), damping and
// sleep policy from the ruleset. Gravity is the world's, not the body's.
pub struct ShipSpec {
    pub y0: f32,
    pub gravity_y: f32,
    pub linear_damping: f32,
    pub angular_damping: f32,
    pub can_sleep: bool,
}

// A static collider handle that means the same thing in either engine: the
// arena (index, generation) pair both Rapier versions expose through
// `ColliderHandle::{into,from}_raw_parts`. Handles are only ever used with
// the world that issued them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ColHandle(u32, u32);

// Boxed: a whole Rapier stack per variant is kilobytes, and `Sim` owns
// exactly one — the box keeps `Sim` itself small (clippy's
// large_enum_variant).
pub enum World {
    Legacy(Box<legacy::World>),
    Modern(Box<modern::World>),
}

macro_rules! dispatch {
    ($self:expr, $w:ident => $e:expr) => {
        match $self {
            World::Legacy($w) => $e,
            World::Modern($w) => $e,
        }
    };
}

impl World {
    pub fn new(engine: Engine, spec: &ShipSpec) -> World {
        match engine {
            Engine::Legacy => World::Legacy(Box::new(legacy::World::new(spec))),
            Engine::Modern => World::Modern(Box::new(modern::World::new(spec))),
        }
    }

    pub fn engine(&self) -> Engine {
        match self {
            World::Legacy(_) => Engine::Legacy,
            World::Modern(_) => Engine::Modern,
        }
    }

    // ---- the tick ----
    pub fn step(&mut self) {
        dispatch!(self, w => w.step())
    }

    // ---- ship body: writes (all wake the body, like the original code) ----
    pub fn set_gravity_scale(&mut self, scale: f32) {
        dispatch!(self, w => w.set_gravity_scale(scale))
    }
    pub fn set_translation(&mut self, x: f32, y: f32) {
        dispatch!(self, w => w.set_translation(x, y))
    }
    // The EXACT unit complex, never re-normalised or round-tripped through
    // an angle — a keyframe restore must reproduce the body's bits.
    pub fn set_rotation_unchecked(&mut self, re: f32, im: f32) {
        dispatch!(self, w => w.set_rotation_unchecked(re, im))
    }
    pub fn set_linvel(&mut self, vx: f32, vy: f32) {
        dispatch!(self, w => w.set_linvel(vx, vy))
    }
    pub fn set_angvel(&mut self, w: f32) {
        dispatch!(self, b => b.set_angvel(w))
    }
    pub fn reset_forces(&mut self) {
        dispatch!(self, w => w.reset_forces())
    }
    pub fn reset_torques(&mut self) {
        dispatch!(self, w => w.reset_torques())
    }
    pub fn add_force(&mut self, fx: f32, fy: f32) {
        dispatch!(self, w => w.add_force(fx, fy))
    }
    pub fn add_force_at_point(&mut self, fx: f32, fy: f32, px: f32, py: f32) {
        dispatch!(self, w => w.add_force_at_point(fx, fy, px, py))
    }
    pub fn add_torque(&mut self, t: f32) {
        dispatch!(self, w => w.add_torque(t))
    }

    // ---- ship body: reads ----
    pub fn translation(&self) -> (f32, f32) {
        dispatch!(self, w => w.translation())
    }
    // (re, im) of the body's unit-complex rotation, verbatim.
    pub fn rotation(&self) -> (f32, f32) {
        dispatch!(self, w => w.rotation())
    }
    pub fn angle(&self) -> f32 {
        dispatch!(self, w => w.angle())
    }
    pub fn linvel(&self) -> (f32, f32) {
        dispatch!(self, w => w.linvel())
    }
    pub fn angvel(&self) -> f32 {
        dispatch!(self, w => w.angvel())
    }
    // Has the engine's island manager put the ship to sleep? (Tests only —
    // the sim never reads it; see SimParams::ship_sleep.)
    pub fn is_sleeping(&self) -> bool {
        dispatch!(self, w => w.is_sleeping())
    }

    // ---- static colliders ----
    pub fn add_segment(&mut self, a: Vec2, b: Vec2, friction: f32) -> ColHandle {
        dispatch!(self, w => w.add_segment(a, b, friction))
    }
    // A convex hull of `pts` (local space) placed at (tx, ty) rotated by
    // `rot`; returns the handle plus the hull vertices read BACK from the
    // collider (local space) so rendering matches the collision shape.
    // None when the points span no hull.
    pub fn add_convex_hull(
        &mut self,
        pts: &[Vec2],
        tx: f32,
        ty: f32,
        rot: f32,
        friction: f32,
        restitution: f32,
    ) -> Option<(ColHandle, Vec<Vec2>)> {
        dispatch!(self, w => w.add_convex_hull(pts, tx, ty, rot, friction, restitution))
    }
    pub fn remove(&mut self, h: ColHandle) {
        dispatch!(self, w => w.remove(h))
    }
}

// ---------------------------------------------------------------------
// Legacy engine: rapier 0.23.1. FROZEN — see the module doc.
// ---------------------------------------------------------------------
pub mod legacy {
    use super::{ColHandle, ShipSpec};
    use glam::Vec2;
    use rapier_legacy::prelude::*;

    pub struct World {
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
    }

    impl World {
        pub fn new(spec: &ShipSpec) -> World {
            let mut bodies = RigidBodySet::new();
            let mut colliders = ColliderSet::new();

            let body = RigidBodyBuilder::dynamic()
                .translation(vector![0.0, spec.y0])
                .angular_damping(spec.angular_damping)
                .linear_damping(spec.linear_damping)
                .can_sleep(spec.can_sleep)
                .ccd_enabled(true)
                .build();
            let ship = bodies.insert(body);
            // Compound collider of three capsules tracing the 1.5× scaled
            // lander (CLAUDE.md "Physics notes"). Endpoints in scaled world
            // units.
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

            World {
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
                    dt: crate::sim::PHYSICS_DT,
                    num_solver_iterations: std::num::NonZeroUsize::new(8).unwrap(),
                    ..Default::default()
                },
                gravity: vector![0.0, spec.gravity_y],
                ship,
            }
        }

        pub fn step(&mut self) {
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
        }

        fn rb(&self) -> &RigidBody {
            &self.bodies[self.ship]
        }
        fn rb_mut(&mut self) -> &mut RigidBody {
            self.bodies.get_mut(self.ship).unwrap()
        }

        pub fn set_gravity_scale(&mut self, scale: f32) {
            self.rb_mut().set_gravity_scale(scale, true);
        }
        pub fn set_translation(&mut self, x: f32, y: f32) {
            self.rb_mut().set_translation(vector![x, y], true);
        }
        pub fn set_rotation_unchecked(&mut self, re: f32, im: f32) {
            // new_unchecked, NOT Rotation::new / from_complex: any
            // re-normalisation or angle round-trip would change the bits.
            self.rb_mut().set_rotation(
                Rotation::new_unchecked(rapier_legacy::na::Complex::new(re, im)),
                true,
            );
        }
        pub fn set_linvel(&mut self, vx: f32, vy: f32) {
            self.rb_mut().set_linvel(vector![vx, vy], true);
        }
        pub fn set_angvel(&mut self, w: f32) {
            self.rb_mut().set_angvel(w, true);
        }
        pub fn reset_forces(&mut self) {
            self.rb_mut().reset_forces(true);
        }
        pub fn reset_torques(&mut self) {
            self.rb_mut().reset_torques(true);
        }
        pub fn add_force(&mut self, fx: f32, fy: f32) {
            self.rb_mut().add_force(vector![fx, fy], true);
        }
        pub fn add_force_at_point(&mut self, fx: f32, fy: f32, px: f32, py: f32) {
            self.rb_mut().add_force_at_point(vector![fx, fy], point![px, py], true);
        }
        pub fn add_torque(&mut self, t: f32) {
            self.rb_mut().add_torque(t, true);
        }

        pub fn translation(&self) -> (f32, f32) {
            let t = self.rb().translation();
            (t.x, t.y)
        }
        pub fn rotation(&self) -> (f32, f32) {
            let r = *self.rb().rotation();
            (r.re, r.im)
        }
        pub fn angle(&self) -> f32 {
            self.rb().rotation().angle()
        }
        pub fn linvel(&self) -> (f32, f32) {
            let v = self.rb().linvel();
            (v.x, v.y)
        }
        pub fn angvel(&self) -> f32 {
            self.rb().angvel()
        }
        pub fn is_sleeping(&self) -> bool {
            self.rb().is_sleeping()
        }

        pub fn add_segment(&mut self, a: Vec2, b: Vec2, friction: f32) -> ColHandle {
            let h = self.colliders.insert(
                ColliderBuilder::segment(point![a.x, a.y], point![b.x, b.y])
                    .friction(friction)
                    .build(),
            );
            let (i, g) = h.into_raw_parts();
            ColHandle(i, g)
        }

        pub fn add_convex_hull(
            &mut self,
            pts: &[Vec2],
            tx: f32,
            ty: f32,
            rot: f32,
            friction: f32,
            restitution: f32,
        ) -> Option<(ColHandle, Vec<Vec2>)> {
            let pts: Vec<Point<f32>> = pts.iter().map(|p| point![p.x, p.y]).collect();
            let builder = ColliderBuilder::convex_hull(&pts)?;
            let handle = self.colliders.insert(
                builder
                    .translation(vector![tx, ty])
                    .rotation(rot)
                    .friction(friction)
                    .restitution(restitution)
                    .build(),
            );
            // Read the hull back so rendering matches the collider.
            let verts = self.colliders[handle]
                .shape()
                .as_convex_polygon()
                .map(|cp| cp.points().iter().map(|p| Vec2::new(p.x, p.y)).collect())
                .unwrap_or_else(|| pts.iter().map(|p| Vec2::new(p.x, p.y)).collect());
            let (i, g) = handle.into_raw_parts();
            Some((ColHandle(i, g), verts))
        }

        pub fn remove(&mut self, h: ColHandle) {
            self.colliders.remove(
                ColliderHandle::from_raw_parts(h.0, h.1),
                &mut self.island_manager,
                &mut self.bodies,
                false,
            );
        }
    }
}

// ---------------------------------------------------------------------
// Modern engine: rapier 0.35 (glam math via glamx). Ruleset 3 onwards.
// ---------------------------------------------------------------------
pub mod modern {
    use super::{ColHandle, ShipSpec};
    use glam::Vec2;
    use rapier2d::prelude::*;

    pub struct World {
        bodies: RigidBodySet,
        colliders: ColliderSet,
        physics_pipeline: PhysicsPipeline,
        island_manager: IslandManager,
        broad_phase: DefaultBroadPhase,
        narrow_phase: NarrowPhase,
        impulse_joints: ImpulseJointSet,
        multibody_joints: MultibodyJointSet,
        ccd_solver: CCDSolver,
        integration_params: IntegrationParameters,
        gravity: Vector,
        ship: RigidBodyHandle,
    }

    impl World {
        pub fn new(spec: &ShipSpec) -> World {
            let mut bodies = RigidBodySet::new();
            let mut colliders = ColliderSet::new();

            let body = RigidBodyBuilder::dynamic()
                .translation(Vector::new(0.0, spec.y0))
                .angular_damping(spec.angular_damping)
                .linear_damping(spec.linear_damping)
                .can_sleep(spec.can_sleep)
                .ccd_enabled(true)
                .build();
            let ship = bodies.insert(body);
            // Same three capsules as the legacy engine (CLAUDE.md "Physics
            // notes").
            for (a, b, r) in [
                ((0.0, 0.42), (0.0, -0.08), 0.26),
                ((-0.26, -0.30), (-0.33, -0.64), 0.09),
                ((0.26, -0.30), (0.33, -0.64), 0.09),
            ] {
                colliders.insert_with_parent(
                    ColliderBuilder::new(SharedShape::capsule(
                        Vector::new(a.0, a.1), Vector::new(b.0, b.1), r))
                        .restitution(0.2).build(),
                    ship, &mut bodies,
                );
            }

            World {
                bodies,
                colliders,
                physics_pipeline: PhysicsPipeline::new(),
                island_manager: IslandManager::new(),
                broad_phase: DefaultBroadPhase::new(),
                narrow_phase: NarrowPhase::new(),
                impulse_joints: ImpulseJointSet::new(),
                multibody_joints: MultibodyJointSet::new(),
                ccd_solver: CCDSolver::new(),
                integration_params: IntegrationParameters {
                    dt: crate::sim::PHYSICS_DT,
                    num_solver_iterations: 8,
                    ..Default::default()
                },
                gravity: Vector::new(0.0, spec.gravity_y),
                ship,
            }
        }

        pub fn step(&mut self) {
            // 0.27+: the query pipeline is ephemeral (derived from the
            // broad-phase on demand) and no longer a step argument.
            self.physics_pipeline.step(
                self.gravity,
                &self.integration_params,
                &mut self.island_manager,
                &mut self.broad_phase,
                &mut self.narrow_phase,
                &mut self.bodies,
                &mut self.colliders,
                &mut self.impulse_joints,
                &mut self.multibody_joints,
                &mut self.ccd_solver,
                &(),
                &(),
            );
        }

        fn rb(&self) -> &RigidBody {
            &self.bodies[self.ship]
        }
        fn rb_mut(&mut self) -> &mut RigidBody {
            self.bodies.get_mut(self.ship).unwrap()
        }

        pub fn set_gravity_scale(&mut self, scale: f32) {
            self.rb_mut().set_gravity_scale(scale, true);
        }
        pub fn set_translation(&mut self, x: f32, y: f32) {
            self.rb_mut().set_translation(Vector::new(x, y), true);
        }
        pub fn set_rotation_unchecked(&mut self, re: f32, im: f32) {
            // glamx Rot2 stores the unit complex as (re, im) like nalgebra's
            // UnitComplex did; the unchecked constructor keeps the bits.
            self.rb_mut().set_rotation(Rotation::from_cos_sin_unchecked(re, im), true);
        }
        pub fn set_linvel(&mut self, vx: f32, vy: f32) {
            self.rb_mut().set_linvel(Vector::new(vx, vy), true);
        }
        pub fn set_angvel(&mut self, w: f32) {
            self.rb_mut().set_angvel(w, true);
        }
        pub fn reset_forces(&mut self) {
            self.rb_mut().reset_forces(true);
        }
        pub fn reset_torques(&mut self) {
            self.rb_mut().reset_torques(true);
        }
        pub fn add_force(&mut self, fx: f32, fy: f32) {
            self.rb_mut().add_force(Vector::new(fx, fy), true);
        }
        pub fn add_force_at_point(&mut self, fx: f32, fy: f32, px: f32, py: f32) {
            self.rb_mut().add_force_at_point(Vector::new(fx, fy), Vector::new(px, py), true);
        }
        pub fn add_torque(&mut self, t: f32) {
            self.rb_mut().add_torque(t, true);
        }

        pub fn translation(&self) -> (f32, f32) {
            let t = self.rb().translation();
            (t.x, t.y)
        }
        pub fn rotation(&self) -> (f32, f32) {
            let r = self.rb().rotation();
            (r.re, r.im)
        }
        pub fn angle(&self) -> f32 {
            self.rb().rotation().angle()
        }
        pub fn linvel(&self) -> (f32, f32) {
            let v = self.rb().linvel();
            (v.x, v.y)
        }
        pub fn angvel(&self) -> f32 {
            self.rb().angvel()
        }
        pub fn is_sleeping(&self) -> bool {
            self.rb().is_sleeping()
        }

        pub fn add_segment(&mut self, a: Vec2, b: Vec2, friction: f32) -> ColHandle {
            let h = self.colliders.insert(
                ColliderBuilder::segment(Vector::new(a.x, a.y), Vector::new(b.x, b.y))
                    .friction(friction)
                    .build(),
            );
            let (i, g) = h.into_raw_parts();
            ColHandle(i, g)
        }

        pub fn add_convex_hull(
            &mut self,
            pts: &[Vec2],
            tx: f32,
            ty: f32,
            rot: f32,
            friction: f32,
            restitution: f32,
        ) -> Option<(ColHandle, Vec<Vec2>)> {
            let pts: Vec<Vector> = pts.iter().map(|p| Vector::new(p.x, p.y)).collect();
            let builder = ColliderBuilder::convex_hull(&pts)?;
            let handle = self.colliders.insert(
                builder
                    .translation(Vector::new(tx, ty))
                    .rotation(rot)
                    .friction(friction)
                    .restitution(restitution)
                    .build(),
            );
            let verts = self.colliders[handle]
                .shape()
                .as_convex_polygon()
                .map(|cp| cp.points().iter().map(|p| Vec2::new(p.x, p.y)).collect())
                .unwrap_or_else(|| pts.iter().map(|p| Vec2::new(p.x, p.y)).collect());
            let (i, g) = handle.into_raw_parts();
            Some((ColHandle(i, g), verts))
        }

        pub fn remove(&mut self, h: ColHandle) {
            self.colliders.remove(
                ColliderHandle::from_raw_parts(h.0, h.1),
                &mut self.island_manager,
                &mut self.bodies,
                false,
            );
        }
    }
}
