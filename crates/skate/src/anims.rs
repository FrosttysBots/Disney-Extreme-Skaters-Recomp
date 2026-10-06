//! Which animation the skater plays, as the game's scripts pick them
//! (`TRICKS.q`): `OnGroundAI` on the ground (turning, crouching, pushing,
//! coasting), `GroundGone` and `Airborne` in the air, and `Land2` on
//! landing. Tricks, grinds, manuals and lips pick their own (see the
//! skater's `trick_pose`, `lip_pose` and balance meter).

use crate::skater::{Action, Skater};

/// An animation to show: `first` plays once (from `time`), then `then`
/// loops; or `first` loops itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Anim {
    pub first: &'static str,
    pub then: Option<&'static str>,
    pub looping: bool,
}

impl Anim {
    const fn cycle(name: &'static str) -> Anim {
        Anim {
            first: name,
            then: None,
            looping: true,
        }
    }

    const fn once(name: &'static str) -> Anim {
        Anim {
            first: name,
            then: None,
            looping: false,
        }
    }

    const fn into(first: &'static str, then: &'static str) -> Anim {
        Anim {
            first,
            then: Some(then),
            looping: false,
        }
    }
}

/// How the skater came down (for `Land2`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Landing {
    /// Facing against the way it was going (`Backwards`).
    pub backwards: bool,
    /// Under 0.2 seconds in the air (`LittleAir`).
    pub little_air: bool,
    /// 45 to 60 degrees off the way it was going after long enough in
    /// the air (`YawBetween (45, 60)`): the "Sketchy" landing.
    pub sketchy: bool,
    /// Under half a second in the air (`PlayLandAnim` bumps).
    pub short: bool,
}

/// The animation for what the skater is doing.
pub fn choose(skater: &Skater) -> Anim {
    let turn = skater.turn_input();
    let crouched = skater.is_crouched();
    match skater.action {
        Action::BailManual => Anim::into("BailManual", "BailManualGetUp"),
        Action::BailGrind => Anim::into("BailGrind", "BailGrindGetUp"),
        Action::Bail => Anim::into("Bail1", "BailGetUp1"),
        Action::FlailLeft if crouched => Anim::once("CrouchFlailLeft"),
        Action::FlailRight if crouched => Anim::once("CrouchFlailRight"),
        Action::FlailLeft => Anim::once("StandFlailLeft"),
        Action::FlailRight => Anim::once("StandFlailRight"),
        Action::Grinding => Anim::into("GrindIn1", "GrindRange1"),
        Action::Manual if skater.special_manual => {
            Anim::into("SpecialManualIn", "SpecialManualRange")
        }
        Action::Manual => Anim::into("ManualIn1", "ManualRange1"),
        Action::Lip => Anim::once("LipRange1"),
        // `Land2`.
        Action::Landing => {
            let l = skater.landing;
            if l.backwards {
                Anim::into("LandBackward1", "StandIdle")
            } else if l.sketchy {
                Anim::into("LandSketchy", "StandIdle")
            } else if l.little_air {
                if crouched {
                    Anim::into("CrouchBumpDown", "CrouchIdle")
                } else {
                    Anim::into("LandSmall", "StandIdle")
                }
            } else if l.short {
                Anim::into("CrouchBumpDown", "StandIdle")
            } else {
                Anim::into("Land1", "StandIdle")
            }
        }
        // `GroundGone` and `Airborne`: off an edge `Stand2InAir`, after an
        // ollie its own animation, then turning or idling, and legs
        // stretched for the landing.
        Action::Air => {
            if skater.landing_soon() {
                Anim::once("StretchLegsInit")
            } else if turn < 0.0 {
                Anim::once("AirTurnLeft")
            } else if turn > 0.0 {
                Anim::once("AirTurnRight")
            } else if skater.ollied() {
                Anim::into("Ollie", "AirIdle")
            } else {
                Anim::into("Stand2InAir", "AirIdle")
            }
        }
        // `OnGroundAI`.
        _ => {
            if turn < 0.0 {
                if crouched {
                    Anim::into("CrouchTurnLeft", "CrouchTurnLeftIdle")
                } else {
                    Anim::into("StandTurnLeft", "StandTurnLeftIdle")
                }
            } else if turn > 0.0 {
                if crouched {
                    Anim::into("CrouchTurnRight", "CrouchTurnRightIdle")
                } else {
                    Anim::into("StandTurnRight", "StandTurnRightIdle")
                }
            } else if crouched {
                Anim::into("Crouch", "CrouchIdle")
            } else if skater.pushing {
                Anim::cycle("PushCycle1")
            } else {
                Anim::cycle("StandIdle")
            }
        }
    }
}
