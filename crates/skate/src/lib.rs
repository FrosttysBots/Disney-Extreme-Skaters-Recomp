//! A skater for Disney's Extreme Skate Adventure: the game's physics
//! constants ([`constants`]), ray casts against a level's collision
//! ([`world`]) and the skater that rolls over it ([`skater`]).

pub mod balance;
pub mod constants;
pub mod rails;
pub mod skater;
pub mod world;

pub use balance::{Balance, BalanceParams, Lean};
pub use constants::{Physics, Stats};
pub use rails::{RailHit, Rails, Segment};
pub use skater::{Action, Grind, Input, Skater, VertAir};
pub use world::{Hit, World};
