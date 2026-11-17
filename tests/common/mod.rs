//! Shared by the integration tests: the fixture, a file server, and PNG and
//! GPU helpers.

#![allow(dead_code)] // Each test binary uses a different part of this.

pub mod server;

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

/// The repository's `tests/data` directory.
pub fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data")
}

/// The synthetic fixture's path. See `tests/data/README.md`.
pub fn fixture() -> PathBuf {
    data_dir().join("fixture.copc.laz")
}

/// What `tests/data/generate_fixture.py` put in the fixture.
pub mod expected {
    pub const POINTS: u64 = 7739;
    pub const BOUNDS_MIN: [f64; 3] = [500_000.0, 5_400_000.0, 100.0];
    pub const BOUNDS_MAX: [f64; 3] = [500_100.0, 5_400_060.0, 125.0];
    /// (class, count): ground, low vegetation, high vegetation, building.
    pub const CLASSES: [(u32, usize); 4] = [(2, 3969), (3, 400), (5, 2465), (6, 905)];
}

/// A scratch directory for one test's outputs, emptied first. Under
/// `target/` so it's ignored by git and cleaned by `cargo clean`.
pub fn scratch_dir(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(test);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Set (to anything) where a GPU, real or software, is meant to exist, such
/// as CI: then a missing adapter fails GPU tests instead of skipping them.
pub const REQUIRE_GPU: &str = "COPCAT_REQUIRE_GPU";

/// What a GPU test should do after `copcat::render` failed with `error`:
/// returns normally (so the test can return early and pass as skipped) if
/// there's no GPU adapter and none is required, else panics.
pub fn skip_or_fail_without_gpu(error: &str) {
    let no_adapter = error.starts_with("couldn't find a GPU");
    if no_adapter && std::env::var_os(REQUIRE_GPU).is_none() {
        eprintln!(
            "SKIPPED: no GPU adapter ({error}). Install a software rasterizer \
             (lavapipe, WARP) or set {REQUIRE_GPU}=1 to make this a failure."
        );
        return;
    }
    panic!("render failed: {error}");
}

/// An 8-bit RGB image.
#[derive(Clone)]
pub struct Rgb {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<[u8; 3]>,
}

impl Rgb {
    pub fn at(&self, x: u32, y: u32) -> [u8; 3] {
        self.pixels[(y * self.width + x) as usize]
    }
}

pub fn read_png(path: &Path) -> Rgb {
    let file = File::open(path).unwrap_or_else(|e| panic!("open {}: {e}", path.display()));
    let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().unwrap();
    let mut buffer = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buffer).unwrap();
    let channels = info.color_type.samples();
    let pixels = buffer[..info.buffer_size()]
        .chunks_exact(channels)
        .map(|p| match channels {
            1 | 2 => [p[0]; 3],
            _ => [p[0], p[1], p[2]],
        })
        .collect();
    Rgb {
        width: info.width,
        height: info.height,
        pixels,
    }
}

pub fn write_png(path: &Path, image: &Rgb) {
    let file = File::create(path).unwrap();
    let mut encoder = png::Encoder::new(BufWriter::new(file), image.width, image.height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    writer
        .write_image_data(image.pixels.as_flattened())
        .unwrap();
}
