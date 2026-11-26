//! Icon buttons painted with egui's painter (there's no icon font), and the
//! subcloud tools' strip floating over the top-left of the explore view, as
//! Blender's viewport toolbar does.

use eframe::egui::{self, Color32, Pos2, Rect, Response, Sense, Stroke, Vec2};

use crate::subcloud::ShapeKind;
use crate::theme::Theme;

use super::commands::Command;

/// Each tool button's side, in logical pixels.
const BUTTON_SIZE: f32 = 30.0;
/// Between the strip's edge and its buttons, and between buttons.
const STRIP_PADDING: f32 = 3.0;
/// The strip's gap from the 3D view's edges.
const STRIP_MARGIN: f32 = 10.0;

/// How much of the 3D view's left edge the strip takes up, margins
/// included, so text drawn over the view can start clear of it.
pub(crate) const STRIP_RESERVED_WIDTH: f32 = STRIP_MARGIN + BUTTON_SIZE + 2.0 * STRIP_PADDING;

/// What an icon button shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Icon {
    Shape(ShapeKind),
    /// An X, for closing something.
    Close,
}

/// A square icon button taking `side` × `side` in `ui`'s layout. `label`
/// names it for screen readers and UI tests.
pub(crate) fn icon_button(
    ui: &mut egui::Ui,
    theme: &Theme,
    side: f32,
    icon: Icon,
    label: &str,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(side), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    paint_button(ui, theme, rect, &response, icon, false);
    response
}

/// Line, Circle and Square, stacked down a small panel over the top-left of
/// `view` (the explore view's rect): pick one, then drag on the view to draw
/// that subcloud. Clicking the picked one again puts it down. Pushes what's
/// asked for to `commands`.
///
/// Call after the 3D view's widget is registered: widgets registered later
/// are on top, so the strip, not the view, gets the pointer over it. The
/// panel behind the buttons senses drags, so a drag that starts on the
/// strip neither orbits nor draws.
pub(crate) fn tool_strip(
    ui: &mut egui::Ui,
    theme: &Theme,
    view: Rect,
    picked: Option<ShapeKind>,
    commands: &mut Vec<Command>,
) {
    let count = ShapeKind::ALL.len() as f32;
    let size = Vec2::new(
        BUTTON_SIZE + 2.0 * STRIP_PADDING,
        count * BUTTON_SIZE + (count + 1.0) * STRIP_PADDING,
    );
    let panel = Rect::from_min_size(view.min + Vec2::splat(STRIP_MARGIN), size);
    let id = ui.id().with("subcloud_tool_strip");
    // Takes drags (and clicks between buttons) that start on the strip.
    let _ = ui.interact(panel, id, Sense::click_and_drag());
    let colors = &theme.colors;
    let radius = f32::from(theme.shape.widget_radius) + STRIP_PADDING;
    ui.painter().rect(
        panel,
        radius,
        colors.panel.gamma_multiply(0.9),
        Stroke::new(1.0, colors.stroke),
        egui::StrokeKind::Inside,
    );
    for (at, shape) in ShapeKind::ALL.into_iter().enumerate() {
        let top = panel.top() + STRIP_PADDING + at as f32 * (BUTTON_SIZE + STRIP_PADDING);
        let rect = Rect::from_min_size(
            Pos2::new(panel.left() + STRIP_PADDING, top),
            Vec2::splat(BUTTON_SIZE),
        );
        let is_picked = picked == Some(shape);
        let response = ui.interact(rect, id.with(shape.label()), Sense::click());
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::WidgetType::Button,
                ui.is_enabled(),
                is_picked,
                shape.label(),
            )
        });
        paint_button(ui, theme, rect, &response, Icon::Shape(shape), is_picked);
        let how = match shape {
            ShapeKind::Line => "from one end to the other",
            ShapeKind::Circle | ShapeKind::Square => "from its centre to its edge",
        };
        let response = response.on_hover_text(format!(
            "{}. Then drag on the cloud to draw it, {how}. Esc cancels.",
            shape.description()
        ));
        if response.clicked() {
            commands.push(if is_picked {
                Command::CancelSubcloudTool
            } else {
                Command::SubcloudTool(shape)
            });
        }
    }
}

/// A button's background, in the accent colour if `selected`, and its icon.
fn paint_button(
    ui: &egui::Ui,
    theme: &Theme,
    rect: Rect,
    response: &Response,
    icon: Icon,
    selected: bool,
) {
    let colors = &theme.colors;
    let fill = if response.is_pointer_button_down_on() {
        colors.pressed
    } else if selected {
        colors.active
    } else if response.hovered() {
        colors.hover
    } else {
        Color32::TRANSPARENT
    };
    let ink = if selected && !response.is_pointer_button_down_on() {
        colors.active_ink
    } else {
        colors.text
    };
    let painter = ui.painter();
    painter.rect_filled(rect, theme.shape.widget_radius, fill);
    paint_icon(painter, rect, icon, ink);
}

/// Draws `icon` in the middle of `rect`.
fn paint_icon(painter: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    let stroke = Stroke::new(1.5, color);
    // The icon's box: a little over half the button.
    let inner = Rect::from_center_size(rect.center(), Vec2::splat(rect.width() * 0.5));
    match icon {
        Icon::Shape(ShapeKind::Line) => {
            let (from, to) = (inner.left_bottom(), inner.right_top());
            painter.line_segment([from, to], stroke);
            for end in [from, to] {
                painter.circle_filled(end, 2.5, color);
            }
        }
        Icon::Shape(ShapeKind::Circle) => {
            painter.circle_stroke(inner.center(), inner.width() / 2.0, stroke);
        }
        Icon::Shape(ShapeKind::Square) => {
            painter.rect_stroke(inner, 0.0, stroke, egui::StrokeKind::Middle);
        }
        Icon::Close => {
            let cross = inner.shrink(inner.width() * 0.1);
            painter.line_segment([cross.left_top(), cross.right_bottom()], stroke);
            painter.line_segment([cross.left_bottom(), cross.right_top()], stroke);
        }
    }
}
