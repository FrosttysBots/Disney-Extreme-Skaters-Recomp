//! The skater's sounds: the game's own `.dsp` effects for each surface
//! (from `TERRAIN.q`), played as the skater rolls, ollies, lands, grinds
//! and bails.
//!
//! Rolling and grinding loop, their pitch and loudness following the
//! speed through the ranges `TERRAIN.q` gives (`minPitch`/`maxPitch`,
//! `minVol`/`maxVol`); the rest play once.
//!
//! And the music: the game's streamed songs (`.dtk`), a track at a time,
//! and the level's ambience looping under everything.

use std::collections::HashMap;
use std::num::NonZero;
use std::time::Duration;

use desa_viewer::sounds::{Moment, TerrainSound, TerrainSounds};
use ngc_sound::{Dtk, Sound};
use rodio::buffer::SamplesBuffer;
use rodio::{ChannelCount, DeviceSinkBuilder, MixerDeviceSink, Player, SampleRate, Source};
use skate::{SkateSound, Skater};

/// All sounds a little quieter than full scale, so several together don't
/// clip.
const MASTER: f32 = 0.5;
/// Smooth concrete (`TERRAIN_CONCSMOOTH`), whose sounds every level has.
const CONCRETE: u16 = 1;
/// The speed rolling and grinding sounds reach their top pitch at.
const TOP_SPEED: f32 = 800.0;

/// How loud the songs and the ambience play, against the effects.
const MUSIC_VOLUME: f32 = 0.35;
const AMBIENCE_VOLUME: f32 = 0.5;

/// A `.dtk` track decoded as it plays.
struct Stream(Dtk);

impl Iterator for Stream {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        self.0.next().map(|s| f32::from(s) / 32768.0)
    }
}

impl Source for Stream {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> ChannelCount {
        NonZero::new(2).unwrap()
    }

