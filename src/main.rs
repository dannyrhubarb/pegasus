use rapier3d::prelude::*;

fn main() {
    let mut rigid_body_set = RigidBodySet::new();
    let mut collider_set = ColliderSet::new();

    // Ground plane
    let ground = ColliderBuilder::cuboid(10.0, 0.1, 10.0).build();
    collider_set.insert(ground);

    // Box falling from height 5.0
    let box_body = RigidBodyBuilder::dynamic()
        .translation(vector![0.0, 5.0, 0.0])
        .build();
    let box_handle = rigid_body_set.insert(box_body);

    let box_collider = ColliderBuilder::cuboid(0.5, 0.5, 0.5).build();
    collider_set.insert_with_parent(box_collider, box_handle, &mut rigid_body_set);

    // Physics pipeline setup
    let gravity = vector![0.0, -9.81, 0.0];
    let integration_params = IntegrationParameters::default();
    let mut physics_pipeline = PhysicsPipeline::new();
    let mut island_manager = IslandManager::new();
    let mut broad_phase = DefaultBroadPhase::new();
    let mut narrow_phase = NarrowPhase::new();
    let mut impulse_joint_set = ImpulseJointSet::new();
    let mut multibody_joint_set = MultibodyJointSet::new();
    let mut ccd_solver = CCDSolver::new();
    let mut query_pipeline = QueryPipeline::new();
    let physics_hooks = ();
    let event_handler = ();

    println!("Simulating box falling under gravity...");
    println!("{:<6} {:>10}", "Step", "Height (y)");
    println!("{}", "-".repeat(18));

    for step in 0..=100 {
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
            &physics_hooks,
            &event_handler,
        );

        if step % 10 == 0 {
            let pos = rigid_body_set[box_handle].translation();
            println!("{:<6} {:>10.4}", step, pos.y);
        }
    }

    let final_pos = rigid_body_set[box_handle].translation();
    println!(
        "\nFinal box position: ({:.4}, {:.4}, {:.4})",
        final_pos.x, final_pos.y, final_pos.z
    );
}
