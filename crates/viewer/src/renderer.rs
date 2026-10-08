//! Draws a level, its sky and a character with wgpu.
//!
//! Order: sky (no depth test, camera-centered), then opaque materials, then
//! transparent materials by draw order, then any extra layers (objects,
//! pedestrians), then the character (posed on the CPU; see `character`).
//! Characters and lit layers get simple directional lighting. Every material draws all of its
//! passes, each with the engine's blend mode for that pass. Depth uses
//! reversed Z with an infinite far plane, which keeps precision across
//! levels that span more than 100,000 units.

use std::fs::File;
use std::io::BufWriter;
use std::ops::Range;
use std::path::Path;
use std::sync::mpsc;

use anyhow::{Context, Result};
use bytemuck::{Pod, Zeroable};
use glam::Mat4;
use wgpu::util::DeviceExt;

use crate::camera::{FlyCamera, ScriptedCamera};
use crate::collision::{CollisionView, ColorVertex};
use crate::level::{Level, PassDraw, TextureData, Vertex, blend};

const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const CLEAR_COLOR: wgpu::Color = wgpu::Color {
    r: 0.25,
    g: 0.35,
    b: 0.5,
    a: 1.0,
};
const FOV_Y_DEGREES: f32 = 70.0;
const NEAR: f32 = 4.0;
/// Uniform buffer bindings must start at multiples of this.
const PARAMS_STRIDE: u64 = 256;

/// How a pass combines with what's already drawn.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Opaque,
    Add,
    Subtract,
    Blend,
    Modulate,
    Brighten,
}

const KINDS: [Kind; 6] = [
    Kind::Opaque,
    Kind::Add,
    Kind::Subtract,
    Kind::Blend,
    Kind::Modulate,
    Kind::Brighten,
];

impl Kind {
    fn of(mode: u32, first_pass: bool) -> Kind {
        match mode {
            blend::DIFFUSE if first_pass => Kind::Opaque,
            blend::ADD | blend::ADD_FIXED => Kind::Add,
            blend::SUBTRACT | blend::SUB_FIXED => Kind::Subtract,
            blend::MODULATE | blend::MODULATE_FIXED => Kind::Modulate,
            blend::BRIGHTEN | blend::BRIGHTEN_FIXED => Kind::Brighten,
            // BLEND, BLEND_FIXED, a diffuse layer, and unknown modes.
            _ => Kind::Blend,
        }
    }

    fn blend_state(self) -> Option<wgpu::BlendState> {
        use wgpu::{BlendComponent as C, BlendFactor as F, BlendOperation as O};
        let color = |src, dst, operation| C {
            src_factor: src,
            dst_factor: dst,
            operation,
        };
        let keep_alpha = color(F::Zero, F::One, O::Add);
        let state = |c| {
            Some(wgpu::BlendState {
                color: c,
                alpha: keep_alpha,
            })
        };
        match self {
            Kind::Opaque => None,
            Kind::Add => state(color(F::SrcAlpha, F::One, O::Add)),
            Kind::Subtract => state(color(F::SrcAlpha, F::One, O::ReverseSubtract)),
            Kind::Blend => state(color(F::SrcAlpha, F::OneMinusSrcAlpha, O::Add)),
            // The shader outputs alpha as the color for these two.
            Kind::Modulate => state(color(F::Dst, F::Zero, O::Add)),
            Kind::Brighten => state(color(F::Dst, F::One, O::Add)),
        }
    }

