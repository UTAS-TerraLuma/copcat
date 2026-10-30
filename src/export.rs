//! Rendering a view to a PNG: the `copcat render` command, which runs without
//! a window, and the PNG writing the viewer's Save image shares with it.

use std::collections::HashSet;
use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use clap::{Args, ValueEnum};
use copc_streaming::VoxelKey;
use eframe::egui;
use eframe::egui_wgpu::wgpu;

use crate::app::{format_count, uses_16_bit_color};
use crate::camera::{OrbitCamera, Projection, ViewPreset};
use crate::cloud::CloudInfo;
use crate::loader::{Event, Loader};
use crate::lod::{self, LodSettings};
use crate::renderer::{EdlSettings, Image, PointColor, PointRenderer, PointStyle};

/// `copcat render`'s arguments. The viewer's Copy as command writes them too,
/// with `command_line`.
#[derive(Args, Debug)]
pub struct RenderArgs {
    /// A COPC file path or URL.
    pub source: String,
    /// Where to save the PNG.
    #[arg(short, long)]
    pub output: PathBuf,
    /// Image width in pixels.
    #[arg(long, default_value_t = 1920)]
    pub width: u32,
    /// Image height in pixels.
    #[arg(long, default_value_t = 1080)]
    pub height: u32,
    #[command(flatten)]
    pub camera: CameraArgs,
    #[command(flatten)]
    pub style: StyleArgs,
}

/// Where the camera is. Without any of these, it looks at the whole cloud
/// from the south, as the viewer does when it opens a file.
#[derive(Args, Debug)]
#[command(next_help_heading = "Camera")]
pub struct CameraArgs {
    /// Look along an axis, like the viewer's view buttons. Switches to
    /// orthographic unless --projection says otherwise. Applied before the
    /// options below.
    #[arg(long, value_enum)]
    pub view: Option<ViewArg>,
    /// The point the camera looks at, as X,Y,Z in the file's coordinates.
    /// Defaults to the centre of the cloud.
    #[arg(long, value_name = "X,Y,Z", value_parser = parse_xyz, allow_hyphen_values = true)]
    pub target: Option<[f64; 3]>,
    /// Metres from the eye to the target. In orthographic it sets the zoom
    /// instead: the image is 0.83 × this many metres tall.
    #[arg(long, value_name = "METRES")]
    pub distance: Option<f32>,
    /// Which side of the target the eye is on, in degrees anticlockwise from
    /// east: 0 is east of it looking west, -90 south of it looking north.
    #[arg(long, value_name = "DEGREES", allow_hyphen_values = true)]
    pub yaw: Option<f32>,
    /// How far above the target the eye is, in degrees: 0 is level, 90 looks
    /// straight down.
    #[arg(long, value_name = "DEGREES", allow_hyphen_values = true)]
    pub pitch: Option<f32>,
    #[arg(long, value_enum)]
    pub projection: Option<ProjectionArg>,
    /// How much the image shows top to bottom, in degrees. In orthographic
    /// it scales the zoom, like --distance. Defaults to 45.
    #[arg(long, value_name = "DEGREES")]
    pub fov: Option<f32>,
}

