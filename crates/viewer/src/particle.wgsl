// Particles: camera-facing quads with a texture, tinted by the vertex
// colour (alpha too); blended additively or over what's behind.

struct Globals {
    view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    time: vec4<f32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var particle_texture: texture_2d<f32>;
@group(1) @binding(1) var particle_sampler: sampler;

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_main(
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
) -> VertexOut {
    var out: VertexOut;
    out.clip = globals.view_proj * vec4<f32>(position, 1.0);
    out.uv = uv;
    out.color = color;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let texel = textureSample(particle_texture, particle_sampler, in.uv);
    return texel * in.color;
}
