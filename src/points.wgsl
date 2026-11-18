// Draws each point as a screen-space quad (6 vertices, one instance per point),
// trimmed to a circle in the fragment shader. The vertex entry point picks the
// colour: `vs_height` (viridis ramp on Z), `vs_rgb` (the file's colours) or
// `vs_class` (a colour per LAS class). The `_lifted` variants draw each point
// at a height from a separate buffer (one read from an extra dimension)
// instead of its Z.

struct Uniforms {
    view_proj: mat4x4<f32>,
    viewport_size: vec2<f32>,
    point_size: f32,
    z_min: f32,
    z_max: f32,
    // Multiplies RGB after unpacking: 1 for 16-bit colour, 257 for files that
    // store 8-bit values in the 16-bit fields.
    color_scale: f32,
    // Pixels per metre at 1 m depth (perspective, where clip w is the depth)
    // or at any depth (orthographic, where w is 1).
    pixel_scale: f32,
    // The LOD's target spacing in pixels.
    target_spacing: f32,
    // (fill, min px, max px, on): when on > 0, points are sized by their
    // spacing on screen instead of `point_size`.
    adaptive: vec4<f32>,
    // Points are drawn only inside all four planes: dot(plane, (p, 1)) >= 0.
    // (0, 0, 0, 1) keeps everything.
    clip_planes: array<vec4<f32>, 4>,
    // Points must also be inside this circle on the ground when w > 0:
    // (centre x, centre y, radius, on).
    clip_circle: vec4<f32>,
    // RGB for each LAS class; w is padding.
    class_colors: array<vec4<f32>, 256>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

// The node being drawn, for adaptive sizes.
struct Node {
    // Where the node's octants meet.
    centre: vec3<f32>,
    // Metres between the node's points.
    spacing: f32,
    // For each octant, the spacing of the finest node drawn in it (this one,
    // or one below it): octants 0 to 3, then 4 to 7. An octant is numbered
    // 1 for upper X + 2 for upper Y + 4 for upper Z.
    finest: array<vec4<f32>, 2>,
};

@group(1) @binding(0) var<uniform> node: Node;

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) corner: vec2<f32>,
    @location(1) color: vec3<f32>,
};

@vertex
fn vs_height(
    @builtin(vertex_index) vertex_index: u32,
    @location(0) position: vec3<f32>,
) -> VertexOutput {
    return height_vertex(vertex_index, position, position);
}

@vertex
fn vs_rgb(
    @builtin(vertex_index) vertex_index: u32,
    @location(0) position: vec3<f32>,
    // Unorm16x4: the GPU divides the stored u16s by 65535.
    @location(1) color: vec4<f32>,
) -> VertexOutput {
    return rgb_vertex(vertex_index, position, position, color);
}

@vertex
fn vs_class(
    @builtin(vertex_index) vertex_index: u32,
    @location(0) position: vec3<f32>,
    @location(1) classification: u32,
) -> VertexOutput {
    return class_vertex(vertex_index, position, position, classification);
}

@vertex
fn vs_height_lifted(
    @builtin(vertex_index) vertex_index: u32,
    @location(0) position: vec3<f32>,
    @location(2) height: f32,
) -> VertexOutput {
    return height_vertex(vertex_index, vec3(position.xy, height), position);
}

@vertex
fn vs_rgb_lifted(
    @builtin(vertex_index) vertex_index: u32,
    @location(0) position: vec3<f32>,
    @location(1) color: vec4<f32>,
    @location(2) height: f32,
) -> VertexOutput {
    return rgb_vertex(vertex_index, vec3(position.xy, height), position, color);
}

@vertex
fn vs_class_lifted(
    @builtin(vertex_index) vertex_index: u32,
    @location(0) position: vec3<f32>,
    @location(1) classification: u32,
    @location(2) height: f32,
) -> VertexOutput {
    return class_vertex(vertex_index, vec3(position.xy, height), position, classification);
}