    fn index(self) -> usize {
        KINDS.iter().position(|&k| k == self).unwrap()
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GlobalsData {
    view_proj: [f32; 16],
    view: [f32; 16],
    time: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct PassParams {
    wibble_velocity_frequency: [f32; 4],
    wibble_amplitude_phase: [f32; 4],
    settings: [f32; 4],
    options: [f32; 4],
}

impl PassParams {
    fn new(pass: &PassDraw, kind: Kind, lit: bool) -> Self {
        let w = pass.uv_wibble;
        Self {
            wibble_velocity_frequency: [w[0], w[1], w[2], w[3]],
            wibble_amplitude_phase: [w[4], w[5], w[6], w[7]],
            settings: [
                pass.uv_set as f32,
                pass.fixed_alpha.unwrap_or(-1.0),
                pass.alpha_threshold,
                if pass.environment { 1.0 } else { 0.0 },
            ],
            options: [
                if matches!(kind, Kind::Modulate | Kind::Brighten) {
                    1.0
                } else {
                    0.0
                },
                if lit { 1.0 } else { 0.0 },
                0.0,
                0.0,
            ],
        }
    }
}

struct GpuPass {
    kind: Kind,
    bind_group: wgpu::BindGroup,
    /// The texture slot the material uses, and the one currently bound
    /// (they differ while a texture is swapped, as when blinking).
    texture: usize,
    shown: usize,
    /// This pass's slot in `GpuLevel::params`.
    params_slot: u64,
}

struct GpuBatch {
    indices: Range<u32>,
    passes: Vec<GpuPass>,
    transparent: bool,
    draw_order: f32,
}

struct GpuLevel {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    /// Opaque batches in file order, then transparent ones by draw order.
    batches: Vec<GpuBatch>,
    /// Kept for rebinding passes to other textures.
    views: Vec<wgpu::TextureView>,
    params: wgpu::Buffer,
}

struct Globals {
    buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

pub struct Renderer {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    world_pipelines: Vec<wgpu::RenderPipeline>,
    sky_pipelines: Vec<wgpu::RenderPipeline>,
    world_globals: Globals,
    sky_globals: Globals,
    world: GpuLevel,
    sky: Option<GpuLevel>,
    /// Its vertex buffer is rewritten every frame with the posed vertices.
    character: Option<GpuLevel>,
    /// Extra meshes drawn with the level, and whether each is shown.
    layers: Vec<(GpuLevel, bool)>,
    pass_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    collision: Option<(wgpu::Buffer, u32)>,
    /// Rails and spawn markers.
    markers: Option<(wgpu::Buffer, u32)>,
    /// The skater's shadow: translucent triangles laid on the ground.
    shadow: Option<(wgpu::Buffer, u32)>,
    color_overlay: wgpu::RenderPipeline,
    color_solid: wgpu::RenderPipeline,
    pub collision_view: CollisionView,
    pub show_sky: bool,
    /// When set, frames are drawn from this camera instead of the one
    /// passed to `render` / `encode` (for playing camera paths).
    pub scripted_camera: Option<ScriptedCamera>,
    /// Minimum vertex lighting (0 = as the game lights it). Raising it
    /// shows detail in dark corners.
    pub min_light: f32,
    depth: Option<(wgpu::TextureView, u32, u32)>,
}

impl Renderer {
    pub fn new(
        device: wgpu::Device,
        queue: wgpu::Queue,
        color_format: wgpu::TextureFormat,
        world: &Level,
        sky: Option<&Level>,
        collision: Option<&[ColorVertex]>,
    ) -> Self {
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals"),
            entries: &[uniform_entry(0, wgpu::ShaderStages::VERTEX)],
        });
        let pass_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("pass"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                uniform_entry(2, wgpu::ShaderStages::VERTEX_FRAGMENT),
            ],
        });

        let shader = device.create_shader_module(wgpu::include_wgsl!("shader.wgsl"));
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("level"),
            bind_group_layouts: &[&globals_layout, &pass_layout],
            push_constant_ranges: &[],
        });
        let pipelines = |sky: bool| -> Vec<wgpu::RenderPipeline> {
            KINDS
                .iter()
                .map(|&kind| make_pipeline(&device, &layout, &shader, color_format, kind, sky))
                .collect()
        };
        let world_pipelines = pipelines(false);
        let sky_pipelines = pipelines(true);

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("level"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            anisotropy_clamp: 8,
            ..Default::default()
        });

        let world_globals = make_globals(&device, &globals_layout);
        let sky_globals = make_globals(&device, &globals_layout);
        let world = upload_level(&device, &queue, &pass_layout, &sampler, world, false);
        let sky = sky.map(|sky| upload_level(&device, &queue, &pass_layout, &sampler, sky, false));
        let collision = collision.and_then(|vertices| color_buffer(&device, "collision", vertices));
        let color_overlay = make_collision_pipeline(&device, &globals_layout, color_format, true);
        let color_solid = make_collision_pipeline(&device, &globals_layout, color_format, false);

        Self {
            device,
            queue,
            world_pipelines,
            sky_pipelines,
            world_globals,
            sky_globals,
            world,
            sky,
            character: None,
            layers: Vec::new(),
            pass_layout,
            sampler,
            collision,
            markers: None,
            shadow: None,
            color_overlay,
            color_solid,
            collision_view: CollisionView::Hidden,
            show_sky: true,
            scripted_camera: None,
            min_light: 0.0,
            depth: None,
        }
    }

    pub fn has_collision(&self) -> bool {
        self.collision.is_some()
    }

    /// Replaces the rail and spawn geometry (empty hides it).
    pub fn set_markers(&mut self, vertices: &[ColorVertex]) {
        self.markers = color_buffer(&self.device, "markers", vertices);
    }

    /// The character's shadow (translucent, drawn over the level), or
    /// none.
    pub fn set_shadow(&mut self, vertices: &[ColorVertex]) {
        self.shadow = color_buffer(&self.device, "shadow", vertices);
    }

    /// Uploads a character's mesh (`None` removes it). Its vertices are
    /// then replaced each frame with [`pose_character`](Self::pose_character).
    pub fn set_character(&mut self, mesh: Option<&Level>) {
        self.character = mesh.map(|mesh| {
            upload_level(
                &self.device,
                &self.queue,
                &self.pass_layout,
                &self.sampler,
                mesh,
                true,
            )
        });
    }

    /// Adds a mesh drawn with the level (`lit`: lighting by normal, as for
    /// characters) and returns its id, or `None` if it's empty. Layers can
    /// be hidden and have their vertices replaced, like the character's.
    pub fn add_layer(&mut self, mesh: &Level, lit: bool) -> Option<usize> {
        if mesh.indices.is_empty() {
            return None;
        }
        let gpu = upload_level(
            &self.device,
            &self.queue,
            &self.pass_layout,
            &self.sampler,
            mesh,
            lit,
        );
        self.layers.push((gpu, true));
        Some(self.layers.len() - 1)
    }

    pub fn show_layer(&mut self, id: Option<usize>, shown: bool) {
        if let Some(layer) = id.and_then(|id| self.layers.get_mut(id)) {
            layer.1 = shown;
        }
    }

    /// Replaces a layer's vertices from `first` on (same order as its mesh).
    pub fn update_layer(&self, id: Option<usize>, first: usize, vertices: &[Vertex]) {
        if let Some((layer, _)) = id.and_then(|id| self.layers.get(id)) {
            write_vertices(&self.queue, layer, first, vertices);
        }
    }

    /// Replaces the level's vertices (or the sky's) from `first` on, for
    /// animated vertex colors.
    pub fn update_world(&self, sky: bool, first: usize, vertices: &[Vertex]) {
        let target = if sky {
            self.sky.as_ref()
        } else {
            Some(&self.world)
        };
        if let Some(level) = target {
            write_vertices(&self.queue, level, first, vertices);
        }
    }

    /// Replaces the character's vertices, which must match the mesh given
    /// to [`set_character`](Self::set_character) in count and order.
    pub fn pose_character(&self, vertices: &[Vertex]) {
        if let Some(character) = &self.character {
            self.queue
                .write_buffer(&character.vertices, 0, bytemuck::cast_slice(vertices));
        }
    }

    /// Draws the character's passes that use texture slot `from` with
    /// slot `to` instead (`to == from` restores them). Slots index the
    /// textures of the mesh given to [`set_character`](Self::set_character).
    pub fn swap_character_texture(&mut self, from: usize, to: usize) {
        let Some(character) = &mut self.character else {
            return;
        };
        if to >= character.views.len() {
            return;
        }
        for pass in character.batches.iter_mut().flat_map(|b| &mut b.passes) {
            if pass.texture == from && pass.shown != to {
                pass.bind_group = pass_bind_group(
                    &self.device,
                    &self.pass_layout,
                    &character.views[to],
                    &self.sampler,
                    &character.params,
                    pass.params_slot,
                );
                pass.shown = to;
            }
        }
    }

    /// Renders one frame into `target` and submits it. `time` drives
    /// scrolling textures.
    pub fn render(
        &mut self,
        target: &wgpu::TextureView,
        width: u32,
        height: u32,
        camera: &FlyCamera,
        time: f32,
    ) {
        let mut encoder = self.device.create_command_encoder(&Default::default());
        self.encode(&mut encoder, target, width, height, camera, time);
        self.queue.submit([encoder.finish()]);
    }

    /// Records one frame into `encoder` without submitting, so callers can
    /// draw more (such as a user interface) on top.
    pub fn encode(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        width: u32,
        height: u32,
        camera: &FlyCamera,
        time: f32,
    ) {
        if self
            .depth
            .as_ref()
            .is_none_or(|(_, w, h)| (*w, *h) != (width, height))
        {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("depth"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            self.depth = Some((texture.create_view(&Default::default()), width, height));
        }

        let aspect = width as f32 / height.max(1) as f32;
        let (view, rotation_view, fov_y) = match self.scripted_camera {
            Some(c) => (
                Mat4::from_rotation_translation(c.rotation, c.position).inverse(),
                Mat4::from_quat(c.rotation.conjugate()),
                c.fov_y,
            ),
            None => (
                camera.view(),
                camera.rotation_view(),
                FOV_Y_DEGREES.to_radians(),
            ),
        };
        let projection =
            glam::camera::rh::proj::directx::perspective_infinite_reverse(fov_y, aspect, NEAR);
        let min_light = self.min_light;
        let write = |globals: &Globals, view: Mat4| {
            let data = GlobalsData {
                view_proj: (projection * view).to_cols_array(),
                view: view.to_cols_array(),
                time: [time, min_light, 0.0, 0.0],
            };
            self.queue
                .write_buffer(&globals.buffer, 0, bytemuck::bytes_of(&data));
        };
        write(&self.world_globals, view);
        write(&self.sky_globals, rotation_view);

        {
            let (depth_view, _, _) = self.depth.as_ref().unwrap();
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("frame"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(CLEAR_COLOR),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth_view,
                    // Reversed Z: 0 is infinitely far away.
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            let collision = self
                .collision
                .as_ref()
                .filter(|_| self.collision_view != CollisionView::Hidden);
            let only_collision = collision.is_some() && self.collision_view == CollisionView::Only;
            if !only_collision {
                if let Some(sky) = self.sky.as_ref().filter(|_| self.show_sky) {
                    pass.set_bind_group(0, &self.sky_globals.bind_group, &[]);
                    draw(&mut pass, sky, &self.sky_pipelines);
                }
                pass.set_bind_group(0, &self.world_globals.bind_group, &[]);
                draw(&mut pass, &self.world, &self.world_pipelines);
                for (layer, _) in self.layers.iter().filter(|l| l.1) {
                    draw(&mut pass, layer, &self.world_pipelines);
                }
            }
            if let Some(character) = &self.character {
                pass.set_bind_group(0, &self.world_globals.bind_group, &[]);
                draw(&mut pass, character, &self.world_pipelines);
            }
            pass.set_bind_group(0, &self.world_globals.bind_group, &[]);
            if let Some((buffer, count)) = self.shadow.as_ref().filter(|_| !only_collision) {
                pass.set_pipeline(&self.color_overlay);
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.draw(0..*count, 0..1);
            }
            if let Some((buffer, count)) = &self.markers {
                pass.set_pipeline(&self.color_solid);
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.draw(0..*count, 0..1);
            }
            if let Some((buffer, count)) = collision {
                pass.set_pipeline(if only_collision {
                    &self.color_solid
                } else {
                    &self.color_overlay
                });
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.draw(0..*count, 0..1);
            }
        }
    }
}

fn write_vertices(queue: &wgpu::Queue, level: &GpuLevel, first: usize, vertices: &[Vertex]) {
    if !vertices.is_empty() {
        let offset = (first * std::mem::size_of::<Vertex>()) as u64;
        queue.write_buffer(&level.vertices, offset, bytemuck::cast_slice(vertices));
    }
}

fn color_buffer(
    device: &wgpu::Device,
    label: &str,
    vertices: &[ColorVertex],
) -> Option<(wgpu::Buffer, u32)> {
    if vertices.is_empty() {
        return None;
    }
    let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: bytemuck::cast_slice(vertices),
        usage: wgpu::BufferUsages::VERTEX,
    });
    Some((buffer, vertices.len() as u32))
}

