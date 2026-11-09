//! The look over the finished frame, after bevy_atmos's air and colour
//! grade (`air.rs`, `grade.rs`): distance fog and the grade, drawn from
//! the frame and its depth by one pass at the end. Left alone (`Look`'s
//! default), frames are drawn straight to the window as the game makes
//! them, and this costs nothing.

use wgpu::util::DeviceExt;

/// World units a metre (the game's are inches: a skater's about 70).
pub const UNITS_PER_METRE: f32 = 39.37;

/// What the look changes. The default changes nothing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    /// Metres to half-hidden in fog (0: none) (`fog.distance`).
    pub fog_distance: f32,
    /// Metres before it starts (`fog.start`).
    pub fog_start: f32,
    /// Its colour, as drawn (0..1, gamma).
    pub fog_color: [f32; 3],
    /// Stops (`grade.exposure`).
    pub exposure: f32,
    /// -1 cool .. 1 warm (`grade.warmth`).
    pub warmth: f32,
    /// -1 green .. 1 magenta (`grade.tint`).
    pub tint: f32,
    /// 1 as it is (`grade.contrast`).
    pub contrast: f32,
    /// 1 as it is, 0 grey (`grade.saturation`).
    pub saturation: f32,
    /// A physical sky in place of the level's dome (`sky.physical`).
    pub physical_sky: bool,
    /// Volumetric clouds' cover, 0 none .. 1 overcast (`sky.cover`).
    pub clouds: f32,
    /// The hour, 0..24 (below 0: the level's own) (`time.hour`).
    pub hour: f32,
    /// Game hours a real second while the day runs (0: it stands)
    /// (`time.speed`, in hours).
    pub day_speed: f32,
}

impl Default for Look {
    fn default() -> Self {
        Look {
            fog_distance: 0.0,
            fog_start: 0.0,
            fog_color: [0.62, 0.68, 0.76],
            exposure: 0.0,
            warmth: 0.0,
            tint: 0.0,
            contrast: 1.0,
            saturation: 1.0,
            physical_sky: false,
            clouds: 0.0,
            hour: -1.0,
            day_speed: 0.0,
        }
    }
}

impl Look {
    /// Whether it changes anything (else frames go straight to the window).
    pub fn is_on(&self) -> bool {
        self.fog_distance > 0.0
            || self.exposure != 0.0
            || self.warmth != 0.0
            || self.tint != 0.0
            || self.contrast != 1.0
            || self.saturation != 1.0
            || crate::atmos::wanted(self)
    }

    /// `world_light`: the time of day's light over the level's baked own.
    fn uniform(&self, near: f32, world_light: glam::Vec3) -> [f32; 16] {
        let [r, g, b] = self.fog_color;
        [
            r,
            g,
            b,
            self.fog_distance.max(0.0) * UNITS_PER_METRE,
            self.fog_start.max(0.0) * UNITS_PER_METRE,
            near,
            0.0,
            0.0,
            self.exposure,
            self.warmth.clamp(-1.0, 1.0),
            self.tint.clamp(-1.0, 1.0),
            self.contrast.clamp(0.2, 3.0),
            self.saturation.clamp(0.0, 3.0),
            world_light.x,
            world_light.y,
            world_light.z,
        ]
    }
}

/// The pass, and the frame and depth it reads (the window's size).
pub struct Post {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    format: wgpu::TextureFormat,
    targets: Option<Targets>,
}

struct Targets {
    color: wgpu::TextureView,
    depth: wgpu::TextureView,
    /// The level's dome alone, and the sky as drawn (the dome, or the
    /// sky pass's over it): what fog fades into.
    sky: wgpu::TextureView,
    sky_drawn: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
    /// Fog fading into `sky_drawn`.
    bind_group_drawn: wgpu::BindGroup,
    size: (u32, u32),
}

impl Post {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Post {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let shader = device.create_shader_module(wgpu::include_wgsl!("post.wgsl"));
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("post"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("post"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("post look"),
            contents: bytemuck::cast_slice(&Look::default().uniform(1.0, glam::Vec3::ONE)),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        Post {
            pipeline,
            layout,
            uniform,
            format,
            targets: None,
        }
    }

    /// The frame, depth, dome and drawn sky to draw into instead of the
    /// window, made (or remade) at its size.
    pub fn targets(
        &mut self,
        device: &wgpu::Device,
        depth_format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) -> [wgpu::TextureView; 4] {
        if self
            .targets
            .as_ref()
            .is_none_or(|t| t.size != (width, height))
        {
            let make = |label, format, usage| {
                device
                    .create_texture(&wgpu::TextureDescriptor {
                        label: Some(label),
                        size: wgpu::Extent3d {
                            width: width.max(1),
                            height: height.max(1),
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format,
                        usage: usage
                            | wgpu::TextureUsages::RENDER_ATTACHMENT
                            | wgpu::TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    })
                    .create_view(&Default::default())
            };
            let color = make("post frame", self.format, wgpu::TextureUsages::empty());
            let depth = make("post depth", depth_format, wgpu::TextureUsages::empty());
            let sky = make("post sky", self.format, wgpu::TextureUsages::empty());
            let sky_drawn = make("post sky drawn", self.format, wgpu::TextureUsages::empty());
            let group = |fades_into: &wgpu::TextureView| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("post"),
                    layout: &self.layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&color),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&depth),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: wgpu::BindingResource::TextureView(fades_into),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: self.uniform.as_entire_binding(),
                        },
                    ],
                })
            };
            let (bind_group, bind_group_drawn) = (group(&sky), group(&sky_drawn));
            self.targets = Some(Targets {
                color,
                depth,
                sky,
                sky_drawn,
                bind_group,
                bind_group_drawn,
                size: (width, height),
            });
        }
        let t = self.targets.as_ref().unwrap();
        [
            t.color.clone(),
            t.depth.clone(),
            t.sky.clone(),
            t.sky_drawn.clone(),
        ]
    }

    /// The look over the frame drawn into `targets`, onto `target`; fog
    /// fading into the sky pass's sky (`sky_drawn`) or the dome alone.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        look: &Look,
        near: f32,
        world_light: glam::Vec3,
        sky_drawn: bool,
    ) {
        let Some(t) = &self.targets else { return };
        queue.write_buffer(
            &self.uniform,
            0,
            bytemuck::cast_slice(&look.uniform(near, world_light)),
        );
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("post"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(
            0,
            if sky_drawn {
                &t.bind_group_drawn
            } else {
                &t.bind_group
            },
            &[],
        );
        pass.draw(0..3, 0..1);
    }
}
