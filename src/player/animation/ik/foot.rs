//! Grounded foot IK: drops the hips to the lowest step under the feet, then a two-bone leg solve
//! over the animated pose plants each foot on the ground below it.
use super::*;
use bevy::app::AnimationSystems;
use bevy::ecs::system::ParamSet;
use bevy::transform::{TransformSystems, helper::TransformHelper};

mod knobs {
    /// Furthest a foot reaches up or down from the flat ground the animation assumes
    pub const MAX_STEP: f32 = 0.5;
    pub const BLEND_RATE: f32 = 10.0;
}

const HIPS: &str = "DEF-hips";
const LEFT_LEG: [&str; 3] = ["DEF-thighL", "DEF-shinL", "DEF-footL"];
const RIGHT_LEG: [&str; 3] = ["DEF-thighR", "DEF-shinR", "DEF-footR"];

pub fn plugin(app: &mut App) {
    app.add_systems(
        PostUpdate,
        plant_feet
            .after(AnimationSystems)
            .before(TransformSystems::Propagate)
            .run_if(in_state(Screen::Gameplay)),
    );
}

/// Model root (its origin is the ground the animation stands on), hips and thigh/shin/foot per side.
#[derive(Component)]
pub struct LegBones {
    root: Entity,
    hips: Entity,
    legs: [[Entity; 3]; 2],
    weight: f32,
}

impl LegBones {
    pub fn find(root: Entity, children_q: Query<&Children>, name_q: Query<&Name>) -> Option<Self> {
        let mut legs = [[Entity::PLACEHOLDER; 3]; 2];
        for (leg, names) in legs.iter_mut().zip([LEFT_LEG, RIGHT_LEG]) {
            for (bone, name) in leg.iter_mut().zip(names) {
                *bone = root.get_by_name(children_q, name_q, name)?;
            }
        }
        Some(Self {
            root,
            hips: root.get_by_name(children_q, name_q, HIPS)?,
            legs,
            weight: 0.0,
        })
    }
}

/// The ragdoll removes the KCC, so `With<CharacterController>` also keeps this off it.
fn plant_feet(
    time: Res<Time>,
    spatial: SpatialQuery,
    mut players: Query<
        (Entity, &CharacterControllerState, &Player, &mut LegBones),
        With<CharacterController>,
    >,
    mut transforms: ParamSet<(TransformHelper, Query<&mut Transform>)>,
) {
    for (player, state, Player { animation, .. }, mut bones) in &mut players {
        let planted = state.grounded.is_some()
            && !matches!(
                animation.current,
                AnimationState::Jump | AnimationState::JumpLoop | AnimationState::Dash
            );
        let goal = if planted { 1.0 } else { 0.0 };
        bones.weight +=
            (goal - bones.weight) * (1.0 - (-knobs::BLEND_RATE * time.delta_secs()).exp());
        if bones.weight < 0.001 {
            continue;
        }

        let LegBones {
            root,
            hips,
            legs,
            weight,
        } = *bones;
        let helper = transforms.p0();
        let Ok(root) = helper.compute_global_transform(root) else {
            continue;
        };
        let Ok(hips_global) = helper.compute_global_transform(hips) else {
            continue;
        };
        let Some(legs) = legs
            .iter()
            .map(|leg| {
                let [thigh, shin, foot] =
                    leg.map(|bone| helper.compute_global_transform(bone).ok());
                Some([thigh?, shin?, foot?])
            })
            .collect::<Option<Vec<_>>>()
        else {
            continue;
        };
        let filter = SpatialQueryFilter::from_excluded_entities([player]);
        let ground_below = |at: Vec3| {
            let origin = at + Vec3::Y * knobs::MAX_STEP;
            spatial
                .cast_ray(origin, Dir3::NEG_Y, 2.0 * knobs::MAX_STEP, true, &filter)
                .map(|hit| origin.y - hit.distance)
        };
        // ground under the body, which the KCC stands on and the animation treats as flat
        let Some(ground) = ground_below(root.translation()) else {
            continue;
        };

        // how far the real ground under each foot sits above the flat ground the animation assumes
        let rise: Vec<f32> = legs
            .iter()
            .map(|[.., foot]| ground_below(foot.translation()).map_or(0.0, |y| y - ground) * weight)
            .collect();
        // lower the hips so the foot on the lower step can still reach it
        let drop = Vec3::Y * rise[0].min(rise[1]).min(0.0);
        let knee_forward = root.rotation() * Vec3::Z;
        let lowered = |g: &GlobalTransform| {
            let mut affine = g.affine();
            affine.translation += Vec3A::from(drop);
            GlobalTransform::from(affine)
        };

        let deltas: Vec<_> = legs
            .iter()
            .zip(&rise)
            .map(|([thigh, shin, foot], rise)| {
                let target = foot.translation() + Vec3::Y * *rise;
                let ankle = foot.translation() + drop;
                solve_two_bone(&lowered(thigh), &lowered(shin), ankle, target, knee_forward)
            })
            .collect();

        let mut locals = transforms.p1();
        if let Ok(mut hips_local) = locals.get_mut(hips) {
            // world-space drop into the hips' parent space
            let to_parent = hips_local.compute_affine() * hips_global.affine().inverse();
            hips_local.translation += to_parent.transform_vector3(drop);
        }
        for ([thigh, shin, _], (thigh_delta, shin_delta)) in bones.legs.into_iter().zip(deltas) {
            for (bone, delta) in [(thigh, thigh_delta), (shin, shin_delta)] {
                if let Ok(mut local) = locals.get_mut(bone) {
                    local.rotation *= delta;
                }
            }
        }
    }
}
