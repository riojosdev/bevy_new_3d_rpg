//! Hand IK over the animated pose: on wall runs the wall-side hand takes the shortest path to the wall,
//! and while walking the hand nearest a wall briefly touches it every `hand_lock_distance`.
use super::*;
use bevy::app::AnimationSystems;
use bevy::ecs::system::ParamSet;
use bevy::transform::{TransformSystems, helper::TransformHelper};

mod knobs {
    pub const MAX_REACH: f32 = 1.5;
    /// Walking only reaches for walls about an arm's length away
    pub const WALK_REACH: f32 = 0.7;
    /// Walking touches start this far ahead of the shoulder, so the slower reach lands beside it
    pub const TOUCH_AHEAD: f32 = 0.5;
    /// Hits whose normal leans further from horizontal aren't walls
    pub const MAX_WALL_NORMAL_Y: f32 = 0.3;
    /// Keeps the palm just off the surface
    pub const WALL_OFFSET: f32 = 0.1;
    pub const BLEND_RATE: f32 = 12.0;
    /// Slower reach and release for walking touches
    pub const WALK_BLEND_RATE: f32 = 4.0;
}

const LEFT_ARM: [&str; 3] = ["DEF-upper_armL", "DEF-forearmL", "DEF-handL"];
const RIGHT_ARM: [&str; 3] = ["DEF-upper_armR", "DEF-forearmR", "DEF-handR"];

pub fn plugin(app: &mut App) {
    app.add_systems(
        PostUpdate,
        touch_wall
            .after(AnimationSystems)
            .before(TransformSystems::Propagate)
            .run_if(in_state(Screen::Gameplay)),
    );
}

/// Upper arm, forearm and hand bones per side, plus the IK blend state.
#[derive(Component)]
pub struct ArmBones {
    arms: [[Entity; 3]; 2],
    weight: f32,
    /// Arm index and world-space hand target, kept while blending out
    target: Option<(usize, Vec3)>,
    /// Player position when the walking hand last locked
    locked_at: Vec3,
    /// Arm picked when the wall run started, so hands don't swap mid-run
    wall_run_side: Option<usize>,
}

impl ArmBones {
    pub fn find(root: Entity, children_q: Query<&Children>, name_q: Query<&Name>) -> Option<Self> {
        let mut arms = [[Entity::PLACEHOLDER; 3]; 2];
        for (arm, names) in arms.iter_mut().zip([LEFT_ARM, RIGHT_ARM]) {
            for (bone, name) in arm.iter_mut().zip(names) {
                *bone = root.get_by_name(children_q, name_q, name)?;
            }
        }
        Some(Self {
            arms,
            weight: 0.0,
            target: None,
            locked_at: Vec3::ZERO,
            wall_run_side: None,
        })
    }
}

pub(super) fn touch_wall(
    time: Res<Time>,
    cfg: Res<Config>,
    spatial: SpatialQuery,
    mut players: Query<(
        Entity,
        &Position,
        &LinearVelocity,
        &CharacterControllerState,
        &WallRunState,
        &Player,
        &mut ArmBones,
    )>,
    mut transforms: ParamSet<(TransformHelper, Query<&mut Transform>)>,
) {
    let lock_distance = cfg.player.movement.hand_lock_distance;
    for (player, position, velocity, state, wall_run, Player { animation, .. }, mut bones) in
        &mut players
    {
        let Some(shoulders) = shoulders(&bones, &transforms.p0()) else {
            continue;
        };
        let filter = SpatialQueryFilter::from_excluded_entities([player]);
        // (distance, hand target) on a wall along `dir`
        let touch = |from: Vec3, dir: Vec3, reach: f32| {
            let hit = spatial.cast_ray(from, Dir3::new(dir).ok()?, reach, true, &filter)?;
            (hit.normal.y.abs() < knobs::MAX_WALL_NORMAL_Y).then(|| {
                let target = from + dir * hit.distance + hit.normal * knobs::WALL_OFFSET;
                (hit.distance, target)
            })
        };
        let walking =
            state.grounded.is_some() && matches!(animation.current, AnimationState::Walk(_));

        let reach = if let Some(progress) = &wall_run.progress {
            let into = -*progress.normal;
            let side = *bones
                .wall_run_side
                .get_or_insert(usize::from(shoulders[1].dot(into) > shoulders[0].dot(into)));
            // closest point on the wall, recomputed every frame so the hand slides along it
            let shoulder = shoulders[side];
            spatial
                .project_point(shoulder, true, &filter)
                .filter(|hit| hit.point.distance(shoulder) < knobs::MAX_REACH)
                .map(|hit| {
                    let out = (shoulder - hit.point).normalize_or(-into);
                    (side, hit.point + out * knobs::WALL_OFFSET)
                })
        } else if walking {
            bones.wall_run_side = None;
            if position.distance(bones.locked_at) < lock_distance {
                // hold the touch until the body walks past it, then the animation takes over
                bones.target.filter(|(side, target)| {
                    shoulders[*side].distance(*target) < knobs::WALK_REACH + knobs::TOUCH_AHEAD
                })
            } else {
                let ahead = velocity.with_y(0.0).normalize_or_zero() * knobs::TOUCH_AHEAD;
                let outward = (shoulders[0] - shoulders[1])
                    .with_y(0.0)
                    .normalize_or_zero();
                let touched = [(0, outward), (1, -outward)]
                    .into_iter()
                    .filter_map(|(side, dir)| {
                        let (distance, target) =
                            touch(shoulders[side] + ahead, dir, knobs::WALK_REACH)?;
                        Some((distance, (side, target)))
                    })
                    .min_by(|(a, _), (b, _)| a.total_cmp(b))
                    .map(|(_, reach)| reach);
                if touched.is_some() {
                    bones.locked_at = position.0;
                }
                touched
            }
        } else {
            bones.wall_run_side = None;
            None
        };
        let goal = if reach.is_some() { 1.0 } else { 0.0 };
        if reach.is_some() {
            bones.target = reach;
        }
        let rate = if wall_run.progress.is_some() {
            knobs::BLEND_RATE
        } else {
            knobs::WALK_BLEND_RATE
        };
        bones.weight += (goal - bones.weight) * (1.0 - (-rate * time.delta_secs()).exp());

        let Some((side, target)) = bones.target else {
            continue;
        };
        if bones.weight < 0.001 {
            bones.target = None;
            continue;
        }

        let [upper, fore, hand] = bones.arms[side];
        let helper = transforms.p0();
        let (Ok(a), Ok(b), Ok(c)) = (
            helper.compute_global_transform(upper),
            helper.compute_global_transform(fore),
            helper.compute_global_transform(hand),
        ) else {
            continue;
        };
        // a straight arm bends around a horizontal axis
        let (upper_delta, fore_delta) = solve_two_bone(&a, &b, c.translation(), target, Vec3::Y);

        let mut locals = transforms.p1();
        for (bone, delta) in [(upper, upper_delta), (fore, fore_delta)] {
            if let Ok(mut local) = locals.get_mut(bone) {
                local.rotation *= Quat::IDENTITY.slerp(delta, bones.weight);
            }
        }
    }
}

fn shoulders(bones: &ArmBones, helper: &TransformHelper) -> Option<[Vec3; 2]> {
    let [left, right] = bones.arms.map(|[upper, ..]| {
        helper
            .compute_global_transform(upper)
            .ok()
            .map(|g| g.translation())
    });
    Some([left?, right?])
}
