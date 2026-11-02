//! Main animation system
//!
//! My idea is based on tnua approach - detect if we are to alter the animation state
//! and then change the animation state accordingly
//!
//! Create a new animation graph for each player, start all animations with 0 weight and add weight
//! exponentially between frames based on input
use super::*;
use bevy::animation::{ActiveAnimation, AnimationTargetId};
use bevy::platform::collections::HashSet;
use bevy::world_serialization::WorldInstanceReady;
use ik::{foot::LegBones, hand::ArmBones, look::LookBones};

pub use ik::look::MAX_TWIST;
use ragdoll::RagdollBones;
use std::time::Duration;

mod ik;
mod ragdoll;

/// Animation control knobs
mod knobs {
    /// General damping factor to slow down animations using speed
    pub const DAMPING: f32 = 0.05;
    pub const TRANSITION: f32 = 0.2;
    /// Use a slightly faster transition for jumps to make them feel "snappy"
    pub const JUMP_TRANSITION: f32 = 0.1;
    /// Fraction of the idle-to-run speed that keeps running; riser bumps dip below the full one
    pub const STOP_SPEED_FACTOR: f32 = 0.25;
    pub const CAST_SPEED: f32 = 2.5;
    pub const CAST_BLEND_RATE: f32 = 12.0;
    /// Upper-body share the spell layer takes over locomotion
    pub const MAX_CAST_SHARE: f32 = 0.97;
}

pub fn plugin(app: &mut App) {
    app.add_plugins((ragdoll::plugin, ik::plugin)).add_systems(
        Update,
        (calcucate_animations, animate)
            .chain()
            .run_if(in_state(Screen::Gameplay)),
    );
}

/// Bone used as the spawn point for spell projectiles (see [`SpellShootFrame`]).
const HAND_BONE: &str = "DEF-handL";
/// Root of the bones the spell layer animates
const UPPER_BODY: &str = "DEF-spine001";
const LOWER_BODY_GROUP: u32 = 0;

