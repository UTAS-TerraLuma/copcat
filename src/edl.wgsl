// Eye-dome lighting: a screen-space pass that darkens each pixel by how much
// nearer its neighbours are, which outlines shapes and gives depth cues
// without needing normals. Follows Potree's EDL (Boucheny 2009).
//
// Outputs a shade factor; the pipeline's blend state multiplies it into the
// colour already drawn, so no second colour texture is needed.

struct EdlUniforms {
    near: f32,
    far: f32,
    strength: f32,
    radius_px: f32,
    // 1 for an orthographic projection, 0 for perspective.
    orthographic: u32,
    // Eye to orbit target, which sets the zoom in orthographic mode.
    target_distance: f32,
};

@group(0) @binding(0) var<uniform> e: EdlUniforms;
@group(0) @binding(1) var depth_texture: texture_depth_2d;

const NEIGHBOURS: i32 = 8;
// A background pixel counts each neighbouring point as this much nearer (in
// log2 depth), which draws a dark silhouette just outside the cloud.
const SILHOUETTE_RESPONSE: f32 = 1.0;

// One triangle that covers the whole screen: (-1,-1), (3,-1), (-1,3).
@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4(uv * 2.0 - 1.0, 0.0, 1.0);
}

// Undoes the depth mapping (wgpu's 0..1 range) to get the distance along the
// view direction. Log2 makes the response the same at any zoom.
fn log_depth(ndc_depth: f32) -> f32 {
    if e.orthographic != 0u {
        // Orthographic depth is linear, and can be negative behind the eye, so
        // there's no log. Near the target, d(log2 z) = dz / (z ln 2); dividing
        // by the target distance instead gives the same shading as perspective
        // there, and like perspective it doesn't change when zooming.
        let view_depth = mix(e.near, e.far, ndc_depth);
        return view_depth / (e.target_distance * 0.6931472);
    }
    let view_depth = e.near * e.far / (e.far - ndc_depth * (e.far - e.near));
    return log2(view_depth);
}

@fragment
fn fs_main(@builtin(position) frag_coord: vec4<f32>) -> @location(0) vec4<f32> {
    let size = vec2<i32>(textureDimensions(depth_texture));
    let pixel = vec2<i32>(frag_coord.xy);
    let depth = textureLoad(depth_texture, pixel, 0);
    let is_background = depth >= 1.0;
    let centre = log_depth(depth);

    var response = 0.0;
    for (var i = 0; i < NEIGHBOURS; i++) {
        let angle = f32(i) * 6.2831853 / f32(NEIGHBOURS);
        let offset = vec2<i32>(round(vec2(cos(angle), sin(angle)) * e.radius_px));
        let neighbour = clamp(pixel + offset, vec2(0), size - 1);
        let neighbour_depth = textureLoad(depth_texture, neighbour, 0);
        // Background neighbours add nothing. Where points are sparse (a coarse
        // node, or close to the camera) most neighbours are gaps between
        // points, and counting those as edges turned whole regions black.
        if neighbour_depth >= 1.0 {
            continue;
        }
        if is_background {
            response += SILHOUETTE_RESPONSE;
        } else {
            response += max(0.0, centre - log_depth(neighbour_depth));
        }
    }
    let shade = exp(-response / f32(NEIGHBOURS) * 300.0 * e.strength);
    return vec4(shade, shade, shade, 1.0);
}
