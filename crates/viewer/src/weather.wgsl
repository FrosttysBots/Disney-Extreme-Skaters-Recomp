// Rain and snow, after bevy_atmos's (`rain.wgsl`, MIT or Apache-2.0):
// streaks, splashes and flakes, placed here from the clock, the camera and
// the shelter map (where rain can't fall: the level seen from above). Each
// quad is six vertices, made from the vertex index alone.
//
// Worked in metres; the level's units are inches (39.37 a metre).

struct Globals {
    view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    time: vec4<f32>,
};

struct Weather {
    // xyz: the box's centre (the eye, metres), w: the clock (seconds).
    centre: vec4<f32>,
    // xyz: the box's half size (m), w: the share of quads drawn.
    extent: vec4<f32>,
    // xyz: the drops' velocity (m/s), w: streak length per m/s.
    velocity: vec4<f32>,
    // rgb: the air's light (as drawn, 0..1), w: how strong.
    light: vec4<f32>,
    // xyz: toward the key light, w: unused.
    key_dir: vec4<f32>,
    // rgb: the key light's colour and strength.
    key_light: vec4<f32>,
    // x: 0 near streaks, 1 far streaks, 2 splashes, 5 near snow, 6 far
    // snow.
    kind: vec4<f32>,
    // The shelter map: x, z of its corner and its size (m), its top and
    // bottom heights (m).
    shelter: vec4<f32>,
    shelter_y: vec4<f32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var<uniform> w: Weather;
// The level seen from above: depth 0 at the top, 1 at the bottom.
@group(1) @binding(1) var heights: texture_depth_2d;

const UNITS: f32 = 39.37;
const NEAR_EXTENT: vec3<f32> = vec3<f32>(14.0, 9.0, 14.0);
const SNOW_NEAR: vec3<f32> = vec3<f32>(12.0, 8.0, 12.0);
const SPLASH_RATE: f32 = 2.2;
const PI: f32 = 3.14159265;

struct Out {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) fade: f32,
    @location(2) world: vec3<f32>,
    // The eye (m): the fragments can't read the globals.
    @location(3) eye: vec3<f32>,
}

fn hash(n: u32) -> u32 {
    var x = n * 747796405u + 2891336453u;
    x = ((x >> ((x >> 28u) + 4u)) ^ x) * 277803737u;
    return (x >> 22u) ^ x;
}

fn rand(n: u32) -> f32 {
    return f32(hash(n)) / 4294967295.0;
}

// How high rain gets at `p` (m): the top of the level there, or very low
// off the map.
fn height_at(p: vec3<f32>) -> f32 {
    let size = vec2<f32>(textureDimensions(heights));
    let uv = (p.xz - w.shelter.xy) / w.shelter.z;
    if any(uv < vec2<f32>(0.0)) || any(uv >= vec2<f32>(1.0)) {
        return -1.0e6;
    }
    let d = textureLoad(heights, vec2<i32>(uv * size), 0);
    if d >= 1.0 {
        return -1.0e6;
    }
    return mix(w.shelter_y.x, w.shelter_y.y, d);
}

// The eye (m), from the view matrix.
fn eye() -> vec3<f32> {
    let v = globals.view;
    let r = mat3x3<f32>(v[0].xyz, v[1].xyz, v[2].xyz);
    return -(transpose(r) * v[3].xyz) / UNITS;
}

fn hidden(out: Out) -> Out {
    var o = out;
    o.clip = vec4<f32>(2.0, 2.0, 2.0, 1.0);
    o.fade = 0.0;
    return o;
}

