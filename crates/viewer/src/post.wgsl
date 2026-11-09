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
};

@group(0) @binding(0) var scene: texture_2d<f32>;
@group(0) @binding(1) var depth: texture_depth_2d;
@group(0) @binding(2) var<uniform> look: Look;
// The sky alone, drawn behind everything: what's far fades into it.
@group(0) @binding(3) var sky: texture_2d<f32>;
// The level from straight above (depth 0 at the top): what's under a roof.
@group(0) @binding(4) var shelter: texture_depth_2d;

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

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let at = vec2<i32>(in.clip.xy);
    let behind = to_linear(textureLoad(sky, at, 0).rgb);
    var c = to_linear(textureLoad(scene, at, 0).rgb);
    let d = textureLoad(depth, at, 0);

    if d > 0.0 {
        // The time of day's light over the level's own.
        c = c * look.grade2.xyz;
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
                    let refl = mix(look.sky.rgb * air, hit.rgb, hit.w);
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
    if d > 0.0 && look.fog.a > 0.0 {
        let distance = look.fog_shape.y / d;
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
