//! Headless renders of the fixture through `copcat::render` (what `copcat
//! render` runs), checked two ways: robust facts about where the cloud is
//! drawn, and a tolerant comparison with a committed golden image.
//!
//! Needs a GPU adapter, real or software. Without one the tests print
//! SKIPPED and pass, unless `COPCAT_REQUIRE_GPU` is set (as in CI).
//!
//! Goldens live in `tests/golden/`. To accept new renders after a deliberate
//! change, run `COPCAT_UPDATE_GOLDENS=1 cargo test --test golden_render`,
//! look at every changed PNG, and give the reason in the commit message.
//! On a mismatch the render and a diff image are written to
//! `target/tmp/golden-failures/`.

mod common;

use std::path::{Path, PathBuf};

use clap::Parser;
use common::Rgb;

const WIDTH: u32 = 320;
const HEIGHT: u32 = 200;

// Calibration (Windows, 2026-11): the goldens were made on an NVIDIA RTX
// 5080 (DX12). Intel integrated graphics and WARP (software) differ from them
// by at most 1 level in any block, with no differing blocks. A 2° yaw change
// or point size 2 -> 2.5 fails (5% and 3% of drawn blocks). Lavapipe on Linux
// hasn't been measured yet.

/// Downsampling before comparing, so a point that lands a pixel over on
/// another GPU's rasterizer doesn't count as a difference.
const BLOCK: u32 = 4;
/// A block differs when any channel of its average is off by more than this.
const BLOCK_TOLERANCE: u8 = 24;
/// The share of blocks allowed to differ, out of those where either image
/// draws something (so empty background doesn't dilute it).
const MAX_DIFFERING_BLOCKS: f64 = 0.02;

#[derive(Parser)]
struct Cli {
    #[command(flatten)]
    render: copcat::RenderArgs,
}

/// Renders the fixture with `copcat render`'s `flags`, or `None` (after
/// printing SKIPPED) when there's no GPU and none is required.
fn render(name: &str, flags: &[&str]) -> Option<Rgb> {
    let dir = common::scratch_dir(&format!("golden_render-{name}"));
    let output = dir.join(format!("{name}.png"));
    let size = [WIDTH.to_string(), HEIGHT.to_string()];
    let fixture = common::fixture();
    let mut argv = vec![
        "render",
        fixture.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "--width",
        &size[0],
        "--height",
        &size[1],
    ];
    argv.extend_from_slice(flags);
    let args = Cli::try_parse_from(argv).unwrap().render;
    if let Err(error) = copcat::render(&args) {
        common::skip_or_fail_without_gpu(&error);
        return None;
    }
    Some(common::read_png(&output))
}

/// The renderer's clear colour, read from a corner nothing is drawn in.
fn background(image: &Rgb) -> [u8; 3] {
    image.at(0, 0)
}

fn is_drawn(pixel: [u8; 3], background: [u8; 3]) -> bool {
    pixel
        .iter()
        .zip(background)
        .any(|(&a, b)| a.abs_diff(b) > 12)
}

/// How many pixels differ from the background, and the box around them as
/// (min x, min y, max x, max y).
fn drawn(image: &Rgb) -> (usize, [u32; 4]) {
    let background = background(image);
    let mut count = 0;
    let mut bbox = [u32::MAX, u32::MAX, 0, 0];
    for y in 0..image.height {
        for x in 0..image.width {
            if is_drawn(image.at(x, y), background) {
                count += 1;
                bbox = [
                    bbox[0].min(x),
                    bbox[1].min(y),
                    bbox[2].max(x),
                    bbox[3].max(y),
                ];
            }
        }
    }
    (count, bbox)
}

fn golden_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden")
        .join(format!("{name}.png"))
}

fn downsample(image: &Rgb) -> Vec<[u8; 3]> {
    let (bw, bh) = (image.width / BLOCK, image.height / BLOCK);
    let mut blocks = Vec::with_capacity((bw * bh) as usize);
    for by in 0..bh {
        for bx in 0..bw {
            let mut sum = [0u32; 3];
            for y in by * BLOCK..(by + 1) * BLOCK {
                for x in bx * BLOCK..(bx + 1) * BLOCK {
                    for (s, c) in sum.iter_mut().zip(image.at(x, y)) {
                        *s += u32::from(c);
                    }
                }
            }
            blocks.push(sum.map(|s| (s / (BLOCK * BLOCK)) as u8));
        }
    }
    blocks
}

