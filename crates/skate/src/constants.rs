//! The physics constants, read from the game's own scripts (`PHYSICS.q`),
//! and the character stats that scale many of them.
//!
//! A stat-scaled constant is written `{ (min, max) [limit = n] STATS_X }`:
//! the value for stat `s` is `min + (max - min) * s / 10`, at most `limit`
//! (stats run past 10; the game's cheats set them to 16). Characters' stats
//! are in their profiles (in `disneytricks.q`): `Air`, `Ollie`, `speed`,
//! `spin` and so on, 5 by default (`Skater_Default_Stats`).
//!
//! Distances are in the game's units, which look like inches (the skater's
//! head is 77 up; speeds of 400-900 a second are skating speeds). Turning
//! rates are taken as radians a second; both readings are this crate's
//! assumptions until checked against the running game.

use qb::vm::Program;
use qb::{Value, checksum};

/// A character's stats, in the order of the `STATS_*` indices.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stats(pub [f32; 10]);

impl Default for Stats {
    fn default() -> Self {
        Stats([5.0; 10])
    }
}

/// The profile keys for each stat index (`STATS_AIR = 0`, ...).
const STAT_KEYS: [&str; 10] = [
    "Air",
    "hangtime",
    "Ollie",
    "speed",
    "spin",
    "flip_speed",
    "switch",
    "rail_balance",
    "lip_balance",
    "manual_balance",
];

impl Stats {
    /// The stats from the profile whose `Name` is `character` (such as
    /// `jessie`), searching every global value; the defaults if none.
    pub fn of(program: &Program, character: &str) -> Stats {
        let name = checksum(character);
        let mut found = None;
        for (_, value) in program.values() {
            find_profile(value, name, &mut found);
            if found.is_some() {
                break;
            }
        }
        let Some(profile) = found else {
            return Stats::default();
        };
        let mut stats = Stats::default();
        for (i, key) in STAT_KEYS.iter().enumerate() {
            if let Some(v) = profile.get(checksum(key)).and_then(Value::as_f32) {
                stats.0[i] = v;
            }
        }
        stats
    }
}

fn find_profile<'a>(value: &'a Value, name: u32, found: &mut Option<&'a Value>) {
    if found.is_some() {
        return;
    }
    match value {
        Value::Struct(items) => {
            let named = value.get(checksum("Name")).and_then(Value::as_name) == Some(name);
            if named && value.get(checksum("Air")).is_some() {
                *found = Some(value);
                return;
            }
            for (_, v) in items {
                find_profile(v, name, found);
            }
        }
        Value::Array(items) => {
            for v in items {
                find_profile(v, name, found);
            }
        }
        _ => {}
    }
}

