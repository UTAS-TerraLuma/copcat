//! Small layout helpers and widgets the panel sections share.

use std::ops::RangeInclusive;

use eframe::egui;
use eframe::egui::emath;

use crate::theme::Theme;

/// One section of the Point cloud window, in a box filling its width.
pub(crate) fn section<R>(
    ui: &mut egui::Ui,
    theme: &Theme,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    theme
        .box_frame()
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add_contents(ui)
        })
        .inner
}

/// A striped two-column grid of read-only values, whose rows are added with
/// [`stat_row`].
pub(crate) fn stat_grid(ui: &mut egui::Ui, id: &str, add_rows: impl FnOnce(&mut egui::Ui)) {
    egui::Grid::new(id)
        .num_columns(2)
        .striped(true)
        .show(ui, add_rows);
}

/// A row of a [`stat_grid`]: the label in secondary text, then the value,
/// cut short with "…" where the box is too narrow for it. Returns the value's
/// response, for hover text.
pub(crate) fn stat_row(
    ui: &mut egui::Ui,
    label: &str,
    value: impl Into<egui::WidgetText>,
) -> egui::Response {
    ui.weak(label);
    let value = ui
        .horizontal(|ui| {
            // The value's cell fills the row, so the stripes run across the
            // whole box, however short the value.
            ui.set_min_width(ui.available_width());
            ui.add(egui::Label::new(value).truncate())
        })
        .inner;
    ui.end_row();
    value
}

/// A value, such as a number, set in the monospaced font.
pub(crate) fn mono(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text).monospace()
}

/// How wide `text` is on one line, in `style` unless it says otherwise.
pub(crate) fn text_width(
    ui: &egui::Ui,
    text: impl Into<egui::WidgetText>,
    style: egui::TextStyle,
) -> f32 {
    text.into()
        .into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, style)
        .size()
        .x
}

/// A two-column grid of settings, labels on the left. Grids given the same
/// `label_width` line their controls up.
pub(crate) fn settings_grid(
    ui: &mut egui::Ui,
    id: &str,
    label_width: f32,
    add_rows: impl FnOnce(&mut egui::Ui),
) {
    egui::Grid::new(id)
        .num_columns(2)
        .min_col_width(label_width)
        .show(ui, add_rows);
}

/// A small drop-down for picking one of a few values, showing `selected`,
/// wide enough for the longest of `labels`. `add_options` adds a selectable
/// button per value. Label it with `labelled_by` on the response.
pub(crate) fn choice_combo<'l>(
    ui: &mut egui::Ui,
    id_salt: &str,
    selected: &str,
    labels: impl IntoIterator<Item = &'l str>,
    add_options: impl FnOnce(&mut egui::Ui),
) -> egui::Response {
    let longest = labels
        .into_iter()
        .map(|label| text_width(ui, label, egui::TextStyle::Button))
        .fold(0.0, f32::max);
    let spacing = ui.spacing();
    let fits =
        longest + spacing.icon_width + spacing.item_spacing.x + 2.0 * spacing.button_padding.x;
    // Narrower if that's all there's room for.
    let width = ui.available_width().min(spacing.combo_width.max(fits));
    egui::ComboBox::from_id_salt(id_salt)
        .selected_text(selected)
        .width(width)
        .show_ui(ui, add_options)
        .response
}

/// How far a drag goes to cross a number field's whole range, in points
/// (ten times as far with Shift held).
const DRAG_RANGE_POINTS: f64 = 400.0;

type GetSetValue<'a> = Box<dyn 'a + FnMut(Option<f64>) -> f64>;
type Formatter<'a> = Box<dyn 'a + Fn(f64) -> String>;
type Parser<'a> = Box<dyn 'a + Fn(&str) -> Option<f64>>;

/// A number in a field, as in Blender: drag the middle sideways to change it,
/// click it to type a value (Enter to keep, Esc to cancel), or step it with
/// the arrows at either end. While dragging, the cursor is hidden and held
/// still, so the drag can carry on past the edge of the window.
///
/// Label it with `labelled_by` on the response; `name` names the arrows.
pub(crate) struct NumberField<'a> {
    get_set: GetSetValue<'a>,
    range: RangeInclusive<f64>,
    name: &'a str,
    step: f64,
    precision: f64,
    logarithmic: bool,
    formatter: Option<Formatter<'a>>,
    parser: Option<Parser<'a>>,
}

