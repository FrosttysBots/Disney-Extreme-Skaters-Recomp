//! A skater for Disney's Extreme Skate Adventure: the game's physics
//! constants ([`constants`]), ray casts against a level's collision
//! ([`world`]) and the skater that rolls over it ([`skater`]).

pub mod anims;
pub mod balance;
pub mod camera;
pub mod constants;
pub mod rails;
pub mod score;
pub mod skater;
pub mod tricks;
pub mod world;

pub use anims::{Anim, Landing};
pub use balance::{Balance, BalanceParams, Lean};
pub use camera::ChaseCamera;
pub use constants::{Physics, Stats};
pub use rails::{RailHit, Rails, Segment};
pub use score::{Combo, ComboTrick};
pub use skater::{Action, Grind, Input, Landed, Lip, Phase, Playing, Skater, VertAir};
pub use tricks::{Button, Dir, Kind, LipTrick, Trick, TrickBook};
pub use world::{Hit, World};
