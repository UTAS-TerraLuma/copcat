# copcat

A native viewer for Cloud-Optimized Point Clouds (Rust 2024, eframe/egui 0.36,
wgpu). `src/lib.rs` is the library; `copcat-viewer` is the window and
`copcat` the command line (`copcat render` draws a PNG without a window).

## Commands

- Build: `cargo build`
- Format: `cargo fmt` (CI runs `cargo fmt --check`)
- Lint: `cargo clippy --all-targets --locked -- -D warnings`
- Test: `cargo test --locked` (unit tests, plus `tests/`: file/HTTP loading,
  golden renders, an egui_kittest UI test)
- Render the fixture: `cargo run --bin copcat -- render tests/data/fixture.copc.laz -o <scratch>/out.png`
- Update goldens: `COPCAT_UPDATE_GOLDENS=1 cargo test --test golden_render`;
  UI snapshots: `UPDATE_SNAPSHOTS=1 cargo test --test ui_kittest`

GPU tests skip (printing `SKIPPED:`) without a GPU adapter, unless
`COPCAT_REQUIRE_GPU=1`. `WGPU_ADAPTER_NAME=<part of a name>` picks the adapter
for `copcat render`, e.g. `basic render` for WARP or `llvmpipe` for lavapipe.

Test data: `tests/data/fixture.copc.laz` is synthetic; see
`tests/data/README.md` for what is in it and how it was made.

## Verifying a change

Follow `.claude/skills/verify/SKILL.md` before calling work done: fmt, clippy,
tests, then render the fixture and look at the PNG.

- Never add sleeps or retries, or loosen tolerances, to make tests pass.
- Golden images, snapshots and tolerances change only with a stated reason in
  the commit, after looking at every changed image.
- After three failed rounds of fixing, stop and escalate to the human.
