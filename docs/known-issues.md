# Known issues

## Access violation with several WARP devices in one process

**Status:** open, worked around in the tests. Investigated 2026-11-18; the
investigation was stopped before a fix, so the next steps below are untried.

### What happens

A process that uses several Direct3D 12 devices on the WARP software adapter
at the same time, from different threads, sometimes dies with
`STATUS_ACCESS_VIOLATION` (`0xc0000005`). The fault is inside `d3d10warp.dll`.

In copcat this shows up in the UI tests: each egui_kittest harness makes its
own wgpu device, egui_kittest prefers a software adapter (WARP on Windows),
and tests in one binary run on parallel threads.

### Workaround in place

Every UI test file with more than one test (`tests/ui_connectors.rs`,
`ui_palette.rs`, `ui_settings.rs`) holds a static `ONE_VIEWER_AT_A_TIME` mutex
while its viewer is alive, so only one device is in use at a time. With
it, `tests/ui_settings.rs` ran 40 times without a crash, and the full suite 5
times.

### What was measured

Debug builds, Windows 11, `d3d10warp.dll` 10.0.26100.9711, D3D12Core
10.0.26100.9549, wgpu 30.0.1. "Unlocked" means the mutex was bypassed.

| Configuration | Runs | Crashes |
|---|---|---|
| `ui_settings` unlocked, default backends (WARP, DX12) | 100 | 10 |
| `ui_settings` unlocked, `WGPU_BACKEND=dx12` (still WARP) | 60 | 9 |
| `ui_settings` unlocked, `WGPU_BACKEND=vulkan` (no WARP) | 60 | 0 (3 runs had ordinary test failures, cause not captured) |
| `warp_repro`, 5 threads, separate WARP devices | 30 | 1 |
| `warp_repro` under cdb, 8 threads, separate WARP devices | 10 | 10 |
| `warp_repro` under cdb, 8 threads, one shared WARP device | 25 | 0 |
| `warp_repro` under cdb, 8 threads, separate NVIDIA DX12 devices | 25 | 0 |

`tests/warp_repro.rs` is a reproducer with no copcat or egui code, only wgpu.
Running under a debugger raises the crash rate a lot.

### Stacks

From cdb, without Microsoft symbols, so system DLLs show export offsets. Two
families, seen in both the copcat tests and the pure-wgpu reproducer.

On a WARP thread-pool worker, with no copcat or wgpu frames:

```
d3d10warp!Ordinal199+0x8c763   cmp dword ptr [r9+rcx*4],0FFFFFFFFh  (rcx=ffffffff)
d3d10warp!Ordinal199+0x8c061 / +0x8aa8e / +0x8a6b5 / +0x8a4f1
d3d10warp!OpenAdapter10_2+0x34a63
d3d10warp!Ordinal199+0x79138 / +0x78e68
ntdll!TpCallbackMayRunLong+0x5f0 ... ntdll!RtlUserThreadStart
```

On the submitting thread, during ExecuteCommandLists:

```
d3d10warp!Ordinal199+0x5f1bd (also +0x5f209, +0x5f30f, +0x5f485, +0x5f715)
d3d10warp!Ordinal199+0x5f0fb
d3d10warp!OpenAdapter10_2+0x7273a ... d3d10warp!OpenAdapter10_2+0x428b8
D3D12Core+0x11678 / D3D12Core+0x1115d
wgpu_hal::dx12 ... submit
wgpu_core::device::queue::Queue::submit
copcat::renderer::PointRenderer::draw
```

### Conclusions, and how sure they are

- **Not copcat's code** (high confidence): the pure-wgpu reproducer crashes in
  the same functions. That also rules out the loader thread, teardown order
  and the per-node uniform buffer as the cause.
- **Needs several separate WARP devices** (high confidence): one shared WARP
  device never crashed, nor did separate NVIDIA devices.
- **A bug in WARP itself, rather than wgpu misusing D3D12 in a way only WARP
  trips over** (medium confidence): no plain D3D12 reproducer has been written
  to rule wgpu out.
- Teardown happening on another thread may raise the rate (9 of the 10 base
  crashes came just after a test finished), but one crash came before any test
  had finished, so it isn't required.

### Not yet known

- **Whether the `copcat render` exit crash is the same problem.** Two separate
  runs of a single `copcat render` on the fixture crashed with the same status
  after writing the image. That path has one device, not on WARP by default,
  and it wasn't reproduced in about 15 more runs. It was not investigated.
- **Whether `tests/golden_render.rs` is exposed on CI.** Its tests each make a
  device in parallel, and Windows CI selects WARP (`WGPU_ADAPTER_NAME`). It
  has not been looped on WARP.
- Release builds and other test thread counts.

### Reproducing

```sh
cargo test --locked --test warp_repro --no-run
# Separate WARP devices (crashes sometimes; nearly always under a debugger):
WGPU_BACKEND=dx12 REPRO_THREADS=8 target/debug/deps/warp_repro-*.exe --ignored
# One shared device (doesn't crash):
WGPU_BACKEND=dx12 REPRO_THREADS=8 REPRO_SHARED=1 target/debug/deps/warp_repro-*.exe --ignored
# A stack, with cdb from the Windows SDK:
cdb -g -G -c "sxd av; g; .lastevent; r; kn 60; ~*kn 40; q" <exe> --ignored
```

For the copcat tests, give each test in `tests/ui_settings.rs` its own mutex
instead of `ONE_VIEWER_AT_A_TIME`, build, and loop
`target/debug/deps/ui_settings-*.exe` with `COPCAT_REQUIRE_GPU=1`. Expect about
1 crash in 10 runs.

### Next steps

1. **Share one device between harnesses** in a test binary, in place of the
   mutex: build a render state once in `tests/common` and pass it to each
   harness (`WgpuSetup::Existing` through `.wgpu_setup(...)`). Then remove the
   locks and loop 100+ unlocked runs; at the 10% base rate, no crash in 100
   runs would happen by luck about 0.003% of the time.
2. **Loop `golden_render` on WARP** (`WGPU_ADAPTER_NAME="basic render"`) to
   see whether CI is exposed, and serialise or share a device there if so.
3. **Loop `copcat render`** a few hundred times on the default adapter and on
   WARP, under cdb, to find out whether the exit crash is a separate problem.
4. **Rule out wgpu** with a D3D12-only reproducer (C++ or windows-rs), then
   report it to wgpu and to Microsoft with the stacks and versions above.
   Microsoft's symbol server would give real function names.
5. Find out why 3 of the 60 Vulkan runs had test failures.
