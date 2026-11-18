//! A minimal reproducer for the WARP crash in docs/known-issues.md, with no
//! copcat or egui code: several threads, each with its own wgpu instance and
//! device on the same adapter, drawing at once. Ignored by default; run it
//! with the commands in that document.
//!
//! - `REPRO_THREADS`: how many threads.
//! - `REPRO_SHARED=1`: one device shared by every thread (doesn't crash).
//! - `REPRO_ADAPTER`: pick the adapter by name (default: the CPU one, WARP).
use eframe::egui_wgpu::wgpu;

const SHADER: &str = r#"
@vertex fn vs(@builtin(vertex_index) v: u32, @builtin(instance_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32(v & 1u) - 0.5 + f32(i % 17u) * 0.01;
    let y = f32(v >> 1u) - 0.5 + f32(i % 13u) * 0.01;
    return vec4<f32>(x, y, 0.5, 1.0);
}
@fragment fn fs() -> @location(0) vec4<f32> { return vec4<f32>(1.0, 0.5, 0.2, 1.0); }
"#;

fn device() -> (wgpu::Device, wgpu::Queue) {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
    let wanted = std::env::var("REPRO_ADAPTER")
        .ok()
        .map(|s| s.to_lowercase());
    let adapter = adapters
        .into_iter()
        .find(|a| match &wanted {
            Some(w) => a.get_info().name.to_lowercase().contains(w),
            None => a.get_info().device_type == wgpu::DeviceType::Cpu,
        })
        .expect("no adapter");
    eprintln!("adapter: {:?}", adapter.get_info());
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap()
}

fn work(frames: u32, shared: Option<(wgpu::Device, wgpu::Queue)>) {
    let (device, queue) = shared.unwrap_or_else(device);
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: None,
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: None,
        layout: None,
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleStrip,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(format.into())],
        }),
        multiview_mask: None,
        cache: None,
    });
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: 960,
            height: 700,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    for _ in 0..frames {
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&pipeline);
            pass.draw(0..4, 0..2000);
        }
        queue.submit([encoder.finish()]);
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    }
}

#[test]
#[ignore = "a reproducer for a crash, see docs/known-issues.md"]
fn parallel_devices() {
    let threads: usize = std::env::var("REPRO_THREADS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);
    let shared = std::env::var("REPRO_SHARED").is_ok();
    for _round in 0..3 {
        let one = shared.then(device);
        let handles: Vec<_> = (0..threads)
            .map(|i| {
                let one = one.clone();
                std::thread::spawn(move || work(100 + 37 * i as u32, one))
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
    }
}
