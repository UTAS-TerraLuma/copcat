# Handoff: level-of-detail (LOD) streaming

## Context
A COPC point cloud viewer in Rust (eframe/egui 0.36 + wgpu 30). The goal is to view
high-density drone scans of vegetation sites, and later draw transects and classify
points. I'm learning Rust (I come from React, Three.js and some WebGPU), so explain the
Rust ideas as you go and follow the `rust-best-practices` skill.

**Current state:** loads the header and **only the root octree node** (level 0) on a
background thread, then renders it as instanced screen-space circles coloured by height.
It has an orbit camera and a floating egui window with the cloud info.

## Code map
- `src/cloud.rs`: `Location::parse` (http/file URL/path), `load_root_node()`, `PointCloud`.
  Uses `copc-streaming` 0.2 (async) driven by `pollster::block_on`. There's no tokio.
- `src/http_source.rs`: `HttpSource`, a `ByteSource` built on ureq range requests.
- `src/app.rs`: `ViewerApp` with a `LoadState` enum (`Idle/Loading/Loaded/Failed`).
  A worker thread sends its result over an `mpsc` channel, `poll_loading()` checks it
  each frame, and the worker calls `ctx.request_repaint()` when done.
- `src/renderer.rs`: `PointRenderer` renders into an **offscreen** Rgba8Unorm texture with
  its own Depth32Float, registered as an egui image. `set_points(&[[f32;3]], z_range)`
  uploads one buffer, and `render()` issues a single instanced draw.
- `src/camera.rs`: Z-up `OrbitCamera` (`view_projection`, `frame_bounds`, orbit/pan/zoom).
- `src/points.wgsl`: 6 vertices per instance, size in pixels, circle via discard, viridis.

## Gotchas already hit
- **egui 0.36:** it's `App::ui(&mut self, ui, frame)`, not `update(ctx)`, and
  `egui::Panel::top/left`, not `TopBottomPanel`. Panels use `.show(ui, ..)`.
- **glam 0.34:** `Mat4::look_at_rh`/`perspective_rh` are gone. Use
  `glam::camera::rh::view::look_at_mat4` and `rh::proj::directx::perspective`
  (directx = wgpu's 0..1 depth).
- **wgpu 30:** descriptors take `Some(..)` for bind group layouts and vertex buffers,
  `depth_write_enabled: Some(true)`, `immediate_size`, `multiview_mask`.
- **Precision:** positions are `f32` **relative to the COPC octree centre** (`origin`),
  because UTM coordinates in `f32` only resolve about 6 cm. Keep this for every node.
- **Always check the real API** in `~/.cargo/registry/src/*/<crate>-<ver>/` before writing
  code. These crates change quickly.

## Task: stream finer octree levels as the camera moves
Suggested design (open to better ideas):
1. **Persistent loader worker.** Keep one `CopcStreamingReader` alive on a worker thread
   instead of loading once. The UI sends it the "wanted node keys" and it sends back
   decoded chunks (positions relative to `origin`). It should drop requests that are no
   longer wanted, because the camera moves faster than the network.
2. **Hierarchy.** Start simple with `load_all_hierarchy()` at open (each entry is 32 bytes,
   so it's small). Lazy loading (`load_hierarchy_for_bounds_to_level` / pending pages)
   can come later.
3. **Node selection: a pure, unit-testable function.** Inputs: camera, viewport,
   hierarchy and point budget. Output: the visible keys. Traverse from the root; cull with
   `VoxelKey::bounds(&root_bounds)` against the view frustum; refine a node while its
   projected point spacing (`copc_info.spacing / 2^level`) exceeds a pixel threshold;
   stop at the point budget (default about 5–10 M points).
4. **GPU side.** One vertex buffer per node (`HashMap<VoxelKey, NodeBuffer>`), a draw call
   per visible node, and an LRU eviction when GPU memory is over budget. Keep parents
   drawn until their children arrive, so there are no holes.
5. **UI.** Show nodes loaded, points drawn and pending requests in the floating window,
   plus sliders for point budget and detail threshold.

## Test data
- Autzen (US S3, 10.6 M points, 61,201 in the root node):
  `https://s3.amazonaws.com/hobu-lidar/autzen-classified.copc.laz`
- My drone data (Metashape, AU object store): see the ignored test in `src/cloud.rs`.

## Workflow notes
- `cargo run -- <url-or-path>` auto-loads on startup.
- `cargo test`, and `cargo test -- --ignored` for the network tests.
  Keep `cargo clippy --all-targets -- -D warnings` clean.
- Visual checks: launch the exe, then screenshot its window with PowerShell
  (`System.Drawing` `CopyFromScreen` plus `GetWindowRect`, after `SetProcessDPIAware`).
  **I sometimes interact with windows that pop up**, so unexpected camera changes in a
  screenshot may be me.

## Done when
Zooming into my drone site shows full density close up, the frame rate stays smooth, the
UI never blocks on IO, and node selection has unit tests.
