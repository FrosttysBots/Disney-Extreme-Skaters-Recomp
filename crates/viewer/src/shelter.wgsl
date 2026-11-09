// The shelter map: the level seen from straight above, depth only (the
// highest thing over each spot), for where rain and snow can't fall.

@group(0) @binding(0) var<uniform> projection: mat4x4<f32>;

@vertex
fn vs_main(@location(0) position: vec3<f32>) -> @builtin(position) vec4<f32> {
    return projection * vec4<f32>(position, 1.0);
}
