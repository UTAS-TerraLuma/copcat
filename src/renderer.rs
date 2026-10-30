//! Renders point cloud nodes into an offscreen texture that egui displays as an image.
//!
//! Each frame runs up to three passes into the same colour texture:
//! 1. points (colour + depth),
//! 2. eye-dome lighting, which reads that depth and darkens the colour,
//! 3. debug lines, depth-tested against the points but not shaded by EDL.
//!
//! The same passes can also run without a window (see [`PointRenderer::headless`])
//! and be read back as an image with [`PointRenderer::capture`].

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use copc_streaming::VoxelKey;
use eframe::egui;
use eframe::egui::mutex::RwLock;
use eframe::egui_wgpu::{self, wgpu};
use glam::{Vec3, Vec4};
use wgpu::util::DeviceExt;

use crate::camera::{OrbitCamera, Projection};
use crate::subcloud::Footprint;

const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const VERTICES_PER_POINT: u32 = 6;
const BACKGROUND: wgpu::Color = wgpu::Color {
    r: 0.08,
    g: 0.08,
    b: 0.09,
    a: 1.0,
};

/// Must match `Uniforms` in `points.wgsl` byte for byte.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    view_proj: [[f32; 4]; 4],
    viewport_size: [f32; 2],
    point_size: f32,
    z_min: f32,
    z_max: f32,
    color_scale: f32,
    /// WGSL aligns the `vec4` array below to 16 bytes.
    _padding: [f32; 2],
    /// Points are only drawn inside all four, as `(normal, d)` with inside
    /// where `normal·p + d >= 0`. `(0, 0, 0, 1)` keeps everything.
    clip_planes: [[f32; 4]; 4],
    /// `(centre x, centre y, radius, on)`: when `on` is positive, points are
    /// only drawn inside this circle on the ground too.
    clip_circle: [f32; 4],
}

/// A clip plane every point is inside.
const NO_CLIP: Vec4 = Vec4::W;

/// The uniforms' clip planes and circle for `clip`.
fn clip_uniforms(clip: Option<Footprint>) -> ([[f32; 4]; 4], [f32; 4]) {
    let no_planes = [NO_CLIP.to_array(); 4];
    let no_circle = [0.0; 4];
    match clip {
        None => (no_planes, no_circle),
        Some(Footprint::Planes(planes)) => (planes.map(|plane| plane.to_array()), no_circle),
        Some(Footprint::Circle { centre, radius }) => {
            (no_planes, [centre.x, centre.y, radius, 1.0])
        }
    }
}

/// Must match `EdlUniforms` in `edl.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct EdlUniforms {
    near: f32,
    far: f32,
    strength: f32,
    radius_px: f32,
    /// A WGSL `u32` used as a bool, since `bool` can't go in a uniform.
    orthographic: u32,
    target_distance: f32,
    _padding: [f32; 2],
}

/// One end of a debug line segment. Must match `lines.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LineVertex {
    pub position: [f32; 3],
    /// Straight (not premultiplied) RGBA.
    pub color: [f32; 4],
}

