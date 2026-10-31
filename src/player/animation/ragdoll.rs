//! `ToggleRagdoll` swaps the KCC for physics: main bones get dynamic capsule bodies joined by
//! spherical joints, and their poses are written back over the animation each frame.
use super::*;
use bevy::app::AnimationSystems;
use bevy::ecs::system::ParamSet;
use bevy::math::Affine3A;
use bevy::platform::collections::HashMap;
use bevy::transform::{TransformSystems, helper::TransformHelper};
use std::f32::consts::{FRAC_PI_3, FRAC_PI_4};

mod knobs {
    /// Head has no tail bone to measure from
    pub const HEAD_LENGTH: f32 = 0.25;
    /// Player origin sits this far above the hips when standing back up
    pub const STAND_UP_OFFSET: f32 = 1.0;
}

/// (bone, tail bone defining capsule length, radius); parents listed before children
const PARTS: &[(&str, Option<&str>, f32)] = &[
    ("DEF-hips", Some("DEF-spine001"), 0.12),
    ("DEF-spine001", Some("DEF-spine002"), 0.11),
    ("DEF-spine002", Some("DEF-spine003"), 0.11),
    ("DEF-spine003", Some("DEF-neck"), 0.12),
    ("DEF-head", None, 0.1),
    ("DEF-upper_armL", Some("DEF-forearmL"), 0.05),
    ("DEF-forearmL", Some("DEF-handL"), 0.045),
    ("DEF-upper_armR", Some("DEF-forearmR"), 0.05),
    ("DEF-forearmR", Some("DEF-handR"), 0.045),
    ("DEF-thighL", Some("DEF-shinL"), 0.07),
    ("DEF-shinL", Some("DEF-footL"), 0.055),
    ("DEF-thighR", Some("DEF-shinR"), 0.07),
    ("DEF-shinR", Some("DEF-footR"), 0.055),
];

/// Parts collide with the world but not each other, so overlapping capsules don't explode
const RAGDOLL_LAYER: LayerMask = LayerMask(1 << 5);

pub fn plugin(app: &mut App) {
    app.add_observer(toggle_ragdoll).add_systems(
        PostUpdate,
        drive_bones
            .after(AnimationSystems)
            .before(TransformSystems::Propagate)
            .run_if(in_state(Screen::Gameplay)),
    );
}

/// Bone and tail entities for each of [`PARTS`], found once the model loads.
#[derive(Component)]
pub struct RagdollBones(Vec<(Entity, Option<Entity>)>);

impl RagdollBones {
    pub fn find(root: Entity, children_q: Query<&Children>, name_q: Query<&Name>) -> Option<Self> {
        PARTS
            .iter()
            .map(|(bone, tail, _)| {
                let bone = root.get_by_name(children_q, name_q, bone)?;
                let tail = match tail {
                    Some(tail) => Some(root.get_by_name(children_q, name_q, tail)?),
                    None => None,
                };
                Some((bone, tail))
            })
            .collect::<Option<_>>()
            .map(Self)
    }
}

/// Present while ragdolled; holds the removed controller to restore on stand-up.
#[derive(Component)]
struct Ragdoll {
    controller: CharacterController,
    parts: Vec<RagdollPart>,
    joints: Vec<Entity>,
}

struct RagdollPart {
    bone: Entity,
    body: Entity,
    half_len: f32,
    scale: Vec3,
}

fn toggle_ragdoll(
    on: On<Start<ToggleRagdoll>>,
    parents: Query<&ChildOf>,
    mut players: Query<(
        Option<&CharacterController>,
        Option<&Ragdoll>,
        &RagdollBones,
        &LinearVelocity,
        &mut CharacterControllerState,
    )>,
    mut transforms: ParamSet<(TransformHelper, Query<&mut Transform>)>,
    mut commands: Commands,
) -> Result {
    let player = on.context;
    let (controller, ragdoll, bones, velocity, mut state) = players.get_mut(player)?;

    match (ragdoll, controller) {
        (Some(ragdoll), _) => {
            *state = CharacterControllerState::default();
            stand_up(player, ragdoll, &mut transforms.p1(), &mut commands)
        }
        (None, Some(controller)) => {
            let ragdoll = go_limp(
                controller,
                bones,
                *velocity,
                &transforms.p0(),
                &parents,
                &mut commands,
            )?;
            if let Some(hips) = ragdoll.parts.first() {
                move_camera_target(player, hips.body, &mut commands);
            }
            commands
                .entity(player)
                .remove::<CharacterController>()
                .insert((ragdoll, RigidBodyDisabled, ColliderDisabled));
            Ok(())
        }
        (None, None) => Ok(()),
    }
}

