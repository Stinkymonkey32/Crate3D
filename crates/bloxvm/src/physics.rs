//! Rapier 3D physics: a static world built from scene parts, a blocky
//! avatar, and a simple third-person character controller.
//!
//! Distances are Roblox studs. The player is a dynamic capsule that is
//! locked upright and driven with a direct velocity model so movement
//! feels like Roblox (constant walkspeed, gravity, a single jump).
//!
//! The classic "disintegration" death is implemented too: on death the
//! capsule is removed and each avatar part becomes its own dynamic box
//! that is thrown apart and falls with gravity; the character respawns
//! after a short delay.

use std::f32::consts::TAU;

use glam::Vec3;
use rapier3d::prelude::*;

use crate::render::{RenderPart, Scene};

/// Roblox gravity, in studs per second squared.
const GRAVITY: f32 = 196.2;
/// Roblox default walkspeed, in studs per second.
const WALK_SPEED: f32 = 16.0;
/// Vertical launch speed of a jump, in studs per second.
const JUMP_SPEED: f32 = 50.0;
/// Character capsule geometry: cylinder half-height and radius.
const CAP_HALF_HEIGHT: f32 = 1.75;
const CAP_RADIUS: f32 = 0.9;
/// Extra downward reach for the grounded raycast.
const GROUNDED_SLACK: f32 = 0.2;

/// Walk cycle: radians of limb swing per stud travelled (≈1.5 Hz at 16 studs/s).
const WALK_FREQ: f32 = 0.6;
/// Max arm and leg swing angles while walking, in radians.
const ARM_SWING: f32 = 0.55;
const LEG_SWING: f32 = 0.45;

/// Classic Roblox BrickColors (linear-ish render values): torso "Bright blue",
/// head + arms "Bright yellow", legs "Bright yellowish green".
const TORSO_COLOR: [f32; 3] = [0.0, 0.635, 1.0];
const HEAD_COLOR: [f32; 3] = [0.961, 0.804, 0.188];
const LIMB_COLOR: [f32; 3] = [0.78, 0.824, 0.235];

/// Non-swinging avatar parts: (name, size, local offset from the feet, color).
/// The head sits slightly above the torso so its bottom face is not coplanar
/// with the torso top (which would z-fight).
const CORE_PARTS: [(&str, [f32; 3], [f32; 3], [f32; 3]); 2] = [
    ("Torso", [2.0, 2.0, 1.0], [0.0, 3.0, 0.0], TORSO_COLOR),
    ("Head", [1.0, 1.0, 1.0], [0.0, 4.55, 0.0], HEAD_COLOR),
];

/// Swinging limbs: (name, size, joint offset from the feet, color, swing
/// amplitude). Positive swing rotates the limb about its joint's X axis.
/// Arms hang from the torso top (shoulders), legs from the torso bottom
/// (hips), so arms span torso-top to hip-height and legs span hips to feet.
/// Arm joints sit just outside the torso (half-width 1.0) so the arms are
/// not buried inside it and share no coplanar faces.
const LIMBS: [(&str, [f32; 3], [f32; 3], [f32; 3], f32); 4] = [
    ("LeftArm", [1.0, 2.0, 1.0], [1.55, 4.0, 0.0], HEAD_COLOR, ARM_SWING),
    ("RightArm", [1.0, 2.0, 1.0], [-1.55, 4.0, 0.0], HEAD_COLOR, -ARM_SWING),
    ("LeftLeg", [1.0, 2.0, 1.0], [-0.45, 2.0, 0.0], LIMB_COLOR, -LEG_SWING),
    ("RightLeg", [1.0, 2.0, 1.0], [0.45, 2.0, 0.0], LIMB_COLOR, LEG_SWING),
];

/// Max yaw change per step when turning toward the movement direction,
/// in radians. Keeps the character's facing smooth instead of snapping.
const TURN_SPEED: f32 = 0.15;

/// Fixed timestep used by `world.step()` (seconds).
const STEP_TIME: f32 = 1.0 / 60.0;
/// Falling below this Y coordinate kills the character ("fell out of the world").
const KILL_Y: f32 = -50.0;
/// Seconds before a dead character respawns at the spawn point.
const RESPAWN_DELAY: f32 = 2.5;
/// Maximum horizontal scatter speed, in studs per second.
const SCATTER_SPEED: f32 = 14.0;