/// The constants the skater uses, with stats applied.
#[derive(Clone, Debug, PartialEq)]
pub struct Physics {
    pub max_standing_kick_speed: f32,
    pub max_crouched_kick_speed: f32,
    pub standing_acceleration: f32,
    pub crouched_acceleration: f32,
    pub max_speed: f32,
    /// The hard limit on horizontal speed; between `max_speed` and this,
    /// `heavy_air_friction` drags the skater back.
    pub max_max_speed: f32,
    /// Ollie speed after a full crouch, and after the shortest one; in
    /// between it scales with the crouch time up to `max_tense_time`
    /// seconds (main.dol 0x800F62C4).
    pub jump_speed: f32,
    pub jump_speed_min: f32,
    pub max_tense_time: f32,
    /// Gravity in the air, before dividing by `air_hang` (the game does
    /// that: `main.dol` at 0x800FC634).
    pub air_gravity: f32,
    /// The hang-time stat's divisor for air gravity (1.0 at every stat on
    /// the US disc; vert air uses `Physics_Vert_hang_Stat`, 1.0-1.1).
    pub air_hang: f32,
    pub ground_gravity: f32,
    pub brake_acceleration: f32,
    pub rolling_friction: f32,
    /// Quadratic drag while pushing, standing or crouched, and above
    /// `max_speed`.
    pub standing_air_friction: f32,
    pub crouched_air_friction: f32,
    pub heavy_air_friction: f32,
    /// Turning on the ground, in radians a second; the sharp rate while
    /// braking (main.dol 0x800ED848).
    pub ground_rotation: f32,
    pub ground_sharp_rotation: f32,
    pub air_rotation: f32,
    pub ground_snap_up: f32,
    /// How far up a ledge the skater can be popped onto from the air
    /// (main.dol 0x800F81A8).
    pub air_snap_up: f32,
    /// The skater only sticks to ground whose normal is within this many
    /// degrees of the one it stands on, when the ground falls away
    /// (main.dol 0x800F52AC); otherwise it leaves the ground.
    pub ground_stick_angle: f32,
    pub ground_snap_down: f32,
    pub min_distance_to_wall: f32,
    /// The wall check on the ground (main.dol 0x800F7D38): a line this
    /// high along the skater's up, from where it was to where it's going
    /// and `forward_collision_length` further.
    pub forward_collision_height: f32,
    pub forward_collision_length: f32,
    /// Faces whose normal rises less than this many degrees above the
    /// horizontal are walls, unless their flags say otherwise
    /// (0x800F66F0).
    pub wall_non_skatable_angle: f32,
    /// Bouncing off a wall (0x800F6860): the skater turns by this times
    /// the angle it hit at, and hits more head-on than this many degrees
    /// lose speed (all of it head-on). Faster than the flail speed, it
    /// flails.
    pub wall_bounce_angle_multiplier: f32,
    pub wall_bounce_dont_slow_angle: f32,
    pub wall_bounce_dont_flail_speed: f32,
    pub head_height: f32,
    /// Grinding: how far a rail can be to get onto it, gravity along it,
    /// the boost getting on, the sharpest corner followed (degrees), how
    /// far an ollie off it turns with the steering (degrees), and how long
    /// after leaving a rail (seconds) before grinding again.
    pub rail_max_snap: f32,
    pub rail_gravity: f32,
    pub rail_speed_boost: f32,
    pub rail_corner_leave_angle: f32,
    pub rail_jump_angle: f32,
    pub regrind_time: f32,
    /// Chase camera: distance behind and height above (in feet, as the
    /// game's camera settings seem to be), and its horizontal FOV.
    pub camera_behind: f32,
    pub camera_above: f32,
    pub camera_fov: f32,
}

impl Physics {
    /// Reads the constants from `program` (which must have `PHYSICS.q`
    /// loaded), scaled by `stats`. Missing values fall back to the ones on
    /// the US disc.
    pub fn new(program: &Program, stats: &Stats) -> Physics {
        let scaled =
            |name: &str, fallback: f32| stat_value(program, name, stats).unwrap_or(fallback);
        let plain = |name: &str, fallback: f32| {
            program
                .value(checksum(name))
                .and_then(Value::as_f32)
                .unwrap_or(fallback)
        };
        let camera = program.value(checksum("Skater_Camera_Standard_Medium"));
        let camera_value = |key: &str, fallback: f32| {
            camera
                .and_then(|c| c.get(checksum(key)))
                .and_then(Value::as_f32)
                .unwrap_or(fallback)
        };
        Physics {
            max_standing_kick_speed: scaled("Skater_Max_Standing_Kick_Speed_Stat", 425.5),
            max_crouched_kick_speed: scaled("Skater_Max_Crouched_Kick_Speed_Stat", 575.0),
            standing_acceleration: scaled("Physics_Standing_Acceleration_Stat", 650.0),
            crouched_acceleration: scaled("Physics_Crouching_Acceleration_stat", 1100.0),
            max_speed: scaled("Skater_Max_Speed_Stat", 800.0),
            max_max_speed: scaled("Skater_Max_Max_Speed_Stat", 1000.0),
            jump_speed: scaled("Physics_Jump_Speed_Stat", 425.0),
            jump_speed_min: scaled("Physics_Jump_Speed_min_Stat", 350.0),
            max_tense_time: plain("Skater_max_tense_time", 200.0) / 1000.0,
            air_gravity: plain("Physics_Air_Gravity", -1350.0),
            air_hang: scaled("Physics_Air_hang_Stat", 1.0),
            ground_gravity: plain("Physics_Ground_Gravity", -1000.0),
            brake_acceleration: plain("Physics_Brake_Acceleration", 900.0),
            rolling_friction: plain("Physics_Rolling_Friction", 0.00001),
            standing_air_friction: plain("Physics_Standing_Air_Friction", 0.00001),
            crouched_air_friction: plain("Physics_Crouched_Air_Friction", 0.000002),
            heavy_air_friction: plain("Physics_Heavy_Air_Friction", 0.00001),
            ground_rotation: plain("Physics_Ground_Rotation", 1.8),
            ground_sharp_rotation: plain("Physics_Ground_Sharp_Rotation", 3.6),
            air_rotation: scaled("Physics_Air_Rotation_stat", 7.125),
            ground_snap_up: plain("Physics_Ground_Snap_Up", 13.0),
            air_snap_up: plain("Physics_Air_Snap_Up", 15.0),
            ground_stick_angle: plain("Ground_stick_angle", 30.0),
            ground_snap_down: plain("Physics_Ground_Snap_Down", 8.2),
            min_distance_to_wall: plain("Skater_Min_Distance_To_Wall", 8.0),
            forward_collision_height: plain("Skater_First_Forward_Collision_Height", 8.1),
            forward_collision_length: plain("Skater_First_Forward_Collision_Length", 10.0),
            wall_non_skatable_angle: plain("Wall_Non_Skatable_Angle", 25.0),
            wall_bounce_angle_multiplier: plain("Wall_Bounce_Angle_Multiplier", 1.1),
            wall_bounce_dont_slow_angle: plain("Wall_Bounce_Dont_Slow_Angle", 30.0),
            wall_bounce_dont_flail_speed: plain("Wall_Bounce_Dont_Flail_Speed", 100.0),
            head_height: plain("Skater_default_head_height", 77.0),
            rail_max_snap: plain("Rail_Max_Snap", 40.0),
            rail_gravity: plain("Physics_Rail_Gravity", -2000.0),
            rail_speed_boost: plain("Rail_Speed_Boost", 150.0),
            rail_corner_leave_angle: plain("Rail_Corner_Leave_Angle", 50.0),
            rail_jump_angle: plain("Rail_Jump_Angle", 15.0),
            regrind_time: plain("Skater_regrind_time", 500.0) / 1000.0,
            camera_behind: camera_value("behind", 12.0),
            camera_above: camera_value("above", 4.3),
            camera_fov: camera_value("horiz_fov", 72.0),
        }
    }
}