/// The 12 edges of a box as 24 line-list vertices.
pub fn box_lines(min: Vec3, max: Vec3, color: [f32; 4]) -> impl Iterator<Item = LineVertex> {
    // Corner `i` takes max on the axes whose bit is set: 1 = X, 2 = Y, 4 = Z.
    let corner = move |i: usize| LineVertex {
        position: Vec3::select(
            glam::BVec3::new(i & 1 != 0, i & 2 != 0, i & 4 != 0),
            max,
            min,
        )
        .to_array(),
        color,
    };
    // An edge joins two corners that differ in exactly one bit.
    (0..8).flat_map(move |i| {
        [1, 2, 4]
            .into_iter()
            .filter(move |bit| i & bit == 0)
            .flat_map(move |bit| [corner(i), corner(i | bit)])
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PointColor {
    /// Viridis ramp over the cloud's height range.
    Height,
    /// The file's RGB. `scale` multiplies the unpacked 0..1 values: 1 for true
    /// 16-bit colour, 257 for files that store 0..255 in the 16-bit fields.
    Rgb { scale: f32 },
}

impl PointColor {
    /// The file's RGB, stretching 0..255 stored in 16 bits to the full range
    /// unless some value so far has needed more than 8 bits.
    pub fn rgb(is_16_bit: bool) -> Self {
        Self::Rgb {
            scale: if is_16_bit { 1.0 } else { 257.0 },
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct EdlSettings {
    /// How dark the shading gets. 1 matches Potree's default.
    pub strength: f32,
    /// How far away the neighbours are sampled, in physical pixels.
    pub radius_px: f32,
}

/// How to draw the points this frame.
pub struct PointStyle {
    /// Diameter in physical pixels.
    pub size_px: f32,
    pub color: PointColor,
    /// `None` turns eye-dome lighting off.
    pub edl: Option<EdlSettings>,
    /// Only draw points above this outline, such as a subcloud's.
    pub clip: Option<Footprint>,
}

/// An RGBA image read back from the GPU, rows top to bottom.
pub struct Image {
    pub width: u32,
    pub height: u32,
    /// 4 bytes per pixel, with no padding between rows.
    pub rgba: Vec<u8>,
}

pub struct PointRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// Shows render targets as egui textures. `None` without a window.
    egui_renderer: Option<Arc<RwLock<egui_wgpu::Renderer>>>,
    height_pipeline: wgpu::RenderPipeline,
    rgb_pipeline: wgpu::RenderPipeline,
    line_pipeline: wgpu::RenderPipeline,
    edl_pipeline: wgpu::RenderPipeline,
    /// Kept to build each viewport's bind group.
    bind_group_layout: wgpu::BindGroupLayout,
    /// The EDL bind group includes the depth texture, so it's rebuilt with
    /// the render target.
    edl_bind_group_layout: wgpu::BindGroupLayout,
    /// GPU copies of loaded octree nodes, drawn or cached for later.
    nodes: HashMap<VoxelKey, NodeBuffer>,
    /// Total points across `nodes`.
    resident_points: u64,
    lines: Option<LineBuffer>,
    /// Heights mapped to the ends of the colour ramp.
    z_range: [f32; 2],
    /// Counts `render` calls, to find the least recently drawn nodes.
    frame: u64,
}

/// One image to draw into, with its own uniforms and textures. Each view on
/// screen (the 3D view, the subcloud map) owns one, and they share the
/// renderer's pipelines and loaded nodes.
///
/// Separate uniform buffers matter: `queue.write_buffer` only lands at the
/// next submit, so two views sharing one buffer would rely on each `render`
/// submitting before the next one writes.
pub struct Viewport {
    uniform_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    edl_uniform_buffer: wgpu::Buffer,
    /// Created on the first `render`, and again when the size changes.
    target: Option<RenderTarget>,
    /// The target's colour texture as egui knows it. Kept across rebuilt
    /// targets, so the id egui is drawing stays valid.
    texture_id: Option<egui::TextureId>,
}

struct NodeBuffer {
    positions: wgpu::Buffer,
    /// `None` when the file has no colour.
    colors: Option<wgpu::Buffer>,
    count: u32,
    last_drawn: u64,
}

/// Debug line vertices, reused across frames and grown when needed.
struct LineBuffer {
    buffer: wgpu::Buffer,
    capacity: usize,
}

/// The textures we draw into, recreated whenever the viewport size changes.
struct RenderTarget {
    size: [u32; 2],
    /// Kept, beside its view, to copy from in `capture`.
    color_texture: wgpu::Texture,
    color: wgpu::TextureView,
    depth: wgpu::TextureView,
    edl_bind_group: wgpu::BindGroup,
    /// Whether `Viewport::texture_id` points at this target's colour yet.
    registered: bool,
}

impl PointRenderer {
    /// A renderer for the viewer window, whose images egui can show.
    pub fn new(render_state: &egui_wgpu::RenderState) -> Self {
        Self::with_device(
            render_state.device.clone(),
            render_state.queue.clone(),
            Some(render_state.renderer.clone()),
        )
    }

    /// A renderer without a window, which can only `capture`.
    pub fn headless(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        Self::with_device(device, queue, None)
    }

    fn with_device(
        device: wgpu::Device,
        queue: wgpu::Queue,
        egui_renderer: Option<Arc<RwLock<egui_wgpu::Renderer>>>,
    ) -> Self {
        let device = &device;

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bind_group_layout"),
            entries: &[uniform_layout_entry(0, wgpu::ShaderStages::VERTEX)],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pipeline_layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        // One position (and colour) per *instance*: every point reuses the
        // same 6 quad vertices, so the GPU steps these buffers per point.
        let position_attributes = wgpu::vertex_attr_array![0 => Float32x3];
        let position_buffer = wgpu::VertexBufferLayout {
            array_stride: size_of::<[f32; 3]>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &position_attributes,
        };
        let color_attributes = wgpu::vertex_attr_array![1 => Unorm16x4];
        let color_buffer = wgpu::VertexBufferLayout {
            array_stride: size_of::<[u16; 4]>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &color_attributes,
        };
        let point_depth = Some(DepthTest {
            write: true,
            compare: wgpu::CompareFunction::Less,
        });

        let points_shader = device.create_shader_module(wgpu::include_wgsl!("points.wgsl"));
        let height_pipeline = create_pipeline(
            device,
            PipelineConfig {
                label: "height_pipeline",
                layout: &layout,
                shader: &points_shader,
                vertex_entry: "vs_height",
                vertex_buffers: &[Some(position_buffer.clone())],
                topology: wgpu::PrimitiveTopology::TriangleList,
                depth: point_depth,
                blend: None,
            },
        );
        let rgb_pipeline = create_pipeline(
            device,
            PipelineConfig {
                label: "rgb_pipeline",
                layout: &layout,
                shader: &points_shader,
                vertex_entry: "vs_rgb",
                vertex_buffers: &[Some(position_buffer), Some(color_buffer)],
                topology: wgpu::PrimitiveTopology::TriangleList,
                depth: point_depth,
                blend: None,
            },
        );

        let line_attributes = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4];
        let line_pipeline = create_pipeline(
            device,
            PipelineConfig {
                label: "line_pipeline",
                layout: &layout,
                shader: &device.create_shader_module(wgpu::include_wgsl!("lines.wgsl")),
                vertex_entry: "vs_main",
                vertex_buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: size_of::<LineVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &line_attributes,
                })],
                topology: wgpu::PrimitiveTopology::LineList,
                // Hidden behind points, but they don't hide each other.
                depth: Some(DepthTest {
                    write: false,
                    compare: wgpu::CompareFunction::LessEqual,
                }),
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
            },
        );

        let edl_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("edl_bind_group_layout"),
                entries: &[
                    uniform_layout_entry(0, wgpu::ShaderStages::FRAGMENT),
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            // A depth format may be read as unfilterable float,
                            // which, unlike `Depth`, also works on the GL backend.
                            sample_type: wgpu::TextureSampleType::Float { filterable: false },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                ],
            });
        let edl_pipeline = create_pipeline(
            device,
            PipelineConfig {
                label: "edl_pipeline",
                layout: &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("edl_pipeline_layout"),
                    bind_group_layouts: &[Some(&edl_bind_group_layout)],
                    immediate_size: 0,
                }),
                shader: &device.create_shader_module(wgpu::include_wgsl!("edl.wgsl")),
                vertex_entry: "vs_main",
                vertex_buffers: &[],
                topology: wgpu::PrimitiveTopology::TriangleList,
                depth: None,
                blend: Some(MULTIPLY_BLEND),
            },
        );

        Self {
            device: device.clone(),
            queue,
            egui_renderer,
            height_pipeline,
            rgb_pipeline,
            line_pipeline,
            edl_pipeline,
            bind_group_layout,
            edl_bind_group_layout,
            nodes: HashMap::new(),
            resident_points: 0,
            lines: None,
            z_range: [0.0, 1.0],
            frame: 0,
        }
    }

    pub fn create_viewport(&self) -> Viewport {
        let device = &self.device;
        let uniform_buffer = create_uniform_buffer(device, "uniforms", size_of::<Uniforms>());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bind_group"),
            layout: &self.bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });
        Viewport {
            uniform_buffer,
            bind_group,
            edl_uniform_buffer: create_uniform_buffer(
                device,
                "edl_uniforms",
                size_of::<EdlUniforms>(),
            ),
            target: None,
            texture_id: None,
        }
    }

    /// The largest image `capture` can draw, each way.
    pub fn max_image_size(&self) -> u32 {
        self.device.limits().max_texture_dimension_2d
    }

    pub fn set_z_range(&mut self, z_range: [f32; 2]) {
        self.z_range = z_range;
    }

    pub fn has_node(&self, key: &VoxelKey) -> bool {
        self.nodes.contains_key(key)
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn resident_points(&self) -> u64 {
        self.resident_points
    }

    /// Uploads a node, replacing any previous copy. `colors`, if given, has one
    /// entry per position.
    pub fn insert_node(
        &mut self,
        key: VoxelKey,
        positions: &[[f32; 3]],
        colors: Option<&[[u16; 4]]>,
    ) {
        let device = &self.device;
        let upload = |label, contents| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents,
                usage: wgpu::BufferUsages::VERTEX,
            })
        };
        let node = NodeBuffer {
            positions: upload("node_positions", bytemuck::cast_slice(positions)),
            colors: colors.map(|colors| upload("node_colors", bytemuck::cast_slice(colors))),
            count: positions.len() as u32,
            // Count as just drawn, so a node isn't evicted before its first frame.
            last_drawn: self.frame,
        };
        self.resident_points += u64::from(node.count);
        if let Some(previous) = self.nodes.insert(key, node) {
            self.resident_points -= u64::from(previous.count);
        }
    }

    pub fn clear_nodes(&mut self) {
        self.nodes.clear();
        self.resident_points = 0;
    }

    /// Frees the least recently drawn nodes, except those in `keep`, until at most
    /// `max_points` are resident. Returns the evicted keys.
    pub fn evict_to_budget(&mut self, max_points: u64, keep: &HashSet<VoxelKey>) -> Vec<VoxelKey> {
        if self.resident_points <= max_points {
            return Vec::new();
        }
        let mut candidates: Vec<(u64, VoxelKey)> = self
            .nodes
            .iter()
            .filter(|(key, _)| !keep.contains(key))
            .map(|(key, node)| (node.last_drawn, *key))
            .collect();
        candidates.sort_unstable_by_key(|(last_drawn, _)| *last_drawn);

        let mut evicted = Vec::new();
        for (_, key) in candidates {
            if self.resident_points <= max_points {
                break;
            }
            if let Some(node) = self.nodes.remove(&key) {
                self.resident_points -= u64::from(node.count);
                evicted.push(key);
            }
        }
        evicted
    }

    /// Draws the `visible` nodes that are loaded, then `lines` on top, into
    /// `viewport` at `size` physical pixels. Returns the texture to show.
    ///
    /// Panics if the renderer was made with `headless`.
    pub fn render(
        &mut self,
        viewport: &mut Viewport,
        size: [u32; 2],
        camera: &OrbitCamera,
        style: &PointStyle,
        visible: &[VoxelKey],
        lines: &[LineVertex],
    ) -> egui::TextureId {
        self.draw(viewport, size, camera, style, visible, lines);
        self.register_texture(viewport)
    }

    /// Draws like `render`, without lines, then reads the image back, waiting
    /// for the GPU. Errors if `size` is bigger than `max_image_size`.
    pub fn capture(
        &mut self,
        viewport: &mut Viewport,
        size: [u32; 2],
        camera: &OrbitCamera,
        style: &PointStyle,
        visible: &[VoxelKey],
    ) -> Result<Image, String> {
        let max = self.max_image_size();
        if size.iter().any(|&side| side == 0 || side > max) {
            return Err(format!(
                "can't render {} × {} pixels: each side must be 1 to {max}",
                size[0], size[1]
            ));
        }
        self.draw(viewport, size, camera, style, visible, &[]);
        let target = viewport.target.as_ref().expect("draw creates the target");
        self.read_back(&target.color_texture, size)
    }

    /// Copies a colour texture into memory, dropping the row padding the copy needs.
    fn read_back(
        &self,
        texture: &wgpu::Texture,
        [width, height]: [u32; 2],
    ) -> Result<Image, String> {
        let row_bytes = width * 4;
        let padded_row_bytes = row_bytes.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("capture"),
            size: u64::from(padded_row_bytes) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("capture_encoder"),
            });
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row_bytes),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);

        let slice = buffer.slice(..);
        // The callback runs inside `poll`, which blocks until the copy is done.
        slice.map_async(wgpu::MapMode::Read, |_| {});
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| format!("the GPU failed while reading back the image: {e}"))?;
        let mapped = slice
            .get_mapped_range()
            .map_err(|e| format!("couldn't read back the image: {e}"))?;
        let rgba = mapped
            .chunks_exact(padded_row_bytes as usize)
            .flat_map(|row| &row[..row_bytes as usize])
            .copied()
            .collect();
        Ok(Image {
            width,
            height,
            rgba,
        })
    }

    /// Points `viewport.texture_id` at its target's colour texture, which
    /// changes whenever the target is rebuilt.
    fn register_texture(&self, viewport: &mut Viewport) -> egui::TextureId {
        let egui_renderer = self
            .egui_renderer
            .as_ref()
            .expect("a headless renderer can't show images in egui");
        let target = viewport.target.as_mut().expect("draw creates the target");
        if let Some(id) = viewport.texture_id
            && target.registered
        {
            return id;
        }
        // The texture maps 1:1 to screen pixels, so no filtering is needed.
        let mut egui_renderer = egui_renderer.write();
        let id = match viewport.texture_id {
            Some(id) => {
                egui_renderer.update_egui_texture_from_wgpu_texture(
                    &self.device,
                    &target.color,
                    wgpu::FilterMode::Nearest,
                    id,
                );
                id
            }
            None => egui_renderer.register_native_texture(
                &self.device,
                &target.color,
                wgpu::FilterMode::Nearest,
            ),
        };
        target.registered = true;
        viewport.texture_id = Some(id);
        id
    }

    /// Draws the `visible` nodes that are loaded, then `lines` on top, into
    /// `viewport`'s target, rebuilt at `size` if needed.
    fn draw(
        &mut self,
        viewport: &mut Viewport,
        size: [u32; 2],
        camera: &OrbitCamera,
        style: &PointStyle,
        visible: &[VoxelKey],
        lines: &[LineVertex],
    ) {
        let size = [size[0].max(1), size[1].max(1)];
        // Take the target out, reuse or rebuild it, then put it back below.
        let target = match viewport.target.take() {
            Some(target) if target.size == size => target,
            _ => self.create_target(size, &viewport.edl_uniform_buffer),
        };

        self.frame += 1;
        for key in visible {
            if let Some(node) = self.nodes.get_mut(key) {
                node.last_drawn = self.frame;
            }
        }
        self.upload_lines(lines);
        self.write_uniforms(viewport, size, camera, style);

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("point_encoder"),
            });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("point_pass"),
                color_attachments: &[Some(color_attachment(
                    &target.color,
                    wgpu::LoadOp::Clear(BACKGROUND),
                ))],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &target.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        // Kept for the EDL and line passes.
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &viewport.bind_group, &[]);
            let pipeline = match style.color {
                PointColor::Height => &self.height_pipeline,
                PointColor::Rgb { .. } => &self.rgb_pipeline,
            };
            pass.set_pipeline(pipeline);
            // One draw per node keeps nodes independent, so loading or evicting
            // one never touches the others' buffers.
            let drawable = visible
                .iter()
                .filter_map(|key| self.nodes.get(key))
                .filter(|node| node.count > 0);
            for node in drawable {
                pass.set_vertex_buffer(0, node.positions.slice(..));
                if let PointColor::Rgb { .. } = style.color {
                    // The RGB pipeline needs both buffers; a node without
                    // colour can't be drawn with it.
                    let Some(colors) = &node.colors else { continue };
                    pass.set_vertex_buffer(1, colors.slice(..));
                }
                pass.draw(0..VERTICES_PER_POINT, 0..node.count);
            }
        }

        if style.edl.is_some() {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("edl_pass"),
                color_attachments: &[Some(color_attachment(&target.color, wgpu::LoadOp::Load))],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.edl_pipeline);
            pass.set_bind_group(0, &target.edl_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        if let Some(line_buffer) = self.lines.as_ref().filter(|_| !lines.is_empty()) {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("line_pass"),
                color_attachments: &[Some(color_attachment(&target.color, wgpu::LoadOp::Load))],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &target.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &viewport.bind_group, &[]);
            pass.set_pipeline(&self.line_pipeline);
            pass.set_vertex_buffer(0, line_buffer.buffer.slice(..));
            pass.draw(0..lines.len() as u32, 0..1);
        }

        // egui submits its own work later on the same queue, so this draw is
        // guaranteed to finish before egui samples the texture.
        self.queue.submit([encoder.finish()]);
        viewport.target = Some(target);
    }

    fn write_uniforms(
        &self,
        viewport: &Viewport,
        size: [u32; 2],
        camera: &OrbitCamera,
        style: &PointStyle,
    ) {
        let aspect = size[0] as f32 / size[1] as f32;
        let (clip_planes, clip_circle) = clip_uniforms(style.clip);
        let uniforms = Uniforms {
            view_proj: camera.view_projection(aspect).to_cols_array_2d(),
            viewport_size: [size[0] as f32, size[1] as f32],
            point_size: style.size_px,
            z_min: self.z_range[0],
            z_max: self.z_range[1],
            color_scale: match style.color {
                PointColor::Height => 1.0,
                PointColor::Rgb { scale } => scale,
            },
            _padding: [0.0; 2],
            clip_planes,
            clip_circle,
        };
        let queue = &self.queue;
        queue.write_buffer(&viewport.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));

        if let Some(edl) = style.edl {
            let (near, far) = camera.clip_planes();
            let edl_uniforms = EdlUniforms {
                near,
                far,
                strength: edl.strength,
                radius_px: edl.radius_px,
                orthographic: u32::from(camera.projection() == Projection::Orthographic),
                target_distance: camera.distance,
                _padding: [0.0; 2],
            };
            queue.write_buffer(
                &viewport.edl_uniform_buffer,
                0,
                bytemuck::bytes_of(&edl_uniforms),
            );
        }
    }

    fn upload_lines(&mut self, lines: &[LineVertex]) {
        if lines.is_empty() {
            return;
        }
        if self.lines.as_ref().is_none_or(|l| l.capacity < lines.len()) {
            // Doubling avoids reallocating every frame while the count creeps up.
            let capacity = lines.len().next_power_of_two();
            let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("debug_lines"),
                size: (capacity * size_of::<LineVertex>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.lines = Some(LineBuffer { buffer, capacity });
        }
        if let Some(line_buffer) = &self.lines {
            self.queue
                .write_buffer(&line_buffer.buffer, 0, bytemuck::cast_slice(lines));
        }
    }

    fn create_target(&self, size: [u32; 2], edl_uniforms: &wgpu::Buffer) -> RenderTarget {
        let device = &self.device;
        let extent = wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        };
        let make_texture = |label, format, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        // TEXTURE_BINDING so egui can show it, COPY_SRC so `capture` can read it.
        let color_texture = make_texture(
            "point_color",
            COLOR_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
        );
        let color = color_texture.create_view(&wgpu::TextureViewDescriptor::default());
        // TEXTURE_BINDING so the EDL pass can read it.
        let depth = make_texture(
            "point_depth",
            DEPTH_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        )
        .create_view(&wgpu::TextureViewDescriptor::default());
        let edl_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("edl_bind_group"),
            layout: &self.edl_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: edl_uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&depth),
                },
            ],
        });

        RenderTarget {
            size,
            color_texture,
            color,
            depth,
            edl_bind_group,
            registered: false,
        }
    }
}