// `position` is where the point is drawn; `in_octree` is where it is in the
// octree (its file Z), which tells which octant of its node it's in.

fn height_vertex(vertex_index: u32, position: vec3<f32>, in_octree: vec3<f32>) -> VertexOutput {
    let t = clamp((position.z - u.z_min) / max(u.z_max - u.z_min, 1e-6), 0.0, 1.0);
    return point_vertex(vertex_index, position, in_octree, viridis(t));
}

fn rgb_vertex(
    vertex_index: u32,
    position: vec3<f32>,
    in_octree: vec3<f32>,
    color: vec4<f32>,
) -> VertexOutput {
    let rgb = min(color.rgb * u.color_scale, vec3(1.0));
    return point_vertex(vertex_index, position, in_octree, rgb);
}

fn class_vertex(
    vertex_index: u32,
    position: vec3<f32>,
    in_octree: vec3<f32>,
    classification: u32,
) -> VertexOutput {
    return point_vertex(vertex_index, position, in_octree, u.class_colors[classification].rgb);
}

// The point's diameter in pixels, when its clip-space w is `w`. Must match
// `AdaptiveSize::diameter_px` in renderer.rs.
fn diameter_px(w: f32, in_octree: vec3<f32>) -> f32 {
    let adaptive = u.adaptive;
    if adaptive.w <= 0.0 {
        return u.point_size;
    }
    // The node's spacing on screen at this point's depth.
    let spacing_px = node.spacing * u.pixel_scale / max(w, 1e-6);
    // Finer nodes drawn in this point's octant add points between its
    // node's, so its neighbours are closer: halve the spacing once per
    // level the LOD refined here (until it was within the target), but not
    // below the finest node actually drawn there.
    let d = in_octree - node.centre;
    let octant = select(0u, 1u, d.x >= 0.0) | select(0u, 2u, d.y >= 0.0)
        | select(0u, 4u, d.z >= 0.0);
    let finest_px = node.finest[octant / 4u][octant % 4u] * u.pixel_scale / max(w, 1e-6);
    let halvings = max(ceil(log2(spacing_px / max(u.target_spacing, 1e-6))), 0.0);
    let refined = spacing_px * exp2(-halvings);
    let spacing = min(spacing_px, max(refined, finest_px));
    return clamp(adaptive.x * spacing, adaptive.y, adaptive.z);
}

fn point_vertex(
    vertex_index: u32,
    position: vec3<f32>,
    in_octree: vec3<f32>,
    color: vec3<f32>,
) -> VertexOutput {
    var corners = array<vec2<f32>, 6>(
        vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(1.0, 1.0),
        vec2(-1.0, -1.0), vec2(1.0, 1.0), vec2(-1.0, 1.0),
    );
    let corner = corners[vertex_index];

    var out: VertexOutput;
    if is_clipped(position) {
        // Put all 6 vertices at one spot outside clip space, so the quad has
        // no area and the GPU drops it.
        out.clip_position = vec4(2.0, 2.0, 2.0, 1.0);
        return out;
    }

    let center = u.view_proj * vec4(position, 1.0);
    // Offset in NDC so the quad is the diameter in pixels whatever the depth.
    // Multiplying by w undoes the perspective divide the GPU applies afterwards.
    let offset = corner * diameter_px(center.w, in_octree) / u.viewport_size * center.w;

    out.clip_position = vec4(center.xy + offset, center.zw);
    out.corner = corner;
    out.color = color;
    return out;
}

// Whether a point is outside the subcloud being shown.
fn is_clipped(position: vec3<f32>) -> bool {
    for (var i = 0; i < 4; i++) {
        if dot(u.clip_planes[i], vec4(position, 1.0)) < 0.0 {
            return true;
        }
    }
    let circle = u.clip_circle;
    return circle.w > 0.0 && distance(position.xy, circle.xy) > circle.z;
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
