// The look over the finished frame (after bevy_atmos's air, wet surfaces,
// screen-space reflections and grade): the time of day's light, wet
// ground and puddles mirroring the level, settled snow, distance fog from
// the depth buffer, then the colour grade.

struct Look {
    // rgb: the fog's colour (as drawn, gamma), a: metres to half-hidden in
    // world units (0: no fog)
    fog: vec4<f32>,
    // x: where fog starts (world units), y: the near plane (reversed,
    // infinite Z: distance = near / depth), z: world units a metre, w:
    // seconds
    fog_shape: vec4<f32>,
    // x: exposure (stops), y: warmth, z: tint, w: contrast
    grade: vec4<f32>,
    // xyz: the time of day's light over the level's own (baked) light,
    // w: saturation
    grade2: vec4<f32>,
    // x: wetness, y: settled snow, z: rain (for its rings), w: views (1 or
    // 2)
    weather: vec4<f32>,
    // rgb: the sky's colour (linear): what puddles show past the screen
    sky: vec4<f32>,
    // Each view's clip-to-world and world-to-clip, rectangle (pixels) and
    // eye (world units).
    inv: array<mat4x4<f32>, 2>,
    view_proj: array<mat4x4<f32>, 2>,
    rect: array<vec4<f32>, 2>,
    eye: array<vec4<f32>, 2>,
    // The shelter map: x, z of its corner and its size (world units; size
    // 0: not made), and its top and bottom heights.
    shelter: vec4<f32>,
    shelter_y: vec4<f32>,
    // x: the water's waves (0: the disc's own water)
    water: vec4<f32>,
    // xyz: toward the key light (the sun, or the moon); its colour.
    sun: vec4<f32>,
    sun_color: vec4<f32>,
};

@group(0) @binding(0) var scene: texture_2d<f32>;
@group(0) @binding(1) var depth: texture_depth_2d;
@group(0) @binding(2) var<uniform> look: Look;
// The sky alone, drawn behind everything: what's far fades into it.
@group(0) @binding(3) var sky: texture_2d<f32>;
// The level from straight above (depth 0 at the top): what's under a roof.
@group(0) @binding(4) var shelter: texture_depth_2d;
// The water drawn alone: its own colour (as the disc paints it) and depth.
@group(0) @binding(5) var water_base: texture_2d<f32>;
@group(0) @binding(6) var water_depth: texture_depth_2d;

// How open to the sky `p` is, 0..1: whether nothing's above it on the
// shelter map (looked up a little out along its normal `n`, so walls count
// as the ground beside them).
fn open_sky(p: vec3<f32>, n: vec3<f32>) -> f32 {
    if look.shelter.z <= 0.0 {
        return 1.0;
    }
    let size = vec2<f32>(textureDimensions(shelter));
    let q = p + vec3<f32>(n.x, 0.0, n.z) * look.fog_shape.z * 0.3;
    let uv = (q.xz - look.shelter.xy) / look.shelter.z;
    if any(uv < vec2<f32>(0.0)) || any(uv >= vec2<f32>(1.0)) {
        return 1.0;
    }
    let d = textureLoad(shelter, vec2<i32>(uv * size), 0);
    if d >= 1.0 {
        return 1.0;
    }
    let top = mix(look.shelter_y.x, look.shelter_y.y, d);
    return smoothstep(-24.0, -8.0, p.y - top);
}

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
};

// One triangle over the whole target.
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

fn luminance(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

fn hash2(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(127.1, 311.7))) * 43758.5453);
}

// Smooth value noise, 0..1.
fn noise2(p: vec2<f32>) -> f32 {
    let c = floor(p);
    let f = p - c;
    let u = f * f * (3.0 - 2.0 * f);
    return mix(mix(hash2(c), hash2(c + vec2<f32>(1.0, 0.0)), u.x), mix(hash2(c + vec2<f32>(0.0, 1.0)), hash2(c + vec2<f32>(1.0, 1.0)), u.x), u.y);
}

