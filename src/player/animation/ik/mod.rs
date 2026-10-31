//! Limb IK layered over the animated pose, with the shared two-bone solver.
use super::*;

pub mod foot;
pub mod hand;

pub fn plugin(app: &mut App) {
    app.add_plugins((hand::plugin, foot::plugin));
}

/// Analytic two-bone IK; returns local rotation deltas for the upper and lower bone.
/// `bend` picks the bend plane when the chain is straight, e.g. knees bend forward.
pub fn solve_two_bone(
    a: &GlobalTransform,
    b: &GlobalTransform,
    c: Vec3,
    t: Vec3,
    bend: Vec3,
) -> (Quat, Quat) {
    let (a_pos, a_rot) = (a.translation(), a.compute_transform().rotation);
    let (b_pos, b_rot) = (b.translation(), b.compute_transform().rotation);
    let angle = |u: Vec3, v: Vec3| {
        u.normalize_or_zero()
            .dot(v.normalize_or_zero())
            .clamp(-1.0, 1.0)
            .acos()
    };

    let lab = a_pos.distance(b_pos);
    let lcb = b_pos.distance(c);
    let lat = a_pos.distance(t).clamp(0.01, lab + lcb - 0.01);

    let ac_ab_0 = angle(c - a_pos, b_pos - a_pos);
    let ba_bc_0 = angle(a_pos - b_pos, c - b_pos);
    let ac_at_0 = angle(c - a_pos, t - a_pos);
    let ac_ab_1 = ((lcb * lcb - lab * lab - lat * lat) / (-2.0 * lab * lat))
        .clamp(-1.0, 1.0)
        .acos();
    let ba_bc_1 = ((lat * lat - lab * lab - lcb * lcb) / (-2.0 * lab * lcb))
        .clamp(-1.0, 1.0)
        .acos();

    let bent = (c - a_pos).cross(b_pos - a_pos);
    let axis0 = if bent.length_squared() > 1e-6 {
        bent
    } else {
        (c - a_pos).cross(bend)
    }
    .normalize_or(Vec3::X);
    let r0 = Quat::from_axis_angle(axis0, ac_ab_1 - ac_ab_0);
    let r1 = Quat::from_axis_angle(axis0, ba_bc_1 - ba_bc_0);
    let r2 = (c - a_pos)
        .cross(t - a_pos)
        .try_normalize()
        .map_or(Quat::IDENTITY, |axis1| {
            Quat::from_axis_angle(axis1, ac_at_0)
        });

    (
        a_rot.inverse() * r2 * r0 * a_rot,
        b_rot.inverse() * r1 * b_rot,
    )
}
