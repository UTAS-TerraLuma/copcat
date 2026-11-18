---
name: verify
description: Verify a change to copcat before calling it done - format, lint, unit and integration tests, golden render and UI snapshot tests, then render the test fixture to a PNG and look at it. Use after any code change, before committing, and whenever asked to check, test or verify copcat.
---

# Verify a copcat change

Run these in order from the repository root. Stop at the first failure, fix
it, and start again from step 1.

1. `cargo fmt --check` (if it fails, run `cargo fmt` and re-check)
2. `cargo clippy --all-targets --locked -- -D warnings`
3. `cargo test --locked`: unit tests plus everything in `tests/`
   - `tests/open_sources.rs`: the fixture from a path, a `file://` URL and
     over a local HTTP server with Range support; file and HTTP must load
     identically.
   - `tests/golden_render.rs`: headless renders checked for where the cloud
     is drawn and against `tests/golden/*.png`.
   - `tests/ui_kittest.rs`: the real viewer in egui_kittest, loaded with the
     fixture and snapshotted against `tests/snapshots/*.png`.
4. Read the test output for `SKIPPED:` lines. A skipped GPU test verified
   nothing; say so in your report. `COPCAT_REQUIRE_GPU=1 cargo test --locked`
   turns a missing adapter into a failure.
5. Render the fixture and look at it:

   ```sh
   cargo run --bin copcat -- render tests/data/fixture.copc.laz -o <scratch>/verify.png --width 960 --height 600
   ```

   Open `verify.png` with the Read tool and check it: a brown ground plane
   seen from the south, a red-roofed building at the back left (north-west)
   with a green hedge in front of it, and four green cone trees on the
   right (south-east). Nothing black, blank, mirrored or cut off. If the change affects colour modes, cameras or heights, render
   those too (`--color classification`, `--view top`, ...) and look at each.

   Write the PNG to a scratch directory, never into the repository.

## When something fails

- Fix the cause. Never add sleeps, retries or longer timeouts to make a test
  pass, and never loosen a tolerance, threshold or expected count to get
  green.
- Golden images (`tests/golden/`), UI snapshots (`tests/snapshots/`) and
  tolerances change only for a stated reason: a deliberate visual change.
  Regenerate with `COPCAT_UPDATE_GOLDENS=1 cargo test --test golden_render`
  or `UPDATE_SNAPSHOTS=1 cargo test --test ui_kittest`, open every changed
  image with the Read tool to confirm it shows what the change intended, and
  give the reason in the commit message.
- On a golden mismatch the render, golden and diff are in
  `target/tmp/golden-failures/`; for a snapshot, egui_kittest writes
  `*.new.png` and `*.diff.png` beside it. Look at them before changing code.
- After three rounds of fixing that still fail, stop and escalate to the
  human with what failed, what you tried and what you think is wrong.

## Report

Say which steps passed, which tests were skipped and why, what you saw in the
rendered PNG, and anything you couldn't verify (for example CI, which can't be
run locally).
