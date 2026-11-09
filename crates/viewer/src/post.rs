//! The look over the finished frame, after bevy_atmos's air, wet
//! surfaces and colour grade (`air.rs`, `wet.wgsl`, `ssr.wgsl`,
//! `grade.rs`): wet ground with puddles mirroring the level (screen-space
//! reflections), settled snow, distance fog and the grade, drawn from the
//! frame and its depth by one pass at the end. Left alone (`Look`'s
//! default), frames are drawn straight to the window as the game makes
//! them, and this costs nothing.

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
    /// Rain and snow, 0..1 (`weather.rain`, `weather.snow`).
    pub rain: f32,
    pub snow: f32,
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
            rain: 0.0,
            snow: 0.0,
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
            || self.rain > 0.0
            || self.snow > 0.0
    }

    fn params(&self, near: f32, frame: &Frame) -> Params {
        let [r, g, b] = self.fog_color;
        let view = |n: usize| frame.views[n.min(frame.views.len() - 1)];
        Params {
            fog: [r, g, b, self.fog_distance.max(0.0) * UNITS_PER_METRE],
            fog_shape: [
                self.fog_start.max(0.0) * UNITS_PER_METRE,
                near,
                UNITS_PER_METRE,
                frame.time,
            ],
            grade: [
                self.exposure,
                self.warmth.clamp(-1.0, 1.0),
                self.tint.clamp(-1.0, 1.0),
                self.contrast.clamp(0.2, 3.0),
            ],
            grade2: frame
                .world_light
                .extend(self.saturation.clamp(0.0, 3.0))
                .to_array(),
            weather: [
                frame.wetness,
                frame.snow_cover,
                self.rain,
                frame.views.len() as f32,
            ],
            sky: frame.sky.extend(0.0).to_array(),
            inv: [view(0).inv.to_cols_array(), view(1).inv.to_cols_array()],
            view_proj: [
                view(0).view_proj.to_cols_array(),
                view(1).view_proj.to_cols_array(),
            ],
            rect: [view(0).rect, view(1).rect],
            eye: [
                view(0).eye.extend(0.0).to_array(),
                view(1).eye.extend(0.0).to_array(),
            ],
            shelter: frame.shelter.map_or([0.0; 4], |a| [a.x, a.z, a.size, 0.0]),
            shelter_y: frame
                .shelter
                .map_or([0.0; 4], |a| [a.top, a.bottom, 0.0, 0.0]),
        }
    }
}

/// One view of the frame (two players: two): its camera's matrices, its
/// rectangle (pixels: x, y, width, height) and eye (world units).
#[derive(Clone, Copy, Debug)]
pub struct ViewInfo {
    pub view_proj: glam::Mat4,
    pub inv: glam::Mat4,
    pub rect: [f32; 4],
    pub eye: glam::Vec3,
}

/// What the post pass needs of the frame besides the look.
#[derive(Clone, Debug)]
pub struct Frame {
    /// The time of day's light over the level's baked own.
    pub world_light: glam::Vec3,
    /// How wet the ground is, and how much snow has settled, 0..1.
    pub wetness: f32,
    pub snow_cover: f32,
    /// The sky's colour (linear), what puddles show past the screen.
    pub sky: glam::Vec3,
    pub views: Vec<ViewInfo>,
    pub time: f32,
    /// What the shelter map covers (world units), once made: rain and snow
    /// only wet what's open to the sky.
    pub shelter: Option<crate::weather::ShelterArea>,
}

/// The post pass's uniform: see `Look` in `post.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    fog: [f32; 4],
    fog_shape: [f32; 4],
    grade: [f32; 4],
    grade2: [f32; 4],
    weather: [f32; 4],
    sky: [f32; 4],
    inv: [[f32; 16]; 2],
    view_proj: [[f32; 16]; 2],
    rect: [[f32; 4]; 2],
    eye: [[f32; 4]; 2],
    shelter: [f32; 4],
    shelter_y: [f32; 4],
}

/// The pass, and the frame and depth it reads (the window's size).
pub struct Post {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    format: wgpu::TextureFormat,
    targets: Option<Targets>,
    /// The shelter map (see [`crate::weather`]), or a stand-in till made.
    shelter: wgpu::TextureView,
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
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
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
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("post look"),
            size: std::mem::size_of::<Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shelter = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("post shelter stand-in"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default());
        Post {
            pipeline,
            layout,
            uniform,
            format,
            targets: None,
            shelter,
        }
    }

    /// The shelter map, once the weather's made it.
    pub fn set_shelter(&mut self, device: &wgpu::Device, shelter: &wgpu::TextureView) {
        self.shelter = shelter.clone();
        if let Some(t) = self.targets.take() {
            let (bind_group, bind_group_drawn) = (
                self.group(device, &t.color, &t.depth, &t.sky),
                self.group(device, &t.color, &t.depth, &t.sky_drawn),
            );
            self.targets = Some(Targets {
                bind_group,
                bind_group_drawn,
                ..t
            });
        }
    }

    fn group(
        &self,
        device: &wgpu::Device,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        fades_into: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("post"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(color),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(depth),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(fades_into),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&self.shelter),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.uniform.as_entire_binding(),
                },
            ],
        })
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
            let (bind_group, bind_group_drawn) = (
                self.group(device, &color, &depth, &sky),
                self.group(device, &color, &depth, &sky_drawn),
            );
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
        frame: &Frame,
        sky_drawn: bool,
    ) {
        let Some(t) = &self.targets else { return };
        if frame.views.is_empty() {
            return;
        }
        queue.write_buffer(
            &self.uniform,
            0,
            bytemuck::bytes_of(&look.params(near, frame)),
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