/// One avatar part that has been turned into its own dynamic body after death.
struct ScatterPart {
    handle: RigidBodyHandle,
    name: String,
    size: [f32; 3],
    color: [f32; 3],
}

/// Tiny xorshift64 PRNG for the disintegration randomness.
struct XorShift(u64);

impl XorShift {
    fn new() -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15);
        XorShift(seed | 1)
    }

    /// Uniform float in `[0, 1)`.
    fn next(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// The simulated world plus the player's character body.
pub struct Player {
    world: PhysicsWorld,
    body: RigidBodyHandle,
    collider: ColliderHandle,
    spawn: Vec3,
    grounded: bool,
    yaw: f32,
    /// Accumulated walk-cycle phase, in radians.
    phase: f32,
    /// 0..1 envelope so the limbs ease out when standing still.
    motion: f32,
    /// `true` while the avatar is disintegrated into scatter parts.
    dead: bool,
    /// The avatar parts flying around as individual bodies, when dead.
    scatter: Vec<ScatterPart>,
    /// Seconds until auto-respawn after death.
    respawn_timer: f32,
    /// Camera target while dead (the spot where the character died).
    death_pos: Vec3,
    /// Random source for the scatter impulse.
    rng: XorShift,
}

impl Player {
    /// Picks a spawn point above the scene's highest part.
    pub fn spawn_point(scene: &Scene) -> Vec3 {
        if let Some((min, max)) = scene.bounds {
            Vec3::new(
                (min[0] + max[0]) * 0.5,
                max[1] + (CAP_HALF_HEIGHT + CAP_RADIUS) + 1.0,
                (min[2] + max[2]) * 0.5,
            )
        } else {
            Vec3::new(0.0, 3.0, 0.0)
        }
    }

    /// Builds fixed colliders for every scene part and spawns the avatar.
    pub fn new(scene: &Scene, spawn: Vec3) -> Self {
        let mut world = PhysicsWorld::new();
        world.gravity = Vector::new(0.0, -GRAVITY, 0.0);

        for part in &scene.parts {
            let half = Vec3::from(part.size) * 0.5;
            let pose = Pose::from_rotation(rotation_from(&part.rotation));
            world.insert(
                RigidBodyBuilder::fixed()
                    .translation(Vector::new(part.position[0], part.position[1], part.position[2])),
                ColliderBuilder::cuboid(half.x, half.y, half.z)
                    .position(pose)
                    .friction(0.6),
            );
        }

        let mut player = Player {
            world,
            body: RigidBodyHandle::invalid(),
            collider: ColliderHandle::invalid(),
            spawn,
            grounded: false,
            yaw: 0.0,
            phase: 0.0,
            motion: 0.0,
            dead: false,
            scatter: Vec::new(),
            respawn_timer: 0.0,
            death_pos: spawn,
            rng: XorShift::new(),
        };
        player.spawn_character(spawn);
        player
    }

    /// Inserts the character capsule at `at` and resets the player state.
    /// Used both at construction and on respawn.
    fn spawn_character(&mut self, at: Vec3) {
        let (body, collider) = self.world.insert(
            RigidBodyBuilder::dynamic()
                .translation(Vector::new(at.x, at.y, at.z))
                .lock_rotations()
                .can_sleep(false),
            ColliderBuilder::capsule_y(CAP_HALF_HEIGHT, CAP_RADIUS).friction(0.2),
        );
        self.body = body;
        self.collider = collider;
        self.dead = false;
        self.yaw = 0.0;
        self.phase = 0.0;
        self.motion = 0.0;
        self.respawn_timer = 0.0;
    }

    /// Applies camera-relative horizontal movement for this frame.
    ///
    /// `right`/`forward` are -1..=1; `camera_yaw` is the orbit camera's yaw.
    pub fn set_move_input(&mut self, right: f32, forward: f32, camera_yaw: f32) {
        if self.dead {
            return;
        }
        let (sy, cy) = camera_yaw.sin_cos();
        // World direction the camera looks at, projected onto the ground.
        let fwd = Vec3::new(-cy, 0.0, -sy);
        // Screen-right direction from that view.
        let rgt = Vec3::new(sy, 0.0, -cy);
        let mut dir = fwd * forward + rgt * right;
        if dir.length_squared() > 1.0 {
            dir = dir.normalize();
        }

        let body = &mut self.world.bodies[self.body];
        let vy = body.linvel().y;
        body.set_linvel(Vector::new(dir.x * WALK_SPEED, vy, dir.z * WALK_SPEED), true);

        if dir.length_squared() > 1e-6 {
            // Smoothly turn to face the movement direction; when there's no
            // input, keep the current facing instead of snapping back.
            let target = (-dir.x).atan2(dir.z);
            self.yaw = turn_toward(self.yaw, target, TURN_SPEED);
        }
    }

    /// Starts a jump if the character is grounded.
    pub fn try_jump(&mut self) {
        if self.dead || !self.grounded {
            return;
        }
        let body = &mut self.world.bodies[self.body];
        let vel = body.linvel();
        body.set_linvel(Vector::new(vel.x, JUMP_SPEED, vel.z), true);
        self.grounded = false;
    }

    /// Refreshes the grounded state, advances the simulation one step, and
    /// updates the walk-cycle phase from the distance actually travelled.
    /// While dead, steps the scattering parts and auto-respawns when the
    /// delay elapses.
    pub fn step(&mut self) {
        if self.dead {
            self.world.step();
            self.respawn_timer -= STEP_TIME;
            if self.respawn_timer <= 0.0 {
                self.respawn();
            }
            return;
        }

        let before = self.world.bodies[self.body].translation();
        let filter = QueryFilter::default().exclude_collider(self.collider);
        self.grounded = self
            .world
            .cast_ray(
                &Ray::new(before, Vector::new(0.0, -1.0, 0.0)),
                CAP_HALF_HEIGHT + CAP_RADIUS + GROUNDED_SLACK,
                true,
                filter,
            )
            .is_some();
        self.world.step();

        let after = self.world.bodies[self.body].translation();
        if after.y < KILL_Y {
            self.die();
            return;
        }
        let hx = after.x - before.x;
        let hz = after.z - before.z;
        let moved = (hx * hx + hz * hz).sqrt();
        if moved > 0.001 {
            self.phase += moved * WALK_FREQ;
            self.motion = (self.motion + 0.05).min(1.0);
        } else {
            self.motion = (self.motion - 0.08).max(0.0);
        }
    }

    /// Classic disintegration: removes the character body and turns each
    /// avatar part into its own dynamic box thrown away from the body.
    pub fn die(&mut self) {
        if self.dead {
            return;
        }
        let parts = self.avatar_parts();
        let center = self.world.bodies[self.body].translation();
        self.death_pos = Vec3::new(center.x, center.y, center.z);

        self.world.remove_body(self.body);
        self.world.remove_collider(self.collider);

        for part in parts {
            let half = [part.size[0] * 0.5, part.size[1] * 0.5, part.size[2] * 0.5];
            let (handle, _) = self.world.insert(
                RigidBodyBuilder::dynamic()
                    .pose(Pose::from_rotation(rotation_from(&part.rotation)))
                    .translation(Vector::new(part.position[0], part.position[1], part.position[2]))
                    .linear_damping(0.3)
                    .angular_damping(0.8)
                    .can_sleep(true),
                ColliderBuilder::cuboid(half[0], half[1], half[2])
                    .friction(0.4)
                    .restitution(0.1),
            );

            let vx = (self.rng.next() * 2.0 - 1.0) * SCATTER_SPEED;
            let vz = (self.rng.next() * 2.0 - 1.0) * SCATTER_SPEED;
            let vy = 2.0 + self.rng.next() * 6.0;
            self.world.bodies[handle].set_linvel(Vector::new(vx, vy, vz), true);
            let spin = 2.0 + self.rng.next() * 8.0;
            self.world.bodies[handle].set_angvel(
                Vector::new(spin, spin * 0.7, spin * 1.3),
                true,
            );

            self.scatter.push(ScatterPart {
                handle,
                name: part.name,
                size: part.size,
                color: part.color,
            });
        }

        self.dead = true;
        self.respawn_timer = RESPAWN_DELAY;
    }

    /// Removes the scattered parts and spawns the character afresh.
    fn respawn(&mut self) {
        for sp in &self.scatter {
            self.world.remove_body(sp.handle);
        }
        self.scatter.clear();
        self.spawn_character(self.spawn);
    }

    /// Whether the avatar is currently disintegrated.
    pub fn is_dead(&self) -> bool {
        self.dead
    }

    /// Character world position and facing yaw (renderer convention). While
    /// dead, returns the spot where the character died so the camera stays.
    pub fn character_transform(&self) -> (Vec3, f32) {
        if self.dead {
            return (self.death_pos, self.yaw);
        }
        let pos = self.world.bodies[self.body].translation();
        (Vec3::new(pos.x, pos.y, pos.z), self.yaw)
    }

    /// Builds the blocky avatar parts at the character's current pose, or the
    /// scattering part bodies when dead.
    ///
    /// The torso and head rotate only with the character's yaw; the limbs
    /// additionally swing around their shoulder/hip joints so the box
    /// rotation is applied about the joint, not the part center.
    pub fn avatar_parts(&self) -> Vec<RenderPart> {
        if self.dead {
            return self.scatter_parts();
        }
        let (center, yaw) = self.character_transform();
        let feet = Vec3::new(center.x, center.y - (CAP_HALF_HEIGHT + CAP_RADIUS), center.z);
        let (s, c) = yaw.sin_cos();
        let yaw_rot = [c, 0.0, s, 0.0, 1.0, 0.0, -s, 0.0, c];
        let swing = self.phase.sin() * self.motion;

        let mut parts = Vec::with_capacity(CORE_PARTS.len() + LIMBS.len());

        for (name, size, local, color) in CORE_PARTS {
            parts.push(RenderPart {
                name: name.to_string(),
                position: [
                    feet.x + c * local[0] - s * local[2],
                    feet.y + local[1],
                    feet.z + s * local[0] + c * local[2],
                ],
                rotation: yaw_rot,
                size,
                color,
            });
        }

        for (name, size, joint, color, amp) in LIMBS {
            let angle = swing * amp;
            let (sa, ca) = angle.sin_cos();
            // M = R_yaw * R_x(angle); the limb center hangs 1 stud below its
            // joint, so M * (0, -1, 0) is the center's offset from the joint.
            let rotation = [c, 0.0, s, -s * sa, ca, c * sa, -s * ca, -sa, c * ca];
            let offset = [s * sa, -ca, -c * sa];
            parts.push(RenderPart {
                name: name.to_string(),
                // The joint anchors the limb to the body, so it must be
                // yaw-rotated with it before adding the (already world-space)
                // swing offset — otherwise limbs stay glued to the world.
                position: [
                    feet.x + c * joint[0] - s * joint[2] + offset[0],
                    feet.y + joint[1] + offset[1],
                    feet.z + s * joint[0] + c * joint[2] + offset[2],
                ],
                rotation,
                size,
                color,
            });
        }

        parts
    }

    /// Reads the current world transforms of the scattered part bodies.
    fn scatter_parts(&self) -> Vec<RenderPart> {
        let mut parts = Vec::with_capacity(self.scatter.len());
        for sp in &self.scatter {
            let body = &self.world.bodies[sp.handle];
            let t = body.translation();
            let (x, y, z, w) = {
                let q = *body.rotation();
                (q.x, q.y, q.z, q.w)
            };
            // Standard quaternion -> rotation matrix; columns are arranged in
            // the renderer's `RenderPart` order (same as `rotation_from`).
            let r00 = 1.0 - 2.0 * (y * y + z * z);
            let r11 = 1.0 - 2.0 * (x * x + z * z);
            let r22 = 1.0 - 2.0 * (x * x + y * y);
            let r01 = 2.0 * (x * y - z * w);
            let r10 = 2.0 * (x * y + z * w);
            let r02 = 2.0 * (x * z + y * w);
            let r20 = 2.0 * (x * z - y * w);
            let r12 = 2.0 * (y * z - x * w);
            let r21 = 2.0 * (y * z + x * w);
            parts.push(RenderPart {
                name: sp.name.clone(),
                position: [t.x, t.y, t.z],
                rotation: [r00, r10, r20, r01, r11, r21, r02, r12, r22],
                size: sp.size,
                color: sp.color,
            });
        }
        parts
    }
}

/// Rotates `current` toward `target` by at most `max_delta` radians, taking
/// the shortest way around the circle.
fn turn_toward(current: f32, target: f32, max_delta: f32) -> f32 {
    let mut delta = (target - current) % TAU;
    if delta > TAU * 0.5 {
        delta -= TAU;
    }
    if delta < -TAU * 0.5 {
        delta += TAU;
    }
    current + delta.abs().min(max_delta) * delta.signum()
}

/// Converts a part's row-major rotation matrix into a quaternion, using the
/// same column convention the renderer applies in `RenderPart`.
fn rotation_from(r: &[f32; 9]) -> Rotation {
    let m00 = r[0];
    let m01 = r[3];
    let m02 = r[6];
    let m10 = r[1];
    let m11 = r[4];
    let m12 = r[7];
    let m20 = r[2];
    let m21 = r[5];
    let m22 = r[8];

    let (w, x, y, z) = if m00 + m11 + m22 > 0.0 {
        let s = (m00 + m11 + m22 + 1.0).sqrt() * 2.0;
        (0.25 * s, (m21 - m12) / s, (m02 - m20) / s, (m10 - m01) / s)
    } else if m00 > m11 && m00 > m22 {
        let s = (m00 - m11 - m22 + 1.0).sqrt() * 2.0;
        ((m21 - m12) / s, 0.25 * s, (m01 + m10) / s, (m02 + m20) / s)
    } else if m11 > m22 {
        let s = (m11 - m00 - m22 + 1.0).sqrt() * 2.0;
        ((m02 - m20) / s, (m01 + m10) / s, 0.25 * s, (m12 + m21) / s)
    } else {
        let s = (m22 - m00 - m11 + 1.0).sqrt() * 2.0;
        ((m10 - m01) / s, (m02 + m20) / s, (m12 + m21) / s, 0.25 * s)
    };
    Rotation::from_xyzw(x, y, z, w)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn floor_scene() -> Scene {
        Scene {
            parts: vec![RenderPart {
                name: "Floor".into(),
                position: [0.0, 0.0, 0.0],
                rotation: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
                size: [20.0, 1.0, 20.0],
                color: [1.0, 1.0, 1.0],
            }],
            bounds: Some(([-10.0, -0.5, -10.0], [10.0, 0.5, 10.0])),
        }
    }

    #[test]
    fn player_falls_and_lands() {
        let scene = floor_scene();
        let mut player = Player::new(&scene, Vec3::new(0.0, 10.0, 0.0));
        for _ in 0..300 {
            player.step();
        }
        let (pos, _) = player.character_transform();
        let floor_top = 0.5;
        let rest_y = floor_top + CAP_HALF_HEIGHT + CAP_RADIUS;
        assert!(
            (pos.y - rest_y).abs() < 0.1,
            "expected to rest at y = {rest_y}, got y = {}",
            pos.y
        );
    }

    #[test]
    fn jump_when_grounded() {
        let scene = floor_scene();
        let mut player = Player::new(&scene, Vec3::new(0.0, 10.0, 0.0));
        for _ in 0..300 {
            player.step();
        }
        let (before, _) = player.character_transform();
        player.try_jump();
        player.step();
        let (after, _) = player.character_transform();
        assert!(
            after.y > before.y + 0.01,
            "jump should move up: {} -> {}",
            before.y,
            after.y
        );
    }

    #[test]
    fn horizontal_move() {
        let scene = floor_scene();
        let mut player = Player::new(&scene, Vec3::new(0.0, 10.0, 0.0));
        for _ in 0..120 {
            player.step();
        }
        let (start, _) = player.character_transform();
        player.set_move_input(1.0, 0.0, 0.0);
        for _ in 0..60 {
            player.step();
        }
        let (end, _) = player.character_transform();
        let dist = (end - start).length();
        assert!(dist > 0.5, "expected to move, travelled {dist} studs");
    }

    #[test]
    fn rotates_on_move() {
        let scene = floor_scene();
        let mut player = Player::new(&scene, Vec3::new(0.0, 10.0, 0.0));
        for _ in 0..120 {
            player.step();
        }
        player.set_move_input(0.0, 1.0, 0.0);
        let (_, yaw) = player.character_transform();
        assert_ne!(yaw, 0.0, "avatar should turn to face its movement direction");
    }

    #[test]
    fn avatar_has_six_parts() {
        let scene = floor_scene();
        let player = Player::new(&scene, Vec3::new(0.0, 10.0, 0.0));
        assert_eq!(player.avatar_parts().len(), 6);
    }

    #[test]
    fn limbs_swing_while_walking() {
        let scene = floor_scene();
        let mut player = Player::new(&scene, Vec3::new(0.0, 10.0, 0.0));
        for _ in 0..120 {
            player.step();
        }
        let rest = player.avatar_parts();
        player.set_move_input(1.0, 0.0, 0.0);
        for _ in 0..20 {
            player.step();
        }
        let moving = player.avatar_parts();
        assert_ne!(
            rest[2].position, moving[2].position,
            "the left arm should move while walking"
        );
    }

    #[test]
    fn limbs_rotate_with_body_yaw() {
        let scene = floor_scene();
        let mut player = Player::new(&scene, Vec3::new(0.0, 10.0, 0.0));
        for _ in 0..120 {
            player.step();
        }
        let rest = player.avatar_parts();
        player.set_move_input(0.0, 1.0, 0.0); // turn to face the movement direction
        for _ in 0..5 {
            player.step();
        }
        let turned = player.avatar_parts();
        let (_, yaw) = player.character_transform();
        assert!(yaw.abs() > 0.1, "expected the avatar to turn, yaw = {yaw}");

        // Arm offset from the torso in the XZ plane must rotate with the yaw.
        let arm0_x = rest[2].position[0] - rest[0].position[0];
        let arm0_z = rest[2].position[2] - rest[0].position[2];
        let arm1_x = turned[2].position[0] - turned[0].position[0];
        let arm1_z = turned[2].position[2] - turned[0].position[2];
        assert!(arm0_z.abs() < 1e-4, "at yaw 0 the arm is out on X, z = {arm0_z}");
        assert!(
            (arm1_x - arm0_x).abs() > 0.1 || (arm1_z - arm0_z).abs() > 0.1,
            "arm should swing with the body: ({arm0_x},{arm0_z}) -> ({arm1_x},{arm1_z})"
        );
    }

    #[test]
    fn arms_span_torso_to_hips() {
        let scene = floor_scene();
        let player = Player::new(&scene, Vec3::new(0.0, 10.0, 0.0));
        let parts = player.avatar_parts();
        let torso = &parts[0];
        let arm = &parts[2];
        let leg = &parts[4];
        assert_eq!(arm.position[1] + 1.0, torso.position[1] + 1.0, "arm top at torso top");
        assert_eq!(arm.position[1] - 1.0, leg.position[1] + 1.0, "arm bottom at leg top");
    }

    #[test]
    fn disintegrates_into_scatter_parts() {
        let scene = floor_scene();
        let mut player = Player::new(&scene, Vec3::new(0.0, 10.0, 0.0));
        for _ in 0..120 {
            player.step();
        }
        player.die();
        assert!(player.is_dead(), "should be dead after die()");
        let (death_pos, _) = player.character_transform();
        assert_eq!(player.avatar_parts().len(), 6, "scatter keeps all six parts");

        for _ in 0..10 {
            player.step();
        }
        let moved = player
            .avatar_parts()
            .iter()
            .any(|p| (Vec3::from(p.position) - death_pos).length() > 0.1);
        assert!(moved, "scatter parts should fly apart and move");
    }

    #[test]
    fn respawns_after_delay() {
        let scene = floor_scene();
        let mut player = Player::new(&scene, Vec3::new(0.0, 10.0, 0.0));
        for _ in 0..120 {
            player.step();
        }
        player.die();
        for _ in 0..200 {
            player.step();
        }
        assert!(!player.is_dead(), "should have respawned after the delay");
        let parts = player.avatar_parts();
        assert_eq!(parts.len(), 6);
        assert_eq!(parts[0].name, "Torso", "respawn should restore the rig");
        assert_eq!(parts[2].name, "LeftArm");
    }

    #[test]
    fn falling_out_of_world_kills() {
        let scene = floor_scene();
        let mut player = Player::new(&scene, Vec3::new(0.0, 10.0, 0.0));
        for _ in 0..120 {
            player.step();
        }
        let below = world_set_below_kill(&mut player);
        assert!(below, "helper should place the player below KILL_Y");
        player.step();
        assert!(player.is_dead(), "falling below KILL_Y should kill");
    }

    /// Teleports the character capsule below `KILL_Y` and reports whether it
    /// actually landed below it.
    fn world_set_below_kill(player: &mut Player) -> bool {
        player
            .world
            .bodies
            .get_mut(player.body)
            .map(|b| {
                b.set_translation(Vector::new(0.0, KILL_Y - 10.0, 0.0), true);
                b.translation().y < KILL_Y
            })
            .unwrap_or(false)
    }
}
