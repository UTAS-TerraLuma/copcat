//! Renders points into an offscreen texture that egui displays as an image.

use eframe::egui;
use eframe::egui_wgpu::{self, wgpu};
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
    /// WGSL rounds uniform structs up to a multiple of 16 bytes.
    _padding: [f32; 3],
}

pub struct PointRenderer {
    render_state: egui_wgpu::RenderState,
    pipeline: wgpu::RenderPipeline,
    uniform_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    points: Option<PointBuffer>,
    target: Option<RenderTarget>,
}

struct PointBuffer {
    buffer: wgpu::Buffer,
    count: u32,
    z_range: [f32; 2],
}

/// The textures we draw into, recreated whenever the viewport size changes.
struct RenderTarget {
    size: [u32; 2],
    color: wgpu::TextureView,
    depth: wgpu::TextureView,
    texture_id: egui::TextureId,
}

impl PointRenderer {
    pub fn new(render_state: &egui_wgpu::RenderState) -> Self {
        let device = &render_state.device;

        let shader = device.create_shader_module(wgpu::include_wgsl!("points.wgsl"));

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("point_uniforms"),
            size: size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("point_bind_group_layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("point_bind_group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("point_pipeline_layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("point_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                // One [f32; 3] position per *instance*: every point reuses the
                // same 6 quad vertices, so the GPU steps this buffer per point.
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: size_of::<[f32; 3]>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3],
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: COLOR_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview_mask: None,
            cache: None,
        });

        Self {
            render_state: render_state.clone(),
            pipeline,
            uniform_buffer,
            bind_group,
            points: None,
            target: None,
        }
    }

    /// Uploads point positions to the GPU, replacing any previous points.
    pub fn set_points(&mut self, positions: &[[f32; 3]], z_range: [f32; 2]) {
        if positions.is_empty() {
            self.points = None;
            return;
        }
        let buffer = self
            .render_state
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("point_positions"),
                contents: bytemuck::cast_slice(positions),
                usage: wgpu::BufferUsages::VERTEX,
            });
        self.points = Some(PointBuffer {
            buffer,
            count: positions.len() as u32,
            z_range,
        });
    }

    /// Draws the points at `size` physical pixels and returns the texture to show.
    pub fn render(
        &mut self,
        size: [u32; 2],
        camera: &OrbitCamera,
        point_size: f32,
    ) -> egui::TextureId {
        let size = [size[0].max(1), size[1].max(1)];
        // Take the target out, reuse or rebuild it, then put it back below.
        let target = match self.target.take() {
            Some(target) if target.size == size => target,
            previous => self.create_target(size, previous.map(|t| t.texture_id)),
        };

        let aspect = size[0] as f32 / size[1] as f32;
        let z_range = self.points.as_ref().map_or([0.0, 1.0], |p| p.z_range);
        let uniforms = Uniforms {
            view_proj: camera.view_projection(aspect).to_cols_array_2d(),
            viewport_size: [size[0] as f32, size[1] as f32],
            point_size,
            z_min: z_range[0],
            z_max: z_range[1],
            _padding: [0.0; 3],
        };
        let queue = &self.render_state.queue;
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));

        let mut encoder =
            self.render_state
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("point_encoder"),
                });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("point_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.color,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(BACKGROUND),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &target.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });
            if let Some(points) = &self.points {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.bind_group, &[]);
                pass.set_vertex_buffer(0, points.buffer.slice(..));
                pass.draw(0..VERTICES_PER_POINT, 0..points.count);
            }
        }
        // egui submits its own work later on the same queue, so this draw is
        // guaranteed to finish before egui samples the texture.
        queue.submit([encoder.finish()]);

        let texture_id = target.texture_id;
        self.target = Some(target);
        texture_id
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
        let depth = make_texture(
            "point_depth",
            DEPTH_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        );

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
            texture_id,
        }
    }
}