/// Draws colored triangles: translucent over the level (`overlay`), or
/// solid on their own.
fn make_collision_pipeline(
    device: &wgpu::Device,
    globals_layout: &wgpu::BindGroupLayout,
    color_format: wgpu::TextureFormat,
    overlay: bool,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::include_wgsl!("collision.wgsl"));
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("collision"),
        bind_group_layouts: &[globals_layout],
        push_constant_ranges: &[],
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("collision"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<ColorVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Unorm8x4],
            }],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some(if overlay { "fs_overlay" } else { "fs_solid" }),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: color_format,
                blend: overlay.then_some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: !overlay,
            depth_compare: wgpu::CompareFunction::GreaterEqual,
            stencil: Default::default(),
            // Pull the overlay slightly toward the camera (reversed Z: larger
            // is closer) so it wins against coincident level geometry.
            bias: if overlay {
                wgpu::DepthBiasState {
                    constant: 4,
                    slope_scale: 2.0,
                    clamp: 0.0,
                }
            } else {
                Default::default()
            },
        }),
        multisample: Default::default(),
        multiview: None,
        cache: None,
    })
}

fn draw(pass: &mut wgpu::RenderPass, level: &GpuLevel, pipelines: &[wgpu::RenderPipeline]) {
    pass.set_vertex_buffer(0, level.vertices.slice(..));
    pass.set_index_buffer(level.indices.slice(..), wgpu::IndexFormat::Uint32);
    for batch in &level.batches {
        for layer in &batch.passes {
            pass.set_pipeline(&pipelines[layer.kind.index()]);
            pass.set_bind_group(1, &layer.bind_group, &[]);
            pass.draw_indexed(batch.indices.clone(), 0, 0..1);
        }
    }
}

