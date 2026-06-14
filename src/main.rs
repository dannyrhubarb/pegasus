use macroquad::prelude::*;
use rapier2d::prelude::*;
use std::sync::atomic::{AtomicU32, Ordering};

static TOUCH_THRUST: AtomicU32 = AtomicU32::new(0);
static TOUCH_TORQUE: AtomicU32 = AtomicU32::new(0);

#[unsafe(no_mangle)]
pub extern "C" fn set_touch_thrust(active: i32) {
    TOUCH_THRUST.store(active as u32, Ordering::Relaxed);
}

#[unsafe(no_mangle)]
pub extern "C" fn set_touch_torque(value: f32) {
    TOUCH_TORQUE.store(value.to_bits(), Ordering::Relaxed);
}

fn window_conf() -> Conf {
    Conf {
        window_title: "Rapier 2D — Box falls".to_string(),
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

const SCALE: f32 = 80.0; // pixels per meter

fn world_to_screen(x: f32, y: f32, screen_h: f32) -> (f32, f32) {
    // Flip Y: rapier Y goes up, screen Y goes down
    (x * SCALE + screen_width() / 2.0, screen_h - y * SCALE)
}

#[macroquad::main(window_conf)]
async fn main() {
    let mut rigid_body_set = RigidBodySet::new();
    let mut collider_set = ColliderSet::new();

    // Ground
    let ground_collider = ColliderBuilder::cuboid(5.0, 0.1).translation(vector![0.0, 0.0]).build();
    collider_set.insert(ground_collider);

    // Box starting high
    let box_body = RigidBodyBuilder::dynamic()
        .translation(vector![0.0, 5.0])
        .angular_damping(3.0)
        .build();
    let box_handle = rigid_body_set.insert(box_body);
    let box_collider = ColliderBuilder::cuboid(0.5, 0.5).restitution(0.4).build();
    collider_set.insert_with_parent(box_collider, box_handle, &mut rigid_body_set);

    let gravity = vector![0.0, -1.62];
    let mut integration_params = IntegrationParameters::default();
    let mut physics_pipeline = PhysicsPipeline::new();
    let mut island_manager = IslandManager::new();
    let mut broad_phase = DefaultBroadPhase::new();
    let mut narrow_phase = NarrowPhase::new();
    let mut impulse_joint_set = ImpulseJointSet::new();
    let mut multibody_joint_set = MultibodyJointSet::new();
    let mut ccd_solver = CCDSolver::new();
    let mut query_pipeline = QueryPipeline::new();

    loop {
        integration_params.dt = get_frame_time().min(0.05);
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

        clear_background(Color::from_rgba(20, 20, 30, 255));

        let sh = screen_height();

        // Draw ground
        let gw = 5.0 * 2.0 * SCALE;
        let gh = 0.1 * 2.0 * SCALE;
        let (gx, gy) = world_to_screen(-5.0, 0.1, sh);
        draw_rectangle(gx, gy, gw, gh, GRAY);

        // Draw box with rotation
        let body = &rigid_body_set[box_handle];
        let pos = body.translation();
        let angle = body.rotation().angle();
        let bw = 0.5 * 2.0 * SCALE;
        let bh = 0.5 * 2.0 * SCALE;
        let (cx, cy) = world_to_screen(pos.x, pos.y, sh);
        draw_rectangle_ex(cx, cy, bw, bh, DrawRectangleParams {
            offset: vec2(0.5, 0.5),
            rotation: -angle,
            color: RED,
        });

        // Triangle marker on the bottom face (local -Y = thrust direction)
        let rot = |lx: f32, ly: f32| -> (f32, f32) {
            let wx = pos.x + lx * angle.cos() - ly * angle.sin();
            let wy = pos.y + lx * angle.sin() + ly * angle.cos();
            world_to_screen(wx, wy, sh)
        };
        let (tx, ty) = rot(0.0, -0.65);
        let (lx, ly) = rot(-0.25, -0.45);
        let (rx, ry) = rot(0.25, -0.45);
        draw_triangle(vec2(tx, ty), vec2(lx, ly), vec2(rx, ry), YELLOW);

        draw_text(
            &format!("y = {:.3} m   [press R to reset]   FPS: {}", pos.y, get_fps()),
            10.0, 24.0, 20.0, WHITE,
        );

        let rb = rigid_body_set.get_mut(box_handle).unwrap();
        rb.reset_forces(true);
        rb.reset_torques(true);
        let thrusting = is_mouse_button_down(MouseButton::Left)
            || is_key_down(KeyCode::Down)
            || TOUCH_THRUST.load(Ordering::Relaxed) != 0;
        if thrusting {
            let angle = rb.rotation().angle();
            let force = vector![-angle.sin() * 8.0, angle.cos() * 8.0];
            rb.add_force(force, true);
        }
        let touch_torque = f32::from_bits(TOUCH_TORQUE.load(Ordering::Relaxed));
        if is_key_down(KeyCode::Left) {
            rb.add_torque(-1.0, true);
        } else if is_key_down(KeyCode::Right) {
            rb.add_torque(1.0, true);
        } else {
            rb.add_torque(touch_torque, true);
        }

        // Reset on R
        if is_key_pressed(KeyCode::R) {
            let rb = rigid_body_set.get_mut(box_handle).unwrap();
            rb.set_translation(vector![0.0, 5.0], true);
            rb.set_linvel(vector![0.0, 0.0], true);
            rb.set_angvel(0.0, true);
        }

        next_frame().await;
    }
}
