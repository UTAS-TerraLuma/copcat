//! Drawing the 3D views: what each needs for a frame, the space it takes up,
//! and Explore mode's view of the whole cloud.

use eframe::egui;

use crate::camera::OrbitCamera;
use crate::gizmo;
use crate::heights::HeightKind;
use crate::lod;
use crate::renderer::{AdaptiveSize, EdlSettings, PointColor, PointRenderer, PointStyle, Viewport};
use crate::subcloud::{Footprint, Subcloud};
use crate::theme::Theme;

use super::camera_input::camera_controls;
use super::commands::Command;
use super::session::{Session, Views};
use super::settings::{DiagnosticsSettings, MapStyleSettings, PointSizeMode, ViewSettings};
use super::subcloud_tool::SubcloudTool;
use super::tool_strip::tool_strip;

/// Gap between the 3D view's edges and the gizmo, in logical pixels.
const OVERLAY_MARGIN: f32 = 10.0;

/// Where each image is rendered.
pub(crate) struct Viewports {
    /// The 3D view, in either mode.
    pub(crate) main: Viewport,
    /// The subcloud map.
    pub(crate) map: Viewport,
}

/// What every 3D view needs from the app, borrowed for one frame.
pub(crate) struct Scene<'a> {
    pub(crate) renderer: &'a mut PointRenderer,
    pub(crate) viewports: &'a mut Viewports,
    pub(crate) session: &'a mut Session,
    pub(crate) settings: &'a ViewSettings,
    /// How the subcloud map draws its points.
    pub(crate) map_style: &'a MapStyleSettings,
    pub(crate) diagnostics: &'a DiagnosticsSettings,
    /// The view of the image being saved, if any, whose nodes load too.
    pub(crate) export_view: Option<&'a lod::View>,
    /// Width / height of the image Save image will make, when it isn't the
    /// view's own. The view frames it.
    pub(crate) image_aspect: Option<f32>,
    pub(crate) theme: &'a Theme,
}

/// Point style from the settings. Sizes are set in logical pixels, so
/// they're scaled to physical pixels like the render target.
pub(crate) fn point_style(
    settings: &ViewSettings,
    session: &Session,
    pixels_per_point: f32,
    clip: Option<Footprint>,
    theme: &Theme,
) -> PointStyle {
    let adaptive = &settings.adaptive_size;
    PointStyle {
        size_px: settings.point_size * pixels_per_point,
        adaptive: (settings.point_size_mode == PointSizeMode::Adaptive).then(|| AdaptiveSize {
            fill: adaptive.fill,
            min_px: adaptive.min_px * pixels_per_point,
            max_px: adaptive.max_px * pixels_per_point,
            target_spacing_px: settings.lod_for_view(pixels_per_point).target_spacing_px,
        }),
        color: session.point_color(settings.color_mode),
        height: session.shown,
        edl: settings.edl_enabled.then_some(EdlSettings {
            radius_px: settings.edl.radius_px * pixels_per_point,
            ..settings.edl
        }),
        clip,
        background: theme.viewport_background(),
    }
}

/// Point style for the subcloud map: its own size and shading from `map`,
/// whatever the 3D view's are, coloured like the 3D view. Nothing is
/// clipped: the map shows the whole cloud around the subcloud.
pub(crate) fn map_point_style(
    map: &MapStyleSettings,
    color: PointColor,
    height: HeightKind,
    pixels_per_point: f32,
    theme: &Theme,
) -> PointStyle {
    PointStyle {
        size_px: map.point_size * pixels_per_point,
        // Always a fixed size: the map is a small overview.
        adaptive: None,
        color,
        height,
        edl: map.edl_enabled.then_some(EdlSettings {
            radius_px: map.edl.radius_px * pixels_per_point,
            ..map.edl
        }),
        clip: None,
        background: theme.viewport_background(),
    }
}

