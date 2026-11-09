// The sky, after bevy_atmos's (`sky.wgsl`, MIT or Apache-2.0): drawn per
// pixel behind the level rather than into a cube map.
//
// - Its colours are the level's own sky dome (drawn alone just before,
//   `base`), or a physical sky (Rayleigh and Mie scattering).
// - Clouds: a layer 1.5-2.6 km up over a round earth, raymarched through
//   value-noise fbm and lit by the sun (Beer's law along the sun, a
//   two-lobed phase and some multiple-scattering brightening) and by the
//   sky; they drift with the wind and fade into the horizon far away.
// - The sun (physical sky): a disc with limb darkening and its glow; the
//   moon and stars by night.
//
// Colours are linear in here; the frame is gamma (as the game draws), so
// the base is read through `to_linear` and the result written through
// `to_gamma`.

struct SkyParams {
    // Clip space (x, y at depth 0.5) to a direction from the eye.
    inv: mat4x4<f32>,
    // The view's rectangle in the target: x, y, width, height (pixels).
    rect: vec4<f32>,
    // Toward the sun, and the disc's angular radius (radians).
    sun_dir: vec4<f32>,
    // The sun's colour (brightest channel 1), and the clouds' cover 0..1.
    sun_color: vec4<f32>,
    // The horizon's colour (linear), and 1 for the far clouds' haze.
    horizon: vec4<f32>,
    // How far the clouds have drifted (m, x and z), overcast darkness,
    // seconds.
    wind: vec4<f32>,
    // Toward the moon, and 0 day .. 1 night.
    moon_dir: vec4<f32>,
    // The light the clouds take (the sun, or the moon at night), and 1 for
    // a physical sky.
    key_dir: vec4<f32>,
    // The key light's colour and strength, and 1 for clouds.
    key_color: vec4<f32>,
    // The level's sky overhead (linear), and the march's steps.
    zenith: vec4<f32>,
    // The level's sky on the whole (linear), and its light on the world
    // (night darker) for the level's own sky.
    average: vec4<f32>,
};

@group(0) @binding(0) var<uniform> sky: SkyParams;
@group(0) @binding(1) var base_sky: texture_2d<f32>;
@group(0) @binding(2) var noise: texture_3d<f32>;
@group(0) @binding(3) var noise_sampler: sampler;

const PI: f32 = 3.14159265;
const EARTH: f32 = 6360000.0;
const CLOUD_BASE: f32 = 1500.0;
const CLOUD_TOP: f32 = 2600.0;
const LIGHT_STEPS: u32 = 3u;

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VertexOut {
    var out: VertexOut;
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    out.clip = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    return out;
}

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    return pow(max(c, vec3<f32>(0.0)), vec3<f32>(2.2));
}

fn to_gamma(c: vec3<f32>) -> vec3<f32> {
    return pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2));
}

// --- noise

fn hash(p: vec3<i32>) -> f32 {
    var v = bitcast<vec3<u32>>(p) * 1664525u + 1013904223u;
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    v ^= v >> vec3(16u);
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    return f32(v.x) * (1.0 / 4294967296.0);
}

// Smooth value noise, one lattice cell per unit: a tiling 3D texture of
// it (32 cells across, `atmos.rs`).
fn value_noise(p: vec3<f32>) -> f32 {
    return textureSampleLevel(noise, noise_sampler, p * (1.0 / 32.0), 0.0).r;
}

fn fbm(p: vec3<f32>, octaves: u32) -> f32 {
    var sum = 0.0;
    var amp = 0.5;
    var q = p;
    var norm = 0.0;
    for (var i = 0u; i < octaves; i += 1u) {
        sum += amp * value_noise(q);
        norm += amp;
        amp *= 0.5;
        q = q * 2.03 + vec3(17.1, 3.7, 9.3);
    }
    return sum / norm;
}

// --- clouds

// Distance along `d` (from the ground) to a shell `h` metres up.
fn to_shell(dy: f32, h: f32) -> f32 {
    let k = h * (2.0 * EARTH + h);
    let b = EARTH * dy;
    return k / (b + sqrt(b * b + k));
}

