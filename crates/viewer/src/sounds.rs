//! Which sound each surface makes, from the game's `TERRAIN.q`.
//!
//! Each `SetTerrain...` script loads its sounds (`LoadSound
//! "Terrains\\RollConcRough" Vol = 100`) and says which plays for what on
//! which terrain:
//!
//! ```text
//! SetTerrainSfxProperties RollConcRough {
//!     terrain = TERRAIN_CONCROUGH Table = SK3SFX_TABLE_WHEELROLL maxPitch = 120 minPitch = 50 }
//! ```
//!
//! The tables are the moments: rolling, jumping, landing, bonking a wall,
//! grinding (and slides) and jumping off and landing on rails, and the
//! cess (a revert's slide). The face's terrain number comes from the
//! collision (`TERRAIN_CONCSMOOTH = 1` and so on).

use std::collections::HashMap;

use qb::checksum;
use qb::token::Token;
use qb::vm::Program;

/// The moment a sound is for (`SK3SFX_TABLE_...`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Moment {
    Roll,
    Jump,
    Land,
    Bonk,
    Grind,
    GrindJump,
    GrindLand,
    Slide,
    SlideJump,
    SlideLand,
    Cess,
}

impl Moment {
    fn from_table(name: u32) -> Option<Moment> {
        [
            ("SK3SFX_TABLE_WHEELROLL", Moment::Roll),
            ("SK3SFX_TABLE_JUMP", Moment::Jump),
            ("SK3SFX_TABLE_LAND", Moment::Land),
            ("SK3SFX_TABLE_BONK", Moment::Bonk),
            ("SK3SFX_TABLE_GRIND", Moment::Grind),
            ("SK3SFX_TABLE_GRINDJUMP", Moment::GrindJump),
            ("SK3SFX_TABLE_GRINDLAND", Moment::GrindLand),
            ("SK3SFX_TABLE_SLIDE", Moment::Slide),
            ("SK3SFX_TABLE_SLIDEJUMP", Moment::SlideJump),
            ("SK3SFX_TABLE_SLIDELAND", Moment::SlideLand),
            ("SK3SFX_TABLE_CESS", Moment::Cess),
        ]
        .into_iter()
        .find_map(|(n, m)| (checksum(n) == name).then_some(m))
    }
}

/// A sound for a moment on a terrain: its file (lowercase, no `.dsp`),
/// its volume (`LoadSound ... Vol`, 1 is 100), and the pitch and volume
/// ranges the game scales it through with speed (fractions; 1 is 100).
#[derive(Clone, Debug, PartialEq)]
pub struct TerrainSound {
    pub file: String,
    pub volume: f32,
    pub pitch: (f32, f32),
    pub loudness: (f32, f32),
}

/// Every terrain's sounds.
#[derive(Clone, Debug, Default)]
pub struct TerrainSounds {
    sounds: HashMap<(u16, Moment), TerrainSound>,
}

