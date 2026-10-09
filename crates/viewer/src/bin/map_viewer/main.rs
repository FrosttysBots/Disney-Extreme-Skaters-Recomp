// No console window in release builds; the panel shows any errors.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

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
    /// "seconds:-Key" separated by commas (keys W A S D Space E Q F R J L);
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
    /// For --skate: play the level's S-K-A-T-E letters goal (from its start)
    #[arg(long, requires = "skate")]
    letters: bool,
    /// For --skate: bring up the game's pause menu at the end
    #[arg(long, requires = "skate")]
    pause: bool,
    /// For --skate: presses to the game's screen at the end, in order (u up,
    /// d down, c choose, b back): its pause menu with --pause, else
    /// whatever it shows (a goal's speech box)
    #[arg(long, requires = "skate", default_value = "")]
    pause_keys: String,
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
    let main_menu = args.level.is_none() && app.model.start_menu;
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
    let behaviour = Behaviour::new(&nodes, &scripts);
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
    let world = Level::from_bytes_filtered(&files.scene, files.textures.as_deref(), |s| {
        !hidden.contains(&s)
            && !breakable_sectors.contains(&s)
            && !bouncy_names.contains(&s)
            && !mover_names.contains(&s)
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
    // The level's moving pieces, each in a layer of its own, shown while
    // its object is there and put where it is.
    let mut movers = Vec::new();
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
        movers.push(MoverState {
            object: behaviour.mover(i),
            name: m.name,
            layer,
            base: mesh.vertices,
            pivot: m.position,
            placed: m.rotation(),
            last: (m.created_at_start, m.position, m.rotation()),
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
}

impl LoadedLevel {
    /// The particle effects and hidden sectors the scripts started and
    /// stopped, created and killed.
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
                    .map(|v| desa_viewer::level::Vertex {
                        position: (now.1 + turn * (Vec3::from(v.position) - m.pivot)).to_array(),
                        normal: (turn * Vec3::from(v.normal)).to_array(),
                        ..*v
                    })
                    .collect();
                self.renderer.update_layer(m.layer, 0, &moved);
                // Its collision goes with it.
                if let Some(world) = &mut self.world {
                    let transform = Mat4::from_translation(now.1)
                        * Mat4::from_quat(turn)
                        * Mat4::from_translation(-m.pivot);
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
struct MoverState {
    object: usize,
    name: u32,
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
            "preview_skater_menu",
            "launch_select_skater_menu",
            "start_2p",
            "launch_options_menu_load_game_sequence",
            "launch_options_menu_save_game_sequence",
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
        let area = ctx.available_rect();
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
    /// The objects that already reacted to the skater coming near before
    /// it started (what's new since is the goal's, for the map).
    before: HashSet<usize>,
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
    pad_back: bool,
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
    last_frame: Instant,
    started: Instant,
    error: Option<anyhow::Error>,
}

impl<'a> App<'a> {
    fn new(settings: &'a mut Settings) -> Self {
        let speed = settings.speed.unwrap_or(DEFAULT_SPEED);
        let camera = settings.best.get("camera").map_or(1, |c| *c as usize);
        let volume = |key: &str| settings.best.get(key).map_or(1.0, |v| *v as f32 / 100.0);
        let (effects_volume, music_volume) = (volume("volume.effects"), volume("volume.music"));
        let intro = settings.best.get("intro") != Some(&0);
        let start_menu = settings.best.get("start_menu") != Some(&0);
        App {
            settings,
            gpu: None,
            data: None,
            level: None,
            model: ui::Model {
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
                    cheats: Default::default(),
                    sound: true,
                    music: true,
                    effects_volume,
                    music_volume,
                    rumble: true,
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
            pad_back: false,
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
                    if std::mem::take(&mut self.main_menu_pending) && shop {
                        screen
                            .screen
                            .run(qb::checksum("launch_main_menu"), Vec::new());
                    }
                }
                if !shop {
                    self.settings.last_skated = Some(info.id.clone());
                }
                self.settings.last_level = Some(info.id);
                self.settings.save();
                // Arrived through a warp: skating on.
                if std::mem::take(&mut self.skate_on_load) {
                    self.toggle_skate();
                }
            }
            Err(err) => self.model.message = Some(format!("{err:#}")),
        }
    }

    fn play_camera_path(&mut self, index: usize) {
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
        if let Some(level) = &mut self.level {
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
                audio.say_line(&sound);
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
        // The level's gaps, as a checklist of those landed before.
        let landed = self.settings.gaps.get(&level.id);
        let mut gap_list: Vec<(String, u32, bool)> = Vec::new();
        for trigger in skater.gap_triggers.values() {
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
                self.goal_cutscenes(if pro { "ProScore" } else { "HighScore" });
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
            if ready && self.warp_offer.is_none() && across < WARP_ENTER && to.y.abs() < WARP_HEIGHT
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
        for (object, trigger) in &skater.gap_triggers {
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
        let skating = self.skating.as_ref().map(|(s, ..)| s.position);
        let busy = self.run.is_some()
            || self.letters.is_some()
            || self.racing.is_some()
            || self.goal_run.is_some();
        let Some(level) = &mut self.level else { return };
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
            if !busy && self.goal_offer.is_none() && self.warp_offer.is_none() && near < PRO_NEAR {
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
                        scripts.extend(b.spec.collide_script);
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
        for script in scripts {
            level.behaviour.run_level_script(script, Vec::new());
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
        b.goal_needed = goal.needed;
        b.goal_won = false;
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
        for script in [goal.activate, goal.start_script].into_iter().flatten() {
            b.run_level_script(script, params.clone());
        }
        self.goal_cutscenes(&kind);
        self.goal_run = Some(GoalRun {
            kind,
            index,
            left: goal.time,
            goal,
            over: false,
            before,
        });
    }

    /// The goal played from its scripts: the gaps' scripts run (they set
    /// its flags), the clock counts down, and it's won with all its flags
    /// or out of time lost.
    fn update_goal_run(&mut self, dt: f32) {
        let (Some((skater, ..)), Some(level)) = (&mut self.skating, &mut self.level) else {
            return;
        };
        // Landed gaps' scripts run whether a goal's on or not (they ask).
        for script in std::mem::take(&mut skater.gap_scripts) {
            level.behaviour.run_level_script(script, Vec::new());
        }
        // Camera paths the scripts play (a goal's cutscenes: each arcade
        // machine coming on), one at a time.
        for path in std::mem::take(&mut level.behaviour.cameras) {
            if !self.cutscene && self.cutscene_request.is_none() {
                self.cutscene_request = Some(path);
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
            if let Some(&script) = level.touch_scripts.get(&object) {
                level.behaviour.run_level_script(script, Vec::new());
            }
        }
        let Some(run) = &mut self.goal_run else {
            return;
        };
        let model = &mut self.model.character;
        let got = level.behaviour.goal_progress();
        // (What it asks, unless that's its name over.)
        let text = if run.goal.text == run.goal.name {
            String::new()
        } else {
            run.goal.text.clone()
        };
        model.goal_progress = Some((
            run.goal.name.clone(),
            text,
            got.min(run.goal.needed),
            run.goal.needed,
        ));
        model.run_clock = run.left;
        if run.over {
            return;
        }
        if let Some(left) = &mut run.left {
            *left = (*left - dt).max(0.0);
        }
        let won = level.behaviour.goal_won || (run.goal.needed > 0 && got >= run.goal.needed);
        if won || run.left == Some(0.0) {
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
            let letter = "SKATE".chars().nth(i).unwrap_or('?');
            self.lines_to_say.extend(self.letter_lines[i]);
            skater.message = Some((letter.to_string(), 1.0));
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
        if let Some((score, win)) = run.goal.as_ref().filter(|_| !run.won) {
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
                // `win_message_text`, a little longer than a trick's.
                skater.message = Some((win.clone(), 3.0));
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
            // The C-stick looks round; Start pauses.
            let stick = glam::Vec2::new(pad.value(Axis::RightStickX), pad.value(Axis::RightStickY));
            if stick.length() > look.length() && stick.length() > 0.2 {
                look = stick;
            }
            start |= pressed(Button::Start);
            back |= pressed(Button::Select);
        }
        self.pad_look = look;
        if start && !self.pad_start && self.skating.is_some() {
            self.toggle_pause();
        }
        self.pad_start = start;
        if back && !self.pad_back {
            self.next_camera();
        }
        self.pad_back = back;
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

    /// Pauses skating, or goes on.
    fn toggle_pause(&mut self) {
        if self.skating.is_some() && self.replay.is_none() {
            self.model.character.paused = !self.model.character.paused;
            // The game's own pause menu (`create_pause_menu`), if it can
            // be made; taken away again on resuming (the panel stays).
            let paused = self.model.character.paused;
            if let Some(screen) = &mut self.screen {
                screen.screen.destroy_id("pause_menu");
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
        let (screen_calls, screen_scripts) = self
            .level
            .as_mut()
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
        let Some((skater, ..)) = &self.skating else {
            if self.hud.up {
                self.hud = GameHud::default();
                s.destroy_id("player1_panel_container");
                s.destroy_id("the_time");
                s.destroy_id("current_goal");
                s.destroy_id("goal_points_text");
                s.destroy_id("minigame_timer");
            }
            self.model.character.game_hud = false;
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
        // The goal on, for the UI's scripts that ask after it.
        let b = &level.behaviour;
        s.goal = b
            .active_goal
            .or(b.ended_goal)
            .map(|id| (id, b.goal_params.clone()));
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
        // The goal on, and how far it's got, in the game's goal text.
        let goal = match &self.model.character.goal_progress {
            Some((name, _, got, needed)) if *needed > 0 => format!(
                "{name}
{got} of {needed}"
            ),
            Some((name, ..)) => name.clone(),
            None => " ".to_string(),
        };
        s.set(
            "current_goal",
            &props(vec![("text", Value::String(goal))]),
            program,
        );
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

    /// The game's main menu (`launch_main_menu`), in the Skate Shop as the
    /// game has it: there now, or once the shop's loaded.
    fn open_main_menu(&mut self) {
        if self.skating.is_some() {
            self.toggle_skate();
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

    /// The game's menus run on: their scripts (with the level's), sounds,
    /// and what they ask of the viewer (`unpausegame`: resume).
    fn update_screen(&mut self, dt: f32) {
        self.menu_pad();
        let (Some(screen), Some(level)) = (&mut self.screen, &self.level) else {
            return;
        };
        screen.screen.update(level.behaviour.program(), dt);
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
        let sounds = std::mem::take(&mut screen.screen.sounds);
        let requests = std::mem::take(&mut screen.screen.requests);
        self.model.character.game_menu = screen.screen.takes_pad();
        if let Some(audio) = self.audio.as_ref().filter(|_| self.model.character.sound) {
            for sound in sounds {
                audio.play_named(sound, 1.0);
            }
        }
        let c = qb::checksum;
        for (name, _) in requests {
            if name == c("unpausegame") && self.model.character.paused {
                self.model.character.paused = false;
                if let Some(screen) = &mut self.screen {
                    screen.screen.clear();
                }
            } else if name == c("preview_skater_menu") || name == c("launch_select_skater_menu") {
                // Play Game: skating in the Hub, as the career starts.
                // Free Skate: on the level last skated.
                let level = if name == c("preview_skater_menu") {
                    "HUB".to_string()
                } else {
                    self.settings
                        .last_skated
                        .clone()
                        .unwrap_or_else(|| "HUB".into())
                };
                if let Some(screen) = &mut self.screen {
                    screen.screen.clear();
                }
                if let Some(i) = self
                    .model
                    .levels
                    .iter()
                    .position(|l| l.id.eq_ignore_ascii_case(&level))
                {
                    self.skate_on_load = true;
                    self.start_load(i);
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
        if let Some(index) = self.goal_run.as_ref().map(|r| r.index) {
            self.start_goal(index);
        } else if self.letters.is_some() {
            self.start_letters();
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
        if self.warp_offer.is_some()
            || self.goal_offer.is_some()
            || self.cutscene
            || self.model.character.paused
        {
            // (The pad's still read, for Start.)
            let _ = self.pad_input();
            return;
        }
        let pad = self.pad_input();
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
            // Small vehicles (the toy cars, not the Hub's plane or Zurg's
            // platform) are solid too: the skater bumps off them.
            obstacles.extend(vehicles.iter().filter(|(_, v)| v.half_length < 60.0).map(
                |(_, v)| skate::world::Obstacle {
                    base: v.position - Vec3::Y * 24.0,
                    radius: v.half_length * 0.8,
                    height: 64.0,
                },
            ));
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
        if cheats.moon {
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
            // The animals' and toy cars' shadows too, those near.
            if let Some(world) = &level.world {
                for o in world.obstacles() {
                    let feet = o.base + Vec3::Y * 24.0;
                    if feet.distance(self.camera.position) < SHADOW_RANGE {
                        shadow.extend(blob_shadow(feet, o.radius * 0.8, world));
                    }
                }
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
        // The keys drive the skater instead while skating (paused, they fly
        // the camera round for pictures).
        if self.skating.is_some() && !self.model.character.paused {
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
        self.update_cutscene();
        self.say_lines();
        // Paused (or a goal's camera path playing): the clocks stop too.
        if !self.model.character.paused && !self.cutscene {
            self.update_run(dt);
            self.update_letters(dt);
            self.update_race(dt);
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
        let Some(gpu) = &mut self.gpu else { return };
        let raw = gpu.egui_state.take_egui_input(&gpu.window);
        let model = &mut self.model;
        let movie = &mut self.movie;
        let screen = &mut self.screen;
        let mut actions = Vec::new();
        let output = gpu.egui_ctx.run(raw, |ctx| {
            actions = ui::draw(ctx, model);
            if let Some(screen) = screen {
                screen.paint(ctx);
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
        // Paused: P resumes, Esc stops skating.
        if self.model.character.paused {
            match code {
                KeyCode::KeyP if !repeat => self.toggle_pause(),
                KeyCode::Escape if !repeat => self.toggle_skate(),
                _ => {}
            }
            return;
        }
        if code == KeyCode::KeyM && !repeat && self.skating.is_some() {
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
                KeyCode::Enter | KeyCode::NumpadEnter if !repeat => self.take_goal(),
                KeyCode::Escape if !repeat => self.not_now(),
                _ => {}
            }
            return;
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
        if args.skate > 0.0 {
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
        app.model.character.map_image = loaded
            .minimap
            .as_ref()
            .map(|m| egui::ColorImage::from_rgba_unmultiplied([m.width, m.height], &m.pixels));
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
                app.start_goal(index);
                // From --skate-from, if given, rather than the goal's start.
                if let (Some(_), Some((skater, ..))) = (&args.skate_from, &mut app.skating) {
                    skater.position = from;
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
            if let Some(left) = args.run {
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
                app.update_letters(1.0 / 60.0);
                app.update_race(1.0 / 60.0);
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
                if let Some(level) = &mut app.level {
                    level.behaviour.set_skater(skater);
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
                        'u' => Pad::Up,
                        'd' => Pad::Down,
                        'c' => Pad::Choose,
                        'b' => Pad::Back,
                        _ => continue,
                    };
                    if let Some(screen) = &mut app.screen {
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
    let first = egui_ctx.run(input(), |ctx| {
        if !clean {
            ui::draw(ctx, &mut app.model);
        }
        if let Some(screen) = &mut app.screen {
            screen.paint(ctx);
        }
    });
    let mut output = egui_ctx.run(input(), |ctx| {
        if !clean {
            ui::draw(ctx, &mut app.model);
        }
        if let Some(screen) = &mut app.screen {
            screen.paint(ctx);
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
                "J" => KeyCode::KeyJ,
                "L" => KeyCode::KeyL,
                "SPACE" => KeyCode::Space,
                other => anyhow::bail!("unknown key {other:?}"),
            };
            Ok((at, key, down))
        })
        .collect()
}
