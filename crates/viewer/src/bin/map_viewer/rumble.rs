//! Gamepad rumble, where the game's scripts vibrate the pad (`Vibrate
//! Actuator = ... percent = ... duration = ...`): actuator 1 is the strong
//! motor, 0 the weak one.
//!
//! - an ollie (`Ollie`): strong 50% for 0.05 s;
//! - a landing (`Land2`): strong 80% for 0.1 s;
//! - starting a grind (`Grind`): strong 50% for 0.25 s, and weak 50% for
//!   as long as it lasts (until `VibrateOff` on leaving the rail);
//! - a revert (`Revert`): weak 80% for 0.5 s and strong 80% for 0.1 s;
//! - a flail (`FlailVibrate`): strong 80% for 0.25 s;
//! - a bail or a smack into a wall (`GeneralBail`, `BailSmack`): strong
//!   100% for 0.2 s.
use std::time::{Duration, Instant};

use gilrs::Gilrs;
use gilrs::ff::{BaseEffect, BaseEffectType, Effect, EffectBuilder, Repeat, Replay, Ticks};
use skate::skater::SkateSound;
use skate::{Action, Skater};

/// A pulse playing, and when it's done.
struct Pulse {
    _effect: Effect,
    until: Instant,
}

pub struct Rumble {
    pulses: Vec<Pulse>,
    /// The weak buzz while grinding.
    grind: Option<Effect>,
    last: Action,
}

impl Rumble {
    pub fn new() -> Self {
        Self {
            pulses: Vec::new(),
            grind: None,
            last: Action::Standing,
        }
    }

    /// An effect on every pad that can rumble: the strong and weak motors
    /// at these fractions, for `seconds` (or until dropped).
    fn effect(gilrs: &mut Gilrs, strong: f32, weak: f32, seconds: Option<f32>) -> Option<Effect> {
        let pads: Vec<_> = gilrs
            .gamepads()
            .filter(|(_, pad)| pad.is_ff_supported())
            .map(|(id, _)| id)
            .collect();
        if pads.is_empty() {
            return None;
        }
        let scheduling = match seconds {
            Some(s) => Replay {
                play_for: Ticks::from_ms((s * 1000.0) as u32),
                ..Default::default()
            },
            None => Replay::default(),
        };
        let mut builder = EffectBuilder::new();
        for (kind, level) in [
            (BaseEffectType::Strong { magnitude: 0 }, strong),
            (BaseEffectType::Weak { magnitude: 0 }, weak),
        ] {
            if level <= 0.0 {
                continue;
            }
            let magnitude = (level.min(1.0) * f32::from(u16::MAX)) as u16;
            let kind = match kind {
                BaseEffectType::Strong { .. } => BaseEffectType::Strong { magnitude },
                _ => BaseEffectType::Weak { magnitude },
            };
            builder.add_effect(BaseEffect {
                kind,
                scheduling,
                ..Default::default()
            });
        }
        let effect = builder.gamepads(&pads).finish(gilrs).ok()?;
        // Effects repeat until stopped unless told otherwise.
        let repeat = match seconds {
            Some(s) => Repeat::For(Ticks::from_ms((s * 1000.0) as u32)),
            None => Repeat::Infinitely,
        };
        effect.set_repeat(repeat).ok()?;
        effect.play().ok()?;
        Some(effect)
    }

    fn pulse(&mut self, gilrs: &mut Gilrs, strong: f32, weak: f32, seconds: f32) {
        if let Some(effect) = Self::effect(gilrs, strong, weak, Some(seconds)) {
            self.pulses.push(Pulse {
                _effect: effect,
                until: Instant::now() + Duration::from_secs_f32(seconds + 0.1),
            });
        }
    }

    /// Rumbles for what the skater did this frame (before its sounds are
    /// taken).
    pub fn update(&mut self, gilrs: &mut Gilrs, skater: &Skater) {
        for sound in &skater.sounds {
            match sound {
                SkateSound::Jump { .. } => self.pulse(gilrs, 0.5, 0.0, 0.05),
                SkateSound::Land => self.pulse(gilrs, 0.8, 0.0, 0.1),
                SkateSound::RailOn => self.pulse(gilrs, 0.5, 0.0, 0.25),
                SkateSound::Bail | SkateSound::Smack => self.pulse(gilrs, 1.0, 0.0, 0.2),
                _ => {}
            }
        }
        let action = skater.action;
        if action != self.last {
            match action {
                Action::Revert { .. } => {
                    self.pulse(gilrs, 0.8, 0.0, 0.1);
                    self.pulse(gilrs, 0.0, 0.8, 0.5);
                }
                Action::FlailLeft | Action::FlailRight => self.pulse(gilrs, 0.8, 0.0, 0.25),
                _ => {}
            }
            self.last = action;
        }
        let grinding = action == Action::Grinding;
        if grinding && self.grind.is_none() {
            self.grind = Self::effect(gilrs, 0.0, 0.5, None);
        } else if !grinding {
            self.grind = None;
        }
        let now = Instant::now();
        self.pulses.retain(|p| p.until > now);
    }

    /// Stops all rumbling.
    pub fn stop(&mut self) {
        self.pulses.clear();
        self.grind = None;
    }
}
