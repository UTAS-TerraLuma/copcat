//! The Save image section: its settings, the image waiting for its detail to
//! load, writing the PNG, and Copy as command.

use std::path::PathBuf;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use eframe::egui;

use crate::camera::OrbitCamera;
use crate::export::{self, CameraArgs, ColorArg, PointSizeArg, RenderArgs, StyleArgs};
use crate::heights::HeightSettings;
use crate::lod;
use crate::subcloud::Footprint;

use super::commands::{Command, Keymap};
use super::scene::point_style;
use super::session::Session;
use super::settings::{ColorMode, ExportSettings, PointSizeMode, ViewSettings};
use super::{LoadState, Mode, ViewerApp};

/// The image being saved. Its size is in [`ExportSettings`].
#[derive(Default)]
pub(crate) struct ImageExport {
    /// Waiting for the nodes it needs to load.
    pub(crate) job: Option<ExportJob>,
    /// Writing the PNG, which can take a few seconds for a big image, off the
    /// UI thread. Gives the path written.
    pub(crate) writing: Option<JoinHandle<Result<PathBuf, String>>>,
    /// What the last click did, to show under the buttons.
    pub(crate) status: Option<Result<String, String>>,
}

impl ImageExport {
    /// Waiting for detail, or writing the PNG: another can't start yet.
    pub(crate) fn is_busy(&self) -> bool {
        self.job.is_some() || self.writing.is_some()
    }
}

/// An image to render once the detail for it has loaded. The view is copied
/// when Save is clicked, so moving the camera meanwhile doesn't change it.
pub(crate) struct ExportJob {
    pub(crate) path: PathBuf,
    pub(crate) size: [u32; 2],
    pub(crate) camera: OrbitCamera,
    /// The subcloud's outline, in Subcloud mode.
    pub(crate) clip: Option<Footprint>,
    /// Scales point sizes as on screen, which sets them in logical pixels.
    pub(crate) pixels_per_point: f32,
}

impl ExportJob {
    pub(crate) fn view(&self) -> lod::View {
        let view = lod::View::new(&self.camera, self.size);
        match self.clip {
            Some(footprint) => view.clipped_to(footprint),
            None => view,
        }
    }
}

impl ViewerApp {
    /// Asks where to save the 3D view as a PNG, then waits for its detail to
    /// load. Blocks until the dialog closes, which is fine: nothing else can
    /// be clicked meanwhile anyway.
    pub(crate) fn save_image(&mut self, ctx: &egui::Context) {
        if !matches!(self.state, LoadState::Open(_)) {
            return;
        }
        let path = rfd::FileDialog::new()
            .add_filter("PNG image", &["png"])
            .set_file_name("copcat.png")
            .save_file();
        let Some(mut path) = path else { return };
        if path.extension().is_none() {
            path.set_extension("png");
        }
        let size = self.settings.export.size;
        let view_size_px = self.view_size_px;
        let clip = self.clip();
        let camera = framed_camera(self.active_camera(), view_size_px, size);
        self.export.job = Some(ExportJob {
            path,
            size,
            camera,
            clip,
            pixels_per_point: ctx.pixels_per_point(),
        });
        self.export.status = None;
    }

    /// Copies a `copcat render` command for the 3D view to the clipboard.
    pub(crate) fn copy_render_command(&mut self, ctx: &egui::Context) {
        let size = self.settings.export.size;
        let view_size_px = self.view_size_px;
        let camera = framed_camera(self.active_camera(), view_size_px, size);
        let LoadState::Open(session) = &self.state else {
            return;
        };
        let args = render_args(
            session,
            &self.settings.view,
            &self.heights,
            &camera,
            size,
            ctx.pixels_per_point(),
        );
        ctx.copy_text(args.command_line());
        self.export.status = Some(Ok("Copied a copcat render command".to_owned()));
    }