// Which view a pixel's in (two players: the second's rectangle).
fn view_of(px: vec2<f32>) -> i32 {
    if look.weather.w > 1.5 {
        let r = look.rect[1];
        if px.x >= r.x && px.y >= r.y && px.x < r.x + r.z && px.y < r.y + r.w {
            return 1;
        }
    }
    return 0;
}

// The world (units) at pixel `at`, at depth `d`, in view `v`.
fn world_at(at: vec2<i32>, d: f32, v: i32) -> vec3<f32> {
    let r = look.rect[v];
    let px = vec2<f32>(at) + 0.5;
    let ndc = vec2<f32>((px.x - r.x) / r.z * 2.0 - 1.0, 1.0 - (px.y - r.y) / r.w * 2.0);
    let p = look.inv[v] * vec4<f32>(ndc, d, 1.0);
    return p.xyz / p.w;
}

// The surface's normal at `at` from its neighbours' depths (the nearer
// side each way, so edges don't bend it), toward the eye.
fn normal_at(at: vec2<i32>, p: vec3<f32>, v: i32) -> vec3<f32> {
    let size = vec2<i32>(textureDimensions(depth));
    let xr = min(at + vec2<i32>(1, 0), size - 1);
    let xl = max(at - vec2<i32>(1, 0), vec2<i32>(0));
    let yd = min(at + vec2<i32>(0, 1), size - 1);
    let yu = max(at - vec2<i32>(0, 1), vec2<i32>(0));
    let pr = world_at(xr, textureLoad(depth, xr, 0), v);
    let pl = world_at(xl, textureLoad(depth, xl, 0), v);
    let pd = world_at(yd, textureLoad(depth, yd, 0), v);
    let pu = world_at(yu, textureLoad(depth, yu, 0), v);
    var dx = pr - p;
    if length(p - pl) < length(dx) {
        dx = p - pl;
    }
    var dy = pd - p;
    if length(p - pu) < length(dy) {
        dy = p - pu;
    }
    var n = normalize(cross(dy, dx));
    if dot(n, look.eye[v].xyz - p) < 0.0 {
        n = -n;
    }
    return n;
}

// Screen-space reflection (bevy_atmos's `ssr.wgsl`, simply): along `r`
// from `p` in view `v`, the level where the ray goes behind it, else the
// sky. `w` of the result: 1 hit, 0 the sky.
fn reflect_ray(p: vec3<f32>, r: vec3<f32>, v: i32) -> vec4<f32> {
    let units = look.fog_shape.z;
    let rect = look.rect[v];
    let size = vec2<i32>(textureDimensions(depth));
    var t = units * 0.3;
    var step = units * 0.25;
    for (var i = 0; i < 28; i += 1) {
        let q = p + r * t;
        let clip = look.view_proj[v] * vec4<f32>(q, 1.0);
        if clip.w <= 0.0 {
            break;
        }
        let ndc = clip.xy / clip.w;
        if any(abs(ndc) > vec2<f32>(1.0)) {
            break;
        }
        let px = rect.xy + (ndc * vec2<f32>(0.5, -0.5) + 0.5) * rect.zw;
        let at = clamp(vec2<i32>(px), vec2<i32>(0), size - 1);
        let ds = textureLoad(depth, at, 0);
        if ds > 0.0 {
            let scene_dist = look.fog_shape.y / ds;
            let behind = clip.w - scene_dist;
            if behind > 0.0 && behind < step * 2.0 + units * 0.5 {
                // Fading toward the screen's edges.
                let edge = 1.0 - smoothstep(0.8, 1.0, max(abs(ndc.x), abs(ndc.y)));
                let seen = to_linear(textureLoad(scene, at, 0).rgb) * look.grade2.xyz;
                return vec4<f32>(seen, edge);
            }
        }
        t += step;
        step *= 1.22;
    }
    return vec4<f32>(look.sky.rgb, 0.0);
}

