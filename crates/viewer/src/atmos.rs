//! The sky and the time of day, after bevy_atmos (`sky.rs`, `time.rs`;
//! MIT or Apache-2.0): a physical sky or the level's own dome, volumetric
//! clouds over either, and a sun (and moon) that the hour moves. Drawn
//! per pixel behind the level by `atmos_sky.wgsl`, on only when the look
//! asks for one of them (see [`wanted`]).

use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

use crate::level::Level;
use crate::post::Look;

/// The sun path's latitude (bevy_atmos's `time.latitude` default).
const LATITUDE: f32 = 35.0;
/// The noise texture's size and lattice cells across (it tiles).
const NOISE_SIZE: u32 = 128;
const NOISE_CELLS: u32 = 32;
/// The sun's angular radius (radians).
const SUN_SIZE: f32 = 0.0093;

/// Whether the look needs the sky pass: a physical sky, clouds, or an
/// hour of its own.
pub fn wanted(look: &Look) -> bool {
    look.physical_sky || look.clouds > 0.0 || look.hour >= 0.0
}

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Toward the sun at `hours` on an equinox day at `latitude` (radians).
pub fn path_dir(hours: f32, latitude: f32) -> Vec3 {
    let h = (hours - 12.0) / 24.0 * std::f32::consts::TAU;
    let (sl, cl) = latitude.sin_cos();
    Vec3::new(-h.sin(), cl * h.cos(), sl * h.cos()).normalize()
}

/// The time of day and what it gives.
#[derive(Clone, Copy, Debug)]
pub struct TimeOfDay {
    /// An hour of its own (else the level's: day, the sun where it was).
    pub clocked: bool,
    pub sun_dir: Vec3,
    pub moon_dir: Vec3,
    /// The sun's colour, brightest channel 1.
    pub sun_color: Vec3,
    /// 0 day .. 1 night.
    pub night: f32,
    /// The light the clouds take (the sun, or by night the moon), and its
    /// colour and strength.
    pub key_dir: Vec3,
    pub key_color: Vec3,
    /// The light on the level (its lighting's baked: a tint over it), 1 at
    /// the level's own hour.
    pub world_light: Vec3,
}

impl TimeOfDay {
    /// At `hours` (below 0: the level's own, a fixed morning sun).
    pub fn at(hours: f32) -> TimeOfDay {
        let latitude = LATITUDE.to_radians();
        if hours < 0.0 {
            let sun = path_dir(10.0, latitude);
            return TimeOfDay {
                clocked: false,
                sun_dir: sun,
                moon_dir: -sun,
                sun_color: Vec3::new(1.0, 0.96, 0.9),
                night: 0.0,
                key_dir: sun,
                key_color: Vec3::new(1.0, 0.96, 0.9),
                world_light: Vec3::ONE,
            };
        }
        let hours = hours.rem_euclid(24.0);
        let sun = path_dir(hours, latitude);
        let moon = {
            let m = glam::Quat::from_rotation_y(0.3) * path_dir(hours + 12.0, latitude);
            Vec3::new(m.x, m.y + 0.1, m.z).normalize()
        };
        // (Twilight lasts: full night past about 11 degrees down.)
        let night = 1.0 - smooth((sun.y + 0.2) / 0.22);
        // Redder and dimmer low down (air mass).
        let m = 1.0 / sun.y.max(0.035);
        let red = Vec3::new((-0.03 * m).exp(), (-0.07 * m).exp(), (-0.16 * m).exp());
        let color = red / red.max_element().max(1e-3);
        let at_level = path_dir(10.0, latitude).y.max(0.05).sqrt();
        let share = smooth((sun.y + 0.02) / 0.19) * (sun.y.max(0.0).sqrt() / at_level).min(1.3);
        let (key_dir, key_color) = if night > 0.5 {
            (
                moon,
                Vec3::new(0.62, 0.72, 1.0) * 0.1 * night * smooth(moon.y / 0.1),
            )
        } else {
            (sun, color * share)
        };
        // The level's baked light under the hour: dimmer and bluer by
        // night, warmer low down.
        let day = (0.25 + 0.75 * (1.0 - night)) * (0.4 + 0.6 * smooth((sun.y + 0.02) / 0.19));
        let tint = Vec3::ONE.lerp(color, 0.45 * (1.0 - night));
        let tint = tint.lerp(Vec3::new(0.55, 0.65, 1.0), night);
        TimeOfDay {
            clocked: true,
            sun_dir: sun,
            moon_dir: moon,
            sun_color: color,
            night,
            key_dir,
            key_color,
            world_light: tint * day,
        }
    }