fn place(world: vec3<f32>) -> vec4<f32> {
    return globals.view_proj * vec4<f32>(world * UNITS, 1.0);
}

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> Out {
    let i = vid / 6u;
    let corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
    );
    let corner = corners[vid % 6u];
    let k = i32(w.kind.x + 0.5);
    var out: Out;
    out.uv = corner;
    out.fade = 1.0;
    out.clip = vec4<f32>(0.0);
    out.world = vec3<f32>(0.0);
    let eye = eye();
    out.eye = eye;
    if rand(i * 9u + 3u) > w.extent.w {
        return hidden(out);
    }
    let t = w.centre.w;
    if k == 5 || k == 6 {
        // A snowflake: drifting down through the box round the camera (and
        // wrapping in it), fluttering as it goes; none under a roof.
        let size = w.extent.xyz * 2.0;
        let lo = w.centre.xyz - w.extent.xyz;
        let base = vec3<f32>(rand(i * 9u), rand(i * 9u + 1u), rand(i * 9u + 2u)) * size;
        let speed = 0.8 + 0.4 * rand(i * 9u + 4u);
        let ph = rand(i * 9u + 5u) * 2.0 * PI;
        let f = 0.6 + 0.8 * rand(i * 9u + 6u);
        let flutter = vec3<f32>(sin(t * f * 2.1 + ph), 0.0, cos(t * f * 1.7 + ph * 1.3)) * (0.18 / f);
        let p0 = lo + fract((base + w.velocity.xyz * speed * t - lo) / size) * size;
        let p = p0 + flutter;
        let off = p0 - w.centre.xyz;
        if k == 6 && all(abs(off) < SNOW_NEAR) {
            return hidden(out);
        }
        if p.y < height_at(p) {
            return hidden(out);
        }
        let to_eye = eye - p;
        let dist = length(to_eye);
        let fwd = to_eye / max(dist, 1e-3);
        let side = normalize(cross(vec3<f32>(0.0, 1.0, 0.0), fwd));
        let up = cross(fwd, side);
        let r = select(0.012 + 0.016 * rand(i * 9u + 7u), 0.008 + 0.01 * rand(i * 9u + 7u), k == 5);
        let width = max(r, dist * 0.0012);
        let world = p + (side * corner.x + up * (corner.y * 2.0 - 1.0)) * width;
        out.world = world;
        let edge = min(min(w.extent.x - abs(off.x), w.extent.y - abs(off.y)), w.extent.z - abs(off.z));
        let shrink = select((r / width) * (r / width), r / width, k == 6);
        let close = select(1.0, mix(0.45, 1.0, smoothstep(1.5, 5.0, dist)), k == 5);
        out.fade = smoothstep(0.4, 1.4, dist) * smoothstep(0.0, 2.0, edge) * shrink * close * (0.6 + 0.4 * rand(i * 9u + 8u));
        out.clip = place(world);
        return out;
    }
    if k == 2 {
        // A splash: a ripple ring spreading flat on whatever the rain lands
        // on, each in a place of its own (wrapped into the box), a new
        // place each time round.
        let phase = t * SPLASH_RATE + rand(i * 9u + 5u);
        let life = fract(phase);
        let s = i * 9u + u32(floor(phase)) * 7919u;
        let size2 = w.extent.xz * 2.0;
        let lo = w.centre.xz - w.extent.xz;
        let base = vec2<f32>(rand(s), rand(s + 1u)) * 997.0;
        let xz = lo + fract((base - lo) / size2) * size2;
        let spot = vec3<f32>(xz.x, 0.0, xz.y);
        let h = height_at(spot);
        if h < -1.0e5 || h > w.centre.y + 4.0 {
            return hidden(out);
        }
        let p = vec3<f32>(spot.x, h + 0.02, spot.z);
        let dist = length(eye - p);
        let radius = 0.04 + 0.1 * sqrt(life);
        let world = p + vec3<f32>(corner.x * radius, 0.0, (corner.y * 2.0 - 1.0) * radius);
        let edge = min(w.extent.x - abs(xz.x - w.centre.x), w.extent.z - abs(xz.y - w.centre.z));
        out.fade = (1.0 - smoothstep(w.extent.x * 0.5, w.extent.x, dist)) * smoothstep(0.0, 1.5, edge);
        out.world = vec3<f32>(life, 0.0, 0.0);
        out.clip = place(world);
        return out;
    }
    // A streak: drops fall through the box and wrap round in it, so they
    // stay put in the world as the box follows the camera.
    let size = w.extent.xyz * 2.0;
    let lo = w.centre.xyz - w.extent.xyz;
    let base = vec3<f32>(rand(i * 9u), rand(i * 9u + 1u), rand(i * 9u + 2u)) * size;
    let speed = 0.85 + 0.3 * rand(i * 9u + 4u);
    let vel = w.velocity.xyz * speed;
    let p = lo + fract((base + vel * t - lo) / size) * size;
    let off = p - w.centre.xyz;
    if k == 1 && all(abs(off) < NEAR_EXTENT) {
        return hidden(out);
    }
    // None indoors, under cover or below a roof.
    if p.y < height_at(p) {
        return hidden(out);
    }
    let dir = normalize(vel);
    let len = length(vel) * w.velocity.w * select(1.0, 2.5, k == 1) + 0.12;
    let head = p;
    let tail = p - dir * len;
    let to_eye = eye - p;
    let dist = length(to_eye);
    let side = normalize(cross(dir, to_eye));
    let width = max(0.0035, dist * 0.0011);
    let world = mix(head, tail, corner.y) + side * corner.x * width;
    out.world = world;
    let edge = min(min(w.extent.x - abs(off.x), w.extent.y - abs(off.y)), w.extent.z - abs(off.z));
    let bright = 0.35 + 0.65 * pow(rand(i * 9u + 8u), 3.0);
    out.fade = smoothstep(0.6, 2.5, dist) * smoothstep(0.0, 2.0, edge) * (0.0035 / width) * bright;
    out.clip = place(world);
    return out;
}