fn uniform_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn make_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    color_format: wgpu::TextureFormat,
    kind: Kind,
    sky: bool,
) -> wgpu::RenderPipeline {
    let opaque = kind == Kind::Opaque;
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(if sky { "sky" } else { "world" }),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<Vertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![
                    0 => Float32x3, 1 => Float32x3, 2 => Float32x2,
                    3 => Float32x2, 4 => Float32x2, 5 => Unorm8x4
                ],
            }],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(if opaque { "fs_opaque" } else { "fs_blend" }),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: color_format,
                blend: kind.blend_state(),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        // Level geometry isn't consistently single-sided, so draw both faces.
        primitive: wgpu::PrimitiveState {
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            // Only opaque world passes write depth. Later passes of the same
            // material sit at exactly the same depth, hence GreaterEqual.
            depth_write_enabled: opaque && !sky,
            depth_compare: if sky {
                wgpu::CompareFunction::Always
            } else {
                wgpu::CompareFunction::GreaterEqual
            },
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: Default::default(),
        multiview: None,
        cache: None,
    })
}

fn make_globals(device: &wgpu::Device, layout: &wgpu::BindGroupLayout) -> Globals {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("globals"),
        size: std::mem::size_of::<GlobalsData>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("globals"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: buffer.as_entire_binding(),
        }],
    });
    Globals { buffer, bind_group }
}