impl TerrainSounds {
    /// Reads every `SetTerrainSfxProperties` in the loaded scripts.
    pub fn new(program: &Program) -> Self {
        let mut sounds = HashMap::new();
        for (_, body) in program.scripts() {
            // The sounds this script loads, by their name's checksum.
            let mut loaded: HashMap<u32, (String, f32)> = HashMap::new();
            for (i, token) in body.iter().enumerate() {
                if *token == Token::Name(checksum("LoadSound")) {
                    if let Some(Token::String(path)) = body.get(i + 1) {
                        let file = path
                            .rsplit(['\\', '/'])
                            .next()
                            .unwrap_or(path)
                            .to_ascii_lowercase();
                        let volume = number_after(&body[i + 2..], "Vol").unwrap_or(100.0) / 100.0;
                        loaded.insert(checksum(&file), (file, volume));
                    }
                }
            }
            for (i, token) in body.iter().enumerate() {
                if *token != Token::Name(checksum("SetTerrainSfxProperties")) {
                    continue;
                }
                let Some(Token::Name(sound)) = body.get(i + 1) else {
                    continue;
                };
                let props: Vec<Token> = body[i + 2..]
                    .iter()
                    .skip_while(|t| !matches!(t, Token::StartStruct))
                    .take_while(|t| !matches!(t, Token::EndStruct))
                    .cloned()
                    .collect();
                let name_after = |key: &str| {
                    props.windows(3).find_map(|w| match w {
                        [Token::Name(k), Token::Equals, Token::Name(v)] if *k == checksum(key) => {
                            Some(*v)
                        }
                        _ => None,
                    })
                };
                let (Some(terrain), Some(table)) = (name_after("terrain"), name_after("Table"))
                else {
                    continue;
                };
                let Some(terrain) = program
                    .value(terrain)
                    .and_then(|v| v.as_int())
                    .and_then(|t| u16::try_from(t).ok())
                else {
                    continue;
                };
                let Some(moment) = Moment::from_table(table) else {
                    continue;
                };
                let (file, volume) = loaded
                    .get(sound)
                    .cloned()
                    .unwrap_or_else(|| (String::new(), 1.0));
                if file.is_empty() {
                    continue;
                }
                let range = |min: &str, max: &str| {
                    let lo = number_after(&props, min).unwrap_or(100.0) / 100.0;
                    let hi = number_after(&props, max).unwrap_or(100.0) / 100.0;
                    (lo, hi)
                };
                sounds.insert(
                    (terrain, moment),
                    TerrainSound {
                        file,
                        volume,
                        pitch: range("minPitch", "maxPitch"),
                        loudness: range("minVol", "maxVol"),
                    },
                );
            }
        }
        TerrainSounds { sounds }
    }

    /// The sound for `moment` on `terrain`, or the default terrain's.
    pub fn get(&self, terrain: u16, moment: Moment) -> Option<&TerrainSound> {
        self.sounds
            .get(&(terrain, moment))
            .or_else(|| self.sounds.get(&(0, moment)))
            .or_else(|| self.sounds.get(&(1, moment)))
    }

    pub fn len(&self) -> usize {
        self.sounds.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sounds.is_empty()
    }
}

/// The number after `key =` in a run of tokens.
fn number_after(tokens: &[Token], key: &str) -> Option<f32> {
    let key = checksum(key);
    tokens.windows(3).find_map(|w| match w {
        [Token::Name(k), Token::Equals, Token::Integer(n)] if *k == key => Some(*n as f32),
        [Token::Name(k), Token::Equals, Token::Float(n)] if *k == key => Some(*n),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_terrains_sounds() {
        let n = |s: &str| Token::Name(checksum(s));
        let mut program = Program::new();
        program.add_value(checksum("TERRAIN_CONCROUGH"), qb::Value::Integer(2));
        program.add_script(
            checksum("SetTerrainConcRough"),
            vec![
                n("LoadSound"),
                Token::String("Terrains\\RollConcRough".into()),
                n("Vol"),
                Token::Equals,
                Token::Integer(70),
                Token::EndOfLine,
                n("SetTerrainSfxProperties"),
                n("RollConcRough"),
                Token::StartStruct,
                n("terrain"),
                Token::Equals,
                n("TERRAIN_CONCROUGH"),
                n("Table"),
                Token::Equals,
                n("SK3SFX_TABLE_WHEELROLL"),
                n("maxPitch"),
                Token::Equals,
                Token::Integer(120),
                n("minPitch"),
                Token::Equals,
                Token::Integer(50),
                Token::EndStruct,
                Token::EndOfLine,
            ],
        );
        let sounds = TerrainSounds::new(&program);
        let roll = sounds.get(2, Moment::Roll).expect("the roll");
        assert_eq!(roll.file, "rollconcrough");
        assert!((roll.volume - 0.7).abs() < 1e-6);
        assert_eq!(roll.pitch, (0.5, 1.2));
        assert_eq!(roll.loudness, (1.0, 1.0));
        assert!(sounds.get(2, Moment::Land).is_none());
    }
}
