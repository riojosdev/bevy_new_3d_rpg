use super::*;
use crate::player::Player;
use avian3d::prelude::RigidBody;
use bevy::input::mouse::AccumulatedMouseScroll;
use bevy_ahoy::prelude::CharacterControllerCameraOf;
use bevy_third_person_camera::{CameraSyncSet, ThirdPersonCamera, ThirdPersonCameraTarget, Zoom};

mod knobs {
    /// Zooming in to this radius hands the camera to ahoy's first person view
    pub const FIRST_PERSON_RADIUS: f32 = 0.1;
    /// Radius restored when scrolling out of first person; above the switch radius
    pub const LEAVE_FIRST_PERSON_RADIUS: f32 = 0.5;
}

pub fn plugin(app: &mut App) {
    app.add_systems(OnEnter(Screen::Gameplay), add_tpv_cam)
        .add_systems(OnExit(Screen::Gameplay), rm_tpv_cam)
        .add_systems(
            PostUpdate,
            enter_first_person
                .after(CameraSyncSet)
                .run_if(in_state(Screen::Gameplay)),
        )
        .add_systems(
            Update,
            leave_first_person.run_if(in_state(Screen::Gameplay)),
        );
}

fn add_tpv_cam(
    cfg: Res<Config>,
    mut commands: Commands,
    mut camera: Query<Entity, With<SceneCamera>>,
    mut tpv_cam: Query<Entity, With<ThirdPersonCamera>>,
) -> Result {
    let Ok(cam) = camera.single_mut() else {
        return Ok(());
    };
    if tpv_cam.single_mut().is_ok() {
        debug!("Tried to add ThirdPersonCamera to an entiry that already has it");
        return Ok(());
    }

    let (min, max) = cfg.player.zoom;
    let mut zoom = Zoom::new(min, max);
    // lets zooming in reach `FIRST_PERSON_RADIUS`
    zoom.min = 0.0;
    commands.entity(cam).insert((
        ThirdPersonCamera {
            // aim_speed: 3.0,
            // aim_zoom: 0.7,
            // aim_enabled: true,
            zoom_enabled: true,
            zoom,
            offset_enabled: true,
            offset_toggle_enabled: true,
            cursor_lock_key: KeyCode::KeyL,
            // bounds: vec![Bound::NO_FLIP, Bound::ABOVE_FLOOR],
            ..default()
        },
        Projection::from(PerspectiveProjection {
            fov: cfg.player.fov.to_radians(),
            ..Default::default()
        }),
    ));

    Ok(())
}

fn rm_tpv_cam(
    mut commands: Commands,
    mut camera: Query<
        Entity,
        (
            With<SceneCamera>,
            Or<(With<ThirdPersonCamera>, With<CharacterControllerCameraOf>)>,
        ),
    >,
) {
    if let Ok(camera) = camera.single_mut() {
        commands
            .entity(camera)
            .remove::<(RigidBody, ThirdPersonCamera, CharacterControllerCameraOf)>();
    }
}

/// `Zoom::radius` is private, but the camera sits at `radius` along its own back axis from the
/// target (the offset is perpendicular to it), so measure it after the camera crate syncs.
fn enter_first_person(
    camera: Single<(Entity, &Transform), (With<ThirdPersonCamera>, Without<Player>)>,
    target: Single<&Transform, (With<ThirdPersonCameraTarget>, Without<ThirdPersonCamera>)>,
    player: Single<Entity, With<Player>>,
    mut commands: Commands,
) {
    let (camera, cam_transform) = *camera;
    let radius = (cam_transform.translation - target.translation).dot(*cam_transform.back());
    if radius <= knobs::FIRST_PERSON_RADIUS {
        commands
            .entity(camera)
            .remove::<ThirdPersonCamera>()
            .insert(CharacterControllerCameraOf::new(*player));
    }
}

fn leave_first_person(
    cfg: Res<Config>,
    scroll: Res<AccumulatedMouseScroll>,
    camera: Single<Entity, With<CharacterControllerCameraOf>>,
    mut commands: Commands,
) {
    // matches the camera crate: negative scroll zooms out
    if scroll.delta.y >= 0.0 {
        return;
    }
    commands
        .entity(*camera)
        .remove::<CharacterControllerCameraOf>();
    commands.run_system_cached(add_tpv_cam);
    let max = cfg.player.zoom.1;
    commands.queue(move |world: &mut World| {
        let Ok(mut cam) = world.query::<&mut ThirdPersonCamera>().single_mut(world) else {
            return;
        };
        // `Zoom::new` starts at the midpoint, so this lands on the leave radius
        cam.zoom = Zoom::new(0.0, 2.0 * knobs::LEAVE_FIRST_PERSON_RADIUS);
        cam.zoom.max = max;
    });
}
