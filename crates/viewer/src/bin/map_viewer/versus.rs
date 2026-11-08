//! Two players' games, as the GAME OPTIONS menu chooses them
//! (`mp_game_type_info`): Score Challenge, King of the Hill and Horse.
//!
//! The game's scripts put their messages, rankings and Horse's score
//! panel up (`goal_scorechallenge_activate`, `create_rankings`,
//! `create_horse_panel_message`...); the rules their engine keeps are
//! played here: who has the crown and for how long (`SpawnCrown`,
//! `NewKingYou`), whose turn it is at Horse and who gets a letter
//! (`StartHorseRun`, `EndHorseRun`, `SwitchHorsePlayers`).

use glam::{Mat4, Quat, Vec3};
use qb::Value;
use skate::{Action as SkateAction, ChaseCamera, Skater};

use desa_viewer::camera::FlyCamera;
use desa_viewer::level::Vertex;

use super::App;

/// The game being played.
pub enum Versus {
    /// First to `target` points wins (`goal_scorechallenge_active`'s
    /// `AnySkaterTotalScoreAtLeast`).
    ScoreChallenge {
        target: u32,
    },
    King(King),
    Horse(Horse),
}

/// King of the Hill: hold the crown for `target` seconds.
pub struct King {
    pub target: f32,
    /// Each player's time with the crown.
    pub held: [f32; 2],
    /// Where the crown lies when no one has it.
    pub at: Vec3,
    pub king: Option<usize>,
    /// A while after dropping it before each can take it again.
    pub wait: [f32; 2],
    /// Its turning where it lies.
    pub spin: f32,
    /// Each skater's height, for the crown on their head.
    pub heights: [f32; 2],
}

/// Horse: turns at one spot after another, the lower score getting a
/// letter of the word.
pub struct Horse {
    pub word: String,
    /// Seconds to start a trick (`horse_time_limit`).
    pub limit: f32,
    pub turn: usize,
    pub letters: [usize; 2],
    /// The score set to beat (none at a round's start).
    pub to_beat: Option<u32>,
    /// The `Type = Horse` restart the round is at.
    pub spot: usize,
    pub phase: Phase,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase {
    /// The player's name and what to do (`print_horse_string`), both
    /// held still (for so long, at least a moment for them to come up).
    Intro(f32),
    /// The clock running till a trick's started.
    Clock(f32),
    /// The combo going, till it's landed or bailed (`combos_ended` was
    /// this at its start); then a moment to settle.
    Trick(u32, f32),
    /// How it went, both held still; `true` when the word's spelled.
    Outcome(bool, f32),
}

/// What a Horse turn's score did (`HorseStatusEquals`).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Status {
    SetScore,
    NoScoreSet,
    BeatScore,
    TieScore,
    GotLetter,
}

impl Horse {
    /// The letters a player has: the word's first `n`, spaces kept.
    fn spelled(&self, n: usize) -> String {
        let mut out = String::new();
        let mut got = 0;
        for c in self.word.chars() {
            if got == n {
                break;
            }
            out.push(c);
            if c != ' ' {
                got += 1;
            }
        }
        out
    }

    fn length(&self) -> usize {
        self.word.chars().filter(|c| *c != ' ').count()
    }

    /// The next letter (to get).
    fn letter(&self, n: usize) -> String {
        self.word
            .chars()
            .filter(|c| *c != ' ')
            .nth(n)
            .map(String::from)
            .unwrap_or_default()
    }
}

/// `true` when bailing (the crown's dropped).
fn bailing(skater: &Skater) -> bool {
    matches!(
        skater.action,
        SkateAction::Bail
            | SkateAction::BailManual
            | SkateAction::BailGrind
            | SkateAction::BailFall { .. }
    )
}

/// A time as the game's clock shows it.
fn clock(seconds: f32) -> String {
    let s = seconds.max(0.0).ceil() as u32;
    format!("{}:{:02}", s / 60, s % 60)
}

