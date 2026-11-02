//! Look twist over the animated pose: the body keeps facing its movement while the spine, neck and
//! head turn toward the camera, each bone taking a growing share so the head turns the most.
use super::*;
use crate::camera::SceneCamera;
use bevy::app::AnimationSystems;
use bevy::ecs::system::ParamSet;
use bevy::transform::{TransformSystems, helper::TransformHelper};
use std::f32::consts::{PI, TAU};

/// Looking further behind than this turns back to face forward instead of snapping sides
pub const MAX_TWIST: f32 = 2.0;

mod knobs {
    pub const BLEND_RATE: f32 = 8.0;
    /// Share of the twist per bone, waist to head; sums to 1 so the head faces the camera
    pub const WEIGHTS: [f32; 5] = [0.1, 0.15, 0.2, 0.25, 0.3];
}

const CHAIN: [&str; 5] = [
    "DEF-spine001",
    "DEF-spine002",
    "DEF-spine003",
    "DEF-neck",
    "DEF-head",
];

pub fn plugin(app: &mut App) {
    app.add_systems(
        PostUpdate,
        look_at_camera
            .after(AnimationSystems)
            .before(hand::touch_wall)
            .before(TransformSystems::Propagate)
            .run_if(in_state(Screen::Gameplay)),
    );
}

/// Spine-to-head chain and the current smoothed twist angle.
#[derive(Component)]
pub struct LookBones {
    chain: [Entity; 5],
    twist: f32,
}

impl LookBones {
    pub fn find(root: Entity, children_q: Query<&Children>, name_q: Query<&Name>) -> Option<Self> {
        let mut chain = [Entity::PLACEHOLDER; 5];
        for (bone, name) in chain.iter_mut().zip(CHAIN) {
            *bone = root.get_by_name(children_q, name_q, name)?;
        }
        Some(Self { chain, twist: 0.0 })
    }
}

/// The ragdoll removes the KCC, so `With<CharacterController>` also keeps this off it.
fn look_at_camera(
    time: Res<Time>,
    camera: Single<&GlobalTransform, With<SceneCamera>>,
    mut players: Query<(&GlobalTransform, &Player, &mut LookBones), With<CharacterController>>,
    mut transforms: ParamSet<(TransformHelper, Query<&mut Transform>)>,
) {
    let yaw = |dir: Vec3| dir.x.atan2(dir.z);
    let look = yaw(*camera.forward());
    for (body, Player { animation, .. }, mut bones) in &mut players {
        // model faces +Z
        let delta = (look - yaw(body.rotation() * Vec3::Z) + PI).rem_euclid(TAU) - PI;
        let goal = if delta.abs() > MAX_TWIST || animation.current == AnimationState::Dash {
            0.0
        } else {
            delta
        };
        bones.twist +=
            (goal - bones.twist) * (1.0 - (-knobs::BLEND_RATE * time.delta_secs()).exp());
        if bones.twist.abs() < 0.001 {
            continue;
        }

        let helper = transforms.p0();
        let Ok(globals) = bones
            .chain
            .map(|bone| helper.compute_global_transform(bone).map(|g| g.rotation()))
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
        else {
            continue;
        };
        // rotations about one axis commute, so each bone's pre-twist global is enough
        let mut locals = transforms.p1();
        for ((bone, global), weight) in bones.chain.iter().zip(globals).zip(knobs::WEIGHTS) {
            if let Ok(mut local) = locals.get_mut(*bone) {
                let turn = Quat::from_rotation_y(bones.twist * weight);
                local.rotation *= global.inverse() * turn * global;
            }
        }
    }
}
