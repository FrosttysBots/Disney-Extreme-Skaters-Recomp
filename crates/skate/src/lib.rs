//! A skater for Disney's Extreme Skate Adventure: the game's physics
//! constants ([`constants`]), ray casts against a level's collision
//! ([`world`]) and the skater that rolls over it ([`skater`]).

pub mod constants;
pub mod skater;
pub mod world;

pub use constants::{Physics, Stats};
pub use skater::{Action, Input, Skater};
pub use world::{Hit, World};