    /// The subcloud's outline, in Subcloud mode.
    fn clip(&self) -> Option<Footprint> {
        match (self.mode, &self.subcloud) {
            (Mode::Subcloud, Some(subcloud_mode)) => Some(subcloud_mode.subcloud.footprint()),
            _ => None,
        }
    }

    /// Renders the image being saved once its nodes have loaded, and
    /// reports when the PNG has been written.
    pub(crate) fn finish_export(&mut self, ctx: &egui::Context) {
        let export = &mut self.export;
        if let LoadState::Open(session) = &mut self.state
            && let Some((done, total)) = session.export_progress(&self.renderer)
            && done == total
            && let Some(job) = export.job.take()
        {
            let style = point_style(
                &self.settings.view,
                session,
                job.pixels_per_point,
                job.clip,
                &self.theme,
            );
            let keys = session.export_selection.take().unwrap_or_default().keys;
            // Its own viewport, dropped afterwards, so the big target doesn't
            // stay in GPU memory.
            let mut viewport = self.renderer.create_viewport();
            match self
                .renderer
                .capture(&mut viewport, job.size, &job.camera, &style, &keys)
            {
                Ok(image) => {
                    let path = job.path;
                    export.writing = Some(thread::spawn(move || {
                        export::write_png(&path, &image).map(|()| path)
                    }));
                }
                Err(error) => export.status = Some(Err(error)),
            }
        }

        match &export.writing {
            Some(writing) if writing.is_finished() => {
                let result = export
                    .writing
                    .take()
                    .and_then(|writing| writing.join().ok())
                    .unwrap_or_else(|| Err("the PNG writer crashed".to_owned()));
                export.status = Some(result.map(|path| format!("Saved {}", path.display())));
            }
            // Nothing else wakes egui when the thread finishes.
            Some(_) => ctx.request_repaint_after(Duration::from_millis(100)),
            None => {}
        }
    }
}

/// What the Save image section shows, beyond its own settings.
pub(crate) struct ExportContext {
    /// The biggest side the GPU can draw.
    pub(crate) max_size: u32,
    /// Nodes loaded of those the image needs, while one is waiting.
    pub(crate) progress: Option<(usize, usize)>,
    pub(crate) in_subcloud: bool,
}

/// The Save image section. Its buttons ask for commands.
pub(crate) fn export_ui(
    ui: &mut egui::Ui,
    export: &ImageExport,
    settings: &mut ExportSettings,
    context: ExportContext,
    keymap: &Keymap,
    commands: &mut Vec<Command>,
) {
    ui.strong("Save image");
    ui.horizontal_wrapped(|ui| {
        ui.add_enabled_ui(!settings.match_view, |ui| {
            for (side, label) in settings.size.iter_mut().zip(["Width", "Height"]) {
                let label = ui.weak(label);
                ui.add(
                    egui::DragValue::new(side)
                        .range(1..=context.max_size)
                        .suffix(" px"),
                )
                .labelled_by(label.id);
            }
        });
        ui.checkbox(&mut settings.match_view, "Match view")
            .on_hover_text(
                "Save the 3D view as it is on screen, at its size in physical pixels. \
                 Untick to choose a size: the view then darkens what will be outside \
                 the image.",
            );
    });

    ui.horizontal_wrapped(|ui| {
        let save_hint = with_shortcut(
            ui,
            keymap,
            &Command::SaveImage,
            "Render the 3D view at this size, once the detail for it has loaded. A \
             bigger image than the view gets more detail, not stretched pixels.",
        );
        let save = ui
            .add_enabled(!export.is_busy(), egui::Button::new("Save PNG…"))
            .on_hover_text(save_hint);
        if save.clicked() {
            commands.push(Command::SaveImage);
        }
        let copy_hint = if context.in_subcloud {
            "Copy a command that renders this view from the command line. It shows the \
             whole cloud from this camera: it can't clip to the subcloud."
        } else {
            "Copy a command that renders this view from the command line"
        };
        if ui
            .button("Copy as command")
            .on_hover_text(copy_hint)
            .clicked()
        {
            commands.push(Command::CopyCommandLine);
        }
    });

    if let Some((done, total)) = context.progress {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.add(egui::Label::new(format!("Loading detail: {done} of {total} nodes")).wrap());
        });
    } else if export.is_busy() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Saving");
        });
    } else {
        match &export.status {
            Some(Ok(message)) => {
                ui.weak(message.as_str());
            }
            Some(Err(error)) => {
                ui.colored_label(ui.visuals().error_fg_color, error.as_str());
            }
            None => {}
        }
    }
}