impl<'a> NumberField<'a> {
    pub(crate) fn new<Num: emath::Numeric>(
        value: &'a mut Num,
        range: RangeInclusive<Num>,
        name: &'a str,
    ) -> Self {
        Self {
            get_set: Box::new(move |new| {
                if let Some(new) = new {
                    *value = Num::from_f64(new);
                }
                value.to_f64()
            }),
            range: range.start().to_f64()..=range.end().to_f64(),
            name,
            step: 1.0,
            precision: 0.01,
            logarithmic: false,
            formatter: None,
            parser: None,
        }
    }

    /// What the field is called, which also names its arrows.
    pub(crate) fn name(&self) -> &'a str {
        self.name
    }

    /// How much an arrow click changes the value: to the next multiple of
    /// `step`.
    pub(crate) fn step(mut self, step: f64) -> Self {
        self.step = step;
        self
    }

    /// What dragged values are rounded to; also sets the decimals shown.
    pub(crate) fn precision(mut self, precision: f64) -> Self {
        self.precision = precision;
        self
    }

    /// Drag by ratios rather than amounts, for ranges spanning orders of
    /// magnitude. The range must be above zero.
    pub(crate) fn logarithmic(mut self) -> Self {
        self.logarithmic = true;
        self
    }

    pub(crate) fn formatter(mut self, formatter: impl 'a + Fn(f64) -> String) -> Self {
        self.formatter = Some(Box::new(formatter));
        self
    }

    pub(crate) fn parser(mut self, parser: impl 'a + Fn(&str) -> Option<f64>) -> Self {
        self.parser = Some(Box::new(parser));
        self
    }

    /// The middle: egui's `DragValue` for showing and typing the value, with
    /// dragging done here instead, from raw mouse motion, which keeps coming
    /// while the cursor is held still.
    fn middle(&mut self, ui: &mut egui::Ui, width: f32) -> egui::Response {
        // See-through, on the field's background.
        let widgets = &mut ui.visuals_mut().widgets;
        for state in [&mut widgets.inactive, &mut widgets.hovered] {
            state.weak_bg_fill = egui::Color32::TRANSPARENT;
            state.bg_stroke = egui::Stroke::NONE;
        }
        for state in [
            &mut widgets.inactive,
            &mut widgets.hovered,
            &mut widgets.active,
        ] {
            state.expansion = 0.0;
        }
        ui.spacing_mut().interact_size.x = width;

        // No DragValue dragging of its own: its speed only steps the value
        // with the arrow keys while typing, and through AccessKit.
        let speed = if ui.input(|i| i.pointer.any_down()) {
            0.0
        } else {
            self.step
        };
        // DragValue still applies the typed text on Esc, the pass after (it
        // looks for lost focus over two passes, and for Esc only in the
        // second). So keep the value from before typing, and put it back for
        // both passes if typing ended with Esc.
        let id = ui.next_auto_id();
        let before_id = id.with("before_typing");
        let cancel_id = id.with("cancelled");
        let pass = ui.ctx().cumulative_pass_nr();
        let typing = ui.memory(|memory| memory.has_focus(id));
        if typing && ui.data(|data| data.get_temp::<f64>(before_id).is_none()) {
            let before = (self.get_set)(None);
            ui.data_mut(|data| data.insert_temp(before_id, before));
        }
        let escaped = ui.input(|i| {
            i.raw.events.iter().any(|event| {
                matches!(
                    event,
                    egui::Event::Key {
                        key: egui::Key::Escape,
                        pressed: true,
                        ..
                    }
                )
            })
        });
        // A value kept from before typing, with no typing now, means typing
        // ended this pass.
        let cancelled = if typing {
            None
        } else {
            let before = ui.data_mut(|data| data.remove_temp::<f64>(before_id));
            match before.filter(|_| escaped) {
                Some(before) => Some((before, pass + 1)),
                None => ui.data_mut(|data| data.remove_temp::<(f64, u64)>(cancel_id)),
            }
        };

        let get_set = &mut self.get_set;
        let mut drag_value = egui::DragValue::from_get_set(get_set)
            .range(self.range.clone())
            .speed(speed)
            .fixed_decimals(decimals(self.precision))
            .update_while_editing(false);
        if let Some(formatter) = &self.formatter {
            drag_value = drag_value.custom_formatter(|value, _| formatter(value));
        }
        if let Some(parser) = &self.parser {
            drag_value = drag_value.custom_parser(|text| parser(text));
        }
        let mut response = ui.add(drag_value);
        if let Some((before, until)) = cancelled {
            if (self.get_set)(None) != before {
                (self.get_set)(Some(before));
                response.mark_changed();
            }
            if pass < until {
                ui.data_mut(|data| data.insert_temp(cancel_id, (before, until)));
            }
        }

        let precise_id = response.id.with("precise");
        let raw_id = response.id.with("raw_motion");
        if response.drag_started() {
            ui.data_mut(|data| {
                data.remove::<f64>(precise_id);
                data.remove::<bool>(raw_id);
            });
            let origin = ui.input(|i| i.pointer.press_origin());
            pointer_capture::begin(ui.ctx(), origin.unwrap_or(response.rect.center()));
        }
        if response.dragged() {
            pointer_capture::hold(ui.ctx());
            let (motion, pixels_per_point, shift) =
                ui.input(|i| (i.pointer.motion(), i.pixels_per_point, i.modifiers.shift));
            // Raw motion where the platform gives it (in about pixels), the
            // pointer's own movement where it doesn't, as in tests. Never
            // both in one drag, or a move could count twice.
            let raw_seen = ui.data(|data| data.get_temp::<bool>(raw_id).unwrap_or(false));
            let points = match motion {
                Some(motion) => {
                    ui.data_mut(|data| data.insert_temp(raw_id, true));
                    motion.x / pixels_per_point
                }
                None if !raw_seen => response.drag_delta().x,
                None => 0.0,
            };
            if points != 0.0 {
                let points = f64::from(points) / if shift { 10.0 } else { 1.0 };
                // Kept unrounded between frames, so slow drags still move.
                let old = (self.get_set)(None);
                let precise = ui
                    .data(|data| data.get_temp::<f64>(precise_id))
                    .unwrap_or(old);
                let precise = dragged(precise, points, &self.range, self.logarithmic);
                ui.data_mut(|data| data.insert_temp(precise_id, precise));
                let new =
                    rounded(precise, self.precision).clamp(*self.range.start(), *self.range.end());
                if new != old {
                    (self.get_set)(Some(new));
                    response.mark_changed();
                }
            }
        }
        response
    }
}