    /// The horizon's colour (linear) for the physical sky: by day a pale
    /// blue haze, warm at sunrise and sunset, dark blue by night.
    fn horizon(&self) -> Vec3 {
        let day = Vec3::new(0.55, 0.62, 0.72);
        let low = 1.0 - smooth(self.sun_dir.y / 0.25);
        let warm = day.lerp(Vec3::new(0.85, 0.55, 0.38), low * (1.0 - self.night));
        warm.lerp(Vec3::new(0.02, 0.03, 0.06), self.night)
    }
}

/// The level's sky's colours (linear): overhead, on the whole and at the
/// horizon, from its dome's textures and vertex colours (as bevy_atmos
/// reads them off the host's skybox).
#[derive(Clone, Copy, Debug)]
pub struct SkyColours {
    pub zenith: Vec3,
    pub average: Vec3,
    pub horizon: Vec3,
}

impl SkyColours {
    /// A plain day's, when the level has no dome.
    pub fn plain() -> SkyColours {
        SkyColours {
            zenith: Vec3::new(0.18, 0.3, 0.55),
            average: Vec3::new(0.3, 0.4, 0.55),
            horizon: Vec3::new(0.55, 0.62, 0.72),
        }
    }

    pub fn of(sky: &Level) -> SkyColours {
        let linear = |c: f32| (c.max(0.0)).powf(2.2);
        let mut sums = [(Vec3::ZERO, 0.0f32); 3];
        for batch in &sky.batches {
            let Some(pass) = batch.passes.first() else {
                continue;
            };
            let texture = sky.textures.get(pass.texture);
            let first = batch.first_index as usize;
            let last = (first + batch.index_count as usize).min(sky.indices.len());
            for &i in &sky.indices[first..last] {
                let Some(v) = sky.vertices.get(i as usize) else {
                    continue;
                };
                let dir = Vec3::from(v.position).normalize_or_zero();
                let uv = v.uvs[pass.uv_set as usize % v.uvs.len()];
                let texel = texture.map_or(Vec3::ONE, |t| sample(t, uv));
                let shade = Vec3::new(
                    f32::from(v.color[0]),
                    f32::from(v.color[1]),
                    f32::from(v.color[2]),
                ) / 128.0;
                let shown = texel * shade;
                let c = Vec3::new(linear(shown.x), linear(shown.y), linear(shown.z));
                let slots = [dir.y > 0.75, dir.y > 0.05, (0.0..0.25).contains(&dir.y)];
                for (sum, on) in sums.iter_mut().zip(slots) {
                    if on {
                        sum.0 += c;
                        sum.1 += 1.0;
                    }
                }
            }
        }
        let plain = SkyColours::plain();
        let mean = |(sum, n): (Vec3, f32), fallback: Vec3| {
            if n > 0.0 { sum / n } else { fallback }
        };
        let average = mean(sums[1], plain.average);
        SkyColours {
            zenith: mean(sums[0], average),
            average,
            horizon: mean(sums[2], average),
        }
    }
}

/// A texture's colour at `uv` (gamma, 0..1), from a small mip.
fn sample(texture: &crate::level::TextureData, uv: [f32; 2]) -> Vec3 {
    let level = texture.levels.len().saturating_sub(1).min(2);
    let shift = level as u32;
    let (w, h) = (
        (texture.width >> shift).max(1),
        (texture.height >> shift).max(1),
    );
    let Some(pixels) = texture.levels.get(level) else {
        return Vec3::ONE;
    };
    let x = ((uv[0].rem_euclid(1.0) * w as f32) as u32).min(w - 1);
    let y = (((1.0 - uv[1]).rem_euclid(1.0) * h as f32) as u32).min(h - 1);
    let at = ((y * w + x) * 4) as usize;
    match pixels.get(at..at + 3) {
        Some(p) => Vec3::new(f32::from(p[0]), f32::from(p[1]), f32::from(p[2])) / 255.0,
        None => Vec3::ONE,
    }
}