/// The game's string `name` (strings.q), or `fallback`.
fn string(program: &qb::vm::Program, name: &str, fallback: &str) -> String {
    match program.value(qb::checksum(name)) {
        Some(Value::String(s) | Value::LocalString(s)) => s.clone(),
        _ => fallback.to_string(),
    }
}

fn params(items: &[(&str, Value)]) -> qb::vm::Params {
    items
        .iter()
        .map(|(k, v)| (Some(qb::checksum(k)), v.clone()))
        .collect()
}

impl App<'_> {
    /// GAME OPTIONS' Ready (`StartNetworkGame`): both players from the
    /// start, the game type chosen on.
    pub(super) fn start_versus(&mut self) {
        let c = qb::checksum;
        let (game, target, horse_time, word) = match (&mut self.screen, &self.level) {
            (Some(screen), Some(level)) => {
                let program = level.behaviour.program();
                let s = &mut screen.screen;
                (
                    s.game_type,
                    s.split_pref(program, "target_score"),
                    s.split_pref(program, "horse_time_limit"),
                    s.split_pref(program, "horse_word"),
                )
            }
            _ => (0, None, None, None),
        };
        self.restart();
        self.versus = None;
        if self.player2.is_none() {
            return;
        }
        if let Some((skater, ..)) = &mut self.skating {
            skater.score = 0;
        }
        if let Some(p2) = &mut self.player2 {
            p2.skater.score = 0;
        }
        if game == c("scorechallenge") {
            let target = target
                .and_then(|p| p.number)
                .filter(|n| *n >= 1000)
                .unwrap_or(50_000) as u32;
            self.versus = Some(Versus::ScoreChallenge { target });
            // "Score Challenge: 50,000 pts to win".
            self.run_script(
                "goal_scorechallenge_activate",
                params(&[("score", Value::Integer(target as i32))]),
            );
        } else if game == c("king") {
            // (The target's a time here: `select_game_option`'s `time_120`.)
            let seconds = target
                .filter(|p| p.checksum.is_some_and(|k| k != c("score_50000")))
                .and_then(|p| p.number)
                .filter(|n| (1..=600).contains(n))
                .unwrap_or(120) as f32;
            let crowns = self
                .level
                .as_ref()
                .map(|l| l.nodes.crowns.clone())
                .unwrap_or_default();
            let near = self.skating.as_ref().map_or(Vec3::ZERO, |s| s.0.position);
            // (`SpawnCrown`: at one of the level's crown spots.)
            let at = crowns
                .get(rand_index(crowns.len()))
                .copied()
                .unwrap_or(near + Vec3::new(0.0, 0.0, 300.0));
            let height = |mesh: &desa_viewer::level::Level, scale: f32| {
                mesh.vertices
                    .iter()
                    .map(|v| v.position[1])
                    .fold(0.0f32, f32::max)
                    * scale
            };
            let heights = [
                self.character
                    .as_ref()
                    .map_or(70.0, |ch| height(&ch.mesh, ch.scale)),
                self.player2
                    .as_ref()
                    .map_or(70.0, |p2| height(&p2.character.mesh, p2.character.scale)),
            ];
            self.versus = Some(Versus::King(King {
                target: seconds,
                held: [0.0; 2],
                at,
                king: None,
                wait: [0.0; 2],
                spin: 0.0,
                heights,
            }));
            self.run_script("goal_king_activate", Vec::new());
        } else if game == c("Horse") {
            let limit = horse_time.and_then(|p| p.number).unwrap_or(20) as f32;
            let word = word
                .map(|p| p.text)
                .filter(|w| !w.trim().is_empty())
                .unwrap_or_else(|| "HORSE".into());
            self.versus = Some(Versus::Horse(Horse {
                word,
                limit,
                turn: 0,
                letters: [0; 2],
                to_beat: None,
                spot: rand_index(8),
                phase: Phase::Intro(0.0),
            }));
            self.run_script("create_horse_score_menu", Vec::new());
            self.horse_turn();
        }
    }

    /// The game's string `name` (strings.q), or `fallback`.
    fn game_string(&self, name: &str, fallback: &str) -> String {
        self.level.as_ref().map_or_else(
            || fallback.to_string(),
            |l| string(l.behaviour.program(), name, fallback),
        )
    }

    /// A script of the game's run on its screen.
    fn run_script(&mut self, name: &str, params: qb::vm::Params) {
        if let Some(screen) = &mut self.screen {
            screen.screen.run(qb::checksum(name), params);
        }
    }

    fn player_name(&self, n: usize) -> String {
        let index = match n {
            0 => self.model.character.current,
            _ => self.player2.as_ref().map(|p2| p2.index),
        };
        index
            .and_then(|i| self.model.character.characters.get(i))
            .map_or_else(|| format!("Player {}", n + 1), |c| c.title.clone())
    }

    fn versus_skater(&self, n: usize) -> Option<&Skater> {
        match n {
            0 => self.skating.as_ref().map(|s| &s.0),
            _ => self.player2.as_ref().map(|p2| &p2.skater),
        }
    }

    fn versus_skater_mut(&mut self, n: usize) -> Option<&mut Skater> {
        match n {
            0 => self.skating.as_mut().map(|s| &mut s.0),
            _ => self.player2.as_mut().map(|p2| &mut p2.skater),
        }
    }

    /// Whether player `n` is held still (Horse: not their turn, or the
    /// messages up between turns).
    pub(super) fn versus_frozen(&self, n: usize) -> bool {
        match &self.versus {
            Some(Versus::Horse(h)) => {
                matches!(h.phase, Phase::Intro(_) | Phase::Outcome(..)) || h.turn != n
            }
            _ => false,
        }
    }

    /// Horse: whose turn shows, alone on the whole screen.
    pub(super) fn versus_solo(&self) -> Option<usize> {
        match &self.versus {
            Some(Versus::Horse(h)) => Some(h.turn),
            _ => None,
        }
    }

    /// The game type left (the TWO PLAYERS menu's `SetGameType
    /// freeskate2p`, or out of two players): the game over.
    pub(super) fn versus_check(&mut self) {
        if self.versus.is_none() {
            return;
        }
        let on = self.player2.is_some()
            && self.screen.as_ref().is_some_and(|s| {
                let g = s.screen.game_type;
                s.screen.split_screen && g != qb::checksum("freeskate2p") && g != 0
            });
        if !on {
            self.end_versus();
        }
    }

    fn end_versus(&mut self) {
        if let Some(Versus::Horse(_)) = &self.versus {
            if let Some(screen) = &mut self.screen {
                screen.screen.destroy_id("horse_score_menu");
            }
        }
        self.versus = None;
        if let Some(level) = &mut self.level {
            if let Some((layer, _)) = &level.layers.crown {
                level.renderer.show_layer(*layer, false);
            }
            level.renderer.solo_character2 = false;
        }
        self.model.split_solo = None;
    }

    /// The game's rules, a frame on.
    pub(super) fn update_versus(&mut self, dt: f32) {
        // The RANKINGS up: the game paused under them.
        if self
            .screen
            .as_ref()
            .is_some_and(|s| s.screen.exists("rankings_anchor"))
        {
            self.model.character.paused = true;
            return;
        }
        self.versus_check();
        match &self.versus {
            Some(Versus::ScoreChallenge { target }) => {
                let target = *target;
                let scores = [0, 1].map(|n| self.versus_skater(n).map_or(0, |s| s.score));
                if scores.iter().any(|s| *s >= target) {
                    let rows = [0, 1].map(|n| (self.player_name(n), scores[n]));
                    self.finish_versus(rows.map(|(name, s)| (name, s, s.to_string())), false);
                }
            }
            Some(Versus::King(_)) => self.update_king(dt),
            Some(Versus::Horse(_)) => self.update_horse(dt),
            None => {}
        }
        // Horse's turns: one player on the whole screen.
        let solo = self.versus_solo();
        self.model.split_solo = solo;
        if let Some(level) = &mut self.level {
            level.renderer.solo_character2 = solo == Some(1);
        }
        if solo == Some(1) {
            if let Some(p2) = &self.player2 {
                self.camera = p2.camera;
            }
        }
    }

    /// Someone's won: the RANKINGS, best first (`create_rankings`, by
    /// way of the game's `goal_..._finished`), the game paused under them.
    fn finish_versus(&mut self, rows: [(String, u32, String); 2], king: bool) {
        let mut rows = rows.to_vec();
        rows.sort_by_key(|row| std::cmp::Reverse(row.1));
        if let Some(screen) = &mut self.screen {
            screen.screen.rankings = rows.into_iter().map(|(n, _, s)| (n, s)).collect();
        }
        self.end_versus();
        // (Paused once they're up: `create_rankings`' own
        // `exit_pause_menu` would take them down with the pause.)
        let finished = if king {
            "goal_king_finished"
        } else {
            "goal_scorechallenge_finished"
        };
        self.run_script(finished, Vec::new());
    }

    fn update_king(&mut self, dt: f32) {
        let positions = [0, 1].map(|n| {
            self.versus_skater(n)
                .map(|s| (s.position, s.up, s.heading, bailing(s)))
        });
        let cameras = [
            self.camera,
            self.player2.as_ref().map_or(self.camera, |p2| p2.camera),
        ];
        let Some(Versus::King(k)) = &mut self.versus else {
            return;
        };
        k.spin += dt * 2.0;
        for w in &mut k.wait {
            *w = (*w - dt).max(0.0);
        }
        let mut new_king = None;
        let mut dropped = None;
        match k.king {
            Some(n) => match positions[n] {
                // Bailed: the crown falls where they did.
                Some((at, _, _, true)) => {
                    k.at = at;
                    k.king = None;
                    k.wait[n] = 2.0;
                    dropped = Some(n);
                }
                Some(_) => k.held[n] += dt,
                None => {}
            },
            None => {
                for (n, position) in positions.iter().enumerate() {
                    let Some((at, up, _, bailing)) = *position else {
                        continue;
                    };
                    // (The star floats over its spot, its model 30 to 80
                    // up.)
                    let reach = (at + up * 30.0).distance(k.at + Vec3::Y * 50.0);
                    if !bailing && k.wait[n] <= 0.0 && reach < 80.0 {
                        k.king = Some(n);
                        new_king = Some(n);
                        break;
                    }
                }
            }
        }
        let won = k.king.filter(|n| k.held[*n] >= k.target);
        // The crown: on the king's head, else turning where it lies.
        let placement = match k.king.and_then(|n| positions[n].map(|p| (n, p))) {
            Some((n, (at, up, heading, _))) => Mat4::from_rotation_translation(
                Quat::from_rotation_arc(Vec3::Y, up) * Quat::from_rotation_y(heading),
                // (Over their head, as on the game's `head` bone.)
                at + up * (k.heights[n] * 0.85),
            ),
            None => Mat4::from_rotation_translation(
                Quat::from_rotation_y(k.spin),
                k.at + Vec3::Y * ((k.spin * 1.5).sin() * 4.0),
            ),
        };
        let crown_at = placement.transform_point3(Vec3::ZERO);
        let (held, target, king) = (k.held, k.target, k.king);
        if let Some(level) = &mut self.level {
            if let Some((layer, base)) = &level.layers.crown {
                let vertices: Vec<Vertex> = base
                    .iter()
                    .map(|v| Vertex {
                        position: placement.transform_point3(v.position.into()).into(),
                        normal: placement
                            .transform_vector3(v.normal.into())
                            .normalize_or_zero()
                            .into(),
                        ..*v
                    })
                    .collect();
                level.renderer.update_layer(*layer, 0, &vertices);
                level.renderer.show_layer(*layer, true);
            }
        }
        // "You are king of the hill!", and "You dropped your crown!".
        if let Some(n) = new_king {
            self.run_script("NewKingYou", Vec::new());
            let _ = n;
        }
        if let Some(n) = dropped {
            let text = self.game_string("net_message_dropped_crown_you", "You dropped your crown!");
            if let Some(skater) = self.versus_skater_mut(n) {
                skater.message = Some((text, 2.0));
            }
        }
        if let Some(huds) = &mut self.model.split_huds {
            for n in 0..2 {
                huds[n].extra = Some(format!("Star {} / {}", clock(held[n]), clock(target)));
                // The arrow: which way the crown is, but for the king.
                huds[n].pointer = (king != Some(n)).then(|| pointer(&cameras[n], crown_at));
            }
        }
        if let Some(n) = won {
            let _ = n;
            let rows = [0, 1].map(|n| {
                let ms = (held[n] * 1000.0) as u32;
                (self.player_name(n), ms, clock(held[n]))
            });
            self.finish_versus(rows, true);
        }
    }

    /// A Horse turn starting: the player at the round's spot, their name
    /// and what to do up (`horse_start_run`).
    fn horse_turn(&mut self) {
        let Some(Versus::Horse(h)) = &mut self.versus else {
            return;
        };
        h.phase = Phase::Intro(0.0);
        let (turn, spot) = (h.turn, h.spot);
        let to_beat = h.to_beat;
        let spots: Vec<(Vec3, f32)> = self
            .level
            .as_ref()
            .map(|l| {
                let horse: Vec<_> = l
                    .nodes
                    .spawns
                    .iter()
                    .filter(|s| s.kind.eq_ignore_ascii_case("Horse"))
                    .collect();
                horse
                    .iter()
                    .map(|s| {
                        let f = s.facing();
                        (s.position, f.x.atan2(f.z))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let at = spots
            .get(spot % spots.len().max(1))
            .copied()
            .or_else(|| self.versus_skater(turn).map(|s| (s.position, s.heading)));
        if let Some((position, heading)) = at {
            self.place_versus_skater(turn, position, heading);
        }
        let prerun = match to_beat {
            Some(score) => format!(
                "{} {score}",
                self.game_string("horse_str_scoretobeat", "Score to beat:")
            ),
            None => self.game_string("horse_str_setscore", "Set score"),
        };
        let name = self.player_name(turn);
        self.horse_panel(&name, &prerun, false);
        self.horse_scores(&prerun);
    }

    /// The two lines of a Horse message (`print_horse_string` top and
    /// bottom, or the last ones' `TopFinal` and `BottomFinal`).
    fn horse_panel(&mut self, top: &str, bottom: &str, last: bool) {
        self.run_script("kill_horse_panel_messages", Vec::new());
        let ids = if last {
            ["TopFinal", "BottomFinal"]
        } else {
            ["top", "bottom"]
        };
        for (id, text) in ids.iter().zip([top, bottom]) {
            self.run_script(
                "create_horse_panel_message",
                params(&[
                    ("id", Value::Name(qb::checksum(id))),
                    ("text", Value::String(text.to_string())),
                ]),
            );
        }
    }

    /// Horse's panel at the left: each player's letters, and what to do.
    fn horse_scores(&mut self, what: &str) {
        let Some(Versus::Horse(h)) = &self.versus else {
            return;
        };
        let lines = [0, 1].map(|n| {
            let got = h.spelled(h.letters[n]);
            format!("{}: {}", self.player_name(n), got)
        });
        for (i, text) in lines.iter().map(String::as_str).chain([what]).enumerate() {
            self.run_script(
                "update_horse_score",
                params(&[
                    (
                        "id",
                        Value::Name(qb::checksum(&format!("horse_score_{}", i + 1))),
                    ),
                    ("text", Value::String(text.to_string())),
                ]),
            );
        }
    }

    fn place_versus_skater(&mut self, n: usize, position: Vec3, heading: f32) {
        let Some(level) = &self.level else { return };
        let Some(world) = &level.world else { return };
        match n {
            0 => {
                if let Some((skater, physics, chase)) = &mut self.skating {
                    skater.place(position, heading, physics, world);
                    *chase = ChaseCamera::behind(skater, physics);
                    self.camera = FlyCamera::looking_at(chase.eye, chase.target);
                    self.placement = skater.placement();
                }
            }
            _ => {
                if let Some(p2) = &mut self.player2 {
                    p2.skater.place(position, heading, &p2.physics, world);
                    p2.chase = ChaseCamera::behind(&p2.skater, &p2.physics);
                    p2.camera = FlyCamera::looking_at(p2.chase.eye, p2.chase.target);
                }
            }
        }
    }

    /// Whether Horse's messages are still up (`WaitForPanelDone`).
    fn horse_panels_up(&self) -> bool {
        self.screen.as_ref().is_some_and(|s| {
            ["top", "bottom", "TopFinal", "BottomFinal"]
                .iter()
                .any(|id| s.screen.exists(id))
        })
    }

    fn update_horse(&mut self, dt: f32) {
        let Some(Versus::Horse(h)) = &self.versus else {
            return;
        };
        let (turn, phase) = (h.turn, h.phase);
        let panels = self.horse_panels_up();
        let skater = self.versus_skater(turn).map(|s| {
            (
                s.combo_tricks.is_empty(),
                s.combos_ended,
                bailing(s),
                s.on_ground,
            )
        });
        let mut next = phase;
        let mut score = None;
        match phase {
            Phase::Intro(t) if t > 0.5 && !panels => next = Phase::Clock(self.horse_limit()),
            Phase::Intro(t) => next = Phase::Intro(t + dt),
            Phase::Clock(left) => match skater {
                // The trick's started: the clock stops (`FirstTrickStarted`).
                Some((false, ended, ..)) => next = Phase::Trick(ended, 0.0),
                _ if left - dt <= 0.0 => score = Some(0),
                _ => next = Phase::Clock(left - dt),
            },
            Phase::Trick(start, settle) => match skater {
                // Landed or bailed, and still (`AllSkatersAreIdle`).
                Some((_, ended, bailing, on_ground)) if ended > start => {
                    if !bailing && on_ground && settle >= 0.5 {
                        let total = self
                            .versus_skater(turn)
                            .and_then(|s| s.last_combo.as_ref())
                            .map_or(0, |l| l.total);
                        score = Some(total);
                    } else {
                        next = Phase::Trick(start, settle + dt);
                    }
                }
                _ => {}
            },
            Phase::Outcome(over, t) if t > 0.5 && !panels => {
                if over {
                    // The game's done: the TWO PLAYERS menu again.
                    self.end_versus();
                    self.model.character.paused = true;
                    self.run_script("create_end_run_menu", Vec::new());
                    return;
                }
                // `SwitchHorsePlayers`.
                if let Some(Versus::Horse(h)) = &mut self.versus {
                    h.turn = 1 - h.turn;
                    if h.to_beat.is_none() {
                        h.spot += 1;
                    }
                }
                self.horse_turn();
                return;
            }
            Phase::Outcome(over, t) => next = Phase::Outcome(over, t + dt),
        }
        if let Some(Versus::Horse(h)) = &mut self.versus {
            h.phase = next;
        }
        if let Some(score) = score {
            self.end_horse_turn(score);
        }
        // Each one's letters, and the clock running.
        let Some(Versus::Horse(h)) = &self.versus else {
            return;
        };
        let extra = [0, 1].map(|n| {
            let got = h.spelled(h.letters[n]);
            let got = if got.is_empty() { "-".into() } else { got };
            match h.phase {
                Phase::Clock(left) if n == h.turn => format!("{got}   {}", clock(left)),
                _ => got,
            }
        });
        let frozen = [0, 1].map(|n| self.versus_frozen(n));
        if let Some(huds) = &mut self.model.split_huds {
            for n in 0..2 {
                huds[n].extra = Some(extra[n].clone());
                // (Held still, their last combo's gone.)
                if frozen[n] {
                    huds[n].combo = None;
                }
            }
        }
    }

    fn horse_limit(&self) -> f32 {
        match &self.versus {
            Some(Versus::Horse(h)) => h.limit,
            _ => 0.0,
        }
    }

    /// A Horse turn's over with `score` (`horse_end_run`): set, beaten,
    /// tied, or a letter.
    fn end_horse_turn(&mut self, score: u32) {
        let Some(Versus::Horse(h)) = &mut self.versus else {
            return;
        };
        let n = h.turn;
        let status = match h.to_beat {
            None if score == 0 => Status::NoScoreSet,
            None => Status::SetScore,
            Some(t) if score > t => Status::BeatScore,
            Some(t) if score == t => Status::TieScore,
            Some(_) => Status::GotLetter,
        };
        let mut letter = String::new();
        match status {
            Status::SetScore | Status::BeatScore => h.to_beat = Some(score),
            Status::NoScoreSet => {}
            Status::TieScore => h.to_beat = None,
            Status::GotLetter => {
                letter = h.letter(h.letters[n]);
                h.letters[n] += 1;
                h.to_beat = None;
            }
        }
        let over = h.letters[n] >= h.length();
        h.phase = Phase::Outcome(over, 0.0);
        let word = h.word.clone();
        let postrun = match status {
            Status::SetScore | Status::BeatScore => {
                format!(
                    "{} {score}",
                    self.game_string("horse_str_setscoreis", "Set score is")
                )
            }
            Status::NoScoreSet => self.game_string("horse_str_noscoreset", "No score set"),
            Status::TieScore => self.game_string(
                "horse_str_youtiedthetargetscore",
                "You tied the target score",
            ),
            Status::GotLetter => format!(
                "{} '{letter}'",
                self.game_string("horse_str_getstheletter", "gets the letter")
            ),
        };
        // (`play_appropriate_horse_sound`.)
        let sound = match status {
            _ if over => "PlayYouLoseSound",
            Status::GotLetter => "PlayYouGetLetterSound",
            Status::TieScore => "PlayYouTieScoreSound",
            Status::BeatScore => "PlayYouBeatScoreSound",
            Status::NoScoreSet => "PlayNoScoreSetSound",
            Status::SetScore => "",
        };
        if !sound.is_empty() {
            self.run_script(sound, Vec::new());
        }
        let name = self.player_name(n);
        if over {
            // "Jessie is a" / "HORSE".
            let vowel = word
                .chars()
                .next()
                .is_some_and(|c| "AEIOU".contains(c.to_ascii_uppercase()));
            let is_a = if vowel {
                self.game_string("horse_str_isan", "is an")
            } else {
                self.game_string("horse_str_isa", "is a")
            };
            self.horse_panel(&format!("{name} {is_a}"), &format!("{word}!"), true);
            self.run_script("EndGoal_Horse_Mp", Vec::new());
            self.horse_scores("");
        } else {
            self.horse_panel(&name, &postrun, false);
            self.horse_scores(&postrun);
        }
    }
}

/// Which way `at` is from `camera`'s view, turned clockwise from
/// straight ahead (up the screen).
fn pointer(camera: &FlyCamera, at: Vec3) -> f32 {
    let v = camera.view().transform_point3(at);
    v.x.atan2(-v.z)
}

/// A pick among `n` (the crown's spot, Horse's first), from the clock.
fn rand_index(n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos() as usize);
    nanos % n
}