fn go_limp(
    controller: &CharacterController,
    bones: &RagdollBones,
    velocity: LinearVelocity,
    helper: &TransformHelper,
    parents: &Query<&ChildOf>,
    commands: &mut Commands,
) -> Result<Ragdoll> {
    let mut parts = Vec::with_capacity(PARTS.len());
    let mut joints = Vec::new();
    let mut bodies_by_bone = HashMap::new();
    for (&(bone, tail), &(name, _, radius)) in bones.0.iter().zip(PARTS) {
        let (scale, rotation, head) = helper
            .compute_global_transform(bone)?
            .to_scale_rotation_translation();
        let len = match tail {
            Some(tail) => head.distance(helper.compute_global_transform(tail)?.translation()),
            None => knobs::HEAD_LENGTH,
        };
        let half_len = len / 2.0;

        let body = commands
            .spawn((
                Name::new(format!("Ragdoll {name}")),
                RigidBody::Dynamic,
                Collider::capsule(radius, (len - 2.0 * radius).max(0.01)),
                CollisionLayers::new(RAGDOLL_LAYER, !RAGDOLL_LAYER),
                Transform::from_translation(head + rotation * Vec3::Y * half_len)
                    .with_rotation(rotation),
                velocity,
                TransformInterpolation,
                DespawnOnExit(Screen::Gameplay),
            ))
            .id();

        // nearest ragdolled ancestor; bones in between (shoulders, neck) stay animated
        if let Some(parent_body) = parents
            .iter_ancestors(bone)
            .find_map(|a| bodies_by_bone.get(&a).copied())
        {
            let joint = commands
                .spawn((
                    SphericalJoint::new(parent_body, body)
                        .with_anchor(head)
                        .with_basis(rotation)
                        .with_swing_limits(-FRAC_PI_3, FRAC_PI_3)
                        .with_twist_limits(-FRAC_PI_4, FRAC_PI_4),
                    JointCollisionDisabled,
                    DespawnOnExit(Screen::Gameplay),
                ))
                .id();
            joints.push(joint);
        }

        bodies_by_bone.insert(bone, body);
        parts.push(RagdollPart {
            bone,
            body,
            half_len,
            scale,
        });
    }

    Ok(Ragdoll {
        controller: controller.clone(),
        parts,
        joints,
    })
}

/// Moves the player to where the hips landed and restores the KCC.
fn stand_up(
    player: Entity,
    Ragdoll {
        controller,
        parts,
        joints,
    }: &Ragdoll,
    transforms: &mut Query<&mut Transform>,
    commands: &mut Commands,
) -> Result {
    if let Some(hips) = parts.first() {
        move_camera_target(hips.body, player, commands);
    }
    let mut player_e = commands.entity(player);
    if let Some(hips) = parts.first() {
        let body = *transforms.get(hips.body)?;
        let head = body.translation - body.rotation * Vec3::Y * hips.half_len;
        let standing = head + Vec3::Y * knobs::STAND_UP_OFFSET;
        transforms.get_mut(player)?.translation = standing;
        // interpolation would otherwise ease back from the stale physics position
        player_e.insert(Position(standing));
    }
    player_e
        .insert((controller.clone(), LinearVelocity::ZERO))
        .remove::<(Ragdoll, RigidBodyDisabled, ColliderDisabled)>();
    for entity in parts.iter().map(|p| p.body).chain(joints.iter().copied()) {
        commands.entity(entity).despawn();
    }
    Ok(())
}

/// Camera crates follow their single target entity, so hand it between player and hips.
#[cfg_attr(
    not(any(feature = "third_person", feature = "top_down")),
    allow(unused_variables)
)]
fn move_camera_target(from: Entity, to: Entity, commands: &mut Commands) {
    #[cfg(feature = "third_person")]
    {
        commands.entity(from).remove::<ThirdPersonCameraTarget>();
        commands.entity(to).insert(ThirdPersonCameraTarget);
    }
    #[cfg(feature = "top_down")]
    {
        commands.entity(from).remove::<TopDownCameraTarget>();
        commands.entity(to).insert(TopDownCameraTarget);
    }
}

/// Overrides the animated pose of ragdolled bones with their bodies' world transforms.
fn drive_bones(
    ragdolls: Query<&Ragdoll>,
    parents: Query<&ChildOf>,
    mut transforms: Query<&mut Transform>,
) {
    for ragdoll in &ragdolls {
        let mut targets = HashMap::<Entity, Affine3A>::new();
        for part in &ragdoll.parts {
            let Ok(body) = transforms.get(part.body) else {
                continue;
            };
            let head = body.translation - body.rotation * Vec3::Y * part.half_len;
            let target = Affine3A::from_scale_rotation_translation(part.scale, body.rotation, head);

            let parent_global = parents.get(part.bone).map_or(Affine3A::IDENTITY, |p| {
                global(p.parent(), &targets, &parents, &transforms)
            });
            if let Ok(mut local) = transforms.get_mut(part.bone) {
                *local = Transform::from_matrix((parent_global.inverse() * target).into());
            }
            targets.insert(part.bone, target);
        }
    }
}

/// World transform of `entity`, using ragdoll targets already written this frame where available.
fn global(
    entity: Entity,
    targets: &HashMap<Entity, Affine3A>,
    parents: &Query<&ChildOf>,
    transforms: &Query<&mut Transform>,
) -> Affine3A {
    let mut chain = vec![];
    let mut base = Affine3A::IDENTITY;
    let mut current = Some(entity);
    while let Some(e) = current {
        if let Some(target) = targets.get(&e) {
            base = *target;
            break;
        }
        chain.push(e);
        current = parents.get(e).ok().map(ChildOf::parent);
    }
    chain.iter().rev().fold(base, |acc, &e| {
        acc * transforms
            .get(e)
            .map_or(Affine3A::IDENTITY, |t| t.compute_affine())
    })
}
