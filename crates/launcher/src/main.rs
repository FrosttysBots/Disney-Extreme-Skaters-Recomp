//! The launcher for Disney's Extreme Skate Adventure (the recomp): sets
//! it up on first run, keeps it up to date from GitHub, and starts the
//! game or the map viewer.
//!
//! `desa-launcher --screenshot out.png [--page play|notes|installs|settings|setup]`
//! draws one frame offscreen instead of opening a window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod install;
mod net;
mod ui;

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

use install::Config;
use ui::{Launcher, Page};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1).cloned())
    };
    if let Some(out) = flag("--screenshot") {
        let page = flag("--page").unwrap_or_else(|| "play".into());
        return screenshot(&out, &page);
    }
    if let Some(i) = args.iter().position(|a| a == "--fetch") {
        // `--fetch <tag> <dir>`: download a release into `dir`, for
        // checking the downloads without the window or the saved config.
        let (Some(tag), Some(dir)) = (args.get(i + 1), args.get(i + 2)) else {
            anyhow::bail!("--fetch <tag> <dir>");
        };
        let releases = net::releases()?;
        let release = releases
            .iter()
            .find(|r| &r.tag == tag)
            .with_context(|| format!("no release {tag}"))?;
        let config = Config {
            install_dir: dir.into(),
            ..Config::default()
        };
        install::install_version(&config, release, &mut |p| {
            eprint!(
                "
{} {}/{}      ",
                p.what, p.done, p.total
            );
        })?;
        println!(
            "
installed: {:?}",
            config.installed()
        );
        return Ok(());
    }
    let event_loop = EventLoop::new()?;
    let mut app = App {
        gpu: None,
        launcher: Launcher::new(Config::load()),
        error: None,
    };
    event_loop.run_app(&mut app)?;
    if let Some(e) = app.error {
        return Err(e);
    }
    Ok(())
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

struct App {
    gpu: Option<Gpu>,
    launcher: Launcher,
    error: Option<anyhow::Error>,
}

impl App {
    fn init(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        let attributes = Window::default_attributes()
            .with_title("Disney's Extreme Skate Adventure Launcher")
            .with_inner_size(LogicalSize::new(1100.0, 680.0))
            .with_min_inner_size(LogicalSize::new(900.0, 580.0));
        let window = Arc::new(event_loop.create_window(attributes)?);
        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(window.clone())?;
        let (adapter, device, queue) = pollster::block_on(desa_viewer::renderer::request_device(
            &instance,
            Some(&surface),
        ))?;
        let caps = surface.get_capabilities(&adapter);
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
        ui::install_fonts(&egui_ctx);
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
        Ok(())
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        let Some(gpu) = &mut self.gpu else {
            return;
        };
        let raw = gpu.egui_state.take_egui_input(&gpu.window);
        let launcher = &mut self.launcher;
        let output = gpu.egui_ctx.run(raw, |ctx| launcher.ui(ctx));
        // The disc's logo, once read, on the title bar and the taskbar.
        if let Some((rgba, w, h)) = self.launcher.icon.take() {
            let icon = |rgba: Vec<u8>| winit::window::Icon::from_rgba(rgba, w, h).ok();
            #[cfg(windows)]
            {
                use winit::platform::windows::WindowExtWindows;
                gpu.window.set_taskbar_icon(icon(rgba.clone()));
            }
            gpu.window.set_window_icon(icon(rgba));
        }
        gpu.egui_state
            .handle_platform_output(&gpu.window, output.platform_output.clone());
        if self.launcher.quit {
            event_loop.exit();
            return;
        }
        let frame = match gpu.surface.get_current_texture() {
            Ok(f) => f,
            Err(_) => {
                gpu.surface.configure(&gpu.device, &gpu.config);
                return;
            }
        };
        let view = frame.texture.create_view(&Default::default());
        paint(
            &gpu.device,
            &gpu.queue,
            &mut gpu.egui_renderer,
            &gpu.egui_ctx,
            output,
            &view,
            [gpu.config.width, gpu.config.height],
        );
        frame.present();
    }
}

