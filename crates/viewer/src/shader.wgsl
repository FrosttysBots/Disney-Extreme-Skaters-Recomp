struct Globals {
    view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    // x: seconds since start, y: minimum light level
    time: vec4<f32>,
};

struct PassParams {
    // u velocity, v velocity, u frequency, v frequency
    wibble_velocity_frequency: vec4<f32>,
    // u amplitude, v amplitude, u phase, v phase
    wibble_amplitude_phase: vec4<f32>,
    // x: UV set, y: fixed alpha (negative = none), z: alpha test threshold,
    // w: 1 if the pass is environment mapped
    settings: vec4<f32>,
    // x: 1 to output alpha as the color (modulate and brighten blending)
    options: vec4<f32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var base_texture: texture_2d<f32>;
@group(1) @binding(1) var base_sampler: sampler;
@group(1) @binding(2) var<uniform> pass_params: PassParams;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv0: vec2<f32>,
    @location(3) uv1: vec2<f32>,
    @location(4) uv2: vec2<f32>,
    @location(5) color: vec4<f32>,
};

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    var out: VertexOut;
    out.clip = globals.view_proj * vec4<f32>(in.position, 1.0);

    var uv = in.uv0;
    let uv_set = u32(pass_params.settings.x);
    if uv_set == 1u {
        uv = in.uv1;
    } else if uv_set == 2u {
        uv = in.uv2;
    }
    if pass_params.settings.w > 0.5 {
        // Sphere-style environment map from the view-space normal.
        let n = (globals.view * vec4<f32>(in.normal, 0.0)).xyz;
        uv = n.xy * 0.5 + 0.5;
    }
    let t = globals.time.x;
    let vf = pass_params.wibble_velocity_frequency;
    let ap = pass_params.wibble_amplitude_phase;
    uv += vf.xy * t + ap.xy * sin(vf.zw * t + ap.zw);

    // UVs have a bottom-left origin; textures are uploaded top row first.
    out.uv = vec2<f32>(uv.x, 1.0 - uv.y);
    // Vertex colors use 0x80 as full brightness, so they can brighten up to 2x.
    // time.y is an optional minimum light level for seeing into dark areas.
    let lit = in.color * (255.0 / 128.0);
    out.color = vec4<f32>(max(lit.rgb, vec3<f32>(globals.time.y)), lit.a);
    return out;
}

@fragment
fn fs_opaque(in: VertexOut) -> @location(0) vec4<f32> {
    let texel = textureSample(base_texture, base_sampler, in.uv);
    if texel.a < pass_params.settings.z {
        discard;
    }
    return vec4<f32>(texel.rgb * in.color.rgb, 1.0);
}

@fragment
fn fs_blend(in: VertexOut) -> @location(0) vec4<f32> {
    let texel = textureSample(base_texture, base_sampler, in.uv);
    var alpha = texel.a * min(in.color.a, 1.0);
    if pass_params.settings.y >= 0.0 {
        alpha = pass_params.settings.y;
    }
    if pass_params.options.x > 0.5 {
        return vec4<f32>(vec3<f32>(alpha), alpha);
    }
    return vec4<f32>(texel.rgb * in.color.rgb, alpha);
}