/// `hint`, followed by the shortcut for `command` if it has one.
fn with_shortcut(ui: &egui::Ui, keymap: &Keymap, command: &Command, hint: &str) -> String {
    match keymap.shortcut(command.id()) {
        Some(shortcut) => format!("{hint} ({})", ui.ctx().format_shortcut(&shortcut)),
        None => hint.to_owned(),
    }
}

/// `camera` narrowed to show just the frame the view draws for an image of
/// `image_size`: the biggest rectangle of the image's shape that fits in a
/// view of `view_size`. Cropping keeps the eye where it is, so only the
/// field of view changes.
pub(crate) fn framed_camera(
    camera: &OrbitCamera,
    view_size: [u32; 2],
    image_size: [u32; 2],
) -> OrbitCamera {
    let aspect = |[width, height]: [u32; 2]| width.max(1) as f32 / height.max(1) as f32;
    // The frame is the view's full height unless the image is wider.
    let height_fraction = (aspect(view_size) / aspect(image_size)).min(1.0);
    let mut framed = camera.clone();
    framed.fov_y = 2.0 * ((camera.fov_y * 0.5).tan() * height_fraction).atan();
    framed
}

/// `copcat render` arguments for `camera`'s view with these settings.
pub(crate) fn render_args(
    session: &Session,
    settings: &ViewSettings,
    heights: &HeightSettings,
    camera: &OrbitCamera,
    size: [u32; 2],
    pixels_per_point: f32,
) -> RenderArgs {
    let [width, height] = size;
    RenderArgs {
        source: session.info.source.clone(),
        output: PathBuf::from("copcat.png"),
        width,
        height,
        camera: CameraArgs::from_camera(camera, session.info.origin),
        style: StyleArgs {
            color: match settings.color_mode {
                ColorMode::Rgb => ColorArg::Rgb,
                ColorMode::Height => ColorArg::Height,
                ColorMode::Classification => ColorArg::Classification,
            },
            z: Some(session.shown),
            point_size_mode: Some(match settings.point_size_mode {
                PointSizeMode::Adaptive => PointSizeArg::Adaptive,
                PointSizeMode::Fixed => PointSizeArg::Fixed,
            }),
            // The command takes physical pixels, like `point_style` makes.
            point_size: Some(settings.point_size * pixels_per_point),
            fill: settings.adaptive_size.fill,
            min_point_size: settings.adaptive_size.min_px * pixels_per_point,
            max_point_size: settings.adaptive_size.max_px * pixels_per_point,
            no_edl: !settings.edl_enabled,
            edl_strength: settings.edl.strength,
            edl_radius: settings.edl.radius_px * pixels_per_point,
            point_budget: settings.lod.point_budget,
            target_spacing: settings.lod.target_spacing_px,
        },
        heights: heights.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framed_camera_should_show_half_the_height_for_an_image_twice_as_wide() {
        let camera = OrbitCamera::default();
        let framed = framed_camera(&camera, [1000, 500], [2000, 500]);
        assert!((framed.half_height() - camera.half_height() * 0.5).abs() < 1e-4);
    }

    #[test]
    fn framed_camera_should_keep_the_height_for_a_narrower_image() {
        let camera = OrbitCamera::default();
        let framed = framed_camera(&camera, [1600, 900], [500, 900]);
        assert_eq!(framed.fov_y, camera.fov_y);
    }
}