/// How the points look, matching the viewer's settings.
#[derive(Args, Debug)]
#[command(next_help_heading = "Style")]
pub struct StyleArgs {
    /// Colour by the file's RGB (if it has any, else height) or by height.
    #[arg(long, value_enum, default_value_t = ColorArg::Rgb)]
    pub color: ColorArg,
    /// Point diameter in pixels.
    #[arg(long, value_name = "PIXELS", default_value_t = 2.0)]
    pub point_size: f32,
    /// Turn off eye-dome lighting, the shading that shows depth.
    #[arg(long)]
    pub no_edl: bool,
    #[arg(long, default_value_t = 1.0)]
    pub edl_strength: f32,
    #[arg(long, value_name = "PIXELS", default_value_t = 1.4)]
    pub edl_radius: f32,
    /// The most points to draw.
    #[arg(long, default_value_t = LodSettings::default().point_budget)]
    pub point_budget: u64,
    /// Load detail until points are at most this many pixels apart (or the
    /// point budget runs out). Lower is more detail.
    #[arg(long, value_name = "PIXELS", default_value_t = LodSettings::default().target_spacing_px)]
    pub target_spacing: f32,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum ViewArg {
    Top,
    Bottom,
    Front,
    Back,
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum ProjectionArg {
    Perspective,
    Orthographic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum ColorArg {
    Rgb,
    Height,
}

impl From<ViewArg> for ViewPreset {
    fn from(view: ViewArg) -> Self {
        match view {
            ViewArg::Top => Self::Top,
            ViewArg::Bottom => Self::Bottom,
            ViewArg::Front => Self::Front,
            ViewArg::Back => Self::Back,
            ViewArg::Left => Self::Left,
            ViewArg::Right => Self::Right,
        }
    }
}

impl From<Projection> for ProjectionArg {
    fn from(projection: Projection) -> Self {
        match projection {
            Projection::Perspective => Self::Perspective,
            Projection::Orthographic => Self::Orthographic,
        }
    }
}

impl From<ProjectionArg> for Projection {
    fn from(projection: ProjectionArg) -> Self {
        match projection {
            ProjectionArg::Perspective => Self::Perspective,
            ProjectionArg::Orthographic => Self::Orthographic,
        }
    }
}

impl CameraArgs {
    /// These exact settings for `camera`, with its target given in the
    /// coordinates of a file whose octree centre is `origin`.
    pub fn from_camera(camera: &OrbitCamera, origin: [f64; 3]) -> Self {
        let local = camera.target.to_array();
        Self {
            view: None,
            target: Some(std::array::from_fn(|i| f64::from(local[i]) + origin[i])),
            distance: Some(camera.distance),
            yaw: Some(camera.yaw.to_degrees()),
            pitch: Some(camera.pitch.to_degrees()),
            projection: Some(camera.projection().into()),
            fov: Some(camera.fov_y.to_degrees()),
        }
    }

    /// The camera these describe, for the cloud `info` describes.
    pub fn camera(&self, info: &CloudInfo) -> OrbitCamera {
        let mut camera = OrbitCamera::default();
        // First, so the default distance fits the cloud in this view.
        if let Some(fov) = self.fov {
            camera.fov_y = fov.clamp(0.1, 179.0).to_radians();
        }
        // Also sets the clip planes to fit the cloud, which nothing else here does.
        let (min, max) = info.local_bounds();
        camera.frame_bounds(min, max);
        if let Some(view) = self.view {
            camera.set_view(view.into());
        }
        if let Some(target) = self.target {
            camera.target = glam::Vec3::from_array(crate::cloud::to_local(target, info.origin));
        }
        if let Some(distance) = self.distance {
            camera.distance = distance.max(1e-3);
        }
        if let Some(yaw) = self.yaw {
            camera.yaw = yaw.to_radians();
        }
        if let Some(pitch) = self.pitch {
            camera.pitch = pitch.clamp(-90.0, 90.0).to_radians();
        }
        if let Some(projection) = self.projection {
            camera.set_projection(projection.into());
        }
        camera
    }
}

impl StyleArgs {
    pub fn lod(&self) -> LodSettings {
        LodSettings {
            point_budget: self.point_budget,
            target_spacing_px: self.target_spacing,
        }
    }

    fn point_style(&self, has_rgb: bool, rgb_is_16_bit: bool) -> PointStyle {
        PointStyle {
            size_px: self.point_size,
            color: match self.color {
                ColorArg::Rgb if has_rgb => PointColor::rgb(rgb_is_16_bit),
                _ => PointColor::Height,
            },
            edl: (!self.no_edl).then_some(EdlSettings {
                strength: self.edl_strength,
                radius_px: self.edl_radius,
            }),
            clip: None,
        }
    }
}

impl RenderArgs {
    /// A `copcat render` command that renders the same image.
    pub fn command_line(&self) -> String {
        let quote = |text: &str| format!("\"{text}\"");
        let mut parts = vec![
            "copcat render".to_owned(),
            quote(&self.source),
            format!("-o {}", quote(&self.output.to_string_lossy())),
            format!("--width {} --height {}", self.width, self.height),
        ];
        let camera = &self.camera;
        if let Some(view) = camera.view {
            parts.push(format!("--view {}", value_name(view)));
        }
        if let Some([x, y, z]) = camera.target {
            parts.push(format!("--target {x:.3},{y:.3},{z:.3}"));
        }
        if let Some(distance) = camera.distance {
            parts.push(format!("--distance {distance:.3}"));
        }
        // `=` so a negative angle isn't read as a flag.
        if let Some(yaw) = camera.yaw {
            parts.push(format!("--yaw={yaw:.2}"));
        }
        if let Some(pitch) = camera.pitch {
            parts.push(format!("--pitch={pitch:.2}"));
        }
        if let Some(projection) = camera.projection {
            parts.push(format!("--projection {}", value_name(projection)));
        }
        if let Some(fov) = camera.fov {
            parts.push(format!("--fov {fov:.4}"));
        }
        let style = &self.style;
        parts.push(format!("--color {}", value_name(style.color)));
        parts.push(format!("--point-size {:.2}", style.point_size));
        if style.no_edl {
            parts.push("--no-edl".to_owned());
        } else {
            parts.push(format!(
                "--edl-strength {:.2} --edl-radius {:.2}",
                style.edl_strength, style.edl_radius
            ));
        }
        parts.push(format!(
            "--point-budget {} --target-spacing {:.2}",
            style.point_budget, style.target_spacing
        ));
        parts.join(" ")
    }
}

/// The name clap parses `value` from.
fn value_name(value: impl ValueEnum) -> String {
    value
        .to_possible_value()
        .expect("no variants are skipped")
        .get_name()
        .to_owned()
}

fn parse_xyz(text: &str) -> Result<[f64; 3], String> {
    let values: Vec<f64> = text
        .split(',')
        .map(|part| part.trim().parse::<f64>())
        .collect::<Result<_, _>>()
        .map_err(|e| format!("{e}: expected three numbers like 512000.5,5403000,120"))?;
    values
        .try_into()
        .map_err(|_| "expected three numbers like 512000.5,5403000,120".to_owned())
}

/// Opens `args.source`, loads the nodes its view needs at full detail, and
/// saves the image. Blocks until it's written.
pub fn render(args: &RenderArgs) -> Result<(), String> {
    let size = [args.width, args.height];
    let (device, queue) = create_device()?;
    let mut renderer = PointRenderer::headless(device, queue);
    let max = renderer.max_image_size();
    if size.iter().any(|&side| side == 0 || side > max) {
        return Err(format!("width and height must be 1 to {max} on this GPU"));
    }

    // The loader wakes egui on news; a context with no window ignores that.
    let loader = Loader::spawn(args.source.clone(), egui::Context::default());
    let opened = match loader.recv() {
        Some(Event::Opened(opened)) => opened,
        Some(Event::Failed(error)) => return Err(error.to_string()),
        Some(_) => return Err("the loader sent nodes before opening the file".to_owned()),
        None => return Err("the loader thread crashed".to_owned()),
    };
    let info = &opened.info;
    let (min, max) = info.local_bounds();
    renderer.set_z_range([min.z, max.z]);

    let camera = args.camera.camera(info);
    let view = lod::View::new(&camera, size);
    let selection = lod::select_nodes(&opened.octree, &view, &args.style.lod());
    eprintln!(
        "Loading {} nodes ({} points)",
        selection.keys.len(),
        format_count(selection.points)
    );

    loader.want(selection.keys.clone());
    let mut waiting: HashSet<VoxelKey> = selection.keys.iter().copied().collect();
    let mut failed = 0;
    let mut rgb_is_16_bit = false;
    while !waiting.is_empty() {
        match loader.recv() {
            Some(Event::Node { key, result }) => {
                waiting.remove(&key);
                match result {
                    Ok(node) => {
                        let colors = node.colors.as_deref();
                        rgb_is_16_bit |= colors.is_some_and(uses_16_bit_color);
                        renderer.insert_node(key, &node.positions, colors);
                    }
                    Err(error) => {
                        failed += 1;
                        eprintln!("copcat: node {key:?} failed to load: {error}");
                    }
                }
            }
            Some(Event::Fetching(_) | Event::Opened(_)) => {}
            Some(Event::Failed(error)) => return Err(error.to_string()),
            None => return Err("the loader thread crashed".to_owned()),
        }
    }
    if failed > 0 {
        eprintln!("copcat: {failed} nodes failed to load, so the image is missing their points");
    }

    let style = args.style.point_style(info.has_rgb, rgb_is_16_bit);
    let mut viewport = renderer.create_viewport();
    let image = renderer.capture(&mut viewport, size, &camera, &style, &selection.keys)?;
    write_png(&args.output, &image)?;
    println!(
        "Saved {} ({} × {}, {} points)",
        args.output.display(),
        image.width,
        image.height,
        format_count(selection.points)
    );
    Ok(())
}

/// A GPU device with no window, allowing images as big as the GPU can draw.
fn create_device() -> Result<(wgpu::Device, wgpu::Queue), String> {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .map_err(|e| format!("couldn't find a GPU: {e}"))?;
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("copcat render"),
        required_limits: adapter.limits(),
        ..Default::default()
    }))
    .map_err(|e| format!("couldn't start the GPU: {e}"))
}

