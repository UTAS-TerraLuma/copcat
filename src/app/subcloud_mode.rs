//! Subcloud mode: a map to draw a line, circle or square on, and a 3D view of
//! just the points above it.

use eframe::egui;

use crate::camera::OrbitCamera;
use crate::lod;
use crate::subcloud::{self, ShapeKind, Subcloud};
use crate::subcloud_map::{MapFrame, SubcloudMap};

use super::camera_input::camera_controls;
use super::scene::{Scene, ViewArea, show_gizmo};
use super::session::Views;

/// Subcloud mode's state, which lasts while the cloud is open.
pub(crate) struct SubcloudMode {
    pub(crate) subcloud: Subcloud,
    /// The 3D view of the subcloud. Separate from the Explore camera, so
    /// switching modes returns each view to where it was.
    pub(crate) camera: OrbitCamera,
    pub(crate) map: SubcloudMap,
    /// How to point `camera` at the subcloud next frame, once the 3D view's
    /// aspect ratio is known.
    pub(crate) reframe: Option<Reframe>,
    /// The subcloud's centre when `camera` last followed it. Editing the
    /// shape moves the camera by as much as its centre moved, keeping the
    /// view's angle and zoom.
    pub(crate) followed_centre: glam::Vec2,
    /// How far one Ctrl+scroll (or Ctrl+Shift+scroll) notch slides the
    /// subcloud, as a percentage of its size that way.
    pub(crate) sweep_step_percent: f32,
    /// Where the local origin is in the file's coordinates, for showing
    /// where the subcloud is.
    origin: [f64; 3],
}

/// Ways to point the subcloud camera at the subcloud.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reframe {
    /// Centre and zoom to fit, keeping the camera's angle.
    Fit,
    /// Look at it side-on, then fit.
    Aim,
}

impl SubcloudMode {
    /// Starts with `subcloud`, as drawn in Explore mode, seen side-on.
    /// `bounds` are the cloud's local bounds, for the map, and `origin`
    /// where local coordinates start in the file's.
    pub(crate) fn new(
        subcloud: Subcloud,
        bounds: (glam::Vec3, glam::Vec3),
        origin: [f64; 3],
    ) -> Self {
        Self {
            subcloud,
            camera: OrbitCamera::default(),
            map: SubcloudMap::new(bounds),
            reframe: Some(Reframe::Aim),
            followed_centre: subcloud.centre(),
            sweep_step_percent: subcloud::DEFAULT_SWEEP_STEP_PERCENT,
            origin,
        }
    }

    /// Replaces the subcloud with a newly drawn one and looks at it
    /// side-on, keeping the map where it was.
    pub(crate) fn replace(&mut self, subcloud: Subcloud) {
        self.subcloud = subcloud;
        self.followed_centre = subcloud.centre();
        self.reframe = Some(Reframe::Aim);
    }

    /// The left panel: subcloud settings above the map.
    pub(crate) fn map_panel(&mut self, ui: &mut egui::Ui) -> MapFrame {
        ui.horizontal(|ui| {
            ui.label("Shape");
            for shape in ShapeKind::ALL {
                let response = ui
                    .selectable_value(&mut self.subcloud.shape, shape, shape.label())
                    .on_hover_text(shape.description());
                // A shape of a very different size may not fit the view.
                if response.changed() {
                    self.reframe = Some(Reframe::Fit);
                }
            }
        });
        self.shape_controls(ui);

        let edit_hint = match self.subcloud.shape {
            ShapeKind::Line => "Drag: draw · Drag A, B, ■ or the strip: edit",
            ShapeKind::Circle | ShapeKind::Square => {
                "Drag: draw · Drag ■: resize · Drag inside: move"
            }
        };
        let hint = format!(
            "{edit_hint}\nDrag the round handle: turn (Shift: snap to 15°). It points \
             forward."
        );
        ui.horizontal(|ui| {
            let label = ui.label("Step");
            ui.add(
                egui::DragValue::new(&mut self.sweep_step_percent)
                    .range(1.0..=500.0)
                    .speed(1.0)
                    .max_decimals(0)
                    .suffix(" %"),
            )
            .labelled_by(label.id)
            .on_hover_text(
                "How far one Ctrl+scroll notch slides the subcloud, as a percentage of \
                 its size that way: its width (or diameter, or side) forward and back, \
                 and with Shift, its length (or diameter, or side) to the right and \
                 left. 100% steps to the next slice with no gap or overlap.",
            );
            let (depth, length) = self.subcloud.depth_and_length();
            let percent = self.sweep_step_percent / 100.0;
            ui.weak(format!(
                "{:.2} m forward · {:.2} m sideways",
                depth * percent,
                length * percent
            ));
        });
        ui.horizontal(|ui| {
            if self.subcloud.shape == ShapeKind::Line
                && ui
                    .button("Swap ends")
                    .on_hover_text("View the slice from the other side")
                    .clicked()
            {
                let line = &mut self.subcloud.line;
                (line.start, line.end) = (line.end, line.start);
                self.reframe = Some(Reframe::Aim);
            }
            if ui
                .button("Reset view")
                .on_hover_text(
                    "Look at the subcloud side-on again, fitted to the points loaded so \
                     far (or double-click it)",
                )
                .clicked()
            {
                self.reframe = Some(Reframe::Aim);
            }
        });
        ui.weak(format!("{hint}\nRight-drag: pan · Scroll: zoom"));

        // The 3D view follows whatever this does to the shape, in `view`.
        self.map.show(ui, &mut self.subcloud)
    }