// Cloud density (0..1) at `p` (m, from the eye), `h` its height through
// the layer (0..1); `detail` adds the wispy edges.
fn density(p: vec3<f32>, h: f32, cover: f32, detail: bool) -> f32 {
    let q = p + vec3(sky.wind.x, 0.0, sky.wind.y);
    let weather = fbm(vec3(q.x, 0.0, q.z) * (1.0 / 7000.0), 3u);
    // Billows: the noise folded about its middle (puffy cauliflower).
    let n = saturate((fbm(q * (1.0 / 1600.0), 4u) - 0.5) * 2.8 + 0.5);
    let shape = 1.0 - (n * 2.0 - 1.0) * (n * 2.0 - 1.0);
    let base = saturate((weather * 0.55 + shape * 0.45 - 0.5) * 3.5 + 0.5);
    let lo = 1.0 - cover;
    var d = smoothstep(lo - 0.03, lo + mix(0.1, 0.45, smoothstep(0.55, 0.85, cover)), base);
    // Flat bottoms, rounded tops; overcast, a continuous deck under the
    // masses.
    d = max(d, 0.35 * smoothstep(0.85, 0.95, cover) * (1.0 - h));
    d *= smoothstep(0.0, 0.12, h) * smoothstep(1.0, mix(0.35, 0.8, d), h);
    if (detail && d > 0.0) {
        // Detail erosion: small noise eats into the thin edges (the cores
        // stay solid), billowy at the tops and wispy underneath.
        let wisps = fbm(q * (1.0 / 220.0), 3u);
        let fine = fbm(q * (1.0 / 90.0), 2u);
        let billow = mix(fine, 1.0 - fine, saturate(h * 2.5));
        let erode = wisps * 0.3 + billow * 0.42;
        d = saturate((d - erode * (1.0 - d * 0.7)) / max(1.0 - erode * 0.5, 0.2));
        d = min(smoothstep(0.0, 0.22, d) * d * 1.3, 1.0);
    }
    return d;
}

fn hg(c: f32, g: f32) -> f32 {
    let denom = 1.0 + g * g - 2.0 * g * c;
    return (1.0 - g * g) / (4.0 * PI * denom * sqrt(denom));
}

// --- the physical sky

const ATMOSPHERE: f32 = 60000.0;
const DAY_ZENITH: f32 = 0.55;
const NIGHT_CLOUD: f32 = 0.15;
const MOONLIGHT: f32 = 0.02;
const RAYLEIGH: vec3<f32> = vec3(5.8e-6, 13.5e-6, 33.1e-6);
const MIE: f32 = 21e-6;
const RAYLEIGH_HEIGHT: f32 = 8000.0;
const MIE_HEIGHT: f32 = 1200.0;