/// Build animation graph when scene loads
pub fn prepare_animations(
    on: On<WorldInstanceReady>,
    models: Res<Models>,
    gltfs: Res<Assets<Gltf>>,
    children_q: Query<&Children>,
    name_q: Query<&Name>,
    targets_q: Query<&AnimationTargetId>,
    animation_player_q: Query<Entity, With<AnimationPlayer>>,
    mut animation_players: Query<&mut AnimationPlayer>,
    mut graphs: ResMut<Assets<AnimationGraph>>,
    mut players: Query<(Entity, &mut Player)>,
    mut commands: Commands,
) {
    let Some(animation_player_e) = on.entity.get_recursive(children_q, animation_player_q) else {
        return;
    };
    let hand = on.entity.get_by_name(children_q, name_q, HAND_BONE);
    let Ok(mut animation_player) = animation_players.get_mut(animation_player_e) else {
        return;
    };
    let Some(gltf) = gltfs.get(&models.player) else {
        return;
    };

    // we list acnimations here in the same order they are listed in AnimationState enum
    let clips = vec![
        gltf.named_animations["Idle_Loop"].clone(),
        gltf.named_animations["Jog_Fwd_Loop"].clone(),
        gltf.named_animations["Sprint_Loop"].clone(),
        gltf.named_animations["Jump_Start"].clone(),
        gltf.named_animations["Jump_Loop"].clone(),
        gltf.named_animations["Jump_Land"].clone(),
        gltf.named_animations["Crouch_Fwd_Loop"].clone(),
        gltf.named_animations["Crouch_Idle_Loop"].clone(),
        gltf.named_animations["Roll"].clone(),
        gltf.named_animations["Spell_Simple_Enter"].clone(),
        gltf.named_animations["Spell_Simple_Exit"].clone(),
        gltf.named_animations["Spell_Simple_Shoot"].clone(),
        gltf.named_animations["Walk_Loop"].clone(),
    ];

    let (mut graph, nodes) = AnimationGraph::from_clips(clips);
    // spell clips skip the lower body so legs keep running while casting
    if let Some(spine) = on.entity.get_by_name(children_q, name_q, UPPER_BODY) {
        let upper: HashSet<Entity> = children_q.iter_descendants(spine).chain([spine]).collect();
        for bone in children_q.iter_descendants(on.entity) {
            if !upper.contains(&bone)
                && let Ok(target) = targets_q.get(bone)
            {
                graph.add_target_to_mask_group(*target, LOWER_BODY_GROUP);
            }
        }
        for state in [
            AnimationState::SpellEnter,
            AnimationState::SpellShoot,
            AnimationState::SpellExit,
        ] {
            if let Some(node) = graph.get_mut(nodes[state.idx()]) {
                node.add_mask(1 << LOWER_BODY_GROUP);
            }
        }
    } else {
        warn!("could not find bone \"{UPPER_BODY}\" on player model; spells animate the full body");
    }
    let graph_handle = graphs.add(graph);

    commands
        .entity(animation_player_e)
        .insert(AnimationGraphHandle(graph_handle))
        .insert(AnimationTransitions::default());

    let idle_node = nodes[0];
    animation_player.play(idle_node).repeat();

    if let Ok((player_e, mut player)) = players.single_mut() {
        if let Some(arms) = ArmBones::find(on.entity, children_q, name_q) {
            commands.entity(player_e).insert(arms);
        } else {
            warn!("could not find arm bones on player model; wall run hand IK disabled");
        }
        if let Some(ragdoll) = RagdollBones::find(on.entity, children_q, name_q) {
            commands.entity(player_e).insert(ragdoll);
        } else {
            warn!("could not find ragdoll bones on player model; ragdoll disabled");
        }
        if let Some(legs) = LegBones::find(on.entity, children_q, name_q) {
            commands.entity(player_e).insert(legs);
        } else {
            warn!("could not find leg bones on player model; foot IK disabled");
        }
        if let Some(look) = LookBones::find(on.entity, children_q, name_q) {
            commands.entity(player_e).insert(look);
        } else {
            warn!("could not find spine bones on player model; look twist disabled");
        }
        debug!("adding animations to player: {}", player.id);
        player.animation = Animations {
            // state: (AnimationState::StandIdle, AnimationState::StandIdle),
            current: AnimationState::StandIdle,
            nodes,
            animation_player_e,
            ..Default::default()
        };
        if let Some(hand) = hand {
            player.hand = hand;
        } else {
            warn!(
                "could not find bone \"{HAND_BONE}\" on player model; spell casting will not spawn a projectile"
            );
        }
    }
}