/// egui's shapes onto `view`, cleared to the launcher's navy first.
fn paint(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    renderer: &mut egui_wgpu::Renderer,
    ctx: &egui::Context,
    output: egui::FullOutput,
    view: &wgpu::TextureView,
    size: [u32; 2],
) {
    let mut encoder = device.create_command_encoder(&Default::default());
    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: size,
        pixels_per_point: output.pixels_per_point,
    };
    for (id, delta) in &output.textures_delta.set {
        renderer.update_texture(device, queue, *id, delta);
    }
    let jobs = ctx.tessellate(output.shapes, output.pixels_per_point);
    let extra = renderer.update_buffers(device, queue, &mut encoder, &jobs, &screen);
    {
        let [r, g, b, _] = ui::NAVY.to_array();
        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("launcher"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: r as f64 / 255.0,
                        g: g as f64 / 255.0,
                        b: b as f64 / 255.0,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        renderer.render(&mut pass.forget_lifetime(), &jobs, &screen);
    }
    queue.submit(extra.into_iter().chain([encoder.finish()]));
    for id in &output.textures_delta.free {
        renderer.free_texture(id);
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_none()
            && let Err(e) = self.init(event_loop)
        {
            self.error = Some(e);
            event_loop.exit();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        if let Some(gpu) = &mut self.gpu {
            let _ = gpu.egui_state.on_window_event(&gpu.window, &event);
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(gpu) = &mut self.gpu {
                    gpu.config.width = size.width.max(1);
                    gpu.config.height = size.height.max(1);
                    gpu.surface.configure(&gpu.device, &gpu.config);
                    gpu.window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => self.redraw(event_loop),
            _ => {
                if let Some(gpu) = &self.gpu {
                    gpu.window.request_redraw();
                }
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // The art's slow drift and the downloads' progress: ~30 frames a
        // second is plenty.
        if let Some(gpu) = &self.gpu {
            gpu.window.request_redraw();
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(
            Instant::now() + Duration::from_millis(33),
        ));
    }
}

/// One frame of `page` drawn offscreen to a PNG, with the releases and
/// art fetched first.
fn screenshot(out: &str, page: &str) -> Result<()> {
    let (width, height) = (1100u32, 680u32);
    let mut config = Config::load();
    let setup = page == "setup";
    config.setup_done = !setup;
    let mut launcher = Launcher::new(config);
    launcher.screenshot = true;
    if let Some(p) = Page::parse(page) {
        launcher.page = p;
    }
    launcher.wait_for_data();

    let instance = wgpu::Instance::default();
    let (_adapter, device, queue) =
        pollster::block_on(desa_viewer::renderer::request_device(&instance, None))?;
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let ctx = egui::Context::default();
    ui::install_fonts(&ctx);
    // No fading in: one frame has to show everything.
    ctx.style_mut(|s| s.animation_time = 0.0);
    let mut renderer =
        egui_wgpu::Renderer::new(&device, format, egui_wgpu::RendererOptions::default());
    let input = || egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(width as f32, height as f32),
        )),
        ..Default::default()
    };
    // Two passes to settle the layout, keeping the first's uploads.
    let first = ctx.run(input(), |c| launcher.ui(c));
    let mut output = ctx.run(input(), |c| launcher.ui(c));
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
    let view = target.create_view(&Default::default());
    paint(
        &device,
        &queue,
        &mut renderer,
        &ctx,
        output,
        &view,
        [width, height],
    );

    let row = (width * 4).div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: (row * height) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .context("waiting for the GPU")?;
    let data = slice.get_mapped_range();
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        let start = (y * row) as usize;
        rgba.extend_from_slice(&data[start..start + (width * 4) as usize]);
    }
    let file = std::fs::File::create(out).with_context(|| format!("can't write {out}"))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&rgba)?;
    println!("wrote {out}");
    Ok(())
}