/// Saves `image` as an 8-bit RGB PNG. The renderer's alpha is always opaque,
/// so it's dropped.
pub fn write_png(path: &Path, image: &Image) -> Result<(), String> {
    let error = |e: &dyn std::fmt::Display| format!("couldn't write {}: {e}", path.display());
    let file = File::create(path).map_err(|e| error(&e))?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), image.width, image.height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let rgb: Vec<u8> = image
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|[r, g, b, _]| [*r, *g, *b])
        .collect();
    let mut writer = encoder.write_header().map_err(|e| error(&e))?;
    writer.write_image_data(&rgb).map_err(|e| error(&e))?;
    writer.finish().map_err(|e| error(&e))
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        render: RenderArgs,
    }

    #[test]
    fn command_line_should_parse_back_to_the_same_camera() {
        let args = RenderArgs {
            source: "C:\\data\\site 1.copc.laz".to_owned(),
            output: PathBuf::from("out.png"),
            width: 800,
            height: 600,
            camera: CameraArgs {
                view: None,
                target: Some([512_000.25, -5_403_000.5, 120.0]),
                distance: Some(42.5),
                yaw: Some(-90.0),
                pitch: Some(-12.5),
                projection: Some(ProjectionArg::Orthographic),
                fov: Some(22.5),
            },
            style: StyleArgs {
                color: ColorArg::Height,
                point_size: 3.0,
                no_edl: true,
                edl_strength: 1.0,
                edl_radius: 1.4,
                point_budget: 1_000_000,
                target_spacing: 1.5,
            },
        };
        let line = args.command_line();
        // A shell would strip the quotes; split on them the same way.
        let words = line
            .split('"')
            .enumerate()
            .flat_map(|(i, part)| {
                if i % 2 == 1 {
                    vec![part.to_owned()]
                } else {
                    part.split_whitespace().map(str::to_owned).collect()
                }
            })
            .skip(2); // "copcat", "render"
        let parsed = Cli::try_parse_from(std::iter::once("copcat".to_owned()).chain(words))
            .unwrap()
            .render;

        assert_eq!(parsed.source, args.source);
        assert_eq!(parsed.camera.target, args.camera.target);
        assert_eq!(parsed.camera.pitch, Some(-12.5));
        assert_eq!(parsed.camera.fov, Some(22.5));
        assert_eq!(parsed.camera.projection, Some(ProjectionArg::Orthographic));
        assert!(parsed.style.no_edl && parsed.style.color == ColorArg::Height);
    }

    #[test]
    fn parse_xyz_should_reject_two_numbers() {
        assert!(parse_xyz("1,2").is_err());
    }
}