pub fn calcucate_animations(
    cfg: Res<Config>,
    spatial: SpatialQuery,
    mut players: Query<(
        Entity,
        &CharacterController,
        &CharacterControllerState,
        &DashState,
        &WallRunState,
        &AirJumpState,
        &JumpTimer,
        &Transform,
        &LinearVelocity,
        &mut Player,
    )>,
    mut commands: Commands,
) {
    let idle_to_run_ani = cfg.player.movement.idle_to_run_threshold * 1000.0;

    for (
        entity,
        ahoy,
        ahoy_state,
        dash,
        wall_run,
        air_jump,
        jump_timer,
        pos,
        velocity,
        mut player,
    ) in players.iter_mut()
    {
        let animation = &mut player.animation;
        let air_jumped = air_jump.used > animation.air_jumps;
        animation.air_jumps = air_jump.used;
        let dash_started = dash.progress.is_some() && !animation.dashing;
        animation.dashing = dash.progress.is_some();

        // KCC velocity, unlike frame displacement, ignores step teleports and interpolation resets
        let h_speed = velocity.xz().length();
        let v_speed = velocity.y;

        // hysteresis so stair bumps don't flip Run to Idle and restart it
        let stop_speed = if animation.current.is_moving() {
            idle_to_run_ani * knobs::STOP_SPEED_FACTOR
        } else {
            idle_to_run_ani
        };
        let moving = h_speed > stop_speed;
        let grounded = ahoy_state.grounded.is_some();
        // tracing::debug!(
        //     "player speed: {h_speed}, idle to run threshold: {idle_to_run_ani}, grounded: {grounded}"
        // );

        // MANTLE
        if ahoy_state.mantle.is_some() {
            animation.request(AnimationState::Dash); // or Mantle
            continue;
        }

        if air_jumped {
            animation.air_jump(air_jump.used);
            continue;
        }

        if dash_started {
            animation.dash();
            continue;
        }
        if dash.progress.is_some() {
            animation.request(AnimationState::Dash);
            continue;
        }

        // hand reaching for the wall is layered on top in `ik::hand`
        if wall_run.progress.is_some() {
            animation.request(AnimationState::JumpLoop);
            continue;
        }

        // Handle stationary players: if speed is near zero and we're not moving up,
        // treat as grounded to avoid false jump animations when physics incorrectly reports ungrounded
        let treat_as_grounded =
            !grounded && h_speed < 0.1 && v_speed <= 0.1 && !animation.current.is_jumping();
        // stairs unground the KCC for a few frames; ground within a step below isn't a fall
        let HitboxPreset { radius, height } = &cfg.player.hitbox;
        let feet = height / 2.0 + radius;
        let near_ground = spatial
            .cast_ray(
                pos.translation,
                Dir3::NEG_Y,
                feet + ahoy.step_size,
                true,
                &SpatialQueryFilter::from_excluded_entities([entity]),
            )
            .is_some();
        // bounded by the timer since a blocked jump never lands to pause it
        let jumped = !jump_timer.is_paused() && !jump_timer.is_finished();
        let effectively_grounded = grounded || treat_as_grounded || !jumped && near_ground;

        // in the air animation
        if !effectively_grounded {
            if !animation.current.is_jumping() {
                if v_speed > 0.1 {
                    animation.request(AnimationState::Jump);
                } else {
                    animation.request(AnimationState::JumpLoop);
                }
            }
            continue;
        }

        // at this point we are GROUNDED
        // Only transition from jump to land/run - ignore if crouching
        if effectively_grounded && animation.current.is_jumping() && !ahoy_state.crouching {
            commands.entity(entity).trigger(PlayerLanded);
            if moving {
                animation.request(AnimationState::Run(ahoy.speed));
            } else {
                animation.request(AnimationState::Land);
            }
            continue;
        }

        // Also handle releasing crouch: if in crouch animation but not holding button, go to StandIdle
        if animation.current.is_crouching() && !ahoy_state.crouching {
            animation.request(AnimationState::StandIdle);
            continue;
        }

        // CROUCH - only request crouch if holding crouch button
        if ahoy_state.crouching && animation.current.can_crouch() {
            if moving {
                animation.request(AnimationState::Crouch(ahoy.speed));
            } else {
                animation.request(AnimationState::CrouchIdle);
            }
            continue;
        }

        // SPRINT
        let is_sprinting = moving && ahoy.speed > cfg.player.movement.speed();
        if is_sprinting {
            animation.request(AnimationState::Sprint(ahoy.speed));
            continue;
        }

        // WALK, toggled by lowering the controller speed
        if moving && ahoy.speed < cfg.player.movement.speed() {
            animation.request(AnimationState::Walk(ahoy.speed));
            continue;
        }

        // and finally RUN\IDLE
        if moving {
            animation.request(AnimationState::Run(ahoy.speed));
        } else {
            animation.request(AnimationState::StandIdle);
        }
    }
}

