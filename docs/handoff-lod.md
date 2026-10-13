# Handoff: level-of-detail (LOD) streaming

## Context
A COPC point cloud viewer in Rust (eframe/egui 0.36 + wgpu 30). The goal is to view
high-density drone scans of vegetation sites, and later draw transects and classify
points. I'm learning Rust (I come from React, Three.js and some WebGPU), so explain the
Rust ideas as you go and follow the `rust-best-practices` skill.

**Current state:** LOD streaming works. A loader thread opens the file, loads the whole
hierarchy, then streams the nodes the camera needs. Each frame the UI picks nodes with a
budgeted, frustum-culled octree traversal, draws one buffer per node and evicts the least
recently drawn nodes when the GPU cache goes over budget. "Show node bounds" draws each
node's cube, clipped to the cloud's bounding box and coloured by status. Points are
coloured by RGB when the file has it (else a height ramp), with optional eye-dome lighting.

## Code map
- `src/cloud.rs`: `Location::parse` (http/file URL/path), `CloudInfo` (header summary,
  `origin`, `local_bounds`), `to_local()` (f64 world → f32 relative to origin), `LoadError`.
- `src/loader.rs`: `Loader` handle around a worker thread that owns the
  `CopcStreamingReader`. UI → worker: `want(keys)` (replaces the queue, priority order),
  `evicted(key)`. Worker → UI: `Event::{Opened, Fetching, Node, Failed}`. Fetches
  `BATCH_SIZE` (8) nodes per `read_ranges` call and decodes them in parallel into
  `NodeData` (positions, plus `[u16; 4]` RGB when the point format has colour).
  `FetchQueue` skips nodes already delivered, so a request racing a delivery isn't refetched.
- `src/lod.rs`: pure node selection. `Octree` (point counts, node bounds clipped to the
  data box), `View` (frustum + eye + projection scale), `LodSettings` (point budget,
  target spacing in px), `select_nodes()` → `Selection` (parents always before children).
- `src/http_source.rs`: `HttpSource`, a `ByteSource` built on ureq range requests,
  parallel across `MAX_PARALLEL_REQUESTS` threads.
- `src/app.rs`: `ViewerApp` with `LoadState::{Idle, Opening, Open(Session), Failed}`.
  `Session::receive` uploads arriving nodes, and `Session::update` selects, requests the
  missing nodes and evicts (cache = 3× point budget).
- `src/renderer.rs`: `PointRenderer` renders into an **offscreen** Rgba8Unorm texture with
  its own Depth32Float, registered as an egui image. `HashMap<VoxelKey, NodeBuffer>` with
  an LRU (`evict_to_budget`). `render` takes a `PointStyle` (size, `PointColor::{Height, Rgb}`,
  optional `EdlSettings`) and runs three passes: points (one instanced draw per node, height
  or RGB pipeline), EDL (full-screen, reads the depth texture, multiplies a shade into the
  colour), then debug lines (`box_lines`), depth-tested but not shaded.
- `src/camera.rs`: Z-up `OrbitCamera` (`view_projection`, `frame_bounds`, orbit/pan/zoom).
- `src/points.wgsl`: 6 vertices per instance, size in pixels, circle via discard;
  `vs_height` (viridis) or `vs_rgb` (Unorm16x4 colour × `color_scale`).
- `src/lines.wgsl`: coloured line list for debug overlays.
- `src/edl.wgsl`: eye-dome lighting (Potree-style, 8 neighbours, log2 of linearised depth).
  Background neighbours add a fixed response, which draws silhouettes.

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
- **RGB bit depth:** LAS RGB should be 16-bit, but some writers store 0..255. `Session`
  flags `rgb_is_16_bit` once any value exceeds 255, else the shader scales by 257.
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

## Status (LOD streaming done)
Checked on the drone site: it opens in 0.6 s (11,618 nodes), shows 343 k points at the
overview and 3.5 M points close up. 24 unit tests pass, plus the 2 network tests.

Ideas for next:
- Lazy hierarchy loading (`load_hierarchy_for_bounds_to_level`) for very large files.
- Failed nodes are never retried; add a retry for transient network errors.
- Point size could grow with node depth to hide the gaps at coarse levels.

## Status (RGB + eye-dome lighting done)
RGB is the default colour mode, falling back to height for files without colour. EDL is on
by default (strength 1, radius 1.4 logical px). On the drone site it outlines shrubs well.
Sparse areas get dark speckles where background shows between points. Larger points or
a lower target spacing reduce that.