    /// Exact sizes (and a square's rotation) for the current shape.
    pub(crate) fn shape_controls(&mut self, ui: &mut egui::Ui) {
        // A nested `fn` rather than a closure: closures can't say that the
        // widget they return borrows `value`, but `'_` in a fn signature can.
        fn size(value: &mut f32) -> egui::DragValue<'_> {
            egui::DragValue::new(value)
                .range(subcloud::MIN_SIZE..=10_000.0)
                .speed(0.05)
                .max_decimals(2)
                .suffix(" m")
        }
        ui.horizontal(|ui| {
            match self.subcloud.shape {
                ShapeKind::Line => {
                    let label = ui.label("Width");
                    ui.add(size(&mut self.subcloud.line.width))
                        .labelled_by(label.id);
                }
                ShapeKind::Circle => {
                    let label = ui.label("Radius");
                    ui.add(size(&mut self.subcloud.circle.radius))
                        .labelled_by(label.id);
                }
                ShapeKind::Square => {
                    let label = ui.label("Side");
                    ui.add(size(&mut self.subcloud.square.side))
                        .labelled_by(label.id);
                }
            }
            let label = ui.label("Rotation");
            // A copy, because a line stores its rotation in its ends. Shown
            // in degrees, stored in radians, and kept in 0..360°.
            let mut rotation = self.subcloud.rotation().rem_euclid(std::f32::consts::TAU);
            if ui
                .drag_angle(&mut rotation)
                .labelled_by(label.id)
                .on_hover_text(
                    "Anticlockwise turn. At 0° the round handle (forward) points north, \
                     and a line runs from A in the west to B in the east.",
                )
                .changed()
            {
                self.subcloud.set_rotation(rotation);
            }
        });
        if self.subcloud.shape == ShapeKind::Line {
            ui.label(format!("Length {:.1} m", self.subcloud.line.length()));
        }
        let centre = self.subcloud.centre();
        let [east, north] = [0, 1].map(|axis| self.origin[axis] + f64::from(centre[axis]));
        ui.label(format!("Centre {east:.1} E, {north:.1} N"))
            .on_hover_text("Where the middle of the shape is, in the file's coordinates");
    }

    /// The 3D view of just the subcloud, then the map, which needs this
    /// frame's node selection.
    pub(crate) fn view(
        &mut self,
        ui: &mut egui::Ui,
        scene: &mut Scene<'_>,
        map_frame: Option<MapFrame>,
    ) {
        let area = ViewArea::allocate(ui);
        let input = camera_controls(ui, &area.response, &mut self.camera, true);
        if input.reset {
            self.reframe = Some(Reframe::Aim);
        }
        if input.pinch_px != 0.0 {
            self.sweep(ui, input.pinch_px, input.sideways);
        }

        // Follow the shape wherever the map, the panel or a sweep moved it,
        // keeping the camera's angle and zoom. The view presets and gizmo
        // turn with the shape.
        let centre = self.subcloud.centre();
        self.camera.target += (centre - self.followed_centre).extend(0.0);
        self.followed_centre = centre;
        self.camera.axes = self.subcloud.axes();

        let footprint = self.subcloud.footprint();
        if let Some(reframe) = self.reframe.take() {
            // A small plot is lost in the height of the whole cloud, so
            // frame just the height of the points loaded around it.
            let session = &scene.session;
            let (min, max) = session.octree.bounds();
            let known = session.octree.known_heights(session.shown);
            let z_range =
                lod::z_range_under(&session.octree, &footprint, known).unwrap_or((min.z, max.z));
            let aspect = area.rect.aspect_ratio();
            match reframe {
                Reframe::Fit => self.subcloud.frame(&mut self.camera, z_range, aspect),
                Reframe::Aim => self.subcloud.aim(&mut self.camera, z_range, aspect),
            }
        }
        show_gizmo(ui, area.rect, &mut self.camera, scene.theme);

        let view = lod::View::new(&self.camera, area.size_px).clipped_to(footprint);
        let map_view = map_frame
            .as_ref()
            .map(|frame| lod::View::new(self.map.camera(), frame.size_px));
        let views = Views {
            main: &view,
            map: map_view.as_ref(),
            export: scene.export_view,
        };
        scene
            .session
            .update(scene.renderer, views, &scene.settings.lod);
        let texture_id = scene.render_main(&area, &self.camera, Some(footprint));
        area.paint(ui, texture_id, scene.image_aspect, scene.theme);

        if let Some(frame) = map_frame {
            let style = scene.map_style(area.pixels_per_point);
            let texture_id = scene.renderer.render(
                &mut scene.viewports.map,
                frame.size_px,
                self.map.camera(),
                &style,
                &scene.session.map_selection.keys,
                &[],
            );
            self.map.paint(&frame, texture_id, &self.subcloud);
        }
    }

    /// Slides the subcloud for a Ctrl+scroll of `pinch_px`: scrolling up
    /// moves it forward (where its round handle points), and down back.
    /// `sideways` (Shift held) slides it right instead, 90° clockwise from
    /// forward, or left scrolling down. The camera doesn't matter.
    pub(crate) fn sweep(&mut self, ui: &egui::Ui, pinch_px: f32, sideways: bool) {
        let px_per_notch = ui.ctx().options(|o| o.input_options.line_scroll_speed);
        let (depth, length) = self.subcloud.depth_and_length();
        let (direction, size) = if sideways {
            (self.subcloud.along(), length)
        } else {
            (self.subcloud.across(), depth)
        };
        let step = size * self.sweep_step_percent / 100.0;
        // The camera follows in `view`, so the subcloud stays put on screen.
        self.subcloud
            .translate(direction * (pinch_px / px_per_notch * step));
    }
}
