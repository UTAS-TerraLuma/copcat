//! Renders point cloud nodes into an offscreen texture that egui displays as an image.
//!
//! Each frame runs up to three passes into the same colour texture:
//! 1. points (colour + depth),
//! 2. eye-dome lighting, which reads that depth and darkens the colour,
//! 3. debug lines, depth-tested against the points but not shaded by EDL.

use std::collections::{HashMap, HashSet};

use copc_streaming::VoxelKey;
use eframe::egui;
use eframe::egui_wgpu::{self, wgpu};
use glam::Vec3;
use wgpu::util::DeviceExt;

use crate::camera::OrbitCamera;

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
    /// WGSL rounds uniform structs up to a multiple of 16 bytes.
    _padding: [f32; 2],
}

/// Must match `EdlUniforms` in `edl.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct EdlUniforms {
    near: f32,
    far: f32,
    strength: f32,
    radius_px: f32,
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
}

pub struct PointRenderer {
    render_state: egui_wgpu::RenderState,
    height_pipeline: wgpu::RenderPipeline,
    rgb_pipeline: wgpu::RenderPipeline,
    line_pipeline: wgpu::RenderPipeline,
    edl_pipeline: wgpu::RenderPipeline,
    uniform_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    edl_uniform_buffer: wgpu::Buffer,
    /// The EDL bind group includes the depth texture, so it's rebuilt with
    /// the render target; this layout is kept to do that.
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
    target: Option<RenderTarget>,
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
    color: wgpu::TextureView,
    depth: wgpu::TextureView,
    edl_bind_group: wgpu::BindGroup,
    texture_id: egui::TextureId,
}

impl PointRenderer {
    pub fn new(render_state: &egui_wgpu::RenderState) -> Self {
        let device = &render_state.device;

        let uniform_buffer = create_uniform_buffer(device, "uniforms", size_of::<Uniforms>());
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bind_group_layout"),
            entries: &[uniform_layout_entry(0, wgpu::ShaderStages::VERTEX)],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bind_group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
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

        let edl_uniform_buffer =
            create_uniform_buffer(device, "edl_uniforms", size_of::<EdlUniforms>());
        let edl_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("edl_bind_group_layout"),
                entries: &[
                    uniform_layout_entry(0, wgpu::ShaderStages::FRAGMENT),
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Depth,
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
            render_state: render_state.clone(),
            height_pipeline,
            rgb_pipeline,
            line_pipeline,
            edl_pipeline,
            uniform_buffer,
            bind_group,
            edl_uniform_buffer,
            edl_bind_group_layout,
            nodes: HashMap::new(),
            resident_points: 0,
            lines: None,
            z_range: [0.0, 1.0],
            frame: 0,
            target: None,
        }
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
        let device = &self.render_state.device;
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

    /// Draws the `visible` nodes that are loaded, then `lines` on top, at `size`
    /// physical pixels. Returns the texture to show.
    pub fn render(
        &mut self,
        size: [u32; 2],
        camera: &OrbitCamera,
        style: &PointStyle,
        visible: &[VoxelKey],
        lines: &[LineVertex],
    ) -> egui::TextureId {
        let size = [size[0].max(1), size[1].max(1)];
        // Take the target out, reuse or rebuild it, then put it back below.
        let target = match self.target.take() {
            Some(target) if target.size == size => target,
            previous => self.create_target(size, previous.map(|t| t.texture_id)),
        };

        self.frame += 1;
        for key in visible {
            if let Some(node) = self.nodes.get_mut(key) {
                node.last_drawn = self.frame;
            }
        }
        self.upload_lines(lines);
        self.write_uniforms(size, camera, style);

        let mut encoder =
            self.render_state
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
            pass.set_bind_group(0, &self.bind_group, &[]);
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
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.set_pipeline(&self.line_pipeline);
            pass.set_vertex_buffer(0, line_buffer.buffer.slice(..));
            pass.draw(0..lines.len() as u32, 0..1);
        }

        // egui submits its own work later on the same queue, so this draw is
        // guaranteed to finish before egui samples the texture.
        self.render_state.queue.submit([encoder.finish()]);

        let texture_id = target.texture_id;
        self.target = Some(target);
        texture_id
    }

    fn write_uniforms(&self, size: [u32; 2], camera: &OrbitCamera, style: &PointStyle) {
        let aspect = size[0] as f32 / size[1] as f32;
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
        };
        let queue = &self.render_state.queue;
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));

        if let Some(edl) = style.edl {
            let (near, far) = camera.clip_planes();
            let edl_uniforms = EdlUniforms {
                near,
                far,
                strength: edl.strength,
                radius_px: edl.radius_px,
            };
            queue.write_buffer(&self.edl_uniform_buffer, 0, bytemuck::bytes_of(&edl_uniforms));
        }
    }

    fn upload_lines(&mut self, lines: &[LineVertex]) {
        if lines.is_empty() {
            return;
        }
        if self.lines.as_ref().is_none_or(|l| l.capacity < lines.len()) {
            // Doubling avoids reallocating every frame while the count creeps up.
            let capacity = lines.len().next_power_of_two();
            let buffer = self.render_state.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("debug_lines"),
                size: (capacity * size_of::<LineVertex>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.lines = Some(LineBuffer { buffer, capacity });
        }
        if let Some(line_buffer) = &self.lines {
            self.render_state
                .queue
                .write_buffer(&line_buffer.buffer, 0, bytemuck::cast_slice(lines));
        }
    }

    fn create_target(&self, size: [u32; 2], reuse_id: Option<egui::TextureId>) -> RenderTarget {
        let device = &self.render_state.device;
        let extent = wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        };
        let make_texture = |label, format, usage| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: extent,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        let color = make_texture(
            "point_color",
            COLOR_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        // TEXTURE_BINDING so the EDL pass can read it.
        let depth = make_texture(
            "point_depth",
            DEPTH_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let edl_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("edl_bind_group"),
            layout: &self.edl_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.edl_uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&depth),
                },
            ],
        });

        // The texture maps 1:1 to screen pixels, so no filtering is needed.
        let mut egui_renderer = self.render_state.renderer.write();
        let texture_id = match reuse_id {
            Some(id) => {
                egui_renderer.update_egui_texture_from_wgpu_texture(
                    device,
                    &color,
                    wgpu::FilterMode::Nearest,
                    id,
                );
                id
            }
            None => egui_renderer.register_native_texture(device, &color, wgpu::FilterMode::Nearest),
        };

        RenderTarget {
            size,
            color,
            depth,
            edl_bind_group,
            texture_id,
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

fn uniform_layout_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
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