impl egui::Widget for NumberField<'_> {
    fn ui(mut self, ui: &mut egui::Ui) -> egui::Response {
        let height = ui.spacing().interact_size.y;
        let arrow_width = (height * 0.8).round();
        let width = ui.available_width().max(2.0 * arrow_width + height);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            // Drawn once it's known whether any part is hovered.
            let background = ui.painter().add(egui::Shape::Noop);
            let (low, high) = (*self.range.start(), *self.range.end());

            let mut changed = false;
            let value = (self.get_set)(None);
            let decrease_name = format!("Decrease {}", self.name);
            let decrease = arrow(ui, arrow_width, false, value > low, &decrease_name);
            if decrease.clicked() {
                (self.get_set)(Some(stepped(value, self.step, false, &self.range)));
                changed = true;
            }
            let mut middle = ui
                .scope(|ui| self.middle(ui, width - 2.0 * arrow_width))
                .inner;
            let value = (self.get_set)(None);
            let increase_name = format!("Increase {}", self.name);
            let increase = arrow(ui, arrow_width, true, value < high, &increase_name);
            if increase.clicked() {
                (self.get_set)(Some(stepped(value, self.step, true, &self.range)));
                changed = true;
                // The middle already shows the old value.
                ui.ctx().request_repaint();
            }
            if changed {
                middle.mark_changed();
            }

            let hovered = decrease.hovered() || middle.hovered() || increase.hovered();
            let visuals = ui.visuals();
            let state = if hovered {
                &visuals.widgets.hovered
            } else {
                &visuals.widgets.inactive
            };
            ui.painter().set(
                background,
                egui::epaint::RectShape::new(
                    decrease.rect.union(increase.rect),
                    state.corner_radius,
                    state.bg_fill,
                    state.bg_stroke,
                    egui::StrokeKind::Inside,
                ),
            );
            middle
        })
        .inner
    }
}