/// `result = destination × source`: the EDL pass outputs a shade factor that
/// darkens what's already drawn, leaving alpha alone.
const MULTIPLY_BLEND: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::Zero,
        dst_factor: wgpu::BlendFactor::Src,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::Zero,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    },
};

fn create_uniform_buffer(device: &wgpu::Device, label: &str, size: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: size as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn uniform_layout_entry(
    binding: u32,
    visibility: wgpu::ShaderStages,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn color_attachment(
    view: &wgpu::TextureView,
    load: wgpu::LoadOp<wgpu::Color>,
) -> wgpu::RenderPassColorAttachment<'_> {
    wgpu::RenderPassColorAttachment {
        view,
        resolve_target: None,
        ops: wgpu::Operations {
            load,
            store: wgpu::StoreOp::Store,
        },
        depth_slice: None,
    }
}

#[derive(Clone, Copy)]
struct DepthTest {
    write: bool,
    compare: wgpu::CompareFunction,
}

/// The parts that differ between pipelines.
struct PipelineConfig<'a> {
    label: &'static str,
    layout: &'a wgpu::PipelineLayout,
    shader: &'a wgpu::ShaderModule,
    vertex_entry: &'static str,
    vertex_buffers: &'a [Option<wgpu::VertexBufferLayout<'a>>],
    topology: wgpu::PrimitiveTopology,
    /// `None` for passes without a depth attachment.
    depth: Option<DepthTest>,
    blend: Option<wgpu::BlendState>,
}

fn create_pipeline(device: &wgpu::Device, config: PipelineConfig<'_>) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(config.label),
        layout: Some(config.layout),
        vertex: wgpu::VertexState {
            module: config.shader,
            entry_point: Some(config.vertex_entry),
            buffers: config.vertex_buffers,
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        primitive: wgpu::PrimitiveState {
            topology: config.topology,
            ..wgpu::PrimitiveState::default()
        },
        depth_stencil: config.depth.map(|depth| wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(depth.write),
            depth_compare: Some(depth.compare),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: config.shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: COLOR_FORMAT,
                blend: config.blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        multiview_mask: None,
        cache: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_lines_should_draw_twelve_axis_aligned_edges() {
        let lines: Vec<_> = box_lines(Vec3::ZERO, Vec3::ONE, [1.0; 4]).collect();
        let axis_aligned = lines.chunks(2).all(|edge| {
            let delta = Vec3::from(edge[1].position) - Vec3::from(edge[0].position);
            delta.length() == 1.0
        });
        assert!(lines.len() == 24 && axis_aligned);
    }
}