pub fn animate(
    time: Res<Time>,
    mut commands: Commands,
    mut players: Query<(Entity, &mut Player)>,
    mut animation_players: Query<&mut AnimationPlayer>,
    mut transitions_query: Query<&mut AnimationTransitions>,
) {
    for (entity, mut player) in players.iter_mut() {
        let ani = &mut player.animation;
        let Ok(mut animation_player) = animation_players.get_mut(ani.animation_player_e) else {
            continue;
        };

        let Ok(mut transitions) = transitions_query.get_mut(ani.animation_player_e) else {
            continue;
        };

        if let Some(next) = ani.requested.take() {
            let node = ani.nodes[next.idx()];

            let duration = if next.is_jumping() {
                knobs::JUMP_TRANSITION
            } else {
                knobs::TRANSITION
            };

            // debug!("animate: playing {:?} duration:{}", next, duration);

            transitions.play(
                &mut animation_player,
                node,
                Duration::from_secs_f32(duration),
            );

            if !next.is_locked() {
                animation_player.animation_mut(node).map(|a| a.repeat());
            }

            // Set speed on the NEW animation, not the old one
            let next_node = ani.nodes[next.idx()];
            if let Some(active) = animation_player.animation_mut(next_node) {
                match next {
                    AnimationState::Run(s)
                    | AnimationState::Sprint(s)
                    | AnimationState::Crouch(s) => active.set_speed(s * knobs::DAMPING),
                    AnimationState::Land | AnimationState::Dash => active.set_speed(2.5),
                    AnimationState::Jump => active.set_speed(2.0),
                    _ => active.set_speed(1.0),
                };
            }

            ani.current = next;
        }

        if ani.current.is_locked() {
            let node = ani.nodes[ani.current.idx()];

            if let Some(active) = animation_player.animation(node)
                && active.is_finished()
            {
                let chained = match ani.current {
                    AnimationState::Jump => Some(AnimationState::JumpLoop),
                    AnimationState::Land | AnimationState::Dash => Some(AnimationState::StandIdle),
                    _ => None,
                };

                if let Some(next) = chained {
                    // debug!("chained animation: {next:?}");
                    ani.requested = Some(next);
                }
            }
        }

        // spell layer: restarted per phase outside `transitions`, which only fades locomotion
        if let Some(next) = ani.cast_requested.take() {
            if let Some(prev) = ani.cast {
                animation_player.stop(ani.nodes[prev.idx()]);
            }
            animation_player
                .start(ani.nodes[next.idx()])
                .set_speed(knobs::CAST_SPEED);
            if next == AnimationState::SpellShoot {
                commands.entity(entity).trigger(SpellShootFrame);
            }
            ani.cast = Some(next);
        }
        let Some(phase) = ani.cast else {
            continue;
        };
        let node = ani.nodes[phase.idx()];
        let finished = animation_player
            .animation(node)
            .is_none_or(ActiveAnimation::is_finished);
        let chained = match phase {
            AnimationState::SpellEnter => Some(AnimationState::SpellShoot),
            AnimationState::SpellShoot if ani.casting => Some(AnimationState::SpellShoot),
            AnimationState::SpellShoot => Some(AnimationState::SpellExit),
            AnimationState::SpellExit if ani.casting => Some(AnimationState::SpellEnter),
            _ => None,
        };
        if finished {
            ani.cast_requested = chained;
        }
        let done = finished && chained.is_none();
        let goal = if done { 0.0 } else { 1.0 };
        ani.cast_weight +=
            (goal - ani.cast_weight) * (1.0 - (-knobs::CAST_BLEND_RATE * time.delta_secs()).exp());
        if done && ani.cast_weight < 0.01 {
            animation_player.stop(node);
            ani.cast = None;
            ani.cast_weight = 0.0;
        } else if let Some(active) = animation_player.animation_mut(node) {
            // blends average by weight and locomotion weights sum to 1
            let share = ani.cast_weight.min(knobs::MAX_CAST_SHARE);
            active.set_weight(share / (1.0 - share));
        }
    }
}

#[derive(Component, Reflect, Clone, Debug)]
#[reflect(Component)]
pub struct Animations {
    /// (current, next)
    // pub state: (AnimationState, AnimationState),
    pub current: AnimationState,
    pub requested: Option<AnimationState>,
    /// Spell phase on the upper-body layer, independent of `current`
    pub cast: Option<AnimationState>,
    pub cast_requested: Option<AnimationState>,
    /// Upper-body blend of the spell layer, 0..1
    pub cast_weight: f32,
    /// Cast button held: Shoot loops instead of chaining into Exit
    pub casting: bool,
    /// Last seen ahoy air jump count, to detect new air jumps
    pub air_jumps: u8,
    /// Whether ahoy reported a dash last frame, to detect new dashes
    pub dashing: bool,
    /// AnimationState → Graph node
    pub nodes: Vec<AnimationNodeIndex>,
    /// Entity that owns AnimationPlayer
    pub animation_player_e: Entity,
}