    fn sample_rate(&self) -> SampleRate {
        NonZero::new(ngc_sound::dtk::SAMPLE_RATE).unwrap()
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

/// A decoded sound, ready to play.
struct Clip {
    rate: u32,
    samples: Vec<f32>,
}

impl Clip {
    fn buffer(&self) -> SamplesBuffer {
        SamplesBuffer::new(
            NonZero::new(1).unwrap(),
            NonZero::new(self.rate.max(1)).unwrap(),
            self.samples.clone(),
        )
    }
}

/// A looping sound (rolling or grinding) and the file it plays.
struct Loop {
    file: String,
    player: Player,
}

pub struct Audio {
    sink: MixerDeviceSink,
    clips: HashMap<String, Clip>,
    terrain: TerrainSounds,
    /// Each rail's terrain, by segment (as the world's rails are ordered).
    rail_terrain: Vec<u16>,
    roll: Option<Loop>,
    grind: Option<Loop>,
    /// The song playing, and the level's ambience with its track (played
    /// again each time it ends).
    music: Option<Player>,
    ambience: Option<(Player, Vec<u8>)>,
}

impl Audio {
    /// The default output device, or none (no sound then).
    pub fn new() -> Option<Self> {
        let mut sink = DeviceSinkBuilder::open_default_sink().ok()?;
        sink.log_on_drop(false);
        Some(Audio {
            sink,
            clips: HashMap::new(),
            terrain: TerrainSounds::default(),
            rail_terrain: Vec::new(),
            roll: None,
            grind: None,
            music: None,
            ambience: None,
        })
    }

    /// The sounds to play: `files` by lowercase name (the level's archive
    /// and `skater_sounds.prg`), which surface makes which, and each rail's
    /// terrain.
    pub fn load(
        &mut self,
        files: HashMap<String, Vec<u8>>,
        terrain: TerrainSounds,
        rail_terrain: Vec<u16>,
    ) {
        self.stop();
        self.clips = files
            .into_iter()
            .filter_map(|(name, data)| {
                let sound = Sound::parse(&data).ok()?;
                Some((
                    name,
                    Clip {
                        rate: sound.sample_rate,
                        samples: sound.floats(),
                    },
                ))
            })
            .collect();
        self.terrain = terrain;
        self.rail_terrain = rail_terrain;
    }

    /// Silences the loops and the music (skating stopped).
    pub fn stop(&mut self) {
        self.roll = None;
        self.grind = None;
        self.music = None;
        self.ambience = None;
    }

    /// Plays a song (a `.dtk` track) in place of the last.
    pub fn play_music(&mut self, track: Vec<u8>) {
        let player = Player::connect_new(self.sink.mixer());
        player.set_volume(MUSIC_VOLUME);
        player.append(Stream(Dtk::new(track)));
        self.music = Some(player);
    }

    pub fn stop_music(&mut self) {
        self.music = None;
    }

    /// No song playing (none started, or it ended).
    pub fn music_finished(&self) -> bool {
        self.music.as_ref().is_none_or(|p| p.empty())
    }

    /// The level's ambience (a `.dtk` track), looping; or none.
    pub fn set_ambience(&mut self, track: Option<Vec<u8>>) {
        self.ambience = track.map(|track| {
            let player = Player::connect_new(self.sink.mixer());
            player.set_volume(AMBIENCE_VOLUME);
            player.append(Stream(Dtk::new(track.clone())));
            (player, track)
        });
    }

    pub fn has_ambience(&self) -> bool {
        self.ambience.is_some()
    }

    /// Starts the ambience over when it ends.
    pub fn keep_ambience(&mut self) {
        if let Some((player, track)) = &self.ambience {
            if player.empty() {
                player.append(Stream(Dtk::new(track.clone())));
            }
        }
    }

    fn play(&self, file: &str, volume: f32, speed: f32) {
        let Some(clip) = self.clips.get(file) else {
            return;
        };
        let player = Player::connect_new(self.sink.mixer());
        player.set_volume(volume * MASTER);
        player.set_speed(speed.max(0.05));
        player.append(clip.buffer());
        player.detach();
    }

    fn play_terrain(&self, terrain: u16, moment: Moment) {
        if let Some(sound) = self.terrain_sound(terrain, moment) {
            self.play(&sound.file, sound.volume, 1.0);
        }
    }

    /// The sound for `moment` on `terrain`, or smooth concrete's when this
    /// level doesn't have that terrain's file.
    fn terrain_sound(&self, terrain: u16, moment: Moment) -> Option<TerrainSound> {
        [terrain, CONCRETE]
            .into_iter()
            .filter_map(|t| self.terrain.get(t, moment))
            .find(|s| self.clips.contains_key(&s.file))
            .cloned()
    }

    /// Plays what the skater did since last time, and keeps the rolling
    /// and grinding loops going at its speed.
    pub fn update(&mut self, skater: &mut Skater) {
        let rail = skater
            .grind
            .map(|g| self.rail_terrain.get(g.segment).copied().unwrap_or(0));
        for sound in std::mem::take(&mut skater.sounds) {
            match sound {
                SkateSound::Jump { from_rail: true } => {
                    self.play_terrain(rail.unwrap_or(skater.terrain), Moment::GrindJump)
                }
                SkateSound::Jump { from_rail: false } => {
                    self.play_terrain(skater.terrain, Moment::Jump)
                }
                SkateSound::Land => self.play_terrain(skater.terrain, Moment::Land),
                SkateSound::RailOn => {
                    self.play_terrain(rail.unwrap_or(skater.terrain), Moment::GrindLand)
                }
                SkateSound::Cess => self.play_terrain(skater.terrain, Moment::Cess),
                SkateSound::Bail => self.play("bail_knee1", 1.0, 1.0),
                SkateSound::Smack => self.play("bodysmacka", 1.0, 1.0),
                SkateSound::Gap => self.play("hud_jumpgap", 1.0, 1.0),
                SkateSound::Teleport => self.play("bigsplash", 1.0, 1.0),
            }
        }
        // Rolling on the ground (not bailing or off the board).
        let speed = skater.speed().abs();
        let rolling = skater.on_ground
            && speed > 10.0
            && !matches!(
                skater.action,
                skate::Action::Bail
                    | skate::Action::BailManual
                    | skate::Action::BailGrind
                    | skate::Action::OffBoard
                    | skate::Action::OnBoard
            );
        let roll = rolling
            .then(|| self.terrain_sound(skater.terrain, Moment::Roll))
            .flatten();
        Self::keep_looping(&self.clips, &self.sink, &mut self.roll, roll, speed);
        let grind = rail.and_then(|t| self.terrain_sound(t, Moment::Grind));
        let grind_speed = skater.grind.map_or(0.0, |g| g.speed);
        Self::keep_looping(&self.clips, &self.sink, &mut self.grind, grind, grind_speed);
    }

    /// Keeps `slot` playing `wanted` (starting it or changing it over),
    /// pitched and as loud as `speed` makes it; silent with none.
    fn keep_looping(
        clips: &HashMap<String, Clip>,
        sink: &MixerDeviceSink,
        slot: &mut Option<Loop>,
        wanted: Option<TerrainSound>,
        speed: f32,
    ) {
        let Some(sound) = wanted else {
            *slot = None;
            return;
        };
        if slot.as_ref().is_none_or(|l| l.file != sound.file) {
            let Some(clip) = clips.get(&sound.file) else {
                *slot = None;
                return;
            };
            let player = Player::connect_new(sink.mixer());
            player.append(clip.buffer().repeat_infinite());
            *slot = Some(Loop {
                file: sound.file.clone(),
                player,
            });
        }
        let f = (speed / TOP_SPEED).clamp(0.0, 1.0);
        let pitch = sound.pitch.0 + (sound.pitch.1 - sound.pitch.0) * f;
        let loudness = sound.loudness.0 + (sound.loudness.1 - sound.loudness.0) * f;
        if let Some(l) = slot {
            l.player.set_speed(pitch.max(0.05));
            l.player
                .set_volume(sound.volume * loudness * MASTER * (0.3 + 0.7 * f));
        }
    }
}