/// One of a number field's arrows, dimmed when it can't go further that way.
fn arrow(ui: &mut egui::Ui, width: f32, right: bool, can_move: bool, name: &str) -> egui::Response {
    let height = ui.spacing().interact_size.y;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), name));
    if ui.is_rect_visible(rect) {
        let visuals = ui.visuals();
        let color = if !can_move {
            visuals.weak_text_color().gamma_multiply(0.4)
        } else if response.hovered() {
            visuals.strong_text_color()
        } else {
            visuals.weak_text_color()
        };
        let centre = rect.center();
        let (half_height, half_width) = (height * 0.16, height * 0.1);
        let tip = if right { half_width } else { -half_width };
        let points = vec![
            centre + egui::vec2(-tip, -half_height),
            centre + egui::vec2(tip, 0.0),
            centre + egui::vec2(-tip, half_height),
        ];
        ui.painter().add(egui::Shape::convex_polygon(
            points,
            color,
            egui::Stroke::NONE,
        ));
    }
    response
}

/// How many decimals show a value rounded to `precision`.
fn decimals(precision: f64) -> usize {
    if precision >= 1.0 {
        0
    } else {
        (-precision.log10() - 1e-9).ceil() as usize
    }
}

/// `value` to the nearest multiple of `precision`.
fn rounded(value: f64, precision: f64) -> f64 {
    emath::round_to_decimals((value / precision).round() * precision, 9)
}

/// `value` one `step` up or down, onto the next multiple of `step`, within
/// `range`.
fn stepped(value: f64, step: f64, up: bool, range: &RangeInclusive<f64>) -> f64 {
    // A little slack, so 0.3 counts as a multiple of 0.1.
    let steps = value / step;
    let next = if up {
        (steps + 1e-6).floor() + 1.0
    } else {
        (steps - 1e-6).ceil() - 1.0
    };
    emath::round_to_decimals(next * step, 9).clamp(*range.start(), *range.end())
}

/// `value` after a drag of `points` to the right: across the whole range in
/// [`DRAG_RANGE_POINTS`], by ratio if `logarithmic`.
fn dragged(value: f64, points: f64, range: &RangeInclusive<f64>, logarithmic: bool) -> f64 {
    let (low, high) = (*range.start(), *range.end());
    let value = value.clamp(low, high);
    let moved = if logarithmic {
        (value.ln() + points * (high.ln() - low.ln()) / DRAG_RANGE_POINTS).exp()
    } else {
        value + points * (high - low) / DRAG_RANGE_POINTS
    };
    moved.clamp(low, high)
}

/// Holds the mouse cursor still and hidden while a number field is dragged,
/// and puts it back where the drag started afterwards: when the drag ends,
/// the window loses focus, or the field stops being drawn.
///
/// `Locked` holds the cursor on Windows (winit clips it to one pixel), macOS
/// and Wayland. X11 can't lock it, so there it's only hidden and moves
/// freely, still reporting raw motion, until it's put back.
mod pointer_capture {
    use eframe::egui::{self, Context, CursorGrab, Id, Pos2, ViewportCommand};

    #[derive(Clone, Copy)]
    struct Capture {
        restore_to: Pos2,
        /// The last pass a field was dragged in.
        held_in: u64,
    }

    fn key() -> Id {
        Id::new("number_field_pointer_capture")
    }

