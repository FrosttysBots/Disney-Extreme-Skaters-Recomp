//! Balancing manuals and grinds: the game's balance meter (main.dol
//! 0x800CAA50 starts one, 0x800CAD30 updates it each frame).
//!
//! The lean `angle` runs from -4096 to 4096 on the meter. It falls away
//! from the middle on its own, faster the longer the trick is held
//! (`Instable_Base` plus `Instable_Rate` a second), and moves by the lean
//! `speed`, which the two balance buttons push one way or the other
//! (`Lean_Acc`). Left alone, the speed wanders at random. Past
//! `Lean_Bail_Angle` the skater falls off one end or the other. Manuals,
//! grinds and lips each have their own tuning (`ManualParams`,
//! `GrindParams`, `LipParams` in `PHYSICS.q`), scaled by a stat.

use qb::vm::Program;
use qb::{Value, checksum};

use crate::constants::{Stats, scale};

/// A balance trick's tuning, with stats applied.
#[derive(Clone, Debug, PartialEq)]
pub struct BalanceParams {
    /// A buffer added to the lean of the next balance trick in a combo,
    /// draining over `cheese_frames`.
    pub cheese: f32,
    pub cheese_frames: f32,
    pub lean_gravity: f32,
    pub instable_rate: f32,
    pub instable_base: f32,
    pub lean_min_speed: f32,
    pub lean_rnd_speed: f32,
    pub repeat_min: f32,
    pub repeat_multiplier: f32,
    pub lean_repeat_multiplier: f32,
    pub lean_acc: f32,
    pub lean_bail_angle: f32,
    /// Seconds after starting during which a button that would lean the
    /// skater further over has no effect (`BalanceSafeButtonPeriod`).
    pub safe_button_period: f32,
}

impl BalanceParams {
    /// `ManualParams`, `GrindParams` or `LipParams`, scaled by `stats`
    /// (the values on the US disc when missing).
    pub fn new(program: &Program, name: &str, stats: &Stats) -> Self {
        let params = program.value(checksum(name));
        let get = |key: &str, fallback: f32| {
            params
                .and_then(|p| p.get(checksum(key)))
                .and_then(|v| scale(program, v, stats))
                .unwrap_or(fallback)
        };
        let safe = program
            .value(checksum("BalanceSafeButtonPeriod"))
            .and_then(Value::as_f32)
            .unwrap_or(1000.0);
        BalanceParams {
            cheese: get("Cheese", 0.0),
            cheese_frames: get("CheeseFrames", 10.0),
            lean_gravity: get("Lean_Gravity_Stat", 0.02),
            instable_rate: get("Instable_Rate", 0.08),
            instable_base: get("Instable_Base", 0.25),
            lean_min_speed: get("Lean_Min_Speed", 5.0),
            lean_rnd_speed: get("Lean_Rnd_Speed", 20.0),
            repeat_min: get("Repeat_Min", 1.0),
            repeat_multiplier: get("Repeat_Multiplier", 0.0),
            lean_repeat_multiplier: get("Lean_Repeat_Multiplier", 0.0),
            lean_acc: get("Lean_Acc", 10.0),
            lean_bail_angle: get("Lean_Bail_Angle", 4000.0),
            safe_button_period: safe / 1000.0,
        }
    }
}

/// How a balance update went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lean {
    Balanced,
    /// Off the top of the meter (`OffMeterTop`).
    OffTop,
    /// Off the bottom (`OffMeterBottom`).
    OffBottom,
}

/// The balance meter's state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Balance {
    pub angle: f32,
    pub speed: f32,
    /// Seconds into the trick, and the time the instability grows with.
    time: f32,
    unstable_time: f32,
    cheese: f32,
    /// Buttons count once both have been let go.
    buttons_ok: bool,
    seed: u32,
}

/// The meter's full scale.
pub const METER: f32 = 4096.0;

impl Balance {
    /// A random number below `n` (the game's 0x8000541C).
    fn random(&mut self, n: u32) -> u32 {
        self.seed = self.seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        (self.seed >> 16) % n.max(1)
    }

