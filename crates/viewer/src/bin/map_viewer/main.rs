// No console window in release builds; the panel shows any errors.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

//! DESA Map Viewer: pick a level from your disc image and fly around it.
//!
//! Reads levels straight from the disc (or a folder of extracted `.prg`
//! archives), so no unpacking step is needed. The disc's location is
//! remembered between runs. Any of the playable characters can stand on a
//! spawn point and play their animations.

mod audio;
mod rumble;
mod settings;
mod sparks;
mod ui;

use std::collections::HashSet;
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
use settings::Settings;

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
    /// "seconds:-Key" separated by commas (keys W A S D Space E Q F R);
    /// without it W is held throughout
    #[arg(long, requires = "screenshot")]
    skate_keys: Option<String>,
    /// For --skate: start at x,y,z facing a heading in degrees (0 along +Z,
    /// 90 along +X) instead of the level's start
    #[arg(long, requires = "screenshot", allow_hyphen_values = true)]
    skate_from: Option<String>,
    /// For --skate: skate a two-minute run with this many seconds left on
    /// its clock
    #[arg(long, requires = "skate")]
    run: Option<f32>,
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

fn main() -> Result<()> {
    let args = Args::parse();
    let mut settings = Settings::load();
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
    if let Some(path) = data_path {
        app.open_data(&path);
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
    // Open the requested level, else the last one viewed, else the hub.
    if let Some(level) = args
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
    /// Animated vertex colors of the level, its sky and its goal geometry.
    colors: [ColorAnimation; 3],
    /// Which marker sets the renderer currently has: (rails, spawns).
    markers: (bool, bool),
}

/// Loads a level from the game data and uploads it to the GPU.
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
    // Sectors that aren't there at the start go in their own layer.
    let hidden = &nodes.hidden_sectors;
    let world = Level::from_bytes_filtered(&files.scene, files.textures.as_deref(), |s| {
        !hidden.contains(&s)
    })?;
    let goal_geometry = Level::from_bytes_filtered(&files.scene, files.textures.as_deref(), |s| {
        hidden.contains(&s)
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
    // Objects' scripts: the game's shared scripts, then the level's own.
    let mut scripts = data.global_scripts().unwrap_or_else(|e| {
        eprintln!("warning: no shared scripts: {e:#}");
        Vec::new()
    });
    scripts.extend(files.scripts.iter().cloned());
    let behaviour = Behaviour::new(&nodes, &scripts);
    let skate_world = files
        .collision
        .as_deref()
        .and_then(|c| ngc_collision::Collision::parse(c).ok())
        .map(|c| {
            World::new(c).with_rails(Rails::new(
                nodes
                    .rails
                    .iter()
                    .map(|r| Segment {
                        start: r.start,
                        end: r.end,
                    })
                    .collect(),
            ))
        });
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
    let layers = ObjectLayers {
        goal_geometry: renderer.add_layer(&goal_geometry, false),
        props: renderer.add_layer(&objects.props.mesh, false),
        goal_props: renderer.add_layer(&objects.goal_props.mesh, false),
        crowd: renderer.add_layer(&objects.crowd.mesh, true),
        goal_crowd: renderer.add_layer(&objects.goal_crowd.mesh, true),
    };
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
            colors,
            markers: (false, false),
        },
        stats,
    ))
}

/// The renderer layers holding a level's objects (`None` when empty).
struct ObjectLayers {
    goal_geometry: Option<usize>,
    props: Option<usize>,
    goal_props: Option<usize>,
    crowd: Option<usize>,
    goal_crowd: Option<usize>,
}

