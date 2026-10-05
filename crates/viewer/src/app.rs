//! The interactive window: free-fly camera over a loaded level.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use glam::Vec3;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{
    DeviceEvent, DeviceId, ElementState, MouseButton, MouseScrollDelta, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

use crate::camera::FlyCamera;
use crate::collision::{CollisionVertex, CollisionView};
use crate::level::Level;
use crate::renderer::{Renderer, request_device};

pub const CONTROLS: &str = "\
Controls:
  Hold right mouse   look around
  W A S D            move
  E / Space          up
  Q / Ctrl           down
  Shift              move 5x faster
  Mouse wheel        change speed
  K                  cycle collision: hidden, overlay, only
  C                  print the camera (for --camera / --screenshot)
  R                  reset the camera
  Esc                quit";

const MOUSE_SENSITIVITY: f32 = 0.0025;

pub fn run(
    name: String,
    world: Level,
    sky: Option<Level>,
    collision: Option<Vec<CollisionVertex>>,
    collision_view: CollisionView,
    start: FlyCamera,
) -> Result<()> {
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let speed = (world.focus.1 * 0.15).clamp(100.0, 5000.0);
    let mut app = App {
        name,
        world: Some(world),
        sky,
        collision,
        collision_view,
        start,
        camera: start,
        speed,
        keys: HashSet::new(),
        looking: false,
        gpu: None,
        last_frame: Instant::now(),
        title_timer: Instant::now(),
        frames: 0,
        started: Instant::now(),
        error: None,
    };
    event_loop.run_app(&mut app)?;
    match app.error {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

struct Gpu {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
}

struct App {
    name: String,
    /// Moved into the renderer once the window exists.
    world: Option<Level>,
    sky: Option<Level>,
    collision: Option<Vec<CollisionVertex>>,
    collision_view: CollisionView,
    start: FlyCamera,
    camera: FlyCamera,
    speed: f32,
    keys: HashSet<KeyCode>,
    looking: bool,
    gpu: Option<Gpu>,
    last_frame: Instant,
    title_timer: Instant,
    frames: u32,
    started: Instant,
    error: Option<anyhow::Error>,
}

impl App {
    fn init(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        let attributes = Window::default_attributes()
            .with_title(format!("DESA viewer - {}", self.name))
            .with_inner_size(LogicalSize::new(1280.0, 720.0));
        let window = Arc::new(event_loop.create_window(attributes)?);

        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(window.clone())?;
        let (adapter, device, queue) =
            pollster::block_on(request_device(&instance, Some(&surface)))?;

        let caps = surface.get_capabilities(&adapter);
        // Prefer a non-sRGB format: the game's colors are already gamma-encoded.
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

        let world = self.world.take().expect("init runs once");
        let mut renderer = Renderer::new(
            device,
            queue,
            format,
            &world,
            self.sky.as_ref(),
            self.collision.as_deref(),
        );
        renderer.collision_view = self.collision_view;
        self.sky = None;
        self.collision = None;
        self.gpu = Some(Gpu {
            window,
            surface,
            config,
            renderer,
        });
        self.last_frame = Instant::now();
        Ok(())
    }

    fn update(&mut self, dt: f32) {
        let pressed = |k| self.keys.contains(&k);
        let mut direction = Vec3::ZERO;
        let (forward, right) = (self.camera.forward(), self.camera.right());
        if pressed(KeyCode::KeyW) {
            direction += forward;
        }
        if pressed(KeyCode::KeyS) {
            direction -= forward;
        }
        if pressed(KeyCode::KeyD) {
            direction += right;
        }
        if pressed(KeyCode::KeyA) {
            direction -= right;
        }
        if pressed(KeyCode::KeyE) || pressed(KeyCode::Space) {
            direction += Vec3::Y;
        }
        if pressed(KeyCode::KeyQ) || pressed(KeyCode::ControlLeft) {
            direction -= Vec3::Y;
        }
        let boost = if pressed(KeyCode::ShiftLeft) || pressed(KeyCode::ShiftRight) {
            5.0
        } else {
            1.0
        };
        self.camera.position += direction.normalize_or_zero() * self.speed * boost * dt;
    }

    fn redraw(&mut self) {
        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;
        self.update(dt);

        let Some(gpu) = &mut self.gpu else { return };
        let frame = match gpu.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Outdated | wgpu::SurfaceError::Lost) => {
                gpu.surface.configure(&gpu.renderer.device, &gpu.config);
                return;
            }
            Err(err) => {
                eprintln!("frame skipped: {err}");
                return;
            }
        };
        let view = frame.texture.create_view(&Default::default());
        let time = self.started.elapsed().as_secs_f32();
        gpu.renderer.render(
            &view,
            gpu.config.width,
            gpu.config.height,
            &self.camera,
            time,
        );
        frame.present();

        self.frames += 1;
        let elapsed = self.title_timer.elapsed().as_secs_f32();
        if elapsed >= 0.5 {
            gpu.window.set_title(&format!(
                "DESA viewer - {} - {:.0} fps - speed {:.0}",
                self.name,
                self.frames as f32 / elapsed,
                self.speed
            ));
            self.frames = 0;
            self.title_timer = Instant::now();
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
            // Some platforms only support one grab mode; looking still works without it.
            let _ = gpu
                .window
                .set_cursor_grab(mode)
                .or_else(|_| gpu.window.set_cursor_grab(CursorGrabMode::Locked));
            gpu.window.set_cursor_visible(!looking);
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_none() && self.world.is_some() {
            if let Err(err) = self.init(event_loop) {
                self.error = Some(err);
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(gpu) = &mut self.gpu {
                    gpu.config.width = size.width.max(1);
                    gpu.config.height = size.height.max(1);
                    gpu.surface.configure(&gpu.renderer.device, &gpu.config);
                }
            }
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::KeyboardInput { event, .. } => {
                let PhysicalKey::Code(code) = event.physical_key else {
                    return;
                };
                match event.state {
                    ElementState::Pressed => {
                        self.keys.insert(code);
                        match code {
                            KeyCode::Escape => event_loop.exit(),
                            KeyCode::KeyC if !event.repeat => {
                                println!("--camera {}", self.camera.describe())
                            }
                            KeyCode::KeyR => self.camera = self.start,
                            KeyCode::KeyK if !event.repeat => {
                                self.collision_view = self.collision_view.next();
                                if let Some(gpu) = &mut self.gpu {
                                    gpu.renderer.collision_view = self.collision_view;
                                }
                                println!("collision: {:?}", self.collision_view);
                            }
                            _ => {}
                        }
                    }
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
                self.set_looking(state == ElementState::Pressed);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let steps = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 / 50.0,
                };
                self.speed = (self.speed * 1.25f32.powf(steps)).clamp(10.0, 100_000.0);
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