impl Scene<'_> {
    /// The subcloud map's point style (see [`map_point_style`]).
    pub(crate) fn map_style(&self, pixels_per_point: f32) -> PointStyle {
        map_point_style(
            self.map_style,
            self.session.point_color(self.settings.color_mode),
            self.session.shown,
            pixels_per_point,
            self.theme,
        )
    }

    pub(crate) fn style(&self, pixels_per_point: f32, clip: Option<Footprint>) -> PointStyle {
        point_style(
            self.settings,
            self.session,
            pixels_per_point,
            clip,
            self.theme,
        )
    }

    /// Draws the selected nodes (and debug bounds) into the main viewport.
    pub(crate) fn render_main(
        &mut self,
        area: &ViewArea,
        camera: &OrbitCamera,
        clip: Option<Footprint>,
    ) -> egui::TextureId {
        let lines = if self.diagnostics.show_bounds {
            self.session
                .bounds_lines(self.renderer, self.diagnostics.show_loaded_bounds)
        } else {
            Vec::new()
        };
        let style = self.style(area.pixels_per_point, clip);
        self.renderer.render(
            &mut self.viewports.main,
            area.size_px,
            camera,
            &style,
            &self.session.selection.keys,
            &lines,
        )
    }
}

/// The space a 3D view takes up this frame.
pub(crate) struct ViewArea {
    pub(crate) rect: egui::Rect,
    pub(crate) response: egui::Response,
    /// Render at physical pixel resolution so points stay crisp on high-DPI screens.
    pub(crate) size_px: [u32; 2],
    pub(crate) pixels_per_point: f32,
    /// The bottom layer, reserved for the 3D image. It can only be rendered
    /// once the gizmo has had its say about the camera, but has to be
    /// painted under the gizmo.
    pub(crate) image_slot: egui::layers::ShapeIdx,
    /// Above the image and below the gizmo: the Save image frame.
    pub(crate) frame_slot: egui::layers::ShapeIdx,
}

impl ViewArea {
    pub(crate) fn allocate(ui: &mut egui::Ui) -> Self {
        let (rect, response) =
            ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
        // Named for screen readers and UI tests.
        response
            .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "3D view"));
        let pixels_per_point = ui.ctx().pixels_per_point();
        Self {
            rect,
            response,
            size_px: [rect.width(), rect.height()]
                .map(|side| (side * pixels_per_point).round() as u32),
            pixels_per_point,
            image_slot: ui.painter().add(egui::Shape::Noop),
            frame_slot: ui.painter().add(egui::Shape::Noop),
        }
    }

    /// Paints the 3D image with a panel's rounded corners, and if
    /// `image_aspect` is given, darkens what's outside an image of that shape.
    pub(crate) fn paint(
        &self,
        ui: &egui::Ui,
        texture_id: egui::TextureId,
        image_aspect: Option<f32>,
        theme: &Theme,
    ) {
        let full_uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        let painter = ui.painter();
        let image = egui::epaint::RectShape::filled(
            self.rect,
            theme.shape.panel_radius,
            egui::Color32::WHITE,
        )
        .with_texture(texture_id, full_uv);
        painter.set(self.image_slot, image);
        let Some(aspect) = image_aspect else { return };
        let view = self.rect;
        let frame = image_frame(view, aspect);
        let shade = egui::Color32::from_black_alpha(179);
        // Two of these are empty: the frame touches either the sides or the
        // top and bottom.
        let outside = [
            egui::Rect::from_x_y_ranges(view.x_range(), view.top()..=frame.top()),
            egui::Rect::from_x_y_ranges(view.x_range(), frame.bottom()..=view.bottom()),
            egui::Rect::from_x_y_ranges(view.left()..=frame.left(), frame.y_range()),
            egui::Rect::from_x_y_ranges(frame.right()..=view.right(), frame.y_range()),
        ];
        let mut shapes: Vec<egui::Shape> = outside
            .into_iter()
            .map(|rect| egui::Shape::rect_filled(rect, 0.0, shade))
            .collect();
        shapes.push(egui::Shape::rect_stroke(
            frame,
            0.0,
            egui::Stroke::new(1.0, egui::Color32::from_white_alpha(96)),
            egui::StrokeKind::Inside,
        ));
        painter.set(self.frame_slot, egui::Shape::Vec(shapes));
    }
}