// Rain's rings on a puddle at `xz` (metres): the surface's tilt.
fn rings(xz: vec2<f32>) -> vec2<f32> {
    var tilt = vec2<f32>(0.0);
    let cell = floor(xz * 2.0);
    for (var j = 0; j < 4; j += 1) {
        let c = cell + vec2<f32>(f32(j & 1), f32(j >> 1u)) - 0.5;
        let seed = hash2(c);
        let life = fract(look.fog_shape.w * 1.3 + seed);
        let centre = (c + vec2<f32>(hash2(c + 3.1), hash2(c + 7.7))) / 2.0;
        let off = xz - centre;
        let dist = length(off);
        let radius = life * 0.35;
        let ring = exp(-pow((dist - radius) / 0.025, 2.0)) * (1.0 - life);
        tilt += off / max(dist, 1e-3) * ring * 0.35;
    }
    return tilt * look.weather.z;
}

// The water's slope at `xz` (metres): Gerstner waves (bevy_atmos's
// `modern_water`, as slopes: the surface itself stays the level's), and
// finer ripples drifting over them; each fading out as its waves get
// smaller than the pixels (`fp`: metres a pixel spans there), so far
// water doesn't shimmer.
fn wave_slope(xz: vec2<f32>, t: f32, strength: f32, fp: f32) -> vec2<f32> {
    var g = vec2<f32>(0.0);
    let wind = vec2<f32>(0.5, -0.866);
    for (var i = 0; i < 6; i += 1) {
        let fi = f32(i);
        let a = (hash2(vec2<f32>(fi, 3.0)) - 0.5) * 1.8;
        let dir = vec2<f32>(cos(a) * wind.x - sin(a) * wind.y, sin(a) * wind.x + cos(a) * wind.y);
        let wl = 12.0 * pow(0.63, fi);
        let k = 6.2831853 / wl;
        let speed = sqrt(9.8 / k);
        let height = wl * 0.014 * strength * clamp(wl / (fp * 8.0) - 0.5, 0.0, 1.0);
        g += dir * (k * height * cos(k * (dot(dir, xz) - speed * t) + fi * 1.7));
    }
    for (var j = 0; j < 2; j += 1) {
        let s = select(2.3, 4.1, j == 1);
        let fine = clamp(1.0 / (s * fp * 8.0) - 0.5, 0.0, 1.0);
        if fine > 0.0 {
            let drift = select(vec2<f32>(0.31, -0.22), vec2<f32>(-0.27, -0.35), j == 1) * t;
            let q = xz * s + drift;
            let n0 = noise2(q);
            let gx = noise2(q + vec2<f32>(0.05, 0.0)) - n0;
            let gz = noise2(q + vec2<f32>(0.0, 0.05)) - n0;
            g += vec2<f32>(gx, gz) / 0.05 * 0.06 * (0.4 + strength) * fine;
        }
    }
    return g;
}

// The sky toward `r` in view `v`: the sky drawn alone where that is on
// screen, its nearest edge where not.
fn sky_toward(r_in: vec3<f32>, v: i32) -> vec3<f32> {
    // (Behind the camera: turned forward till it's in view, smoothly.)
    let mid = look.inv[v] * vec4<f32>(0.0, 0.0, 0.5, 1.0);
    let f = normalize(mid.xyz / mid.w - look.eye[v].xyz);
    let r = normalize(r_in + f * max(0.25 - dot(r_in, f), 0.0));
    let clip = look.view_proj[v] * vec4<f32>(r, 0.0);
    if clip.w <= 1e-4 {
        return look.sky.rgb;
    }
    let ndc = clamp(clip.xy / clip.w, vec2<f32>(-1.0), vec2<f32>(1.0));
    let rect = look.rect[v];
    let px = rect.xy + (ndc * vec2<f32>(0.5, -0.5) + 0.5) * max(rect.zw - 1.0, vec2<f32>(0.0));
    return to_linear(textureLoad(sky, vec2<i32>(px), 0).rgb);
}