// Light scattered toward the eye along `d` (from the ground) from a light
// toward `l` (single scattering, Rayleigh and Mie), for a light of
// radiance 1.
fn scatter(d: vec3<f32>, l: vec3<f32>) -> vec3<f32> {
    let steps = 12;
    let dt = to_shell(max(d.y, 0.0), ATMOSPHERE) / f32(steps);
    var od_r = 0.0;
    var od_m = 0.0;
    var sum_r = vec3(0.0);
    var sum_m = vec3(0.0);
    for (var i = 0; i < steps; i += 1) {
        let p = d * ((f32(i) + 0.5) * dt);
        let height = max(length(vec2(length(p.xz), p.y + EARTH)) - EARTH, 0.0);
        let hr = exp(-height / RAYLEIGH_HEIGHT) * dt;
        let hm = exp(-height / MIE_HEIGHT) * dt;
        od_r += hr;
        od_m += hm;
        let up = normalize(vec3(p.x, p.y + EARTH, p.z));
        let cos_l = dot(up, l);
        if (cos_l < -0.1) {
            continue;
        }
        let ldt = to_shell(max(cos_l, 0.0), ATMOSPHERE - height) / 4.0;
        var lod_r = 0.0;
        var lod_m = 0.0;
        for (var j = 0; j < 4; j += 1) {
            let q = p + l * ((f32(j) + 0.5) * ldt);
            let qh = max(length(vec2(length(q.xz), q.y + EARTH)) - EARTH, 0.0);
            lod_r += exp(-qh / RAYLEIGH_HEIGHT) * ldt;
            lod_m += exp(-qh / MIE_HEIGHT) * ldt;
        }
        let tau = RAYLEIGH * (od_r + lod_r) + MIE * 1.1 * (od_m + lod_m);
        let attenuation = exp(-tau) * smoothstep(-0.1, 0.02, cos_l);
        sum_r += attenuation * hr;
        sum_m += attenuation * hm;
    }
    let mu = dot(d, l);
    let phase_r = 3.0 / (16.0 * PI) * (1.0 + mu * mu);
    let g = 0.76;
    let phase_m = 3.0 / (8.0 * PI) * ((1.0 - g * g) * (1.0 + mu * mu)) / ((2.0 + g * g) * pow(1.0 + g * g - 2.0 * g * mu, 1.5));
    return sum_r * RAYLEIGH * phase_r + sum_m * MIE * phase_m;
}

fn night_sky(d: vec3<f32>) -> vec3<f32> {
    return sky.horizon.rgb * mix(1.0, 0.33, pow(saturate(d.y), 0.6));
}

// Stars: points on a fine grid of directions, twinkling.
fn stars(d: vec3<f32>) -> vec3<f32> {
    let q = d * 300.0;
    let cell = vec3<i32>(floor(q));
    let r = hash(cell);
    if (r < 0.985) {
        return vec3(0.0);
    }
    let centre = vec3<f32>(cell) + 0.5 + (vec3(hash(cell + 7), hash(cell + 13), hash(cell + 29)) - 0.5) * 0.6;
    let fall = exp(-dot(q - centre, q - centre) * 18.0);
    let twinkle = 0.75 + 0.25 * sin(sky.wind.w * (2.0 + 4.0 * hash(cell + 3)) + r * 50.0);
    let tint = mix(vec3(0.75, 0.85, 1.0), vec3(1.0, 0.9, 0.75), hash(cell + 5));
    return tint * fall * twinkle * (r - 0.985) * 120.0;
}

struct SkyOut {
    // The frame, and the sky alone (what fog fades into).
    @location(0) frame: vec4<f32>,
    @location(1) alone: vec4<f32>,
};