fn upload_level(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pass_layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    level: &Level,
    lit: bool,
) -> GpuLevel {
    let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("vertices"),
        contents: bytemuck::cast_slice(&level.vertices),
        // Characters and layers have their vertices replaced as they move.
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
    });
    let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("indices"),
        contents: bytemuck::cast_slice(&level.indices),
        usage: wgpu::BufferUsages::INDEX,
    });
    let views: Vec<wgpu::TextureView> = level
        .textures
        .iter()
        .map(|t| upload_texture(device, queue, t))
        .collect();

    // Every pass gets a 256-byte slot in one uniform buffer.
    let mut params = Vec::new();
    for batch in &level.batches {
        for (i, pass) in batch.passes.iter().enumerate() {
            let mut slot = [0u8; PARAMS_STRIDE as usize];
            let data = PassParams::new(pass, Kind::of(pass.blend_mode, i == 0), lit);
            slot[..std::mem::size_of::<PassParams>()].copy_from_slice(bytemuck::bytes_of(&data));
            params.extend_from_slice(&slot);
        }
    }
    let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("pass params"),
        contents: if params.is_empty() {
            &[0; PARAMS_STRIDE as usize]
        } else {
            &params
        },
        usage: wgpu::BufferUsages::UNIFORM,
    });

    let mut slot = 0u64;
    let mut batches: Vec<GpuBatch> = level
        .batches
        .iter()
        .map(|batch| {
            let passes = batch
                .passes
                .iter()
                .enumerate()
                .map(|(i, pass)| {
                    let bind_group = pass_bind_group(
                        device,
                        pass_layout,
                        &views[pass.texture],
                        sampler,
                        &params_buffer,
                        slot,
                    );
                    slot += 1;
                    GpuPass {
                        kind: Kind::of(pass.blend_mode, i == 0),
                        bind_group,
                        texture: pass.texture,
                        shown: pass.texture,
                        params_slot: slot - 1,
                    }
                })
                .collect();
            GpuBatch {
                indices: batch.first_index..batch.first_index + batch.index_count,
                passes,
                transparent: batch.transparent,
                draw_order: batch.draw_order,
            }
        })
        .collect();
    // Opaque first (stable, keeps file order), then transparent by draw order.
    batches.sort_by(|a, b| {
        a.transparent.cmp(&b.transparent).then(if a.transparent {
            a.draw_order.total_cmp(&b.draw_order)
        } else {
            std::cmp::Ordering::Equal
        })
    });
    GpuLevel {
        vertices,
        indices,
        batches,
        views,
        params: params_buffer,
    }
}