// The water at `at` (its depth `wd`; the level beneath, `under`, at `d`):
// the waves; the level and sky mirrored by Fresnel; what's beneath seen
// through it, bent by the waves and lost to the water's own colour with
// depth; the sun's glitter; foam where it's shallow; rain's rings.
fn water_at(at: vec2<i32>, under: vec3<f32>, d: f32, wd: f32, px: vec2<f32>) -> vec3<f32> {
    let v = view_of(px);
    let units = look.fog_shape.z;
    let t = look.fog_shape.w;
    let strength = look.water.x;
    let p = world_at(at, wd, v);
    let eye = look.eye[v].xyz;
    let dist = length(p - eye) / units;
    let xz = p.xz / units;
    let grazing = max(abs(normalize(p - eye).y), 0.03);
    let fp = dist / (look.rect[v].w * grazing);
    var slope = wave_slope(xz, t, strength, fp);
    if look.weather.z > 0.0 {
        slope -= rings(xz) * 0.6;
    }
    let n = normalize(vec3<f32>(-slope.x, 1.0, -slope.y));
    let e = normalize(p - eye);
    var r = reflect(e, n);
    r.y = abs(r.y);
    let facing = max(dot(-e, n), 0.0);
    let fresnel = 0.02 + 0.98 * pow(1.0 - facing, 5.0);
    // The disc's own water colour, lit as the level is.
    let own = to_linear(textureLoad(water_base, at, 0).rgb) * look.grade2.xyz;
    var below = under;
    // Metres through the water, and straight down to the bottom.
    var thick = 1.0e4;
    var deep = 1.0e4;
    if d > 0.0 {
        let pf = world_at(at, d, v);
        thick = length(pf - p) / units;
        deep = (p.y - pf.y) / units;
        let size = vec2<i32>(textureDimensions(scene));
        let bend = vec2<i32>(slope * 30.0 * min(thick, 1.5) / max(dist * 0.1, 1.0));
        let at2 = clamp(at + bend, vec2<i32>(0), size - 1);
        let d2 = textureLoad(depth, at2, 0);
        if d2 > 0.0 && d2 < wd {
            below = to_linear(textureLoad(scene, at2, 0).rgb) * look.grade2.xyz;
        }
    }
    let through = exp(-vec3<f32>(0.55, 0.22, 0.16) * thick);
    var col = below * through + own * 0.9 * (1.0 - through);
    let hit = reflect_ray(p, r, v);
    let mirrored = mix(sky_toward(r, v), hit.rgb, hit.w);
    col = mix(col, mirrored, fresnel);
    let shallow = 1.0 - smoothstep(0.0, 0.35 + 0.3 * strength, deep);
    let lace = smoothstep(0.35, 0.75, noise2(xz * 3.0 + vec2<f32>(t * 0.15, 0.0)) * 0.6 + noise2(xz * 9.0 - t * 0.2) * 0.4 + shallow * 0.5);
    let foam = shallow * lace;
    let toward = max(dot(r, normalize(look.sun.xyz)), 0.0);
    col += look.sun_color.rgb * (pow(toward, 900.0) * 30.0 + pow(toward, 90.0) * 0.25) * (1.0 - foam);
    col = mix(col, vec3<f32>(0.85) * luminance(look.grade2.xyz), foam * 0.85);
    return col;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let at = vec2<i32>(in.clip.xy);
    let behind = to_linear(textureLoad(sky, at, 0).rgb);
    var c = to_linear(textureLoad(scene, at, 0).rgb);
    let d = textureLoad(depth, at, 0);

    if d > 0.0 {
        // The time of day's light over the level's own.
        c = c * look.grade2.xyz;
    }
    // The water, drawn alone, over what's beneath it where it's nearer.
    let wd = textureLoad(water_depth, at, 0);
    let water = look.water.x > 0.0 && wd > 0.0 && wd > d;
    var seen = d;
    if water {
        c = water_at(at, c, d, wd, in.clip.xy);
        seen = wd;
    }
    if d > 0.0 && !water {
        if look.weather.x > 0.0 || look.weather.y > 0.0 {
            let v = view_of(in.clip.xy);
            let p = world_at(at, d, v);
            let n = normal_at(at, p, v);
            let open = open_sky(p, n);
            let wet = look.weather.x * open;
            let snow = look.weather.y * open;
            let units = look.fog_shape.z;
            let xz = p.xz / units;
            let up = smoothstep(0.7, 0.92, n.y);
            let air = luminance(look.grade2.xyz);
            if wet > 0.0 {
                // Soaked: darker, the ground most.
                c = c * mix(1.0, 0.55, wet * up) * mix(1.0, 0.85, wet * (1.0 - up));
                // Puddles where it's flat, as the rain has filled them.
                let m = noise2(xz / 2.5) * 0.65 + noise2(xz / 0.9 + 4.0) * 0.35;
                let level = smoothstep(0.95, 0.99, n.y);
                let puddle = smoothstep(0.6, 0.66, m + (wet - 1.0) * 0.25) * level * smoothstep(0.25, 0.7, wet);
                var nr = n;
                if puddle > 0.0 {
                    let tilt = rings(xz);
                    nr = normalize(vec3<f32>(tilt.x, 1.0, tilt.y));
                }
                let eye_dir = normalize(p - look.eye[v].xyz);
                let r = reflect(eye_dir, nr);
                let fresnel = 0.02 + 0.98 * pow(1.0 - max(dot(-eye_dir, nr), 0.0), 5.0);
                let sheen = wet * up * fresnel * 0.5;
                if puddle > 0.0 || sheen > 0.02 {
                    let hit = reflect_ray(p, r, v);
                    let refl = mix(sky_toward(vec3<f32>(r.x, abs(r.y), r.z), v) * air, hit.rgb, hit.w);
                    c = mix(c, refl, puddle * (0.45 + 0.5 * fresnel));
                    c = c + refl * sheen * (1.0 - puddle);
                }
            }
            if snow > 0.0 {
                // Settled snow on what faces up, patchy at first.
                let patches = noise2(xz / 1.3) * 0.6 + noise2(xz / 0.35) * 0.4;
                let lie = smoothstep(0.55, 0.85, n.y) * smoothstep(1.0 - snow, 1.0 - snow + 0.25, patches * 0.7 + 0.3 * snow + 0.15);
                c = mix(c, vec3<f32>(0.85, 0.88, 0.95) * min(air * 1.1, 1.0), clamp(lie * snow * 1.4, 0.0, 0.92));
            }
        }
    }

    // Fog where something's drawn (depth 0 is the sky, infinitely far),
    // exponential, half-hidden at `fog.a` past `fog_shape.x`, fading into
    // the sky behind (as bevy_atmos's is the sky's horizon colour), a
    // little of `fog.rgb`'s haze in it.
    if seen > 0.0 && look.fog.a > 0.0 {
        let distance = look.fog_shape.y / seen;
        let past = max(distance - look.fog_shape.x, 0.0);
        let hidden = 1.0 - exp(-0.6931472 * past / look.fog.a);
        let haze = mix(behind, to_linear(look.fog.rgb), 0.25);
        c = mix(c, haze, clamp(hidden, 0.0, 1.0));
    }

    // The grade: exposure, white balance (warmth, tint), contrast round
    // middle grey, saturation.
    c = c * exp2(look.grade.x);
    let warm = look.grade.y;
    let tint = look.grade.z;
    let balance = vec3<f32>(1.0 + 0.25 * warm + 0.1 * tint, 1.0 - 0.2 * tint, 1.0 - 0.25 * warm + 0.1 * tint);
    let before = luminance(c);
    c = c * balance;
    c = c * (before / max(luminance(c), 1e-5));
    let grey = 0.18;
    c = grey * pow(max(c, vec3<f32>(0.0)) / grey, vec3<f32>(look.grade.w));
    c = mix(vec3<f32>(luminance(c)), c, look.grade2.w);
    return vec4<f32>(to_gamma(c), 1.0);
}
