// No console window in release builds; the panel shows any errors.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
// (Help texts name placeholders like `<x>` and options like `[,speed]`,
// which rustdoc takes for HTML and links.)
#![allow(rustdoc::invalid_html_tags, rustdoc::broken_intra_doc_links)]

//! DESA Map Viewer: pick a level from your disc image and fly around it.
//!
//! Reads levels straight from the disc (or a folder of extracted `.prg`
//! archives), so no unpacking step is needed. The disc's location is
//! remembered between runs. Any of the playable characters can stand on a
//! spawn point and play their animations.

mod audio;
mod minimap;
mod rumble;
mod settings;
mod sparks;
mod ui;
mod versus;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use clap::Parser;
use glam::{Mat4, Quat, Vec3};
use ngc_anim::CameraPath;
use skate::{
    Action as SkateAction, ChaseCamera, Input, Physics, Rails, Segment, Skater, Stats, TrickBook,
    World,
};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{
    DeviceEvent, DeviceId, ElementState, MouseButton, MouseScrollDelta, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

use desa_viewer::behaviour::Behaviour;
use desa_viewer::camera::{FlyCamera, ScriptedCamera, vertical_fov};
use desa_viewer::character::Character;
use desa_viewer::collision::{self, CollisionView};
use desa_viewer::level::{ColorAnimation, Level};
use desa_viewer::nodes::{LevelNodes, Spawn};
use desa_viewer::objects::{self, LevelObjects};
use desa_viewer::renderer::{self, Renderer, request_device};
use desa_viewer::source::{GameData, LevelInfo};
use settings::{ASPECTS, Bars, FRAME_LIMITS, Settings, Video, WindowMode};

const MOUSE_SENSITIVITY: f32 = 0.0025;
const DEFAULT_SPEED: f32 = 1500.0;

#[derive(Parser)]
#[command(
    name = "desa-map-viewer",
    about = "Fly around the levels of Disney's Extreme Skate Adventure"
)]
struct Args {
    /// Disc image (.iso) or a folder with the extracted .prg archives
    /// (default: the last one opened, or one found next to the program)
    data: Option<PathBuf>,
    /// Level to open first, by archive name (e.g. HUB, beach, ToyStory_Bedroom)
    #[arg(long)]
    level: Option<String>,
    /// Character to show, by id (e.g. jessie, buzz, simba)
    #[arg(long)]
    character: Option<String>,
    /// Don't play the game's opening movies (the logos and the intro) at
    /// the start
    #[arg(long)]
    no_intro: bool,
    /// Render one frame, panel included, to this PNG and exit
    #[arg(long)]
    screenshot: Option<PathBuf>,
    /// For --screenshot: view from this camera path (by name) at --time
    #[arg(long, requires = "screenshot")]
    camera_path: Option<String>,
    /// For --screenshot: the character's animation (default StandIdle)
    #[arg(long, requires = "screenshot")]
    animation: Option<String>,
    /// For --screenshot: seconds into the animation (and blinking, and the
    /// camera path)
    #[arg(long, requires = "screenshot", default_value_t = 0.0)]
    time: f32,
    /// For --screenshot: leave out the panel, rails and spawn markers
    #[arg(long, requires = "screenshot")]
    clean: bool,
    /// For --screenshot: image size as WIDTHxHEIGHT
    #[arg(long, requires = "screenshot", default_value = "1400x850", value_parser = parse_size)]
    size: (u32, u32),
    /// For --screenshot: the picture's shape, as WIDTH:HEIGHT (the Video
    /// Options' Aspect Ratio), black bars round it
    #[arg(long, requires = "screenshot", value_parser = parse_aspect)]
    aspect: Option<(u32, u32)>,
    /// For --screenshot: a loading screen's picture round it (by name,
    /// e.g. loadscrn_pizza_buzz), or `custom` for bars.png
    #[arg(long, requires = "aspect")]
    bars: Option<String>,
    /// For --screenshot: the spawn point (by number in the panel's list,
    /// from 0) the character stands on, instead of the level's start
    #[arg(long, requires = "screenshot")]
    spawn: Option<usize>,
    /// For --screenshot: camera angle around the character in degrees
    /// (0 = in front, 90 = their left side)
    #[arg(
        long,
        requires = "screenshot",
        default_value_t = 0.0,
        allow_negative_numbers = true
    )]
    orbit: f32,
    /// For --screenshot: camera distance from the character
    #[arg(long, requires = "screenshot", default_value_t = 170.0)]
    distance: f32,
    /// For --screenshot: the camera as x,y,z,yaw,pitch (degrees; yaw 0
    /// looks down -Z)
    #[arg(long, requires = "screenshot", allow_hyphen_values = true, value_parser = FlyCamera::parse)]
    camera: Option<FlyCamera>,
    /// For --screenshot: look at the object or pedestrian with this node
    /// name (e.g. TRG_Goal_Letter_S)
    #[arg(long, requires = "screenshot")]
    object: Option<String>,
    /// For --screenshot: skate the character forward (holding W) for this
    /// many seconds first, and look through the chase camera
    #[arg(long, requires = "character", default_value_t = 0.0)]
    skate: f32,
    /// For --skate: keys pressed and let go at times, as "seconds:+Key" and
    /// "seconds:-Key" separated by commas (keys W A S D Space E Q F R J L,
    /// and Enter: taking a warp or goal offered, or A on the game's screen);
    /// without it W is held throughout
    #[arg(long, requires = "screenshot")]
    skate_keys: Option<String>,
    /// For --skate: put the skater somewhere at a time, as
    /// "seconds@x,y,z,heading[,speed]" (heading in degrees, 0 along +Z,
    /// and going that way at that speed in the air); can be
    /// given more than once (a goal that starts it elsewhere, say)
    #[arg(long, requires = "skate")]
    skate_place: Vec<String>,
    /// For --skate: start at x,y,z facing a heading in degrees (0 along +Z,
    /// 90 along +X) instead of the level's start
    #[arg(long, requires = "screenshot", allow_hyphen_values = true)]
    skate_from: Option<String>,
    /// For --skate: skate a two-minute run with this many seconds left on
    /// its clock (with --goal, the goal's clock, if it has one)
    #[arg(long, requires = "skate")]
    run: Option<f32>,
    /// For --skate: which of the game's chase cameras, 0 to 3 (near,
    /// standard, far, standard LTG; default standard)
    #[arg(long, requires = "skate")]
    chase_camera: Option<usize>,
    /// For --skate: play the level's score goal, `high` or `pro` (from its
    /// start)
    #[arg(long, requires = "skate", value_parser = ["high", "pro"])]
    score_goal: Option<String>,
    /// For --skate: cheats on (perfect_manual, perfect_rail,
    /// perfect_skitch, always_special, moon, slomo, stats_13)
    #[arg(long, requires = "skate", value_delimiter = ',')]
    cheat: Vec<String>,
    /// For --skate: play the level's race (from its start)
    #[arg(long, requires = "skate")]
    race: bool,
    /// For --skate: two players' free skate, player two this character
    /// (by id), the screen split; player two holds Up throughout unless
    /// --skate-keys says otherwise (Up Down Left Right, N0-N5)
    #[arg(long, requires = "skate")]
    two_player: Option<String>,
    /// For --two-player: the halves side by side (the game's Vertical)
    #[arg(long, requires = "two_player")]
    side_by_side: bool,
    /// For --two-player: play one of two players' games from the start
    /// (scorechallenge, king or horse), as GAME OPTIONS' Ready does
    #[arg(long, requires = "two_player")]
    versus: Option<String>,
    /// For --skate: play the level's S-K-A-T-E letters goal (from its start)
    #[arg(long, requires = "skate")]
    letters: bool,
    /// For --skate: bring up the game's pause menu at the end
    #[arg(long, requires = "skate")]
    pause: bool,
    /// For --skate: presses to the game's screen at the end, in order (u up,
    /// d down, l left, r right, c choose, b back, w wait): its pause menu with --pause, else
    /// whatever it shows (a goal's speech box)
    #[arg(long, requires = "skate", default_value = "")]
    pause_keys: String,
    /// For --screenshot in the Skate Shop with --character: the game's
    /// main menu, given these presses in order (u d l r up down left
    /// right, c choose, b back, w wait): its skater select, say.
    #[arg(long, requires = "character", default_value = "")]
    menu_keys: String,
    /// For --screenshot of the kid: its parts coloured, as
    /// "Part:hue:saturation:value" separated by commas (`Shirt:0:100:50`)
    #[arg(long, requires = "character", default_value = "")]
    kid_color: String,
    /// The Advanced menu's look, as NAME=VALUE separated by commas:
    /// fog (metres to half-hidden; 0 off), exposure (stops), warmth, tint
    /// (-1 to 1), contrast, saturation (1 as it is)
    #[arg(long, default_value = "")]
    look: String,
    /// For --skate: play another of the level's goals from its own
    /// scripts, by its type (`Gaps`, `Gaps2`...)
    #[arg(long, requires = "skate")]
    goal: Option<String>,
    /// For --goal: scripts to run as it starts, as if the skater had done
    /// what runs them (a gap's script: `StrengthGrind`)
    #[arg(long, requires = "goal", value_delimiter = ',')]
    goal_script: Vec<String>,
    /// For --replay-at: which replay camera (0 as played, 1 behind, 2
    /// front, 3 left, 4 right)
    #[arg(long, requires = "replay_at")]
    replay_camera: Option<usize>,
    /// For --run: once skated, show the run's replay this many seconds in
    #[arg(long, requires = "run")]
    replay_at: Option<f32>,
    /// For --screenshot: also show what goals add later (pickups, goal
    /// pedestrians, warp portals)
    #[arg(long, requires = "screenshot")]
    goal_objects: bool,
    /// For --screenshot: raise the character this far off the ground, as if
    /// in the air (animations leave jump height to the game's physics)
    #[arg(long, requires = "screenshot", default_value_t = 0.0)]
    lift: f32,
    /// For --screenshot: camera height above the character's feet
    #[arg(
        long,
        requires = "screenshot",
        default_value_t = 75.0,
        allow_negative_numbers = true
    )]
    camera_height: f32,
}

fn parse_size(text: &str) -> Result<(u32, u32), String> {
    let (w, h) = text
        .split_once(['x', 'X'])
        .ok_or("expected WIDTHxHEIGHT, e.g. 1920x1080")?;
    let parse = |v: &str| v.trim().parse::<u32>().map_err(|e| e.to_string());
    let (w, h) = (parse(w)?, parse(h)?);
    if !(16..=8192).contains(&w) || !(16..=8192).contains(&h) {
        return Err("width and height must be 16 to 8192".into());
    }
    Ok((w, h))
}

/// What a goal asks, for its pop-up: its `goal_text`, else its name.
fn objective(name: &str, text: &str) -> String {
    if text.is_empty() {
        name.to_string()
    } else {
        text.to_string()
    }
}

/// A goal's settings for its HUD counter's scripts: its caption read
/// from the global it names (`cp_hud_counter_caption_collect_string`),
/// and not its pro (whose hints the counter would put up again).
fn counter_params(b: &Behaviour, program: &qb::vm::Program) -> qb::vm::Params {
    let caption = qb::checksum("hud_counter_caption");
    b.goal_params
        .iter()
        .filter(|(k, _)| *k != Some(qb::checksum("trigger_obj_id")))
        .map(|(k, v)| match (k, v) {
            (Some(k), qb::Value::Name(n)) if *k == caption => (
                Some(*k),
                program.value(*n).cloned().unwrap_or_else(|| v.clone()),
            ),
            item => (*item.0, item.1.clone()),
        })
        .collect()
}

/// A goal ended: one of the level's by its type (a score, letters or
/// race goal), or one played from its scripts, by its name and text.
enum GoalEnded {
    Kind(&'static str),
    Goal(String, String),
}

/// The game's messages for a goal's end: won, its name, a check and
/// "Complete!" (`goal_success_messages`), the objective taken down; lost,
/// an X and "Incomplete!" (`goal_fail`, which takes it down too).
fn end_messages(s: &mut desa_viewer::screen::Screen, won: bool, name: &str, text: &str) {
    let string = |t: &str| qb::Value::String(t.to_string());
    let params = goal_panel_params(&[
        ("view_goals_text", string(name)),
        ("goal_text", string(&objective(name, text))),
    ]);
    if won {
        s.destroy_id("current_goal");
        s.run(qb::checksum("goal_success_messages"), params);
    } else {
        s.run(qb::checksum("goal_fail"), params);
    }
}

/// A race's settings for its HUD counter's scripts (its caption read from
/// the global it names), as `counter_params` a goal's.
fn race_counter_params(program: &qb::vm::Program, level: &str) -> qb::vm::Params {
    let caption = qb::checksum("hud_counter_caption");
    desa_viewer::goals::generic_goal(program, level, "Race")
        .map(|g| g.params)
        .unwrap_or_default()
        .into_iter()
        .filter(|(k, _)| *k != qb::checksum("trigger_obj_id"))
        .map(|(k, v)| match v {
            qb::Value::Name(n) if k == caption => (
                Some(k),
                program.value(n).cloned().unwrap_or(qb::Value::Name(n)),
            ),
            v => (Some(k), v),
        })
        .collect()
}

/// Named parameters for the game's panel scripts.
fn goal_panel_params(pairs: &[(&str, qb::Value)]) -> qb::vm::Params {
    pairs
        .iter()
        .map(|(k, v)| (Some(qb::checksum(k)), v.clone()))
        .collect()
}

fn parse_aspect(text: &str) -> Result<(u32, u32), String> {
    let (w, h) = text
        .split_once(':')
        .ok_or("expected WIDTH:HEIGHT, e.g. 16:9")?;
    let parse = |v: &str| v.trim().parse::<u32>().map_err(|e| e.to_string());
    match (parse(w)?, parse(h)?) {
        (0, _) | (_, 0) => Err("width and height must be more than 0".into()),
        aspect => Ok(aspect),
    }
}

// (Unused when the game build takes this file in as a module.)
#[allow(dead_code)]
fn main() -> Result<()> {
    run(false)
}

/// Runs the viewer, or with `game` the game alone: no panel, debug keys or
/// markers, booting through the opening movies into the main menu.
pub fn run(game: bool) -> Result<()> {
    let args = Args::parse();
    let mut settings = Settings::load();
    if !args.look.is_empty() {
        settings.look = parse_look(&args.look)?;
        settings.read_only = true;
    }
    let data_path = args
        .data
        .clone()
        .or_else(|| settings.data_path.clone().filter(|p| p.exists()))
        .or_else(settings::find_game_data);

    if let Some(out) = &args.screenshot {
        let path = data_path.context("no game data found; pass the disc image's path")?;
        return screenshot(&path, &args, out);
    }

    let mut app = App::new(&mut settings);
    if game {
        app.set_game_mode();
    }
    if let Some(path) = data_path {
        app.open_data(&path);
    } else if game {
        // Nothing to play: the panel, to open the disc.
        app.model.panel_open = true;
    }
    // The game's opening, as it boots (`startup_loading_screen`): the
    // publisher's, Disney Interactive's and Toys for Bob's logos, then
    // the intro.
    if !args.no_intro && app.model.intro {
        app.movie_queue
            .extend(["ATVI", "DI_Logo", "TFBlogo", "intro"].map(String::from));
    }
    // Show the requested character, else the last one shown.
    if let Some(id) = args
        .character
        .clone()
        .or_else(|| app.settings.last_character.clone())
    {
        if let Some(i) = app.character_index(&id) {
            app.load_character(Some(i));
        }
    }
    // As the game boots: the Skate Shop and its main menu (unless a level
    // was asked for, or the panel says not to). Else the last level
    // viewed, else the hub.
    let main_menu = game || (args.level.is_none() && app.model.start_menu);
    if main_menu {
        app.open_main_menu();
    } else if let Some(level) = args
        .level
        .or_else(|| app.settings.last_level.clone())
        .or_else(|| Some("HUB".into()))
    {
        if let Some(i) = app
            .model
            .levels
            .iter()
            .position(|l| l.id.eq_ignore_ascii_case(&level))
        {
            app.start_load(i);
        }
    }

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    event_loop.run_app(&mut app)?;
    app.settings.speed = Some(app.model.speed);
    app.settings.save();
    match app.error {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

struct LoadedLevel {
    /// The level's archive name (`HUB`, `beach`...).
    id: String,
    renderer: Renderer,
    /// Collision triangles, for keeping spawn cameras out of walls.
    collision: Vec<desa_viewer::collision::ColorVertex>,
    nodes: LevelNodes,
    start: FlyCamera,
    /// Where a character stands until you go to a spawn point.
    home: Mat4,
    /// The level's camera paths, by name.
    camera_paths: Vec<(String, CameraPath)>,
    objects: LevelObjects,
    layers: ObjectLayers,
    behaviour: Behaviour,
    /// The collision as a world to skate on.
    world: Option<World>,
    /// Rails on moving objects (each object's model's own, in mesh space
    /// about its origin), the world's moving rails in this order.
    object_rails: Vec<ObjectRails>,
    /// The vehicles with collision of their own (their models' `.col`:
    /// the toy cars), and its object in the world.
    vehicle_collision: Vec<(usize, u32)>,
    /// Animated vertex colors of the level, its sky and its goal geometry.
    colors: [ColorAnimation; 3],
    /// The level's warps to other levels.
    portals: Vec<Portal>,
    /// The pros offering the goals the viewer plays.
    pros: Vec<GoalPro>,
    /// What each teleporter plays and says.
    teleport_effects: HashMap<u32, desa_viewer::triggers::TeleportEffect>,
    /// The level's scene and textures, kept to make a layer for a hidden
    /// sector when a script creates it; those made so far.
    scene: Vec<u8>,
    scene_textures: Option<Vec<u8>>,
    sector_layers: HashMap<u32, Option<usize>>,
    /// Where each breakable sector is.
    sector_centres: HashMap<u32, Vec3>,
    /// The bouncy objects, knocked about.
    bouncies: Vec<BouncyState>,
    /// The level's pieces scripts move.
    movers: Vec<MoverState>,
    /// The screen a movie plays on (the `moviescreen` sector: the Skate
    /// Shop's TV, the Hub's), in a layer of its own; the movie playing
    /// there and when it started.
    movie_screen: Option<usize>,
    /// The level's texture file, for textures its pieces' scripts swap in
    /// (`Obj_ReplaceTexture`), and those decoded so far, by name.
    texture_file: Option<Vec<u8>>,
    swapped_textures: HashMap<String, Option<desa_viewer::level::TextureData>>,
    screen_movie: Option<(desa_viewer::movie::Movie, Instant, bool)>,
    /// The breakables (trigger object: its script, what it shatters, its
    /// sound) and the triggers already broken.
    breakables: HashMap<u32, (u32, Vec<u32>, Option<u32>)>,
    /// Trigger geometry's own scripts, by collision object, run when the
    /// skater touches it.
    touch_scripts: HashMap<u32, u32>,
    broken: HashSet<u32>,
    /// The level from above, for the map in the corner.
    minimap: Option<minimap::Minimap>,
    /// The level's particle effects (steam, sparks, dust), and their
    /// textures' places in the renderer's list by name.
    particles: desa_viewer::particles::Particles,
    particle_textures: HashMap<u32, usize>,
    /// Which marker sets the renderer currently has: (rails, spawns).
    markers: (bool, bool),
}

/// Loads a level from the game data and uploads it to the GPU.
/// A moving object's rails (its index, and each segment's ends about its
/// origin with the segment's node).
type ObjectRails = (usize, Vec<(Vec3, Vec3, u32)>);

fn load_level(
    data: &mut GameData,
    info: &LevelInfo,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    format: wgpu::TextureFormat,
) -> Result<(LoadedLevel, String)> {
    let mut files = data
        .load_level(&info.id)
        .with_context(|| format!("could not read {}", info.title))?;
    let nodes = files
        .nodes
        .as_deref()
        .and_then(|n| LevelNodes::from_bytes(n).ok())
        .unwrap_or_default();
    // Objects' scripts: the game's shared scripts, then the level's own.
    let mut scripts = data.global_scripts().unwrap_or_else(|e| {
        eprintln!("warning: no shared scripts: {e:#}");
        Vec::new()
    });
    scripts.extend(files.scripts.iter().cloned());
    let mut behaviour = Behaviour::new(&nodes, &scripts);
    // (Its goal manager names goals as the level's scripts do.)
    behaviour.goal_prefix = desa_viewer::goals::goal_prefix(&info.id);
    // Its goals added (`<level>_Goals`, from its start-up script): the
    // ones its triggers turn on themselves need their set-up (Zurg's low
    // gravity zones, `zurg_goal_MoonGrav`).
    for level_name in [info.id.clone(), behaviour.goal_prefix.clone()] {
        let script = qb::checksum(&format!("{level_name}_Goals"));
        if behaviour.program().has_script(script) {
            behaviour.run_level_script(script, Vec::new());
            break;
        }
    }
    // Breakables (what touching a trigger shatters): those that are sectors
    // there at the start get layers of their own, to take away.
    let breakables = desa_viewer::triggers::breakables(&nodes, behaviour.program());
    // The other trigger geometry's scripts, run as the skater touches it
    // (a goal's things: Canyon's pesky birds). Teleporters, gaps and
    // breakables are done their own ways.
    let touch_scripts: HashMap<u32, u32> = {
        let teleports = desa_viewer::triggers::teleports(&nodes, behaviour.program());
        let gaps = desa_viewer::triggers::gaps(&nodes, behaviour.program());
        nodes
            .geometry_scripts
            .iter()
            .filter(|(o, _)| {
                !teleports.contains_key(o) && !gaps.contains_key(o) && !breakables.contains_key(o)
            })
            // And the rail nodes' (grinding Tarzan's bowsprit).
            .chain(
                nodes
                    .rail_scripts
                    .iter()
                    .filter(|(o, _)| !gaps.contains_key(o)),
            )
            .copied()
            .collect()
    };
    // Bouncy objects there at the start get layers of their own too, to
    // knock about.
    let bouncy_names: HashSet<u32> = nodes
        .bouncies
        .iter()
        .filter(|b| b.created_at_start)
        .map(|b| b.name)
        .collect();
    let breakable_sectors: HashSet<u32> = breakables
        .values()
        .flat_map(|(_, names, _)| names.iter().copied())
        .filter(|n| !nodes.hidden_sectors.contains(n) && behaviour.object(*n).is_none())
        .collect();
    // Sectors that aren't there at the start go in their own layer.
    let hidden = &nodes.hidden_sectors;
    // The pieces scripts move are in layers of their own too.
    let mover_names: HashSet<u32> = nodes.movers.iter().map(|m| m.name).collect();
    // The screen movies play on (`playmovie ... mesh = moviescreen`).
    let screen_name = qb::checksum("moviescreen");
    let world = Level::from_bytes_filtered(&files.scene, files.textures.as_deref(), |s| {
        !hidden.contains(&s)
            && !breakable_sectors.contains(&s)
            && !bouncy_names.contains(&s)
            && !mover_names.contains(&s)
            && s != screen_name
    })?;
    let goal_geometry = Level::from_bytes_filtered(&files.scene, files.textures.as_deref(), |s| {
        hidden.contains(&s) && !mover_names.contains(&s)
    })?;
    let sets = data.animation_sets().unwrap_or_else(|e| {
        eprintln!("warning: no pedestrian animations: {e:#}");
        Default::default()
    });
    data.add_animations(&mut files, &objects::needed_animations(&nodes, &sets))?;
    let objects = LevelObjects::build(&nodes, &files, &sets);
    for missing in &objects.missing {
        eprintln!("{}: couldn't load {missing}", info.id);
    }
    let mut skate_world = files
        .collision
        .as_deref()
        .and_then(|c| ngc_collision::Collision::parse(c).ok())
        .map(|c| {
            World::new(c).with_rails(
                Rails::named(
                    nodes
                        .rails
                        .iter()
                        .map(|r| {
                            let segment = Segment {
                                start: r.start,
                                end: r.end,
                            };
                            (segment, r.node)
                        })
                        .collect(),
                )
                .with_lip_override(
                    nodes
                        .rails
                        .iter()
                        .filter(|r| r.lip_override)
                        .map(|r| r.node),
                )
                .with_chain(nodes.rail_chain.clone()),
            )
        })
        .map(|mut world| {
            // Rails waiting for a script to create them.
            for r in nodes.rails.iter().filter(|r| !r.created_at_start) {
                world.rails.set_present(r.node, false);
            }
            world
        });
    // Rails on moving objects, from their models' own node arrays
    // (`Veh_Viper_NodeArray`: the toy cars, the lifeboat, the crane), there
    // as the objects start; moved with them as they go.
    let object_rails: Vec<ObjectRails> = nodes
        .objects
        .iter()
        .enumerate()
        .filter_map(|(i, o)| {
            let file = o.model.rsplit(['/', '\\']).next()?;
            let stem = file.split('.').next()?;
            let array = behaviour
                .program()
                .value(qb::checksum(&format!("{stem}_NodeArray")))
                .and_then(qb::Value::as_array)?;
            let rails = desa_viewer::nodes::rails_in(array);
            (!rails.is_empty()).then_some((i, rails))
        })
        .collect();
    if let Some(world) = skate_world.as_mut() {
        let placed = object_rails.iter().flat_map(|(i, rails)| {
            let o = &nodes.objects[*i];
            let (at, turn) = (o.position, o.rotation());
            rails.iter().map(move |&(a, b, name)| {
                (
                    Segment {
                        start: at + turn * a,
                        end: at + turn * b,
                    },
                    name,
                )
            })
        });
        world.rails.add_moving(placed.collect());
    }
    // Vehicles with collision of their own (the toy cars' models' `.col`),
    // each an object of the world placed where it is.
    let mut vehicle_collision = Vec::new();
    if let Some(world) = skate_world.as_mut() {
        for (i, o) in nodes.objects.iter().enumerate() {
            if o.kind != desa_viewer::nodes::ObjectKind::Vehicle {
                continue;
            }
            let Some(stem) = o.model.strip_suffix(".mdl") else {
                continue;
            };
            let Some(data) = files.models.get(&format!("{stem}.col")) else {
                continue;
            };
            let Ok(collision) = ngc_collision::Collision::parse(data) else {
                continue;
            };
            let checksum = qb::checksum(&format!("{}#col", o.label));
            world.add_vehicle_collision(checksum, &collision.objects);
            world.place(
                checksum,
                Some(Mat4::from_rotation_translation(o.rotation(), o.position)),
            );
            vehicle_collision.push((i, checksum));
        }
    }
    // Bouncy objects are knocked away, not run into.
    if let Some(world) = skate_world.as_mut() {
        for name in &bouncy_names {
            world.disable(*name);
        }
    }
    let sky = files
        .sky
        .as_ref()
        .and_then(|(scene, tex)| Level::from_bytes(scene, tex.as_deref()).ok());
    let collision = files
        .collision
        .as_deref()
        .and_then(|c| collision::from_bytes(c).ok());

    let (center, radius) = world.focus;
    let start = nodes.start().map_or_else(
        || FlyCamera::looking_at(center + Vec3::new(0.0, radius * 0.5, radius * 1.2), center),
        |spawn| spawn.camera_clear_of(collision.as_deref().unwrap_or_default()),
    );
    let camera_paths: Vec<(String, CameraPath)> = files
        .cameras
        .iter()
        .filter_map(|(name, data)| Some((name.clone(), CameraPath::parse(data).ok()?)))
        .collect();
    let home = nodes
        .start()
        .map_or(Mat4::from_translation(center), placement_at);
    let stats = format!(
        "{} triangles, {} textures, {} rail segments, {} spawn points, {} objects, {} pedestrians ({} more for goals){}",
        world.indices.len() / 3,
        world.textures.len() - 1,
        nodes.rails.len(),
        nodes.spawns.len(),
        nodes.objects.len() - objects.crowd.len() - objects.goal_crowd.len(),
        objects.crowd.len(),
        objects.goal_crowd.len(),
        if collision.is_some() {
            ", collision"
        } else {
            ""
        },
    );
    let mut renderer = Renderer::new(
        device.clone(),
        queue.clone(),
        format,
        &world,
        sky.as_ref(),
        collision.as_deref(),
    );
    let colors = [
        world.color_animation.clone(),
        sky.as_ref()
            .map(|s| s.color_animation.clone())
            .unwrap_or_default(),
        goal_geometry.color_animation.clone(),
    ];
    // Warps, each Hub portal's film strip in a layer of its own (shown
    // when it appears).
    let portals = desa_viewer::warps::warps(behaviour.program(), &nodes)
        .into_iter()
        .map(|warp| {
            let strip = warp.sector.and_then(|sector| {
                let mesh =
                    Level::from_bytes_filtered(&files.scene, files.textures.as_deref(), |s| {
                        s == sector
                    })
                    .ok()
                    .filter(|m| !m.vertices.is_empty())?;
                let layer = renderer.add_layer(&mesh, false);
                renderer.show_layer(layer, false);
                Some((layer, mesh.vertices))
            });
            Portal {
                warp,
                strip,
                appeared: None,
                declined: false,
            }
        })
        .collect();
    let layers = ObjectLayers {
        goal_geometry: renderer.add_layer(&goal_geometry, false),
        props: renderer.add_layer(&objects.props.mesh, false),
        goal_props: renderer.add_layer(&objects.goal_props.mesh, false),
        crowd: renderer.add_layer(&objects.crowd.mesh, true),
        goal_crowd: renderer.add_layer(&objects.goal_crowd.mesh, true),
        crown: files
            .models
            .get("crown/crown.mdl")
            .and_then(|data| {
                Level::from_bytes(data, files.models.get("crown/crown.tex").map(Vec::as_slice)).ok()
            })
            .filter(|mesh| !mesh.vertices.is_empty())
            .map(|mesh| {
                let layer = renderer.add_layer(&mesh, false);
                renderer.show_layer(layer, false);
                (layer, mesh.vertices)
            }),
    };
    let minimap = collision.as_deref().and_then(minimap::Minimap::new);
    let teleport_effects = desa_viewer::triggers::teleport_effects(&nodes, behaviour.program());
    let mut behaviour = behaviour;
    let pros = goal_pros(behaviour.program(), &info.id, &behaviour);
    // The pros are there from the start, as in the career (the scripts of
    // those around them look for them).
    for pro in &pros {
        behaviour.set_alive(pro.object, true);
    }
    // The bouncy objects, each in a layer of its own.
    let mut bouncies = Vec::new();
    // (Those not there at the start are made hidden, for scripts to
    // create.)
    for b in &nodes.bouncies {
        let Ok(mut mesh) =
            Level::from_bytes_filtered(&files.scene, files.textures.as_deref(), |s| s == b.name)
        else {
            continue;
        };
        if mesh.vertices.is_empty() {
            continue;
        }
        let mut centre = mesh.focus.0;
        if centre.length() < b.position.distance(centre) {
            for v in &mut mesh.vertices {
                v.position = (Vec3::from(v.position) + b.position).to_array();
            }
            centre += b.position;
        }
        let (low, high) = mesh.vertices.iter().fold(
            (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
            |(lo, hi), v| (lo.min(v.position.into()), hi.max(v.position.into())),
        );
        let layer = renderer.add_layer(&mesh, false);
        renderer.show_layer(layer, b.created_at_start);
        bouncies.push(BouncyState {
            shown: b.created_at_start,
            collided: false,
            spec: b.clone(),
            layer,
            base: mesh.vertices,
            centre,
            reach: ((high - low).with_y(0.0).length() * 0.5).max(10.0),
            half_height: ((high.y - low.y) * 0.5).max(4.0),
            offset: Vec3::ZERO,
            velocity: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            spin: Vec3::ZERO,
            moving: false,
            since: f32::INFINITY,
        });
    }
    // The movie screen, in a layer of its own to draw a movie's frames on
    // (showing its own picture till one plays).
    let movie_screen = Level::from_bytes_filtered(&files.scene, files.textures.as_deref(), |s| {
        s == screen_name
    })
    .ok()
    .filter(|mesh| !mesh.vertices.is_empty())
    .and_then(|mesh| renderer.add_layer(&mesh, false));
    // The level's moving pieces, each in a layer of its own, shown while
    // its object is there and put where it is.
    let mut movers = Vec::new();
    // The stage's light (the Skate Shop's, where its camera stands over
    // the stage), for the film strip that turns there.
    let stage_light = camera_paths
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("Camera_stage"))
        .map(|(_, path)| path_camera(path, path_length(path)).position);
    for (i, m) in nodes.movers.iter().enumerate() {
        let Ok(mesh) =
            Level::from_bytes_filtered(&files.scene, files.textures.as_deref(), |s| s == m.name)
        else {
            continue;
        };
        if mesh.vertices.is_empty() {
            continue;
        }
        let layer = renderer.add_layer(&mesh, false);
        renderer.show_layer(layer, m.created_at_start);
        if let (false, Some(world)) = (m.created_at_start, skate_world.as_mut()) {
            world.disable(m.name);
        }
        // Most pieces' geometry is in their own space, round the origin
        // (the bedroom's toilet, Pizza's arcade screen): put where their
        // node is, turned its way, they and their collision. (A few are
        // in the level's space already, where their node is.)
        let (lo, hi) = mesh.vertices.iter().fold(
            (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
            |(lo, hi), v| (lo.min(v.position.into()), hi.max(v.position.into())),
        );
        let (centre, radius) = ((lo + hi) / 2.0, (hi - lo).length() / 2.0);
        let local =
            centre.length() < radius + 1.0 && m.position.distance(centre) > radius * 2.0 + 50.0;
        let mut base = mesh.vertices;
        if local {
            let turn = m.rotation();
            for v in &mut base {
                v.position = (m.position + turn * Vec3::from(v.position)).to_array();
                v.normal = (turn * Vec3::from(v.normal)).to_array();
            }
            renderer.update_layer(layer, 0, &base);
            if let Some(world) = skate_world.as_mut() {
                world.place(
                    m.name,
                    Some(Mat4::from_rotation_translation(turn, m.position)),
                );
            }
        }
        // A piece turning round its middle with no normals of its own (the
        // skater select's film strip, round the stage): lit from the
        // stage's camera, the panel facing it bright, those turned away
        // dim, as the game's light on the stage has it.
        let carousel = base.iter().all(|v| v.normal == [0.0; 3]) && {
            let (lo, hi) = base.iter().fold(
                (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
                |(lo, hi), v| (lo.min(v.position.into()), hi.max(v.position.into())),
            );
            (lo.x..=hi.x).contains(&m.position.x) && (lo.z..=hi.z).contains(&m.position.z)
        };
        let spot = stage_light.filter(|_| carousel);
        movers.push(MoverState {
            object: behaviour.mover(i),
            spot,
            name: m.name,
            layer,
            base,
            local,
            pivot: m.position,
            placed: m.rotation(),
            // (Lit, it's put on the first update.)
            last: (
                m.created_at_start,
                if spot.is_some() {
                    Vec3::NAN
                } else {
                    m.position
                },
                m.rotation(),
            ),
        });
    }
    // The breakable sectors, shown until broken, and where each is.
    let mut sector_layers = HashMap::new();
    let mut sector_centres = HashMap::new();
    for &name in &breakable_sectors {
        let Ok(mesh) =
            Level::from_bytes_filtered(&files.scene, files.textures.as_deref(), |s| s == name)
        else {
            continue;
        };
        if mesh.vertices.is_empty() {
            continue;
        }
        sector_centres.insert(name, mesh.focus.0);
        let layer = renderer.add_layer(&mesh, false);
        renderer.show_layer(layer, true);
        sector_layers.insert(name, layer);
    }
    let particles = desa_viewer::particles::Particles::new(behaviour.program(), &nodes);
    // The particles' textures: a soft round one first (for those missing),
    // then the level's.
    let mut particle_list = vec![soft_disc()];
    let mut particle_textures = HashMap::new();
    for (name, data) in &files.particle_images {
        let image = ngc_texture::img::ImgFile::parse(data).and_then(|f| f.decode());
        if let Ok(image) = image {
            particle_textures.insert(*name, particle_list.len());
            particle_list.push(desa_viewer::level::TextureData {
                checksum: *name,
                width: image.width,
                height: image.height,
                levels: vec![image.rgba],
            });
        }
    }
    renderer.set_particle_textures(&particle_list);
    Ok((
        LoadedLevel {
            id: info.id.clone(),
            renderer,
            collision: collision.unwrap_or_default(),
            nodes,
            start,
            home,
            camera_paths,
            objects,
            layers,
            behaviour,
            world: skate_world,
            object_rails,
            vehicle_collision,
            colors,
            pros,
            teleport_effects,
            portals,
            scene: files.scene.clone(),
            scene_textures: files.textures.clone(),
            sector_layers,
            sector_centres,
            bouncies,
            movers,
            movie_screen,
            texture_file: files.textures.clone(),
            swapped_textures: HashMap::new(),
            screen_movie: None,
            breakables,
            touch_scripts,
            broken: HashSet::new(),
            minimap,
            particles,
            particle_textures,
            markers: (false, false),
        },
        stats,
    ))
}

/// A warp on the level, and how it's shown.
struct Portal {
    warp: desa_viewer::warps::Warp,
    /// A Hub portal's film strip: its layer and vertices as stored.
    strip: Option<(Option<usize>, Vec<desa_viewer::level::Vertex>)>,
    /// Seconds since it appeared (the skater came within 60 feet).
    appeared: Option<f32>,
    /// Offered and turned down: not again until the skater's gone off.
    declined: bool,
}

/// How near a Hub portal appears (`LevelWarp`: `Obj_SetInnerRadius 60`),
/// and how long it takes to open out (the game's `WarpAppears` turns and
/// moves the strip in its own axes, which aren't read yet; this grows it
/// from its middle instead, ending where the level stores it).
const PORTAL_APPEAR: f32 = 60.0 * 12.0;
const PORTAL_TIME: f32 = 0.6;
/// How near the skater goes through a warp (across, and up or down), and
/// how far it has to go after turning one down to be offered it again
/// (`WarpDialogHub`: `Obj_SetOuterRadius 20`).
const WARP_ENTER: f32 = 8.0 * 12.0;
const WARP_HEIGHT: f32 = 250.0;
const WARP_LEAVE: f32 = 20.0 * 12.0;

/// The way back to the Hub, as a ring of glowing streaks turning in the air
/// (the game shows a particle portal there to the Kid).
fn portal_ring(center: Vec3, time: f32, eye: Vec3) -> Vec<collision::ColorVertex> {
    const COUNT: usize = 28;
    const RADIUS: f32 = 70.0;
    let center = center + Vec3::Y * 70.0;
    let mut out = Vec::with_capacity(COUNT * 6);
    let to_eye = (eye - center).normalize_or(Vec3::Z);
    // The ring faces the camera, about the vertical.
    let flat = Vec3::new(to_eye.x, 0.0, to_eye.z).normalize_or(Vec3::Z);
    let side = Vec3::Y.cross(flat).normalize_or(Vec3::X);
    for i in 0..COUNT {
        let a = i as f32 / COUNT as f32 * std::f32::consts::TAU + time * 1.5;
        let wobble = 1.0 + 0.08 * (time * 3.0 + i as f32).sin();
        let p = center + (side * a.cos() + Vec3::Y * a.sin()) * RADIUS * wobble;
        let tangent = (-side * a.sin() + Vec3::Y * a.cos()) * 9.0;
        let across = tangent.cross(to_eye).normalize_or(Vec3::Y) * 4.0;
        let glow = 0.6 + 0.4 * (time * 4.0 + i as f32 * 0.7).sin();
        let color = [
            (120.0 + 100.0 * glow) as u8,
            (90.0 + 60.0 * glow) as u8,
            255,
            (200.0 * glow) as u8,
        ];
        let v = |p: Vec3| collision::ColorVertex {
            position: p.to_array(),
            color,
        };
        let (a0, a1, b0, b1) = (
            p - tangent - across,
            p - tangent + across,
            p + tangent - across,
            p + tangent + across,
        );
        out.extend([v(a0), v(a1), v(b1), v(a0), v(b1), v(b0)]);
    }
    out
}

/// The renderer layers holding a level's objects (`None` when empty).
struct ObjectLayers {
    goal_geometry: Option<usize>,
    props: Option<usize>,
    goal_props: Option<usize>,
    crowd: Option<usize>,
    goal_crowd: Option<usize>,
    /// King of the Hill's crown (`crown.mdl` in the level's Ped archive),
    /// hidden till played, and its vertices as modelled.
    crown: Option<(Option<usize>, Vec<desa_viewer::level::Vertex>)>,
}

impl LoadedLevel {
    /// The particle effects, hidden sectors and rails the scripts started
    /// and stopped, created and killed.
    fn apply_creates(&mut self) {
        for (name, created) in std::mem::take(&mut self.behaviour.other_creates) {
            // A bouncy object: shown at rest, or gone.
            if let Some(b) = self.bouncies.iter_mut().find(|b| b.spec.name == name) {
                if created != b.shown {
                    b.shown = created;
                    b.collided = false;
                    b.offset = Vec3::ZERO;
                    b.velocity = Vec3::ZERO;
                    b.rotation = Quat::IDENTITY;
                    b.moving = false;
                    self.renderer.update_layer(b.layer, 0, &b.base);
                    self.renderer.show_layer(b.layer, created);
                }
                continue;
            }
            if self.nodes.hidden_sectors.contains(&name) {
                self.show_sector(name, created);
                continue;
            }
            if self.nodes.rails.iter().any(|r| r.node == name) {
                if let Some(world) = &mut self.world {
                    world.rails.set_present(name, created);
                }
                continue;
            }
            if created {
                self.particles
                    .start(self.behaviour.program(), &self.nodes, name);
            } else {
                self.particles.stop(name);
            }
        }
    }

    /// Shows or hides a sector that isn't there at the start (made into a
    /// layer of its own the first time).
    fn show_sector(&mut self, name: u32, shown: bool) {
        let node = self
            .nodes
            .nodes
            .iter()
            .find(|n| n.name == name)
            .and_then(|n| n.position);
        let layer = *self.sector_layers.entry(name).or_insert_with(|| {
            let mut level =
                Level::from_bytes_filtered(&self.scene, self.scene_textures.as_deref(), |s| {
                    s == name
                })
                .ok()
                .filter(|l| !l.vertices.is_empty())?;
            // Some are stored round the origin, to be put where their node
            // is (a race's gates); others already sit in place.
            let mut centre = level.focus.0;
            if let Some(at) = node {
                if centre.length() < at.distance(centre) {
                    for v in &mut level.vertices {
                        v.position = (Vec3::from(v.position) + at).to_array();
                    }
                    centre += at;
                }
            }
            // (Where it is, for chunks if it's smashed.)
            self.sector_centres.insert(name, centre);
            self.renderer.add_layer(&level, false)
        });
        self.renderer.show_layer(layer, shown);
    }

    /// Runs objects' scripts for `dt` seconds, shows or hides the object
    /// layers, poses the pedestrians shown and animates vertex colors.
    fn update_objects(&mut self, objects: bool, goal_objects: bool, seconds: f32, dt: f32) {
        let placed = self.behaviour.update(
            &mut self.objects,
            seconds,
            dt,
            goal_objects,
            self.world.as_ref(),
        );
        // Textures its pieces' scripts swap (`Obj_ReplaceTexture`): Pizza's
        // arcade screens, a Treehouse piece's five frames.
        // (The texture's swapped wherever it's used, as the game's are
        // shared: the master piece holds the frames, the cabinets show
        // them.)
        for (_, src, dest) in std::mem::take(&mut self.behaviour.texture_swaps) {
            let from = desa_viewer::level::texture_checksums(&src);
            let texture = if src.eq_ignore_ascii_case(&dest) {
                None
            } else {
                let file = &self.texture_file;
                let found = self
                    .swapped_textures
                    .entry(dest.to_ascii_lowercase())
                    .or_insert_with(|| {
                        let to = desa_viewer::level::texture_checksums(&dest);
                        file.as_deref()
                            .and_then(|f| desa_viewer::level::decode_texture_from(f, &to))
                    })
                    .clone();
                let Some(found) = found else { continue };
                Some(found)
            };
            self.renderer.swap_texture(&from, texture.as_ref());
        }
        // The level's pieces the scripts moved, made or took away.
        for m in &mut self.movers {
            let now = (
                self.behaviour.alive(m.object),
                self.behaviour.position(m.object),
                self.behaviour.rotation(m.object),
            );
            if now == m.last {
                continue;
            }
            if now.0 != m.last.0 {
                self.renderer.show_layer(m.layer, now.0);
                if let Some(world) = &mut self.world {
                    if now.0 {
                        world.enable(m.name);
                    } else {
                        world.disable(m.name);
                    }
                }
            }
            if (now.1, now.2) != (m.last.1, m.last.2) {
                let turn = now.2 * m.placed.inverse();
                let moved: Vec<desa_viewer::level::Vertex> = m
                    .base
                    .iter()
                    .map(|v| {
                        let position = now.1 + turn * (Vec3::from(v.position) - m.pivot);
                        desa_viewer::level::Vertex {
                            position: position.to_array(),
                            normal: (turn * Vec3::from(v.normal)).to_array(),
                            color: m.spot.map_or(v.color, |spot| {
                                spot_lit(v.color, position - now.1, spot - now.1)
                            }),
                            ..*v
                        }
                    })
                    .collect();
                self.renderer.update_layer(m.layer, 0, &moved);
                // Its collision goes with it.
                if let Some(world) = &mut self.world {
                    // (Collision in its own space goes straight where it is.)
                    let transform = if m.local {
                        Mat4::from_rotation_translation(now.2, now.1)
                    } else {
                        Mat4::from_translation(now.1)
                            * Mat4::from_quat(turn)
                            * Mat4::from_translation(-m.pivot)
                    };
                    world.place(m.name, Some(transform));
                }
            }
            m.last = now;
        }
        for (goal, copy, placement) in placed {
            let (props, layer) = if goal {
                (&self.objects.goal_props, self.layers.goal_props)
            } else {
                (&self.objects.props, self.layers.props)
            };
            let (first, vertices) = props.place(copy, placement);
            self.renderer.update_layer(layer, first, &vertices);
        }
        let l = &self.layers;
        let r = &mut self.renderer;
        let [world, sky, goal] = &self.colors;
        let clocks: HashMap<u32, f32> = self
            .behaviour
            .color_clocks
            .iter()
            .map(|(s, (t, _))| (*s, *t))
            .collect();
        if !world.is_empty() {
            r.update_world(
                false,
                world.first_vertex,
                &world.at_clocks(seconds, &clocks),
            );
        }
        if !sky.is_empty() {
            r.update_world(true, sky.first_vertex, &sky.at(seconds));
        }
        if goal_objects && !goal.is_empty() {
            r.update_layer(
                l.goal_geometry,
                goal.first_vertex,
                &goal.at_clocks(seconds, &clocks),
            );
        }
        for (id, shown) in [
            (l.props, objects),
            (l.crowd, objects),
            (l.goal_geometry, goal_objects),
            // Goals' objects that scripts have made show with the rest
            // (the others are shrunk away unless previewing goals).
            (l.goal_props, objects || goal_objects),
            (l.goal_crowd, objects || goal_objects),
        ] {
            r.show_layer(id, shown);
        }
        if objects && !self.objects.crowd.is_empty() {
            r.update_layer(l.crowd, 0, &self.objects.crowd.pose(seconds));
        }
        if (objects || goal_objects) && !self.objects.goal_crowd.is_empty() {
            r.update_layer(l.goal_crowd, 0, &self.objects.goal_crowd.pose(seconds));
        }
    }
}

/// Where a character stands at a spawn point, facing its way. Character
/// models face +Z.
/// The skater's shadow, as the game draws one under it: a dark disc laid on
/// the ground straight below, smaller and fainter the higher it is.
fn skater_shadow(skater: &skate::Skater, world: &skate::World) -> Vec<collision::ColorVertex> {
    blob_shadow(skater.position, 16.0, world)
}

/// A round shadow on the ground under `at`, `radius` across at the ground,
/// fading and shrinking with height.
fn blob_shadow(at: Vec3, radius: f32, world: &skate::World) -> Vec<collision::ColorVertex> {
    const FADE: f32 = 400.0;
    const SIDES: usize = 20;
    let from = at + Vec3::Y * 10.0;
    let Some(hit) = world.ray(from, from - Vec3::Y * FADE) else {
        return Vec::new();
    };
    // Faces flagged to take no skater shadow.
    if hit.flags & ngc_collision::face_flags::NO_SKATER_SHADOW != 0 {
        return Vec::new();
    }
    let height = (at.y - hit.point.y).max(0.0);
    let fade = (1.0 - height / FADE).clamp(0.0, 1.0);
    let alpha = (150.0 * fade) as u8;
    if alpha == 0 {
        return Vec::new();
    }
    let normal = hit.normal.normalize_or(Vec3::Y);
    let side = normal.any_orthonormal_vector();
    let along = normal.cross(side);
    let radius = radius * (0.6 + 0.4 * fade);
    let centre = hit.point + normal * 0.5;
    let point = |i: usize| {
        let a = i as f32 / SIDES as f32 * std::f32::consts::TAU;
        centre + (side * a.cos() + along * a.sin()) * radius
    };
    let vertex = |p: Vec3, a: u8| collision::ColorVertex {
        position: p.to_array(),
        color: [0, 0, 0, a],
    };
    (0..SIDES)
        .flat_map(|i| {
            [
                vertex(centre, alpha),
                vertex(point(i), alpha / 3),
                vertex(point(i + 1), alpha / 3),
            ]
        })
        .collect()
}

/// A goal's pro: their object, the goal they offer (its words and how to
/// start it), and whether it's been turned down till the skater's gone.
struct GoalPro {
    object: usize,
    title: String,
    /// What they say offering it.
    line: Option<u32>,
    start: ui::Action,
    declined: bool,
    /// The game's offer, as its speech box says it (`Woody: Press \m5 to
    /// talk.`).
    talk: String,
}

/// A goal's setting, a bare global's name read as its value.
fn goal_setting(
    program: &qb::vm::Program,
    level: &str,
    kind: &str,
    key: &str,
) -> Option<qb::Value> {
    let goal = desa_viewer::goals::generic_goal(program, level, kind)?;
    let v = goal
        .params
        .iter()
        .find(|(k, _)| *k == qb::checksum(key))
        .map(|(_, v)| v)?;
    Some(match v {
        qb::Value::Name(n) => program.value(*n).unwrap_or(v).clone(),
        v => v.clone(),
    })
}

fn value_string(v: &qb::Value) -> Option<String> {
    match v {
        qb::Value::String(s) | qb::Value::LocalString(s) => Some(s.clone()),
        _ => None,
    }
}

/// A goal's pro by name: their `full_name`, or `pro` and its
/// `goal_pro_last_names` entry if it has one.
fn pro_name(program: &qb::vm::Program, level: &str, kind: &str) -> (Option<String>, bool) {
    if let Some(name) =
        goal_setting(program, level, kind, "full_name").and_then(|v| value_string(&v))
    {
        return (Some(name), true);
    }
    let Some(pro) = goal_setting(program, level, kind, "pro").and_then(|v| value_string(&v)) else {
        return (None, false);
    };
    let last = program
        .value(qb::checksum("goal_pro_last_names"))
        .and_then(|names| match names {
            qb::Value::Struct(items) => items
                .iter()
                .find(|(k, _)| *k == Some(qb::checksum(&pro)))
                .and_then(|(_, v)| value_string(v)),
            _ => None,
        });
    match last {
        Some(last) => (Some(format!("{pro} {last}")), true),
        None => (Some(pro), false),
    }
}

/// What a goal's pro says offering it (`goal_trigger_wait`'s
/// `accept_text`, the GameCube's): their name, then "Press (B) to talk.".
fn talk_text(program: &qb::vm::Program, level: &str, kind: &str) -> String {
    match pro_name(program, level, kind).0 {
        Some(name) => format!("{name}: Press \\m5 to talk."),
        None => "Press \\m5 to talk.".to_string(),
    }
}

/// What a goal's pro tells the skater as it starts
/// (`goal_description_and_speech`): their name (a full one only), its
/// `goal_description` lines, then "Press (A) to continue.".
fn description_text(program: &qb::vm::Program, level: &str, kind: &str) -> Option<String> {
    let lines = match goal_setting(program, level, kind, "goal_description")? {
        qb::Value::Array(items) => items.iter().filter_map(value_string).collect::<String>(),
        v => value_string(&v)?,
    };
    let name = match pro_name(program, level, kind) {
        (Some(name), true) => format!("\\c7{name} :\\c0\n"),
        _ => String::new(),
    };
    Some(format!("{name}{lines}\nPress \\m0 to continue."))
}

/// The pros of the goals the viewer plays (`trigger_obj_id`).
fn goal_pros(program: &qb::vm::Program, level: &str, behaviour: &Behaviour) -> Vec<GoalPro> {
    let goals = desa_viewer::goals::level_goals(program, level);
    let mut pros: Vec<GoalPro> = [
        ("HighScore", "HighScore", ui::Action::StartRun(Some(false))),
        ("ProScore", "ProScore", ui::Action::StartRun(Some(true))),
        ("SKATE", "Skate", ui::Action::StartLetters),
        ("Race", "Race", ui::Action::StartRace),
    ]
    .into_iter()
    .filter_map(|(script, kind, start)| {
        let object = behaviour.object(desa_viewer::goals::goal_pro(program, level, script)?)?;
        let title = goals.iter().find(|g| g.kind == kind)?.text.clone();
        Some(GoalPro {
            object,
            title,
            line: desa_viewer::goals::goal_intro_line(program, level, script),
            start,
            declined: false,
            talk: talk_text(program, level, kind),
        })
    })
    .collect();
    // The rest, played from their own scripts (one goal a pro).
    for (i, goal) in goals.iter().enumerate() {
        if ["HighScore", "ProScore", "Skate", "Race"].contains(&goal.kind.as_str()) {
            continue;
        }
        let Some(generic) = desa_viewer::goals::generic_goal(program, level, &goal.kind) else {
            continue;
        };
        let Some(object) = generic.pro.and_then(|p| behaviour.object(p)) else {
            continue;
        };
        if pros.iter().any(|p| p.object == object) {
            continue;
        }
        pros.push(GoalPro {
            object,
            title: goal.text.clone(),
            line: desa_viewer::goals::goal_intro_line(program, level, &goal.kind),
            start: ui::Action::StartGoal(i),
            declined: false,
            talk: talk_text(program, level, &goal.kind),
        });
    }
    pros
}

/// How near a pro the skater's offered their goal, and how far it has to
/// go after saying not now.
const PRO_NEAR: f32 = 10.0 * 12.0;
const PRO_LEAVE: f32 = 25.0 * 12.0;

/// A bouncy object: its settings, layer and vertices as stored, where it
/// rests, how far it reaches and half its height, and how it's moving
/// (offset from rest, velocity, turn and spin), and how long since it was
/// last knocked.
struct BouncyState {
    /// Whether it's there (created), and whether its `CollideScript` has
    /// run since it was.
    shown: bool,
    collided: bool,
    spec: desa_viewer::nodes::Bouncy,
    layer: Option<usize>,
    base: Vec<desa_viewer::level::Vertex>,
    centre: Vec3,
    reach: f32,
    half_height: f32,
    offset: Vec3,
    velocity: Vec3,
    rotation: Quat,
    spin: Vec3,
    moving: bool,
    since: f32,
}

/// A piece of the level scripts move (`Obj_MoveToPos`, `Obj_Rotate`,
/// `create`, `kill`): its object, name, layer and vertices as stored,
/// where and how it was placed, and how it was last drawn (there?, where,
/// how turned).
/// A vertex's colour lit by a light on the stage: how squarely it faces
/// the light round the piece's middle (from the middle, out to it and to
/// the light) — the panel facing it brighter than it's stored, those
/// turned away into the shade.
fn spot_lit(color: [u8; 4], out: Vec3, light: Vec3) -> [u8; 4] {
    let flat = |v: Vec3| Vec3::new(v.x, 0.0, v.z).normalize_or_zero();
    let facing = flat(out).dot(flat(light)).max(0.0);
    let k = STAGE_SHADE + (STAGE_LIT - STAGE_SHADE) * facing.powi(8);
    let channel = |c: u8| (f32::from(c) * k).round().clamp(0.0, 255.0) as u8;
    [
        channel(color[0]),
        channel(color[1]),
        channel(color[2]),
        color[3],
    ]
}

/// How bright the stage's light makes the panel facing it, and those
/// turned away (of their colours as stored).
const STAGE_LIT: f32 = 2.6;
const STAGE_SHADE: f32 = 0.55;

struct MoverState {
    object: usize,
    /// The light it's lit by as it turns, if it's lit (see `spot_lit`).
    spot: Option<Vec3>,
    name: u32,
    /// Whether its geometry (and collision) is in its own space, put
    /// where it is (else in the level's, moved from where it was).
    local: bool,
    layer: Option<usize>,
    base: Vec<desa_viewer::level::Vertex>,
    pivot: Vec3,
    placed: Quat,
    last: (bool, Vec3, Quat),
}

/// The game's panel while skating (see `App::update_game_hud`): whether
/// it's up, the tricks in the combo last frame, how long the trick text's
/// been showing, and the song last announced.
#[derive(Default)]
struct GameHud {
    up: bool,
    tricks: usize,
    shown: Option<f32>,
    song: Option<Instant>,
    /// The count last shown on a goal's HUD counter (`goal_counter_anchor`).
    counted: Option<usize>,
    /// The pro whose offer the game's speech box shows.
    offer: Option<usize>,
}

/// A movie playing (through ffmpeg: see `desa_viewer::movie`), since
/// when, and the texture its frames go to.
struct MoviePlaying {
    movie: desa_viewer::movie::Movie,
    started: Instant,
    texture: Option<egui::TextureHandle>,
    /// The newest frame, not yet in the texture.
    pending: Option<egui::ColorImage>,
}

impl MoviePlaying {
    /// Over the whole window, letterboxed on black; true if clicked (to
    /// skip it).
    fn draw(&mut self, ctx: &egui::Context) -> bool {
        if let Some(image) = self.pending.take() {
            match &mut self.texture {
                Some(t) => t.set(image, egui::TextureOptions::LINEAR),
                None => {
                    self.texture =
                        Some(ctx.load_texture("movie", image, egui::TextureOptions::LINEAR))
                }
            }
        }
        let screen = ctx.content_rect();
        let mut clicked = false;
        egui::Area::new(egui::Id::new("movie"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                let response = ui.allocate_rect(screen, egui::Sense::click());
                ui.painter().rect_filled(screen, 0.0, egui::Color32::BLACK);
                if let Some(texture) = &self.texture {
                    let (w, h) = (self.movie.info.width as f32, self.movie.info.height as f32);
                    let scale = (screen.width() / w).min(screen.height() / h);
                    let rect =
                        egui::Rect::from_center_size(screen.center(), egui::vec2(w, h) * scale);
                    ui.painter().image(
                        texture.id(),
                        rect,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                }
                clicked = response.clicked();
            });
        // Keep drawing: the movie moves on by itself.
        ctx.request_repaint();
        clicked
    }
}

/// The game's screen elements (`desa_viewer::screen`) with what they're
/// drawn with: the panel sprites and fonts, as egui textures once used.
struct ScreenUi {
    screen: desa_viewer::screen::Screen,
    images: HashMap<u32, ngc_texture::Image>,
    textures: HashMap<u32, egui::TextureHandle>,
    font_textures: HashMap<u32, egui::TextureHandle>,
}

impl ScreenUi {
    /// The shared panel sprites and fonts, and the hub theme's.
    fn load(data: &mut GameData) -> Option<ScreenUi> {
        let ui = data.ui_files(&["panelsprites.prg", "hubpanel.prg"]).ok()?;
        let mut images = HashMap::new();
        let mut sizes = HashMap::new();
        for (name, bytes) in &ui.images {
            let Ok(file) = ngc_texture::img::ImgFile::parse(bytes) else {
                continue;
            };
            let Ok(image) = file.decode() else { continue };
            sizes.insert(qb::checksum(name), (file.width, file.height));
            images.insert(qb::checksum(name), image.cropped(file.width, file.height));
        }
        let fonts: HashMap<u32, desa_viewer::font::Font> = ui
            .fonts
            .iter()
            .filter_map(|(n, b)| Some((qb::checksum(n), desa_viewer::font::Font::parse(b).ok()?)))
            .collect();
        if fonts.is_empty() {
            return None;
        }
        let mut screen = desa_viewer::screen::Screen::new(fonts, sizes);
        // What the viewer does itself: resuming, and the main menu's
        // choices whose screens need the career's profiles.
        screen.listen(&[
            "unpausegame",
            "launch_options_menu_load_game_sequence",
            "launch_options_menu_save_game_sequence",
            "RetryCurrentGoal",
            "end_current_goal_run",
            "view_goals_menu_start_goal",
            "really_change_level",
            "skip_track",
            "viewer_video",
            "viewer_look",
            "viewer_exit_game",
            "StartNetworkGame",
        ]);
        Some(ScreenUi {
            screen,
            images,
            textures: HashMap::new(),
            font_textures: HashMap::new(),
        })
    }

    /// Over the 3D view (beside the panel), the game's 640x480 screen
    /// fitted in.
    fn paint(&mut self, ctx: &egui::Context) {
        use desa_viewer::screen::{Draw, HEIGHT, WIDTH};
        let draws = self.screen.draw();
        if draws.is_empty() {
            return;
        }
        let area = picture_area(ctx);
        let k = (area.width() / WIDTH).min(area.height() / HEIGHT);
        let origin = area.center() - egui::vec2(WIDTH, HEIGHT) * k / 2.0;
        let to_screen = |r: [f32; 4]| {
            egui::Rect::from_min_max(
                origin + egui::vec2(r[0], r[1]) * k,
                origin + egui::vec2(r[2], r[3]) * k,
            )
        };
        let tint = |c: [f32; 4]| {
            let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0) as u8;
            egui::Color32::from_rgba_unmultiplied(b(c[0]), b(c[1]), b(c[2]), b(c[3]))
        };
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("game_screen"),
        ));
        for d in draws {
            match d {
                Draw::Sprite {
                    texture,
                    rect,
                    rgba,
                    angle,
                } => {
                    let Some(image) = self.images.get(&texture) else {
                        continue;
                    };
                    let handle = self.textures.entry(texture).or_insert_with(|| {
                        ctx.load_texture(
                            format!("sprite_{texture:08x}"),
                            egui::ColorImage::from_rgba_unmultiplied(
                                [image.width as usize, image.height as usize],
                                &image.rgba,
                            ),
                            egui::TextureOptions::LINEAR,
                        )
                    });
                    let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                    if angle == 0.0 {
                        painter.image(handle.id(), to_screen(rect), uv, tint(rgba));
                    } else {
                        // Turned about its middle.
                        let r = to_screen(rect);
                        let mut mesh = egui::Mesh::with_texture(handle.id());
                        mesh.add_rect_with_uv(r, uv, tint(rgba));
                        let (sin, cos) = angle.to_radians().sin_cos();
                        let c = r.center();
                        for v in &mut mesh.vertices {
                            let d = v.pos - c;
                            v.pos = c + egui::vec2(d.x * cos - d.y * sin, d.x * sin + d.y * cos);
                        }
                        painter.add(egui::Shape::mesh(mesh));
                    }
                }
                Draw::Glyph {
                    font,
                    source,
                    rect,
                    rgba,
                } => {
                    let Some(f) = self.screen.font(font) else {
                        continue;
                    };
                    let (aw, ah) = (f.atlas_width as f32, f.atlas_height as f32);
                    let handle = self.font_textures.entry(font).or_insert_with(|| {
                        ctx.load_texture(
                            format!("font_{font:08x}"),
                            egui::ColorImage::from_rgba_unmultiplied(
                                [f.atlas_width as usize, f.atlas_height as usize],
                                &f.atlas,
                            ),
                            egui::TextureOptions::LINEAR,
                        )
                    });
                    let [x, y, w, h] = source.map(f32::from);
                    painter.image(
                        handle.id(),
                        to_screen(rect),
                        egui::Rect::from_min_max(
                            egui::pos2(x / aw, y / ah),
                            egui::pos2((x + w) / aw, (y + h) / ah),
                        ),
                        tint(rgba),
                    );
                }
            }
        }
    }
}

/// Units a foot (the bouncy objects' settings are in feet).
const FEET: f32 = 12.0;

/// A two-minute run (the game's single session).
struct Run {
    /// Seconds left on the clock.
    left: f32,
    /// Out of time with the last combo landed: seconds since, braking.
    ending: Option<f32>,
    /// Stopped, with the result up.
    over: bool,
    /// A score goal: the score to reach and what it says when it is.
    goal: Option<(u32, String)>,
    won: bool,
}

/// This session's skating, since the viewer started.
#[derive(Default)]
struct Session {
    /// Seconds skated (not paused), and units travelled.
    time: f32,
    distance: f32,
    last_position: Option<Vec3>,
    /// Combos landed, their tricks and points, the best, bails and gaps.
    combos: u32,
    tricks: u32,
    points: u32,
    best: u32,
    bails: u32,
    gaps: u32,
}

impl Session {
    fn lines(&self) -> Vec<String> {
        let minutes = (self.time / 60.0) as u32;
        vec![
            format!("Skated {minutes}:{:02}", self.time as u32 % 60),
            // Units are inches.
            format!("Distance {:.2} miles", self.distance / 63_360.0),
            format!("Combos landed {} ({} tricks)", self.combos, self.tricks),
            format!("Points {}", self.points),
            format!("Best combo {}", self.best),
            format!("Gaps {}", self.gaps),
            format!("Bails {}", self.bails),
        ]
    }
}

/// How long the camera stays to see a splash (seconds).
const SPLASH_HOLD: f32 = 0.8;

/// Looking round: how far the camera swings round (radians, either way)
/// and up or down at full stick, and how fast it follows the stick.
const LOOK_YAW: f32 = 2.6;
const LOOK_PITCH: f32 = 0.6;
const LOOK_RATE: f32 = 8.0;

/// A camera's name for the panel: "standard ltg" as "Standard LTG".
fn title_case(name: &str) -> String {
    name.split(' ')
        .map(|word| {
            if word == "ltg" {
                return word.to_ascii_uppercase();
            }
            let mut chars = word.chars();
            chars.next().map_or_else(String::new, |c| {
                c.to_ascii_uppercase().to_string() + chars.as_str()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Where F12 saves a picture: the Pictures folder's `DESA Map Viewer`,
/// named by the time.
fn photo_path() -> PathBuf {
    let pictures = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(|home| PathBuf::from(home).join("Pictures"))
        .filter(|p| p.is_dir())
        .unwrap_or_else(|| PathBuf::from("."));
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    pictures
        .join("DESA Map Viewer")
        .join(format!("desa_{stamp}.png"))
}

/// A soft round particle texture: white, fading out from the middle.
fn soft_disc() -> desa_viewer::level::TextureData {
    const SIZE: u32 = 32;
    let mut pixels = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let d = Vec3::new(x as f32 + 0.5 - 16.0, y as f32 + 0.5 - 16.0, 0.0).length() / 16.0;
            let a = ((1.0 - d).clamp(0.0, 1.0) * 255.0) as u8;
            pixels.extend([255, 255, 255, a]);
        }
    }
    desa_viewer::level::TextureData {
        checksum: 0,
        width: SIZE,
        height: SIZE,
        levels: vec![pixels],
    }
}

/// The particles' quads for the renderer, their textures by place.
fn particle_batches(
    particles: &desa_viewer::particles::Particles,
    textures: &HashMap<u32, usize>,
    eye: Vec3,
) -> Vec<renderer::ParticleBatch> {
    particles
        .quads(eye)
        .into_iter()
        .map(|(texture, additive, vertices)| renderer::ParticleBatch {
            texture: textures.get(&texture).copied().unwrap_or(0),
            additive,
            vertices,
        })
        .collect()
}

/// How near the camera the animals' and cars' shadows are drawn.
const SHADOW_RANGE: f32 = 3000.0;

/// The replay's cameras: as it was played, then the game's replay
/// cameras (`Skater_Camera_Replay_*`, `behind` and `above` in feet).
const REPLAY_CAMERAS: [(&str, Option<&str>); 5] = [
    ("As played", None),
    ("Behind", Some("Skater_Camera_Replay_Behind")),
    ("Front", Some("Skater_Camera_Replay_Front")),
    ("Left", Some("Skater_Camera_Replay_Left")),
    ("Right", Some("Skater_Camera_Replay_Right")),
];

/// How far away objects' sounds fade out (units).
const OBJECT_SOUND_RANGE: f32 = 2000.0;

/// One frame of a run as it was shown, for the replay.
struct ReplayFrame {
    /// Seconds into the run.
    time: f32,
    placement: Mat4,
    pose: Pose,
    flipped: bool,
    position: Vec3,
    camera: FlyCamera,
    /// What the screen showed: score, combo, message, balance and special
    /// meters, and the clock.
    score: u32,
    combo: Option<String>,
    message: Option<String>,
    balance: Option<f32>,
    special: (f32, bool),
    clock: f32,
}

/// The S-K-A-T-E letters goal under way.
struct LetterRun {
    /// The letters' objects, S to E, and which are got.
    objects: [usize; 5],
    got: [bool; 5],
    /// Seconds left, out of the goal's time.
    left: f32,
    time: f32,
    over: bool,
}

/// The character's collectibles on the level, while skating.
struct Collecting {
    /// Their objects and their bits in `got` (which are collected, kept in
    /// the settings under `key`).
    objects: Vec<(usize, u32)>,
    /// The world's special item and what it's called.
    special: Option<(usize, String)>,
    got: u32,
    kind: String,
    key: String,
}

/// How near the skater picks a collectible up
/// (`set_goal_collect_exception_25`: 7 feet), and how fast they spin
/// (`Obj_RotY speed = 250`).
const COLLECT_RADIUS: f32 = 7.0 * 12.0;
const COLLECT_SPIN: f32 = 250.0;
/// The special item's bit in what's collected.
const SPECIAL_BIT: u32 = 1 << 25;

/// The records kept, and how the panel names them.
const RECORDS: [(&str, &str); 5] = [
    ("combo", "Best combo"),
    ("grind", "Longest grind"),
    ("manual", "Longest manual"),
    ("lip", "Longest lip trick"),
    ("tricks", "Most tricks in a combo"),
];

/// How long each record announcement shows (`time = 2000`).
const RECORD_TIME: f32 = 2.0;

/// A record's value as the game words it: points, "12.34 seconds", "7
/// Tricks".
fn record_value(kind: &str, value: u32) -> String {
    match kind {
        "combo" => format!("{value}"),
        "tricks" => format!("{value} Tricks"),
        _ => format!("{}.{:02} seconds", value / 100, value % 100),
    }
}

/// A race under way: its waypoints (where, script, seconds added), the
/// next to reach, the clock and the time taken, the object its scripts
/// run on, and its end script.
struct RaceRun {
    points: Vec<(Vec3, Option<u32>, f32)>,
    next: usize,
    left: f32,
    time: f32,
    over: bool,
    runner: usize,
    goal_runner: usize,
    end_script: Option<u32>,
    started: bool,
}

/// A goal played from its own scripts (`GenericGoal`): its type (as in
/// `<level>_AddGoal_<kind>`) and place in the goal list, the clock (none
/// if untimed), and whether it's over.
struct GoalRun {
    kind: String,
    index: usize,
    goal: desa_viewer::goals::GenericGoal,
    left: Option<f32>,
    over: bool,
    /// A Skatetris goal's list of tricks to do (see [`TetrisRun`]).
    tetris: Option<TetrisRun>,
    /// The objects that already reacted to the skater coming near before
    /// it started (what's new since is the goal's, for the map).
    before: HashSet<usize>,
}

/// A Skatetris goal going: the tricks on the list (each its trick in the
/// character's book and its element on the game's screen), the next slot
/// from the goal's list, how long till the next joins, the time between
/// them now, and the skater's combos already counted (a trick counts once
/// its combo's landed, not bailed).
struct TetrisRun {
    rules: desa_viewer::goals::Tetris,
    list: Vec<(usize, u32)>,
    next: usize,
    until: f32,
    interval: f32,
    since_faster: f32,
    combos: u32,
    made: u32,
    /// Lost (the list full), or won (none left once no more come).
    lost: bool,
    won: bool,
}

impl TetrisRun {
    fn new(rules: desa_viewer::goals::Tetris) -> Self {
        TetrisRun {
            interval: rules.trick_time,
            until: 1.0,
            rules,
            list: Vec::new(),
            next: 0,
            since_faster: 0.0,
            combos: u32::MAX,
            made: 0,
            lost: false,
            won: false,
        }
    }

    /// One frame of it: tricks done come off the list, the next joins in
    /// its time (none in the last seconds), the list full loses and empty
    /// once they've stopped coming wins. Screen commands for the game's
    /// Skatetris list go to `screen`.
    fn update(
        &mut self,
        dt: f32,
        left: Option<f32>,
        skater: &Skater,
        program: &qb::vm::Program,
        screen: &mut Vec<(u32, qb::Value)>,
    ) {
        use qb::{Value, checksum};
        let s = |k: &str| Some(checksum(k));
        // Tricks done: a combo landed, each of its tricks taking the
        // first of its kind off the list.
        if self.combos == u32::MAX {
            self.combos = skater.combos_ended;
        }
        let landed = match &skater.last_combo {
            Some(last) if skater.combos_ended != self.combos && !last.bailed => {
                last.combo.tricks.clone()
            }
            _ => Vec::new(),
        };
        self.combos = skater.combos_ended;
        for done in &landed {
            let found = self.list.iter().position(|&(trick, _)| {
                skater
                    .tricks
                    .tricks
                    .get(trick)
                    .is_some_and(|t| t.name == done.name)
            });
            if let Some(i) = found {
                let (_, id) = self.list.remove(i);
                screen.push((
                    checksum("RunScriptOnScreenElement"),
                    Value::Struct(vec![
                        (s("id"), Value::Name(id)),
                        (None, Value::Name(checksum("goal_tetris_remove_trick"))),
                        (s("params"), Value::Struct(vec![(s("id"), Value::Name(id))])),
                    ]),
                ));
                screen.push((
                    checksum("RunScriptOnScreenElement"),
                    Value::Struct(vec![
                        (s("id"), Value::Name(id)),
                        (
                            None,
                            Value::Name(checksum("goal_tetris_play_trick_removed_sound")),
                        ),
                    ]),
                ));
            }
        }
        // Coming faster.
        self.since_faster += dt;
        if self.since_faster >= self.rules.acceleration_interval {
            self.since_faster = 0.0;
            self.interval *= 1.0 - self.rules.acceleration_percent;
        }
        let adding = left.is_none_or(|l| l > self.rules.stop_adding);
        self.until -= dt;
        if adding && self.until <= 0.0 && !self.rules.combos.is_empty() {
            self.until = self.interval;
            // The next slot the character has a trick in.
            for _ in 0..self.rules.combos.len() {
                let slot = self.rules.combos[self.next % self.rules.combos.len()];
                self.next += 1;
                let Some(trick) = skater.tricks.slot_trick(slot) else {
                    continue;
                };
                self.made += 1;
                let id = checksum(&format!("tetris_trick_{}", self.made));
                let name = skater.tricks.tricks[trick].name.clone();
                // Its buttons, as the game spells them.
                let buttons = match program.value(checksum("goal_tetris_trick_text")) {
                    Some(table) => table.get(slot).cloned(),
                    None => None,
                }
                .unwrap_or(Value::String(String::new()));
                let element = |items: Vec<(Option<u32>, Value)>| {
                    (checksum("CreateScreenElement"), Value::Struct(items))
                };
                screen.push(element(vec![
                    (s("Type"), Value::Name(checksum("ContainerElement"))),
                    (s("parent"), Value::Name(checksum("tetris_tricks_menu"))),
                    (s("id"), Value::Name(id)),
                    (s("dims"), Value::Pair([100.0, 20.0])),
                ]));
                for text in [Value::String(name), buttons] {
                    screen.push(element(vec![
                        (s("Type"), Value::Name(checksum("TextElement"))),
                        (s("parent"), Value::Name(id)),
                        (s("font"), Value::Name(checksum("small"))),
                        (s("text"), text),
                        (
                            s("rgba"),
                            Value::Array([128, 128, 128, 75].map(Value::Integer).to_vec()),
                        ),
                    ]));
                }
                // Red once the list's nearly full.
                let script = if self.list.len() + 1 >= self.rules.max_tricks.saturating_sub(2) {
                    "goal_tetris_add_red_trick"
                } else {
                    "goal_tetris_add_trick"
                };
                screen.push((
                    checksum("RunScriptOnScreenElement"),
                    Value::Struct(vec![
                        (s("id"), Value::Name(id)),
                        (None, Value::Name(checksum(script))),
                        (s("params"), Value::Struct(vec![(s("id"), Value::Name(id))])),
                    ]),
                ));
                self.list.push((trick, id));
                break;
            }
        }
        if self.list.len() >= self.rules.max_tricks {
            self.lost = true;
        } else if !adding && self.list.is_empty() {
            self.won = true;
        }
    }
}

/// How near the skater reaches a race waypoint (`Obj_SetInnerRadius 8`).
const RACE_RADIUS: f32 = 8.0 * 12.0;

/// How near the skater picks a letter up: `Obj_SetInnerRadius 8` (feet).
const LETTER_RADIUS: f32 = 8.0 * 12.0;
/// How fast the letters spin (`Obj_RotY speed = 200`, degrees a second).
const LETTER_SPIN: f32 = 200.0;

/// How long a run lasts (`StartGoal_TrickAttack time = 120`).
const RUN_TIME: f32 = 120.0;

/// A character's bones, each relative to its parent.
type Pose = Vec<(Quat, Vec3)>;

/// The animation a skater shows: which (and the one shown last, to blend
/// from on a change), how far in, its length, the pose it's blending
/// from, and the pose shown (see `skate_pose`).
#[derive(Default)]
struct PoseState {
    animation: usize,
    shown: usize,
    time: f32,
    duration: f32,
    blend_from: Option<(Pose, f32, f32)>,
    pose: Option<Pose>,
}

/// Player two, in two players' free skate (`freeskate2p`): their
/// character, skater and chase camera, skated as player one's are.
struct Player2 {
    index: usize,
    character: Character,
    skater: Skater,
    physics: Physics,
    chase: ChaseCamera,
    camera: FlyCamera,
    pose: PoseState,
}

/// Player two's keys: the arrows, and the numpad's 0 (ollie), 1 (flip),
/// 2 (grab), 3 (grind) and 5 (revert), or Right Shift , . / ; for those.
fn player2_keys(held: impl Fn(KeyCode) -> bool) -> Input {
    Input {
        push: held(KeyCode::ArrowUp),
        brake: held(KeyCode::ArrowDown),
        turn: f32::from(u8::from(held(KeyCode::ArrowRight)))
            - f32::from(u8::from(held(KeyCode::ArrowLeft))),
        crouch: held(KeyCode::Numpad0) || held(KeyCode::ShiftRight),
        flip: held(KeyCode::Numpad1) || held(KeyCode::Comma),
        grab: held(KeyCode::Numpad2) || held(KeyCode::Period),
        grind: held(KeyCode::Numpad3) || held(KeyCode::Slash),
        revert: held(KeyCode::Numpad5) || held(KeyCode::Semicolon),
    }
}

/// The keys and a pad together: either's buttons, the keys' turn first.
fn either_input(keys: Input, pad: Input) -> Input {
    Input {
        push: keys.push || pad.push,
        brake: keys.brake || pad.brake,
        turn: if keys.turn != 0.0 {
            keys.turn
        } else {
            pad.turn
        },
        crouch: keys.crouch || pad.crouch,
        grind: keys.grind || pad.grind,
        flip: keys.flip || pad.flip,
        grab: keys.grab || pad.grab,
        revert: keys.revert || pad.revert,
    }
}

/// The skater's animation: the one it picked (the game's scripts'
/// choice), its first part once and then the next looping, or looping,
/// or held on its last frame; a lip trick's, a balance's along its meter,
/// an air trick's; blended from the last as the game's `PlayAnim` does.
fn skate_pose(character: &Character, skater: &Skater, st: &mut PoseState, dt: f32) {
    let pick = skater.anim;
    let mut time = skater.anim_time;
    let mut chosen = None;
    let mut hold = false;
    // A character without the first part (Buzz and Simba have no
    // `Crouch`) goes straight to the next.
    if let (None, Some(then)) = (
        character.animation(pick.first),
        pick.then.and_then(|then| character.animation(then)),
    ) {
        chosen = Some((then, character.animations[then].1.duration));
    } else if let Some(first) = character.animation(pick.first) {
        let duration = character.animations[first].1.duration;
        match pick.then.and_then(|then| character.animation(then)) {
            Some(then) if time > duration => {
                time -= duration;
                chosen = Some((then, character.animations[then].1.duration));
            }
            _ => {
                hold = !pick.looping;
                chosen = Some((first, duration));
            }
        }
    }
    // A lip trick shows its own animations: in, held along the meter,
    // and out.
    let by_checksum = |anim: u32| {
        character
            .animations
            .iter()
            .position(|(name, _)| qb::checksum(name) == anim)
    };
    let length = |anim: u32| by_checksum(anim).map_or(1.0, |i| character.animations[i].1.duration);
    let balance_pose = skater.balance_pose(length);
    if let Some((anim, at)) = balance_pose {
        if let Some(index) = by_checksum(anim) {
            let duration = character.animations[index].1.duration;
            chosen = Some((index, duration));
            time = at.clamp(0.0, duration * 0.999);
        }
    }
    // An air trick shows its own animation.
    if let Some((anim, at, looping)) = skater.trick_pose() {
        if let Some(index) = by_checksum(anim) {
            let duration = character.animations[index].1.duration;
            chosen = Some((index, duration));
            time = if looping {
                at
            } else {
                at.clamp(0.0, duration * 0.999)
            };
        }
    }
    if let Some((index, mut duration)) = chosen {
        // A balance pose (`ManualRange1`, `GrindRange1`) follows the
        // balance meter from one end of the animation to the other,
        // as the game plays its range animations.
        let range = character.animations[index].0.contains("Range")
            && skater.lip.is_none()
            && balance_pose.is_none();
        if let Some(meter) = skater.balance_meter().filter(|_| range) {
            time = (meter + 1.0) / 2.0 * duration;
            duration = duration.max(f32::EPSILON);
        }
        // Into a different animation: blend from the pose shown, as
        // the game's `PlayAnim` does over its `BlendPeriod`.
        let name = character.animations[index].0.as_str();
        if index != st.shown {
            let period = skate::anims::blend_period(name);
            st.blend_from = match (&st.pose, period > 0.0) {
                (Some(pose), true) => Some((pose.clone(), 0.0, period)),
                _ => None,
            };
        }
        st.animation = index;
        st.shown = index;
        st.duration = duration;
        // Held on its last frame, or looping round.
        st.time = if duration <= 0.0 {
            0.0
        } else if hold {
            time.clamp(0.0, duration * 0.999)
        } else {
            time.rem_euclid(duration)
        };
    }
    // The pose: the animation's, blended from the last one's.
    let mut local = character.local_pose(st.animation, st.time);
    if let Some((from, elapsed, period)) = &mut st.blend_from {
        *elapsed += dt;
        let t = (*elapsed / *period).clamp(0.0, 1.0);
        // Eased, so it starts and settles gently.
        let t = t * t * (3.0 - 2.0 * t);
        for (to, from) in local.iter_mut().zip(from.iter()) {
            to.0 = from.0.slerp(to.0, t);
            to.1 = from.1.lerp(to.1, t);
        }
        if *elapsed >= *period {
            st.blend_from = None;
        }
    }
    st.pose = Some(local);
}

/// Where and how the skater's drawn `time` into going through a warp
/// (`WarpAi`): at the warp, spinning ever faster (each `Rotate y = N
/// duration = d` a quarter second after the last, until the 10080 over
/// 5 seconds), and from 1.75 s shrinking to a tenth over 3 s
/// (`calcEase`), then gone (`skater:Hide`).
/// When the skater's gone, going through a warp (`skater:Hide`).
const WARP_GONE: f32 = 4.75;

fn warp_spin_placement(at: Vec3, time: f32, stood: Mat4) -> Mat4 {
    // (Start, degrees a second.)
    const SPINS: [(f32, f32); 7] = [
        (0.0, 360.0),
        (0.25, 540.0),
        (0.5, 720.0),
        (0.75, 1080.0),
        (1.0, 1440.0),
        (1.25, 1800.0),
        (1.75, 2016.0),
    ];
    let mut degrees = 0.0;
    for (i, &(start, rate)) in SPINS.iter().enumerate() {
        let end = SPINS.get(i + 1).map_or(start + 5.0, |next| next.0);
        degrees += rate * (time.min(end) - start).max(0.0);
    }
    let shrink = ((time - 1.75) / 3.0).clamp(0.0, 1.0);
    let left = 1.0 - shrink * 0.9;
    let scale = if time > WARP_GONE {
        1e-4
    } else {
        left * left * (3.0 - 2.0 * left)
    };
    let (stood_scale, rotation, _) = stood.to_scale_rotation_translation();
    Mat4::from_scale_rotation_translation(
        stood_scale * scale,
        Quat::from_rotation_y(degrees.to_radians()) * rotation,
        at,
    )
}

fn placement_at(spawn: &Spawn) -> Mat4 {
    Mat4::from_rotation_translation(
        Quat::from_rotation_arc(Vec3::Z, spawn.facing()),
        spawn.position,
    )
}

/// Field of view for camera paths that don't set one (the engine's usual 72
/// degrees, horizontal).
const DEFAULT_PATH_FOV: f32 = 72.0;

/// How long the viewer plays a camera path: until a second after its last
/// key (its stated duration often runs much longer; see `ngc_anim::camera`).
fn path_length(path: &CameraPath) -> f32 {
    (path.last_key_time() + 1.0).min(path.duration.max(0.0))
}

/// Where a camera path puts the camera at `seconds`.
fn path_camera(path: &CameraPath, seconds: f32) -> ScriptedCamera {
    let at = path.sample(seconds);
    ScriptedCamera {
        rotation: at.rotation,
        position: at.position,
        fov_y: vertical_fov(at.fov.unwrap_or(DEFAULT_PATH_FOV.to_radians())),
    }
}

/// The picture's shape for the game's screen over it (see
/// `picture_area`), kept in egui's memory.
fn set_picture_aspect(ctx: &egui::Context, aspect: Option<f32>) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("picture_aspect"), aspect));
}

/// Where the game's screen goes: in the picture (its shape's, if it has
/// one of its own: see `Video::aspect`), beside the panel.
fn picture_area(ctx: &egui::Context) -> egui::Rect {
    let aspect = ctx.data(|d| d.get_temp::<Option<f32>>(egui::Id::new("picture_aspect")));
    let window = ctx.viewport_rect();
    let (x, y, w, h) =
        desa_viewer::camera::picture_box(window.width(), window.height(), aspect.flatten());
    let picture = egui::Rect::from_min_size(window.min + egui::vec2(x, y), egui::vec2(w, h));
    ctx.available_rect().intersect(picture)
}

/// The bars' picture: a loading screen's, or the player's `bars.png` (or
/// `.jpg`).
fn bars_image(data: &mut GameData, bars: &Bars) -> Option<egui::ColorImage> {
    let (w, h, rgba) = match bars {
        Bars::Black => return None,
        Bars::Art(name) => {
            let image = data.loading_screen(name).ok()??;
            (image.width, image.height, image.rgba)
        }
        Bars::Custom => {
            let path = settings::custom_bars()?;
            let png = path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("png"));
            if !png {
                use zune_jpeg::zune_core::{colorspace::ColorSpace, options::DecoderOptions};
                let bytes = std::fs::read(&path).ok()?;
                let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA);
                let mut decoder = zune_jpeg::JpegDecoder::new_with_options(&bytes[..], options);
                let rgba = decoder.decode().ok()?;
                let (w, h) = decoder.dimensions()?;
                return Some(egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba));
            }
            let file = std::fs::File::open(path).ok()?;
            let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
            decoder.set_transformations(png::Transformations::normalize_to_color8());
            let mut reader = decoder.read_info().ok()?;
            let mut buf = vec![0; reader.output_buffer_size()?];
            let info = reader.next_frame(&mut buf).ok()?;
            buf.truncate(info.buffer_size());
            let rgba = match info.color_type {
                png::ColorType::Rgba => buf,
                png::ColorType::Rgb => buf
                    .chunks_exact(3)
                    .flat_map(|p| [p[0], p[1], p[2], 255])
                    .collect(),
                png::ColorType::GrayscaleAlpha => buf
                    .chunks_exact(2)
                    .flat_map(|p| [p[0], p[0], p[0], p[1]])
                    .collect(),
                png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
                png::ColorType::Indexed => return None,
            };
            (info.width, info.height, rgba)
        }
    };
    Some(egui::ColorImage::from_rgba_unmultiplied(
        [w as usize, h as usize],
        &rgba,
    ))
}

/// A loading screen's name as the Video Options show it
/// (`loadscrn_pizza_buzz`: "Pizza Buzz").
fn art_title(name: &str) -> String {
    let name = name
        .get(..9)
        .filter(|p| p.eq_ignore_ascii_case("loadscrn_"))
        .map_or(name, |_| &name[9..]);
    name.split('_')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut chars = w.chars();
            chars.next().map_or(String::new(), |c| {
                c.to_uppercase().chain(chars).collect::<String>()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The bars round a picture not the window's shape, with a picture in
/// them (each bar covered by it, its middle, dimmed a little: a tall
/// picture, a box's cover, fits the bars at the sides), under everything
/// else on the screen.
fn paint_bars(ctx: &egui::Context, texture: Option<&egui::TextureHandle>) {
    let Some(texture) = texture else { return };
    let aspect = ctx.data(|d| d.get_temp::<Option<f32>>(egui::Id::new("picture_aspect")));
    let Some(aspect) = aspect.flatten() else {
        return;
    };
    let window = ctx.viewport_rect();
    let (x, y, w, h) =
        desa_viewer::camera::picture_box(window.width(), window.height(), Some(aspect));
    let picture = egui::Rect::from_min_size(window.min + egui::vec2(x, y), egui::vec2(w, h));
    let size = texture.size_vec2();
    let painter = ctx.layer_painter(egui::LayerId::background());
    let bars = [
        egui::Rect::from_min_max(window.min, egui::pos2(picture.min.x, window.max.y)),
        egui::Rect::from_min_max(egui::pos2(picture.max.x, window.min.y), window.max),
        egui::Rect::from_min_max(window.min, egui::pos2(window.max.x, picture.min.y)),
        egui::Rect::from_min_max(egui::pos2(window.min.x, picture.max.y), window.max),
    ];
    for bar in bars
        .into_iter()
        .filter(|b| b.width() > 0.5 && b.height() > 0.5)
    {
        // The picture's middle, the bar's shape.
        let k = (bar.width() / size.x).max(bar.height() / size.y);
        let seen = bar.size() / (size * k);
        let uv = egui::Rect::from_center_size(egui::pos2(0.5, 0.5), seen);
        painter.image(texture.id(), bar, uv, egui::Color32::from_gray(190));
    }
}

/// The HUD's arrows (the game's `HUD_arrow` model, at the top of its
/// 640x480 screen): drawn flat, turned by each angle (0 straight up,
/// positive to the right), gold with a dark edge.
fn paint_hud_arrows(ctx: &egui::Context, angles: &[f32]) {
    use desa_viewer::screen::{HEIGHT, WIDTH};
    if angles.is_empty() {
        return;
    }
    let area = picture_area(ctx);
    let k = (area.width() / WIDTH).min(area.height() / HEIGHT);
    let origin = area.center() - egui::vec2(WIDTH, HEIGHT) * k / 2.0;
    let centre = origin + egui::vec2(320.0, 70.0) * k;
    let size = 22.0 * k;
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("hud_arrows"),
    ));
    let gold = egui::Color32::from_rgb(250, 196, 40);
    let edge = egui::Stroke::new(2.0 * k.max(0.5), egui::Color32::from_rgb(70, 40, 10));
    for &angle in angles {
        let (sin, cos) = angle.sin_cos();
        let at = |x: f32, y: f32| centre + egui::vec2(x * cos - y * sin, x * sin + y * cos) * size;
        let (tip, left, notch, right) = (at(0.0, -1.0), at(-0.7, 0.6), at(0.0, 0.2), at(0.7, 0.6));
        for half in [[tip, left, notch], [tip, notch, right]] {
            painter.add(egui::Shape::convex_polygon(
                half.to_vec(),
                gold,
                egui::Stroke::NONE,
            ));
        }
        painter.add(egui::Shape::closed_line(
            vec![tip, left, notch, right],
            edge,
        ));
    }
}

/// A camera in front of a character placed by `placement`, looking at it.
fn camera_facing(placement: Mat4) -> FlyCamera {
    camera_around(placement, 0.0, 170.0, 75.0)
}

/// A camera `distance` from a character, `orbit` degrees around them from
/// the front (toward their left), `height` above their feet, looking at
/// their middle.
fn camera_around(placement: Mat4, orbit: f32, distance: f32, height: f32) -> FlyCamera {
    let position = placement.transform_point3(Vec3::ZERO);
    let forward = placement.transform_vector3(Vec3::Z).normalize_or(Vec3::Z);
    let direction = Quat::from_rotation_y(orbit.to_radians()) * forward;
    FlyCamera::looking_at(
        position + direction * distance + Vec3::Y * height,
        position + Vec3::Y * 45.0,
    )
}

struct Gpu {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    device: wgpu::Device,
    queue: wgpu::Queue,
    egui_ctx: egui::Context,
    egui_state: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,
}

struct App<'a> {
    settings: &'a mut Settings,
    /// The game alone (the game build): no panel, debug keys or markers.
    game: bool,
    /// In the game alone, the panel's message shown on screen instead, and
    /// since when (it goes after a few seconds).
    notice: Option<(String, Instant)>,
    /// The camera path playing is the menus' (`playskaterCamAnim ...
    /// play_hold`): held on its last frame when it ends, not handed to the
    /// free camera (whose view's wider).
    camera_hold: bool,
    /// The main menu's Exit Game chosen (and its QUIT? said yes to).
    exit_game: bool,
    /// The Video Options' values as last shown in its menu.
    video_labels: Option<[String; 6]>,
    /// The Advanced menu's look values as last shown.
    look_labels: Option<[String; 6]>,
    /// The Video Options as changed in its menu, till Apply (or leaving
    /// it, which forgets them).
    video_pending: Option<Video>,
    /// The bars' picture as loaded (what it's of, and it).
    bars: Option<(Bars, egui::TextureHandle)>,
    /// When the next frame's due, under a frame limit.
    next_frame: Instant,
    gpu: Option<Gpu>,
    data: Option<GameData>,
    level: Option<LoadedLevel>,
    model: ui::Model,
    camera: FlyCamera,
    keys: HashSet<KeyCode>,
    /// Gamepads, if the platform has them.
    gamepads: Option<gilrs::Gilrs>,
    looking: bool,
    /// A level to load, and how many frames the "Loading" message has shown.
    pending: Option<(usize, u32)>,
    next_spawn: usize,
    character: Option<Character>,
    /// Where the character stands.
    placement: Mat4,
    /// Skating: the skater, its constants and the chase camera's eye.
    skating: Option<(Skater, Physics, ChaseCamera)>,
    /// The skater's sounds (none in screenshots, or without a sound
    /// device).
    audio: Option<audio::Audio>,
    /// The sparks off a grinding board.
    sparks: sparks::Sparks,
    /// The gamepad's rumble.
    rumble: rumble::Rumble,
    /// A two-minute run under way.
    run: Option<Run>,
    /// The run's frames as they were shown, and how far into watching
    /// them again (seconds) while a replay plays.
    recording: Vec<ReplayFrame>,
    replay: Option<f32>,
    /// The S-K-A-T-E letters goal under way, and what its pro says for
    /// each letter.
    letters: Option<LetterRun>,
    letter_lines: [Option<u32>; 5],
    /// The race goal under way.
    racing: Option<RaceRun>,
    /// A goal played from its own scripts, under way.
    goal_run: Option<GoalRun>,
    /// A goal's objective to pop up as it starts (see `update_game_hud`),
    /// and frames till then.
    goal_popup: Option<(String, u32, Option<String>)>,
    /// The next goal started as a retry (`RetryCurrentGoal`): straight
    /// on, without its pro's description (the game's `quick_start`).
    quick_start: bool,
    /// A score, letters or race goal just ended (won, its type), for
    /// the game's messages (see `update_game_hud`).
    goal_end: Option<(bool, GoalEnded)>,
    /// The camera turned to look round (yaw, and up or down) and the right
    /// stick as last read; whether the pad's Start was down, to toggle the
    /// pause on pressing it.
    look: glam::Vec2,
    pad_look: glam::Vec2,
    pad_start: bool,
    /// This session's skating (since the viewer started).
    session: Session,
    /// The camera setting following the skater now (index into the
    /// panel's list), and whether the pad's Back was down.
    camera_shown: usize,
    /// The created kid's wardrobe, as a boy (false) and a girl (true), with
    /// the parts picked: read from the Skate Shop's lists when first shown.
    wardrobes: HashMap<bool, Vec<desa_viewer::source::KidSlot>>,
    /// Which of them the panel's showing.
    kid_shown: Option<bool>,
    /// The kid's parts as last kept by the Edit Skater menus (a part
    /// accepted), to put back what was only tried on.
    kid_kept: HashMap<bool, Vec<KidWorn>>,
    /// Dressed by the menus just now (not the panel: not kept).
    kid_from_menus: bool,
    /// The kid's body shape (bones scaled by name), as the shop makes it,
    /// and its overall scale if not its appearance's (a crew kid's); and
    /// those as last kept by the menus.
    kid_shape: Vec<(u32, Vec3)>,
    kid_scale: Option<f32>,
    /// Going through a warp (`WarpAi`, `WarpAIHub`): where it stands, how
    /// long since, and how the skater stood, for its spin and shrink.
    warp_spin: Option<(Vec3, f32, Mat4)>,
    kid_shape_kept: Option<(KidShape, Option<f32>)>,
    /// An appearance for the other kid (boy or girl), put on once it's
    /// shown.
    kid_pending: Option<desa_viewer::screen::Appearance>,
    /// A created skater's slot for the other kid, put on once it's shown.
    kid_pending_slot: Option<usize>,
    pad_back: bool,
    /// The pad's B, as last read, and pressed since (to talk to a pro).
    pad_talk: bool,
    talk_pressed: bool,
    /// What a teleporter created or killed on the way, to apply.
    pending_creates: Vec<(desa_viewer::triggers::CreateTarget, bool)>,
    /// After a splash: the camera held where it was, and for how long.
    splash_hold: Option<(FlyCamera, f32)>,
    /// Frames drawn (for things done now and then).
    frame: u32,
    /// The level objects of the world's vehicles, in order, and the one
    /// the skater's skitching on.
    vehicle_objects: Vec<usize>,
    skitched: Option<usize>,
    /// Which replay camera, and where its eye is (eased).
    replay_camera: usize,
    replay_eye: Option<Vec3>,
    /// A goal's camera path playing (skating held till it's over or
    /// skipped), the one asked for next, and the goal on's success path.
    cutscene: bool,
    cutscene_request: Option<u32>,
    /// The skater select's choice last followed (`disney_skater`), and
    /// whether the shop's stage camera has been put up since arriving.
    shop_skater: Option<i32>,
    shop_staged: bool,
    /// How far the skater select's turned the character on the stage
    /// (its L and R: radians).
    stage_turn: f32,
    /// The song the game's menus play (the skater select's theme for the
    /// character), by name, and whether it's started.
    menu_track: Option<(String, bool)>,
    /// The movie last asked for on the level's screen, to loop it, and
    /// one a teleporter asked for (or stopped) for it.
    screen_movie_name: Option<String>,
    /// When the level's screen movie was paused (`PauseMovie`).
    screen_movie_paused: Option<Instant>,
    teleport_movie: Option<Option<(String, bool)>>,
    /// Whether the camera path playing is one the level's scripts asked
    /// for (theirs to stop).
    script_cutscene: bool,
    /// A goal just won (its kind), to keep.
    goal_won: Option<String>,
    /// A warp's level, to load when its camera path's over.
    load_after_cutscene: Option<usize>,
    success_camera: Option<u32>,
    /// Where the goal pedestrians' voice lines are (read once), and lines
    /// to say.
    goal_streams: Option<HashMap<u32, (u64, u64)>>,
    lines_to_say: Vec<u32>,
    /// The Moon Gravity and Slomo cheats' factors.
    moon_gravity: f32,
    slomo_speed: f32,
    /// The game's own menus (its screen elements), when the data has
    /// their sprites and fonts; and the main menu waiting on the Skate
    /// Shop loading.
    screen: Option<ScreenUi>,
    main_menu_pending: bool,
    /// The controller's menu presses held last frame.
    menu_held: Vec<desa_viewer::screen::Pad>,
    /// The game's own panel while skating, and the frame's time for it.
    hud: GameHud,
    hud_dt: f32,
    /// The movie playing, and those to play after it (by name).
    movie: Option<MoviePlaying>,
    movie_queue: std::collections::VecDeque<String>,
    /// F12 pressed: the next frame is saved as a picture too.
    photo: bool,
    /// The goal a pro's offering (its place in the level's pros).
    goal_offer: Option<usize>,
    /// The warp offered (the level's portal) and, once taken, to skate on
    /// arriving.
    warp_offer: Option<usize>,
    skate_on_load: bool,
    /// The character's collectibles on the level, while skating.
    collecting: Option<Collecting>,
    /// The combo under way's longest grind, manual and lip trick (seconds)
    /// and those going on, the combos ended so far, and new records
    /// waiting to be announced (with how long the one shown has had).
    combo_lengths: [f32; 3],
    running_lengths: [f32; 3],
    combos_seen: u32,
    record_queue: std::collections::VecDeque<String>,
    record_shown: f32,
    /// The songs (`playlist_tracks`, shuffled) and the next to play, and
    /// the level's ambience (`ambient_track`), by name.
    playlist: Vec<(String, String)>,
    /// The song that started last (its title) and when, shown a moment.
    now_playing: Option<(String, Instant)>,
    next_track: usize,
    ambience: Option<String>,
    /// The animation the character's time belongs to.
    shown_animation: usize,
    /// Skating: the pose shown (blended), and the pose being blended from
    /// with seconds into the blend and its length.
    skate_pose: Option<Pose>,
    blend_from: Option<(Pose, f32, f32)>,
    /// Two players' free skate: player two's character (by index in the
    /// list), from the menus' second skater select.
    two_player: Option<usize>,
    /// Player one's character, picked in the menus before player two's.
    p1_pick: Option<usize>,
    /// Whose skater the menus were choosing, last seen.
    menu_profile: usize,
    player2: Option<Player2>,
    /// Two players' game on (Score Challenge, King of the Hill, Horse).
    versus: Option<versus::Versus>,
    /// Player two's pad, as last read (the second pad).
    pad2: Input,
    /// The halves side by side (the game's `split_vertical`), else one
    /// over the other.
    side_by_side: bool,
    /// Two players' first level this time: paused on the game's TWO
    /// PLAYERS menu as it arrives (`FirstTimeInSplitScreen`).
    split_first: bool,
    last_frame: Instant,
    started: Instant,
    error: Option<anyhow::Error>,
}

/// A part of the created kid as worn: the choice picked and its colour.
type KidWorn = (usize, Option<[f32; 3]>);

/// The created kid's bones scaled, by name.
type KidShape = Vec<(u32, Vec3)>;

impl<'a> App<'a> {
    fn new(settings: &'a mut Settings) -> Self {
        let speed = settings.speed.unwrap_or(DEFAULT_SPEED);
        let camera = settings.best.get("camera").map_or(1, |c| *c as usize);
        let simplified = settings.best.get("simplified") == Some(&1);
        let spin_taps = settings.best.get("spintaps") == Some(&1);
        let volume = |key: &str| settings.best.get(key).map_or(1.0, |v| *v as f32 / 100.0);
        let (effects_volume, music_volume) = (volume("volume.effects"), volume("volume.music"));
        let intro = settings.best.get("intro") != Some(&0);
        let start_menu = settings.best.get("start_menu") != Some(&0);
        App {
            settings,
            game: false,
            notice: None,
            camera_hold: false,
            exit_game: false,
            video_labels: None,
            look_labels: None,
            video_pending: None,
            bars: None,
            next_frame: Instant::now(),
            gpu: None,
            data: None,
            level: None,
            model: ui::Model {
                split_huds: None,
                split_side_by_side: false,
                split_solo: None,
                data_path: None,
                levels: Vec::new(),
                movies: Vec::new(),
                intro,
                start_menu,
                level_progress: Vec::new(),
                current: None,
                loading: None,
                stats: None,
                spawns: Vec::new(),
                has_collision: false,
                show_sky: true,
                show_rails: true,
                show_spawns: true,
                show_objects: true,
                show_goal_objects: false,
                brighten: 0.0,
                collision: CollisionView::Hidden,
                speed,
                camera_text: String::new(),
                message: None,
                panel_open: true,
                camera_paths: ui::CameraPathModel {
                    paths: Vec::new(),
                    playing: None,
                    time: 0.0,
                },
                character: ui::CharacterModel {
                    characters: Vec::new(),
                    current: None,
                    animations: Vec::new(),
                    animation: 0,
                    playing: true,
                    speed: 1.0,
                    time: 0.0,
                    duration: 0.0,
                    can_blink: false,
                    blink: true,
                    can_skate: false,
                    skating: false,
                    balance: None,
                    score: 0,
                    combo: None,
                    message: None,
                    special: (0.0, false),
                    auto_kick: true,
                    wallride_anywhere: true,
                    cheats: Default::default(),
                    sound: true,
                    music: true,
                    effects_volume,
                    music_volume,
                    rumble: true,
                    simplified,
                    spin_taps,
                    kid_slots: Vec::new(),
                    run_clock: None,
                    run_result: None,
                    run_goal: None,
                    run_goal_won: None,
                    score_goals: [None, None],
                    collect: true,
                    collected: None,
                    replaying: false,
                    replay_camera: REPLAY_CAMERAS[0].0.to_string(),
                    goals: Vec::new(),
                    goals_won: Vec::new(),
                    goal_progress: None,
                    goal_result: None,
                    game_menu: false,
                    game_hud: false,
                    can_letters: false,
                    race_name: None,
                    race: None,
                    race_result: None,
                    letters: None,
                    letters_result: None,
                    skate_status: String::new(),
                    trick_list: Vec::new(),
                    gap_list: Vec::new(),
                    records: Vec::new(),
                    record_message: None,
                    warp_prompt: None,
                    paused: false,
                    goal_prompt: None,
                    session: Vec::new(),
                    switch: false,
                    show_map: true,
                    map_image: None,
                    map_texture: None,
                    map: None,
                    cameras: Vec::new(),
                    camera,
                },
            },
            camera: FlyCamera::looking_at(Vec3::new(0.0, 500.0, 1000.0), Vec3::ZERO),
            keys: HashSet::new(),
            gamepads: gilrs::Gilrs::new().ok(),
            looking: false,
            pending: None,
            next_spawn: 0,
            character: None,
            placement: Mat4::IDENTITY,
            skating: None,
            audio: None,
            sparks: sparks::Sparks::new(),
            rumble: rumble::Rumble::new(),
            run: None,
            recording: Vec::new(),
            replay: None,
            letters: None,
            letter_lines: [None; 5],
            racing: None,
            goal_run: None,
            goal_popup: None,
            quick_start: false,
            goal_end: None,
            photo: false,
            movie: None,
            movie_queue: Default::default(),
            screen: None,
            main_menu_pending: false,
            menu_held: Vec::new(),
            hud: GameHud::default(),
            hud_dt: 0.0,
            moon_gravity: 0.5,
            slomo_speed: 0.5,
            goal_streams: None,
            lines_to_say: Vec::new(),
            cutscene: false,
            shop_skater: None,
            shop_staged: false,
            stage_turn: 0.0,
            menu_track: None,
            screen_movie_name: None,
            screen_movie_paused: None,
            teleport_movie: None,
            script_cutscene: false,
            cutscene_request: None,
            goal_won: None,
            load_after_cutscene: None,
            success_camera: None,
            replay_camera: 0,
            replay_eye: None,
            vehicle_objects: Vec::new(),
            skitched: None,
            frame: 0,
            splash_hold: None,
            pending_creates: Vec::new(),
            camera_shown: usize::MAX,
            wardrobes: HashMap::new(),
            kid_shown: None,
            kid_kept: HashMap::new(),
            kid_from_menus: false,
            kid_shape: Vec::new(),
            kid_scale: None,
            warp_spin: None,
            kid_shape_kept: None,
            kid_pending: None,
            kid_pending_slot: None,
            pad_back: false,
            pad_talk: false,
            talk_pressed: false,
            session: Session::default(),
            look: glam::Vec2::ZERO,
            pad_look: glam::Vec2::ZERO,
            pad_start: false,
            warp_offer: None,
            goal_offer: None,
            skate_on_load: false,
            collecting: None,
            combo_lengths: [0.0; 3],
            running_lengths: [0.0; 3],
            combos_seen: 0,
            record_queue: Default::default(),
            record_shown: 0.0,
            playlist: Vec::new(),
            now_playing: None,
            next_track: 0,
            ambience: None,
            shown_animation: 0,
            skate_pose: None,
            blend_from: None,
            two_player: None,
            p1_pick: None,
            menu_profile: 0,
            player2: None,
            versus: None,
            pad2: Input::default(),
            side_by_side: false,
            split_first: true,
            last_frame: Instant::now(),
            started: Instant::now(),
            error: None,
        }
    }

    fn open_data(&mut self, path: &Path) {
        match GameData::open(path) {
            Ok(mut data) => {
                self.model.levels = data.levels();
                self.model.movies = data.movies();
                self.screen = ScreenUi::load(&mut data);
                self.model.data_path = Some(path.display().to_string());
                self.model.message = if self.model.levels.is_empty() {
                    Some("No levels found there.".into())
                } else {
                    None
                };
                self.model.current = None;
                self.level = None;
                self.model.character.characters = data.characters();
                self.data = Some(data);
                // Characters come from the old data too. Keep the remembered
                // one, so it can be shown again from the new data.
                self.character = None;
                self.model.character.current = None;
                self.model.character.animations.clear();
                self.settings.data_path = Some(path.to_path_buf());
                self.settings.save();
            }
            Err(err) => self.model.message = Some(format!("{err:#}")),
        }
    }

    fn start_load(&mut self, index: usize) {
        self.warp_spin = None;
        if let Some(info) = self.model.levels.get(index) {
            self.model.loading = Some(info.title.clone());
            self.pending = Some((index, 0));
        }
    }

    fn finish_load(&mut self, index: usize) {
        self.model.loading = None;
        let (Some(gpu), Some(data)) = (&self.gpu, &mut self.data) else {
            return;
        };
        let info = self.model.levels[index].clone();
        match load_level(data, &info, &gpu.device, &gpu.queue, gpu.config.format) {
            Ok((mut level, stats)) => {
                self.camera = level.start;
                self.placement = level.home;
                if let Some(character) = &self.character {
                    level.renderer.set_character(Some(&character.mesh));
                }
                self.model.current = Some(index);
                self.model.stats = Some(stats);
                // A new level: stop skating the old one, and whether this
                // one can be skated.
                if self.skating.take().is_some() {
                    self.model.character.skating = false;
                    if let Some(audio) = &mut self.audio {
                        audio.stop();
                    }
                    self.model.character.playing = true;
                    self.model.character.balance = None;
                    self.model.character.combo = None;
                }
                let shop = info.id.eq_ignore_ascii_case("SkateShop");
                self.model.character.can_skate =
                    self.character.is_some() && !shop && level.world.is_some();
                self.model.character.score_goals = [false, true].map(|pro| {
                    desa_viewer::goals::score_goal(level.behaviour.program(), &info.id, pro)
                        .map(|g| g.name)
                });
                self.model.character.goals =
                    desa_viewer::goals::level_goals(level.behaviour.program(), &info.id)
                        .into_iter()
                        .map(|g| (g.kind, g.text))
                        .collect();
                self.model.character.race_name =
                    desa_viewer::goals::race(level.behaviour.program(), &info.id).map(|r| r.name);
                self.model.character.can_letters = level
                    .nodes
                    .objects
                    .iter()
                    .any(|o| o.name == qb::checksum("TRG_Goal_Letter_S"));
                self.model.has_collision = level.renderer.has_collision();
                if !self.model.has_collision {
                    self.model.collision = CollisionView::Hidden;
                }
                self.model.set_spawns(&level.nodes.spawns);
                // The new level's map.
                self.model.character.map = None;
                self.model.character.map_texture = None;
                self.model.character.map_image = level.minimap.as_ref().map(|m| {
                    egui::ColorImage::from_rgba_unmultiplied([m.width, m.height], &m.pixels)
                });
                self.camera_hold = false;
                self.model.camera_paths = ui::CameraPathModel {
                    paths: level
                        .camera_paths
                        .iter()
                        .map(|(name, path)| (name.clone(), path_length(path)))
                        .collect(),
                    playing: None,
                    time: 0.0,
                };
                self.model.message = None;
                self.next_spawn = 0;
                self.level = Some(level);
                self.scale_character();
                // The goals won here before, for the scripts that ask.
                if let Some(level) = &mut self.level {
                    for kind in ["HighScore", "ProScore", "SKATE", "Race"] {
                        let id = desa_viewer::goals::goal_id(&info.id, kind);
                        if self.settings.best.contains_key(&format!("won.{id:08x}")) {
                            level.behaviour.won_goals.insert(id);
                        }
                    }
                }
                // The game's menus know the level (`LevelIs load_skateshop`),
                // and the main menu comes up if it was waiting on it.
                let shop = info.id.eq_ignore_ascii_case("SkateShop");
                if let Some(screen) = &mut self.screen {
                    screen.screen.clear();
                    screen.screen.level = Some(qb::checksum(&format!("load_{}", info.id)));
                }
                if !shop {
                    self.main_menu_pending = false;
                }
                self.launch_pending_main_menu();
                if !shop {
                    self.settings.last_skated = Some(info.id.clone());
                }
                self.settings.last_level = Some(info.id);
                self.settings.save();
                // Arrived through a warp: skating on (its sounds with it);
                // else the level's sounds and songs now, for its menus.
                if std::mem::take(&mut self.skate_on_load) {
                    self.toggle_skate();
                } else {
                    self.start_audio();
                }
            }
            Err(err) => self.model.message = Some(format!("{err:#}")),
        }
    }

    fn play_camera_path(&mut self, index: usize) {
        self.camera_hold = false;
        self.model.camera_paths.playing = Some(index);
        self.model.camera_paths.time = 0.0;
    }

    /// Plays a goal's camera path by name, holding the skater till it's
    /// over (or skipped with a key).
    fn play_cutscene(&mut self, name: u32) {
        let Some(level) = &self.level else { return };
        let Some(index) = level
            .camera_paths
            .iter()
            .position(|(n, _)| qb::checksum(n) == name)
        else {
            return;
        };
        self.play_camera_path(index);
        self.cutscene = true;
    }

    /// The cutscene asked for, started; and when one's over, the chase
    /// camera back.
    fn update_cutscene(&mut self) {
        if let Some(kind) = self.goal_won.take() {
            self.won_goal(&kind);
        }
        if let Some(name) = self.cutscene_request.take() {
            self.play_cutscene(name);
        }
        if self.cutscene && self.model.camera_paths.playing.is_none() {
            self.cutscene = false;
            // (Shown again after the scripts' cutscene, whatever they do.)
            if self.script_cutscene {
                if let Some(level) = &mut self.level {
                    level.behaviour.skater_hidden = false;
                }
            }
            self.script_cutscene = false;
        }
        // The skater hidden by the scripts while their cutscene plays.
        if let Some(level) = &mut self.level {
            level.renderer.hide_character = self.script_cutscene && level.behaviour.skater_hidden;
        }
        // A warp's camera path over: on to the level.
        if !self.cutscene {
            if let Some(index) = self.load_after_cutscene.take() {
                self.start_load(index);
            }
        }
    }

    /// Says the voice lines asked for: the goal pedestrians' own
    /// (`streams/goalpeds`), read off the disc as they're wanted.
    fn say_lines(&mut self) {
        let heard = self.audio.is_some() && self.model.character.sound;
        if let Some(level) = &mut self.level {
            level.behaviour.voices_heard = heard;
            level.behaviour.hawk_controls = !self.model.character.simplified;
            // (Cut off by the scripts, unless a new one's said after.)
            if std::mem::take(&mut level.behaviour.stop_voice) {
                if let Some(audio) = &mut self.audio {
                    audio.stop_line();
                }
            }
            self.lines_to_say
                .extend(std::mem::take(&mut level.behaviour.voice_lines));
        }
        if self.lines_to_say.is_empty() {
            return;
        }
        let lines = std::mem::take(&mut self.lines_to_say);
        let (Some(audio), Some(data)) = (
            self.audio.as_mut().filter(|_| self.model.character.sound),
            &mut self.data,
        ) else {
            return;
        };
        if self.goal_streams.is_none() {
            self.goal_streams = Some(data.goal_streams().unwrap_or_default());
        }
        let Some(streams) = &self.goal_streams else {
            return;
        };
        // The last asked for (a newer line cuts an older one off).
        if let Some(range) = lines.iter().rev().find_map(|l| streams.get(l)) {
            if let Ok(Some(sound)) = data.stream(*range) {
                let seconds = audio.say_line(&sound);
                // (For the scripts that wait on it: `Obj_WaitStreamFinished`.)
                if let Some(level) = &mut self.level {
                    level.behaviour.voice_for(seconds);
                }
            }
        }
    }

    /// A goal won: kept, and the level's scripts told.
    fn won_goal(&mut self, kind: &str) {
        let Some(level) = &mut self.level else { return };
        let id = desa_viewer::goals::goal_id(&level.id, kind);
        level.behaviour.won_goals.insert(id);
        self.settings.best.insert(format!("won.{id:08x}"), 1);
        self.settings.save();
    }

    /// Starts the next movie waiting, and takes the playing one's frame
    /// for now; at its end, on to the next.
    fn update_movie(&mut self) {
        if let Some(playing) = &mut self.movie {
            let seconds = playing.started.elapsed().as_secs_f64();
            let (w, h) = (playing.movie.info.width, playing.movie.info.height);
            if let Some((frame, true)) = playing.movie.frame_at(seconds) {
                playing.pending = Some(egui::ColorImage::from_rgba_unmultiplied(
                    [w as usize, h as usize],
                    frame,
                ));
            }
            // (A little past its last frame, for the sound to end.)
            if playing.movie.finished() || seconds > playing.movie.info.duration() + 1.0 {
                self.stop_movie();
            }
            return;
        }
        let Some(name) = self.movie_queue.pop_front() else {
            // The opening's over: the main menu, if it was waiting.
            self.launch_pending_main_menu();
            return;
        };
        let (Some(source), Some(ffmpeg)) = (
            self.data.as_ref().and_then(|d| d.movie(&name)),
            desa_viewer::movie::ffmpeg(),
        ) else {
            // (No ffmpeg: none of them, then.)
            self.movie_queue.clear();
            self.model.message =
                Some("Movies need ffmpeg (on the PATH, or named by DESA_FFMPEG).".into());
            return;
        };
        match desa_viewer::movie::Movie::start(&source, &ffmpeg) {
            Ok((movie, sound)) => {
                if let Some(audio) = &mut self.audio {
                    audio.play_movie(sound);
                }
                self.movie = Some(MoviePlaying {
                    movie,
                    started: Instant::now(),
                    texture: None,
                    pending: None,
                });
            }
            Err(err) => self.model.message = Some(format!("{name}: {err:#}")),
        }
    }

    /// The main menu waiting on the Skate Shop's loading, once it's loaded
    /// and the opening movies are over (the game boots into it after the
    /// intro).
    fn launch_pending_main_menu(&mut self) {
        let shop = self
            .level
            .as_ref()
            .is_some_and(|l| l.id.eq_ignore_ascii_case("SkateShop"));
        if !self.main_menu_pending || !shop || self.movie.is_some() || !self.movie_queue.is_empty()
        {
            return;
        }
        self.main_menu_pending = false;
        if let Some(screen) = &mut self.screen {
            screen
                .screen
                .run(qb::checksum("launch_main_menu"), Vec::new());
        }
    }

    /// Stops the movie playing (the next waiting starts after).
    fn stop_movie(&mut self) {
        self.movie = None;
        if let Some(audio) = &mut self.audio {
            audio.play_movie(None);
        }
    }

    /// The goal on's camera paths (`kind` as in `<level>_AddGoal_<kind>`):
    /// its start one plays now, its success one when it's won.
    fn goal_cutscenes(&mut self, kind: &str) {
        let Some(level) = &self.level else { return };
        let (start, success) =
            desa_viewer::goals::goal_cameras(level.behaviour.program(), &level.id, kind);
        self.success_camera = success;
        self.cutscene_request = start;
    }

    /// Stops a camera path, leaving the free camera where the path was.
    fn stop_camera_path(&mut self) {
        self.camera_hold = false;
        self.model.camera_paths.playing = None;
        self.cutscene = false;
        if let Some(level) = &mut self.level {
            if let Some(scripted) = level.renderer.scripted_camera.take() {
                self.camera = scripted.to_fly();
            }
        }
    }

    /// Advances the playing camera path, if any, and hands it to the renderer.
    fn play(&mut self, dt: f32) {
        let Some(index) = self.model.camera_paths.playing else {
            return;
        };
        let Some(level) = &mut self.level else { return };
        let Some((_, path)) = level.camera_paths.get(index) else {
            self.model.camera_paths.playing = None;
            return;
        };
        let time = &mut self.model.camera_paths.time;
        level.renderer.scripted_camera = Some(path_camera(path, *time));
        if *time >= path_length(path) {
            // (The menus' held there, as the game's `play_hold`.)
            if !self.camera_hold {
                self.stop_camera_path();
            }
        } else {
            *time = (*time + dt).min(path_length(path));
        }
    }

    /// Loads the level's sounds, songs and ambience (with a window and a
    /// sound device): as it loads, and again to skate.
    fn start_audio(&mut self) {
        if self.gpu.is_none() {
            return;
        }
        if self.audio.is_none() {
            self.audio = audio::Audio::new();
        }
        let (Some(audio), Some(data), Some(level)) = (&mut self.audio, &mut self.data, &self.level)
        else {
            return;
        };
        let mut files = data
            .sounds(&format!("{}.prg", level.id))
            .unwrap_or_default();
        files.extend(data.sounds("skater_sounds.prg").unwrap_or_default());
        let program = level.behaviour.program();
        let terrain = desa_viewer::sounds::TerrainSounds::new(program);
        // Each rail's terrain (metal where it doesn't say).
        let rail_terrain = level
            .nodes
            .rails
            .iter()
            .map(|r| {
                r.terrain
                    .and_then(|t| program.value(t))
                    .and_then(|v| v.as_int())
                    .and_then(|v| u16::try_from(v).ok())
                    .unwrap_or(3)
            })
            .collect();
        audio.load(files, terrain, rail_terrain);
        if let Some(index) = self.model.character.current {
            let id = self.model.character.characters[index].id.clone();
            audio.load_voices(data.voices(&id).unwrap_or_default());
        }
        // The songs, shuffled, and this level's ambience.
        let track = |v: &qb::Value| match v.get(qb::checksum("on_disk")) {
            Some(qb::Value::String(s)) => {
                let file = s.rsplit(['\\', '/']).next()?.to_string();
                let title = match v.get(qb::checksum("track_title")) {
                    Some(qb::Value::String(t)) => t.clone(),
                    _ => file.clone(),
                };
                Some((file, title))
            }
            _ => None,
        };
        let mut playlist: Vec<(String, String)> = program
            .value(qb::checksum("playlist_tracks"))
            .and_then(|v| v.as_array())
            .unwrap_or_default()
            .iter()
            .filter_map(track)
            .collect();
        let seed = self.started.elapsed().as_nanos() as usize;
        for i in (1..playlist.len()).rev() {
            playlist.swap(
                i,
                seed.wrapping_mul(2_654_435_761).wrapping_add(i * 40_503) % (i + 1),
            );
        }
        self.playlist = playlist;
        self.next_track = 0;
        let level_key = qb::checksum("level");
        let ambient_key = qb::checksum("ambient_track");
        self.ambience =
            program
                .values()
                .find_map(|(_, v)| match (v.get(level_key), v.get(ambient_key)) {
                    (Some(qb::Value::String(l)), Some(qb::Value::String(a)))
                        if l.eq_ignore_ascii_case(&level.id) =>
                    {
                        a.rsplit(['\\', '/']).next().map(str::to_string)
                    }
                    _ => None,
                });
        audio.set_ambience(None);
    }

    /// Keeps the music going while skating: the next song when one ends
    /// (with Music on), and the level's ambience (with Sound on).
    fn update_music(&mut self) {
        // The panel's volumes, kept when changed.
        let volumes = (
            self.model.character.effects_volume,
            self.model.character.music_volume,
        );
        if let Some(audio) = &mut self.audio {
            audio.set_volumes(volumes.0, volumes.1);
        }
        for (key, value) in [("volume.effects", volumes.0), ("volume.music", volumes.1)] {
            let value = (value * 100.0).round() as u32;
            if self.settings.best.get(key) != Some(&value) {
                self.settings.best.insert(key.into(), value);
                self.settings.save();
            }
        }
        let skating = self.skating.is_some();
        // No songs under a movie, nor before the main menu's up (the
        // opening's movies, the shop loading behind them).
        let movie = self.movie.is_some() || !self.movie_queue.is_empty() || self.main_menu_pending;
        // In the Skate Shop's menus the soundtrack plays too (the game's
        // `SetCDToMusic`), but for a song they ask for themselves.
        let shop = self
            .level
            .as_ref()
            .is_some_and(|l| l.id.eq_ignore_ascii_case("SkateShop"))
            && self.menu_track.is_none()
            && !movie;
        let (Some(audio), Some(data)) = (&mut self.audio, &mut self.data) else {
            return;
        };
        if (skating || shop) && self.model.character.music && !self.playlist.is_empty() {
            if audio.music_finished() {
                for _ in 0..self.playlist.len() {
                    let (name, title) = &self.playlist[self.next_track % self.playlist.len()];
                    self.next_track += 1;
                    if let Ok(Some(track)) = data.music(name) {
                        audio.play_music(track);
                        self.now_playing = Some((title.clone(), Instant::now()));
                        break;
                    }
                }
            }
        } else if let Some((name, started)) = self
            .menu_track
            .as_mut()
            .filter(|_| !skating && !movie && self.model.character.music)
        {
            // The menus' song, over and over (a new one in place of the
            // last).
            if !*started || audio.music_finished() {
                *started = true;
                match data.music(name) {
                    Ok(Some(track)) => audio.play_music(track),
                    _ => audio.stop_music(),
                }
            }
        } else {
            audio.stop_music();
        }
        if skating && self.model.character.sound {
            if !audio.has_ambience() {
                let track = self
                    .ambience
                    .as_deref()
                    .and_then(|name| data.music(name).ok().flatten());
                audio.set_ambience(track);
            }
            audio.keep_ambience();
        } else if audio.has_ambience() {
            audio.set_ambience(None);
        }
    }

    fn go_to_spawn(&mut self, index: usize) {
        self.stop_camera_path();
        if let Some(spawn) = self.level.as_ref().and_then(|l| l.nodes.spawns.get(index)) {
            self.camera = spawn.camera_clear_of(&self.level.as_ref().unwrap().collision);
            self.placement = placement_at(spawn);
            self.next_spawn = index + 1;
        }
    }

    /// Starts or stops skating the character from where it stands.
    fn toggle_skate(&mut self) {
        // What the scripts asked meanwhile isn't for this run.
        if let Some(level) = &mut self.level {
            level.behaviour.cameras.clear();
            level.behaviour.messages.clear();
        }
        self.skate_pose = None;
        self.run = None;
        self.warp_offer = None;
        self.model.character.warp_prompt = None;
        self.goal_offer = None;
        self.model.character.goal_prompt = None;
        self.model.character.paused = false;
        self.look = glam::Vec2::ZERO;
        self.session.last_position = None;
        self.recording.clear();
        self.replay = None;
        self.model.character.replaying = false;
        // The letters go when the goal does.
        if let Some(letters) = self.letters.take() {
            if let Some(level) = &mut self.level {
                for object in letters.objects {
                    level.behaviour.set_alive(object, false);
                }
            }
        }
        self.model.character.letters = None;
        self.model.character.letters_result = None;
        // A goal played from its scripts stops: its own end script runs.
        if let Some(run) = self.goal_run.take() {
            self.end_goal_run(run);
        }
        self.model.character.goal_progress = None;
        self.model.character.goal_result = None;
        // A race stops: its own end script runs (cars back, gates gone).
        if let Some(race) = self.racing.take() {
            if let (Some(level), Some(script)) = (&mut self.level, race.end_script) {
                level.behaviour.run_script(race.goal_runner, script);
            }
        }
        self.model.character.race = None;
        self.model.character.race_result = None;
        self.model.character.run_goal = None;
        self.model.character.run_goal_won = None;
        // The collectibles go too (they come back on skating again).
        if let Some(collecting) = self.collecting.take() {
            if let Some(level) = &mut self.level {
                for (object, _) in collecting.objects {
                    level.behaviour.set_alive(object, false);
                }
                if let Some((object, _)) = collecting.special {
                    level.behaviour.set_alive(object, false);
                }
            }
        }
        self.model.character.collected = None;
        self.model.character.run_clock = None;
        self.model.character.run_result = None;
        self.blend_from = None;
        // Player two goes with player one.
        self.player2 = None;
        self.versus = None;
        self.model.split_huds = None;
        if let Some(level) = &mut self.level {
            level.renderer.set_character2(None);
            level.renderer.split = None;
        }
        if self.skating.take().is_some() {
            self.model.character.skating = false;
            self.sparks.clear();
            self.rumble.stop();
            if let Some(audio) = &mut self.audio {
                audio.stop();
            }
            self.model.character.playing = true;
            return;
        }
        let (Some(level), Some(index)) = (&self.level, self.model.character.current) else {
            return;
        };
        if level.world.is_none() {
            return;
        }
        let id = &self.model.character.characters[index].id;
        let program = level.behaviour.program();
        let stats = Stats::of(program, id);
        // The Stats 13 cheat: every stat at 13.
        let stats = if self.model.character.cheats.stats_13 {
            Stats([13.0; 10])
        } else {
            stats
        };
        // The Moon Gravity and Slomo cheats' factors (`Moon_gravity`,
        // `slomo_speed`).
        let factor = |name: &str, default: f32| {
            program
                .value(qb::checksum(name))
                .and_then(qb::Value::as_f32)
                .unwrap_or(default)
        };
        self.moon_gravity = factor("Moon_gravity", 0.5);
        self.slomo_speed = factor("slomo_speed", 0.5);
        let mut physics = Physics::new(program, &stats);
        // The chase camera picked.
        let choices = Physics::camera_choices(program);
        self.model.character.cameras = choices.iter().map(|(name, _)| title_case(name)).collect();
        if let Some((_, setting)) = choices.get(self.model.character.camera) {
            physics.set_camera(program, *setting);
        }
        self.camera_shown = self.model.character.camera;
        let position = self.placement.transform_point3(Vec3::ZERO);
        let forward = self.placement.transform_vector3(Vec3::Z);
        let mut skater = Skater::new(position, forward.x.atan2(forward.z));
        // (`DESA_SKATE_TRACE` set: why it leaves rails, and the like.)
        skater.trace = std::env::var_os("DESA_SKATE_TRACE").is_some();
        // The character's tricks, timed by its animations.
        let mut tricks =
            TrickBook::with_controls(program, id, &stats, self.model.character.simplified);
        skater.simplified = self.model.character.simplified;
        if let Some(character) = &self.character {
            // And its animations' lengths, to play pushes, landings and
            // bails through.
            skater.anim_lengths = character
                .animations
                .iter()
                .map(|(name, a)| (name.clone(), a.duration))
                .collect();
            tricks.set_durations(|anim| {
                character
                    .animations
                    .iter()
                    .find(|(name, _)| qb::checksum(name) == anim)
                    .map(|(_, a)| a.duration)
            });
        }
        self.model.character.trick_list = tricks.trick_list();
        skater.tricks = tricks;
        // Standing on the ground to start, not dropped onto it.
        if let Some(world) = &level.world {
            let heading = skater.heading;
            skater.place(position, heading, &physics, world);
        }
        // Each run starts with the character's `StartLevel` (`warpIn`),
        // but in the Skate Shop.
        if !level.id.eq_ignore_ascii_case("SkateShop") {
            let kid = id == "kid";
            skater.start_level(kid);
            // (And the Kid arrives at `TRG_Start_P1`, `warpIn`'s
            // `Obj_MoveToNode`, when starting from the level's start: on the
            // Hub and the Graveyard not the first `Player1` restart.)
            let start = level
                .nodes
                .spawns
                .iter()
                .find(|s| s.name == qb::checksum("TRG_Start_P1"));
            if let (true, true, Some(start), Some(world)) =
                (kid, self.placement == level.home, start, &level.world)
            {
                let facing = start.facing();
                skater.place(start.position, facing.x.atan2(facing.z), &physics, world);
                skater.start_level(kid);
            }
        }
        // Falling out of the level puts the skater back at the nearest
        // spawn point.
        skater.spawns = level
            .nodes
            .spawns
            .iter()
            .map(|spawn| {
                let facing = spawn.facing();
                (spawn.position, facing.x.atan2(facing.z))
            })
            .collect();
        // Teleporters: trigger faces whose scripts send the skater to a
        // restart.
        skater.teleports = desa_viewer::triggers::teleports(&level.nodes, program)
            .into_iter()
            .map(|(object, i)| {
                let spawn = &level.nodes.spawns[i];
                let facing = spawn.facing();
                (object, (spawn.position, facing.x.atan2(facing.z)))
            })
            .collect();
        skater.gap_triggers = desa_viewer::triggers::gaps(&level.nodes, program);
        // Trick spots asking for a slot's trick ask for this character's.
        let book = &skater.tricks;
        skate::name_slots(&mut skater.gap_triggers, |slot| {
            book.slot_trick(slot).map(|i| book.tricks[i].name.clone())
        });
        // The level's gaps, as a checklist of those landed before.
        let landed = self.settings.gaps.get(&level.id);
        let mut gap_list: Vec<(String, u32, bool)> = Vec::new();
        for trigger in skater.gap_triggers.values().flatten() {
            if let skate::gaps::GapTrigger::End { text, score, .. } = trigger {
                // (A goal's own gaps have no name.)
                if text.is_empty() {
                    continue;
                }
                if !gap_list.iter().any(|g| &g.0 == text) {
                    let got = landed.is_some_and(|l| l.contains(text));
                    gap_list.push((text.clone(), *score, got));
                }
            }
        }
        gap_list.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
        // (And for the game's own gap checklist.)
        if let Some(screen) = &mut self.screen {
            screen.screen.gaps = gap_list
                .iter()
                .map(|(name, score, got)| (name.clone(), *score, u32::from(*got)))
                .collect();
        }
        self.model.character.gap_list = gap_list;
        self.start_audio();
        self.stop_camera_path();
        let chase = ChaseCamera::behind(&skater, &physics);
        self.skating = Some((skater, physics, chase));
        self.start_collecting();
        self.combo_lengths = [0.0; 3];
        self.running_lengths = [0.0; 3];
        self.combos_seen = 0;
        self.record_queue.clear();
        self.model.character.record_message = None;
        self.show_records();
        self.model.character.skating = true;
        self.model.character.playing = false;
        self.set_looking(false);
        self.spawn_player2();
    }

    /// Starts a two-minute run (`StartGoal_TrickAttack time = 120`) from
    /// where the character stands at the level's start.
    ///
    /// With `Some(pro)`, the level's High Score or Pro Score goal
    /// (`AddGoal_HighScore`, `AddGoal_ProScore`): its score to reach in its
    /// time, from its restart node.
    fn start_run(&mut self, goal: Option<bool>) {
        if self.skating.is_some() {
            self.toggle_skate();
        }
        let Some(level) = &self.level else { return };
        let target = goal.and_then(|pro| {
            desa_viewer::goals::score_goal(level.behaviour.program(), &level.id, pro)
        });
        if goal.is_some() && target.is_none() {
            return;
        }
        self.placement = level.home;
        let start = target
            .as_ref()
            .and_then(|t| t.restart)
            .and_then(|name| level.nodes.nodes.iter().find(|n| n.name == name))
            .and_then(|n| n.position);
        if let Some(start) = start {
            let (_, rotation, _) = level.home.to_scale_rotation_translation();
            self.placement = Mat4::from_rotation_translation(rotation, start);
        }
        self.toggle_skate();
        if self.skating.is_some() {
            self.recording.clear();
            self.model.character.run_goal = goal
                .zip(target.as_ref())
                .map(|(pro, t)| (pro, t.name.clone(), t.score));
            self.model.character.run_goal_won = target.as_ref().map(|_| false);
            if let Some(pro) = goal {
                let kind = if pro { "ProScore" } else { "HighScore" };
                self.goal_cutscenes(kind);
                self.queue_objective(kind);
            }
            self.run = Some(Run {
                left: target.as_ref().map_or(RUN_TIME, |t| t.time),
                ending: None,
                over: false,
                goal: target.map(|t| (t.score, t.win)),
                won: false,
            });
        }
    }

    /// The character's collectibles on this level (`AddGoal_Collect25`),
    /// those not got yet put out spinning and hovering
    /// (`create_goal_disney_collect_object`).
    fn start_collecting(&mut self) {
        self.collecting = None;
        self.model.character.collected = None;
        if !self.model.character.collect {
            return;
        }
        let (Some(level), Some(index)) = (&mut self.level, self.model.character.current) else {
            return;
        };
        let character = self.model.character.characters[index].id.clone();
        let Some(list) = desa_viewer::goals::collectibles(level.behaviour.program(), &character)
        else {
            return;
        };
        let key = format!("collected.{}.{character}", level.id);
        let got = self.settings.best.get(&key).copied().unwrap_or(0);
        let objects: Vec<(usize, u32)> = list
            .objects
            .iter()
            .enumerate()
            .filter_map(|(i, name)| Some((level.behaviour.object(*name)?, 1 << i)))
            .collect();
        if objects.is_empty() {
            return;
        }
        // The special item, after the 25 (its own bit, not counted with them).
        let special = list
            .special
            .as_ref()
            .and_then(|(name, title)| Some((level.behaviour.object(*name)?, title.clone())));
        if let Some((object, _)) = &special {
            if got & SPECIAL_BIT == 0 {
                level.behaviour.set_alive(*object, true);
                level.behaviour.set_spin(*object, COLLECT_SPIN.to_radians());
                level.behaviour.set_hover(*object, 10.0, 1.0);
            }
        }
        for &(object, bit) in &objects {
            if got & bit == 0 {
                level.behaviour.set_alive(object, true);
                level.behaviour.set_spin(object, COLLECT_SPIN.to_radians());
                level.behaviour.set_hover(object, 10.0, 1.0);
            }
        }
        self.model.character.collected = Some((
            objects.iter().filter(|(_, bit)| got & bit != 0).count() as u32,
            objects.len() as u32,
        ));
        self.collecting = Some(Collecting {
            objects,
            special,
            got,
            kind: list.kind,
            key,
        });
    }

    /// Picks up the collectibles the skater reaches
    /// (`set_goal_collect_exception_25`: within 7 feet), with the gap sound
    /// and "3 of 25 Cowgirl Boots" (`goal_collect_got_object`), and keeps
    /// what's got.
    fn update_collecting(&mut self) {
        if self.replay.is_some() {
            return;
        }
        let (Some(collecting), Some((skater, ..)), Some(level)) =
            (&mut self.collecting, &mut self.skating, &mut self.level)
        else {
            return;
        };
        let body = skater.position + Vec3::Y * 30.0;
        if let Some((object, title)) = &collecting.special {
            if collecting.got & SPECIAL_BIT == 0
                && body.distance(level.behaviour.position(*object)) <= COLLECT_RADIUS
            {
                collecting.got |= SPECIAL_BIT;
                level.behaviour.set_alive(*object, false);
                if let Some(audio) = self.audio.as_ref().filter(|_| self.model.character.sound) {
                    audio.play_named(qb::checksum("GoalDone"), 1.0);
                }
                skater.message = Some((format!("Got the {title}!"), 2.5));
                self.settings
                    .best
                    .insert(collecting.key.clone(), collecting.got);
                self.settings.save();
            }
        }
        let mut changed = false;
        for &(object, bit) in &collecting.objects {
            if collecting.got & bit != 0
                || body.distance(level.behaviour.position(object)) > COLLECT_RADIUS
            {
                continue;
            }
            collecting.got |= bit;
            changed = true;
            level.behaviour.set_alive(object, false);
            if let Some(audio) = self.audio.as_ref().filter(|_| self.model.character.sound) {
                audio.play_named(qb::checksum("gapsound"), 1.0);
            }
        }
        if !changed {
            return;
        }
        let got = collecting
            .objects
            .iter()
            .filter(|(_, bit)| collecting.got & bit != 0)
            .count();
        let of = collecting.objects.len();
        skater.message = Some((
            if got == of {
                format!("All {of} {} collected!", collecting.kind)
            } else {
                format!("{got} of {of} {}", collecting.kind)
            },
            2.0,
        ));
        self.model.character.collected = Some((got as u32, of as u32));
        self.settings
            .best
            .insert(collecting.key.clone(), collecting.got);
        self.settings.save();
    }

    /// A car the skater's started or stopped skitching on: off at its
    /// skitch speed, or back to its own script.
    fn update_skitch(&mut self) {
        let now = self
            .skating
            .as_ref()
            .and_then(|(s, ..)| s.skitch)
            .and_then(|i| self.vehicle_objects.get(i).copied());
        if now == self.skitched {
            return;
        }
        if let Some(level) = &mut self.level {
            if let Some(old) = self.skitched {
                level.behaviour.unskitch(old);
            }
            if let Some(car) = now {
                level.behaviour.skitch(car);
            }
        }
        self.skitched = now;
    }

    /// The settings key for one of the level's records for the character.
    fn record_key(&self, kind: &str) -> String {
        let character = self
            .model
            .character
            .current
            .map_or("", |i| self.model.character.characters[i].id.as_str());
        let level = self.level.as_ref().map_or("", |l| l.id.as_str());
        format!("record.{kind}.{level}.{character}")
    }

    /// The records in the panel.
    fn show_records(&mut self) {
        let get = |app: &Self, kind: &str| app.settings.best.get(&app.record_key(kind)).copied();
        let mut lines = Vec::new();
        for (kind, label) in RECORDS {
            if let Some(value) = get(self, kind) {
                let value = if kind == "tricks" {
                    value.to_string()
                } else {
                    record_value(kind, value)
                };
                lines.push(format!("{label}: {value}"));
            }
        }
        self.model.character.records = lines;
    }

    /// The game's records (`CheckAndDisplayRecordScore`, after each combo
    /// lands): the best combo score, the longest grind, manual and lip
    /// trick, and the most tricks in a combo, each kept for the level and
    /// character. A new one is announced ("Record Combo Score!", then the
    /// value) when it's past the game's showing mark (10,000 points, 10
    /// seconds, 5 tricks), one message after another with the gap sound.
    fn update_records(&mut self, dt: f32) {
        // The announcement showing, then the next.
        self.record_shown += dt;
        if self.model.character.record_message.is_some() && self.record_shown > RECORD_TIME {
            self.model.character.record_message = None;
        }
        if self.model.character.record_message.is_none() {
            if let Some(text) = self.record_queue.pop_front() {
                self.model.character.record_message = Some(text);
                self.record_shown = 0.0;
                if let Some(audio) = self.audio.as_ref().filter(|_| self.model.character.sound) {
                    audio.play_named(qb::checksum("gapsound"), 1.0);
                }
            }
        }
        if self.replay.is_some() {
            return;
        }
        let Some((skater, ..)) = &self.skating else {
            return;
        };
        // The session's tally.
        let session = &mut self.session;
        session.time += dt;
        if let Some(last) = session.last_position {
            let step = skater.position.distance(last);
            // (Not a jump to a spawn point or through a teleporter.)
            if step < 100.0 {
                session.distance += step;
            }
        }
        session.last_position = Some(skater.position);
        self.model.character.session = session.lines();
        // How long each balance trick has gone on, the longest kept.
        let now = [skater.grind.is_some(), skater.manual, skater.lip.is_some()];
        for ((running, longest), on) in self
            .running_lengths
            .iter_mut()
            .zip(&mut self.combo_lengths)
            .zip(now)
        {
            *running = if on { *running + dt } else { 0.0 };
            *longest = longest.max(*running);
        }
        if skater.combos_ended == self.combos_seen {
            return;
        }
        self.combos_seen = skater.combos_ended;
        if let Some(landed) = &skater.last_combo {
            let session = &mut self.session;
            if landed.bailed {
                session.bails += 1;
            } else {
                session.combos += 1;
                session.tricks += landed.combo.tricks.len() as u32;
                session.best = session.best.max(landed.total);
                session.points += landed.total;
            }
            self.model.character.session = session.lines();
        }
        let lengths = std::mem::take(&mut self.combo_lengths);
        let Some(landed) = skater.last_combo.as_ref().filter(|l| !l.bailed) else {
            return;
        };
        let values = [
            ("combo", landed.total),
            ("grind", (lengths[0] * 100.0) as u32),
            ("manual", (lengths[1] * 100.0) as u32),
            ("lip", (lengths[2] * 100.0) as u32),
            ("tricks", landed.combo.tricks.len() as u32),
        ];
        let mut changed = false;
        for (kind, value) in values {
            let key = self.record_key(kind);
            if value == 0 || self.settings.best.get(&key).is_some_and(|b| *b >= value) {
                continue;
            }
            self.settings.best.insert(key, value);
            changed = true;
            let (text, mark) = match kind {
                "combo" => ("Record Combo Score!", 10_000),
                "grind" => ("Record Grind Length!", 1000),
                "manual" => ("Record Manual Length!", 1000),
                "lip" => ("Record Liptrick Length!", 1000),
                _ => ("Record Trick Combo!", 5),
            };
            if value >= mark {
                self.record_queue.push_back(text.to_string());
                self.record_queue
                    .push_back(format!("{}!", record_value(kind, value)));
            }
        }
        if changed {
            self.settings.save();
            self.show_records();
        }
    }

    /// The level's warps: a Hub portal animates into place as the skater
    /// comes within 60 feet (`WarpAppears`: dropping and turning, with the
    /// portal sound); going into one (or the way back to the Hub) holds the
    /// skater and offers the level.
    fn update_portals(&mut self, dt: f32) {
        let skater = self.skating.as_ref().map(|(s, ..)| s.position);
        let Some(level) = &mut self.level else { return };
        for (i, portal) in level.portals.iter_mut().enumerate() {
            let Some(skater) = skater else {
                // Not skating: put away again.
                if portal.appeared.take().is_some() {
                    if let Some((layer, _)) = &portal.strip {
                        level.renderer.show_layer(*layer, false);
                    }
                }
                portal.declined = false;
                continue;
            };
            let to = skater - portal.warp.position;
            if let Some((layer, base)) = &portal.strip {
                if portal.appeared.is_none() && to.length() < PORTAL_APPEAR {
                    portal.appeared = Some(0.0);
                    level.renderer.show_layer(*layer, true);
                    // Its sparkle (`create Name = <warpParticle>`).
                    if let Some(particle) = portal.warp.particle {
                        level
                            .particles
                            .start(level.behaviour.program(), &level.nodes, particle);
                    }
                    if let Some(audio) = self.audio.as_ref().filter(|_| self.model.character.sound)
                    {
                        audio.play_named(qb::checksum("portalAppears"), 1.0);
                    }
                }
                if let Some(t) = &mut portal.appeared {
                    if *t <= PORTAL_TIME {
                        *t += dt;
                        let k = (*t / PORTAL_TIME).min(1.0);
                        // Quick to start, easing into place.
                        let k = 1.0 - (1.0 - k) * (1.0 - k);
                        let middle = base
                            .iter()
                            .fold(Vec3::ZERO, |sum, v| sum + Vec3::from(v.position))
                            / base.len().max(1) as f32;
                        let moved: Vec<_> = base
                            .iter()
                            .map(|v| desa_viewer::level::Vertex {
                                position: (middle + (Vec3::from(v.position) - middle) * k)
                                    .to_array(),
                                ..*v
                            })
                            .collect();
                        level.renderer.update_layer(*layer, 0, &moved);
                    }
                }
            }
            // The way back to the Hub glows (`HubWarp`: `create Name =
            // TRG_Warp_Particle_Hub`).
            if portal.strip.is_none() {
                if let Some(particle) = portal.warp.particle {
                    level
                        .particles
                        .start(level.behaviour.program(), &level.nodes, particle);
                }
            }
            let to = skater - portal.warp.position;
            let across = Vec3::new(to.x, 0.0, to.z).length();
            if portal.declined {
                portal.declined = across < WARP_LEAVE;
                continue;
            }
            let ready = portal.strip.is_none() || portal.appeared.is_some_and(|t| t > PORTAL_TIME);
            // (Not for two players: the warps are the career's.)
            if ready
                && self.player2.is_none()
                && self.warp_offer.is_none()
                && across < WARP_ENTER
                && to.y.abs() < WARP_HEIGHT
            {
                self.warp_offer = Some(i);
                self.model.character.warp_prompt = Some(portal.warp.title.clone());
            }
        }
    }

    /// What the map in the corner shows round the skater: the collectibles
    /// and letters still to get, and the warps.
    fn update_map(&mut self) {
        let (Some((skater, ..)), Some(level)) = (&self.skating, &self.level) else {
            self.model.character.map = None;
            return;
        };
        let Some(map) = &level.minimap else { return };
        let mut markers = Vec::new();
        if let Some(c) = &self.collecting {
            for &(object, bit) in &c.objects {
                if c.got & bit == 0 {
                    markers.push((
                        map.pixel(level.behaviour.position(object)),
                        ui::MapMark::Collectible,
                    ));
                }
            }
        }
        // What the goal on wants gone near.
        if let Some(run) = self.goal_run.as_ref().filter(|r| !r.over) {
            for object in level.behaviour.radius_trigger_objects() {
                if !run.before.contains(&object) {
                    markers.push((
                        map.pixel(level.behaviour.position(object)),
                        ui::MapMark::Target,
                    ));
                }
            }
        }
        if let Some(race) = self.racing.as_ref().filter(|r| !r.over) {
            if let Some((at, ..)) = race.points.get(race.next) {
                markers.push((map.pixel(*at), ui::MapMark::Letter));
            }
        }
        if let Some(run) = &self.letters {
            for (i, &object) in run.objects.iter().enumerate() {
                if !run.got[i] {
                    markers.push((
                        map.pixel(level.behaviour.position(object)),
                        ui::MapMark::Letter,
                    ));
                }
            }
        }
        for portal in &level.portals {
            markers.push((map.pixel(portal.warp.position), ui::MapMark::Warp));
        }
        for pro in &level.pros {
            markers.push((
                map.pixel(level.behaviour.position(pro.object)),
                ui::MapMark::Pro,
            ));
        }
        // The gaps' ends (where each scores), landed or not.
        let ends = skater
            .gap_triggers
            .iter()
            .flat_map(|(object, triggers)| triggers.iter().map(move |t| (object, t)));
        for (object, trigger) in ends {
            let skate::gaps::GapTrigger::End { text, .. } = trigger else {
                continue;
            };
            if text.is_empty() {
                continue;
            }
            let Some(at) = level
                .nodes
                .nodes
                .iter()
                .find(|n| n.name == *object)
                .and_then(|n| n.position)
            else {
                continue;
            };
            let landed = self
                .model
                .character
                .gap_list
                .iter()
                .any(|(name, _, got)| *got && name == text);
            markers.insert(0, (map.pixel(at), ui::MapMark::Gap(landed)));
        }
        self.model.character.map = Some(ui::MapView {
            centre: map.pixel(skater.position),
            heading: skater.heading,
            size: (map.width as f32, map.height as f32),
            markers,
        });
    }

    /// The goals' pros: standing in the level while skating (with no goal
    /// on), and offering their goal when the skater rolls up.
    fn update_pros(&mut self) {
        // (Two players' free skate has no goals to offer.)
        if self.player2.is_some() {
            return;
        }
        let skating = self.skating.as_ref().map(|(s, ..)| s.position);
        // (Nor while the level's start plays: `warpIn` has the skater's
        // exceptions off.)
        let starting = self.skating.as_ref().is_some_and(|(s, ..)| {
            matches!(s.action, skate::Action::StartLevel | skate::Action::WarpIn)
        });
        let busy = self.run.is_some()
            || self.letters.is_some()
            || self.racing.is_some()
            || self.goal_run.is_some()
            || starting;
        // The pad's B at a pro: their goal (`pad_circle_script =
        // goal_accept_trigger`).
        if std::mem::take(&mut self.talk_pressed) && self.goal_offer.is_some() {
            self.take_goal();
            return;
        }
        let on_ground = self.skating.as_ref().is_some_and(|(s, ..)| s.on_ground);
        let Some(level) = &mut self.level else { return };
        // Rolled off out of reach: the offer's taken back
        // (`goal_trigger_refuse`), to come again next time.
        if let (Some(i), Some(at)) = (self.goal_offer, skating) {
            let far = level
                .pros
                .get(i)
                .is_none_or(|p| at.distance(level.behaviour.position(p.object)) > PRO_LEAVE);
            if far {
                self.goal_offer = None;
                self.model.character.goal_prompt = None;
            }
        }
        for (i, pro) in level.pros.iter_mut().enumerate() {
            let Some(at) = skating else {
                pro.declined = false;
                continue;
            };
            let near = at.distance(level.behaviour.position(pro.object));
            if pro.declined {
                pro.declined = near < PRO_LEAVE;
                continue;
            }
            if !busy
                && on_ground
                && self.goal_offer.is_none()
                && self.warp_offer.is_none()
                && near < PRO_NEAR
            {
                self.goal_offer = Some(i);
                self.model.character.goal_prompt = Some(pro.title.clone());
                // They turn to the skater (`Obj_LookAtObject Type = skater`).
                level.behaviour.look_at(pro.object, at, 0.4);
                self.lines_to_say.extend(pro.line);
            }
        }
    }

    /// Starts the goal offered.
    fn take_goal(&mut self) {
        let Some(i) = self.goal_offer.take() else {
            return;
        };
        self.model.character.goal_prompt = None;
        let Some(action) = self
            .level
            .as_ref()
            .and_then(|l| l.pros.get(i))
            .map(|p| p.start)
        else {
            return;
        };
        match action {
            ui::Action::StartRun(goal) => self.start_run(goal),
            ui::Action::StartLetters => self.start_letters(),
            ui::Action::StartRace => self.start_race(),
            ui::Action::StartGoal(i) => self.start_goal(i),
            _ => {}
        }
    }

    /// Turns the goal down: not offered again till the skater's gone off.
    fn not_now(&mut self) {
        if let Some(i) = self.goal_offer.take() {
            if let Some(pro) = self.level.as_mut().and_then(|l| l.pros.get_mut(i)) {
                pro.declined = true;
            }
        }
        self.model.character.goal_prompt = None;
    }

    /// Goes through the warp offered: that level loads, and skating goes on
    /// there from its start.
    fn take_warp(&mut self) {
        let Some(i) = self.warp_offer.take() else {
            return;
        };
        self.model.character.warp_prompt = None;
        let Some((level, camera)) = self
            .level
            .as_ref()
            .and_then(|l| l.portals.get(i))
            .map(|p| (p.warp.level, p.warp.camera))
        else {
            return;
        };
        let index = self
            .model
            .levels
            .iter()
            .position(|info| qb::checksum(&format!("load_{}", info.id)) == level);
        if let Some(index) = index {
            self.skate_on_load = true;
            // (The skater spins away at the warp while its camera plays.)
            let at = self
                .level
                .as_ref()
                .and_then(|l| l.portals.get(i))
                .map(|p| p.warp.position);
            self.warp_spin = at.map(|at| (at, 0.0, self.placement));
            // (`Warp_Master:midgoalvoiceover stream = warp_build`.)
            if let Some(level) = &mut self.level {
                level.behaviour.voice_lines.push(qb::checksum("warp_build"));
            }
            // The warp's own camera path first, then the level.
            match camera {
                Some(camera) => {
                    self.play_cutscene(camera);
                    if self.cutscene {
                        self.load_after_cutscene = Some(index);
                    } else {
                        self.start_load(index);
                    }
                }
                None => self.start_load(index),
            }
        }
    }

    /// Turns the warp down: skating on, not offered it again until the
    /// skater's gone off from it.
    fn stay_here(&mut self) {
        if let Some(i) = self.warp_offer.take() {
            if let Some(portal) = self.level.as_mut().and_then(|l| l.portals.get_mut(i)) {
                portal.declined = true;
            }
        }
        self.model.character.warp_prompt = None;
    }

    /// Knocks the bouncy objects the skater runs into flying (up by their
    /// `UpMagnitude`, spinning at `ConstRot`), falling with their
    /// `Gravity` and bouncing (`Bounciness`) till they settle
    /// (`MinBounceVel`), with their `BounceSound`.
    fn update_bouncies(&mut self, dt: f32) {
        let Some(level) = &mut self.level else { return };
        let skater = self
            .skating
            .as_ref()
            .map(|(s, ..)| (s.position + Vec3::Y * 20.0, s.velocity));
        let mut seed = (self.session.time * 1000.0) as u32 | 1;
        let mut random = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5
        };
        let mut scripts = Vec::new();
        for b in &mut level.bouncies {
            if !b.shown {
                continue;
            }
            b.since += dt;
            let at = b.centre + b.offset;
            if let Some((body, velocity)) = skater {
                let near = (body - at).with_y(0.0).length() < b.reach + 14.0
                    && (body.y - at.y).abs() < b.half_height + 40.0;
                if near && b.since > 0.4 && velocity.length() > 60.0 {
                    b.since = 0.0;
                    b.moving = true;
                    // Its script, the first knock.
                    if !std::mem::replace(&mut b.collided, true) {
                        scripts.extend(b.spec.collide_script.map(|s| (b.spec.name, s)));
                    }
                    b.velocity = velocity.with_y(0.0) * 1.1 + Vec3::Y * b.spec.up * FEET;
                    b.spin = Vec3::new(random(), random(), random()).normalize_or(Vec3::X)
                        * b.spec.spin.to_radians();
                    if let (Some(sound), Some(audio)) = (
                        b.spec.sound,
                        self.audio.as_ref().filter(|_| self.model.character.sound),
                    ) {
                        audio.play_named(sound, 1.0);
                    }
                }
            }
            if !b.moving {
                continue;
            }
            b.velocity.y -= b.spec.gravity * FEET * dt;
            b.offset += b.velocity * dt;
            b.rotation = (Quat::from_scaled_axis(b.spin * dt) * b.rotation).normalize();
            // The ground under it: a bounce, losing speed, till it rests.
            let at = b.centre + b.offset;
            if let Some(world) = &level.world {
                let from = at + Vec3::Y * (b.half_height + 20.0);
                if let Some(hit) = world.ray(from, at - Vec3::Y * (b.half_height + 4.0)) {
                    if b.velocity.y < 0.0 && at.y - b.half_height <= hit.point.y {
                        b.offset.y += hit.point.y - (at.y - b.half_height);
                        b.velocity.y = -b.velocity.y * b.spec.bounciness.clamp(0.0, 1.0) * 0.6;
                        b.velocity.x *= 0.7;
                        b.velocity.z *= 0.7;
                        b.spin *= 0.6;
                        if b.velocity.length() < (b.spec.min_bounce * FEET).max(30.0) {
                            b.moving = false;
                            b.velocity = Vec3::ZERO;
                        }
                    }
                }
            }
            // Far below the level: put back where it was.
            if b.offset.y < -5000.0 {
                b.offset = Vec3::ZERO;
                b.rotation = Quat::IDENTITY;
                b.moving = false;
            }
            let moved: Vec<_> = b
                .base
                .iter()
                .map(|v| desa_viewer::level::Vertex {
                    position: (b.centre
                        + b.rotation * (Vec3::from(v.position) - b.centre)
                        + b.offset)
                        .to_array(),
                    normal: (b.rotation * Vec3::from(v.normal)).to_array(),
                    ..*v
                })
                .collect();
            level.renderer.update_layer(b.layer, 0, &moved);
        }
        for (name, script) in scripts {
            level.behaviour.run_object_script(name, script);
        }
    }

    /// Breaks what the skater's touched (the trigger scripts' `Shatter`):
    /// the pieces gone (and their collision), chunks thrown, and the sound
    /// the script plays.
    fn update_breakables(&mut self) {
        let (Some((skater, ..)), Some(level)) = (&self.skating, &mut self.level) else {
            return;
        };
        // What teleporters created or killed (the grocery store
        // restocked): created breakables can break again.
        for (target, created) in std::mem::take(&mut self.pending_creates) {
            let names: Vec<u32> = match target {
                desa_viewer::triggers::CreateTarget::Name(n) => vec![n],
                desa_viewer::triggers::CreateTarget::Prefix(p) => {
                    let p = p.to_ascii_lowercase();
                    level
                        .nodes
                        .labels
                        .iter()
                        .filter(|(_, l)| l.to_ascii_lowercase().starts_with(&p))
                        .map(|(n, _)| *n)
                        .collect()
                }
            };
            for name in names {
                if let Some(object) = level.behaviour.object(name) {
                    level.behaviour.set_alive(object, created);
                } else if level.nodes.hidden_sectors.contains(&name)
                    || level.sector_layers.contains_key(&name)
                {
                    level.show_sector(name, created);
                }
                if created {
                    if let Some(world) = &mut level.world {
                        world.enable(name);
                    }
                    let mended: Vec<u32> = level
                        .breakables
                        .iter()
                        .filter(|(_, (_, names, _))| names.contains(&name))
                        .map(|(t, _)| *t)
                        .collect();
                    for trigger in mended {
                        level.broken.remove(&trigger);
                        if let Some(world) = &mut level.world {
                            world.enable(trigger);
                        }
                    }
                }
            }
        }
        // What the scripts shattered (a door, a pane): gone, in pieces.
        for name in std::mem::take(&mut level.behaviour.shatters) {
            let at = if let Some(object) = level.behaviour.object(name) {
                level.behaviour.set_alive(object, false);
                Some(level.behaviour.position(object))
            } else if let Some(layer) = level.sector_layers.get(&name) {
                level.renderer.show_layer(*layer, false);
                level.sector_centres.get(&name).copied()
            } else {
                if level.nodes.hidden_sectors.contains(&name) {
                    level.show_sector(name, false);
                }
                level.sector_centres.get(&name).copied()
            };
            if let Some(world) = &mut level.world {
                world.disable(name);
            }
            if let Some(at) = at {
                self.sparks.shatter(at);
            }
        }
        for trigger in &skater.touched {
            let Some((_, names, sound)) = level.breakables.get(trigger) else {
                continue;
            };
            if !level.broken.insert(*trigger) {
                continue;
            }
            let (names, sound) = (names.clone(), *sound);
            for name in names {
                let at = if let Some(object) = level.behaviour.object(name) {
                    level.behaviour.set_alive(object, false);
                    Some(level.behaviour.position(object))
                } else {
                    if let Some(layer) = level.sector_layers.get(&name) {
                        level.renderer.show_layer(*layer, false);
                    }
                    level.sector_centres.get(&name).copied()
                };
                if let Some(world) = &mut level.world {
                    world.disable(name);
                }
                if let Some(at) = at {
                    self.sparks.shatter(at);
                }
            }
            if let Some(world) = &mut level.world {
                world.disable(*trigger);
            }
            if let (Some(sound), Some(audio)) = (
                sound,
                self.audio.as_ref().filter(|_| self.model.character.sound),
            ) {
                audio.play_named(sound, 1.0);
            }
        }
    }

    /// Starts the level's race (`AddGoal_Race`) from its restart node: the
    /// goal's start script runs (the racing cars come out), then the first
    /// waypoint's script (its gate), with the first waypoint's time on the
    /// clock.
    fn start_race(&mut self) {
        if self.skating.is_some() {
            self.toggle_skate();
        }
        let Some(level) = &self.level else { return };
        let Some(race) = desa_viewer::goals::race(level.behaviour.program(), &level.id) else {
            return;
        };
        let at = |name: u32| {
            level
                .nodes
                .nodes
                .iter()
                .find(|n| n.name == name)
                .and_then(|n| n.position)
        };
        let points: Vec<(Vec3, Option<u32>, f32)> = race
            .waypoints
            .iter()
            .filter_map(|(name, script, time)| Some((at(*name)?, *script, *time)))
            .collect();
        let Some(first) = points.first() else { return };
        self.placement = level.home;
        if let Some(start) = race.restart.and_then(at) {
            let to = first.0 - start;
            self.placement =
                Mat4::from_rotation_translation(Quat::from_rotation_y(to.x.atan2(to.z)), start);
        }
        // Scripts run on objects that are there all along (they only make
        // and kill things and play sounds): the waypoints' on one, the
        // start and end scripts on another, so neither cuts the other off.
        let mut alive = (0..level.nodes.objects.len()).filter(|&i| level.behaviour.alive(i));
        let runner = alive.next().unwrap_or(0);
        let goal_runner = alive.next().unwrap_or(runner);
        self.toggle_skate();
        if self.skating.is_none() {
            return;
        }
        let Some(level) = &mut self.level else { return };
        if let Some(script) = race.start_script {
            level.behaviour.run_script(goal_runner, script);
        }
        let left = first.2;
        let first_script = first.1;
        self.queue_objective("Race");
        self.racing = Some(RaceRun {
            points,
            next: 0,
            left,
            time: 0.0,
            over: false,
            runner,
            goal_runner,
            end_script: race.end_script,
            started: false,
        });
        let _ = first_script;
        self.model.character.race_name = Some(race.name);
        self.goal_cutscenes("Race");
    }

    /// Counts the race down and takes the waypoints the skater reaches
    /// (`goal_race_init_waypoint`: within 8 feet): each runs the next one's
    /// script and adds its time. All reached wins; out of time loses.
    fn update_race(&mut self, dt: f32) {
        let (Some(race), Some((skater, ..)), Some(level)) =
            (&mut self.racing, &self.skating, &mut self.level)
        else {
            return;
        };
        let model = &mut self.model.character;
        if !race.started {
            // The first waypoint's script (its gate and arrow).
            race.started = true;
            if let Some(script) = race.points[0].1 {
                level.behaviour.run_script(race.runner, script);
            }
        }
        model.run_clock = Some(race.left);
        model.race = Some((race.next, race.points.len()));
        if race.over {
            return;
        }
        race.left = (race.left - dt).max(0.0);
        race.time += dt;
        let body = skater.position + Vec3::Y * 30.0;
        if body.distance(race.points[race.next].0) < RACE_RADIUS {
            race.next += 1;
            if let Some(audio) = self.audio.as_ref().filter(|_| model.sound) {
                audio.play_named(qb::checksum("hud_jumpgap"), 1.0);
            }
            if let Some((_, script, time)) = race.points.get(race.next) {
                race.left += time;
                if let Some(script) = script {
                    level.behaviour.run_script(race.runner, *script);
                }
            }
        }
        let won = race.next >= race.points.len();
        if won {
            self.cutscene_request = self.success_camera.take();
            self.goal_won = Some("Race".to_string());
        }
        if won || race.left == 0.0 {
            race.over = true;
            model.race_result = Some((won, race.time));
            self.goal_end = Some((won, GoalEnded::Kind("Race")));
            if won {
                if let Some(audio) = self.audio.as_ref().filter(|_| model.sound) {
                    audio.play_named(qb::checksum("GoalDone"), 1.0);
                }
            }
            if let Some(script) = race.end_script.take() {
                level.behaviour.run_script(race.goal_runner, script);
            }
        }
    }

    /// Starts one of the level's goals (by its place in the goal list) as
    /// its own scripts play it: the skater at its restart node, its
    /// activate and start scripts run (`goal_ID` its id), and the goal
    /// manager told it's on. The scripts set its flags as they're done
    /// (a gap goal's gaps' `Gapscript`s); enough of them, or a script's
    /// `GoalManager_WinGoal`, wins it.
    /// The objective of the level's goal of `kind` to pop up as it starts:
    /// its `goal_text` (else its name).
    fn queue_objective(&mut self, kind: &str) {
        let quick = std::mem::take(&mut self.quick_start);
        let Some(level) = &self.level else { return };
        let program = level.behaviour.program();
        let description = (!quick)
            .then(|| description_text(program, &level.id, kind))
            .flatten();
        self.goal_popup = desa_viewer::goals::generic_goal(program, &level.id, kind)
            .map(|g| (objective(&g.name, &g.text), 2, description));
    }

    fn start_goal(&mut self, index: usize) {
        if self.skating.is_some() {
            self.toggle_skate();
        }
        let Some(level) = &self.level else { return };
        let Some((kind, _)) = self.model.character.goals.get(index) else {
            return;
        };
        let kind = kind.clone();
        let Some(goal) =
            desa_viewer::goals::generic_goal(level.behaviour.program(), &level.id, &kind)
        else {
            return;
        };
        let start = goal
            .restart
            .and_then(|name| level.nodes.nodes.iter().find(|n| n.name == name));
        // (Restart nodes don't say which way: the level start's way.)
        self.placement = match start.and_then(|n| n.position) {
            Some(at) => {
                let (_, facing, _) = level.home.to_scale_rotation_translation();
                Mat4::from_rotation_translation(facing, at)
            }
            None => level.home,
        };
        self.toggle_skate();
        if self.skating.is_none() {
            return;
        }
        let Some(level) = &mut self.level else { return };
        let b = &mut level.behaviour;
        let before = b.radius_trigger_objects().into_iter().collect();
        b.active_goal = Some(goal.id);
        b.goal_flags.clear();
        b.goal_count = 0;
        b.goal_collected = 0;
        b.goal_collects = desa_viewer::goals::counts_collected(&kind);
        b.goal_needed = goal.needed;
        b.goal_won = false;
        b.goal_lost = false;
        b.goal_paused = false;
        let mut params: qb::vm::Params = goal
            .params
            .iter()
            .map(|(k, v)| (Some(*k), v.clone()))
            .collect();
        params.push((Some(qb::checksum("goal_ID")), qb::Value::Name(goal.id)));
        // (Not talked to the pro yet: its things are made.)
        if !params
            .iter()
            .any(|(k, _)| *k == Some(qb::checksum("talked_to_pro")))
        {
            params.push((Some(qb::checksum("talked_to_pro")), qb::Value::Integer(0)));
        }
        b.goal_params = params.clone();
        // Its pro, there (the game's goal init makes it once the goal's
        // open): it takes things back (Tarzan's fruit).
        if let Some(pro) = goal.pro {
            b.create_object(pro);
        }
        for script in [goal.activate, goal.start_script].into_iter().flatten() {
            b.run_level_script(script, params.clone());
        }
        self.goal_cutscenes(&kind);
        self.queue_objective(&kind);
        let tetris = desa_viewer::goals::Tetris::of(&goal).map(TetrisRun::new);
        self.goal_run = Some(GoalRun {
            kind,
            index,
            left: goal.time,
            goal,
            over: false,
            tetris,
            before,
        });
    }

    /// The goal played from its scripts: the gaps' scripts run (they set
    /// its flags), the clock counts down, and it's won with all its flags
    /// or out of time lost.
    fn update_goal_run(&mut self, dt: f32) {
        let (Some((skater, physics, chase)), Some(level)) = (&mut self.skating, &mut self.level)
        else {
            return;
        };
        // Landed gaps' scripts run whether a goal's on or not (they ask).
        for script in std::mem::take(&mut skater.gap_scripts) {
            level.behaviour.run_level_script(script, Vec::new());
        }
        // Camera paths the scripts play (a goal's cutscenes: each arcade
        // machine coming on), one at a time.
        // (Stopped by them, theirs ends there, and the next can start.)
        if std::mem::take(&mut level.behaviour.camera_stop) && self.script_cutscene {
            if self.cutscene {
                self.model.camera_paths.playing = None;
                self.cutscene = false;
            }
            self.script_cutscene = false;
            self.cutscene_request = None;
        }
        for path in std::mem::take(&mut level.behaviour.cameras) {
            if !self.cutscene && self.cutscene_request.is_none() {
                self.cutscene_request = Some(path);
                self.script_cutscene = true;
            }
        }
        // What the scripts put on screen, as the skater's message.
        // (The game's own screen shows them when it's up.)
        let messages = std::mem::take(&mut level.behaviour.messages);
        if let (Some(text), false) = (messages.last().cloned(), self.model.character.game_hud) {
            skater.message = Some((text, 2.5));
        }
        // And touched trigger geometry's.
        for object in std::mem::take(&mut skater.touches) {
            if qb::vm::trace_printf() {
                let p = skater.position;
                eprintln!(
                    "touch: {} at {:.0} {:.0} {:.0}",
                    level.nodes.labels.get(&object).map_or("?", String::as_str),
                    p.x,
                    p.y,
                    p.z
                );
            }
            if let Some(&script) = level.touch_scripts.get(&object) {
                level.behaviour.run_level_script(script, Vec::new());
            }
        }
        // Knocked down by the scripts (a heckler, a missile).
        if std::mem::take(&mut level.behaviour.knock_down) {
            skater.knock_down();
        }
        // Sent back to a restart by them.
        if let Some(name) = level.behaviour.skater_restart.take() {
            // A restart, or any node with a place (a goal's waypoint),
            // facing its way.
            let at = level
                .nodes
                .spawns
                .iter()
                .find(|s| s.name == name)
                .map(|s| (s.position, s.facing()))
                .or_else(|| {
                    let node = level.nodes.nodes.iter().find(|n| n.name == name)?;
                    let spawn = desa_viewer::nodes::Spawn {
                        label: String::new(),
                        position: node.position?,
                        heading: node.heading,
                        kind: String::new(),
                        name,
                    };
                    Some((spawn.position, spawn.facing()))
                });
            if let (Some((position, facing)), Some(world)) = (at, &level.world) {
                skater.place(position, facing.x.atan2(facing.z), physics, world);
                *chase = ChaseCamera::behind(skater, physics);
            }
        }
        let Some(run) = &mut self.goal_run else {
            return;
        };
        let model = &mut self.model.character;
        let got = level.behaviour.goal_progress();
        // (What it needs, as its scripts may have changed it: Free Zazu's
        // ten tries come down to one once the cage is smashed.)
        let needed = level.behaviour.goal_needed;
        // (What it asks, unless that's its name over.)
        let text = if run.goal.text == run.goal.name {
            String::new()
        } else {
            run.goal.text.clone()
        };
        model.goal_progress = Some((
            run.goal.name.clone(),
            text,
            level.behaviour.goal_shown().min(needed),
            needed,
        ));
        model.run_clock = run.left;
        if run.over {
            return;
        }
        // (Held while the game's screen waits on a press: the goal's
        // speech box, before it starts.)
        let waiting = self.screen.as_ref().is_some_and(|s| s.screen.takes_pad())
            || level.behaviour.goal_paused;
        if let Some(left) = run.left.as_mut().filter(|_| !waiting) {
            *left = (*left - dt).max(0.0);
        }
        // Skatetris: its list, won cleared, lost full.
        let mut tetris_lost = false;
        if let Some(t) = run.tetris.as_mut().filter(|_| !waiting) {
            let mut calls = Vec::new();
            t.update(dt, run.left, skater, level.behaviour.program(), &mut calls);
            level.behaviour.screen_calls.extend(calls);
            if t.won {
                level.behaviour.goal_won = true;
            }
            tetris_lost = t.lost;
        }
        let won = level.behaviour.goal_won || (needed > 0 && got >= needed);
        let lost = tetris_lost || level.behaviour.goal_lost;
        if won || run.left == Some(0.0) || lost {
            run.over = true;
            model.goal_result = Some((run.goal.name.clone(), won));
            if won {
                self.cutscene_request = self.success_camera.take();
                self.goal_won = Some(run.kind.clone());
                if let Some(audio) = self.audio.as_ref().filter(|_| model.sound) {
                    audio.play_named(qb::checksum("GoalDone"), 1.0);
                }
            }
            // Its own scripts for the end: won, the game's success (the
            // goal's outro: Beach's cargo doors open), then its deactivate
            // told so (`just_won_goal`); lost, just the deactivate.
            let b = &mut level.behaviour;
            // The game's messages (see `end_messages`), once the HUD's
            // next up.
            self.goal_end = Some((
                won,
                GoalEnded::Goal(run.goal.name.clone(), run.goal.text.clone()),
            ));
            let mut params = b.goal_params.clone();
            if won {
                if let Some(script) = run.goal.success {
                    b.run_level_script(script, params.clone());
                }
                params.push((None, qb::Value::Name(qb::checksum("just_won_goal"))));
            }
            if let Some(script) = run.goal.deactivate.take() {
                b.run_level_script(script, params);
            }
            b.ended_goal = b.active_goal.take();
        }
    }

    /// A goal played from its scripts stopped: its end script, if it
    /// hasn't run, and the goal manager told it's off.
    fn end_goal_run(&mut self, run: GoalRun) {
        let Some(level) = &mut self.level else { return };
        let b = &mut level.behaviour;
        if let Some(script) = run.goal.deactivate {
            b.run_level_script(script, b.goal_params.clone());
        }
        b.ended_goal = b.active_goal.take();
        b.goal_flags.clear();
    }

    /// Starts the level's S-K-A-T-E letters goal (`AddGoal_Skate`): the
    /// letters appear, spinning and bobbing (`SkateLetter_InitLetter`,
    /// `bounce_skate_letter`), and the skater starts at the goal's restart
    /// node facing the S.
    fn start_letters(&mut self) {
        if self.skating.is_some() {
            self.toggle_skate();
        }
        let Some(level) = &self.level else { return };
        let Some(goal) = desa_viewer::goals::skate_letters(level.behaviour.program(), &level.id)
        else {
            return;
        };
        let objects = goal.letters.map(|name| level.behaviour.object(name));
        if objects.iter().any(Option::is_none) {
            return;
        }
        let objects = objects.map(Option::unwrap);
        let start = goal
            .restart
            .and_then(|name| level.nodes.nodes.iter().find(|n| n.name == name))
            .and_then(|n| n.position);
        if let Some(start) = start {
            let to = level.behaviour.position(objects[0]) - start;
            self.placement =
                Mat4::from_rotation_translation(Quat::from_rotation_y(to.x.atan2(to.z)), start);
        } else {
            self.placement = level.home;
        }
        self.toggle_skate();
        if self.skating.is_none() {
            return;
        }
        let Some(level) = &mut self.level else { return };
        for object in objects {
            level.behaviour.set_alive(object, true);
            level.behaviour.set_spin(object, LETTER_SPIN.to_radians());
            level
                .behaviour
                .run_script(object, qb::checksum("bounce_skate_letter"));
        }
        self.goal_cutscenes("SKATE");
        self.queue_objective("Skate");
        self.letter_lines = goal.streams;
        self.letters = Some(LetterRun {
            objects,
            got: [false; 5],
            left: goal.time,
            time: goal.time,
            over: false,
        });
    }

    /// Counts the letters goal down and picks up the letters the skater
    /// reaches (`Obj_SetInnerRadius 8`, `SkateLetter_GotLetter`): gone,
    /// shown on screen, with the goal sound. All five wins; out of time
    /// loses.
    fn update_letters(&mut self, dt: f32) {
        let (Some(run), Some((skater, ..)), Some(level)) =
            (&mut self.letters, &mut self.skating, &mut self.level)
        else {
            return;
        };
        let model = &mut self.model.character;
        model.letters = Some(run.got);
        model.run_clock = Some(run.left);
        if run.over {
            return;
        }
        run.left = (run.left - dt).max(0.0);
        let body = skater.position + Vec3::Y * 30.0;
        for (i, &object) in run.objects.iter().enumerate() {
            if run.got[i] || body.distance(level.behaviour.position(object)) > LETTER_RADIUS {
                continue;
            }
            run.got[i] = true;
            level.behaviour.set_alive(object, false);
            self.lines_to_say.extend(self.letter_lines[i]);
            // The letter big on the HUD's right, then kept there with the
            // others got (`SkateLetter_HUDLetter`).
            let flag = ["got_S", "got_K", "got_A", "got_T", "got_E"][i];
            level.behaviour.screen_scripts.push((
                qb::checksum("SkateLetter_HUDLetter"),
                goal_panel_params(&[("flag", qb::Value::Name(qb::checksum(flag)))]),
            ));
            if let Some(audio) = self.audio.as_ref().filter(|_| model.sound) {
                audio.play_named(qb::checksum("GoalDone"), 1.0);
            }
        }
        model.letters = Some(run.got);
        let won = run.got.iter().all(|g| *g);
        if won {
            self.cutscene_request = self.success_camera.take();
            self.goal_won = Some("SKATE".to_string());
        }
        if won || run.left == 0.0 {
            run.over = true;
            self.goal_end = Some((won, GoalEnded::Kind("Skate")));
            let taken = run.time - run.left;
            let mut best = None;
            if won {
                let key = format!(
                    "letters.{}.{}",
                    level.id,
                    model
                        .current
                        .map_or("", |i| model.characters[i].id.as_str())
                );
                best = self.settings.best.get(&key).map(|ms| *ms as f32 / 1000.0);
                if best.is_none_or(|b| taken < b) {
                    self.settings
                        .best
                        .insert(key, (taken * 1000.0).round() as u32);
                    self.settings.save();
                }
            }
            model.letters_result = Some((won, taken, best));
        }
    }

    /// Counts a two-minute run down. Out of time, the combo under way
    /// still counts: once the skater's back on the ground with no combo,
    /// the run ends (`EndOfRun`): the skater brakes to a stop and the
    /// score goes against the best.
    fn update_run(&mut self, dt: f32) {
        let (Some(run), Some((skater, ..))) = (&mut self.run, &mut self.skating) else {
            return;
        };
        if self.replay.is_some() {
            return;
        }
        self.model.character.run_clock = Some(run.left);
        if run.over {
            return;
        }
        run.left = (run.left - dt).max(0.0);
        // A score goal is won as soon as the score's there (the combo
        // banked), and the run ends there.
        if let Some((score, _)) = run.goal.as_ref().filter(|_| !run.won) {
            if skater.score >= *score {
                run.won = true;
                self.cutscene_request = self.success_camera.take();
                self.goal_won = Some(
                    match self.model.character.run_goal {
                        Some((true, ..)) => "ProScore",
                        _ => "HighScore",
                    }
                    .to_string(),
                );
                run.ending.get_or_insert(0.0);
                self.model.character.run_goal_won = Some(true);
                if let Some(audio) = self.audio.as_ref().filter(|_| self.model.character.sound) {
                    audio.play_named(qb::checksum("GoalDone"), 1.0);
                }
                self.goal_end = Some((
                    true,
                    GoalEnded::Kind(
                        if matches!(self.model.character.run_goal, Some((true, ..))) {
                            "ProScore"
                        } else {
                            "HighScore"
                        },
                    ),
                ));
            }
        }
        let bailing = matches!(
            skater.action,
            SkateAction::Bail
                | SkateAction::BailFall { .. }
                | SkateAction::BailManual
                | SkateAction::BailGrind
        );
        match &mut run.ending {
            None if run.left == 0.0
                && skater.on_ground
                && !bailing
                && skater.combo_tricks.is_empty()
                && skater.trick.is_none() =>
            {
                run.ending = Some(0.0)
            }
            Some(time) => {
                *time += dt;
                if (*time > 1.0 && skater.speed().abs() < 20.0) || *time > 5.0 {
                    run.over = true;
                    if let (Some((pro, ..)), false) = (&self.model.character.run_goal, run.won) {
                        self.goal_end = Some((
                            false,
                            GoalEnded::Kind(if *pro { "ProScore" } else { "HighScore" }),
                        ));
                    }
                    // Goals keep their own bests.
                    let kind = match &self.model.character.run_goal {
                        Some((true, ..)) => "pro.",
                        Some((false, ..)) => "high.",
                        None => "",
                    };
                    let key = format!(
                        "{kind}{}.{}",
                        self.level.as_ref().map_or("", |l| l.id.as_str()),
                        self.model.character.current.map_or("", |i| self
                            .model
                            .character
                            .characters[i]
                            .id
                            .as_str())
                    );
                    let best = self.settings.best.get(&key).copied().unwrap_or(0);
                    let record = skater.score > best;
                    if record {
                        self.settings.best.insert(key, skater.score);
                        self.settings.save();
                    }
                    self.model.character.run_result = Some((skater.score, best, record));
                }
            }
            None => {}
        }
    }

    /// Moves the skater by the keys held, picks its animation and follows
    /// it with the camera.
    /// The skating controls on a gamepad, the game's way round: the stick
    /// or D-pad steers (up pushes, down brakes), A crouches and ollies, X
    /// (Square) flips, B (Circle) grabs, Y (Triangle) grinds, and the
    /// shoulder buttons revert.
    fn pad_input(&mut self) -> Input {
        use gilrs::{Axis, Button};
        let Some(gilrs) = self.gamepads.as_mut() else {
            return Input::default();
        };
        while gilrs.next_event().is_some() {}
        let mut input = Input::default();
        let mut look = glam::Vec2::ZERO;
        let mut start = false;
        let mut back = false;
        let mut talk = false;
        // (Two players: the second pad is player two's.)
        let two = self.player2.is_some();
        let mut input2 = Input::default();
        for (n, (_, pad)) in gilrs.gamepads().enumerate() {
            let second = two && n == 1;
            let input = if second { &mut input2 } else { &mut input };
            let x = pad.value(Axis::LeftStickX);
            let y = pad.value(Axis::LeftStickY);
            let pressed = |b| pad.is_pressed(b);
            input.push |= y > 0.5 || pressed(Button::DPadUp);
            input.brake |= y < -0.5 || pressed(Button::DPadDown);
            let mut turn = if x.abs() > 0.25 { x } else { 0.0 };
            if pressed(Button::DPadLeft) {
                turn = -1.0;
            }
            if pressed(Button::DPadRight) {
                turn = 1.0;
            }
            if turn != 0.0 {
                input.turn = turn.signum();
            }
            input.crouch |= pressed(Button::South);
            input.flip |= pressed(Button::West);
            input.grab |= pressed(Button::East);
            input.grind |= pressed(Button::North);
            input.revert |= [
                Button::LeftTrigger,
                Button::RightTrigger,
                Button::LeftTrigger2,
                Button::RightTrigger2,
            ]
            .into_iter()
            .any(pressed);
            // The C-stick looks round; Start pauses.
            start |= pressed(Button::Start);
            if second {
                continue;
            }
            let stick = glam::Vec2::new(pad.value(Axis::RightStickX), pad.value(Axis::RightStickY));
            if stick.length() > look.length() && stick.length() > 0.2 {
                look = stick;
            }
            back |= pressed(Button::Select);
            talk |= pressed(Button::East);
        }
        self.pad2 = input2;
        self.pad_look = look;
        if start && !self.pad_start && self.skating.is_some() {
            self.toggle_pause();
        }
        self.pad_start = start;
        if back && !self.pad_back {
            self.next_camera();
        }
        self.pad_back = back;
        self.talk_pressed |= talk && !self.pad_talk;
        self.pad_talk = talk;
        input
    }

    /// The next of the game's chase cameras (`ToggleSkaterCamMode`).
    fn next_camera(&mut self) {
        let count = self.model.character.cameras.len();
        if count > 0 {
            self.model.character.camera = (self.model.character.camera + 1) % count;
        }
    }

    /// Which of the level's goals are won, for the list.
    fn update_goals_won(&mut self) {
        let Some(level) = &self.level else { return };
        self.model.character.goals_won = self
            .model
            .character
            .goals
            .iter()
            .map(|(kind, _)| {
                let id = desa_viewer::goals::goal_id(&level.id, kind);
                self.settings.best.contains_key(&format!("won.{id:08x}"))
            })
            .collect();
        // How many, kept for the level's count elsewhere (the level select).
        let won = self
            .model
            .character
            .goals_won
            .iter()
            .filter(|w| **w)
            .count() as u32;
        let key = format!("goals_won.{}", level.id.to_ascii_lowercase());
        if self.settings.best.get(&key) != Some(&won) {
            self.settings.best.insert(key, won);
            self.settings.save();
        }
        // Every level's, for the game's screens (and the character's
        // records there).
        let character = self
            .model
            .character
            .current
            .and_then(|i| self.model.character.characters.get(i))
            .map_or(String::new(), |c| c.id.clone());
        if let Some(screen) = &mut self.screen {
            screen.screen.records = self
                .model
                .levels
                .iter()
                .map(|l| {
                    let get = |kind: &str| {
                        self.settings
                            .best
                            .get(&format!("record.{kind}.{}.{character}", l.id))
                            .copied()
                            .unwrap_or(0) as i32
                    };
                    (
                        qb::checksum(&format!("load_{}", l.id)),
                        [
                            // (A two-minute run's best, as the run keeps it.)
                            self.settings
                                .best
                                .get(&format!("{}.{character}", l.id))
                                .copied()
                                .unwrap_or(0) as i32,
                            get("combo"),
                            get("grind"),
                            get("manual"),
                            get("lip"),
                            get("tricks"),
                        ],
                    )
                })
                .collect();
            screen.screen.progress = self
                .model
                .levels
                .iter()
                .map(|l| {
                    let won = self
                        .settings
                        .best
                        .get(&format!("goals_won.{}", l.id.to_ascii_lowercase()))
                        .copied()
                        .unwrap_or(0) as i32;
                    let gaps = self.settings.gaps.get(&l.id).map_or(0, |g| g.len()) as i32;
                    (qb::checksum(&format!("load_{}", l.id)), (won, gaps))
                })
                .collect();
        }
    }

    /// Each level's progress for the character shown, beside its name in
    /// the list: its collectibles got and the gaps landed there.
    fn update_progress(&mut self) {
        let character = self
            .model
            .character
            .current
            .map(|i| self.model.character.characters[i].id.clone());
        self.model.level_progress = self
            .model
            .levels
            .iter()
            .map(|level| {
                let mut parts = Vec::new();
                if let Some(c) = &character {
                    let got = self
                        .settings
                        .best
                        .get(&format!("collected.{}.{c}", level.id))
                        .map_or(0, |bits| (bits & 0x01FF_FFFF).count_ones());
                    if got > 0 {
                        parts.push(format!("{got}/25"));
                    }
                }
                if let Some(gaps) = self.settings.gaps.get(&level.id).filter(|g| !g.is_empty()) {
                    parts.push(format!("{} gaps", gaps.len()));
                }
                parts.join(", ")
            })
            .collect();
    }

    /// Follows the skater with the camera picked, when it changes.
    fn apply_camera(&mut self) {
        let wanted = self.model.character.camera;
        if wanted == self.camera_shown {
            return;
        }
        let (Some((_, physics, _)), Some(level)) = (&mut self.skating, &self.level) else {
            return;
        };
        let program = level.behaviour.program();
        if let Some((_, setting)) = Physics::camera_choices(program).get(wanted) {
            physics.set_camera(program, *setting);
            self.camera_shown = wanted;
            self.settings.best.insert("camera".into(), wanted as u32);
            self.settings.save();
        }
    }

    /// The simplified controls or the full ones, when they change: the
    /// skater's tricks read again for them, and the choice kept.
    fn apply_controls(&mut self) {
        let spin_taps = self.model.character.spin_taps;
        if self.settings.best.get("spintaps").is_some_and(|&v| v == 1) != spin_taps {
            self.settings
                .best
                .insert("spintaps".into(), u32::from(spin_taps));
            self.settings.save();
        }
        if let Some((skater, ..)) = &mut self.skating {
            skater.spin_taps = spin_taps;
        }
        let simplified = self.model.character.simplified;
        if self
            .settings
            .best
            .get("simplified")
            .is_some_and(|&v| v == 1)
            != simplified
        {
            self.settings
                .best
                .insert("simplified".into(), u32::from(simplified));
            self.settings.save();
        }
        let (Some((skater, _, _)), Some(level), Some(index)) =
            (&mut self.skating, &self.level, self.model.character.current)
        else {
            return;
        };
        if skater.simplified == simplified {
            return;
        }
        skater.simplified = simplified;
        let id = &self.model.character.characters[index].id;
        let program = level.behaviour.program();
        let stats = if self.model.character.cheats.stats_13 {
            Stats([13.0; 10])
        } else {
            Stats::of(program, id)
        };
        let mut tricks = TrickBook::with_controls(program, id, &stats, simplified);
        if let Some(character) = &self.character {
            tricks.set_durations(|anim| {
                character
                    .animations
                    .iter()
                    .find(|(name, _)| qb::checksum(name) == anim)
                    .map(|(_, a)| a.duration)
            });
        }
        self.model.character.trick_list = tricks.trick_list();
        skater.tricks = tricks;
    }

    /// The character's size, from the scripts (`appearance_jessie`'s
    /// `scale = 1.12`...), once both it and a level are loaded.
    fn scale_character(&mut self) {
        let (Some(character), Some(level), Some(index)) = (
            &mut self.character,
            &self.level,
            self.model.character.current,
        ) else {
            return;
        };
        let name = match self.model.character.characters[index]
            .id
            .to_ascii_lowercase()
            .as_str()
        {
            "kid_girl" => "girl".to_string(),
            id => id.to_string(),
        };
        // (And the kid's body shape, its head smaller; a crew kid's own
        // size.)
        if name == "kid" || name == "girl" {
            character.set_body_shape(&self.kid_shape);
            if let Some(scale) = self.kid_scale {
                character.scale = scale;
                return;
            }
        }
        character.scale = level
            .behaviour
            .program()
            .value(qb::checksum(&format!("appearance_{name}")))
            .and_then(|a| a.get(qb::checksum("scale")))
            .and_then(qb::Value::as_f32)
            .unwrap_or(1.0);
    }

    /// Pauses skating, or goes on.
    fn toggle_pause(&mut self) {
        if self.skating.is_some() && self.replay.is_none() {
            self.model.character.paused = !self.model.character.paused;
            // The game's own pause menu (`create_pause_menu`), if it can
            // be made; taken away again on resuming (the panel stays).
            let paused = self.model.character.paused;
            if let Some(screen) = &mut self.screen {
                screen.screen.destroy_id("pause_menu");
                // (Whichever way it's left, the music goes on.)
                if !paused {
                    screen.screen.music_paused = Some(false);
                }
                if paused && self.level.is_some() {
                    screen
                        .screen
                        .run(qb::checksum("create_pause_menu"), Vec::new());
                }
            }
        }
    }

    /// The game's own panel while skating (`create_gamemode_panel`: the
    /// score, special bar, trick text, clock and balance meter), kept up
    /// to date as the game's code does; or, not skating, none.
    fn update_game_hud(&mut self) {
        use qb::Value;
        // (Kept for the game's panel while it's coming up: a goal's pop-up
        // is queued as the skating starts.)
        let ready = self.skating.is_none()
            || (self.hud.up
                && self
                    .screen
                    .as_ref()
                    .is_some_and(|s| s.screen.exists("the_score")));
        let (screen_calls, screen_scripts) = self
            .level
            .as_mut()
            .filter(|_| ready)
            .map(|l| {
                (
                    std::mem::take(&mut l.behaviour.screen_calls),
                    std::mem::take(&mut l.behaviour.screen_scripts),
                )
            })
            .unwrap_or_default();
        let (Some(screen), Some(level)) = (&mut self.screen, &self.level) else {
            return;
        };
        let program = level.behaviour.program();
        let s = &mut screen.screen;
        // The character, for the menus' profile questions too (the skater
        // select, the level select's worlds), skating or not.
        s.character = self
            .model
            .character
            .current
            .and_then(|i| self.model.character.characters.get(i))
            .map(|c| c.id.clone());
        // (Two players: each their own HUD, see `skate_player2`.)
        let Some((skater, ..)) = self.skating.as_ref().filter(|_| self.player2.is_none()) else {
            // (Two players: the game's panel its menus put up, Split
            // Mode's and Ready's, down again.)
            if self.hud.up || self.player2.is_some() {
                self.hud = GameHud::default();
                s.destroy_id("player1_panel_container");
                s.destroy_id("player2_panel_container");
                s.destroy_id("the_time");
                s.destroy_id("current_goal");
                s.destroy_id("goal_points_text");
                s.destroy_id("minigame_timer");
                s.destroy_id("goal_start_dialog");
            }
            self.model.character.game_hud = false;
            self.goal_popup = None;
            return;
        };
        if !self.hud.up {
            self.hud = GameHud {
                up: true,
                ..GameHud::default()
            };
            s.run(qb::checksum("create_gamemode_panel"), Vec::new());
            return;
        }
        if !s.exists("the_score") {
            return;
        }
        self.model.character.game_hud = true;
        // The level's scripts' screen commands (a goal's messages, text,
        // counters) onto the game's screen.
        for (name, args) in screen_calls {
            s.command(name, &args, program);
        }
        for (script, params) in screen_scripts {
            s.run(script, params);
        }
        // A goal's objective, popped up mid-screen and then kept small at
        // the top right (`create_panel_block id = current_goal style =
        // panel_message_goal`): as the game does, once its start's own
        // scripts have run (they clear the panel's messages, a frame on)
        // and any speech box is closed. A goal that counts on the HUD
        // (`use_hud_counter`) gets its counter then too (the game's
        // `goal_create_counter`, kept up to date below); others show no
        // count.
        let waiting = s.takes_pad() || level.behaviour.goal_paused;
        let race_counter = self
            .racing
            .as_ref()
            .filter(|r| !r.over)
            .map(|r| (r.next.min(r.points.len()), r.points.len()));
        // (A score, letters or race goal over: its objective down, as a
        // goal's end takes it.)
        let goal_on = self.goal_run.as_ref().is_some_and(|r| !r.over)
            || self.letters.as_ref().is_some_and(|r| !r.over)
            || self.racing.as_ref().is_some_and(|r| !r.over)
            || self
                .run
                .as_ref()
                .is_some_and(|r| r.goal.is_some() && !r.over);
        if !goal_on && self.goal_popup.is_none() {
            if s.exists("current_goal") {
                s.destroy_id("current_goal");
            }
            if s.exists("goal_counter_anchor") {
                s.destroy_id("goal_counter_anchor");
            }
            self.hud.counted = None;
        }
        // A score, letters or race goal's end: the game's messages, as
        // for any goal (`goal_success_messages`, `goal_fail`); the
        // letters' own off the HUD first (`kill_skate_messages`).
        // A pro's offer: the game's speech box at the bottom ("Woody:
        // Press (B) to talk.", `create_speech_box anchor_id =
        // goal_start_dialog`), gone when it's taken or turned down.
        if self.hud.offer != self.goal_offer {
            if self.hud.offer.take().is_some() {
                s.run(
                    qb::checksum("speech_box_exit"),
                    goal_panel_params(&[
                        ("anchor_id", Value::Name(qb::checksum("goal_start_dialog"))),
                        ("no_pad_start", Value::Integer(1)),
                    ]),
                );
            }
            if let Some(pro) = self.goal_offer.and_then(|i| level.pros.get(i)) {
                s.run(
                    qb::checksum("create_speech_box"),
                    goal_panel_params(&[
                        ("anchor_id", Value::Name(qb::checksum("goal_start_dialog"))),
                        ("text", Value::String(pro.talk.clone())),
                        ("no_pad_choose", Value::Integer(1)),
                        ("no_pad_start", Value::Integer(1)),
                        (
                            "bg_rgba",
                            Value::Array([100, 100, 100, 128].map(Value::Integer).to_vec()),
                        ),
                        (
                            "text_rgba",
                            Value::Array([128, 128, 128, 128].map(Value::Integer).to_vec()),
                        ),
                        ("bg_x_scale", Value::Float(1.1)),
                        ("z_priority", Value::Integer(5)),
                    ]),
                );
                self.hud.offer = self.goal_offer;
            }
        }
        match self.goal_end.take() {
            Some((won, GoalEnded::Goal(name, text))) => end_messages(s, won, &name, &text),
            Some((won, GoalEnded::Kind(kind))) => {
                if kind == "Skate" {
                    s.run(qb::checksum("kill_skate_messages"), Vec::new());
                }
                if let Some(goal) = desa_viewer::goals::generic_goal(program, &level.id, kind) {
                    end_messages(s, won, &goal.name, &goal.text);
                }
            }
            None => {}
        }
        // Its pro's description first, if it's begun by talking to them:
        // the game's speech box, Press (A) to continue (the objective
        // and the clock waiting on it).
        if let Some(description) = self.goal_popup.as_mut().and_then(|p| p.2.take()) {
            s.run(
                qb::checksum("create_speech_box"),
                goal_panel_params(&[
                    (
                        "anchor_id",
                        Value::Name(qb::checksum("goal_description_anchor")),
                    ),
                    ("text", Value::String(description)),
                    ("style", Value::Name(qb::checksum("speech_box_style"))),
                    ("bg_x_scale", Value::Float(1.7)),
                    (
                        "pad_choose_script",
                        Value::Name(qb::checksum("speech_box_exit")),
                    ),
                    (
                        "pad_choose_params",
                        Value::Struct(goal_panel_params(&[(
                            "anchor_id",
                            Value::Name(qb::checksum("goal_description_anchor")),
                        )])),
                    ),
                ]),
            );
        }
        if let Some((text, frames, _)) = self.goal_popup.as_mut().filter(|_| !waiting) {
            if *frames > 0 {
                *frames -= 1;
            } else {
                let line = text.clone();
                s.run(
                    qb::checksum("create_panel_block"),
                    goal_panel_params(&[
                        ("id", Value::Name(qb::checksum("current_goal"))),
                        ("text", Value::String(line.clone())),
                        ("style", Value::Name(qb::checksum("panel_message_goal"))),
                    ]),
                );
                self.goal_popup = None;
                let b = &level.behaviour;
                let flag = |key: &str| {
                    b.goal_params
                        .iter()
                        .any(|(k, _)| *k == Some(qb::checksum(key)))
                };
                if self.goal_run.is_some() && flag("use_hud_counter") {
                    s.run(
                        qb::checksum("goal_create_counter"),
                        counter_params(b, program),
                    );
                    self.hud.counted = Some(b.goal_shown());
                } else if let Some((got, _)) = race_counter {
                    // A race that counts its waypoints (Canyon's).
                    if goal_setting(program, &level.id, "Race", "use_hud_counter").is_some() {
                        s.run(
                            qb::checksum("goal_create_counter"),
                            race_counter_params(program, &level.id),
                        );
                        self.hud.counted = Some(got);
                    }
                }
            }
        }
        // The goal on, for the UI's scripts that ask after it (and
        // whether there's one to retry).
        s.can_retry = self.goal_run.is_some();
        // The volumes: the sound options' changes taken, else the panel's
        // shown.
        let model = &mut self.model.character;
        if std::mem::take(&mut s.rumble_changed) {
            model.rumble = s.rumble;
        } else {
            s.rumble = model.rumble;
        }
        if std::mem::take(&mut s.hawk_controls_changed) {
            model.simplified = !s.hawk_controls;
        } else {
            s.hawk_controls = !model.simplified;
        }
        // The cheats: as the game's codes and cheats menu set their flags
        // (CHEAT_MOON...), else as the panel has them.
        {
            let flag = |name: &str| {
                desa_viewer::behaviour::flag_number(program, &qb::Value::Name(qb::checksum(name)))
            };
            let cheats = &mut model.cheats;
            let pairs: [(&str, &mut bool); 7] = [
                ("CHEAT_PERFECT_MANUAL", &mut cheats.perfect_manual),
                ("CHEAT_PERFECT_RAIL", &mut cheats.perfect_rail),
                ("CHEAT_PERFECT_SKITCH", &mut cheats.perfect_skitch),
                ("CHEAT_ALWAYS_SPECIAL", &mut cheats.always_special),
                ("CHEAT_MOON", &mut cheats.moon),
                ("CHEAT_SLOMO", &mut cheats.slomo),
                ("CHEAT_STATS_13", &mut cheats.stats_13),
            ];
            let from_game = std::mem::take(&mut s.flags_changed);
            for (name, on) in pairs {
                let Some(number) = flag(name) else { continue };
                if from_game {
                    *on = s.global_flags.contains(&number);
                } else if *on {
                    s.global_flags.insert(number);
                } else {
                    s.global_flags.remove(&number);
                }
            }
        }
        if std::mem::take(&mut s.spin_taps_changed) {
            model.spin_taps = s.spin_taps;
        } else {
            s.spin_taps = model.spin_taps;
        }
        if std::mem::take(&mut s.volumes_changed) {
            model.music_volume = s.music_level as f32 / 10.0;
            model.effects_volume = s.sound_level as f32 / 10.0;
        } else {
            s.music_level = (model.music_volume * 10.0).round() as i32;
            s.sound_level = (model.effects_volume * 10.0).round() as i32;
        }
        // The goals for View Goals.
        s.goal_list = self
            .model
            .character
            .goals
            .iter()
            .enumerate()
            .map(|(i, (kind, text))| {
                (
                    desa_viewer::goals::goal_id(&level.id, kind),
                    text.clone(),
                    self.model
                        .character
                        .goals_won
                        .get(i)
                        .copied()
                        .unwrap_or(false),
                )
            })
            .collect();
        let b = &level.behaviour;
        s.goal = b
            .active_goal
            .or(b.ended_goal)
            .map(|id| (id, b.goal_params.clone()));
        let flags = b
            .goal_params
            .iter()
            .any(|(k, _)| *k == Some(qb::checksum("Goal_Flags")));
        s.goal_counts = match race_counter {
            // A race's: its waypoints reached, of them all.
            Some((got, of)) => (got, Some(of)),
            None => (b.goal_shown(), flags.then_some(b.goal_needed)),
        };
        let props = |items: Vec<(&str, Value)>| {
            Value::Struct(
                items
                    .into_iter()
                    .map(|(k, v)| (Some(qb::checksum(k)), v))
                    .collect(),
            )
        };
        let rgba = |c: [i32; 4]| Value::Array(c.map(Value::Integer).to_vec());
        // The score.
        s.set(
            "the_score",
            &props(vec![("text", Value::String(skater.score.to_string()))]),
            program,
        );
        // The special bar: filling the SPECIAL frame, blue, then yellow
        // with the special on (`special_bar_colors`).
        if let (Some(bar), Some(frame)) = (s.image_size("specialbar"), s.image_size("special")) {
            let full = ((frame.x * 1.73 - 3.0) / bar.x.max(1.0)).max(0.0);
            let fill = (skater.special_meter / 3000.0).clamp(0.0, 1.0);
            let colour = if skater.special {
                [128, 128, 64, 110]
            } else {
                [64, 64, 128, 110]
            };
            s.set(
                "the_special_bar_sprite",
                &props(vec![
                    ("scale", Value::Pair([full * fill, 1.1])),
                    ("rgba", rgba(colour)),
                ]),
                program,
            );
        }
        // The balance meter: over the skater grinding, beside it in a
        // manual (`balance_meter_info`'s bar positions), its arrow along
        // the arc of `arrow_positions` by the lean.
        match skater.balance_meter() {
            Some(lean) => {
                let grinding = skater.grind.is_some() || skater.lip.is_some();
                let bar = if grinding {
                    [320.0, 165.0]
                } else {
                    [250.0, 224.0]
                };
                let arc = [
                    [0.0, -17.0],
                    [10.0, -17.0],
                    [20.0, -15.0],
                    [30.0, -11.0],
                    [40.0, -6.0],
                    [50.0, 1.0],
                    [60.0, 12.0],
                ];
                let k = (lean.abs() * 6.0).round() as usize;
                let [x, y] = arc[k.min(6)];
                let x = if lean < 0.0 { -x } else { x };
                // A manual's stands up beside the skater: turned a quarter
                // round, its arc with it.
                let (angle, [x, y]) = if grinding {
                    (0.0, [x, y])
                } else {
                    (-90.0, [y, -x])
                };
                let middle = s.image_size("balancemeter").unwrap_or_default() / 2.0;
                s.set(
                    "the_balance_meter",
                    &props(vec![
                        ("pos", Value::Pair(bar)),
                        ("rgba", rgba([95, 95, 95, 106])),
                        ("rot_angle", Value::Float(angle)),
                    ]),
                    program,
                );
                s.set(
                    "the_balance_meter",
                    &props(vec![(
                        "tags",
                        props(vec![("tag_turned_on", Value::Integer(1))]),
                    )]),
                    program,
                );
                // Its arrow, the meter's first child.
                let arrow = Value::Struct(vec![
                    (None, Value::Name(qb::checksum("the_balance_meter"))),
                    (Some(qb::checksum("child")), Value::Integer(0)),
                ]);
                s.set_resolved(
                    &arrow,
                    &props(vec![
                        ("pos", Value::Pair([x + middle.x, y + middle.y])),
                        ("rgba", rgba([128, 128, 128, 100])),
                        ("rot_angle", Value::Float(angle)),
                    ]),
                    program,
                );
            }
            None => {
                s.set(
                    "the_balance_meter",
                    &props(vec![("rgba", rgba([128, 128, 128, 0]))]),
                    program,
                );
                let arrow = Value::Struct(vec![
                    (None, Value::Name(qb::checksum("the_balance_meter"))),
                    (Some(qb::checksum("child")), Value::Integer(0)),
                ]);
                s.set_resolved(
                    &arrow,
                    &props(vec![("rgba", rgba([128, 128, 128, 0]))]),
                    program,
                );
            }
        }
        // The clock (a run's or a goal's).
        let clock = self
            .model
            .character
            .run_clock
            .map(|t| {
                let t = t.max(0.0).ceil() as u32;
                format!("{}:{:02}", t / 60, t % 60)
            })
            .unwrap_or_default();
        s.set(
            "the_time",
            &props(vec![("text", Value::String(clock))]),
            program,
        );
        // A goal's HUD counter, its count changed: the game's
        // `goal_update_counter` (the new number popping).
        if let Some(counted) = self.hud.counted {
            let b = &level.behaviour;
            let now = race_counter.map_or(b.goal_shown(), |r| r.0);
            if now != counted && s.exists("goal_counter_anchor") {
                let params = match race_counter {
                    Some(_) => race_counter_params(program, &level.id),
                    None => counter_params(b, program),
                };
                s.run(qb::checksum("goal_update_counter"), params);
                self.hud.counted = Some(now);
            }
        }
        // The trick text: the combo going (its tricks, and its points times
        // its multiplier), the game's own scripts animating each new
        // trick, the landing or the bail, and fading it after.
        let ids = vec![
            (
                Some(qb::checksum("the_trick_text_id")),
                Value::Name(qb::checksum("the_trick_text")),
            ),
            (
                Some(qb::checksum("the_score_pot_text_id")),
                Value::Name(qb::checksum("the_score_pot_text")),
            ),
            (
                Some(qb::checksum("trick_text_container_id")),
                Value::Name(qb::checksum("trick_text_container")),
            ),
        ];
        let names = |c: &skate::Combo| {
            c.tricks
                .iter()
                .map(|t| match t.spins {
                    0 => t.name.clone(),
                    n => format!("{} {}", n * 180, t.name),
                })
                .collect::<Vec<_>>()
                .join(" + ")
        };
        let tricks = skater.combo_tricks.tricks.len();
        if tricks > 0 {
            let combo = &skater.combo_tricks;
            s.set(
                "the_trick_text",
                &props(vec![("text", Value::String(names(combo)))]),
                program,
            );
            s.set(
                "the_score_pot_text",
                &props(vec![(
                    "text",
                    Value::String(format!("{} X {}", combo.points(), combo.multiplier())),
                )]),
                program,
            );
            if tricks != self.hud.tricks {
                s.run(qb::checksum("trick_text_pulse"), ids.clone());
            }
            self.hud.shown = Some(0.0);
        } else if self.hud.tricks > 0 {
            // The combo's over: landed or bailed.
            let bailed = skater.last_combo.as_ref().is_some_and(|l| l.bailed);
            if let Some(last) = &skater.last_combo {
                let pot = if bailed {
                    "Bail!".to_string()
                } else {
                    last.total.to_string()
                };
                s.set(
                    "the_score_pot_text",
                    &props(vec![("text", Value::String(pot))]),
                    program,
                );
            }
            let script = if bailed {
                "trick_text_bail"
            } else {
                "trick_text_landed"
            };
            s.run(qb::checksum(script), ids.clone());
            self.hud.shown = Some(0.0);
        }
        self.hud.tricks = tricks;
        // A while after, it fades.
        if let Some(t) = &mut self.hud.shown {
            *t += self.hud_dt;
            if tricks == 0 && *t > 2.5 {
                s.run(qb::checksum("trick_text_countdown"), ids);
                self.hud.shown = None;
            }
        }
        // A new song: its title, as a panel message at the bottom.
        if let Some((title, at)) = &self.now_playing {
            if self.hud.song.as_ref() != Some(at) {
                self.hud.song = Some(*at);
                s.run(
                    qb::checksum("Create_Panel_Message"),
                    vec![
                        (
                            Some(qb::checksum("id")),
                            Value::Name(qb::checksum("now_playing_message")),
                        ),
                        (Some(qb::checksum("text")), Value::String(title.clone())),
                        (Some(qb::checksum("pos")), Value::Pair([320.0, 380.0])),
                        (Some(qb::checksum("rgba")), rgba([128, 128, 128, 100])),
                        (Some(qb::checksum("time")), Value::Integer(4000)),
                    ],
                );
            }
        }
    }

    /// The controller in the game's menus: the d-pad or left stick moves,
    /// A chooses, B goes back, Start is Start, each on being pressed.
    fn menu_pad(&mut self) {
        use desa_viewer::screen::Pad;
        use gilrs::{Axis, Button};
        // (What's held is kept track of all along, so the Start that
        // paused isn't taken as a press in the menu it brings up.)
        let Some(gilrs) = self.gamepads.as_mut() else {
            return;
        };
        while gilrs.next_event().is_some() {}
        let mut held = Vec::new();
        for (_, pad) in gilrs.gamepads() {
            let (x, y) = (pad.value(Axis::LeftStickX), pad.value(Axis::LeftStickY));
            let pressed = |b| pad.is_pressed(b);
            for (on, p) in [
                (pressed(Button::DPadUp) || y > 0.6, Pad::Up),
                (pressed(Button::DPadDown) || y < -0.6, Pad::Down),
                (pressed(Button::DPadLeft) || x < -0.6, Pad::Left),
                (pressed(Button::DPadRight) || x > 0.6, Pad::Right),
                (pressed(Button::South), Pad::Choose),
                (pressed(Button::East), Pad::Back),
                (pressed(Button::Start), Pad::Start),
            ] {
                if on && !held.contains(&p) {
                    held.push(p);
                }
            }
        }
        let fresh: Vec<Pad> = held
            .iter()
            .copied()
            .filter(|p| !self.menu_held.contains(p))
            .collect();
        self.menu_held = held;
        if !self.model.character.game_menu {
            return;
        }
        if let Some(screen) = &mut self.screen {
            for p in fresh {
                screen.screen.pad(p);
            }
        }
    }

    /// In the game alone, with the panel shut, the panel's latest message
    /// to show on screen: for 4 seconds, then gone.
    fn game_notice(&mut self) -> Option<String> {
        if !self.game || self.model.panel_open {
            return None;
        }
        let message = self.model.message.clone()?;
        if self.notice.as_ref().is_none_or(|(m, _)| *m != message) {
            self.notice = Some((message.clone(), Instant::now()));
        }
        let (_, since) = self.notice.as_ref()?;
        if since.elapsed().as_secs_f32() > 4.0 {
            self.model.message = None;
            self.notice = None;
            return None;
        }
        Some(message)
    }

    /// The game alone: the panel shut, and no rails, spawn markers or map.
    fn set_game_mode(&mut self) {
        self.game = true;
        self.model.panel_open = false;
        self.model.show_rails = false;
        self.model.show_spawns = false;
        self.model.character.show_map = false;
    }

    /// The game's main menu (`launch_main_menu`), in the Skate Shop as the
    /// game has it: there now, or once the shop's loaded.
    fn open_main_menu(&mut self) {
        if self.skating.is_some() {
            self.toggle_skate();
        }
        // (One player again, till 2 Player's chosen.)
        self.two_player = None;
        self.menu_profile = 0;
        if let Some(screen) = &mut self.screen {
            screen.screen.split_screen = false;
            screen.screen.profile = 0;
        }
        let Some(i) = self
            .model
            .levels
            .iter()
            .position(|l| l.id.eq_ignore_ascii_case("SkateShop"))
        else {
            return;
        };
        let here = self
            .level
            .as_ref()
            .is_some_and(|l| l.id.eq_ignore_ascii_case("SkateShop"));
        if here {
            if let Some(screen) = &mut self.screen {
                screen.screen.clear();
                screen
                    .screen
                    .run(qb::checksum("launch_main_menu"), Vec::new());
            }
        } else {
            self.main_menu_pending = true;
            self.start_load(i);
        }
    }

    /// The movie the scripts play on the level's screen (the Skate Shop's
    /// TV shows the skater select's character; the Hub's, a random one):
    /// started when asked for, its frames drawn on the screen's layer as
    /// they're due, round again if it loops. Without sound (the menus'
    /// music plays over it).
    fn update_screen_movie(&mut self) {
        let asked = {
            let from_screen = self
                .screen
                .as_mut()
                .and_then(|s| s.screen.screen_movie.take());
            let level = self.level.as_mut();
            let from_level = level.and_then(|l| l.behaviour.screen_movie.take());
            from_screen.or(from_level).or(self.teleport_movie.take())
        };
        let Some(level) = &mut self.level else { return };
        if level.movie_screen.is_none() {
            return;
        }
        if let Some(asked) = asked {
            level.screen_movie = None;
            if let (Some((name, looping)), Some(data), Some(ffmpeg)) =
                (asked, &self.data, desa_viewer::movie::ffmpeg())
            {
                if let Some(source) = data.movie(&name) {
                    if let Ok((movie, _sound)) = desa_viewer::movie::Movie::start(&source, &ffmpeg)
                    {
                        level.screen_movie = Some((movie, Instant::now(), looping));
                    }
                }
                self.screen_movie_name = Some(name);
            }
        }
        // Paused with the game (`PauseMovie`): held where it is, and on
        // from there after.
        let pause = self
            .screen
            .as_mut()
            .and_then(|s| s.screen.movie_paused.take());
        let Some((movie, started, looping)) = &mut level.screen_movie else {
            return;
        };
        match (pause, self.screen_movie_paused) {
            (Some(true), None) => self.screen_movie_paused = Some(Instant::now()),
            (Some(false), Some(at)) => {
                *started += at.elapsed();
                self.screen_movie_paused = None;
            }
            _ => {}
        }
        if self.screen_movie_paused.is_some() {
            return;
        }
        let seconds = started.elapsed().as_secs_f64();
        let (width, height) = (movie.info.width, movie.info.height);
        if let Some((frame, true)) = movie.frame_at(seconds) {
            level
                .renderer
                .show_on_layer(level.movie_screen, width, height, frame);
        }
        if movie.finished() && *looping {
            // Round again.
            if let (Some(name), Some(data), Some(ffmpeg)) = (
                self.screen_movie_name.clone(),
                &self.data,
                desa_viewer::movie::ffmpeg(),
            ) {
                if let Some(source) = data.movie(&name) {
                    if let Ok((movie, _sound)) = desa_viewer::movie::Movie::start(&source, &ffmpeg)
                    {
                        level.screen_movie = Some((movie, Instant::now(), true));
                    }
                }
            }
        }
    }

    /// Whether L and R are held (a pad's shoulder buttons, or Q and E).
    fn stage_turn_held(&self) -> (bool, bool) {
        use gilrs::Button;
        let pad = |b| {
            self.gamepads
                .as_ref()
                .is_some_and(|g| g.gamepads().any(|(_, p)| p.is_pressed(b)))
        };
        (
            pad(Button::LeftTrigger) || self.keys.contains(&KeyCode::KeyQ),
            pad(Button::RightTrigger) || self.keys.contains(&KeyCode::KeyE),
        )
    }

    /// Which way the HUD's arrows point (`Create3dArrowPointer`, while a
    /// goal's on): each target's angle off straight ahead, as seen from
    /// the skater by the camera (radians, positive to the right).
    fn hud_arrow_angles(&self) -> Vec<f32> {
        let Some(level) = &self.level else {
            return Vec::new();
        };
        let race = self
            .racing
            .as_ref()
            .filter(|r| !r.over)
            .and_then(|r| r.points.get(r.next))
            .map(|p| p.0);
        if self.goal_run.is_none() && level.behaviour.active_goal.is_none() && race.is_none() {
            return Vec::new();
        }
        let (from, forward) = match &self.skating {
            Some((skater, _, chase)) => (skater.position, chase.target - chase.eye),
            None => (self.camera.position, self.camera.forward()),
        };
        let forward = forward.with_y(0.0).normalize_or_zero();
        let right = Vec3::new(-forward.z, 0.0, forward.x);
        level
            .behaviour
            .arrow_targets()
            .into_iter()
            .chain(race)
            .filter_map(|at| {
                let to = (at - from).with_y(0.0).normalize_or_zero();
                (to != Vec3::ZERO && forward != Vec3::ZERO)
                    .then(|| to.dot(right).atan2(to.dot(forward)))
            })
            .collect()
    }

    /// The game's skater select (in the Skate Shop) and the character
    /// shown: it starts on the one the viewer has (`disney_skater`, the
    /// `skater_select_array` index, which the game keeps), and the one it
    /// moves to is loaded to stand on the stage.
    fn follow_skater_select(&mut self) {
        let Some(level) = &self.level else { return };
        if !level.id.eq_ignore_ascii_case("SkateShop") {
            self.shop_skater = None;
            self.shop_staged = false;
            // (And its song stops: the menus' are the shop's.)
            self.menu_track = None;
            return;
        }
        // Arrived: the stage's camera, as the game's `init_stage` has it
        // (the main menu and the skater select both over the stage).
        if !self.shop_staged {
            let index = level
                .camera_paths
                .iter()
                .position(|(name, _)| name.eq_ignore_ascii_case("Camera_stage"));
            if let Some(index) = index {
                self.model.camera_paths.playing = Some(index);
                self.model.camera_paths.time = 0.0;
                self.camera_hold = true;
            }
        }
        self.shop_staged = true;
        let program = level.behaviour.program();
        let c = qb::checksum;
        let Some(qb::Value::Array(skaters)) = program.value(c("skater_select_array")).cloned()
        else {
            return;
        };
        let name_of = |i: i32| {
            skaters
                .get(usize::try_from(i).ok()?)?
                .get(c("Name"))
                .and_then(qb::Value::as_name)
        };
        let current = self
            .model
            .character
            .current
            .and_then(|i| self.model.character.characters.get(i))
            .map(|ch| c(&ch.id));
        let now = program
            .value(c("disney_skater"))
            .and_then(qb::Value::as_int)
            .unwrap_or(-1);
        // On the stage (where the game's stage characters stand,
        // `TRG_Ped_Jessie` and the rest, as its `init_stage` has the
        // skater), else where the shop has the character.
        let stage_id = skaters
            .get(usize::try_from(now).unwrap_or(0))
            .and_then(|s| s.get(c("id")))
            .and_then(qb::Value::as_name);
        let stage = stage_id
            .and_then(|id| level.nodes.nodes.iter().find(|n| n.name == id))
            .and_then(|n| {
                // Facing the stage's camera (`Camera_stage`, which ends in
                // front of the stage): the node's heading turned as the
                // spawn points' are (`Spawn::facing`), the other way from
                // `ObjectNode::rotation`, which had the character's back
                // to it.
                let turn = Quat::from_rotation_y(n.heading);
                Some(Mat4::from_rotation_translation(turn, n.position?))
            });
        // Turned with L and R (Q and E) while the skater select's up, as
        // its `skateshop_rotate_skater` does (3 degrees a frame).
        let selecting = self
            .screen
            .as_ref()
            .is_some_and(|s| s.screen.exists("select_skater_anchor"));
        if selecting {
            let (left, right) = self.stage_turn_held();
            if left != right {
                let step = 3f32.to_radians() * if left { -1.0 } else { 1.0 };
                self.stage_turn = (self.stage_turn + step) % std::f32::consts::TAU;
            }
        }
        // (Or where the menus have moved it: the Edit Skater set. Only a
        // kid's the skater on the stage: for the others the game's
        // `manifest_cur_skater` sends the skater off to `TRG_Start_P1`
        // and makes the character's ped stand there instead, `disney_skater
        // > LAST_CAK`, which is what the viewer shows.)
        let last_cak = program
            .value(c("LAST_CAK"))
            .and_then(qb::Value::as_int)
            .unwrap_or(1);
        let moved = self
            .screen
            .as_ref()
            .filter(|_| now <= last_cak)
            .and_then(|s| s.screen.skater_node)
            // (Back to its place on the stage: as the stage has it.)
            .filter(|&id| Some(id) != stage_id)
            .and_then(|id| level.nodes.nodes.iter().find(|n| n.name == id))
            .and_then(|n| {
                let turn = Quat::from_rotation_y(n.heading);
                Some(Mat4::from_rotation_translation(turn, n.position?))
            });
        let placement = match (moved, stage) {
            (Some(moved), _) => moved,
            (None, Some(stage)) => stage * Mat4::from_rotation_y(self.stage_turn),
            (None, None) => level.home,
        };
        if self.skating.is_none() {
            self.placement = placement;
        }
        // Not set yet: the character the viewer has.
        if now < 0 || name_of(now).is_none() {
            // (The girl kid is the first kid, the boy the one after.)
            let index = if current == Some(c("kid_girl")) {
                Some(0)
            } else if current == Some(c("kid")) {
                Some(1)
            } else {
                (0..skaters.len() as i32).find(|&i| name_of(i).is_some() && name_of(i) == current)
            };
            if let Some(index) = index {
                program.change(c("disney_skater"), qb::Value::Integer(index));
                self.shop_skater = Some(index);
            }
            return;
        }
        if self.shop_skater == Some(now) {
            return;
        }
        self.shop_skater = Some(now);
        // (The first kid's the girl: its theme's CAS_Girl.)
        let name = match name_of(now) {
            Some(n) if n == c("kid") && now == 0 => Some(c("kid_girl")),
            n => n,
        };
        let wanted = name.filter(|n| Some(*n) != current).and_then(|n| {
            self.model
                .character
                .characters
                .iter()
                .position(|ch| c(&ch.id) == n)
        });
        if let Some(index) = wanted {
            self.load_character(Some(index));
        }
    }

    /// The Skate Shop's Edit Skater menus and the created kid: what it
    /// wears, for them to ask after, and the parts they put on it (as each
    /// is picked, as the game shows it), worn and kept.
    fn dress_kid_from_menus(&mut self) {
        use desa_viewer::screen::AppearanceEdit;
        let (Some(screen), Some(girl)) = (&mut self.screen, self.kid_shown) else {
            return;
        };
        let mut edits = std::mem::take(&mut screen.screen.appearance_edits);
        // The created skaters' slots saved, for the menus to list.
        screen.screen.saved_kids = (0..6)
            .filter_map(|i| {
                self.settings.best.get(&format!("cak.{i}.girl")).map(|&g| {
                    let name = self.settings.names.get(&format!("cak.{i}")).cloned();
                    (i, g == 1, name)
                })
            })
            .collect();
        // (A slot waited for while its kid was put on.)
        if let Some(i) = self.kid_pending_slot.take() {
            let name = qb::checksum(&format!("cak_profile{i}"));
            let wanted = self.settings.best.get(&format!("cak.{i}.girl")) == Some(&1);
            if wanted == girl {
                edits.insert(0, AppearanceEdit::Restore(Some(name)));
            } else {
                self.kid_pending_slot = Some(i);
            }
        }
        // (An appearance for this kid, waited for while it was put on.)
        if let Some(app) = self.kid_pending.take() {
            if app.girl == girl {
                edits.insert(0, AppearanceEdit::Apply(app));
            } else {
                self.kid_pending = Some(app);
            }
        }
        // An appearance for the other kid: that kid, then it.
        if let Some(app) = edits.iter().rev().find_map(|e| match e {
            AppearanceEdit::Apply(app) if app.girl != girl => Some(app.clone()),
            _ => None,
        }) {
            let id = if app.girl { "kid_girl" } else { "kid" };
            let index = self
                .model
                .character
                .characters
                .iter()
                .position(|c| c.id.eq_ignore_ascii_case(id));
            self.kid_pending = Some(app);
            self.load_character(index);
            return;
        }
        // Made over as a new boy or girl: that kid, in the game's outfit.
        if let Some(made_girl) = edits.iter().rev().find_map(|e| match e {
            AppearanceEdit::Reset(girl) => Some(*girl),
            _ => None,
        }) {
            let sex = if made_girl { "f" } else { "m" };
            self.wardrobes.remove(&made_girl);
            self.kid_kept.remove(&made_girl);
            // (Its own shape and size again.)
            self.kid_scale = None;
            self.kid_shape_kept = None;
            self.settings
                .best
                .retain(|k, _| !k.starts_with(&format!("outfit.{sex}.")));
            let id = if made_girl { "kid_girl" } else { "kid" };
            let index = self
                .model
                .character
                .characters
                .iter()
                .position(|c| c.id.eq_ignore_ascii_case(id));
            // (Not taking the old one's picks over.)
            self.kid_shown = None;
            self.load_character(index);
            return;
        }
        let Some(slots) = self.wardrobes.get_mut(&girl) else {
            return;
        };
        screen.screen.kid_appearance = slots
            .iter()
            .map(|s| (s.list, s.choices[s.picked].desc_id, s.color))
            .collect();
        if edits.is_empty() {
            return;
        }
        let worn = |slots: &[desa_viewer::source::KidSlot]| -> Vec<KidWorn> {
            slots.iter().map(|s| (s.picked, s.color)).collect()
        };
        let before = worn(slots);
        let kept = self.kid_kept.entry(girl).or_insert_with(|| worn(slots));
        let mut reshaped = false;
        // (A slot for the other kid: that kid, then the slot.)
        let mut switch_to = None;
        let cak_slot =
            |name: u32| (0..6).find(|i| qb::checksum(&format!("cak_profile{i}")) == name);
        for edit in edits {
            match edit {
                AppearanceEdit::Part(list, desc) => {
                    let Some(slot) = slots.iter_mut().find(|s| s.list == list) else {
                        continue;
                    };
                    // (Taken off: the choice that's nothing at all.)
                    let choice = match desc {
                        Some(desc) => slot.choices.iter().position(|c| c.desc_id == desc),
                        None => slot
                            .choices
                            .iter()
                            .position(|c| c.mesh.is_none() && c.swaps.is_empty()),
                    };
                    if let Some(choice) = choice {
                        slot.picked = choice;
                    }
                }
                AppearanceEdit::Color(list, color) => {
                    if let Some(slot) = slots.iter_mut().find(|s| s.list == list) {
                        slot.color = color;
                    }
                }
                AppearanceEdit::Keep(Some(name)) if cak_slot(name).is_some() => {
                    // Saved to a created skater's slot.
                    let i = cak_slot(name).unwrap_or(0);
                    // (With its name, as remembered with it.)
                    if let Some(display) = screen.screen.profile_name(name) {
                        self.settings.names.insert(format!("cak.{i}"), display);
                    }
                    let best = &mut self.settings.best;
                    best.retain(|k, _| !k.starts_with(&format!("cak.{i}.")));
                    best.insert(format!("cak.{i}.girl"), u32::from(girl));
                    if let Some(scale) = self.kid_scale {
                        best.insert(format!("cak.{i}.scale"), (scale * 1000.0).round() as u32);
                    }
                    for slot in slots.iter() {
                        let key = format!("cak.{i}.{}", slot.label);
                        best.insert(key.clone(), slot.picked as u32);
                        if let Some([h, s, v]) = slot.color {
                            best.insert(format!("{key}.c"), 1);
                            best.insert(format!("{key}.h"), h.round() as u32);
                            best.insert(format!("{key}.s"), s.round() as u32);
                            best.insert(format!("{key}.v"), v.round() as u32);
                        }
                    }
                }
                AppearanceEdit::Keep(_) => {
                    *kept = worn(slots);
                    self.kid_shape_kept = Some((self.kid_shape.clone(), self.kid_scale));
                }
                AppearanceEdit::Restore(Some(name)) if cak_slot(name).is_some() => {
                    // A created skater's slot put on (the boy's or the girl's
                    // kid, as it was saved).
                    let i = cak_slot(name).unwrap_or(0);
                    let best = &self.settings.best;
                    let Some(&saved_girl) = best.get(&format!("cak.{i}.girl")) else {
                        continue;
                    };
                    if (saved_girl == 1) != girl {
                        self.kid_pending_slot = Some(i);
                        switch_to = Some(saved_girl == 1);
                        continue;
                    }
                    for slot in slots.iter_mut() {
                        let key = format!("cak.{i}.{}", slot.label);
                        if let Some(&picked) = best.get(&key) {
                            if (picked as usize) < slot.choices.len() {
                                slot.picked = picked as usize;
                            }
                        }
                        slot.color = match best.get(&format!("{key}.c")) {
                            Some(1) => Some(["h", "s", "v"].map(|k| {
                                best.get(&format!("{key}.{k}")).map_or(0.0, |&v| v as f32)
                            })),
                            _ => None,
                        };
                    }
                    let scale = best
                        .get(&format!("cak.{i}.scale"))
                        .map(|&s| s as f32 / 1000.0);
                    reshaped |= scale != self.kid_scale;
                    self.kid_scale = scale;
                }
                AppearanceEdit::Reset(_) => {}
                AppearanceEdit::Restore(_) => {
                    for (slot, &(picked, color)) in slots.iter_mut().zip(kept.iter()) {
                        slot.picked = picked;
                        slot.color = color;
                    }
                    if let Some((shape, scale)) = &self.kid_shape_kept {
                        reshaped |= *shape != self.kid_shape || *scale != self.kid_scale;
                        self.kid_shape = shape.clone();
                        self.kid_scale = *scale;
                    }
                }
                AppearanceEdit::Apply(app) => {
                    // Each part as it says; those it doesn't name off (the
                    // skin as its body says, if it doesn't name one).
                    let body = app
                        .parts
                        .iter()
                        .find(|(list, ..)| *list == qb::checksum("body"))
                        .map(|&(_, desc, _)| desc);
                    for slot in slots.iter_mut() {
                        let entry = app.parts.iter().find(|(list, ..)| *list == slot.list);
                        let choice = match (entry, body) {
                            (Some(&(_, desc, _)), _) => slot
                                .choices
                                .iter()
                                .position(|c| c.desc_id == desc || c.alias == desc),
                            (None, Some(body)) if slot.label == "Skin" => {
                                slot.choices.iter().position(|c| c.alias == body)
                            }
                            (None, _) if slot.label == "Skin" => None,
                            (None, _) => slot
                                .choices
                                .iter()
                                .position(|c| c.mesh.is_none() && c.swaps.is_empty()),
                        };
                        if let Some(choice) = choice {
                            slot.picked = choice;
                        }
                        slot.color = entry.and_then(|&(.., color)| color);
                    }
                    let shape: Vec<(u32, Vec3)> =
                        app.shape.iter().map(|&(b, s)| (b, Vec3::from(s))).collect();
                    if !shape.is_empty() || app.scale.is_some() {
                        reshaped |= shape != self.kid_shape || app.scale != self.kid_scale;
                        self.kid_shape = shape;
                        self.kid_scale = app.scale;
                    }
                }
            }
        }
        if let Some(other) = switch_to {
            let id = if other { "kid_girl" } else { "kid" };
            let index = self
                .model
                .character
                .characters
                .iter()
                .position(|c| c.id.eq_ignore_ascii_case(id));
            self.load_character(index);
            return;
        }
        if worn(slots) == before && !reshaped {
            return;
        }
        // Worn: kept for next time and shown in the panel too.
        let sex = if girl { "f" } else { "m" };
        let key = format!("outfit.{sex}.scale");
        match self.kid_scale {
            Some(scale) => {
                self.settings
                    .best
                    .insert(key, (scale * 1000.0).round() as u32);
            }
            None => {
                self.settings.best.remove(&key);
            }
        }
        for (i, slot) in slots.iter().enumerate() {
            let key = format!("outfit.{sex}.{}", slot.label);
            let best = &mut self.settings.best;
            best.insert(key.clone(), slot.picked as u32);
            best.insert(format!("{key}.c"), u32::from(slot.color.is_some()));
            if let Some([h, s, v]) = slot.color {
                best.insert(format!("{key}.h"), h.round() as u32);
                best.insert(format!("{key}.s"), s.round() as u32);
                best.insert(format!("{key}.v"), v.round() as u32);
            }
            if let Some(view) = self.model.character.kid_slots.get_mut(i) {
                view.picked = slot.picked;
                view.color = slot.color;
            }
        }
        self.kid_from_menus = true;
        self.load_character(self.model.character.current);
    }

    /// The game's menus run on: their scripts (with the level's), sounds,
    /// and what they ask of the viewer (`unpausegame`: resume).
    fn update_screen(&mut self, dt: f32) {
        self.menu_pad();
        self.follow_skater_select();
        self.dress_kid_from_menus();
        // The level's objects they turned (the skater select's film
        // strip), for the level to do.
        if let (Some(screen), Some(level)) = (&mut self.screen, &mut self.level) {
            level
                .behaviour
                .object_commands
                .append(&mut screen.screen.object_commands);
        }
        let (Some(screen), Some(level)) = (&mut self.screen, &self.level) else {
            return;
        };
        screen.screen.update(level.behaviour.program(), dt);
        // The camera path they play (held at its end, as the free camera).
        if let Some(path) = screen.screen.camera.take() {
            let index = level
                .camera_paths
                .iter()
                .position(|(name, _)| qb::checksum(name) == path);
            if let Some(index) = index {
                self.model.camera_paths.playing = Some(index);
                self.model.camera_paths.time = 0.0;
                self.camera_hold = true;
            }
        }
        // Movies over the whole screen (the Options' Movies).
        for movie in std::mem::take(&mut screen.screen.movies) {
            self.movie_queue.push_back(movie);
        }
        // Paused with the game (its pause menu), and on again.
        if let Some(paused) = screen.screen.music_paused.take() {
            if let Some(audio) = &mut self.audio {
                audio.pause_music(paused);
            }
        }
        // The song they ask for.
        if let Some(track) = screen.screen.music.take() {
            // (Stopped: at once, the soundtrack on after.)
            if track.is_none() {
                if let Some(audio) = &mut self.audio {
                    audio.stop_music();
                }
            }
            self.menu_track = track.map(|t| (t, false));
        }
        // The menus' rumbles (the Controls menu's test), with it on.
        for (strong, level) in std::mem::take(&mut screen.screen.vibrations) {
            if let (Some(gilrs), true) = (self.gamepads.as_mut(), self.model.character.rumble) {
                self.rumble.menu_pulse(gilrs, strong, level);
            }
        }
        // The pause menu's bar (`SlicePause_1`) fitted round its items:
        // the game's script scales it by a `paused_bar_scale` the disc
        // doesn't set, and at its own size the items run to its edges.
        if let Some(menu) = screen.screen.size_of("pause_vmenu") {
            if let Some(bar) = screen.screen.image_size("slicepause_1") {
                let want = menu + glam::Vec2::new(32.0, 24.0);
                let scale = (want / bar.max(glam::Vec2::ONE)).max(glam::Vec2::ONE);
                screen.screen.set_sprites(
                    "SlicePause_1",
                    &qb::Value::Struct(vec![(
                        Some(qb::checksum("scale")),
                        qb::Value::Pair(scale.to_array()),
                    )]),
                    level.behaviour.program(),
                );
            }
        }
        // The Video Options' values, as they are.
        if screen.screen.exists("video_display_text") {
            let shown = self
                .video_pending
                .clone()
                .unwrap_or_else(|| self.settings.video.clone());
            let labels = video_labels(&shown, self.gpu.as_ref().map(|g| &g.window));
            if self.video_labels.as_ref() != Some(&labels) {
                let ids = [
                    "video_display_text",
                    "video_size_text",
                    "video_aspect_text",
                    "video_bars_text",
                    "video_vsync_text",
                    "video_fps_text",
                ];
                for (id, label) in ids.iter().zip(&labels) {
                    let props = qb::Value::Struct(vec![(
                        Some(qb::checksum("text")),
                        qb::Value::String(label.clone()),
                    )]);
                    screen.screen.set(id, &props, level.behaviour.program());
                }
                self.video_labels = Some(labels);
            }
        } else {
            // (Left without Apply: as they were.)
            self.video_labels = None;
            self.video_pending = None;
        }
        // The Advanced menu's look values, as they are.
        if screen.screen.exists("look_fog_text") {
            let labels = look_labels(&self.settings.look);
            if self.look_labels.as_ref() != Some(&labels) {
                let ids = [
                    "look_fog_text",
                    "look_exposure_text",
                    "look_warmth_text",
                    "look_tint_text",
                    "look_contrast_text",
                    "look_saturation_text",
                ];
                for (id, label) in ids.iter().zip(&labels) {
                    let props = qb::Value::Struct(vec![(
                        Some(qb::checksum("text")),
                        qb::Value::String(label.clone()),
                    )]);
                    screen.screen.set(id, &props, level.behaviour.program());
                }
                self.look_labels = Some(labels);
            }
        } else {
            self.look_labels = None;
        }
        let sounds = std::mem::take(&mut screen.screen.sounds);
        let requests = std::mem::take(&mut screen.screen.requests);
        // Player two's skater select: player one's pick kept.
        let (split, profile) = (screen.screen.split_screen, screen.screen.profile);
        // (And the halves as the game's Split Mode set them.)
        if split {
            self.side_by_side = screen.screen.side_by_side;
        }
        if profile != self.menu_profile {
            if profile == 1 {
                self.p1_pick = self.model.character.current;
            }
            self.menu_profile = profile;
        }
        self.model.character.game_menu = screen.screen.takes_pad();
        if let Some(audio) = self.audio.as_ref().filter(|_| self.model.character.sound) {
            for sound in sounds {
                audio.play_named(sound, 1.0);
            }
        }
        let c = qb::checksum;
        for (name, args) in requests {
            if name == c("viewer_exit_game") {
                // The main menu's Exit Game: closing, as the window would.
                self.exit_game = true;
                continue;
            }
            if name == c("viewer_look") {
                // The Advanced menu's look: a value turned, or reset.
                let option = args.get(c("option")).and_then(qb::Value::as_name);
                let dir = args.get(c("dir")).and_then(qb::Value::as_int).unwrap_or(1);
                self.change_look(option, dir);
                continue;
            }
            if name == c("viewer_video") {
                // The Video Options: a setting chosen or turned.
                let option = args.get(c("option")).and_then(qb::Value::as_name);
                let dir = args.get(c("dir")).and_then(qb::Value::as_int).unwrap_or(1);
                self.change_video(option, dir);
                continue;
            }
            if name == c("skip_track") {
                // The sound options' Skip Track: the next song.
                if let Some(audio) = &mut self.audio {
                    audio.stop_music();
                }
                continue;
            }
            if name == c("really_change_level") {
                // The level select: that level, skating there.
                let wanted = args.get(c("level")).and_then(qb::Value::as_name);
                let index = self
                    .model
                    .levels
                    .iter()
                    .position(|l| Some(c(&format!("load_{}", l.id))) == wanted);
                self.model.character.paused = false;
                if let Some(screen) = &mut self.screen {
                    screen.screen.clear();
                }
                // (Back to the stage, Exit to Stage: the main menu there.)
                let shop = index
                    .and_then(|i| self.model.levels.get(i))
                    .is_some_and(|l| l.id.eq_ignore_ascii_case("SkateShop"));
                if shop {
                    self.two_player = None;
                    self.open_main_menu();
                } else if let Some(i) = index {
                    if self.skating.is_some() {
                        self.toggle_skate();
                    }
                    // Two players' (player two's Ready): player two's
                    // skater the one shown, player one's back.
                    if split && profile == 1 {
                        self.two_player = self.model.character.current;
                        if let Some(screen) = &mut self.screen {
                            screen.screen.profile = 0;
                        }
                        self.menu_profile = 0;
                        if let Some(p1) = self.p1_pick.take() {
                            if Some(p1) != self.model.character.current {
                                self.load_character(Some(p1));
                            }
                        }
                    } else if !split {
                        self.two_player = None;
                    }
                    self.skate_on_load = true;
                    self.start_load(i);
                }
                continue;
            }
            if name == c("view_goals_menu_start_goal") {
                // View Goals: a goal chosen, started.
                let id = args.get(c("goal_ID")).and_then(qb::Value::as_name);
                let index = self.level.as_ref().and_then(|level| {
                    self.model.character.goals.iter().position(|(kind, _)| {
                        Some(desa_viewer::goals::goal_id(&level.id, kind)) == id
                    })
                });
                self.model.character.paused = false;
                if let Some(screen) = &mut self.screen {
                    screen.screen.destroy_id("pause_menu");
                    screen.screen.destroy_id("current_menu_anchor");
                }
                // (The score, letters and race goals the viewer plays its
                // own ways, as their pros start them.)
                let kind =
                    index.and_then(|i| self.model.character.goals.get(i).map(|g| g.0.clone()));
                match (index, kind.as_deref()) {
                    (_, Some("HighScore")) => self.start_run(Some(false)),
                    (_, Some("ProScore")) => self.start_run(Some(true)),
                    (_, Some("Skate")) => self.start_letters(),
                    (_, Some("Race")) => self.start_race(),
                    (Some(index), _) => self.start_goal(index),
                    _ => {}
                }
                continue;
            }
            if name == c("StartNetworkGame") {
                // Two players' GAME OPTIONS, Ready: both from the start, the
                // game type chosen on.
                self.start_versus();
                continue;
            }
            if name == c("RetryCurrentGoal") || name == c("end_current_goal_run") {
                // The pause menu's Retry Last Goal and End Current Goal.
                self.model.character.paused = false;
                if let Some(screen) = &mut self.screen {
                    screen.screen.destroy_id("pause_menu");
                    screen.screen.destroy_id("current_menu_anchor");
                }
                if name == c("RetryCurrentGoal") {
                    self.restart();
                } else if let Some(run) = self.goal_run.take() {
                    self.end_goal_run(run);
                    self.model.character.goal_progress = None;
                    self.model.character.run_clock = None;
                }
                continue;
            }
            if name == c("unpausegame") {
                // The menu away; the panel stays. (Not paused, nothing: the
                // level select's choice asks it too, on its way.)
                if self.model.character.paused {
                    self.model.character.paused = false;
                    if let Some(screen) = &mut self.screen {
                        screen.screen.destroy_id("pause_menu");
                        screen.screen.destroy_id("current_menu_anchor");
                    }
                }
            } else {
                // Saving, loading and two players aren't the viewer's.
                self.model.message = Some("That isn't in the viewer.".into());
                if let Some(screen) = &mut self.screen {
                    screen.screen.clear();
                    screen.screen.run(c("launch_main_menu"), Vec::new());
                }
            }
        }
    }

    /// Starts over from the start: the run or goal on, again, or skating
    /// from the level's start.
    fn restart(&mut self) {
        self.model.character.paused = false;
        self.quick_start = true;
        if let Some(index) = self.goal_run.as_ref().map(|r| r.index) {
            self.start_goal(index);
        } else if self.letters.is_some() {
            self.start_letters();
        } else if self.racing.is_some() {
            self.start_race();
        } else if self.run.is_some() {
            let goal = self.model.character.run_goal.as_ref().map(|(pro, ..)| *pro);
            self.start_run(goal);
        } else if let Some(level) = &self.level {
            self.placement = level.home;
            if self.skating.is_some() {
                self.toggle_skate();
            }
            self.toggle_skate();
        }
    }

    /// The replay's next camera.
    fn next_replay_camera(&mut self) {
        self.replay_camera = (self.replay_camera + 1) % REPLAY_CAMERAS.len();
        self.replay_eye = None;
        self.model.character.replay_camera = REPLAY_CAMERAS[self.replay_camera].0.to_string();
    }

    /// Shows the next frame of the replay, and at its end goes back to
    /// how the run finished.
    fn play_replay(&mut self, dt: f32) {
        let (Some(t), Some((skater, ..))) = (&mut self.replay, &mut self.skating) else {
            return;
        };
        *t += dt;
        let at = self.recording.partition_point(|f| f.time <= *t);
        let frame = if at >= self.recording.len() {
            self.replay = None;
            self.model.character.replaying = false;
            self.recording.last()
        } else {
            self.recording.get(at)
        };
        let Some(frame) = frame else { return };
        self.placement = frame.placement;
        self.skate_pose = Some(frame.pose.clone());
        self.blend_from = None;
        self.camera = frame.camera;
        // One of the game's replay cameras: hung off the skater by its
        // setting, behind and above, eased after it.
        let (_, setting) = REPLAY_CAMERAS[self.replay_camera];
        let setting = setting.and_then(|name| {
            let program = self.level.as_ref()?.behaviour.program();
            let camera = program.value(qb::checksum(name))?;
            let get = |k: &str| camera.get(qb::checksum(k)).and_then(qb::Value::as_f32);
            Some((get("behind")?, get("above")?, name))
        });
        if let Some((behind, above, name)) = setting {
            let forward = frame
                .placement
                .transform_vector3(Vec3::Z)
                .with_y(0.0)
                .normalize_or(Vec3::Z);
            let side = Vec3::Y.cross(forward);
            let way = if name.contains("Front") {
                forward
            } else if name.contains("Left") {
                side
            } else if name.contains("Right") {
                -side
            } else {
                -forward
            };
            let target = frame.position + Vec3::Y * 40.0;
            let wanted = target + way * behind * 12.0 + Vec3::Y * above * 12.0;
            let eye = match self.replay_eye {
                Some(eye) => eye.lerp(wanted, (dt * 4.0).min(1.0)),
                None => wanted,
            };
            self.replay_eye = Some(eye);
            self.camera = FlyCamera::looking_at(eye, target);
        } else {
            self.replay_eye = None;
        }
        skater.position = frame.position;
        skater.flipped = frame.flipped;
        let model = &mut self.model.character;
        model.score = frame.score;
        model.combo = frame.combo.clone();
        model.message = frame.message.clone();
        model.balance = frame.balance;
        model.special = frame.special;
        model.run_clock = Some(frame.clock);
    }

    fn skate(&mut self, dt: f32) {
        if self.replay.is_some() {
            self.play_replay(dt);
            return;
        }
        // Held still while a warp or goal's offered (`PauseSkaters`), a
        // goal's camera path plays, or paused.
        let told = self
            .screen
            .as_ref()
            .is_some_and(|s| s.screen.exists("goal_description_anchor"));
        if self.warp_offer.is_some() || told || self.cutscene || self.model.character.paused {
            // (The pad's still read, for Start.)
            let _ = self.pad_input();
            if let Some((at, time, stood)) = &mut self.warp_spin {
                // Gone: `warp_poof`.
                if *time < WARP_GONE && *time + dt >= WARP_GONE {
                    if let Some(level) = &mut self.level {
                        level.behaviour.voice_lines.push(qb::checksum("warp_poof"));
                    }
                }
                *time += dt;
                self.placement = warp_spin_placement(*at, *time, *stood);
            }
            return;
        }
        let pad = self.pad_input();
        // (Horse: held still while it's not their turn.)
        if self.versus_frozen(0) {
            self.skate_player2(dt);
            return;
        }
        // (Two players: the arrows are player two's.)
        let arrows = self.player2.is_none();
        // The pedestrians shown are solid: the skater bumps off them.
        let objects = self.model.show_objects;
        let goal_objects = self.model.show_goal_objects;
        if let Some(level) = &mut self.level {
            let crowd = &level.objects.crowd;
            let goal_crowd = &level.objects.goal_crowd;
            let mut obstacles: Vec<skate::world::Obstacle> = crowd
                .footprints()
                .filter(|_| objects)
                .chain(goal_crowd.footprints().filter(|_| goal_objects))
                // From a little below their feet, which don't always meet
                // the ground (the skater's knee-high line would pass under).
                .map(|(base, radius, height)| skate::world::Obstacle {
                    base: base - Vec3::Y * 24.0,
                    radius,
                    height: height + 24.0,
                })
                .collect();
            // The vehicles going round, to skitch on.
            let vehicles: Vec<(usize, skate::world::Vehicle)> = level
                .nodes
                .objects
                .iter()
                .enumerate()
                .filter(|(i, o)| {
                    o.kind == desa_viewer::nodes::ObjectKind::Vehicle && level.behaviour.alive(*i)
                })
                .filter_map(|(i, _)| {
                    let Some(Some(objects::Placed::Prop { goal, copy })) =
                        level.objects.placed.get(i)
                    else {
                        return None;
                    };
                    let props = if *goal {
                        &level.objects.goal_props
                    } else {
                        &level.objects.props
                    };
                    let (facing, speed) = level.behaviour.motion(i);
                    Some((
                        i,
                        skate::world::Vehicle {
                            position: level.behaviour.position(i),
                            forward: facing.with_y(0.0).normalize_or(Vec3::Z),
                            speed,
                            half_length: props.half_length(*copy),
                        },
                    ))
                })
                .collect();
            self.vehicle_objects = vehicles.iter().map(|(i, _)| *i).collect();
            // The moving objects' rails, where the objects are now.
            if let Some(world) = &mut level.world {
                let rails = &mut world.rails;
                let mut at = rails.segments.len() - rails.moving;
                for (i, local) in &level.object_rails {
                    let (position, turn) =
                        (level.behaviour.position(*i), level.behaviour.rotation(*i));
                    // (Not there, its rails far away.)
                    let alive = level.behaviour.alive(*i);
                    for &(a, b, _) in local {
                        rails.segments[at] = if alive {
                            Segment {
                                start: position + turn * a,
                                end: position + turn * b,
                            }
                        } else {
                            Segment {
                                start: Vec3::splat(1e7),
                                end: Vec3::splat(1e7) + Vec3::X,
                            }
                        };
                        at += 1;
                    }
                }
            }
            // The vehicles' own collision, where they are (gone while
            // they aren't).
            if let Some(world) = &mut level.world {
                for &(i, checksum) in &level.vehicle_collision {
                    if level.behaviour.alive(i) {
                        world.enable(checksum);
                        let placed = Mat4::from_rotation_translation(
                            level.behaviour.rotation(i),
                            level.behaviour.position(i),
                        );
                        world.place(checksum, Some(placed));
                    } else {
                        world.disable(checksum);
                    }
                }
            }
            // Small vehicles without collision of their own (not the Hub's
            // plane or Zurg's platform) are solid too: the skater bumps off
            // them.
            let own: HashSet<usize> = level.vehicle_collision.iter().map(|(i, _)| *i).collect();
            obstacles.extend(
                vehicles
                    .iter()
                    .filter(|(i, v)| v.half_length < 60.0 && !own.contains(i))
                    .map(|(_, v)| skate::world::Obstacle {
                        base: v.position - Vec3::Y * 24.0,
                        radius: v.half_length * 0.8,
                        height: 64.0,
                    }),
            );
            if let Some(world) = &mut level.world {
                world.vehicles = vehicles.into_iter().map(|(_, v)| v).collect();
                world.set_obstacles(obstacles);
            }
        }
        let (Some((skater, physics, chase)), Some(level), Some(character)) =
            (&mut self.skating, &self.level, &self.character)
        else {
            return;
        };
        let Some(world) = &level.world else { return };
        let typing = self
            .gpu
            .as_ref()
            .is_some_and(|g| g.egui_ctx.wants_keyboard_input());
        let held = |k| !typing && self.keys.contains(&k);
        let arrow = |k| arrows && held(k);
        let input = Input {
            push: held(KeyCode::KeyW) || arrow(KeyCode::ArrowUp),
            brake: held(KeyCode::KeyS) || arrow(KeyCode::ArrowDown),
            turn: f32::from(u8::from(held(KeyCode::KeyD) || arrow(KeyCode::ArrowRight)))
                - f32::from(u8::from(held(KeyCode::KeyA) || arrow(KeyCode::ArrowLeft))),
            crouch: held(KeyCode::Space),
            grind: held(KeyCode::KeyE),
            flip: held(KeyCode::KeyQ),
            grab: held(KeyCode::KeyF),
            revert: held(KeyCode::KeyR),
        };
        // A gamepad as well as the keys.
        let input = Input {
            push: input.push || pad.push,
            brake: input.brake || pad.brake,
            turn: if input.turn != 0.0 {
                input.turn
            } else {
                pad.turn
            },
            crouch: input.crouch || pad.crouch,
            grind: input.grind || pad.grind,
            flip: input.flip || pad.flip,
            grab: input.grab || pad.grab,
            revert: input.revert || pad.revert,
        };
        // The run's over: no more input, braking to a stop (`EndOfRun`).
        let ended = self.run.as_ref().is_some_and(|r| r.ending.is_some());
        let input = if ended {
            Input {
                brake: true,
                ..Input::default()
            }
        } else {
            input
        };
        skater.auto_kick = self.model.character.auto_kick && !ended;
        skater.wallride_anywhere = self.model.character.wallride_anywhere;
        // Repeats lose points, and combos add up, in a run or a goal, as
        // the game's single session and goals have it; free skating keeps
        // its best combo as the score.
        skater.degrade = self.run.is_some()
            || self.letters.is_some()
            || self.racing.is_some()
            || self.goal_run.is_some()
            || self.versus.is_some();
        skater.accumulate = skater.degrade;
        // The cheats.
        let cheats = self.model.character.cheats;
        skater.perfect_manual = cheats.perfect_manual;
        skater.perfect_rail = cheats.perfect_rail;
        skater.perfect_skitch = cheats.perfect_skitch;
        if cheats.always_special {
            skater.special_meter = 3000.0;
            skater.special = true;
        }
        let gravity = physics.air_gravity;
        // (The cheat, or the level's own low gravity zones setting its flag.)
        let moon_zone = level.behaviour.global_flag("CHEAT_MOON");
        if cheats.moon || moon_zone {
            physics.air_gravity *= self.moon_gravity;
        }
        let dt = if cheats.slomo {
            dt * self.slomo_speed
        } else {
            dt
        };
        let before = skater.position;
        skater.update(input, physics, world, dt);
        physics.air_gravity = gravity;
        // A gap landed: ticked off on the level's list, and kept.
        if let Some((name, _)) = skater.last_gap.take() {
            self.session.gaps += 1;
            if let Some(entry) = self
                .model
                .character
                .gap_list
                .iter_mut()
                .find(|g| g.0 == name)
            {
                if !entry.2 {
                    entry.2 = true;
                    self.settings
                        .gaps
                        .entry(level.id.clone())
                        .or_default()
                        .insert(name);
                    self.settings.save();
                }
            }
        }
        self.sparks.update(skater, dt);
        // Through a teleporter: its sound and message; into the water, a
        // splash where it went in.
        if skater.sounds.contains(&skate::skater::SkateSound::Teleport) {
            let effect = skater
                .last_teleport
                .and_then(|o| level.teleport_effects.get(&o))
                .cloned()
                .unwrap_or_default();
            let water = effect.sound == Some(qb::checksum("bigsplash"));
            if let (Some(sound), Some(audio)) = (
                effect.sound,
                self.audio.as_ref().filter(|_| self.model.character.sound),
            ) {
                audio.play_named(sound, 1.0);
            }
            if let Some(message) = effect.message.clone() {
                skater.message = Some((message, 1.5));
            }
            self.pending_creates.extend(effect.creates.iter().cloned());
            // (And the movie it puts on, or stops: the Hub's drive-in.)
            if let Some(movie) = effect.movie.clone() {
                self.teleport_movie = Some(movie);
            }
            if water {
                self.sparks.splash(before);
                // The camera stays a moment to see it, then cuts to the skater.
                self.splash_hold = Some((self.camera, SPLASH_HOLD));
            }
        }
        if let Some(gilrs) = self.gamepads.as_mut() {
            if self.model.character.rumble {
                self.rumble.update(gilrs, skater);
            } else {
                self.rumble.stop();
            }
        }
        if let Some(audio) = self.audio.as_mut().filter(|_| self.model.character.sound) {
            audio.update(skater);
        } else {
            if let Some(audio) = &mut self.audio {
                audio.stop();
            }
            skater.sounds.clear();
        }
        self.placement = skater.placement();
        self.model.character.balance = skater.balance_meter();
        self.model.character.score = skater.score;
        let p = skater.position;
        self.model.character.skate_status = format!(
            "at {:.0} {:.0} {:.0}\n{:?}, speed {:.0}{}{}{}",
            p.x,
            p.y,
            p.z,
            skater.action,
            skater.speed(),
            if skater.vert.is_some() {
                ", vert air"
            } else {
                ""
            },
            skater
                .balance_trick
                .as_ref()
                .filter(|_| skater.manual || skater.grind.is_some())
                .map_or(String::new(), |t| format!(", {}", t.name)),
            if skater.special { ", SPECIAL" } else { "" },
        );
        self.model.character.special = (skater.special_meter / 3000.0, skater.special);
        self.model.character.switch = skater.flipped;
        self.model.character.combo = combo_text(skater);
        // The skater's message, or for a few seconds the song that began.
        let song = self
            .now_playing
            .as_ref()
            .filter(|(_, at)| at.elapsed().as_secs_f32() < 4.0)
            .map(|(title, _)| format!("Now playing: {title}"));
        // (The game's panel announces songs itself.)
        let song = song.filter(|_| !self.model.character.game_hud);
        self.model.character.message = skater.message.as_ref().map(|(m, _)| m.clone()).or(song);

        // Animation (see `skate_pose`).
        let mut state = PoseState {
            animation: self.model.character.animation,
            shown: self.shown_animation,
            time: self.model.character.time,
            duration: self.model.character.duration,
            blend_from: self.blend_from.take(),
            pose: self.skate_pose.take(),
        };
        skate_pose(character, skater, &mut state, dt);
        let model = &mut self.model.character;
        model.animation = state.animation;
        model.time = state.time;
        model.duration = state.duration;
        self.shown_animation = state.shown;
        self.blend_from = state.blend_from;
        self.skate_pose = state.pose;

        // Chase camera, on the game's medium camera settings.
        chase.update(skater, physics, world, dt);
        // Looking round (our own: the game's look-around is in its code,
        // not read yet): the right stick or J/L swings the camera round the
        // skater and tilts it, springing back when let go; it stays out of
        // walls the camera can't see through.
        let keys =
            f32::from(u8::from(held(KeyCode::KeyL))) - f32::from(u8::from(held(KeyCode::KeyJ)));
        let wanted = glam::Vec2::new(
            (self.pad_look.x + keys).clamp(-1.0, 1.0) * LOOK_YAW,
            self.pad_look.y * LOOK_PITCH,
        );
        self.look = self.look.lerp(wanted, (dt * LOOK_RATE).min(1.0));
        let mut eye = chase.eye;
        if self.look.length() > 1e-3 {
            let back = chase.eye - chase.target;
            let turned = Quat::from_rotation_y(self.look.x) * back;
            let side = turned.cross(Vec3::Y).normalize_or(Vec3::X);
            eye = chase.target + Quat::from_axis_angle(side, self.look.y) * turned;
            if let Some(hit) = world.ray_requiring(chase.target, eye, 0x80) {
                eye = hit.point + (chase.target - hit.point).normalize_or_zero() * 8.0;
            }
        }
        self.camera = FlyCamera::looking_at(eye, chase.target);
        if let Some((camera, left)) = &mut self.splash_hold {
            *left -= dt;
            if *left > 0.0 {
                self.camera = *camera;
            } else {
                self.splash_hold = None;
            }
        }

        // A run is recorded as it's shown, to watch again.
        if let Some(run) = self.run.as_ref().filter(|r| !r.over) {
            let time = self.recording.last().map_or(0.0, |f| f.time + dt);
            let model = &self.model.character;
            self.recording.push(ReplayFrame {
                time,
                placement: self.placement,
                pose: self.skate_pose.clone().unwrap_or_default(),
                flipped: skater.flipped,
                position: skater.position,
                camera: self.camera,
                score: model.score,
                combo: model.combo.clone(),
                message: model.message.clone(),
                balance: model.balance,
                special: model.special,
                clock: run.left,
            });
        }
        self.skate_player2(dt);
    }

    /// Player two skates on: their keys and pad, the same physics, their
    /// animation and camera; and both players' HUDs.
    fn skate_player2(&mut self, dt: f32) {
        let typing = self
            .gpu
            .as_ref()
            .is_some_and(|g| g.egui_ctx.wants_keyboard_input());
        let keys = player2_keys(|k| !typing && self.keys.contains(&k));
        let input = either_input(keys, self.pad2);
        let character = &self.model.character;
        let cheats = character.cheats;
        let (auto_kick, wallride) = (character.auto_kick, character.wallride_anywhere);
        let (moon, slomo) = (self.moon_gravity, self.slomo_speed);
        let (frozen, versus) = (self.versus_frozen(1), self.versus.is_some());
        if let (Some(p2), Some(level), false) = (&mut self.player2, &self.level, frozen) {
            let Some(world) = &level.world else { return };
            let skater = &mut p2.skater;
            // (Two players' games add their combos up, as player one's.)
            skater.degrade = versus;
            skater.accumulate = versus;
            skater.auto_kick = auto_kick;
            skater.wallride_anywhere = wallride;
            skater.perfect_manual = cheats.perfect_manual;
            skater.perfect_rail = cheats.perfect_rail;
            skater.perfect_skitch = cheats.perfect_skitch;
            if cheats.always_special {
                skater.special_meter = 3000.0;
                skater.special = true;
            }
            let gravity = p2.physics.air_gravity;
            if cheats.moon || level.behaviour.global_flag("CHEAT_MOON") {
                p2.physics.air_gravity *= moon;
            }
            let dt = if cheats.slomo { dt * slomo } else { dt };
            skater.update(input, &p2.physics, world, dt);
            p2.physics.air_gravity = gravity;
            // (Gaps and sounds are player one's.)
            skater.last_gap = None;
            skater.sounds.clear();
            skate_pose(&p2.character, skater, &mut p2.pose, dt);
            p2.chase.update(skater, &p2.physics, world, dt);
            p2.camera = FlyCamera::looking_at(p2.chase.eye, p2.chase.target);
        }
        self.update_split_huds();
    }

    /// Each player's own HUD in their half (two players).
    fn update_split_huds(&mut self) {
        let title = |i: Option<usize>| {
            i.and_then(|i| self.model.character.characters.get(i))
                .map_or(String::new(), |c| c.title.clone())
        };
        let one = &self.model.character;
        self.model.split_huds = self.player2.as_ref().map(|p2| {
            let s = &p2.skater;
            [
                ui::PlayerHud {
                    name: format!("P1 {}", title(one.current)),
                    score: one.score,
                    combo: one.combo.clone(),
                    message: one.message.clone(),
                    special: one.special,
                    balance: one.balance,
                    switch: one.switch,
                    extra: None,
                    pointer: None,
                },
                ui::PlayerHud {
                    name: format!("P2 {}", title(Some(p2.index))),
                    score: s.score,
                    combo: combo_text(s),
                    message: s.message.as_ref().map(|(m, _)| m.clone()),
                    special: (s.special_meter / 3000.0, s.special),
                    balance: s.balance_meter(),
                    switch: s.flipped,
                    extra: None,
                    pointer: None,
                },
            ]
        });
        self.model.split_side_by_side = self.side_by_side;
    }

    /// Player two into the level beside player one (two players' free
    /// skate): at the level's nearest multiplayer start, else alongside.
    fn spawn_player2(&mut self) {
        self.player2 = None;
        self.versus = None;
        let Some(index) = self.two_player else { return };
        let Some((p1, ..)) = &self.skating else {
            return;
        };
        let (near, heading) = (p1.position, p1.heading);
        let Some(info) = self.model.character.characters.get(index).cloned() else {
            return;
        };
        let Some(data) = &mut self.data else { return };
        // The kid as dressed (or as the game first dresses them).
        let kid = match info.id.to_ascii_lowercase().as_str() {
            "kid" => Some(false),
            "kid_girl" => Some(true),
            _ => None,
        };
        let files = match kid {
            Some(girl) => {
                let outfit = match self.wardrobes.get(&girl) {
                    Some(slots) => Ok(slots.iter().map(|s| s.worn()).collect::<Vec<_>>()),
                    None => data
                        .kid_wardrobe_and_shape(girl)
                        .map(|(slots, _)| slots.iter().map(|s| s.worn()).collect()),
                };
                outfit.and_then(|outfit| data.load_kid(girl, &outfit))
            }
            None => data.load_character(&info.id),
        };
        let mut character = match files.and_then(|files| Character::from_files(&files)) {
            Ok(character) => character,
            Err(err) => {
                self.model.message = Some(format!("Player two: {err:#}"));
                return;
            }
        };
        let Some(level) = &mut self.level else { return };
        let Some(world) = &level.world else { return };
        let program = level.behaviour.program();
        // Their size, as the game's appearance has it.
        let name = match info.id.to_ascii_lowercase().as_str() {
            "kid_girl" => "girl".to_string(),
            id => id.to_string(),
        };
        character.scale = program
            .value(qb::checksum(&format!("appearance_{name}")))
            .and_then(|a| a.get(qb::checksum("scale")))
            .and_then(qb::Value::as_f32)
            .unwrap_or(1.0);
        let stats = if self.model.character.cheats.stats_13 {
            Stats([13.0; 10])
        } else {
            Stats::of(program, &info.id)
        };
        let mut physics = Physics::new(program, &stats);
        if let Some((_, setting)) =
            Physics::camera_choices(program).get(self.model.character.camera)
        {
            physics.set_camera(program, *setting);
        }
        let start = level
            .nodes
            .spawns
            .iter()
            .filter(|s| s.kind.eq_ignore_ascii_case("Multiplayer"))
            .filter(|s| s.position.distance(near) < 1500.0)
            .min_by(|a, b| {
                a.position
                    .distance(near)
                    .total_cmp(&b.position.distance(near))
            });
        let (position, heading) = match start {
            Some(spawn) => {
                let facing = spawn.facing();
                (spawn.position, facing.x.atan2(facing.z))
            }
            None => (
                near + Vec3::new(heading.cos(), 0.0, -heading.sin()) * 60.0,
                heading,
            ),
        };
        let mut skater = Skater::new(position, heading);
        let simplified = self.model.character.simplified;
        let mut tricks = TrickBook::with_controls(program, &info.id, &stats, simplified);
        skater.simplified = simplified;
        skater.anim_lengths = character
            .animations
            .iter()
            .map(|(name, a)| (name.clone(), a.duration))
            .collect();
        tricks.set_durations(|anim| {
            character
                .animations
                .iter()
                .find(|(name, _)| qb::checksum(name) == anim)
                .map(|(_, a)| a.duration)
        });
        skater.tricks = tricks;
        skater.place(position, heading, &physics, world);
        skater.spawns = level
            .nodes
            .spawns
            .iter()
            .map(|spawn| {
                let facing = spawn.facing();
                (spawn.position, facing.x.atan2(facing.z))
            })
            .collect();
        skater.teleports = desa_viewer::triggers::teleports(&level.nodes, program)
            .into_iter()
            .map(|(object, i)| {
                let spawn = &level.nodes.spawns[i];
                let facing = spawn.facing();
                (object, (spawn.position, facing.x.atan2(facing.z)))
            })
            .collect();
        skater.gap_triggers = desa_viewer::triggers::gaps(&level.nodes, program);
        let chase = ChaseCamera::behind(&skater, &physics);
        let camera = FlyCamera::looking_at(chase.eye, chase.target);
        level.renderer.set_character2(Some(&character.mesh));
        let idle = character.animation("StandIdle").unwrap_or(0);
        let standing = character.local_pose(idle, 0.0);
        self.player2 = Some(Player2 {
            index,
            character,
            skater,
            physics,
            chase,
            camera,
            pose: PoseState {
                animation: idle,
                shown: idle,
                pose: Some(standing),
                ..PoseState::default()
            },
        });
        // Both cameras behind their skaters, the HUDs up, from the start
        // (under a menu, before the first step).
        if let Some((_, _, chase)) = &self.skating {
            self.camera = FlyCamera::looking_at(chase.eye, chase.target);
        }
        self.update_split_huds();
        // The first time, the game's TWO PLAYERS menu (Start Game, Change
        // Level, Split Mode, Exit to Stage), paused.
        if std::mem::take(&mut self.split_first) {
            if let Some(screen) = &mut self.screen {
                self.model.character.paused = true;
                screen
                    .screen
                    .run(qb::checksum("create_end_run_menu"), Vec::new());
            }
        }
    }

    fn character_index(&self, id: &str) -> Option<usize> {
        self.model
            .character
            .characters
            .iter()
            .position(|c| c.id.eq_ignore_ascii_case(id))
    }

    /// Shows character `index` (in the panel's list), or none.
    fn load_character(&mut self, index: Option<usize>) {
        // A different character: stop skating the old one (its tricks and
        // stats belong to it), and go on skating with the new one from
        // where it was.
        let was_skating = self.skating.is_some();
        if was_skating {
            self.toggle_skate();
        }
        self.character = None;
        self.skating = None;
        self.model.character.skating = false;
        if let Some(audio) = &mut self.audio {
            audio.stop();
        }
        self.model.character.balance = None;
        self.model.character.combo = None;
        self.model.character.current = None;
        self.model.character.animations.clear();
        if let Some(level) = &mut self.level {
            level.renderer.set_character(None);
        }
        let Some(index) = index else {
            self.settings.last_character = None;
            self.settings.save();
            return;
        };
        let Some(data) = &mut self.data else { return };
        let info = self.model.character.characters[index].clone();
        // The created kid: dressed as picked (the game's outfit to start).
        let kid = match info.id.to_ascii_lowercase().as_str() {
            "kid" => Some(false),
            "kid_girl" => Some(true),
            _ => None,
        };
        let shown_kid = std::mem::take(&mut self.model.character.kid_slots);
        let kid_shown = self.kid_shown.take();
        let mut outfit = None;
        if let Some(girl) = kid {
            let sex = if girl { "f" } else { "m" };
            if !self.wardrobes.contains_key(&girl) {
                match data.kid_wardrobe_and_shape(girl) {
                    Ok((mut slots, shape)) => {
                        self.kid_shape = shape
                            .into_iter()
                            .map(|(bone, s)| (bone, Vec3::from(s)))
                            .collect();
                        // (Its size, if a crew kid's was kept.)
                        self.kid_scale = self
                            .settings
                            .best
                            .get(&format!("outfit.{sex}.scale"))
                            .map(|&s| s as f32 / 1000.0);
                        for slot in &mut slots {
                            let key = format!("outfit.{sex}.{}", slot.label);
                            let saved = |k: &str| self.settings.best.get(&format!("{key}.{k}"));
                            if let Some(&i) = self.settings.best.get(&key) {
                                if (i as usize) < slot.choices.len() {
                                    slot.picked = i as usize;
                                }
                            }
                            match saved("c") {
                                Some(1) => {
                                    if let (Some(&h), Some(&s), Some(&v)) =
                                        (saved("h"), saved("s"), saved("v"))
                                    {
                                        slot.color = Some([h as f32, s as f32, v as f32]);
                                    }
                                }
                                Some(_) => slot.color = None,
                                None => {}
                            }
                        }
                        self.wardrobes.insert(girl, slots);
                    }
                    Err(e) => eprintln!("the kid's wardrobe: {e:#}"),
                }
            }
            if let Some(slots) = self.wardrobes.get_mut(&girl) {
                // (The panel's picks, when it was showing this kid.)
                if kid_shown == Some(girl) && shown_kid.len() == slots.len() {
                    for (shown, slot) in shown_kid.iter().zip(slots.iter_mut()) {
                        let key = format!("outfit.{sex}.{}", slot.label);
                        if slot.picked != shown.picked {
                            slot.picked = shown.picked;
                            self.settings.best.insert(key.clone(), shown.picked as u32);
                        }
                        if slot.color != shown.color {
                            slot.color = shown.color;
                            let best = &mut self.settings.best;
                            best.insert(format!("{key}.c"), u32::from(shown.color.is_some()));
                            if let Some([h, s, v]) = shown.color {
                                best.insert(format!("{key}.h"), h.round() as u32);
                                best.insert(format!("{key}.s"), s.round() as u32);
                                best.insert(format!("{key}.v"), v.round() as u32);
                            }
                        }
                    }
                }
                // (Changed from the panel: kept, as the game's menus keep
                // a part accepted.)
                if kid_shown == Some(girl)
                    && shown_kid.len() == slots.len()
                    && !std::mem::take(&mut self.kid_from_menus)
                {
                    self.kid_kept
                        .insert(girl, slots.iter().map(|s| (s.picked, s.color)).collect());
                }
                outfit = Some(slots.iter().map(|s| s.worn()).collect::<Vec<_>>());
                self.kid_shown = Some(girl);
                self.model.character.kid_slots = slots
                    .iter()
                    .map(|s| ui::KidSlotView {
                        label: s.label.to_string(),
                        choices: s.choices.iter().map(|c| c.name.clone()).collect(),
                        colorable: s.choices.iter().map(|c| c.colorable).collect(),
                        picked: s.picked,
                        color: s.color,
                    })
                    .collect();
            }
        }
        let files = match (kid, &outfit) {
            (Some(girl), Some(outfit)) => data.load_kid(girl, outfit),
            _ => data.load_character(&info.id),
        };
        let loaded = files
            .and_then(|files| Character::from_files(&files))
            .with_context(|| format!("could not load {}", info.title));
        match loaded {
            Ok(character) => {
                // Not in the Skate Shop: it's the game's menu room (loaded
                // with `InitSkaterHeaps`), open at the sides, not a level.
                let shop = self
                    .model
                    .current
                    .and_then(|i| self.model.levels.get(i))
                    .is_some_and(|l| l.id.eq_ignore_ascii_case("SkateShop"));
                let skateable = !shop && self.level.as_ref().is_some_and(|l| l.world.is_some());
                let model = &mut self.model.character;
                model.animations = character
                    .animations
                    .iter()
                    .map(|(n, _)| n.clone())
                    .collect();
                // (On the shop's stage, its pose: `SkateStageAi`'s.)
                let idles = if shop {
                    ["Stage_Idle", "StandIdle"]
                } else {
                    ["StandIdle", "Stage_Idle"]
                };
                model.animation = idles
                    .iter()
                    .find_map(|name| character.animation(name))
                    .unwrap_or(0);
                model.duration = character.animations[model.animation].1.duration;
                model.time = 0.0;
                model.current = Some(index);
                model.can_blink = character.blink.is_some();
                model.can_skate = skateable;
                // The spawn marker would stand right through the character.
                if self.character.is_none() {
                    self.model.show_spawns = false;
                }
                self.shown_animation = model.animation;
                if let Some(level) = &mut self.level {
                    level.renderer.set_character(Some(&character.mesh));
                }
                self.character = Some(character);
                self.scale_character();
                self.settings.last_character = Some(info.id);
                self.settings.save();
                if was_skating && skateable {
                    self.toggle_skate();
                }
            }
            Err(err) => self.model.message = Some(format!("{err:#}")),
        }
    }

    /// Advances the character's animation, uploads the posed mesh and
    /// shows the eyes for `clock` seconds since the viewer started.
    fn animate(&mut self, dt: f32, clock: f32) {
        // (A two-player game left from its menus, paused or not.)
        self.versus_check();
        let (Some(character), Some(level)) = (&self.character, &mut self.level) else {
            return;
        };
        // Two players: the screen split, player two's half from their
        // camera; player two posed.
        let solo = matches!(self.versus, Some(versus::Versus::Horse(_)));
        level.renderer.split = self
            .player2
            .as_ref()
            .filter(|_| !solo)
            .map(|p2| renderer::Split {
                camera: p2.camera,
                side_by_side: self.side_by_side,
            });
        if let Some(p2) = &self.player2 {
            if let Some(pose) = &p2.pose.pose {
                level
                    .renderer
                    .pose_character2(&p2.character.pose_local_mirrored(
                        pose,
                        p2.skater.placement(),
                        p2.skater.flipped,
                    ));
            }
        }
        let model = &mut self.model.character;
        model.animation = model.animation.min(character.animations.len() - 1);
        if model.animation != self.shown_animation {
            self.shown_animation = model.animation;
            model.time = 0.0;
        }
        model.duration = character.animations[model.animation].1.duration;
        // Skating poses the character itself (blending animations).
        if let (Some((skater, ..)), Some(pose)) = (&self.skating, &self.skate_pose) {
            level
                .renderer
                .pose_character(&character.pose_local_mirrored(
                    pose,
                    self.placement,
                    skater.flipped,
                ));
            let mut shadow = level
                .world
                .as_ref()
                .map(|world| skater_shadow(skater, world))
                .unwrap_or_default();
            // The animals' and toy cars' shadows too, those near.
            if let Some(world) = &level.world {
                for o in world.obstacles() {
                    let feet = o.base + Vec3::Y * 24.0;
                    if feet.distance(self.camera.position) < SHADOW_RANGE {
                        shadow.extend(blob_shadow(feet, o.radius * 0.8, world));
                    }
                }
            }
            if let (Some(p2), Some(world)) = (&self.player2, &level.world) {
                shadow.extend(skater_shadow(&p2.skater, world));
            }
            shadow.extend(self.sparks.vertices(self.camera.position));
            // The way back to the Hub, glowing.
            // (Our own ring where the level has no particles for it.)
            for portal in level.portals.iter().filter(|p| {
                p.strip.is_none()
                    && !p.warp.particle.is_some_and(|name| {
                        desa_viewer::particles::Particles::has_emitter(&level.nodes, name)
                    })
            }) {
                shadow.extend(portal_ring(
                    portal.warp.position,
                    clock,
                    self.camera.position,
                ));
            }
            level.renderer.set_shadow(&shadow);
            if let Some(blink) = character.blink {
                let eyes = if model.blink {
                    blink.texture_at(clock)
                } else {
                    blink.eyes
                };
                level.renderer.swap_character_texture(blink.eyes, eyes);
            }
            return;
        }
        level.renderer.set_shadow(&[]);
        if model.playing && model.duration > 0.0 {
            // Everything loops here, even one-off moves like an ollie.
            model.time = (model.time + dt * model.speed) % model.duration;
        }
        model.time = model.time.clamp(0.0, model.duration);
        level
            .renderer
            .pose_character(&character.pose(model.animation, model.time, self.placement));
        if let Some(blink) = character.blink {
            let eyes = if model.blink {
                blink.texture_at(clock)
            } else {
                blink.eyes
            };
            level.renderer.swap_character_texture(blink.eyes, eyes);
        }
    }

    /// Pushes panel settings into the renderer.
    fn sync_view(&mut self) {
        let Some(level) = &mut self.level else { return };
        level.renderer.show_sky = self.model.show_sky;
        level.renderer.min_light = self.model.brighten;
        level.renderer.look = self.settings.look;
        level.renderer.collision_view = self.model.collision;
        let wanted = (self.model.show_rails, self.model.show_spawns);
        if level.markers != wanted {
            level
                .renderer
                .set_markers(&level.nodes.geometry(wanted.0, wanted.1));
            level.markers = wanted;
        }
    }

    fn pick_data(&mut self, folder: bool) {
        let dialog = rfd::FileDialog::new().set_title("Open Disney's Extreme Skate Adventure");
        let picked = if folder {
            dialog.pick_folder()
        } else {
            dialog
                .add_filter("GameCube disc image", &["iso", "gcm"])
                .pick_file()
        };
        if let Some(path) = picked {
            self.open_data(&path);
            if let Some(i) = (!self.model.levels.is_empty()).then_some(0) {
                self.start_load(i);
            }
        }
    }

    fn init(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        let attributes = Window::default_attributes()
            .with_title(if self.game {
                "Disney's Extreme Skate Adventure"
            } else {
                "DESA Map Viewer"
            })
            .with_inner_size(LogicalSize::new(1400.0, 850.0));
        let window = Arc::new(event_loop.create_window(attributes)?);
        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(window.clone())?;
        let (adapter, device, queue) =
            pollster::block_on(request_device(&instance, Some(&surface)))?;
        let caps = surface.get_capabilities(&adapter);
        // A non-sRGB target: the game's colors and egui both expect one.
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .unwrap_or(caps.formats[0]);
        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
        };
        surface.configure(&device, &config);

        let egui_ctx = egui::Context::default();
        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            None,
            Some(device.limits().max_texture_dimension_2d as usize),
        );
        let egui_renderer =
            egui_wgpu::Renderer::new(&device, format, egui_wgpu::RendererOptions::default());
        self.gpu = Some(Gpu {
            window,
            surface,
            config,
            device,
            queue,
            egui_ctx,
            egui_state,
            egui_renderer,
        });
        self.last_frame = Instant::now();
        self.apply_video();
        // Sound from the start: the opening movies', and the menus' (the
        // level's sounds, if it's in already; else when it is).
        if self.level.is_some() {
            self.start_audio();
        } else if self.audio.is_none() {
            self.audio = audio::Audio::new();
        }
        Ok(())
    }

    /// The Video Options to the window: its mode and size, and vsync.
    fn apply_video(&mut self) {
        use winit::window::Fullscreen;
        let Some(gpu) = &mut self.gpu else { return };
        let video = self.settings.video.clone();
        let window = &gpu.window;
        match video.mode {
            WindowMode::Windowed => {
                window.set_fullscreen(None);
                if let Some((w, h)) = video.size {
                    let _ = window.request_inner_size(winit::dpi::PhysicalSize::new(w, h));
                }
            }
            WindowMode::Borderless => {
                window.set_fullscreen(Some(Fullscreen::Borderless(window.current_monitor())));
            }
            WindowMode::Fullscreen => {
                // The screen's mode of that size (else its own), at its
                // fastest refresh.
                let monitor = window.current_monitor();
                let want = video
                    .size
                    .or_else(|| monitor.as_ref().map(|m| (m.size().width, m.size().height)));
                let mode = monitor.as_ref().and_then(|m| {
                    m.video_modes()
                        .filter(|v| Some((v.size().width, v.size().height)) == want)
                        .max_by_key(|v| (v.bit_depth(), v.refresh_rate_millihertz()))
                });
                window.set_fullscreen(Some(match mode {
                    Some(mode) => Fullscreen::Exclusive(mode),
                    None => Fullscreen::Borderless(monitor),
                }));
            }
        }
        let present = if video.vsync {
            wgpu::PresentMode::AutoVsync
        } else {
            wgpu::PresentMode::AutoNoVsync
        };
        if gpu.config.present_mode != present {
            gpu.config.present_mode = present;
            gpu.surface.configure(&gpu.device, &gpu.config);
        }
    }

    /// The bars' picture, loaded when it's another (and let go, black or
    /// with the picture the window's shape).
    fn update_bars(&mut self) {
        let want = Some(&self.settings.video.bars)
            .filter(|b| **b != Bars::Black && self.settings.video.aspect.is_some());
        if self.bars.as_ref().map(|(b, _)| b) == want {
            return;
        }
        let (Some(want), Some(gpu)) = (want.cloned(), &self.gpu) else {
            self.bars = None;
            return;
        };
        let image = self.data.as_mut().and_then(|d| bars_image(d, &want));
        self.bars = image.map(|image| {
            let texture = gpu
                .egui_ctx
                .load_texture("bars", image, egui::TextureOptions::LINEAR);
            (want, texture)
        });
        if self.bars.is_none() {
            // (Not there: black, without trying each frame.)
            self.settings.video.bars = Bars::Black;
        }
    }

    /// A Video Options setting turned (`dir` 1 the next value, -1 the one
    /// before), shown till Apply puts them to the window and keeps them.
    /// An Advanced menu look value turned (`dir` 1 more, -1 less), or
    /// all reset; shown at once and kept.
    fn change_look(&mut self, option: Option<u32>, dir: i32) {
        let c = qb::checksum;
        let look = &mut self.settings.look;
        let step = |value: &mut f32, by: f32, min: f32, max: f32| {
            *value = ((*value + by * dir.signum() as f32) / by).round() * by;
            *value = value.clamp(min, max);
        };
        match option {
            Some(o) if o == c("look_reset") => *look = desa_viewer::post::Look::default(),
            Some(o) if o == c("look_fog") => {
                let at = FOG_STEPS
                    .iter()
                    .position(|(_, d)| *d == look.fog_distance)
                    .unwrap_or(0) as i32;
                let to = (at + dir.signum()).rem_euclid(FOG_STEPS.len() as i32) as usize;
                look.fog_distance = FOG_STEPS[to].1;
            }
            Some(o) if o == c("look_exposure") => step(&mut look.exposure, 0.25, -3.0, 3.0),
            Some(o) if o == c("look_warmth") => step(&mut look.warmth, 0.1, -1.0, 1.0),
            Some(o) if o == c("look_tint") => step(&mut look.tint, 0.1, -1.0, 1.0),
            Some(o) if o == c("look_contrast") => step(&mut look.contrast, 0.1, 0.5, 2.0),
            Some(o) if o == c("look_saturation") => step(&mut look.saturation, 0.1, 0.0, 2.0),
            _ => return,
        }
        self.settings.save();
        self.sync_view();
    }

    fn change_video(&mut self, option: Option<u32>, dir: i32) {
        let c = qb::checksum;
        if option == Some(c("video_apply")) {
            if let Some(video) = self.video_pending.take() {
                self.settings.video = video;
                self.next_frame = Instant::now();
                self.apply_video();
                self.settings.save();
            }
            return;
        }
        let window = self.gpu.as_ref().map(|g| g.window.clone());
        let arts = self
            .data
            .as_ref()
            .map(GameData::loading_screens)
            .unwrap_or_default();
        let video = self
            .video_pending
            .get_or_insert_with(|| self.settings.video.clone());
        let turn = |at: usize, len: usize| (at as i32 + dir).rem_euclid(len as i32) as usize;
        match option {
            Some(o) if o == c("video_display") => {
                let at = WindowMode::ALL
                    .iter()
                    .position(|&m| m == video.mode)
                    .unwrap_or(0);
                video.mode = WindowMode::ALL[turn(at, WindowMode::ALL.len())];
            }
            Some(o) if o == c("video_size") => {
                let sizes = video_sizes(video.mode, window.as_ref());
                if sizes.is_empty() || video.mode == WindowMode::Borderless {
                    return;
                }
                let now = shown_size(video, window.as_ref());
                // (From where it is, or the nearest if it's between.)
                let at = match sizes.iter().position(|&s| s == now) {
                    Some(at) => turn(at, sizes.len()),
                    None => {
                        let area = u64::from(now.0) * u64::from(now.1);
                        let bigger = sizes
                            .iter()
                            .position(|&(w, h)| u64::from(w) * u64::from(h) > area);
                        match (bigger, dir > 0) {
                            (Some(i), true) => i,
                            (Some(i), false) => i.saturating_sub(1),
                            (None, _) => sizes.len() - 1,
                        }
                    }
                };
                video.size = Some(sizes[at]);
            }
            Some(o) if o == c("video_aspect") => {
                let at = ASPECTS.iter().position(|&a| a == video.aspect).unwrap_or(0);
                video.aspect = ASPECTS[turn(at, ASPECTS.len())];
            }
            Some(o) if o == c("video_bars") => {
                let mut choices = vec![Bars::Black];
                choices.extend(arts.into_iter().map(Bars::Art));
                if settings::custom_bars().is_some() {
                    choices.push(Bars::Custom);
                }
                let at = choices.iter().position(|b| *b == video.bars).unwrap_or(0);
                video.bars = choices[turn(at, choices.len())].clone();
            }
            Some(o) if o == c("video_vsync") => video.vsync = !video.vsync,
            Some(o) if o == c("video_fps") => {
                let at = FRAME_LIMITS
                    .iter()
                    .position(|&f| f == video.frame_limit)
                    .unwrap_or(0);
                video.frame_limit = FRAME_LIMITS[turn(at, FRAME_LIMITS.len())];
            }
            _ => {}
        }
    }

    fn update(&mut self, dt: f32) {
        // The keys drive the skater instead while skating (paused, they fly
        // the camera round for pictures).
        if (self.skating.is_some() && !self.model.character.paused) || self.game {
            return;
        }
        let typing = self
            .gpu
            .as_ref()
            .is_some_and(|g| g.egui_ctx.wants_keyboard_input());
        if typing {
            return;
        }
        // (Q and E turn the character in the game's skater select.)
        let selecting = self
            .screen
            .as_ref()
            .is_some_and(|s| s.screen.exists("select_skater_anchor"));
        let pressed = |k| {
            self.keys.contains(&k) && !(selecting && matches!(k, KeyCode::KeyQ | KeyCode::KeyE))
        };
        let (forward, right) = (self.camera.forward(), self.camera.right());
        let mut direction = Vec3::ZERO;
        for (key, dir) in [
            (KeyCode::KeyW, forward),
            (KeyCode::KeyS, -forward),
            (KeyCode::KeyD, right),
            (KeyCode::KeyA, -right),
            (KeyCode::KeyE, Vec3::Y),
            (KeyCode::Space, Vec3::Y),
            (KeyCode::KeyQ, -Vec3::Y),
            (KeyCode::ControlLeft, -Vec3::Y),
        ] {
            if pressed(key) {
                direction += dir;
            }
        }
        // Flying or looking around takes over from a camera path.
        if self.model.camera_paths.playing.is_some() && (direction != Vec3::ZERO || self.looking) {
            self.stop_camera_path();
            return;
        }
        let boost = if pressed(KeyCode::ShiftLeft) || pressed(KeyCode::ShiftRight) {
            5.0
        } else {
            1.0
        };
        self.camera.position += direction.normalize_or_zero() * self.model.speed * boost * dt;
    }

    fn redraw(&mut self) {
        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;
        self.update(dt);
        self.skate(dt);
        self.update_cutscene();
        self.say_lines();
        // Paused (or a goal's camera path playing): the clocks stop too.
        if !self.model.character.paused && !self.cutscene {
            self.update_run(dt);
            self.update_letters(dt);
            self.update_race(dt);
            self.update_versus(dt);
            self.update_goal_run(dt);
            self.update_breakables();
            self.update_bouncies(dt);
            self.update_collecting();
            self.update_skitch();
            self.update_records(dt);
            self.update_portals(dt);
            self.update_pros();
        }
        self.update_map();
        self.apply_camera();
        self.apply_controls();
        if self.frame % 30 == 0 {
            self.update_progress();
            self.update_goals_won();
        }
        self.frame = self.frame.wrapping_add(1);
        self.update_music();
        self.play(dt);
        self.sync_view();
        let clock = self.started.elapsed().as_secs_f32();
        self.animate(dt, clock);
        if let Some(level) = &mut self.level {
            // The scripts see the skater (birds fly off as it comes near).
            let skater = self.skating.as_ref().map(|(s, ..)| s.position);
            level.behaviour.set_skater(skater);
            level.behaviour.skater_state = self
                .skating
                .as_ref()
                .map(|(s, ..)| skater_state(s))
                .unwrap_or_default();
            level.update_objects(
                self.model.show_objects,
                self.model.show_goal_objects,
                clock,
                dt,
            );
            level.apply_creates();
            level
                .particles
                .update(level.behaviour.program(), dt, self.camera.position);
            let batches = particle_batches(
                &level.particles,
                &level.particle_textures,
                self.camera.position,
            );
            level.renderer.set_particles(&batches);
            // The sounds they played, quieter further from the skater.
            let sounds = std::mem::take(&mut level.behaviour.sounds);
            if let (Some(audio), Some(skater)) = (
                self.audio.as_mut().filter(|_| self.model.character.sound),
                skater,
            ) {
                for (sound, at, volume) in sounds {
                    let near = (1.0 - at.distance(skater) / OBJECT_SOUND_RANGE).clamp(0.0, 1.0);
                    if near > 0.0 {
                        audio.play_named(sound, volume.min(1.5) * near);
                    }
                }
            }
        }
        let p = self.camera.position;
        self.model.camera_text = format!(
            "x {:.0}  y {:.0}  z {:.0}\nyaw {:.0}  pitch {:.0}",
            p.x,
            p.y,
            p.z,
            self.camera.yaw.to_degrees(),
            self.camera.pitch.to_degrees()
        );

        self.update_movie();
        self.hud_dt = dt;
        self.update_game_hud();
        self.update_screen(dt);
        self.update_screen_movie();
        let arrows = self.hud_arrow_angles();
        let notice = self.game_notice();
        let aspect = self.settings.video.aspect.map(|(w, h)| w as f32 / h as f32);
        if let Some(level) = &mut self.level {
            level.renderer.aspect = aspect;
        }
        self.update_bars();
        let bars = self.bars.as_ref().map(|(_, t)| t.clone());
        let Some(gpu) = &mut self.gpu else { return };
        set_picture_aspect(&gpu.egui_ctx, aspect);
        let raw = gpu.egui_state.take_egui_input(&gpu.window);
        let model = &mut self.model;
        let movie = &mut self.movie;
        let screen = &mut self.screen;
        let mut actions = Vec::new();
        let output = gpu.egui_ctx.run(raw, |ctx| {
            paint_bars(ctx, bars.as_ref());
            actions = ui::draw(ctx, model);
            if let Some(screen) = screen {
                screen.paint(ctx);
            }
            paint_hud_arrows(ctx, &arrows);
            if let Some(notice) = &notice {
                ui::notice(ctx, notice);
            }
            if let Some(playing) = movie {
                if playing.draw(ctx) {
                    actions.push(ui::Action::SkipMovie);
                }
            }
        });
        gpu.egui_state
            .handle_platform_output(&gpu.window, output.platform_output.clone());

        let frame = match gpu.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Outdated | wgpu::SurfaceError::Lost) => {
                gpu.surface.configure(&gpu.device, &gpu.config);
                return;
            }
            Err(err) => {
                eprintln!("frame skipped: {err}");
                return;
            }
        };
        let view = frame.texture.create_view(&Default::default());
        let (w, h) = (gpu.config.width, gpu.config.height);
        let time = self.started.elapsed().as_secs_f32();
        // A picture of the frame, as shown, into the Pictures folder.
        if std::mem::take(&mut self.photo) {
            let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("photo"),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: gpu.config.format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            paint(
                &gpu.device,
                &gpu.queue,
                &mut gpu.egui_renderer,
                &gpu.egui_ctx,
                self.level.as_mut().map(|l| &mut l.renderer),
                &self.camera,
                time,
                &target.create_view(&Default::default()),
                (w, h),
                // Paused (photo mode): the picture without the panel and HUD.
                if self.model.character.paused {
                    egui::FullOutput::default()
                } else {
                    output.clone()
                },
            );
            let path = photo_path();
            self.model.message = Some(
                match path
                    .parent()
                    .map(std::fs::create_dir_all)
                    .transpose()
                    .and_then(|_| {
                        renderer::save_png(&gpu.device, &gpu.queue, &target, w, h, &path)
                            .map_err(std::io::Error::other)
                    }) {
                    Ok(_) => format!("Saved {}", path.display()),
                    Err(e) => format!("Couldn't save a picture: {e}"),
                },
            );
        }
        paint(
            &gpu.device,
            &gpu.queue,
            &mut gpu.egui_renderer,
            &gpu.egui_ctx,
            self.level.as_mut().map(|l| &mut l.renderer),
            &self.camera,
            time,
            &view,
            (w, h),
            output,
        );
        frame.present();

        for action in actions {
            match action {
                ui::Action::OpenDisc => self.pick_data(false),
                ui::Action::OpenFolder => self.pick_data(true),
                ui::Action::LoadLevel(i) => self.start_load(i),
                ui::Action::GoToSpawn(i) => self.go_to_spawn(i),
                ui::Action::PlayCameraPath(i) => self.play_camera_path(i),
                ui::Action::StopCameraPath => self.stop_camera_path(),
                ui::Action::PlayMovie(i) => {
                    if let Some(name) = self.model.movies.get(i).cloned() {
                        self.stop_movie();
                        self.movie_queue.clear();
                        self.movie_queue.push_back(name);
                    }
                }
                ui::Action::SkipMovie => self.stop_movie(),
                ui::Action::MainMenu => self.open_main_menu(),
                ui::Action::SetStartMenu(on) => {
                    self.model.start_menu = on;
                    self.settings
                        .best
                        .insert("start_menu".into(), u32::from(on));
                    self.settings.save();
                }
                ui::Action::SetIntro(on) => {
                    self.model.intro = on;
                    self.settings.best.insert("intro".into(), u32::from(on));
                    self.settings.save();
                }
                ui::Action::ResetCamera => {
                    self.stop_camera_path();
                    if let Some(level) = &self.level {
                        self.camera = level.start;
                    }
                }
                ui::Action::LoadCharacter(i) => self.load_character(i),
                ui::Action::LookAtCharacter => self.camera = camera_facing(self.placement),
                ui::Action::ToggleSkate => self.toggle_skate(),
                ui::Action::StartRun(goal) => self.start_run(goal),
                ui::Action::StartLetters => self.start_letters(),
                ui::Action::StartRace => self.start_race(),
                ui::Action::StartGoal(i) => self.start_goal(i),
                ui::Action::Warp => self.take_warp(),
                ui::Action::Pause => self.toggle_pause(),
                ui::Action::Restart => self.restart(),
                ui::Action::StayHere => self.stay_here(),
                ui::Action::TakeGoal => self.take_goal(),
                ui::Action::NotNow => self.not_now(),
                ui::Action::Replay => {
                    if !self.recording.is_empty() {
                        self.replay = Some(0.0);
                        self.model.character.replaying = true;
                        if let Some(audio) = &mut self.audio {
                            audio.stop();
                        }
                    }
                }
                ui::Action::StopReplay => self.replay = Some(f32::INFINITY),
                ui::Action::ReplayCamera => self.next_replay_camera(),
            }
        }
        // Load after the "Loading" message has been on screen for a frame.
        if let Some((index, frames)) = self.pending {
            if frames >= 1 {
                self.pending = None;
                self.finish_load(index);
            } else {
                self.pending = Some((index, frames + 1));
            }
        }
    }

    fn set_looking(&mut self, looking: bool) {
        self.looking = looking;
        if let Some(gpu) = &self.gpu {
            let mode = if looking {
                CursorGrabMode::Confined
            } else {
                CursorGrabMode::None
            };
            let _ = gpu
                .window
                .set_cursor_grab(mode)
                .or_else(|_| gpu.window.set_cursor_grab(CursorGrabMode::Locked));
            gpu.window.set_cursor_visible(!looking);
        }
    }

    fn key_pressed(&mut self, code: KeyCode, repeat: bool) {
        if code == KeyCode::F12 && !repeat {
            self.photo = true;
            return;
        }
        // A movie playing: Esc skips them all, any other key this one.
        if self.movie.is_some() {
            if !repeat {
                if code == KeyCode::Escape {
                    self.movie_queue.clear();
                }
                self.stop_movie();
            }
            return;
        }
        // The game's menu up: the keys are its pad.
        if self.model.character.game_menu {
            if let Some(screen) = &mut self.screen {
                use desa_viewer::screen::Pad;
                let pad = match code {
                    KeyCode::ArrowUp | KeyCode::KeyW => Some(Pad::Up),
                    KeyCode::ArrowDown | KeyCode::KeyS => Some(Pad::Down),
                    KeyCode::ArrowLeft | KeyCode::KeyA => Some(Pad::Left),
                    KeyCode::ArrowRight | KeyCode::KeyD => Some(Pad::Right),
                    KeyCode::Enter | KeyCode::Space => Some(Pad::Choose),
                    KeyCode::Escape | KeyCode::Backspace => Some(Pad::Back),
                    KeyCode::KeyP => Some(Pad::Start),
                    _ => None,
                };
                if let Some(pad) = pad {
                    screen.screen.pad(pad);
                }
            }
            return;
        }
        // Paused: P resumes, Esc stops skating (resumes, in the game alone).
        if self.model.character.paused {
            match code {
                KeyCode::KeyP if !repeat => self.toggle_pause(),
                KeyCode::Escape if !repeat && self.game => self.toggle_pause(),
                KeyCode::Escape if !repeat => self.toggle_skate(),
                _ => {}
            }
            return;
        }
        if code == KeyCode::KeyM && !repeat && self.skating.is_some() && !self.game {
            self.model.character.show_map = !self.model.character.show_map;
            return;
        }
        if code == KeyCode::KeyC && !repeat && self.replay.is_some() {
            self.next_replay_camera();
            return;
        }
        if code == KeyCode::KeyC && !repeat && self.skating.is_some() {
            self.next_camera();
            return;
        }
        if code == KeyCode::KeyP && !repeat && self.skating.is_some() {
            self.toggle_pause();
            return;
        }
        // A goal's camera path: any key skips it.
        if self.cutscene && !repeat {
            self.stop_camera_path();
            return;
        }
        // At a goal's pro: Enter starts it, Esc not now.
        if self.goal_offer.is_some() {
            match code {
                KeyCode::Enter | KeyCode::NumpadEnter if !repeat => {
                    self.take_goal();
                    return;
                }
                KeyCode::Escape if !repeat => {
                    self.not_now();
                    return;
                }
                _ => {}
            }
        }
        // At a warp: Enter goes through, Esc stays.
        if self.warp_offer.is_some() {
            match code {
                KeyCode::Enter | KeyCode::NumpadEnter if !repeat => self.take_warp(),
                KeyCode::Escape if !repeat => self.stay_here(),
                _ => {}
            }
            return;
        }
        // The game alone: Esc pauses, and the viewer's own keys do nothing.
        if self.game {
            if code == KeyCode::Escape && !repeat && self.skating.is_some() {
                self.toggle_pause();
            }
            return;
        }
        match code {
            KeyCode::Escape => {
                self.set_looking(false);
                if self.skating.is_some() {
                    self.toggle_skate();
                }
            }
            KeyCode::F1 if !repeat => self.model.panel_open = !self.model.panel_open,
            KeyCode::KeyK if !repeat && self.model.has_collision => {
                self.model.collision = self.model.collision.next();
            }
            KeyCode::Tab if !repeat => {
                let count = self.level.as_ref().map_or(0, |l| l.nodes.spawns.len());
                if count > 0 {
                    let index = self.next_spawn % count;
                    self.go_to_spawn(index);
                    // Skating: the skater goes there too.
                    if let (Some((skater, physics, chase)), Some(level)) =
                        (&mut self.skating, &self.level)
                    {
                        if let (Some(spawn), Some(world)) =
                            (level.nodes.spawns.get(index), &level.world)
                        {
                            let facing = spawn.facing();
                            skater.place(spawn.position, facing.x.atan2(facing.z), physics, world);
                            *chase = ChaseCamera::behind(skater, physics);
                        }
                    }
                }
            }
            // R reverts while skating.
            KeyCode::KeyR if !repeat && self.skating.is_none() => {
                self.stop_camera_path();
                if let Some(level) = &self.level {
                    self.camera = level.start;
                }
            }
            KeyCode::KeyP if !repeat => {
                self.model.character.playing = !self.model.character.playing;
            }
            KeyCode::BracketLeft | KeyCode::BracketRight => {
                let count = self.model.character.animations.len();
                if count > 0 {
                    let step = if code == KeyCode::BracketRight {
                        1
                    } else {
                        count - 1
                    };
                    let model = &mut self.model.character;
                    model.animation = (model.animation + step) % count;
                }
            }
            _ => {}
        }
    }
}

/// Draws the level (or a plain background) and the panel into `view`.
#[allow(clippy::too_many_arguments)]
fn paint(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    egui_renderer: &mut egui_wgpu::Renderer,
    egui_ctx: &egui::Context,
    level: Option<&mut Renderer>,
    camera: &FlyCamera,
    time: f32,
    view: &wgpu::TextureView,
    (width, height): (u32, u32),
    output: egui::FullOutput,
) {
    let mut encoder = device.create_command_encoder(&Default::default());
    match level {
        Some(renderer) => renderer.encode(&mut encoder, view, width, height, camera, time),
        None => {
            encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("background"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.08,
                            g: 0.09,
                            b: 0.12,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
        }
    }

    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [width, height],
        pixels_per_point: output.pixels_per_point,
    };
    for (id, delta) in &output.textures_delta.set {
        egui_renderer.update_texture(device, queue, *id, delta);
    }
    let jobs = egui_ctx.tessellate(output.shapes, output.pixels_per_point);
    let extra = egui_renderer.update_buffers(device, queue, &mut encoder, &jobs, &screen);
    {
        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("panel"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        egui_renderer.render(&mut pass.forget_lifetime(), &jobs, &screen);
    }
    queue.submit(extra.into_iter().chain([encoder.finish()]));
    for id in &output.textures_delta.free {
        egui_renderer.free_texture(id);
    }
}

impl ApplicationHandler for App<'_> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_none() {
            if let Err(err) = self.init(event_loop) {
                self.error = Some(err);
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let (consumed, over_panel) = match &mut self.gpu {
            Some(gpu) => {
                let response = gpu.egui_state.on_window_event(&gpu.window, &event);
                (response.consumed, gpu.egui_ctx.is_pointer_over_area())
            }
            None => (false, false),
        };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(gpu) = &mut self.gpu {
                    gpu.config.width = size.width.max(1);
                    gpu.config.height = size.height.max(1);
                    gpu.surface.configure(&gpu.device, &gpu.config);
                }
            }
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::KeyboardInput { event, .. } => {
                let PhysicalKey::Code(code) = event.physical_key else {
                    return;
                };
                match event.state {
                    ElementState::Pressed if !consumed => {
                        self.keys.insert(code);
                        self.key_pressed(code, event.repeat);
                    }
                    ElementState::Pressed => {}
                    ElementState::Released => {
                        self.keys.remove(&code);
                    }
                }
            }
            WindowEvent::MouseInput {
                button: MouseButton::Right,
                state,
                ..
            } => {
                let pressed = state == ElementState::Pressed;
                if !pressed || !over_panel {
                    self.set_looking(pressed);
                }
            }
            WindowEvent::MouseWheel { delta, .. } if !over_panel => {
                let steps = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 / 50.0,
                };
                self.model.speed = (self.model.speed * 1.25f32.powf(steps)).clamp(50.0, 20_000.0);
            }
            WindowEvent::Focused(false) => {
                self.keys.clear();
                self.set_looking(false);
            }
            _ => {}
        }
    }

    fn device_event(&mut self, _event_loop: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta: (dx, dy) } = event {
            if self.looking {
                self.camera.turn(
                    dx as f32 * MOUSE_SENSITIVITY,
                    -dy as f32 * MOUSE_SENSITIVITY,
                );
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.exit_game {
            event_loop.exit();
            return;
        }
        if self.gpu.is_none() {
            return;
        }
        // Under a frame limit, the next frame when it's due.
        let limit = self.settings.video.frame_limit;
        if limit > 0 {
            let period = std::time::Duration::from_secs_f64(1.0 / f64::from(limit));
            let now = Instant::now();
            if now < self.next_frame {
                std::thread::sleep(self.next_frame - now);
            }
            // (Not catching up more than a frame behind.)
            let behind = Instant::now().checked_sub(period).unwrap_or(now);
            self.next_frame = self.next_frame.max(behind) + period;
        }
        if let Some(gpu) = &self.gpu {
            gpu.window.request_redraw();
        }
    }
}

/// The sizes the Video Options offer: the screen's modes (in a window,
/// those that fit on it), smallest first.
fn video_sizes(mode: WindowMode, window: Option<&Arc<Window>>) -> Vec<(u32, u32)> {
    let monitor = window.and_then(|w| w.current_monitor());
    let mut sizes: Vec<(u32, u32)> = match &monitor {
        Some(m) => m
            .video_modes()
            .map(|v| (v.size().width, v.size().height))
            .collect(),
        None => vec![(1280, 720), (1600, 900), (1920, 1080)],
    };
    if mode == WindowMode::Windowed {
        if let Some(m) = &monitor {
            let screen = m.size();
            sizes.retain(|&(w, h)| w <= screen.width && h <= screen.height);
        }
    }
    sizes.retain(|&(w, h)| w >= 640 && h >= 480);
    sizes.sort_by_key(|&(w, h)| (u64::from(w) * u64::from(h), w));
    sizes.dedup();
    sizes
}

/// The size the window is (or full screen, will be): as set, else as it is.
fn shown_size(video: &Video, window: Option<&Arc<Window>>) -> (u32, u32) {
    let monitor = window.and_then(|w| w.current_monitor());
    let screen = || {
        monitor
            .as_ref()
            .map(|m| (m.size().width, m.size().height))
            .unwrap_or((0, 0))
    };
    match (video.mode, video.size) {
        (WindowMode::Borderless, _) | (WindowMode::Fullscreen, None) => screen(),
        (_, Some(size)) => size,
        (WindowMode::Windowed, None) => window
            .map(|w| (w.inner_size().width, w.inner_size().height))
            .unwrap_or((0, 0)),
    }
}

/// The Video Options' items' text, as the settings are.
/// The Advanced menu's fog: names and metres to half-hidden.
const FOG_STEPS: [(&str, f32); 5] = [
    ("Off", 0.0),
    ("Light", 400.0),
    ("Medium", 200.0),
    ("Heavy", 100.0),
    ("Thick", 50.0),
];

/// The Advanced menu's look values, as shown.
fn look_labels(look: &desa_viewer::post::Look) -> [String; 6] {
    let fog = FOG_STEPS
        .iter()
        .find(|(_, d)| *d == look.fog_distance)
        .map_or_else(
            || format!("{} m", look.fog_distance),
            |(n, _)| n.to_string(),
        );
    let signed = |v: f32| {
        if v.abs() < 0.005 {
            "0".to_string()
        } else {
            format!("{v:+.2}")
                .trim_end_matches('0')
                .trim_end_matches('.')
                .to_string()
        }
    };
    let percent = |v: f32| format!("{}%", (v * 100.0).round() as i32);
    [
        format!("Fog: {fog}"),
        format!("Exposure: {}", signed(look.exposure)),
        format!("Warmth: {}", signed(look.warmth)),
        format!("Tint: {}", signed(look.tint)),
        format!("Contrast: {}", percent(look.contrast)),
        format!("Saturation: {}", percent(look.saturation)),
    ]
}

/// `--look`'s NAME=VALUE pairs over the default look.
fn parse_look(text: &str) -> Result<desa_viewer::post::Look> {
    let mut look = desa_viewer::post::Look::default();
    for part in text.split(',').filter(|p| !p.trim().is_empty()) {
        let (key, value) = part
            .split_once('=')
            .context("--look takes NAME=VALUE pairs")?;
        let value: f32 = value.trim().parse().context("--look's numbers")?;
        anyhow::ensure!(
            settings::set_look(&mut look, key.trim(), value),
            "--look: no {} (fog, exposure, warmth, tint, contrast, saturation)",
            key.trim()
        );
    }
    Ok(look)
}

fn video_labels(video: &Video, window: Option<&Arc<Window>>) -> [String; 6] {
    let (w, h) = shown_size(video, window);
    [
        format!("Display: {}", video.mode.name()),
        format!("Size: {w} x {h}"),
        match video.aspect {
            None => "Aspect Ratio: Auto".to_string(),
            Some((w, h)) => format!("Aspect Ratio: {w}:{h}"),
        },
        format!(
            "Bars: {}",
            match &video.bars {
                Bars::Black => "Black".to_string(),
                Bars::Custom => "My Picture".to_string(),
                Bars::Art(name) => art_title(name),
            }
        ),
        format!("VSync: {}", if video.vsync { "On" } else { "Off" }),
        match video.frame_limit {
            0 => "Frame Limit: Off".to_string(),
            n => format!("Frame Limit: {n}"),
        },
    ]
}

/// Renders the first frame of a level, panel included, without a window.
/// With a character, the camera looks at it instead.
fn screenshot(data_path: &Path, args: &Args, out: &Path) -> Result<()> {
    let mut data = GameData::open(data_path)?;
    let bars = args.bars.as_deref().and_then(|b| {
        let bars = match b {
            "custom" => Bars::Custom,
            name => Bars::Art(name.to_string()),
        };
        bars_image(&mut data, &bars)
    });
    let levels = data.levels();
    let index = match args.level.as_deref() {
        Some(id) => levels
            .iter()
            .position(|l| l.id.eq_ignore_ascii_case(id))
            .with_context(|| format!("no level named {id}"))?,
        None => 0,
    };
    let instance = wgpu::Instance::default();
    let (_adapter, device, queue) = pollster::block_on(request_device(&instance, None))?;
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let (mut loaded, stats) = load_level(&mut data, &levels[index], &device, &queue, format)?;
    let mut hud_arrows = Vec::new();
    // A spawn marker would stand right where the character does.
    loaded.renderer.set_markers(
        &loaded
            .nodes
            .geometry(!args.clean, !args.clean && args.character.is_none()),
    );

    let mut settings = Settings {
        read_only: true,
        ..Settings::default()
    };
    for part in args.kid_color.split(',').filter(|p| !p.is_empty()) {
        let v: Vec<&str> = part.split(':').collect();
        anyhow::ensure!(v.len() == 4, "--kid-color is Part:hue:saturation:value");
        for sex in ["m", "f"] {
            let key = format!("outfit.{sex}.{}", v[0]);
            settings.best.insert(format!("{key}.c"), 1);
            for (k, value) in ["h", "s", "v"].iter().zip(&v[1..]) {
                settings.best.insert(
                    format!("{key}.{k}"),
                    value.parse().context("--kid-color's numbers")?,
                );
            }
        }
    }
    let mut app = App::new(&mut settings);
    app.model.levels = levels;
    app.model.data_path = Some(data_path.display().to_string());
    app.model.current = Some(index);
    app.model.stats = Some(stats);
    app.model.has_collision = loaded.renderer.has_collision();
    app.model.set_spawns(&loaded.nodes.spawns);
    app.model.camera_text = loaded.start.describe();
    let mut camera = loaded.start;
    app.model.show_goal_objects = args.goal_objects;
    // Let the objects' scripts run up to --time, a 30th of a second at a time.
    let steps = (args.time * 30.0).ceil() as usize;
    for step in 0..=steps {
        let t = (step as f32 / 30.0).min(args.time);
        loaded.update_objects(
            true,
            args.goal_objects,
            t,
            if step == 0 { 0.0 } else { 1.0 / 30.0 },
        );
    }
    if let Some(name) = &args.object {
        let node = loaded
            .nodes
            .objects
            .iter()
            .find(|o| o.label.eq_ignore_ascii_case(name))
            .with_context(|| format!("no object named {name}"))?;
        let (eye, target) = objects::view_of(node);
        // Follow it to where its script has moved it by --time.
        let index = loaded
            .nodes
            .objects
            .iter()
            .position(|o| std::ptr::eq(o, node))
            .unwrap();
        let moved = loaded.behaviour.position(index) - node.position;
        camera = FlyCamera::looking_at(eye + moved, target + moved);
    }
    if let Some(c) = args.camera {
        camera = c;
    }

    if let Some(id) = &args.character {
        app.model.character.characters = data.characters();
        if args.skate > 0.0 || !args.menu_keys.is_empty() {
            app.screen = ScreenUi::load(&mut data);
        }
        app.data = Some(data);
        let i = app
            .character_index(id)
            .with_context(|| format!("no character named {id}"))?;
        app.load_character(Some(i));
        let character = app
            .character
            .as_ref()
            .with_context(|| app.model.message.clone().unwrap_or_default())?;
        if let Some(name) = &args.animation {
            app.model.character.animation = character
                .animation(name)
                .with_context(|| format!("{id} has no animation named {name}"))?;
        }
        app.shown_animation = app.model.character.animation;
        app.model.character.time = args.time;
        loaded.renderer.set_character(Some(&character.mesh));
        app.placement = match args.spawn {
            Some(i) => placement_at(loaded.nodes.spawns.get(i).with_context(|| {
                format!("the level has {} spawn points", loaded.nodes.spawns.len())
            })?),
            None => loaded.home,
        };
        app.placement = Mat4::from_translation(Vec3::Y * args.lift) * app.placement;
        camera = camera_around(app.placement, args.orbit, args.distance, args.camera_height);
        // (An explicit camera still wins.)
        if let Some(c) = args.camera {
            camera = c;
        }
        app.model.character.map_image = loaded
            .minimap
            .as_ref()
            .map(|m| egui::ColorImage::from_rgba_unmultiplied([m.width, m.height], &m.pixels));
        app.level = Some(loaded);
        app.scale_character();
        // The game's main menu, and the presses given it.
        if !args.menu_keys.is_empty() {
            if let (Some(screen), Some(level)) = (&mut app.screen, &app.level) {
                screen.screen.level = Some(qb::checksum(&format!("load_{}", level.id)));
                screen
                    .screen
                    .run(qb::checksum("launch_main_menu"), Vec::new());
            }
            for _ in 0..60 {
                app.update_game_hud();
                app.update_screen(1.0 / 60.0);
            }
            let mut clock = 0.0;
            for key in args.menu_keys.chars() {
                use desa_viewer::screen::Pad;
                let pad = match key {
                    'u' => Some(Pad::Up),
                    'd' => Some(Pad::Down),
                    'l' => Some(Pad::Left),
                    'r' => Some(Pad::Right),
                    'c' => Some(Pad::Choose),
                    'b' => Some(Pad::Back),
                    'w' => None,
                    _ => continue,
                };
                if let (Some(pad), Some(screen)) = (pad, &mut app.screen) {
                    // (As the app gives it: only when the menus take it.)
                    if screen.screen.takes_pad() {
                        screen.screen.pad(pad);
                    } else {
                        println!("menu: {key} not taken (no menu has the focus)");
                    }
                }
                for _ in 0..30 {
                    app.update_game_hud();
                    app.update_screen(1.0 / 60.0);
                    // (The level's objects too: the film strip turning.)
                    clock += 1.0 / 60.0;
                    let (objects, goal_objects) =
                        (app.model.show_objects, app.model.show_goal_objects);
                    if let Some(level) = &mut app.level {
                        level.update_objects(objects, goal_objects, clock, 1.0 / 60.0);
                        level.apply_creates();
                    }
                }
            }
            // (DESA_SCREEN_DUMP: the game's screen elements, for a look.)
            if std::env::var_os("DESA_SCREEN_DUMP").is_some() {
                if let Some(screen) = &app.screen {
                    eprintln!("{}", screen.screen.describe());
                    eprintln!("unknown {:x?}", screen.screen.unknown);
                }
            }
            // A second of the screen's movie (it keeps its own time).
            for _ in 0..30 {
                std::thread::sleep(std::time::Duration::from_millis(33));
                app.update_screen_movie();
            }
            // The camera path the menus play, at its end (held there), for
            // the picture (unless --camera).
            if let (Some(index), Some(level)) = (app.model.camera_paths.playing, &mut app.level) {
                if let Some((name, path)) = level.camera_paths.get(index) {
                    let end = path_camera(path, path_length(path));
                    println!("menu: camera {name} ends at {:.0}", end.position);
                    if args.camera.is_none() {
                        level.renderer.scripted_camera = Some(end);
                    }
                }
            }
            if let Some(level) = &app.level {
                println!("menu: song {:?}", app.menu_track.as_ref().map(|(t, _)| t));
                println!("menu: movies waiting {:?}", app.movie_queue);
                println!(
                    "menu: character {:?}, level {}",
                    app.model
                        .character
                        .current
                        .and_then(|i| app.model.character.characters.get(i))
                        .map(|c| c.id.clone()),
                    level.id
                );
                println!(
                    "menu: message {:?}, loading {:?}, pending {:?}",
                    app.model.message,
                    app.model.loading.clone(),
                    app.pending
                );
            }
        }
        if args.skate > 0.0 {
            if let Some(from) = &args.skate_from {
                let v: Vec<f32> = from
                    .split(',')
                    .map(|x| x.trim().parse())
                    .collect::<Result<_, _>>()
                    .context("--skate-from is x,y,z,heading")?;
                anyhow::ensure!(v.len() == 4, "--skate-from is x,y,z,heading");
                app.placement = Mat4::from_translation(Vec3::new(v[0], v[1], v[2]))
                    * Mat4::from_rotation_y(v[3].to_radians());
            }
            if let Some(id) = &args.two_player {
                app.two_player = Some(
                    app.character_index(id)
                        .with_context(|| format!("no character named {id}"))?,
                );
                app.side_by_side = args.side_by_side;
                // (The TWO PLAYERS menu first only for --pause-keys to
                // work through.)
                app.split_first =
                    !args.pause_keys.is_empty() && !args.pause && args.versus.is_none();
                if let Some(screen) = &mut app.screen {
                    screen.screen.split_screen = true;
                    screen.screen.side_by_side = args.side_by_side;
                }
            }
            for cheat in &args.cheat {
                let c = &mut app.model.character.cheats;
                match cheat.as_str() {
                    "perfect_manual" => c.perfect_manual = true,
                    "perfect_rail" => c.perfect_rail = true,
                    "perfect_skitch" => c.perfect_skitch = true,
                    "always_special" => c.always_special = true,
                    "moon" => c.moon = true,
                    "slomo" => c.slomo = true,
                    "stats_13" => c.stats_13 = true,
                    other => anyhow::bail!("no cheat called {other}"),
                }
            }
            if let Some(camera) = args.chase_camera {
                app.model.character.camera = camera;
            }
            if args.letters {
                let from = app.placement.transform_point3(Vec3::ZERO);
                app.start_letters();
                // From --skate-from, if given, rather than the goal's start
                // (dropped there).
                if let (Some(_), Some((skater, ..))) = (&args.skate_from, &mut app.skating) {
                    skater.position = from;
                    skater.on_ground = false;
                }
                if let Some(run) = &app.letters {
                    let level = app.level.as_ref().unwrap();
                    for (letter, o) in "SKATE".chars().zip(run.objects) {
                        let p = level.behaviour.position(o);
                        println!("letter {letter} at {:.0} {:.0} {:.0}", p.x, p.y, p.z);
                    }
                }
            } else if args.race {
                app.start_race();
            } else if let Some(kind) = &args.goal {
                // (Loaded straight here: the goal list too.)
                let level = app.level.as_ref().unwrap();
                app.model.character.goals =
                    desa_viewer::goals::level_goals(level.behaviour.program(), &level.id)
                        .into_iter()
                        .map(|g| (g.kind, g.text))
                        .collect();
                let index = app
                    .model
                    .character
                    .goals
                    .iter()
                    .position(|(k, _)| k.eq_ignore_ascii_case(kind))
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "no goal of type {kind} here (the goals: {:?})",
                            app.model
                                .character
                                .goals
                                .iter()
                                .map(|(k, _)| k)
                                .collect::<Vec<_>>()
                        )
                    })?;
                let from = app.placement.transform_point3(Vec3::ZERO);
                let facing = app.placement.transform_vector3(Vec3::Z);
                app.start_goal(index);
                // From --skate-from, if given, rather than the goal's start.
                if let (Some(_), Some((skater, ..))) = (&args.skate_from, &mut app.skating) {
                    skater.position = from;
                    skater.heading = facing.x.atan2(facing.z);
                    skater.on_ground = false;
                }
                // Where the things it counts are (those with a script).
                for b in &app.level.as_ref().unwrap().bouncies {
                    if b.spec.collide_script.is_some() {
                        let c = b.centre;
                        println!("bouncy at {:.0} {:.0} {:.0}", c.x, c.y, c.z);
                    }
                }
                if let Some((skater, ..)) = &mut app.skating {
                    skater
                        .gap_scripts
                        .extend(args.goal_script.iter().map(|s| qb::checksum(s)));
                }
            } else if let Some(goal) = &args.score_goal {
                app.start_run(Some(goal == "pro"));
            } else {
                app.toggle_skate();
            }
            if let (Some(left), Some(goal)) = (args.run, &mut app.goal_run) {
                // A goal from its scripts, with this long left.
                if goal.left.is_some() {
                    goal.left = Some(left);
                }
            } else if let Some(left) = args.run {
                // A run (or the score goal started), with this long left.
                match &mut app.run {
                    Some(run) => run.left = left,
                    None => {
                        app.run = Some(Run {
                            left,
                            ending: None,
                            over: false,
                            goal: None,
                            won: false,
                        })
                    }
                }
            }
            if let Some(game) = &args.versus {
                let game = match game.to_ascii_lowercase().as_str() {
                    "scorechallenge" | "score" => "scorechallenge",
                    "king" => "king",
                    "horse" => "Horse",
                    other => anyhow::bail!("--versus {other}: scorechallenge, king or horse"),
                };
                if let Some(screen) = &mut app.screen {
                    screen.screen.game_type = qb::checksum(game);
                }
                app.start_versus();
            }
            let script = args
                .skate_keys
                .as_deref()
                .map(parse_skate_keys)
                .transpose()?;
            if script.is_none() {
                app.keys.insert(KeyCode::KeyW);
                if app.two_player.is_some() {
                    app.keys.insert(KeyCode::ArrowUp);
                }
            }
            let places = args
                .skate_place
                .iter()
                .map(|p| -> Result<(f32, Vec3, f32, f32)> {
                    let (at, spot) = p
                        .split_once('@')
                        .context("--skate-place is seconds@x,y,z,heading")?;
                    let v: Vec<f32> = spot
                        .split(',')
                        .map(|x| x.trim().parse())
                        .collect::<Result<_, _>>()
                        .context("--skate-place is seconds@x,y,z,heading")?;
                    anyhow::ensure!(
                        v.len() == 4 || v.len() == 5,
                        "--skate-place is seconds@x,y,z,heading[,speed]"
                    );
                    let speed = v.get(4).copied().unwrap_or(0.0);
                    Ok((
                        at.parse()?,
                        Vec3::new(v[0], v[1], v[2]),
                        v[3].to_radians(),
                        speed,
                    ))
                })
                .collect::<Result<Vec<_>>>()?;
            let steps = (args.skate * 60.0).round() as usize;
            for step in 0..steps {
                let now = step as f32 / 60.0;
                for &(at, spot, heading, speed) in &places {
                    if (at - now).abs() < 0.5 / 60.0 {
                        if let Some((skater, ..)) = &mut app.skating {
                            skater.position = spot;
                            skater.heading = heading;
                            skater.velocity = Vec3::new(heading.sin(), 0.0, heading.cos()) * speed;
                            skater.on_ground = false;
                        }
                    }
                }
                for &(at, key, down) in script.iter().flatten() {
                    if (at - now).abs() < 0.5 / 60.0 {
                        if key == KeyCode::Enter {
                            // (A on the game's screen, or taking the warp
                            // or goal offered.)
                            if down && app.warp_offer.is_some() {
                                app.take_warp();
                            } else if down && app.goal_offer.is_some() {
                                app.take_goal();
                            } else if let (true, Some(screen)) = (down, &mut app.screen) {
                                screen.screen.pad(desa_viewer::screen::Pad::Choose);
                            }
                        } else if down {
                            app.keys.insert(key);
                        } else {
                            app.keys.remove(&key);
                        }
                    }
                }
                app.skate(1.0 / 60.0);
                app.update_run(1.0 / 60.0);
                app.update_letters(1.0 / 60.0);
                app.update_race(1.0 / 60.0);
                app.update_versus(1.0 / 60.0);
                app.update_goal_run(1.0 / 60.0);
                app.hud_dt = 1.0 / 60.0;
                app.update_game_hud();
                app.update_screen(1.0 / 60.0);
                app.update_breakables();
                app.update_bouncies(1.0 / 60.0);
                app.update_pros();
                app.update_cutscene();
                app.play(1.0 / 60.0);
                app.update_collecting();
                app.update_skitch();
                app.update_records(1.0 / 60.0);
                app.update_portals(1.0 / 60.0);
                app.update_map();
                // The level's scripts see the skater too.
                let skater = app.skating.as_ref().map(|(s, ..)| s.position);
                let state = app.skating.as_ref().map(|(s, ..)| skater_state(s));
                if let Some(level) = &mut app.level {
                    level.behaviour.set_skater(skater);
                    level.behaviour.skater_state = state.unwrap_or_default();
                    level.update_objects(true, args.goal_objects, args.time + now, 1.0 / 60.0);
                    level.apply_creates();
                    level.behaviour.sounds.clear();
                }
            }
            // The game's pause menu brought up, and the pad's presses
            // given to it (--pause-keys: up, down, choose, back).
            if args.pause || !args.pause_keys.is_empty() {
                if args.pause {
                    app.toggle_pause();
                }
                for _ in 0..30 {
                    app.update_screen(1.0 / 60.0);
                }
                for key in args.pause_keys.chars() {
                    use desa_viewer::screen::Pad;
                    let pad = match key {
                        'u' => Some(Pad::Up),
                        'd' => Some(Pad::Down),
                        'l' => Some(Pad::Left),
                        'r' => Some(Pad::Right),
                        'c' => Some(Pad::Choose),
                        'b' => Some(Pad::Back),
                        // (A third of a second more, for what animates in.)
                        'w' => None,
                        _ => continue,
                    };
                    if let (Some(pad), Some(screen)) = (pad, &mut app.screen) {
                        screen.screen.pad(pad);
                    }
                    for _ in 0..20 {
                        app.update_screen(1.0 / 60.0);
                    }
                }
            }
            // (DESA_SCREEN_DUMP: the game's screen elements, for a look.)
            if std::env::var_os("DESA_SCREEN_DUMP").is_some() {
                if let Some(screen) = &app.screen {
                    eprintln!("{}", screen.screen.describe());
                    eprintln!("unknown {:x?}", screen.screen.unknown);
                }
            }
            if let Some(at) = args.replay_at {
                app.replay = Some(0.0);
                app.model.character.replaying = true;
                for _ in 0..args.replay_camera.unwrap_or(0) % REPLAY_CAMERAS.len() {
                    app.next_replay_camera();
                }
                app.play_replay(at);
            }
            camera = app.camera;
            if let Some(name) = &app.screen_movie_name {
                println!("screen movie: {name}");
            }
            if let Some((skater, _, chase)) = &app.skating {
                println!(
                    "camera: {:.0} from the target, {:.0} above it",
                    chase.eye.distance(chase.target),
                    chase.eye.y - chase.target.y
                );
                let model = &app.model.character;
                let shown = app
                    .character
                    .as_ref()
                    .and_then(|c| c.animations.get(model.animation))
                    .map_or("-", |(name, _)| name.as_str());
                println!(
                    "skate: {} | anim {shown} at {:.2}/{:.2} (picked {} at {:.2}) | {}",
                    model.skate_status.replace('\n', ", "),
                    model.time,
                    model.duration,
                    skater.anim.first,
                    skater.anim_time,
                    model.combo.as_deref().unwrap_or("").replace('\n', " / ")
                );
            }
        }
        // The blink clock follows --time too, so blinks can be captured.
        app.animate(0.0, args.time);
        // (The HUD's arrows, while the level's still the app's.)
        hud_arrows = app.hud_arrow_angles();
        loaded = app.level.take().unwrap();
    }
    // The level's particle effects, run for the time asked (two seconds at
    // least) and seen from the camera, with the skater's shadow and sparks.
    {
        let LoadedLevel {
            behaviour,
            particles,
            particle_textures,
            world,
            renderer,
            ..
        } = &mut loaded;
        for _ in 0..(args.time.max(2.0) * 60.0) as usize {
            particles.update(behaviour.program(), 1.0 / 60.0, camera.position);
        }
        let batches = particle_batches(particles, particle_textures, camera.position);
        renderer.set_particles(&batches);
        let mut overlay = Vec::new();
        if let (Some((skater, ..)), Some(world)) = (&app.skating, world.as_ref()) {
            overlay.extend(skater_shadow(skater, world));
            overlay.extend(app.sparks.vertices(camera.position));
        }
        renderer.set_shadow(&overlay);
    }

    let (width, height) = args.size;
    let egui_ctx = egui::Context::default();
    let mut egui_renderer =
        egui_wgpu::Renderer::new(&device, format, egui_wgpu::RendererOptions::default());
    let input = || egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(width as f32, height as f32),
        )),
        ..Default::default()
    };
    // The first pass lays things out; the second draws them settled. Keep
    // the first pass's texture uploads: they include the font atlas, which
    // every egui shape is drawn with.
    let clean = args.clean;
    let aspect = args.aspect.map(|(w, h)| w as f32 / h as f32);
    loaded.renderer.aspect = aspect;
    set_picture_aspect(&egui_ctx, aspect);
    let bars = bars.map(|image| egui_ctx.load_texture("bars", image, egui::TextureOptions::LINEAR));
    let arrows = hud_arrows;
    if !arrows.is_empty() {
        println!("HUD arrows pointing {arrows:.2?}");
    }
    let first = egui_ctx.run(input(), |ctx| {
        paint_bars(ctx, bars.as_ref());
        if !clean {
            ui::draw(ctx, &mut app.model);
        }
        if let Some(screen) = &mut app.screen {
            screen.paint(ctx);
        }
        paint_hud_arrows(ctx, &arrows);
    });
    let mut output = egui_ctx.run(input(), |ctx| {
        paint_bars(ctx, bars.as_ref());
        if !clean {
            ui::draw(ctx, &mut app.model);
        }
        if let Some(screen) = &mut app.screen {
            screen.paint(ctx);
        }
        paint_hud_arrows(ctx, &arrows);
    });
    let mut textures = first.textures_delta;
    textures.append(output.textures_delta);
    output.textures_delta = textures;

    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("screenshot"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    if let Some(name) = &args.camera_path {
        let path = loaded
            .camera_paths
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, p)| p)
            .with_context(|| format!("no camera path named {name}"))?;
        let at = path_camera(path, args.time);
        loaded.renderer.scripted_camera = Some(at);
        app.model.camera_paths.paths = loaded
            .camera_paths
            .iter()
            .map(|(n, p)| (n.clone(), path_length(p)))
            .collect();
        app.model.camera_paths.playing = app
            .model
            .camera_paths
            .paths
            .iter()
            .position(|(n, _)| n.eq_ignore_ascii_case(name));
        app.model.camera_paths.time = args.time;
    }

    let view = target.create_view(&Default::default());
    loaded.renderer.look = parse_look(&args.look)?;
    paint(
        &device,
        &queue,
        &mut egui_renderer,
        &egui_ctx,
        Some(&mut loaded.renderer),
        &camera,
        0.0,
        &view,
        (width, height),
        output,
    );
    renderer::save_png(&device, &queue, &target, width, height, out)?;
    println!("Wrote {}", out.display());
    Ok(())
}

/// The combo on screen: the tricks so far with their points and
/// multiplier, or the last combo's result for a few seconds after it ends.
fn combo_text(skater: &Skater) -> Option<String> {
    let line = |combo: &skate::Combo| {
        combo
            .tricks
            .iter()
            .map(|t| match t.spins {
                0 => t.name.clone(),
                n => format!("{} {}", n * 180, t.name),
            })
            .collect::<Vec<_>>()
            .join(" + ")
    };
    let combo = &skater.combo_tricks;
    if !combo.is_empty() {
        return Some(format!(
            "{}\n{} x {}",
            line(combo),
            combo.points(),
            combo.multiplier()
        ));
    }
    let last = skater.last_combo.as_ref()?;
    if skater.action_time > 3.0 && !matches!(skater.action, SkateAction::Landing) {
        return None;
    }
    Some(if last.bailed {
        format!("{}\nBail!", line(&last.combo))
    } else {
        // `Land2`'s panel message for a sketchy landing.
        let sketchy = if skater.landing.sketchy {
            "  Sketchy"
        } else {
            ""
        };
        format!("{}\n+{}{sketchy}", line(&last.combo), last.total)
    })
}

/// `--skate-keys`: "0.5:+Space,0.7:-Space" as (seconds, key, pressed).
fn parse_skate_keys(spec: &str) -> Result<Vec<(f32, KeyCode, bool)>> {
    spec.split(',')
        .map(|item| {
            let (at, key) = item
                .trim()
                .split_once(':')
                .with_context(|| format!("expected seconds:+Key in {item:?}"))?;
            let at: f32 = at
                .parse()
                .with_context(|| format!("bad time in {item:?}"))?;
            let (down, name) = match key.split_at(1) {
                ("+", name) => (true, name),
                ("-", name) => (false, name),
                _ => anyhow::bail!("expected + or - before the key in {item:?}"),
            };
            let key = match name.to_ascii_uppercase().as_str() {
                "W" => KeyCode::KeyW,
                "A" => KeyCode::KeyA,
                "S" => KeyCode::KeyS,
                "D" => KeyCode::KeyD,
                "E" => KeyCode::KeyE,
                "Q" => KeyCode::KeyQ,
                "F" => KeyCode::KeyF,
                "R" => KeyCode::KeyR,
                "J" => KeyCode::KeyJ,
                "L" => KeyCode::KeyL,
                "SPACE" => KeyCode::Space,
                // (Player two's.)
                "UP" => KeyCode::ArrowUp,
                "DOWN" => KeyCode::ArrowDown,
                "LEFT" => KeyCode::ArrowLeft,
                "RIGHT" => KeyCode::ArrowRight,
                "N0" => KeyCode::Numpad0,
                "N1" => KeyCode::Numpad1,
                "N2" => KeyCode::Numpad2,
                "N3" => KeyCode::Numpad3,
                "N5" => KeyCode::Numpad5,
                // (A press on the game's screen: a speech box's A.)
                "ENTER" => KeyCode::Enter,
                other => anyhow::bail!("unknown key {other:?}"),
            };
            Ok((at, key, down))
        })
        .collect()
}

/// What the skater's doing, as the level's scripts ask it.
fn skater_state(s: &Skater) -> desa_viewer::behaviour::SkaterState {
    let bailing = matches!(
        s.action,
        SkateAction::Bail
            | SkateAction::BailGrind
            | SkateAction::BailManual
            | SkateAction::BailFall { .. }
    );
    let on_rail = s.grind.is_some();
    desa_viewer::behaviour::SkaterState {
        on_ground: s.on_ground,
        in_air: !s.on_ground && !on_rail && s.lip.is_none() && s.wall.is_none(),
        on_rail,
        bailing,
        skitching: s.skitch.is_some(),
        doing_trick: s.trick.is_some() || on_rail || s.manual || s.lip.is_some(),
        speed: s.velocity.length(),
        heading: s.heading,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_warp_spins_faster_then_shrinks_away() {
        let at = Vec3::new(10.0, 0.0, 5.0);
        let stood = Mat4::IDENTITY;
        let size = |t: f32| {
            warp_spin_placement(at, t, stood)
                .to_scale_rotation_translation()
                .0
                .x
        };
        // Full size while it spins up, there at the warp.
        assert_eq!(size(1.0), 1.0);
        let moved = warp_spin_placement(Vec3::ZERO, 1.0, Mat4::from_translation(Vec3::X));
        assert_eq!(moved.to_scale_rotation_translation().2, Vec3::ZERO);
        // Shrinking from 1.75 s, gone by 4.75 s.
        assert!(size(3.0) < 1.0 && size(3.0) > size(4.5));
        assert!(size(5.0) < 0.001);
    }
}
