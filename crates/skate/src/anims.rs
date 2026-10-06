//! Which animation the skater plays, as the game's scripts pick them
//! (`TRICKS.q`): `OnGroundAI` on the ground (turning, crouching, pushing,
//! coasting), `GroundGone` and `Airborne` in the air, and `Land2` on
//! landing. Tricks, grinds, manuals and lips pick their own (see the
//! skater's `trick_pose`, `lip_pose` and balance meter).

use crate::skater::{Action, Skater};

/// An animation to show: `first` plays once (from `time`), then `then`
/// loops; or `first` loops itself. A `commit`ted one plays its first part
/// to the end before anything of the same or lower `priority` takes over
/// (the scripts wait for `AnimFinished`): a push, a landing, a bail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Anim {
    pub first: &'static str,
    pub then: Option<&'static str>,
    pub looping: bool,
    pub priority: u8,
    pub commit: bool,
}

/// What can cut in on what: coasting and pushing, then steering and
/// crouching, landing and the like, the air, and balancing and bails.
const GROUND: u8 = 0;
const STEER: u8 = 1;
const LAND: u8 = 2;
const AIR: u8 = 3;
const TRICK: u8 = 4;

impl Anim {
    const fn cycle(name: &'static str, priority: u8) -> Anim {
        Anim {
            first: name,
            then: None,
            looping: true,
            priority,
            commit: false,
        }
    }

    const fn once(name: &'static str, priority: u8) -> Anim {
        Anim {
            first: name,
            then: None,
            looping: false,
            priority,
            commit: false,
        }
    }

    const fn into(first: &'static str, then: &'static str, priority: u8) -> Anim {
        Anim {
            first,
            then: Some(then),
            looping: false,
            priority,
            commit: false,
        }
    }

    /// Plays its first part through.
    const fn committed(self) -> Anim {
        Anim {
            commit: true,
            ..self
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
        Action::BailManual => Anim::into("BailManual", "BailManualGetUp", TRICK).committed(),
        Action::BailGrind => Anim::into("BailGrind", "BailGrindGetUp", TRICK).committed(),
        Action::Bail => {
            let (fall, get_up) = skater.bail_anims;
            Anim::into(fall, get_up, TRICK).committed()
        }
        Action::FlailLeft if crouched => Anim::once("CrouchFlailLeft", LAND).committed(),
        Action::FlailRight if crouched => Anim::once("CrouchFlailRight", LAND).committed(),
        Action::FlailLeft => Anim::once("StandFlailLeft", LAND).committed(),
        Action::FlailRight => Anim::once("StandFlailRight", LAND).committed(),
        Action::Grinding => Anim::into("GrindIn1", "GrindRange1", TRICK),
        Action::Manual if skater.special_manual => {
            Anim::into("SpecialManualIn", "SpecialManualRange", TRICK)
        }
        Action::Manual => Anim::into("ManualIn1", "ManualRange1", TRICK),
        Action::Lip => Anim::once("LipRange1", TRICK),
        Action::Revert { frontside: true } => Anim::into("RevertFS", "StandIdle", LAND).committed(),
        Action::Revert { frontside: false } => {
            Anim::into("RevertBS", "StandIdle", LAND).committed()
        }
        // `Land2`: the landing plays through (`WaitAnimWhilstChecking`).
        Action::Landing => {
            let l = skater.landing;
            let anim = if l.backwards {
                Anim::into("LandBackward1", "StandIdle", LAND)
            } else if l.sketchy {
                Anim::into("LandSketchy", "StandIdle", LAND)
            } else if l.little_air {
                if crouched {
                    Anim::into("CrouchBumpDown", "CrouchIdle", LAND)
                } else {
                    Anim::into("LandSmall", "StandIdle", LAND)
                }
            } else if l.short {
                Anim::into("CrouchBumpDown", "StandIdle", LAND)
            } else {
                Anim::into("Land1", "StandIdle", LAND)
            };
            anim.committed()
        }
        // `GroundGone` and `Airborne`: off an edge `Stand2InAir`, after an
        // ollie its own animation (both played through), then turning or
        // idling, and legs stretched for the landing.
        Action::Air => {
            if skater.landing_soon() {
                Anim::once("StretchLegsInit", AIR)
            } else if turn < 0.0 {
                Anim::once("AirTurnLeft", AIR)
            } else if turn > 0.0 {
                Anim::once("AirTurnRight", AIR)
            } else if skater.ollied() {
                Anim::into("Ollie", "AirIdle", AIR).committed()
            } else {
                Anim::into("Stand2InAir", "AirIdle", AIR).committed()
            }
        }
        // `OnGroundAI`: steering and crouching cut in on pushing; a push
        // plays through (`DoAPush` waits for `AnimFinished`).
        _ => {
            if turn < 0.0 {
                if crouched {
                    Anim::into("CrouchTurnLeft", "CrouchTurnLeftIdle", STEER)
                } else {
                    Anim::into("StandTurnLeft", "StandTurnLeftIdle", STEER)
                }
            } else if turn > 0.0 {
                if crouched {
                    Anim::into("CrouchTurnRight", "CrouchTurnRightIdle", STEER)
                } else {
                    Anim::into("StandTurnRight", "StandTurnRightIdle", STEER)
                }
            } else if crouched {
                Anim::into("Crouch", "CrouchIdle", STEER)
            } else if skater.pushing {
                Anim::cycle("PushCycle1", GROUND).committed()
            } else {
                Anim::cycle("StandIdle", GROUND)
            }
        }
    }
}