impl Default for Animations {
    fn default() -> Self {
        Self {
            // state: (AnimationState::StandIdle, AnimationState::StandIdle),
            current: AnimationState::StandIdle,
            requested: None,
            cast: None,
            cast_requested: None,
            cast_weight: 0.0,
            casting: false,
            air_jumps: 0,
            dashing: false,
            nodes: Vec::new(),
            animation_player_e: Entity::PLACEHOLDER,
        }
    }
}

/// State change helpers
impl Animations {
    fn request(&mut self, state: AnimationState) {
        let is_moving = self.current.is_moving();
        let is_jumping_to_land = self.current.is_jumping() && matches!(state, AnimationState::Land);
        let requested_is_movement = state.is_moving();
        let same_clip = self.current.idx() == state.idx();
        let is_locked = self.current.is_locked();

        // debug!(
        //     "request: curr={:?} next={:?} same_clip={}",
        //     self.current, state, same_clip
        // );

        if is_locked && !is_jumping_to_land && !is_moving && !requested_is_movement {
            return;
        }

        if same_clip {
            self.current = state;
        } else {
            self.requested = Some(state);
        }
    }

    pub fn idle(&mut self) {
        self.request(AnimationState::StandIdle);
    }

    pub fn run(&mut self, speed: f32) {
        self.request(AnimationState::Run(speed));
    }

    pub fn sprint(&mut self, speed: f32) {
        self.request(AnimationState::Sprint(speed));
    }

    pub fn crouch(&mut self, speed: f32) {
        self.request(AnimationState::Crouch(speed));
    }

    pub fn crouch_idle(&mut self) {
        self.request(AnimationState::CrouchIdle);
    }

    /// Bypasses the lock so a dash rolls even out of a still-playing jump or land clip
    pub fn dash(&mut self) {
        self.requested = Some(AnimationState::Dash);
    }

    pub fn fall(&mut self) {
        self.request(AnimationState::JumpLoop);
    }

    /// Bypasses the lock so each air jump restarts the jump clip, even mid-jump
    pub fn air_jump(&mut self, count: u8) {
        self.requested = Some(match count {
            // TODO: distinct clips per air jump, e.g. flip on the 2nd, spin on 3rd+
            1 => AnimationState::Jump,
            _ => AnimationState::Jump,
        });
    }

    pub fn start_jump(&mut self) {
        self.request(AnimationState::Jump);
    }
    pub fn end_jump(&mut self) {
        self.request(AnimationState::Land);
    }

    /// Starts the Enter→Shoot→Exit cast sequence; `animate` chains the rest, looping Shoot
    /// until [`Self::stop_casting`], and fires [`SpellShootFrame`] each time Shoot begins.
    pub fn cast_spell(&mut self) {
        self.casting = true;
        if self.cast.is_none() {
            self.cast_requested = Some(AnimationState::SpellEnter);
        }
    }
    pub fn stop_casting(&mut self) {
        self.casting = false;
    }
}

/// Fired on the player entity the instant the `Spell_Simple_Shoot` animation node starts
/// playing, so the projectile spawn (see `player::spell`) is synced to the cast animation.
#[derive(EntityEvent)]
pub struct SpellShootFrame(pub Entity);

/// The order is important here because we use it as indexes for animation node vec
#[derive(Component, Default, Reflect, Clone, Copy, PartialEq, Debug)]
#[reflect(Component)]
pub enum AnimationState {
    #[default]
    StandIdle,
    Run(f32),
    Sprint(f32),
    Jump,
    JumpLoop,
    Land,
    Crouch(f32),
    CrouchIdle,
    Dash,
    SpellEnter,
    SpellExit,
    SpellShoot,
    Walk(f32),
}
impl AnimationState {
    pub fn idx(&self) -> usize {
        match self {
            AnimationState::StandIdle => 0,
            AnimationState::Run(_) => 1,
            AnimationState::Sprint(_) => 2,
            AnimationState::Jump => 3,
            AnimationState::JumpLoop => 4,
            AnimationState::Land => 5,
            AnimationState::Crouch(_) => 6,
            AnimationState::CrouchIdle => 7,
            AnimationState::Dash => 8,
            AnimationState::SpellEnter => 9,
            AnimationState::SpellExit => 10,
            AnimationState::SpellShoot => 11,
            AnimationState::Walk(_) => 12,
        }
    }