fn pass_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    texture: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
    params: &wgpu::Buffer,
    params_slot: u64,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("pass"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(texture),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: params,
                    offset: params_slot * PARAMS_STRIDE,
                    size: wgpu::BufferSize::new(std::mem::size_of::<PassParams>() as u64),
                }),
            },
        ],
    })
}

fn upload_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    data: &TextureData,
) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("texture"),
        size: wgpu::Extent3d {
            width: data.width,
            height: data.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: data.levels.len() as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        // Not sRGB: the game blends in gamma space, so the shader does too.
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (level, pixels) in data.levels.iter().enumerate() {
        let width = (data.width >> level).max(1);
        let height = (data.height >> level).max(1);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: level as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * width),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
    }
    texture.create_view(&Default::default())
}

pub async fn request_device(
    instance: &wgpu::Instance,
    surface: Option<&wgpu::Surface<'_>>,
) -> Result<(wgpu::Adapter, wgpu::Device, wgpu::Queue)> {
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: surface,
        })
        .await
        .context("no suitable graphics adapter")?;
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("desa-viewer"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            trace: wgpu::Trace::Off,
        })
        .await
        .context("could not open the graphics device")?;
    Ok((adapter, device, queue))
}

/// What a screenshot shows and how big it is.
pub struct Shot<'a> {
    pub camera: &'a FlyCamera,
    /// Rail and spawn geometry (may be empty).
    pub markers: &'a [ColorVertex],
    pub width: u32,
    pub height: u32,
    /// Animation time in seconds.
    pub time: f32,
    /// Minimum light level (see `Renderer::min_light`).
    pub brighten: f32,
}