    pub(super) fn begin(ctx: &Context, restore_to: Pos2) {
        // Added once per context: it lets the cursor go once no field
        // holds it.
        ctx.add_plugin(Release);
        let held_in = ctx.cumulative_pass_nr();
        let had = ctx.data_mut(|data| {
            let had = data.get_temp::<Capture>(key());
            // Back to where the first drag started, should two overlap.
            let restore_to = had.map_or(restore_to, |had| had.restore_to);
            data.insert_temp(
                key(),
                Capture {
                    restore_to,
                    held_in,
                },
            );
            had
        });
        if had.is_none() {
            ctx.send_viewport_cmd(ViewportCommand::CursorGrab(CursorGrab::Locked));
            ctx.send_viewport_cmd(ViewportCommand::CursorVisible(false));
        }
    }

    pub(super) fn hold(ctx: &Context) {
        let pass = ctx.cumulative_pass_nr();
        ctx.data_mut(|data| {
            if let Some(mut capture) = data.get_temp::<Capture>(key()) {
                capture.held_in = pass;
                data.insert_temp(key(), capture);
            }
        });
    }

    struct Release;

    impl egui::Plugin for Release {
        fn debug_name(&self) -> &'static str {
            "number_field_pointer_capture"
        }

        fn on_end_pass(&mut self, ui: &mut egui::Ui) {
            let ctx = ui.ctx();
            let Some(capture) = ctx.data(|data| data.get_temp::<Capture>(key())) else {
                return;
            };
            let focused = ctx.input(|i| i.focused);
            if focused && capture.held_in == ctx.cumulative_pass_nr() {
                return;
            }
            if !focused {
                // Or the drag would carry on when the window comes back.
                ctx.stop_dragging();
            }
            ctx.data_mut(|data| data.remove::<Capture>(key()));
            ctx.send_viewport_cmd(ViewportCommand::CursorGrab(CursorGrab::None));
            ctx.send_viewport_cmd(ViewportCommand::CursorPosition(capture.restore_to));
            ctx.send_viewport_cmd(ViewportCommand::CursorVisible(true));
        }
    }
}

/// Opaque version of a debug line colour, for the legend.
pub(crate) fn legend_color([r, g, b, _]: [f32; 4]) -> egui::Color32 {
    let to_byte = |channel: f32| (channel * 255.0).round() as u8;
    egui::Color32::from_rgb(to_byte(r), to_byte(g), to_byte(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrows_should_step_to_the_next_multiple_of_the_step() {
        let range = 0.0..=10.0;
        assert_eq!(stepped(2.0, 0.5, true, &range), 2.5);
        assert_eq!(stepped(2.2, 0.5, true, &range), 2.5);
        assert_eq!(stepped(2.2, 0.5, false, &range), 2.0);
        assert_eq!(stepped(0.3, 0.1, true, &range), 0.4, "0.3 is on the grid");
    }

    #[test]
    fn arrows_should_stop_at_the_ends_of_the_range() {
        let range = 1.0..=10.0;
        assert_eq!(stepped(10.0, 0.5, true, &range), 10.0);
        assert_eq!(stepped(1.2, 0.5, false, &range), 1.0);
        assert_eq!(stepped(5e5, 1e6, true, &(5e5..=3e7)), 1e6);
    }

    #[test]
    fn dragging_should_cross_the_range_in_drag_range_points() {
        let range = 0.0..=4.0;
        assert_eq!(dragged(1.0, DRAG_RANGE_POINTS / 4.0, &range, false), 2.0);
        assert_eq!(dragged(1.0, 2.0 * DRAG_RANGE_POINTS, &range, false), 4.0);
        assert_eq!(dragged(1.0, -DRAG_RANGE_POINTS, &range, false), 0.0);
    }

    #[test]
    fn logarithmic_dragging_should_move_by_ratios() {
        let range = 1.0..=100.0;
        let halfway = dragged(1.0, DRAG_RANGE_POINTS / 2.0, &range, true);
        assert!((halfway - 10.0).abs() < 1e-9, "{halfway}");
        assert_eq!(dragged(50.0, DRAG_RANGE_POINTS, &range, true), 100.0);
    }

    #[test]
    fn precision_should_set_rounding_and_decimals() {
        assert_eq!(rounded(1.234, 0.1), 1.2);
        assert_eq!(rounded(5_123_456.0, 1e4), 5_120_000.0);
        assert_eq!(decimals(0.01), 2);
        assert_eq!(decimals(0.1), 1);
        assert_eq!(decimals(1e4), 0);
    }
}