/// A `{ (min, max) [limit = n] STATS_X }` constant for these stats.
fn stat_value(program: &Program, name: &str, stats: &Stats) -> Option<f32> {
    let value = program.value(checksum(name))?;
    let Value::Struct(items) = value else {
        return value.as_f32();
    };
    let (min, max) = items.iter().find_map(|(k, v)| match (k, v) {
        (None, Value::Pair([a, b])) => Some((*a, *b)),
        _ => None,
    })?;
    // The stat is a bare name (STATS_SPEED) whose global value is its index.
    let stat = items
        .iter()
        .filter(|(k, _)| k.is_none())
        .find_map(|(_, v)| match v {
            Value::Name(n) => program.value(*n).and_then(Value::as_f32),
            _ => None,
        })
        .and_then(|i| stats.0.get(i as usize).copied())
        .unwrap_or(5.0);
    let mut v = min + (max - min) * stat / 10.0;
    if let Some(limit) = value.get(checksum("limit")).and_then(Value::as_f32) {
        v = v.min(limit);
    }
    Some(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_scale_constants_up_to_their_limit() {
        let mut program = Program::new();
        // STATS_SPEED = 3 / Speed_Stat = { (100.0, 200.0) limit = 180 STATS_SPEED }
        program.add_value(checksum("STATS_SPEED"), Value::Integer(3));
        program.add_value(
            checksum("Speed_Stat"),
            Value::Struct(vec![
                (None, Value::Pair([100.0, 200.0])),
                (Some(checksum("limit")), Value::Integer(180)),
                (None, Value::Name(checksum("STATS_SPEED"))),
            ]),
        );
        let mut stats = Stats::default();
        assert_eq!(stat_value(&program, "Speed_Stat", &stats), Some(150.0));
        stats.0[3] = 10.0;
        assert_eq!(stat_value(&program, "Speed_Stat", &stats), Some(180.0));
    }

    #[test]
    fn finds_a_characters_profile() {
        let mut program = Program::new();
        let profile = Value::Struct(vec![
            (Some(checksum("Name")), Value::Name(checksum("woody"))),
            (Some(checksum("Air")), Value::Integer(11)),
            (Some(checksum("spin")), Value::Integer(4)),
        ]);
        program.add_value(checksum("profiles"), Value::Array(vec![profile]));
        let stats = Stats::of(&program, "woody");
        assert_eq!(stats.0[0], 11.0);
        assert_eq!(stats.0[4], 4.0);
        assert_eq!(stats.0[3], 5.0);
        assert_eq!(Stats::of(&program, "buzz"), Stats::default());
    }
}