    /// Animations that should not be interrupted by other animations
    pub fn is_locked(&self) -> bool {
        matches!(
            self,
            AnimationState::Jump | AnimationState::Land | AnimationState::Dash
        )
    }
    pub fn can_crouch(&self) -> bool {
        !matches!(
            self,
            AnimationState::Jump
                | AnimationState::JumpLoop
                | AnimationState::Land
                | AnimationState::Dash
                | AnimationState::SpellEnter
                | AnimationState::SpellExit
                | AnimationState::SpellShoot
        )
    }
    pub fn is_running(&self) -> bool {
        matches!(self, AnimationState::Run(_))
    }
    pub fn is_falling(&self) -> bool {
        matches!(self, AnimationState::JumpLoop)
    }
    pub fn is_jumping(&self) -> bool {
        matches!(self, AnimationState::Jump | AnimationState::JumpLoop)
    }
    pub fn is_crouching(&self) -> bool {
        matches!(self, AnimationState::Crouch(_) | AnimationState::CrouchIdle)
    }
    pub fn is_landing(&self) -> bool {
        matches!(self, AnimationState::Land)
    }
    pub fn is_moving(&self) -> bool {
        matches!(
            self,
            AnimationState::Run(_)
                | AnimationState::Walk(_)
                | AnimationState::Sprint(_)
                | AnimationState::Crouch(_)
        )
    }
}

#[cfg(test)]
mod animation_state_tests {
    use super::AnimationState;

    /// One state per animation graph node; `idx()` is used to index straight into that
    /// `Vec`, so every variant must map to a distinct index in `0..ALL.len()`.
    const ALL: &[AnimationState] = &[
        AnimationState::StandIdle,
        AnimationState::Run(1.0),
        AnimationState::Sprint(1.0),
        AnimationState::Jump,
        AnimationState::JumpLoop,
        AnimationState::Land,
        AnimationState::Crouch(1.0),
        AnimationState::CrouchIdle,
        AnimationState::Dash,
        AnimationState::SpellEnter,
        AnimationState::SpellExit,
        AnimationState::SpellShoot,
        AnimationState::Walk(1.0),
    ];

    #[test]
    fn idx_is_a_bijection_onto_0_len() {
        let mut seen = vec![false; ALL.len()];
        for state in ALL {
            let i = state.idx();
            assert!(i < ALL.len(), "{state:?}.idx() = {i} is out of range");
            assert!(
                !seen[i],
                "{state:?}.idx() = {i} collides with another variant"
            );
            seen[i] = true;
        }
    }

    #[test]
    fn locked_states_cannot_crouch() {
        // A locked animation (jump/land/dash) should never also report itself croachable.
        for state in ALL {
            if state.is_locked() {
                assert!(
                    !state.can_crouch(),
                    "{state:?} is locked but reports can_crouch()"
                );
            }
        }
    }

    #[test]
    fn only_jump_and_jumploop_are_jumping() {
        for state in ALL {
            let expected = matches!(state, AnimationState::Jump | AnimationState::JumpLoop);
            assert_eq!(
                state.is_jumping(),
                expected,
                "is_jumping() mismatch for {state:?}"
            );
        }
    }

    #[test]
    fn only_crouch_states_are_crouching() {
        for state in ALL {
            let expected = matches!(
                state,
                AnimationState::Crouch(_) | AnimationState::CrouchIdle
            );
            assert_eq!(
                state.is_crouching(),
                expected,
                "is_crouching() mismatch for {state:?}"
            );
        }
    }

    #[test]
    fn default_state_is_stand_idle() {
        assert_eq!(AnimationState::default(), AnimationState::StandIdle);
    }
}
