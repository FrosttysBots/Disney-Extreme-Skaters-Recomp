struct Globals {
    view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    time: vec4<f32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(@location(0) position: vec3<f32>, @location(1) color: vec4<f32>) -> VertexOut {
    var out: VertexOut;
    out.clip = globals.view_proj * vec4<f32>(position, 1.0);
    out.color = color;
    return out;
}

@fragment
fn fs_overlay(in: VertexOut) -> @location(0) vec4<f32> {
    return in.color;
}

@fragment
fn fs_solid(in: VertexOut) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color.rgb, 1.0);
}