impl LoadedLevel {
    /// Runs objects' scripts for `dt` seconds, shows or hides the object
    /// layers, poses the pedestrians shown and animates vertex colors.
    fn update_objects(&mut self, objects: bool, goal_objects: bool, seconds: f32, dt: f32) {
        for (goal, copy, placement) in
            self.behaviour
                .update(&mut self.objects, seconds, dt, goal_objects)
        {
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
        if !world.is_empty() {
            r.update_world(false, world.first_vertex, &world.at(seconds));
        }
        if !sky.is_empty() {
            r.update_world(true, sky.first_vertex, &sky.at(seconds));
        }
        if goal_objects && !goal.is_empty() {
            r.update_layer(l.goal_geometry, goal.first_vertex, &goal.at(seconds));
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
    const RADIUS: f32 = 16.0;
    const FADE: f32 = 400.0;
    const SIDES: usize = 20;
    let from = skater.position + Vec3::Y * 10.0;
    let Some(hit) = world.ray(from, from - Vec3::Y * FADE) else {
        return Vec::new();
    };
    // Faces flagged to take no skater shadow.
    if hit.flags & ngc_collision::face_flags::NO_SKATER_SHADOW != 0 {
        return Vec::new();
    }
    let height = (skater.position.y - hit.point.y).max(0.0);
    let fade = (1.0 - height / FADE).clamp(0.0, 1.0);
    let alpha = (150.0 * fade) as u8;
    if alpha == 0 {
        return Vec::new();
    }
    let normal = hit.normal.normalize_or(Vec3::Y);
    let side = normal.any_orthonormal_vector();
    let along = normal.cross(side);
    let radius = RADIUS * (0.6 + 0.4 * fade);
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

/// A two-minute run (the game's single session).
struct Run {
    /// Seconds left on the clock.
    left: f32,
    /// Out of time with the last combo landed: seconds since, braking.
    ending: Option<f32>,
    /// Stopped, with the result up.
    over: bool,
}

/// How far away objects' sounds fade out (units).
const OBJECT_SOUND_RANGE: f32 = 2000.0;

/// How long a run lasts (`StartGoal_TrickAttack time = 120`).
const RUN_TIME: f32 = 120.0;

/// A character's bones, each relative to its parent.
type Pose = Vec<(Quat, Vec3)>;

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
    last_frame: Instant,
    started: Instant,
    error: Option<anyhow::Error>,
}

impl<'a> App<'a> {
    fn new(settings: &'a mut Settings) -> Self {
        let speed = settings.speed.unwrap_or(DEFAULT_SPEED);
        App {
            settings,
            gpu: None,
            data: None,
            level: None,
            model: ui::Model {
                data_path: None,
                levels: Vec::new(),
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
                    sound: true,
                    music: true,
                    rumble: true,
                    run_clock: None,
                    run_result: None,
                    skate_status: String::new(),
                    trick_list: Vec::new(),
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
            playlist: Vec::new(),
            now_playing: None,
            next_track: 0,
            ambience: None,
            shown_animation: 0,
            skate_pose: None,
            blend_from: None,
            last_frame: Instant::now(),
            started: Instant::now(),
            error: None,
        }
    }

    fn open_data(&mut self, path: &Path) {
        match GameData::open(path) {
            Ok(data) => {
                self.model.levels = data.levels();
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
                self.model.has_collision = level.renderer.has_collision();
                if !self.model.has_collision {
                    self.model.collision = CollisionView::Hidden;
                }
                self.model.set_spawns(&level.nodes.spawns);
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
                self.settings.last_level = Some(info.id);
                self.settings.save();
            }
            Err(err) => self.model.message = Some(format!("{err:#}")),
        }
    }

    fn play_camera_path(&mut self, index: usize) {
        self.model.camera_paths.playing = Some(index);
        self.model.camera_paths.time = 0.0;
    }

    /// Stops a camera path, leaving the free camera where the path was.
    fn stop_camera_path(&mut self) {
        self.model.camera_paths.playing = None;
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
            self.stop_camera_path();
        } else {
            *time = (*time + dt).min(path_length(path));
        }
    }

    /// Loads the level's sounds for skating (with a window and a sound
    /// device).
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
        let skating = self.skating.is_some();
        let (Some(audio), Some(data)) = (&mut self.audio, &mut self.data) else {
            return;
        };
        if skating && self.model.character.music && !self.playlist.is_empty() {
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
        self.skate_pose = None;
        self.run = None;
        self.model.character.run_clock = None;
        self.model.character.run_result = None;
        self.blend_from = None;
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
        let physics = Physics::new(program, &stats);
        let position = self.placement.transform_point3(Vec3::ZERO);
        let forward = self.placement.transform_vector3(Vec3::Z);
        let mut skater = Skater::new(position, forward.x.atan2(forward.z));
        // The character's tricks, timed by its animations.
        let mut tricks = TrickBook::new(program, id, &stats);
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
        self.start_audio();
        self.stop_camera_path();
        let chase = ChaseCamera::behind(&skater, &physics);
        self.skating = Some((skater, physics, chase));
        self.model.character.skating = true;
        self.model.character.playing = false;
        self.set_looking(false);
    }

    /// Starts a two-minute run (`StartGoal_TrickAttack time = 120`) from
    /// where the character stands at the level's start.
    fn start_run(&mut self) {
        if self.skating.is_some() {
            self.toggle_skate();
        }
        let Some(level) = &self.level else { return };
        self.placement = level.home;
        self.toggle_skate();
        if self.skating.is_some() {
            self.run = Some(Run {
                left: RUN_TIME,
                ending: None,
                over: false,
            });
        }
    }

    /// Counts a two-minute run down. Out of time, the combo under way
    /// still counts: once the skater's back on the ground with no combo,
    /// the run ends (`EndOfRun`): the skater brakes to a stop and the
    /// score goes against the best.
    fn update_run(&mut self, dt: f32) {
        let (Some(run), Some((skater, ..))) = (&mut self.run, &self.skating) else {
            return;
        };
        self.model.character.run_clock = Some(run.left);
        if run.over {
            return;
        }
        run.left = (run.left - dt).max(0.0);
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
                    let key = format!(
                        "{}.{}",
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
        for (_, pad) in gilrs.gamepads() {
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
        }
        input
    }

    fn skate(&mut self, dt: f32) {
        let pad = self.pad_input();
        // The pedestrians shown are solid: the skater bumps off them.
        let objects = self.model.show_objects;
        let goal_objects = self.model.show_goal_objects;
        if let Some(level) = &mut self.level {
            let crowd = &level.objects.crowd;
            let goal_crowd = &level.objects.goal_crowd;
            let obstacles = crowd
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
            if let Some(world) = &mut level.world {
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
        let input = Input {
            push: held(KeyCode::KeyW) || held(KeyCode::ArrowUp),
            brake: held(KeyCode::KeyS) || held(KeyCode::ArrowDown),
            turn: f32::from(u8::from(held(KeyCode::KeyD) || held(KeyCode::ArrowRight)))
                - f32::from(u8::from(held(KeyCode::KeyA) || held(KeyCode::ArrowLeft))),
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
        skater.update(input, physics, world, dt);
        self.sparks.update(skater, dt);
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
        self.model.character.combo = combo_text(skater);
        // The skater's message, or for a few seconds the song that began.
        let song = self
            .now_playing
            .as_ref()
            .filter(|(_, at)| at.elapsed().as_secs_f32() < 4.0)
            .map(|(title, _)| format!("Now playing: {title}"));
        self.model.character.message = skater.message.as_ref().map(|(m, _)| m.clone()).or(song);

        // Animation: the one the skater picked (the game's scripts' choice),
        // its first part once and then the next looping, or looping, or held
        // on its last frame.
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
        let length =
            |anim: u32| by_checksum(anim).map_or(1.0, |i| character.animations[i].1.duration);
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
            if let Some(index) = character
                .animations
                .iter()
                .position(|(name, _)| qb::checksum(name) == anim)
            {
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
            if index != self.shown_animation {
                let period = skate::anims::blend_period(name);
                self.blend_from = match (&self.skate_pose, period > 0.0) {
                    (Some(pose), true) => Some((pose.clone(), 0.0, period)),
                    _ => None,
                };
            }
            let model = &mut self.model.character;
            model.animation = index;
            self.shown_animation = index;
            model.duration = duration;
            // Held on its last frame, or looping round.
            model.time = if duration <= 0.0 {
                0.0
            } else if hold {
                time.clamp(0.0, duration * 0.999)
            } else {
                time.rem_euclid(duration)
            };
        }

        // The pose: the animation's, blended from the last one's.
        let model = &self.model.character;
        let mut local = character.local_pose(model.animation, model.time);
        if let Some((from, elapsed, period)) = &mut self.blend_from {
            *elapsed += dt;
            let t = (*elapsed / *period).clamp(0.0, 1.0);
            // Eased, so it starts and settles gently.
            let t = t * t * (3.0 - 2.0 * t);
            for (to, from) in local.iter_mut().zip(from.iter()) {
                to.0 = from.0.slerp(to.0, t);
                to.1 = from.1.lerp(to.1, t);
            }
            if *elapsed >= *period {
                self.blend_from = None;
            }
        }
        self.skate_pose = Some(local);

        // Chase camera, on the game's medium camera settings.
        chase.update(skater, physics, world, dt);
        self.camera = FlyCamera::looking_at(chase.eye, chase.target);
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
        // stats belong to it).
        if self.skating.take().is_some() {
            self.model.character.skating = false;
            self.sparks.clear();
            self.rumble.stop();
            if let Some(audio) = &mut self.audio {
                audio.stop();
            }
            self.model.character.playing = true;
            self.model.character.balance = None;
            self.model.character.combo = None;
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
        let loaded = data
            .load_character(&info.id)
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
                model.animation = ["StandIdle", "Stage_Idle"]
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
                self.settings.last_character = Some(info.id);
                self.settings.save();
            }
            Err(err) => self.model.message = Some(format!("{err:#}")),
        }
    }

    /// Advances the character's animation, uploads the posed mesh and
    /// shows the eyes for `clock` seconds since the viewer started.
    fn animate(&mut self, dt: f32, clock: f32) {
        let (Some(character), Some(level)) = (&self.character, &mut self.level) else {
            return;
        };
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
            shadow.extend(self.sparks.vertices(self.camera.position));
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
            .with_title("DESA Map Viewer")
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
        Ok(())
    }

    fn update(&mut self, dt: f32) {
        // The keys drive the skater instead while skating.
        if self.skating.is_some() {
            return;
        }
        let typing = self
            .gpu
            .as_ref()
            .is_some_and(|g| g.egui_ctx.wants_keyboard_input());
        if typing {
            return;
        }
        let pressed = |k| self.keys.contains(&k);
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
        self.update_run(dt);
        self.update_music();
        self.play(dt);
        self.sync_view();
        let clock = self.started.elapsed().as_secs_f32();
        self.animate(dt, clock);
        if let Some(level) = &mut self.level {
            // The scripts see the skater (birds fly off as it comes near).
            let skater = self.skating.as_ref().map(|(s, ..)| s.position);
            level.behaviour.set_skater(skater);
            level.update_objects(
                self.model.show_objects,
                self.model.show_goal_objects,
                clock,
                dt,
            );
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

        let Some(gpu) = &mut self.gpu else { return };
        let raw = gpu.egui_state.take_egui_input(&gpu.window);
        let model = &mut self.model;
        let mut actions = Vec::new();
        let output = gpu.egui_ctx.run(raw, |ctx| actions = ui::draw(ctx, model));
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
                ui::Action::ResetCamera => {
                    self.stop_camera_path();
                    if let Some(level) = &self.level {
                        self.camera = level.start;
                    }
                }
                ui::Action::LoadCharacter(i) => self.load_character(i),
                ui::Action::LookAtCharacter => self.camera = camera_facing(self.placement),
                ui::Action::ToggleSkate => self.toggle_skate(),
                ui::Action::StartRun => self.start_run(),
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

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(gpu) = &self.gpu {
            gpu.window.request_redraw();
        }
    }
}

/// Renders the first frame of a level, panel included, without a window.
/// With a character, the camera looks at it instead.
fn screenshot(data_path: &Path, args: &Args, out: &Path) -> Result<()> {
    let mut data = GameData::open(data_path)?;
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
        app.level = Some(loaded);
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
            app.toggle_skate();
            if let Some(left) = args.run {
                app.run = Some(Run {
                    left,
                    ending: None,
                    over: false,
                });
            }
            let script = args
                .skate_keys
                .as_deref()
                .map(parse_skate_keys)
                .transpose()?;
            if script.is_none() {
                app.keys.insert(KeyCode::KeyW);
            }
            let steps = (args.skate * 60.0).round() as usize;
            for step in 0..steps {
                let now = step as f32 / 60.0;
                for &(at, key, down) in script.iter().flatten() {
                    if (at - now).abs() < 0.5 / 60.0 {
                        if down {
                            app.keys.insert(key);
                        } else {
                            app.keys.remove(&key);
                        }
                    }
                }
                app.skate(1.0 / 60.0);
                app.update_run(1.0 / 60.0);
                // The level's scripts see the skater too.
                let skater = app.skating.as_ref().map(|(s, ..)| s.position);
                if let Some(level) = &mut app.level {
                    level.behaviour.set_skater(skater);
                    level.update_objects(true, args.goal_objects, args.time + now, 1.0 / 60.0);
                    level.behaviour.sounds.clear();
                }
            }
            camera = app.camera;
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
        loaded = app.level.take().unwrap();
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
    let first = egui_ctx.run(input(), |ctx| {
        if !clean {
            ui::draw(ctx, &mut app.model);
        }
    });
    let mut output = egui_ctx.run(input(), |ctx| {
        if !clean {
            ui::draw(ctx, &mut app.model);
        }
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
                "SPACE" => KeyCode::Space,
                other => anyhow::bail!("unknown key {other:?}"),
            };
            Ok((at, key, down))
        })
        .collect()
}
