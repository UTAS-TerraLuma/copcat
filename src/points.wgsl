// Draws each point as a screen-space quad (6 vertices, one instance per point),
// trimmed to a circle in the fragment shader.

struct Uniforms {
    view_proj: mat4x4<f32>,
    viewport_size: vec2<f32>,
    point_size: f32,
    z_min: f32,
    z_max: f32,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) corner: vec2<f32>,
    @location(1) color: vec3<f32>,
};

@vertex
fn vs_main(
    @builtin(vertex_index) vertex_index: u32,
    @location(0) position: vec3<f32>,
) -> VertexOutput {
    var corners = array<vec2<f32>, 6>(
        vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(1.0, 1.0),
        vec2(-1.0, -1.0), vec2(1.0, 1.0), vec2(-1.0, 1.0),
    );
    let corner = corners[vertex_index];

    let center = u.view_proj * vec4(position, 1.0);
    // Offset in NDC so the quad is `point_size` pixels wide whatever the depth.
    // Multiplying by w undoes the perspective divide the GPU applies afterwards.
    let offset = corner * u.point_size / u.viewport_size * center.w;

    var out: VertexOutput;
    out.clip_position = vec4(center.xy + offset, center.zw);
    out.corner = corner;
    let t = clamp((position.z - u.z_min) / max(u.z_max - u.z_min, 1e-6), 0.0, 1.0);
    out.color = viridis(t);
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    if dot(in.corner, in.corner) > 1.0 {
        discard;
    }
    return vec4(in.color, 1.0);
}

// Polynomial fit of the viridis colour map (https://www.shadertoy.com/view/WlfXRN).
fn viridis(t: f32) -> vec3<f32> {
    let c0 = vec3(0.2777273272234177, 0.005407344544966578, 0.3340998053353061);
    let c1 = vec3(0.1050930431085774, 1.404613529898575, 1.384590162594685);
    let c2 = vec3(-0.3308618287255563, 0.214847559468213, 0.09509516302823659);
    let c3 = vec3(-4.634230498983486, -5.799100973351585, -19.33244095627987);
    let c4 = vec3(6.228269936347081, 14.17993336680509, 56.69055260068105);
    let c5 = vec3(4.776384997670288, -13.74514537774601, -65.35303263337234);
    let c6 = vec3(-5.435455855934631, 4.645852612178535, 26.3124352495832);
    return c0 + t * (c1 + t * (c2 + t * (c3 + t * (c4 + t * (c5 + t * c6)))));
}