/// Explore mode: the whole cloud in one 3D view, with the subcloud tools
/// floating over it. With a tool picked, dragging draws a subcloud instead
/// of orbiting; returns it once the drag ends. The tools' buttons push
/// their commands to `commands`.
pub(crate) fn explore_view(
    ui: &mut egui::Ui,
    scene: &mut Scene<'_>,
    camera: &mut OrbitCamera,
    tool: &mut SubcloudTool,
    commands: &mut Vec<Command>,
) -> Option<Subcloud> {
    let area = ViewArea::allocate(ui);
    let drawing = tool.is_active();
    let drawn = tool.interact(ui, &area, camera, scene.session);
    let input = camera_controls(ui, &area.response, camera, !drawing);
    if input.reset {
        let (min, max) = scene.session.octree.bounds();
        camera.frame_bounds(min, max);
    }
    // A touchpad pinch zooms like scrolling. (So does Ctrl+scroll, which
    // egui reports the same way.)
    camera.zoom(input.pinch_px);
    show_gizmo(ui, area.rect, camera, scene);

    let view = lod::View::new(camera, area.size_px);
    let views = Views {
        main: &view,
        map: None,
        export: scene.export_view,
    };
    let lod = scene.settings.lod_for_view(area.pixels_per_point);
    scene.session.update(scene.renderer, views, &lod);
    let texture_id = scene.render_main(&area, camera, None);
    area.paint(ui, texture_id, scene.image_aspect, scene.theme);
    tool.paint(ui, &area, camera, scene.theme);
    tool_strip(ui, scene.theme, area.rect, tool.picked(), commands);
    drawn
}

pub(crate) fn show_gizmo(
    ui: &mut egui::Ui,
    view_rect: egui::Rect,
    camera: &mut OrbitCamera,
    scene: &Scene<'_>,
) {
    let rect = egui::Rect::from_min_size(
        egui::pos2(
            view_rect.right() - OVERLAY_MARGIN - gizmo::SIZE,
            view_rect.top() + OVERLAY_MARGIN,
        ),
        egui::Vec2::splat(gizmo::SIZE),
    );
    let colors = scene.theme.axis_colors();
    gizmo::show(ui, rect, camera, colors, scene.session.info.crs);
}

/// The part of a view of `rect` that an image of `aspect` (width / height)
/// shows, centred: the biggest rectangle of that shape that fits.
pub(crate) fn image_frame(rect: egui::Rect, aspect: f32) -> egui::Rect {
    let size = if aspect > rect.aspect_ratio() {
        egui::vec2(rect.width(), rect.width() / aspect)
    } else {
        egui::vec2(rect.height() * aspect, rect.height())
    };
    egui::Rect::from_center_size(rect.center(), size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::settings::Settings;

    /// The map's style for `settings`, as the app builds it.
    fn map_style(settings: &Settings) -> PointStyle {
        map_point_style(
            &settings.map,
            PointColor::Classification,
            HeightKind::Elevation,
            2.0,
            &Theme::default(),
        )
    }

    #[test]
    fn map_style_should_ignore_the_3d_views_point_size_and_shading() {
        let before = Settings::default();
        let mut after = before.clone();
        after.view.point_size = 9.0;
        after.view.edl_enabled = !after.view.edl_enabled;
        after.view.edl.strength = 3.0;
        let (before, after) = (map_style(&before), map_style(&after));
        assert_eq!(before.size_px, after.size_px);
        assert_eq!(before.edl, after.edl);
    }

    #[test]
    fn map_style_should_use_the_map_settings_in_physical_pixels() {
        let settings = Settings::default();
        let style = map_style(&settings);
        assert_eq!(style.size_px, settings.map.point_size * 2.0);
        assert_eq!(style.clip, None);
    }

    #[test]
    fn image_frame_should_fill_the_width_for_a_wide_image() {
        let view = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let frame = image_frame(view, 2.0);
        assert_eq!(
            frame,
            egui::Rect::from_min_max(egui::pos2(0.0, 100.0), egui::pos2(800.0, 500.0))
        );
    }
}