/// The sky pass: its pipeline, the clouds' noise, and a uniform for each
/// view (two players' halves).
pub struct Sky {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniforms: [wgpu::Buffer; 2],
    noise: wgpu::TextureView,
    sampler: wgpu::Sampler,
    pub colours: SkyColours,
    /// The hour now, while the day runs (else the look's).
    hours: Option<f32>,
    last_time: Option<f32>,
}

/// One view's sky uniform: see `SkyParams` in `atmos_sky.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    inv: [f32; 16],
    rect: [f32; 4],
    sun_dir: [f32; 4],
    sun_color: [f32; 4],
    horizon: [f32; 4],
    wind: [f32; 4],
    moon_dir: [f32; 4],
    key_dir: [f32; 4],
    key_color: [f32; 4],
    zenith: [f32; 4],
    average: [f32; 4],
}

impl Sky {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        sky: Option<&Level>,
    ) -> Sky {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("atmos sky"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
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
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D3,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let shader = device.create_shader_module(wgpu::include_wgsl!("atmos_sky.wgsl"));
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("atmos sky"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let target = Some(wgpu::ColorTargetState {
            format,
            blend: None,
            write_mask: wgpu::ColorWrites::ALL,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("atmos sky"),
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
                targets: &[target.clone(), target],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        let uniform = || {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("atmos sky"),
                size: std::mem::size_of::<Params>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let noise = device
            .create_texture_with_data(
                queue,
                &wgpu::TextureDescriptor {
                    label: Some("cloud noise"),
                    size: wgpu::Extent3d {
                        width: NOISE_SIZE,
                        height: NOISE_SIZE,
                        depth_or_array_layers: NOISE_SIZE,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D3,
                    format: wgpu::TextureFormat::R8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::LayerMajor,
                &noise_data(),
            )
            .create_view(&Default::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("cloud noise"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Sky {
            pipeline,
            layout,
            uniforms: [uniform(), uniform()],
            noise,
            sampler,
            colours: sky.map_or_else(SkyColours::plain, SkyColours::of),
            hours: None,
            last_time: None,
        }
    }

    /// The time of day now: the look's hour, run on by its day cycle
    /// (game hours a real second) from `time` (seconds).
    pub fn time_of_day(&mut self, look: &Look, time: f32) -> TimeOfDay {
        let dt = self.last_time.map_or(0.0, |t| (time - t).clamp(0.0, 0.5));
        self.last_time = Some(time);
        if look.day_speed > 0.0 {
            let from = if look.hour >= 0.0 { look.hour } else { 10.0 };
            let now = self.hours.get_or_insert(from);
            *now = (*now + dt * look.day_speed).rem_euclid(24.0);
            TimeOfDay::at(*now)
        } else {
            self.hours = None;
            TimeOfDay::at(look.hour)
        }
    }

    /// Draws one view's sky into `frame` and `alone` (its rectangle of
    /// them; the first view clears them), from the level's dome in `base`.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        (frame, alone, base): (&wgpu::TextureView, &wgpu::TextureView, &wgpu::TextureView),
        n: usize,
        rect: (f32, f32, f32, f32),
        rotation_proj: Mat4,
        look: &Look,
        tod: &TimeOfDay,
        time: f32,
        clear: wgpu::Color,
    ) {
        let inv = rotation_proj.inverse();
        let c = &self.colours;
        let dim = if tod.clocked {
            tod.world_light.dot(Vec3::new(0.2126, 0.7152, 0.0722))
        } else {
            1.0
        };
        let horizon = if look.physical_sky {
            tod.horizon()
        } else {
            c.horizon * dim
        };
        let wind_dir = Vec3::new(30f32.to_radians().sin(), 0.0, -30f32.to_radians().cos());
        // Clouds drift at 10 m/s (the noise moves against the wind).
        let drift = -wind_dir * time * 10.0;
        let cover = look.clouds.clamp(0.0, 1.0);
        let params = Params {
            inv: inv.to_cols_array(),
            rect: [rect.0, rect.1, rect.2.max(1.0), rect.3.max(1.0)],
            sun_dir: tod.sun_dir.extend(SUN_SIZE).to_array(),
            sun_color: tod.sun_color.extend(cover).to_array(),
            horizon: horizon.extend(1.0).to_array(),
            wind: [drift.x, drift.z, smooth((cover - 0.6) / 0.35) * 0.6, time],
            moon_dir: tod.moon_dir.extend(tod.night).to_array(),
            key_dir: tod
                .key_dir
                .extend(if look.physical_sky { 1.0 } else { 0.0 })
                .to_array(),
            key_color: tod
                .key_color
                .extend(if cover > 0.0 { 1.0 } else { 0.0 })
                .to_array(),
            zenith: (c.zenith * dim).extend(40.0).to_array(),
            average: (c.average * dim).extend(dim).to_array(),
        };
        let uniform = &self.uniforms[n.min(1)];
        queue.write_buffer(uniform, 0, bytemuck::bytes_of(&params));
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("atmos sky"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(base),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&self.noise),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        let ops = wgpu::Operations {
            load: if n > 0 {
                wgpu::LoadOp::Load
            } else {
                wgpu::LoadOp::Clear(clear)
            },
            store: wgpu::StoreOp::Store,
        };
        let attachment = |view| {
            Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops,
            })
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("atmos sky"),
            color_attachments: &[attachment(frame), attachment(alone)],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_viewport(rect.0, rect.1, rect.2, rect.3, 0.0, 1.0);
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

/// Smooth, tiling 3D value noise for the clouds (bevy_atmos's).
fn noise_data() -> Vec<u8> {
    let hash = |x: u32, y: u32, z: u32| {
        let (x, y, z) = (x % NOISE_CELLS, y % NOISE_CELLS, z % NOISE_CELLS);
        let mut h =
            x.wrapping_mul(0x8da6_b343) ^ y.wrapping_mul(0xd816_3841) ^ z.wrapping_mul(0xcb1a_b31f);
        h ^= h >> 13;
        h = h.wrapping_mul(0x5bd1_e995);
        h ^= h >> 15;
        (h & 0xffff) as f32 / 65535.0
    };
    let per = (NOISE_SIZE / NOISE_CELLS) as f32;
    let smooth = |t: f32| t * t * (3.0 - 2.0 * t);
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let mut data = Vec::with_capacity((NOISE_SIZE * NOISE_SIZE * NOISE_SIZE) as usize);
    for z in 0..NOISE_SIZE {
        for y in 0..NOISE_SIZE {
            for x in 0..NOISE_SIZE {
                let p = Vec3::new(x as f32, y as f32, z as f32) / per;
                let (i, f) = (p.floor(), p - p.floor());
                let i = (i.x as u32, i.y as u32, i.z as u32);
                let u = Vec3::new(smooth(f.x), smooth(f.y), smooth(f.z));
                let corner = |dx, dy, dz| hash(i.0 + dx, i.1 + dy, i.2 + dz);
                let a = lerp(corner(0, 0, 0), corner(1, 0, 0), u.x);
                let b = lerp(corner(0, 1, 0), corner(1, 1, 0), u.x);
                let c = lerp(corner(0, 0, 1), corner(1, 0, 1), u.x);
                let d = lerp(corner(0, 1, 1), corner(1, 1, 1), u.x);
                data.push((lerp(lerp(a, b, u.y), lerp(c, d, u.y), u.z) * 255.0).round() as u8);
            }
        }
    }
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noon_is_day_and_midnight_night() {
        let noon = TimeOfDay::at(12.0);
        assert!(noon.sun_dir.y > 0.7 && noon.night < 0.01);
        let midnight = TimeOfDay::at(0.0);
        assert!(midnight.sun_dir.y < -0.5 && midnight.night > 0.99);
        assert!(midnight.world_light.length() < noon.world_light.length());
        // The level's own hour leaves its light as it is.
        assert_eq!(TimeOfDay::at(-1.0).world_light, Vec3::ONE);
    }

    #[test]
    fn noise_tiles() {
        let data = noise_data();
        assert_eq!(data.len(), (NOISE_SIZE * NOISE_SIZE * NOISE_SIZE) as usize);
        assert!(data.iter().any(|&v| v > 200) && data.iter().any(|&v| v < 55));
    }
}
