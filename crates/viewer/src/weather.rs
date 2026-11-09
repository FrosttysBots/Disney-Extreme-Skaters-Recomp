//! Rain and snow, after bevy_atmos's weather (`weather.rs`, `rain.wgsl`;
//! MIT or Apache-2.0): streaks, splashes and flakes round the camera,
//! placed by `weather.wgsl` from the clock alone, kept out from under roofs
//! by the shelter map: the level seen from above, drawn once (the highest
//! thing over each spot). Wet ground, puddles and settled snow are the
//! post pass's (`post.wgsl`).

use glam::{Mat4, Vec3};

use crate::atmos::TimeOfDay;
use crate::post::{Look, UNITS_PER_METRE};

/// The shelter map's size (texels a side).
pub const SHELTER_SIZE: u32 = 1024;

/// The kinds drawn: their number in the shader, and quads.
const KINDS: [(f32, u32); 5] = [
    (0.0, 6000), // near streaks
    (1.0, 4000), // far streaks
    (2.0, 1200), // splashes
    (5.0, 5000), // near snow
    (6.0, 4000), // far snow
];

/// What the shelter map covers (world units): its corner (x, z), size,
/// and the heights at its top and bottom.
#[derive(Clone, Copy, Debug)]
pub struct ShelterArea {
    pub x: f32,
    pub z: f32,
    pub size: f32,
    pub top: f32,
    pub bottom: f32,
}

impl ShelterArea {
    /// Round the level's middle (`focus`, skipping huge ground and ocean
    /// planes), from the highest of its points to the lowest.
    pub fn round(focus: (Vec3, f32), positions: impl Iterator<Item = Vec3>) -> ShelterArea {
        let (centre, radius) = focus;
        let half = (radius * 1.4).max(500.0);
        let (mut top, mut bottom) = (f32::MIN, f32::MAX);
        for p in positions {
            if (p.x - centre.x).abs() <= half && (p.z - centre.z).abs() <= half {
                top = top.max(p.y);
                bottom = bottom.min(p.y);
            }
        }
        if top < bottom {
            (top, bottom) = (centre.y + 1000.0, centre.y - 1000.0);
        }
        ShelterArea {
            x: centre.x - half,
            z: centre.z - half,
            size: half * 2.0,
            top: top + 50.0,
            bottom: bottom - 50.0,
        }
    }