@fragment
fn fs_main(in: Out) -> @location(0) vec4<f32> {
    let k = i32(w.kind.x + 0.5);
    let lum = vec3<f32>(0.2126, 0.7152, 0.0722);
    let air = dot(w.light.rgb, lum);
    if k == 5 || k == 6 {
        // A soft round flake, white in the air's light and the key light's.
        let d = length(vec2<f32>(in.uv.x, in.uv.y * 2.0 - 1.0));
        let a = smoothstep(1.0, 0.25, d) * max(in.fade, 0.0);
        let shade = 0.55 + 0.45 * air + 0.25 * dot(w.key_light.rgb, lum);
        return vec4<f32>(vec3<f32>(0.96, 0.98, 1.0) * min(shade, 1.0), a * 0.9);
    }
    if k == 2 {
        // A ripple: a thin ring spreading out and fading.
        let life = in.world.x;
        let r = length(vec2<f32>(in.uv.x, in.uv.y * 2.0 - 1.0));
        let ring = 0.55 + 0.4 * life;
        let a = smoothstep(0.08, 0.0, abs(r - ring)) * (1.0 - life) * (1.0 - life) * max(in.fade, 0.0);
        return vec4<f32>(vec3<f32>(0.75 + 0.25 * air), a * 0.35 * w.light.w);
    }
    // A streak, brighter looking toward the key light (forward scatter).
    let across = 1.0 - abs(in.uv.x);
    let a = across * across * sin(in.uv.y * PI) * max(in.fade, 0.0);
    let v = normalize(in.world - in.eye);
    let cs = dot(v, normalize(w.key_dir.xyz));
    let g = 0.7;
    let hg = (1.0 - g * g) / pow(1.0 + g * g - 2.0 * g * cs, 1.5) / (4.0 * PI);
    let lit = air * 0.8 + dot(w.key_light.rgb, lum) * (0.15 + 1.5 * hg);
    let strength = select(0.55, 0.3, k == 1);
    return vec4<f32>(vec3<f32>(0.9, 0.93, 1.0) * min(lit, 1.2), a * strength * w.light.w);
}