/// Compares `actual` with the golden image `name`, writing it there instead
/// when `COPCAT_UPDATE_GOLDENS` is set.
fn assert_matches_golden(name: &str, actual: &Rgb) {
    let path = golden_path(name);
    if std::env::var_os("COPCAT_UPDATE_GOLDENS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        common::write_png(&path, actual);
        eprintln!("updated golden {}", path.display());
        return;
    }
    assert!(
        path.exists(),
        "no golden image at {}; create it with COPCAT_UPDATE_GOLDENS=1",
        path.display()
    );
    let golden = common::read_png(&path);
    assert_eq!(
        (golden.width, golden.height),
        (actual.width, actual.height),
        "golden {name} is a different size"
    );

    // The clear colour is the same on every GPU, so it must match closely:
    // a block tolerance would let a changed background through.
    let clear = background(&golden);
    let actual_clear = background(actual);
    assert!(
        (0..3).all(|c| clear[c].abs_diff(actual_clear[c]) <= 2),
        "{name}: background is {actual_clear:?}, golden has {clear:?}"
    );

    let (want, got) = (downsample(&golden), downsample(actual));
    let drawn_blocks = want
        .iter()
        .zip(&got)
        .filter(|(a, b)| is_drawn(**a, clear) || is_drawn(**b, clear))
        .count()
        .max(1);
    let differing = want
        .iter()
        .zip(&got)
        .filter(|(a, b)| (0..3).any(|c| a[c].abs_diff(b[c]) > BLOCK_TOLERANCE))
        .count();
    let share = differing as f64 / drawn_blocks as f64;
    let worst = want
        .iter()
        .zip(&got)
        .map(|(a, b)| (0..3).map(|c| a[c].abs_diff(b[c])).max().unwrap())
        .max()
        .unwrap_or(0);
    eprintln!(
        "{name}: {:.2}% of drawn blocks differ, worst block by {worst}, pixel-identical: {}",
        share * 100.0,
        golden.pixels == actual.pixels
    );
    if share > MAX_DIFFERING_BLOCKS {
        let failures = Path::new(env!("CARGO_TARGET_TMPDIR")).join("golden-failures");
        std::fs::create_dir_all(&failures).unwrap();
        common::write_png(&failures.join(format!("{name}.actual.png")), actual);
        common::write_png(&failures.join(format!("{name}.golden.png")), &golden);
        let diff = Rgb {
            width: actual.width,
            height: actual.height,
            pixels: golden
                .pixels
                .iter()
                .zip(&actual.pixels)
                .map(|(a, b)| {
                    let d = (0..3).map(|c| a[c].abs_diff(b[c])).max().unwrap();
                    [d.saturating_mul(4), 0, 0]
                })
                .collect(),
        };
        common::write_png(&failures.join(format!("{name}.diff.png")), &diff);
        panic!(
            "{name}: {:.1}% of {BLOCK}x{BLOCK} blocks differ from the golden by more than \
             {BLOCK_TOLERANCE} (allowed {:.1}%). Images are in {}",
            share * 100.0,
            MAX_DIFFERING_BLOCKS * 100.0,
            failures.display()
        );
    }
}

/// The default view: from the south, perspective, RGB with eye-dome lighting.
#[test]
fn default_view_should_match_golden() {
    let Some(image) = render("default_rgb", &[]) else {
        return;
    };
    let (count, [x0, y0, x1, y1]) = drawn(&image);
    // The cloud is framed in the middle, with a margin all round.
    assert!((3_000..30_000).contains(&count), "{count} pixels drawn");
    assert!(x0 > 10 && y0 > 10 && x1 < WIDTH - 10 && y1 < HEIGHT - 10);
    let centre_x = (x0 + x1) / 2;
    assert!(
        centre_x.abs_diff(WIDTH / 2) < 20,
        "centred at x = {centre_x}"
    );
    assert_matches_golden("default_rgb", &image);
}

/// Straight down in orthographic, coloured by class: the drawn box has the
/// cloud's 100 x 60 m shape, north is up, and the building (north-west) and
/// trees (south-east) are where the generator put them.
#[test]
fn top_view_should_show_the_site_the_right_way_up() {
    let Some(image) = render(
        "top_classification",
        &[
            "--view",
            "top",
            "--color",
            "classification",
            "--no-edl",
            "--point-size",
            "3",
        ],
    ) else {
        return;
    };
    let (count, [x0, y0, x1, y1]) = drawn(&image);
    assert!((15_000..64_000).contains(&count), "{count} pixels drawn");
    let (w, h) = (f64::from(x1 - x0), f64::from(y1 - y0));
    let aspect = w / h;
    assert!((aspect - 100.0 / 60.0).abs() < 0.08, "drawn box {w} x {h}");

    // Average colour around a point given in metres from the south-west corner.
    let sample = |east: f64, north: f64| -> [u32; 3] {
        let x = f64::from(x0) + east / 100.0 * w;
        let y = f64::from(y1) - north / 60.0 * h;
        let mut sum = [0; 3];
        for dy in -2..=2 {
            for dx in -2..=2 {
                let pixel = image.at((x as i32 + dx) as u32, (y as i32 + dy) as u32);
                for c in 0..3 {
                    sum[c] += u32::from(pixel[c]);
                }
            }
        }
        sum.map(|s| s / 25)
    };
    // Building roof: classification red.
    let [r, g, b] = sample(21.0, 44.0);
    assert!(r > g + 40 && r > b + 40, "building at ({r}, {g}, {b})");
    // The tallest tree: classification dark green.
    let [r, g, b] = sample(80.0, 15.0);
    assert!(g > r + 20 && g > b + 10, "tree at ({r}, {g}, {b})");
    // Open ground in the south-west: classification brown.
    let [r, g, b] = sample(20.0, 10.0);
    assert!(r > g && g > b, "ground at ({r}, {g}, {b})");

    assert_matches_golden("top_classification", &image);
}