    /// World to the map's clip space: x across, z down the rows, depth 0
    /// at the top, 1 at the bottom.
    pub fn projection(&self) -> Mat4 {
        let s = self.size;
        let h = self.top - self.bottom;
        Mat4::from_cols_array(&[
            2.0 / s,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            -1.0 / h,
            0.0,
            0.0,
            -2.0 / s,
            0.0,
            0.0,
            -1.0 - 2.0 * self.x / s,
            1.0 + 2.0 * self.z / s,
            self.top / h,
            1.0,
        ])
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    centre: [f32; 4],
    extent: [f32; 4],
    velocity: [f32; 4],
    light: [f32; 4],
    key_dir: [f32; 4],
    key_light: [f32; 4],
    kind: [f32; 4],
    shelter: [f32; 4],
    shelter_y: [f32; 4],
}

/// The weather's pipeline, and a uniform and bind group for each kind and
/// view.
pub struct Weather {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniforms: Vec<wgpu::Buffer>,
    groups: Vec<wgpu::BindGroup>,
}

impl Weather {
    pub fn new(
        device: &wgpu::Device,
        globals_layout: &wgpu::BindGroupLayout,
        color_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
    ) -> Weather {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("weather"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let shader = device.create_shader_module(wgpu::include_wgsl!("weather.wgsl"));
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("weather"),
            bind_group_layouts: &[globals_layout, &layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("weather"),
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
                    format: color_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            // Behind the level where it's in front (reversed Z), never
            // hiding anything itself.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: depth_format,
                depth_write_enabled: false,
                depth_compare: wgpu::CompareFunction::GreaterEqual,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        let uniforms = (0..KINDS.len() * 2)
            .map(|_| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("weather"),
                    size: std::mem::size_of::<Params>() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            })
            .collect();
        Weather {
            pipeline,
            layout,
            uniforms,
            groups: Vec::new(),
        }
    }

    /// The bind groups, with the shelter map (made once it is).
    pub fn bind(&mut self, device: &wgpu::Device, shelter: &wgpu::TextureView) {
        self.groups = self
            .uniforms
            .iter()
            .map(|uniform| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("weather"),
                    layout: &self.layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: uniform.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(shelter),
                        },
                    ],
                })
            })
            .collect();
    }

    pub fn is_bound(&self) -> bool {
        !self.groups.is_empty()
    }

    /// Rain and snow for view `n`, round `eye` (world units), into `pass`
    /// (its globals already set).
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &self,
        queue: &wgpu::Queue,
        pass: &mut wgpu::RenderPass,
        n: usize,
        eye: Vec3,
        look: &Look,
        tod: &TimeOfDay,
        area: &ShelterArea,
        time: f32,
    ) {
        if !self.is_bound() {
            return;
        }
        let (rain, snow) = (look.rain.clamp(0.0, 1.0), look.snow.clamp(0.0, 1.0));
        let m = UNITS_PER_METRE;
        let wind_dir = Vec3::new(30f32.to_radians().sin(), 0.0, -30f32.to_radians().cos());
        let lum = tod.world_light.dot(Vec3::new(0.2126, 0.7152, 0.0722));
        let air = Vec3::splat(0.85) * tod.world_light / lum.max(1e-3) * lum.min(1.0);
        pass.set_pipeline(&self.pipeline);
        for (k, &(kind, quads)) in KINDS.iter().enumerate() {
            let snowy = kind >= 5.0;
            let amount = if snowy { snow } else { rain };
            if amount <= 0.0 {
                continue;
            }
            let (extent, velocity) = match kind as u32 {
                1 => (
                    Vec3::new(40.0, 20.0, 40.0),
                    Vec3::new(0.0, -9.0, 0.0) + wind_dir * 2.0,
                ),
                5 => (
                    Vec3::new(12.0, 8.0, 12.0),
                    Vec3::new(0.0, -1.1, 0.0) + wind_dir * 0.6,
                ),
                6 => (
                    Vec3::new(35.0, 18.0, 35.0),
                    Vec3::new(0.0, -1.1, 0.0) + wind_dir * 0.6,
                ),
                _ => (
                    Vec3::new(14.0, 9.0, 14.0),
                    Vec3::new(0.0, -9.0, 0.0) + wind_dir * 2.0,
                ),
            };
            let params = Params {
                centre: (eye / m).extend(time).to_array(),
                extent: extent.extend(amount.sqrt()).to_array(),
                velocity: velocity.extend(0.012).to_array(),
                light: air.extend(1.0).to_array(),
                key_dir: tod.key_dir.extend(0.0).to_array(),
                key_light: tod.key_color.extend(0.0).to_array(),
                kind: [kind, 0.0, 0.0, 0.0],
                shelter: [area.x / m, area.z / m, area.size / m, 0.0],
                shelter_y: [area.top / m, area.bottom / m, 0.0, 0.0],
            };
            let slot = k + KINDS.len() * n.min(1);
            queue.write_buffer(&self.uniforms[slot], 0, bytemuck::bytes_of(&params));
            pass.set_bind_group(1, &self.groups[slot], &[]);
            pass.draw(0..quads * 6, 0..1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shelter_projection_maps_the_area() {
        let area = ShelterArea {
            x: -100.0,
            z: 50.0,
            size: 400.0,
            top: 300.0,
            bottom: -100.0,
        };
        let p = area.projection();
        // The corner at the top: clip (-1, 1), depth 0.
        let a = p.project_point3(Vec3::new(-100.0, 300.0, 50.0));
        assert!((a - Vec3::new(-1.0, 1.0, 0.0)).length() < 1e-5, "{a}");
        // The far corner at the bottom: clip (1, -1), depth 1.
        let b = p.project_point3(Vec3::new(300.0, -100.0, 450.0));
        assert!((b - Vec3::new(1.0, -1.0, 1.0)).length() < 1e-5, "{b}");
    }
}