@fragment
fn fs_main(in: VertexOut) -> SkyOut {
    // This pixel's direction from the eye.
    let ndc = vec2(
        (in.clip.x - sky.rect.x) / sky.rect.z * 2.0 - 1.0,
        1.0 - (in.clip.y - sky.rect.y) / sky.rect.w * 2.0,
    );
    let far = sky.inv * vec4(ndc, 0.5, 1.0);
    let d = normalize(far.xyz / far.w);
    let sun = normalize(sky.sun_dir.xyz);
    let key = normalize(sky.key_dir.xyz);
    let cs = dot(d, sun);
    let ck = dot(d, key);
    let physical_sky = sky.key_dir.w > 0.5;
    let night = sky.moon_dir.w;
    let cover = sky.sun_color.w;
    let lum = vec3(0.2126, 0.7152, 0.0722);

    // The sky behind the clouds: the level's dome (dimmed by night), or
    // the physical sky.
    var base = to_linear(textureLoad(base_sky, vec2<i32>(in.clip.xy), 0).rgb) * sky.average.w;
    let zenith = sky.zenith.rgb;
    let zenith_lum = select(dot(zenith, lum), DAY_ZENITH, physical_sky);
    if (physical_sky) {
        let reference = scatter(vec3(0.0, 1.0, 0.0), normalize(vec3(0.3, 0.75, 0.2)));
        let scale = DAY_ZENITH / max(dot(reference, lum), 1e-6);
        let moon = normalize(sky.moon_dir.xyz);
        let dir = vec3(d.x, max(d.y, 0.0), d.z);
        base = scale * (scatter(dir, sun) + MOONLIGHT * scatter(dir, moon) * vec3(0.7, 0.8, 1.0));
        base += stars(d) * night * smoothstep(0.0, 0.1, d.y) * zenith_lum;
        base = max(base, night_sky(d) * night);
        // Twilight: the glow the high air still catches, orange to the
        // sun's side low down, a deep blue overhead, rose opposite.
        let tw = smoothstep(-0.2, -0.03, sun.y) * (1.0 - smoothstep(0.02, 0.14, sun.y));
        if (tw > 0.0) {
            let flat_d = normalize(vec2(d.x, d.z) + vec2(1e-5, 0.0));
            let flat_s = normalize(vec2(sun.x, sun.z) + vec2(1e-5, 0.0));
            let toward = dot(flat_d, flat_s) * 0.5 + 0.5;
            let up = max(d.y, 0.0);
            let low = exp(-up / 0.09);
            let depth = smoothstep(-0.2, 0.0, sun.y);
            let band = vec3(1.0, 0.42, 0.14) * low * (0.25 + 0.75 * toward * toward) * depth;
            let rose = vec3(0.55, 0.32, 0.45) * exp(-abs(up - 0.08) / 0.07) * (1.0 - toward) * 0.5 * depth;
            let over = vec3(0.10, 0.16, 0.36) * (1.0 - low) * (0.4 + 0.6 * depth);
            base = max(base, (band + rose + over) * DAY_ZENITH * 0.55 * tw);
        }
        // Under the horizon: the horizon's haze (the level hides most of it).
        if (d.y < 0.0) {
            base = mix(base, sky.horizon.rgb, smoothstep(0.0, -0.08, d.y));
        }
    }
    // The sun's glow in the air round it.
    var glow = sky.sun_color.rgb * (0.04 * pow(max(cs, 0.0), 6.0) + 0.25 * pow(max(cs, 0.0), 120.0)) * (1.0 - night);
    if (!physical_sky) {
        // (The level's sky paints its own sun, or none: only a faint glow.)
        glow *= 0.25;
    }

    var color = vec3(0.0);
    var transmittance = 1.0;
    if (d.y > 0.0 && cover > 0.0 && sky.key_color.w > 0.5) {
        let t0 = to_shell(d.y, CLOUD_BASE);
        let t1 = to_shell(d.y, CLOUD_TOP);
        let steps = u32(sky.zenith.w);
        let dt = (t1 - t0) / f32(steps);
        let jitter = hash(vec3<i32>(vec2<i32>(in.clip.xy), 0));
        let sigma = mix(0.012, 0.03, cover);
        let darkness = sky.wind.z;
        let key_light = sky.key_color.rgb * mix(6.0, 2.5, darkness) * clamp(zenith_lum / 0.35, 0.12, 1.0);
        let average = sky.average.rgb;
        let tint_of = mix(average, zenith, 0.8);
        let hue = mix(vec3(1.0), tint_of / max(dot(tint_of, lum), 1e-3), select(0.85, 0.0, physical_sky));
        let phase = mix(hg(ck, 0.45), hg(ck, -0.2), 0.3);
        var t_sum = 0.0;
        var weight = 0.0;
        for (var i = 0u; i < steps; i += 1u) {
            if (transmittance < 0.01) {
                break;
            }
            let t = t0 + (f32(i) + jitter) * dt;
            let p = d * t;
            let r = length(vec2(length(p.xz), p.y + EARTH)) - EARTH;
            let h = saturate((r - CLOUD_BASE) / (CLOUD_TOP - CLOUD_BASE));
            let dens = density(p, h, cover, true);
            if (dens <= 0.0) {
                continue;
            }
            let s = sigma * dens;
            // Toward the key light through the cloud.
            var depth = 0.0;
            for (var j = 1u; j <= LIGHT_STEPS; j += 1u) {
                let lp = p + key * (f32(j) * 180.0);
                let lr = length(vec2(length(lp.xz), lp.y + EARTH)) - EARTH;
                let lh = saturate((lr - CLOUD_BASE) / (CLOUD_TOP - CLOUD_BASE));
                depth += density(lp, lh, cover, false) * 180.0;
            }
            let od = depth * sigma * (1.0 + darkness);
            let powder = 1.0 - exp(-2.0 * od - 0.2);
            let lit = exp(-od) * mix(1.0, powder, 0.7) + 0.3 * exp(-od * 0.25) * (1.0 - exp(-2.0 * s * 100.0));
            var sky_there = mix(base, zenith, 0.5);
            if (physical_sky) {
                sky_there = mix(base, zenith * (1.0 - night), 0.3);
                sky_there = mix(sky_there, night_sky(d), night);
            }
            let ambient = sky_there * mix(0.25, 1.0, h) * (1.0 - 0.6 * darkness)
                + vec3(0.42, 0.46, 0.55) * zenith_lum * NIGHT_CLOUD * night * mix(0.6, 1.0, h);
            let ph = mix(1.0 / (4.0 * PI), phase, saturate(dens * 1.5));
            let edge_dim = mix(mix(0.3, 0.12, smoothstep(0.7, 0.98, ck)), 1.0, smoothstep(0.05, 0.6, dens));
            let light = (key_light * lit * ph * edge_dim + ambient) * hue;
            let step_t = exp(-s * dt);
            color += transmittance * light * (1.0 - step_t);
            t_sum += t * transmittance * (1.0 - step_t);
            weight += transmittance * (1.0 - step_t);
            transmittance *= step_t;
        }
        // The level's sky's own mood and brightness: clouds as bright, on
        // the whole, as it is.
        if (!physical_sky) {
            let typical = (key_light * 0.3 / (4.0 * PI) + mix(average, zenith, 0.5) * 0.6 * (1.0 - 0.6 * darkness)) * hue;
            color *= clamp(dot(average, lum) / max(dot(typical, lum), 1e-4), 0.15, 2.0);
            let ceiling = max(average, zenith) * 1.25 * (1.0 - transmittance);
            color = min(color, ceiling + glow);
        }
        // Far clouds fade into the horizon's haze.
        if (weight > 0.0) {
            let far_off = 1.0 - exp(-(t_sum / weight) / 30000.0);
            color = mix(color, (sky.horizon.rgb + glow) * (1.0 - transmittance), far_off * sky.horizon.w);
        }
        // Clouds thin out toward the horizon.
        let fade = smoothstep(0.0, 0.15, d.y);
        color *= fade;
        transmittance = mix(1.0, transmittance, fade);
    }

    var discs = vec3(0.0);
    if (physical_sky) {
        // The sun's disc, darker at its rim, behind the clouds.
        let size = sky.sun_dir.w;
        let angle = acos(clamp(cs, -1.0, 1.0));
        let disc = 1.0 - smoothstep(size * 0.85, size, angle);
        let limb = sqrt(max(1.0 - (angle / size) * (angle / size), 0.0));
        discs = sky.sun_color.rgb * 60.0 * disc * mix(0.6, 1.0, limb) * step(-0.02, d.y) * (1.0 - night) * (1.0 - 0.95 * smoothstep(0.7, 0.9, cover));
        // The moon: a pale disc with darker seas.
        let moon = normalize(sky.moon_dir.xyz);
        let ma = acos(clamp(dot(d, moon), -1.0, 1.0));
        let moon_disc = 1.0 - smoothstep(size * 0.8, size, ma);
        let seas = 0.75 + 0.25 * value_noise(d * 900.0);
        discs += vec3(0.75, 0.8, 0.9) * 24.0 * moon_disc * seas * night * zenith_lum;
    }
    let behind = mix(1.0, 0.65, smoothstep(0.6, 0.9, cover));
    let out = (base * behind + glow + discs) * transmittance + color;
    let shown = vec4(to_gamma(out), 1.0);
    var o: SkyOut;
    o.frame = shown;
    o.alone = shown;
    return o;
}
