//! Gaps: named jumps, grinds and lines the level rewards, scored as a
//! trick in the combo. A level's trigger planes run `StartGap GapID = X
//! flags = [...]` where one starts and `EndGap GapID = X text = "..."
//! score = N` where it ends; the gap counts if nothing its flags rule out
//! happened in between (touching the ground for `CANCEL_GROUND`, say).

/// What a gap rules out on the way, or needs at its end.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GapFlags {
    /// Cancelled by rolling on the ground (`CANCEL_GROUND`).
    pub cancel_ground: bool,
    /// Cancelled by being in the air (`CANCEL_AIR`).
    pub cancel_air: bool,
    /// Only in the air throughout (`PURE_AIR`): no ground or rail.
    pub pure_air: bool,
    /// Started and ended grinding (`REQUIRE_RAIL`).
    pub require_rail: bool,
    /// Ended on a lip (`REQUIRE_LIP`).
    pub require_lip: bool,
}

/// What touching a trigger does for gaps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GapTrigger {
    Start {
        id: u32,
        flags: GapFlags,
    },
    /// A gap's end: its name and points (none for a goal's), and the
    /// script run when it's landed (`Gapscript`).
    End {
        id: u32,
        text: String,
        score: u32,
        script: Option<u32>,
    },
}

/// Where the skater is, for a gap's flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    Ground,
    Air,
    Rail,
    Lip,
}

impl GapFlags {
    /// Whether being on `surface` cancels the gap.
    pub fn cancelled_by(&self, surface: Surface) -> bool {
        match surface {
            Surface::Ground => self.cancel_ground || self.pure_air,
            Surface::Air => self.cancel_air,
            Surface::Rail | Surface::Lip => self.pure_air,
        }
    }

    /// Whether the gap can start or end while on `surface`.
    pub fn allows(&self, surface: Surface) -> bool {
        !(self.require_rail && surface != Surface::Rail) && !self.cancelled_by(surface)
    }
}

/// How far across (not counting up and down) the skater must have gone
/// between a gap's start and end. Levels put a start and an end trigger
/// in the same place for gaps that go both ways, which a jump straight up
/// and back down would otherwise score. (This crate's rule; how the game
/// tells them apart isn't known.)
pub const MIN_TRAVEL: f32 = 50.0;

/// A gap under way: its id, flags, and how far across the skater has gone
/// since it started.
#[derive(Clone, Copy, Debug)]
struct Open {
    id: u32,
    flags: GapFlags,
    travelled: f32,
}

/// The gaps under way.
#[derive(Clone, Debug, Default)]
pub struct Gaps {
    open: Vec<Open>,
}

impl Gaps {
    /// Drops the gaps that `surface` cancels, and counts the skater's
    /// movement across since the last update.
    pub fn update(&mut self, surface: Surface, across: f32) {
        self.open.retain(|gap| !gap.flags.cancelled_by(surface));
        for gap in &mut self.open {
            gap.travelled += across;
        }
    }

    /// A trigger touched: starts a gap, or ends one with its name and
    /// score if it's under way.
    pub fn touch(
        &mut self,
        trigger: &GapTrigger,
        surface: Surface,
    ) -> Option<(String, u32, Option<u32>)> {
        match trigger {
            GapTrigger::Start { id, flags } => {
                // Started whatever the skater's on (start pads sit at the
                // lips of kickers, crossed riding up them); what follows
                // can cancel it.
                if !flags.require_rail || surface == Surface::Rail {
                    self.open.retain(|gap| gap.id != *id);
                    self.open.push(Open {
                        id: *id,
                        flags: *flags,
                        travelled: 0.0,
                    });
                }
                None
            }
            GapTrigger::End {
                id,
                text,
                score,
                script,
            } => {
                let at = self.open.iter().position(|gap| gap.id == *id)?;
                let Open {
                    flags, travelled, ..
                } = self.open[at];
                if !flags.allows(surface)
                    || (flags.require_lip && surface != Surface::Lip)
                    || travelled < MIN_TRAVEL
                {
                    return None;
                }
                self.open.remove(at);
                Some((text.clone(), *score, *script))
            }
        }
    }

    pub fn clear(&mut self) {
        self.open.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.open.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start(cancel_ground: bool) -> GapTrigger {
        GapTrigger::Start {
            id: 1,
            flags: GapFlags {
                cancel_ground,
                ..GapFlags::default()
            },
        }
    }

    fn end() -> GapTrigger {
        GapTrigger::End {
            id: 1,
            text: "Chain Link Gap".into(),
            score: 100,
            script: None,
        }
    }

    #[test]
    fn a_gap_scores_from_start_to_end() {
        let mut gaps = Gaps::default();
        assert_eq!(gaps.touch(&end(), Surface::Air), None, "not started");
        gaps.touch(&start(true), Surface::Air);
        gaps.update(Surface::Air, 10.0);
        assert_eq!(
            gaps.touch(&end(), Surface::Air),
            None,
            "straight up and down"
        );
        gaps.update(Surface::Air, MIN_TRAVEL);
        assert_eq!(
            gaps.touch(&end(), Surface::Air),
            Some(("Chain Link Gap".into(), 100, None))
        );
        assert!(gaps.is_empty(), "scored once");
    }

    #[test]
    fn landing_cancels_a_cancel_ground_gap() {
        let mut gaps = Gaps::default();
        gaps.touch(&start(true), Surface::Ground);
        gaps.update(Surface::Air, 100.0);
        assert!(
            gaps.touch(&end(), Surface::Air).is_some(),
            "started riding up a kicker, then in the air"
        );
        gaps.touch(&start(true), Surface::Air);
        gaps.update(Surface::Ground, 100.0);
        assert_eq!(gaps.touch(&end(), Surface::Air), None);
        // Without the flag, rolling on is fine.
        gaps.touch(&start(false), Surface::Ground);
        gaps.update(Surface::Ground, 100.0);
        assert!(gaps.touch(&end(), Surface::Ground).is_some());
    }

    #[test]
    fn rail_gaps_need_the_rail_at_both_ends() {
        let mut gaps = Gaps::default();
        let rail = GapTrigger::Start {
            id: 1,
            flags: GapFlags {
                require_rail: true,
                ..GapFlags::default()
            },
        };
        gaps.touch(&rail, Surface::Air);
        assert!(gaps.is_empty(), "not started off the rail");
        gaps.touch(&rail, Surface::Rail);
        gaps.update(Surface::Rail, 100.0);
        assert_eq!(gaps.touch(&end(), Surface::Air), None);
        assert!(gaps.touch(&end(), Surface::Rail).is_some());
    }
}