/// From the east, a little above, coloured by height: the trees (south,
/// on the left from here) reach higher than anything else.
#[test]
fn east_view_should_match_golden() {
    let Some(image) = render(
        "east_height",
        &[
            "--yaw=0",
            "--pitch=15",
            "--color",
            "height",
            "--point-size",
            "2",
        ],
    ) else {
        return;
    };
    let (count, [x0, _, x1, _]) = drawn(&image);
    assert!((2_000..40_000).contains(&count), "{count} pixels drawn");
    assert!(x0 > 5 && x1 < WIDTH - 5);
    assert_matches_golden("east_height", &image);
}

/// The colour most pixels have.
fn most_common(image: &Rgb) -> [u8; 3] {
    let mut counts = std::collections::HashMap::new();
    for pixel in &image.pixels {
        *counts.entry(*pixel).or_insert(0) += 1;
    }
    counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(pixel, _)| pixel)
        .unwrap()
}

/// The share of pixels in `[x0, y0, x1, y1)` that something is drawn on.
fn coverage(image: &Rgb, [x0, y0, x1, y1]: [u32; 4], background: [u8; 3]) -> f64 {
    let drawn = (y0..y1)
        .flat_map(|y| (x0..x1).map(move |x| (x, y)))
        .filter(|&(x, y)| is_drawn(image.at(x, y), background))
        .count();
    drawn as f64 / f64::from((x1 - x0) * (y1 - y0))
}

/// Looking straight down at open ground in the south-west, about 7 px
/// between points: 2 px points leave most of it empty, while adaptive sizes
/// cover it.
#[test]
fn adaptive_points_should_fill_ground_that_fixed_points_leave_gaps_in() {
    let view = [
        "--view",
        "top",
        "--target",
        "500025,5400015,100",
        "--distance",
        "30",
        "--no-edl",
    ];
    let with = |mode: &str| -> Option<Rgb> {
        let mut flags = view.to_vec();
        flags.extend(["--point-size-mode", mode]);
        render(&format!("ground_close_{mode}"), &flags)
    };
    let (Some(adaptive), Some(fixed)) = (with("adaptive"), with("fixed")) else {
        return;
    };
    // Ground fills the adaptive image to its corners, so take the clear
    // colour from the fixed one, where most pixels are empty.
    let clear = most_common(&fixed);
    // The middle of the image, all ground.
    let middle = [WIDTH / 4, HEIGHT / 4, WIDTH * 3 / 4, HEIGHT * 3 / 4];
    let (adaptive, fixed) = (
        coverage(&adaptive, middle, clear),
        coverage(&fixed, middle, clear),
    );
    eprintln!("ground covered: adaptive {adaptive:.3}, fixed {fixed:.3}");
    assert!(
        adaptive > 0.85 && fixed < 0.3,
        "ground covered: adaptive {adaptive:.2}, fixed {fixed:.2}"
    );
}

/// From far away the points are much closer together than 2 px, so
/// adaptive points are smaller and cover no more than fixed ones.
#[test]
fn adaptive_points_should_not_cover_more_than_fixed_from_far_away() {
    let with = |mode: &str| -> Option<Rgb> {
        let flags = ["--distance", "600", "--no-edl", "--point-size-mode", mode];
        render(&format!("far_{mode}"), &flags)
    };
    let (Some(adaptive), Some(fixed)) = (with("adaptive"), with("fixed")) else {
        return;
    };
    let ((adaptive, _), (fixed, _)) = (drawn(&adaptive), drawn(&fixed));
    eprintln!("pixels drawn from far away: adaptive {adaptive}, fixed {fixed}");
    assert!(
        adaptive > 0 && adaptive <= fixed,
        "pixels drawn: adaptive {adaptive}, fixed {fixed}"
    );
}

/// The same view from a file and over HTTP gives the same pixels.
#[test]
fn render_over_http_should_match_render_from_file() {
    let server = common::server::FileServer::serve(common::data_dir());
    let url = server.url("fixture.copc.laz");
    let dir = common::scratch_dir("golden_render-http");
    let mut images = Vec::new();
    for (name, source) in [
        ("file", common::fixture().to_str().unwrap().to_owned()),
        ("http", url),
    ] {
        let output = dir.join(format!("{name}.png"));
        let argv = [
            "render",
            &source,
            "-o",
            output.to_str().unwrap(),
            "--width",
            "160",
            "--height",
            "100",
        ];
        let args = Cli::try_parse_from(argv).unwrap().render;
        if let Err(error) = copcat::render(&args) {
            common::skip_or_fail_without_gpu(&error);
            return;
        }
        images.push(common::read_png(&output));
    }
    assert!(
        images[0].pixels == images[1].pixels,
        "file and HTTP renders differ"
    );
}