    /// Starts a balance trick (0x800CAA50). A fresh one starts level,
    /// leaning off at `repeat_min` either way; one in the same combo as
    /// the last keeps some of its lean (`lean_repeat_multiplier`) and speed
    /// (`repeat_multiplier`), plus the cheese left over.
    pub fn start(&mut self, p: &BalanceParams, fresh: bool) {
        if fresh {
            self.angle = 0.0;
            self.speed = 0.0;
            self.cheese = 0.0;
        }
        self.time = 0.0;
        self.unstable_time = 0.0;
        self.buttons_ok = false;
        if self.speed == 0.0 {
            self.speed = p.repeat_min;
            if self.random(2) == 0 {
                self.speed = -self.speed;
            }
        } else {
            self.speed *= p.repeat_multiplier;
            if self.speed.abs() < p.repeat_min {
                self.speed = p.repeat_min.copysign(self.speed);
            }
        }
        let sign = if self.angle < 0.0 { -1.0 } else { 1.0 };
        self.angle = self.angle * p.lean_repeat_multiplier + self.cheese * sign;
        if self.angle.abs() > p.lean_bail_angle {
            self.angle = (p.lean_bail_angle * 0.9).copysign(self.speed);
        }
        self.cheese = p.cheese;
    }

    /// Level and still (the Perfect cheats): it can't fall.
    pub fn steady(&mut self) {
        self.angle = 0.0;
        self.speed = 0.0;
    }

    /// One update (0x800CAD30), `dt` seconds: the game scales each change
    /// by the frame's length in 60ths of a second. `a` and `b` are the
    /// balance buttons (up and down for a manual, right and left on a
    /// rail): `a` leans towards the bottom of the meter, `b` the top.
    pub fn update(&mut self, a: bool, b: bool, p: &BalanceParams, dt: f32) -> Lean {
        let frames = |x: f32| x * dt * 60.0;
        self.cheese = (self.cheese - frames(p.cheese / p.cheese_frames.max(1.0))).max(0.0);
        self.time += dt;
        self.unstable_time += dt;
        let instability = self.unstable_time * p.instable_rate + p.instable_base;
        self.angle += frames(self.angle * p.lean_gravity * instability);
        self.angle += frames(self.speed * instability);
        if !a && !b {
            self.buttons_ok = true;
        }
        let early = self.time < p.safe_button_period;
        if a && self.buttons_ok {
            // Over-correcting does nothing for the first moments.
            let effect = if self.angle < 0.0 && early { 0.0 } else { 1.0 };
            self.speed -= frames(p.lean_acc) * effect;
        } else if b && self.buttons_ok {
            let effect = if self.angle > 0.0 && early { 0.0 } else { 1.0 };
            self.speed += frames(p.lean_acc) * effect;
        } else if self.speed.abs() < p.lean_min_speed {
            let sign = if self.speed < 0.0 { -1.0 } else { 1.0 };
            self.speed = self.random(p.lean_rnd_speed as u32 | 1) as f32 * sign;
        } else {
            let sign = if self.speed < 0.0 { -1.0 } else { 1.0 };
            self.speed += self.random(50) as f32 / 100.0 * sign;
        }
        if self.angle.abs() > p.lean_bail_angle {
            let off = if self.angle > 0.0 {
                Lean::OffTop
            } else {
                Lean::OffBottom
            };
            self.angle = 0.0;
            return off;
        }
        Lean::Balanced
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> BalanceParams {
        BalanceParams::new(&Program::new(), "ManualParams", &Stats::default())
    }

    #[test]
    fn left_alone_it_falls_off_the_meter() {
        let p = params();
        let mut balance = Balance::default();
        balance.start(&p, true);
        let mut frames = 0;
        while balance.update(false, false, &p, 1.0 / 60.0) == Lean::Balanced {
            frames += 1;
            assert!(frames < 60 * 30, "should fall within 30 seconds");
        }
        assert!(frames > 30, "not at once: {frames} frames");
    }

    #[test]
    fn correcting_keeps_it_up_longer() {
        let p = params();
        let mut lazy = Balance::default();
        lazy.start(&p, true);
        let mut careful = lazy.clone();
        let mut lazy_frames = 0;
        while lazy.update(false, false, &p, 1.0 / 60.0) == Lean::Balanced {
            lazy_frames += 1;
        }
        let mut careful_frames = 0;
        // Push against where the lean is heading.
        loop {
            let ahead = careful.angle + careful.speed * 20.0;
            let (a, b) = (ahead > 100.0, ahead < -100.0);
            if careful.update(a, b, &p, 1.0 / 60.0) != Lean::Balanced || careful_frames > 600 {
                break;
            }
            careful_frames += 1;
        }
        assert!(
            careful_frames > lazy_frames * 2,
            "{careful_frames} vs {lazy_frames}"
        );
    }
}
