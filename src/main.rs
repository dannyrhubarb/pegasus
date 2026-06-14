use macroquad::prelude::*;
use rapier2d::prelude::*;

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

        // Draw box
        let pos = rigid_body_set[box_handle].translation();
        let bw = 0.5 * 2.0 * SCALE;
        let bh = 0.5 * 2.0 * SCALE;
        let (bx, by) = world_to_screen(pos.x - 0.5, pos.y + 0.5, sh);
        draw_rectangle(bx, by, bw, bh, RED);

        draw_text(
            &format!("y = {:.3} m   [press R to reset]   FPS: {}", pos.y, get_fps()),
            10.0, 24.0, 20.0, WHITE,
        );

        // Hold to apply upward force
        let rb = rigid_body_set.get_mut(box_handle).unwrap();
        rb.reset_forces(true);
        if is_mouse_button_down(MouseButton::Left) {
            rb.add_force(vector![0.0, 8.0], true);
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
