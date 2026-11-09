// The look over the finished frame (after bevy_atmos's air and grade):
// distance fog from the depth buffer, then the colour grade.

struct Look {
    // rgb: the fog's colour (as drawn, gamma), a: metres to half-hidden in
    // world units (0: no fog)
    fog: vec4<f32>,
    // x: where fog starts (world units), y: the near plane (reversed,
    // infinite Z: distance = near / depth)
    fog_shape: vec4<f32>,
    // x: exposure (stops), y: warmth, z: tint, w: contrast
    grade: vec4<f32>,
    // x: saturation
    grade2: vec4<f32>,
};

@group(0) @binding(0) var scene: texture_2d<f32>;
@group(0) @binding(1) var depth: texture_depth_2d;
@group(0) @binding(2) var<uniform> look: Look;
// The sky alone, drawn behind everything: what's far fades into it.
@group(0) @binding(3) var sky: texture_2d<f32>;

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

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let at = vec2<i32>(in.clip.xy);
    let behind = to_linear(textureLoad(sky, at, 0).rgb);
    var c = to_linear(textureLoad(scene, at, 0).rgb);

    // Fog where something's drawn (depth 0 is the sky, infinitely far),
    // exponential, half-hidden at `fog.a` past `fog_shape.x`, fading into
    // the sky behind (as bevy_atmos's is the sky's horizon colour), a
    // little of `fog.rgb`'s haze in it.
    let d = textureLoad(depth, at, 0);
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
    c = mix(vec3<f32>(luminance(c)), c, look.grade2.x);
    return vec4<f32>(to_gamma(c), 1.0);
}
