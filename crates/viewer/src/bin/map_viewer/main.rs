// No console window in release builds; the panel shows any errors.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

//! DESA Map Viewer: pick a level from your disc image and fly around it.
//!
//! Reads levels straight from the disc (or a folder of extracted `.prg`
//! archives), so no unpacking step is needed. The disc's location is
//! remembered between runs. Any of the playable characters can stand on a
//! spawn point and play their animations.

mod settings;
mod ui;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use clap::Parser;
use glam::{Mat4, Quat, Vec3};
use ngc_anim::CameraPath;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{
    DeviceEvent, DeviceId, ElementState, MouseButton, MouseScrollDelta, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

use desa_viewer::camera::{FlyCamera, ScriptedCamera, vertical_fov};
use desa_viewer::character::Character;
use desa_viewer::collision::{self, CollisionView};
use desa_viewer::level::Level;
use desa_viewer::nodes::{LevelNodes, Spawn};
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
    renderer: Renderer,
    /// Collision triangles, for keeping spawn cameras out of walls.
    collision: Vec<desa_viewer::collision::ColorVertex>,
    nodes: LevelNodes,
    start: FlyCamera,
    /// Where a character stands until you go to a spawn point.
    home: Mat4,
    /// The level's camera paths, by name.
    camera_paths: Vec<(String, CameraPath)>,
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
    let files = data
        .load_level(&info.id)
        .with_context(|| format!("could not read {}", info.title))?;
    let world = Level::from_bytes(&files.scene, files.textures.as_deref())?;
    let sky = files
        .sky
        .as_ref()
        .and_then(|(scene, tex)| Level::from_bytes(scene, tex.as_deref()).ok());
    let collision = files
        .collision
        .as_deref()
        .and_then(|c| collision::from_bytes(c).ok());
    let nodes = files
        .nodes
        .as_deref()
        .and_then(|n| LevelNodes::from_bytes(n).ok())
        .unwrap_or_default();

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
        "{} triangles, {} textures, {} rail segments, {} spawn points{}",
        world.indices.len() / 3,
        world.textures.len() - 1,
        nodes.rails.len(),
        nodes.spawns.len(),
        if collision.is_some() {
            ", collision"
        } else {
            ""
        },
    );
    let renderer = Renderer::new(
        device.clone(),
        queue.clone(),
        format,
        &world,
        sky.as_ref(),
        collision.as_deref(),
    );
    Ok((
        LoadedLevel {
            renderer,
            collision: collision.unwrap_or_default(),
            nodes,
            start,
            home,
            camera_paths,
            markers: (false, false),
        },
        stats,
    ))
}

/// Where a character stands at a spawn point, facing its way. Character
/// models face +Z.
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
    looking: bool,
    /// A level to load, and how many frames the "Loading" message has shown.
    pending: Option<(usize, u32)>,
    next_spawn: usize,
    character: Option<Character>,
    /// Where the character stands.
    placement: Mat4,
    /// The animation the character's time belongs to.
    shown_animation: usize,
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
                },
            },
            camera: FlyCamera::looking_at(Vec3::new(0.0, 500.0, 1000.0), Vec3::ZERO),
            keys: HashSet::new(),
            looking: false,
            pending: None,
            next_spawn: 0,
            character: None,
            placement: Mat4::IDENTITY,
            shown_animation: 0,
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

    fn go_to_spawn(&mut self, index: usize) {
        self.stop_camera_path();
        if let Some(spawn) = self.level.as_ref().and_then(|l| l.nodes.spawns.get(index)) {
            self.camera = spawn.camera_clear_of(&self.level.as_ref().unwrap().collision);
            self.placement = placement_at(spawn);
            self.next_spawn = index + 1;
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
        self.character = None;
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
        self.play(dt);
        self.sync_view();
        self.animate(dt, self.started.elapsed().as_secs_f32());
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
            KeyCode::Escape => self.set_looking(false),
            KeyCode::F1 if !repeat => self.model.panel_open = !self.model.panel_open,
            KeyCode::KeyK if !repeat && self.model.has_collision => {
                self.model.collision = self.model.collision.next();
            }
            KeyCode::Tab if !repeat => {
                let count = self.level.as_ref().map_or(0, |l| l.nodes.spawns.len());
                if count > 0 {
                    self.go_to_spawn(self.next_spawn % count);
                }
            }
            KeyCode::KeyR if !repeat => {
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

    let mut settings = Settings::default();
    let mut app = App::new(&mut settings);
    app.model.levels = levels;
    app.model.data_path = Some(data_path.display().to_string());
    app.model.current = Some(index);
    app.model.stats = Some(stats);
    app.model.has_collision = loaded.renderer.has_collision();
    app.model.set_spawns(&loaded.nodes.spawns);
    app.model.camera_text = loaded.start.describe();
    let mut camera = loaded.start;

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