/// Renders one frame off-screen and saves it as a PNG.
pub fn screenshot(
    world: &Level,
    sky: Option<&Level>,
    collision: Option<(&[ColorVertex], CollisionView)>,
    shot: &Shot,
    out: &Path,
) -> Result<()> {
    let Shot {
        camera,
        markers,
        width,
        height,
        time,
        brighten,
    } = *shot;
    let instance = wgpu::Instance::default();
    let (_adapter, device, queue) = pollster::block_on(request_device(&instance, None))?;
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut renderer = Renderer::new(
        device.clone(),
        queue.clone(),
        format,
        world,
        sky,
        collision.map(|c| c.0),
    );
    if let Some((_, view)) = collision {
        renderer.collision_view = view;
    }
    renderer.set_markers(markers);
    renderer.min_light = brighten;

    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("screenshot"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    renderer.render(
        &target.create_view(&Default::default()),
        width,
        height,
        camera,
        time,
    );

    save_png(&device, &queue, &target, width, height, out)
}

/// Copies a rendered `Rgba8Unorm` texture back from the GPU and saves it as a PNG.
pub fn save_png(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    target: &wgpu::Texture,
    width: u32,
    height: u32,
    out: &Path,
) -> Result<()> {
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    // Rows in a texture-to-buffer copy must be padded to 256 bytes.
    let row = 4 * width;
    let padded_row = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("screenshot readback"),
        size: u64::from(padded_row) * u64::from(height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_row),
                rows_per_image: Some(height),
            },
        },
        size,
    );
    queue.submit([encoder.finish()]);

    let slice = buffer.slice(..);
    let (sender, receiver) = mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.send(result);
    });
    device.poll(wgpu::PollType::wait_indefinitely())?;
    receiver.recv()?.context("could not read back the frame")?;

    let mapped = slice.get_mapped_range();
    let mut pixels = Vec::with_capacity((row * height) as usize);
    for line in mapped.chunks(padded_row as usize) {
        pixels.extend_from_slice(&line[..row as usize]);
    }
    // A window's frame is often blue-green-red: put red first.
    if matches!(
        target.format(),
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
    ) {
        for pixel in pixels.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
    }
    let file = File::create(out).with_context(|| format!("could not create {}", out.display()))?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&pixels)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blend_modes_map_to_pipeline_kinds() {
        assert!(Kind::of(blend::DIFFUSE, true) == Kind::Opaque);
        // A diffuse layer on top of another pass still has to blend.
        assert!(Kind::of(blend::DIFFUSE, false) == Kind::Blend);
        assert!(Kind::of(blend::ADD_FIXED, false) == Kind::Add);
        assert!(Kind::of(blend::SUB_FIXED, true) == Kind::Subtract);
        assert!(Kind::of(blend::BLEND_FIXED, true) == Kind::Blend);
        assert!(Kind::of(blend::MODULATE, false) == Kind::Modulate);
        assert!(Kind::of(blend::BRIGHTEN_FIXED, false) == Kind::Brighten);
        assert!(Kind::of(42, false) == Kind::Blend);
        assert!(Kind::Opaque.blend_state().is_none());
    }

    #[test]
    fn fixed_modes_carry_a_constant_alpha() {
        let pass = PassDraw {
            texture: 0,
            uv_set: 1,
            blend_mode: blend::BLEND_FIXED,
            fixed_alpha: Some(0.5),
            alpha_threshold: 0.5,
            environment: true,
            uv_wibble: [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0],
        };
        let params = PassParams::new(&pass, Kind::of(pass.blend_mode, false), true);
        assert_eq!(params.settings, [1.0, 0.5, 0.5, 1.0]);
        assert_eq!(params.wibble_amplitude_phase, [5.0, 6.0, 7.0, 8.0]);
        assert_eq!(params.options, [0.0, 1.0, 0.0, 0.0]);
        assert!(std::mem::size_of::<PassParams>() as u64 <= PARAMS_STRIDE);
    }
}
