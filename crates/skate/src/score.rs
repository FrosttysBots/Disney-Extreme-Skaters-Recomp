//! Scoring combos as the game does (main.dol: the score object's methods
//! around 0x800B05BC).
//!
//! Each trick in a combo scores its points times a degrade percentage, for
//! repeating a trick already in the combo (100, 90, 80, 70, 60 then 50%),
//! times a spin factor (x1, 1.5, 2, 3, 4 then 5 for 0 to 5 or more 180s;
//! 0x800B1630 and 0x800B1658 read the two tables). The spin factor comes
//! from the combo's first trick, or the first after one that blocks spins
//! (grinds and manuals `Display BlockSpin`), and carries on to the tricks
//! after it. The combo's total is the sum times its trick count
//! (0x800B124C).

/// One trick in a combo.
#[derive(Clone, Debug, PartialEq)]
pub struct ComboTrick {
    pub name: String,
    pub score: u32,
    /// How many times the same trick came earlier in the combo.
    pub repeats: u32,
    /// Half-turns spun (180s).
    pub spins: u32,
    /// Grinds and manuals: no spin factor, and the next trick sets it anew.
    pub block_spin: bool,
}

/// The degrade table, in percent, by repeats.
const DEGRADE: [u32; 6] = [100, 90, 80, 70, 60, 50];
/// The spin table, in halves, by 180s.
const SPIN: [u32; 6] = [2, 3, 4, 6, 8, 10];
/// Degrees of slop when counting 180s (`spin_count_slop` in PHYSICS.q).
pub const SPIN_COUNT_SLOP: f32 = 60.0;

/// A combo in progress.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Combo {
    pub tricks: Vec<ComboTrick>,
}

impl Combo {
    /// A trick into the combo (the score object's 0x800B05BC).
    pub fn add(&mut self, name: &str, score: u32, block_spin: bool) {
        let repeats = self.tricks.iter().filter(|t| t.name == name).count() as u32;
        self.tricks.push(ComboTrick {
            name: name.to_string(),
            score,
            repeats,
            spins: 0,
            block_spin,
        });
    }

    /// A trick that doesn't lose points for repeating (`Display
    /// NoDegrade`, as the spine transfer's `SkaterAwardTransfer`).
    pub fn add_no_degrade(&mut self, name: &str, score: u32) {
        self.add(name, score, false);
        if let Some(last) = self.tricks.last_mut() {
            last.repeats = 0;
        }
    }

    /// Points added to the latest trick (the score object's `TweakTrick`,
    /// 0x800B0BA0): each frame of a grind (`SetGrindTweak`, 7), a manual
    /// (`DoBalanceTrick Tweak`, 1) or a held grab (`GrabTweak`, 20).
    pub fn tweak(&mut self, points: u32) {
        if let Some(last) = self.tricks.last_mut() {
            last.score += points;
        }
    }

    /// Credits the spin so far to the latest trick: `degrees` turned, in
    /// 180s with `spin_count_slop` (0x800B0A00). It only ever goes up.
    pub fn spin(&mut self, degrees: f32) {
        let count = ((degrees.abs() + SPIN_COUNT_SLOP) / 180.0) as u32;
        if let Some(last) = self.tricks.last_mut() {
            if !last.block_spin {
                last.spins = last.spins.max(count);
            }
        }
    }

    /// The points before the multiplier (0x800B124C).
    pub fn points(&self) -> u32 {
        let mut spin = SPIN[0];
        let mut points = 0;
        for (i, trick) in self.tricks.iter().enumerate() {
            if trick.block_spin {
                spin = SPIN[0];
            } else if i == 0 || self.tricks[i - 1].block_spin {
                spin = SPIN[(trick.spins as usize).min(SPIN.len() - 1)];
            }
            let degrade = DEGRADE[(trick.repeats as usize).min(DEGRADE.len() - 1)];
            points += trick.score * degrade * spin / 200;
        }
        points
    }

    /// The multiplier: one per trick.
    pub fn multiplier(&self) -> u32 {
        self.tricks.len() as u32
    }

    pub fn total(&self) -> u32 {
        self.points() * self.multiplier()
    }

    pub fn is_empty(&self) -> bool {
        self.tricks.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tweaks_add_to_the_latest_trick() {
        let mut combo = Combo::default();
        combo.add("Grab", 200, false);
        combo.add("Grind", 100, true);
        for _ in 0..60 {
            combo.tweak(7);
        }
        assert_eq!(combo.tricks[0].score, 200);
        assert_eq!(combo.tricks[1].score, 100 + 420);
    }

    #[test]
    fn repeats_degrade_and_spins_multiply() {
        let mut combo = Combo::default();
        combo.add("Kickflip", 100, false);
        combo.spin(170.0); // 170 + 60 slop: one 180
        combo.add("Kickflip", 100, false);
        // 100 * 1.5 + 100 * 90% * 1.5 (the spin carries on), times 2.
        assert_eq!(combo.points(), 150 + 135);
        assert_eq!(combo.total(), 570);
        // A grind blocks the spin; the trick after it sets it anew.
        combo.add("Grind", 100, true);
        combo.add("Grab", 200, false);
        assert_eq!(combo.points(), 150 + 135 + 100 + 200);
        assert_eq!(combo.total(), 585 * 4);
    }
}
