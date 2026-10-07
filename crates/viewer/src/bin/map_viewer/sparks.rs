//! Grind sparks: the board throws sparks while grinding (`SparksOn` in the
//! `Grind` script, for grinds but not slides), drawn as short glowing
//! streaks that fall and fade.
use glam::Vec3;
use skate::{Action, Skater};

use desa_viewer::collision::ColorVertex;

/// Sparks a second while grinding.
const RATE: f32 = 120.0;
/// How long a spark lasts, at most (seconds).
const LIFE: f32 = 0.45;
/// Gravity on a spark (inches a second squared).
const GRAVITY: f32 = 386.0;
/// Sparks closer to the camera than this fade out (inches).
const NEAR: f32 = 60.0;
/// A streak's length (seconds of its travel) and width (inches).
const STREAK: f32 = 0.025;
const WIDTH: f32 = 0.8;

struct Spark {
    position: Vec3,
    velocity: Vec3,
    /// The board's velocity when it came off.
    carried: Vec3,
    age: f32,
    life: f32,
}

pub struct Sparks {
    sparks: Vec<Spark>,
    /// Sparks owed from fractions of a frame.
    owed: f32,
    seed: u32,
}

impl Sparks {
    pub fn new() -> Self {
        Self {
            sparks: Vec::new(),
            owed: 0.0,
            seed: 0x2545_F491,
        }
    }

    /// A number from 0 to 1 (xorshift).
    fn random(&mut self) -> f32 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        (self.seed >> 8) as f32 / (1u32 << 24) as f32
    }

    /// Whether the skater's grinding throws sparks: a grind, not a slide.
    fn sparking(skater: &Skater) -> bool {
        skater.action == Action::Grinding && !skater.balance_trick.as_ref().is_some_and(|t| t.slide)
    }

    /// Throws new sparks off a grinding board and moves the rest along.
    pub fn update(&mut self, skater: &Skater, dt: f32) {
        if Self::sparking(skater) {
            self.owed += RATE * dt;
            // They keep some of the board's speed, so they trail behind it
            // rather than flying back at the camera.
            let forward = skater.velocity;
            let side = skater.velocity.cross(Vec3::Y).normalize_or_zero();
            while self.owed >= 1.0 {
                self.owed -= 1.0;
                let spread = (self.random() - 0.5) * 2.0;
                let up = 40.0 + 80.0 * self.random();
                let along = 0.2 + 0.4 * self.random();
                let life = LIFE * (0.4 + 0.6 * self.random());
                let start = self.random();
                self.sparks.push(Spark {
                    position: skater.position + Vec3::Y * 1.5,
                    velocity: forward * along + side * spread * 90.0 + Vec3::Y * up,
                    carried: forward,
                    // Spread over the frame so they don't come out in clumps.
                    age: start * dt,
                    life,
                });
            }
        } else {
            self.owed = 0.0;
        }
        for spark in &mut self.sparks {
            spark.age += dt;
            spark.velocity.y -= GRAVITY * dt;
            spark.position += spark.velocity * dt;
        }
        self.sparks.retain(|s| s.age < s.life);
    }

    /// No sparks (skating stopped).
    pub fn clear(&mut self) {
        self.sparks.clear();
        self.owed = 0.0;
    }

    /// The sparks as streaks facing `eye`: hot white-yellow new, cooling
    /// to orange and fading out.
    pub fn vertices(&self, eye: Vec3) -> Vec<ColorVertex> {
        let mut out = Vec::with_capacity(self.sparks.len() * 6);
        for spark in &self.sparks {
            let t = spark.age / spark.life;
            let head = spark.position;
            let near = ((head - eye).length() / NEAR - 1.0).clamp(0.0, 1.0);
            if near == 0.0 {
                continue;
            }
            // The streak shows its motion relative to the board's, which
            // the eye follows.
            let tail = head - (spark.velocity - spark.carried) * STREAK;
            let along = head - tail;
            let across = along.cross(eye - head).normalize_or_zero() * WIDTH;
            let color = [
                255,
                (240.0 - 120.0 * t) as u8,
                (170.0 * (1.0 - t).powi(2)) as u8,
                (255.0 * (1.0 - t * t) * near) as u8,
            ];
            let tail_color = [color[0], color[1], color[2], color[3] / 4];
            let v = |p: Vec3, color: [u8; 4]| ColorVertex {
                position: p.to_array(),
                color,
            };
            out.extend([
                v(head - across, color),
                v(head + across, color),
                v(tail + across, tail_color),
                v(head - across, color),
                v(tail + across, tail_color),
                v(tail - across, tail_color),
            ]);
        }
        out
    }
}
