//! Small layout helpers and widgets the panel sections share.

use std::ops::RangeInclusive;

use eframe::egui;
use eframe::egui::emath;

use crate::theme::Theme;

/// The share of a settings box's width left of the split, for labels. The
/// rest, a bit over half as in Blender, is for controls.
pub(crate) const LABEL_SHARE: f32 = 0.4;

/// Where the box being drawn spans, so rows in indented parts of it (such
/// as collapsing sections) still split at the same x.
fn section_span_id() -> egui::Id {
    egui::Id::new("copcat_settings_section_span")
}

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
            ui.spacing_mut().item_spacing.y = theme.spacing.row_gap;
            // Separators and group guide lines: the theme's line colour is for
            // the panel, and all but vanishes on a box in the dark theme.
            let colors = &theme.colors;
            ui.visuals_mut().widgets.noninteractive.bg_stroke.color = egui::lerp(
                egui::Rgba::from(colors.box_fill)..=egui::Rgba::from(colors.strong_stroke),
                0.5,
            )
            .into();
            let span = ui.max_rect().x_range();
            ui.data_mut(|data| data.insert_temp(section_span_id(), span));
            add_contents(ui)
        })
        .inner
}

/// A [`section`] whose contents fold away under a header, as [`Fold`] does.
/// Returns the header's response and, while open, what `add_contents`
/// returned.
pub(crate) fn folding_section<R>(
    ui: &mut egui::Ui,
    theme: &Theme,
    fold: Fold<'_>,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> FoldResponse<R> {
    section(ui, theme, |ui| fold.show(ui, add_contents))
}

/// A group of rows under a header that opens and closes it: a small
/// triangle and the title, the whole row clickable and labelled by the
/// title for AccessKit. Rows in it still split at the box's x, indented or
/// not. Open or closed is kept in egui's memory for the session.
pub(crate) struct Fold<'a> {
    id_salt: &'a str,
    title: &'a str,
    default_open: bool,
    open: Option<bool>,
    indented: bool,
}

pub(crate) struct FoldResponse<R> {
    pub(crate) header: egui::Response,
    /// What the contents returned, if they were shown.
    pub(crate) body: Option<R>,
}

impl<'a> Fold<'a> {
    /// Closed at first. `id_salt` keeps its state apart from other folds'.
    pub(crate) fn new(id_salt: &'a str, title: &'a str) -> Self {
        Self {
            id_salt,
            title,
            default_open: false,
            open: None,
            indented: false,
        }
    }

    /// Rows indented behind a guide line, for a group inside a section, so
    /// it's clear where the group ends.
    pub(crate) fn indented(mut self) -> Self {
        self.indented = true;
        self
    }

    pub(crate) fn default_open(mut self, open: bool) -> Self {
        self.default_open = open;
        self
    }

    /// Open or closed as the caller says, not by clicks on the header; the
    /// caller reads clicks from [`FoldResponse::header`].
    pub(crate) fn open(mut self, open: bool) -> Self {
        self.open = Some(open);
        self
    }

    pub(crate) fn show<R>(
        self,
        ui: &mut egui::Ui,
        add_contents: impl FnOnce(&mut egui::Ui) -> R,
    ) -> FoldResponse<R> {
        let id = ui.make_persistent_id(self.id_salt);
        let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
            ui.ctx(),
            id,
            self.default_open,
        );
        let header = self.header(ui, id.with("header"), &mut state);
        let body = if self.indented {
            state.show_body_indented(&header, ui, add_contents)
        } else {
            state.show_body_unindented(ui, add_contents)
        };
        let body = body.map(|inner| inner.inner);
        FoldResponse { header, body }
    }

    /// The header row, as wide as the box and as tall as a control, laid out
    /// as egui's collapsing headers are.
    fn header(
        &self,
        ui: &mut egui::Ui,
        id: egui::Id,
        state: &mut egui::collapsing_header::CollapsingState,
    ) -> egui::Response {
        let size = egui::vec2(ui.available_width(), ui.spacing().interact_size.y);
        let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
        let mut response = ui.interact(rect, id, egui::Sense::click());
        match self.open {
            Some(open) => {
                if open != state.is_open() {
                    state.toggle(ui);
                }
            }
            None => {
                if response.clicked() {
                    state.toggle(ui);
                    response.mark_changed();
                }
            }
        }
        let title = self.title;
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::CollapsingHeader, ui.is_enabled(), title)
        });
        // So assistive tech, and tests, can tell open from closed.
        let open = state.is_open();
        ui.ctx()
            .accesskit_node_builder(response.id, |node| node.set_expanded(open));
        if ui.is_rect_visible(rect) {
            let indent = ui.spacing().indent;
            let (mut icon_rect, _) = ui.spacing().icon_rectangles(rect);
            icon_rect.set_center(egui::pos2(rect.left() + indent / 2.0, rect.center().y));
            let openness = state.openness(ui.ctx());
            egui::collapsing_header::paint_default_icon(
                ui,
                openness,
                &response.clone().with_new_rect(icon_rect),
            );
            let galley = egui::WidgetText::from(title).into_galley(
                ui,
                Some(egui::TextWrapMode::Truncate),
                (rect.width() - indent).max(0.0),
                egui::TextStyle::Button,
            );
            let at = egui::pos2(
                rect.left() + indent,
                rect.center().y - galley.size().y / 2.0,
            );
            let color = ui.style().interact(&response).text_color();
            ui.painter().galley(at, galley, color);
        }
        response
    }
}

/// The x of a row's label column's left edge, the split, and the right edge
/// of its control column.
struct Columns {
    label_left: f32,
    split: f32,
    right: f32,
}

fn columns(ui: &egui::Ui) -> Columns {
    let own = ui.max_rect().x_range();
    // The box this row is in, unless the row is somewhere else.
    let span = ui
        .data(|data| data.get_temp::<egui::Rangef>(section_span_id()))
        .filter(|span| span.min <= own.min + 0.5 && own.max <= span.max + 0.5)
        .unwrap_or(own);
    let pixels_per_point = ui.pixels_per_point();
    let split = span.min + LABEL_SHARE * span.span();
    // On a whole pixel, so every row's edge lands on the same one.
    let split = (split * pixels_per_point).round() / pixels_per_point;
    Columns {
        label_left: own.min,
        split: split.max(own.min),
        right: own.max,
    }
}

/// A settings row as in Blender's properties: `label` right-aligned left of
/// the split, and the control `add_control` adds right of it, filling the
/// rest of the width. The control is labelled by `label` for AccessKit.
pub(crate) fn prop_row(
    ui: &mut egui::Ui,
    label: &str,
    add_control: impl FnOnce(&mut egui::Ui) -> egui::Response,
) -> egui::Response {
    let label_response = label_cell(ui, label, egui::RichText::new(label).weak());
    prop_control(ui, ("prop_control", label), add_control).labelled_by(label_response.id)
}

/// A row's label, right-aligned against the split, level with the first
/// line of its control. Takes no room: the control column's height sets the
/// row's.
fn label_cell(ui: &mut egui::Ui, id_salt: &str, label: egui::RichText) -> egui::Response {
    let Columns {
        label_left, split, ..
    } = columns(ui);
    let top = ui.cursor().top();
    let height = ui.spacing().interact_size.y;
    let gap = ui.spacing().item_spacing.x;
    let rect = egui::Rect::from_min_max(
        egui::pos2(label_left, top),
        egui::pos2((split - gap).max(label_left), top + height),
    );
    let mut label_ui = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(("prop_label", id_salt))
            .max_rect(rect)
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    // Cut short with "…" rather than pushing the split, the whole label on
    // hover.
    label_ui.add(egui::Label::new(label).truncate())
}

/// A row with nothing left of the split, such as a checkbox, whose text is
/// beside it in the control column.
pub(crate) fn prop_control<R>(
    ui: &mut egui::Ui,
    id_salt: impl egui::AsIdSalt,
    add_control: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let Columns {
        label_left,
        split,
        right,
    } = columns(ui);
    let top = ui.cursor().top();
    let height = ui.spacing().interact_size.y;
    let rect = egui::Rect::from_min_max(
        egui::pos2(split, top),
        egui::pos2(right, ui.max_rect().bottom().max(top + height)),
    );
    let mut control_ui = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(id_salt)
            .max_rect(rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    let inner = add_control(&mut control_ui);
    let bottom = control_ui.min_rect().bottom().max(top + height);
    ui.advance_cursor_after_rect(egui::Rect::from_min_max(
        egui::pos2(label_left, top),
        egui::pos2(right, bottom),
    ));
    inner
}

/// A row of buttons, one per option, filling the width and joined into one
/// control: a toggle between a few choices. Returns which was clicked.
pub(crate) fn segmented(ui: &mut egui::Ui, options: &[(&str, bool)]) -> Option<usize> {
    let height = ui.spacing().interact_size.y;
    let gap = 1.0;
    let count = options.len().max(1) as f32;
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let segment_width = (width - gap * (count - 1.0)) / count;
    let radius = ui.visuals().widgets.inactive.corner_radius;
    let mut clicked = None;
    for (index, &(label, selected)) in options.iter().enumerate() {
        // Edges on whole pixels, the last at the column's edge exactly.
        let pixels_per_point = ui.pixels_per_point();
        let on_pixel = |x: f32| (x * pixels_per_point).round() / pixels_per_point;
        let left = rect.left() + index as f32 * (segment_width + gap);
        let right = if index + 1 == options.len() {
            rect.right()
        } else {
            on_pixel(left + segment_width)
        };
        let left = if index == 0 {
            rect.left()
        } else {
            on_pixel(left)
        };
        let segment = egui::Rect::from_min_max(
            egui::pos2(left, rect.top()),
            egui::pos2(right, rect.bottom()),
        );
        let response = ui.interact(
            segment,
            ui.id().with(("segment", label)),
            egui::Sense::click(),
        );
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::WidgetType::RadioButton,
                ui.is_enabled(),
                selected,
                label,
            )
        });
        if response.clicked() {
            clicked = Some(index);
        }
        if ui.is_rect_visible(segment) {
            let visuals = ui.visuals();
            let state = ui.style().interact(&response);
            let (fill, text_color) = if selected {
                (visuals.selection.bg_fill, visuals.selection.stroke.color)
            } else {
                (state.bg_fill, state.text_color())
            };
            let corners = egui::CornerRadius {
                nw: if index == 0 { radius.nw } else { 0 },
                sw: if index == 0 { radius.sw } else { 0 },
                ne: if index + 1 == options.len() {
                    radius.ne
                } else {
                    0
                },
                se: if index + 1 == options.len() {
                    radius.se
                } else {
                    0
                },
            };
            ui.painter().rect_filled(segment, corners, fill);
            // Narrow padding: two segments share a column a bit over half
            // the panel.
            let padding = ui.spacing().button_padding.x / 2.0;
            let galley = egui::WidgetText::from(label).into_galley(
                ui,
                Some(egui::TextWrapMode::Truncate),
                (segment_width - 2.0 * padding).max(0.0),
                egui::TextStyle::Button,
            );
            let at = egui::Align2::CENTER_CENTER
                .align_size_within_rect(galley.size(), segment)
                .min;
            ui.painter().galley(at, galley, text_color);
        }
    }
    clicked
}

/// A striped table of read-only values, whose rows are added with
/// [`stat_row`], split at the same x as the settings rows.
pub(crate) fn stat_grid(ui: &mut egui::Ui, id: &str, add_rows: impl FnOnce(&mut egui::Ui)) {
    ui.push_id(id, |ui| {
        // Stripes touching, as in a table.
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.data_mut(|data| data.insert_temp(ui.id().with("rows"), 0_usize));
        add_rows(ui);
    });
}

/// A row of a [`stat_grid`]: the label in secondary text, then the value,
/// cut short with "…" where the box is too narrow for it. Returns the value's
/// response, for hover text.
pub(crate) fn stat_row(
    ui: &mut egui::Ui,
    label: &str,
    value: impl Into<egui::WidgetText>,
) -> egui::Response {
    let value = value.into();
    stat_row_with(ui, egui::RichText::new(label).weak(), |ui| {
        ui.horizontal(|ui| ui.add(egui::Label::new(value).truncate()))
            .inner
    })
}

/// A row of a [`stat_grid`] with its own label style and value widget.
pub(crate) fn stat_row_with(
    ui: &mut egui::Ui,
    label: egui::RichText,
    add_value: impl FnOnce(&mut egui::Ui) -> egui::Response,
) -> egui::Response {
    let rows_id = ui.id().with("rows");
    let index = ui.data_mut(|data| {
        let rows = data.get_temp_mut_or_default::<usize>(rows_id);
        *rows += 1;
        *rows - 1
    });
    let top = ui.cursor().top();
    let background = ui.painter().add(egui::Shape::Noop);
    let salt = label.text().to_owned();
    label_cell(ui, &salt, label);
    let value = prop_control(ui, ("stat_value", salt.as_str()), add_value);
    if index % 2 == 1 {
        let Columns {
            label_left, right, ..
        } = columns(ui);
        let rect = egui::Rect::from_min_max(
            egui::pos2(label_left, top),
            egui::pos2(right, ui.min_rect().bottom()),
        );
        let fill = ui.visuals().faint_bg_color;
        ui.painter()
            .set(background, egui::epaint::RectShape::filled(rect, 0.0, fill));
    }
    value
}

/// A value, such as a number, set in the monospaced font.
pub(crate) fn mono(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text).monospace()
}

/// A drop-down for picking one of a few values, showing `selected` and
/// filling the width. Darker than number fields, as in Blender, so the two
/// read apart. `add_options` adds a selectable button per value. Label it
/// with `labelled_by` on the response (`prop_row` does).
pub(crate) fn choice_combo(
    ui: &mut egui::Ui,
    id_salt: impl egui::AsIdSalt,
    selected: impl Into<egui::WidgetText>,
    add_options: impl FnOnce(&mut egui::Ui),
) -> egui::Response {
    let colors = Theme::applied_colors(ui.ctx());
    ui.scope(|ui| {
        let widgets = &mut ui.visuals_mut().widgets;
        widgets.inactive.weak_bg_fill = colors.dropdown;
        widgets.hovered.weak_bg_fill = colors.dropdown_hover;
        widgets.open.weak_bg_fill = colors.dropdown_hover;
        // As tall as a number field.
        let content = text_height(ui, egui::TextStyle::Button).max(ui.spacing().icon_width);
        ui.spacing_mut().button_padding.y = padding_to_fit(ui, content);
        egui::ComboBox::from_id_salt(id_salt)
            .selected_text(selected)
            .width(ui.available_width())
            .truncate()
            .icon(chevron)
            .show_ui(ui, |ui| {
                same_size_items(ui);
                add_options(ui);
            })
            .response
    })
    .inner
}

/// Makes every item in a drop-down's list one size in every state. egui
/// takes a button's outline width off its padding, but draws a selectable
/// button's frame (and so the outline) only once it's hovered, pressed or
/// selected: a plain item was 2 px shorter, and the list moved as the
/// highlight did. With no outline in any state, framed and plain are the
/// same size; the fill still shows the highlight. The padding makes items
/// as tall as a control.
fn same_size_items(ui: &mut egui::Ui) {
    let widgets = &mut ui.visuals_mut().widgets;
    for state in [
        &mut widgets.noninteractive,
        &mut widgets.inactive,
        &mut widgets.hovered,
        &mut widgets.active,
        &mut widgets.open,
    ] {
        state.bg_stroke.width = 0.0;
        state.expansion = 0.0;
    }
    let content = text_height(ui, egui::TextStyle::Button);
    ui.spacing_mut().button_padding.y = padding_to_fit(ui, content);
}

/// How tall a line of text in `style` is laid out.
fn text_height(ui: &egui::Ui, style: egui::TextStyle) -> f32 {
    egui::WidgetText::from("Ag")
        .into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, style)
        .size()
        .y
}

/// Padding above and below `content` that makes a widget exactly the
/// interact height.
fn padding_to_fit(ui: &egui::Ui, content: f32) -> f32 {
    ((ui.spacing().interact_size.y - content) / 2.0).max(0.0)
}

/// A drop-down's small downward chevron.
fn chevron(ui: &egui::Ui, rect: egui::Rect, visuals: &egui::style::WidgetVisuals, _open: bool) {
    let centre = rect.center();
    let half_width = (rect.width() * 0.25).round();
    let half_height = half_width * 0.5;
    ui.painter().line(
        vec![
            centre + egui::vec2(-half_width, -half_height),
            centre + egui::vec2(0.0, half_height),
            centre + egui::vec2(half_width, -half_height),
        ],
        egui::Stroke::new(1.5, visuals.fg_stroke.color),
    );
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
        // No taller than the arrows, so every row is one height.
        let content = text_height(ui, ui.style().drag_value_text_style.clone());
        ui.spacing_mut().button_padding.y = padding_to_fit(ui, content);

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
        // Where the whole field will be, to know before drawing it whether
        // the pointer is over it.
        let field_rect = egui::Rect::from_min_size(ui.cursor().min, egui::vec2(width, height));
        let pointer_over = ui.rect_contains_pointer(field_rect);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            // Drawn once it's known whether any part is hovered.
            let background = ui.painter().add(egui::Shape::Noop);
            let (low, high) = (*self.range.start(), *self.range.end());

            let mut changed = false;
            let value = (self.get_set)(None);
            let decrease_name = format!("Decrease {}", self.name);
            let decrease = arrow(ui, arrow_width, &decrease_name);
            let can_decrease = value > low;
            if decrease.clicked() {
                (self.get_set)(Some(stepped(value, self.step, false, &self.range)));
                changed = true;
            }
            let mut middle = ui
                .scope(|ui| self.middle(ui, width - 2.0 * arrow_width))
                .inner;
            let value = (self.get_set)(None);
            let increase_name = format!("Increase {}", self.name);
            let increase = arrow(ui, arrow_width, &increase_name);
            let can_increase = value < high;
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
            // The arrows only show with the pointer over the field, as in
            // Blender, or when tabbed to; not while typing. They're there to
            // click either way.
            let typing = middle.has_focus() && !middle.dragged();
            let show = ui.is_enabled()
                && !typing
                && (pointer_over || hovered || decrease.has_focus() || increase.has_focus());
            if show {
                paint_arrow(ui, &decrease, false, can_decrease);
                paint_arrow(ui, &increase, true, can_increase);
            }
            middle
        })
        .inner
    }
}

/// Room for one of a number field's arrows, a button for AccessKit, drawn
/// with [`paint_arrow`] when it shows.
fn arrow(ui: &mut egui::Ui, width: f32, name: &str) -> egui::Response {
    let height = ui.spacing().interact_size.y;
    let (_, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), name));
    response
}

/// One of a number field's arrows, dimmed when it can't go further that way.
fn paint_arrow(ui: &egui::Ui, response: &egui::Response, right: bool, can_move: bool) {
    let rect = response.rect;
    let height = rect.height();
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
    use egui::accesskit::Role;
    use egui_kittest::kittest::{NodeT, Queryable};

    /// A settings box `width` wide, with rows at its top level and inside a
    /// collapsing section, which indents them.
    fn rows_harness(width: f32, pixels_per_point: f32) -> egui_kittest::Harness<'static> {
        let mut values = [1.0_f64, 2.0, 3.0];
        let mut ticked = false;
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(width, 400.0))
            .with_pixels_per_point(pixels_per_point)
            .build_ui(move |ui| {
                let theme = Theme::default();
                theme.apply(ui.ctx());
                section(ui, &theme, |ui| {
                    let [a, b, c] = &mut values;
                    prop_row(ui, "Short", |ui| {
                        ui.add(NumberField::new(a, 0.0..=10.0, "Short"))
                    });
                    prop_row(ui, "A much longer label than fits", |ui| {
                        choice_combo(ui, "combo", "Choice", |ui| {
                            ui.label("Option");
                        })
                    });
                    prop_control(ui, "tick", |ui| ui.checkbox(&mut ticked, "Tick"));
                    Fold::new("more", "More")
                        .indented()
                        .default_open(true)
                        .show(ui, |ui| {
                            prop_row(ui, "Inner", |ui| {
                                ui.add(NumberField::new(b, 0.0..=10.0, "Inner"))
                            });
                        });
                    prop_row(ui, "Last", |ui| {
                        ui.add(NumberField::new(c, 0.0..=10.0, "Last"))
                    });
                });
            });
        harness.run();
        harness
    }

    fn rects(harness: &egui_kittest::Harness<'_>, role: Role) -> Vec<egui::Rect> {
        harness
            .query_all_by_role(role)
            .map(|node| node.rect())
            .collect()
    }

    /// Each number field's whole extent, from its left arrow to its right.
    fn number_field_rects(harness: &egui_kittest::Harness<'_>) -> Vec<egui::Rect> {
        let arrow =
            |prefix: &str, name: &str| harness.get_by_label(&format!("{prefix} {name}")).rect();
        harness
            .query_all_by_role(Role::SpinButton)
            .map(|field| {
                let name = field
                    .accesskit_node()
                    .label()
                    .expect("a labelled number field");
                arrow("Decrease", &name).union(arrow("Increase", &name))
            })
            .collect()
    }

    #[test]
    fn rows_should_split_at_one_x_at_any_width_and_scale() {
        for (width, pixels_per_point) in [(300.0, 1.0), (300.0, 2.0), (520.0, 1.5)] {
            let harness = rows_harness(width, pixels_per_point);
            let case = format!("{width} wide at {pixels_per_point}x");
            let controls: Vec<_> = number_field_rects(&harness)
                .into_iter()
                .chain(rects(&harness, Role::ComboBox))
                .collect();
            assert_eq!(controls.len(), 4, "{case}");
            for rect in &controls {
                assert_eq!(rect.height(), controls[0].height(), "{case}: {controls:?}");
            }
            let split = controls[0].left();
            let right = controls[0].right();
            for rect in &controls {
                assert_eq!(rect.left(), split, "{case}: {controls:?}");
                assert_eq!(rect.right(), right, "{case}: {controls:?}");
            }
            // A bit over half the width is for controls.
            let share = (right - split) / width;
            assert!((0.5..0.65).contains(&share), "{case}: {share}");
            let checkbox = rects(&harness, Role::CheckBox);
            assert_eq!(checkbox[0].left(), split, "{case}");

            // Labels end just left of the split, the long one cut short
            // rather than pushing it.
            let label_rights: Vec<_> = ["Short", "A much", "Inner", "Last"]
                .into_iter()
                .map(|label| {
                    harness
                        .query_all_by_role(Role::Label)
                        .find(|node| node.value().is_some_and(|text| text.starts_with(label)))
                        .unwrap_or_else(|| panic!("{case}: no label {label}"))
                        .rect()
                        .right()
                })
                .collect();
            assert_eq!(label_rights.len(), 4, "{case}");
            for right_edge in &label_rights {
                assert_eq!(*right_edge, label_rights[0], "{case}: {label_rights:?}");
                let gap = split - right_edge;
                assert!(gap > 0.0 && gap <= 12.0, "{case}: gap {gap}");
            }
        }
    }

    /// The arrows painted: the only triangles a lone number field draws.
    fn arrows_painted(harness: &egui_kittest::Harness<'_>) -> usize {
        harness
            .output()
            .shapes
            .iter()
            .filter(|clipped| {
                matches!(&clipped.shape, egui::Shape::Path(path) if path.closed && path.points.len() == 3)
            })
            .count()
    }

    #[test]
    fn number_field_arrows_should_show_only_under_the_pointer_and_still_step() {
        let mut value = 2.0_f64;
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(300.0, 100.0))
            .build_ui(move |ui| {
                ui.add(NumberField::new(&mut value, 0.0..=10.0, "Size").step(0.5));
            });
        harness.run();
        let field = harness.get_by_role(Role::SpinButton).rect();
        assert_eq!(arrows_painted(&harness), 0, "hidden at rest");
        // Still there for AccessKit.
        let increase = harness.get_by_label("Increase Size").rect();
        assert!(harness.query_by_label("Decrease Size").is_some());

        harness.hover_at(field.center());
        harness.run();
        assert_eq!(arrows_painted(&harness), 2, "shown under the pointer");
        // The value doesn't move when the arrows show.
        assert_eq!(harness.get_by_role(Role::SpinButton).rect(), field);

        harness.get_by_label("Increase Size").click();
        harness.run();
        let spin = harness.get_by_role(Role::SpinButton);
        assert_eq!(spin.accesskit_node().numeric_value(), Some(2.5));
        assert!(
            increase.right() > field.right(),
            "the arrow is right of the value"
        );

        harness.hover_at(egui::pos2(150.0, 90.0));
        harness.run();
        assert_eq!(arrows_painted(&harness), 0, "hidden again");
    }

    /// A drop-down of four options: one selected, one disabled. Its response
    /// id is kept in the state, to find its list's popup.
    fn combo_harness(theme: Theme) -> egui_kittest::Harness<'static, Option<egui::Id>> {
        let mut chosen = 2_usize;
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(300.0, 300.0))
            .build_ui_state(
                move |ui, combo_id| {
                    theme.apply(ui.ctx());
                    section(ui, &theme, |ui| {
                        let response = prop_row(ui, "Colour", |ui| {
                            choice_combo(
                                ui,
                                "combo",
                                ["Height", "RGB", "Class", "Intensity"][chosen],
                                |ui| {
                                    for (index, label) in ["Height", "RGB", "Class", "Intensity"]
                                        .into_iter()
                                        .enumerate()
                                    {
                                        let option = ui.add_enabled(
                                            index != 1,
                                            egui::Button::selectable(chosen == index, label),
                                        );
                                        if option.clicked() {
                                            chosen = index;
                                        }
                                    }
                                },
                            )
                        });
                        *combo_id = Some(response.id);
                    });
                },
                None,
            );
        harness.run();
        harness
    }

    /// Each option's rect, by label, and the open list's.
    fn list_rects(
        harness: &egui_kittest::Harness<'_, Option<egui::Id>>,
    ) -> (Vec<egui::Rect>, egui::Rect) {
        let items = ["Height", "RGB", "Class", "Intensity"]
            .into_iter()
            .map(|label| {
                harness
                    .query_all_by_label(label)
                    .find(|node| node.accesskit_node().role() == Role::Button)
                    .unwrap_or_else(|| panic!("no option {label}"))
                    .rect()
            })
            .collect();
        let combo_id = harness.state().expect("the drop-down was drawn");
        // What egui's `ComboBox::widget_to_popup_id` makes, which is private.
        let popup_id = combo_id.with("popup");
        let popup = harness
            .ctx
            .memory(|memory| memory.area_rect(popup_id))
            .expect("the list is open");
        (items, popup)
    }

    #[test]
    fn drop_down_options_should_keep_one_size_whatever_is_highlighted() {
        for theme in [Theme::default(), Theme::graphite_light()] {
            let mut harness = combo_harness(theme);
            harness.get_by_role(Role::ComboBox).click();
            harness.run();
            let (items, popup) = list_rects(&harness);
            let height = items[0].height();
            for item in &items {
                assert_eq!(item.height(), height, "{items:?}");
                assert_eq!(item.width(), items[0].width(), "{items:?}");
                // All in the list, so there's nothing to scroll to.
                assert!(popup.contains_rect(*item), "{item:?} outside {popup:?}");
            }

            // Hover each option, then the list's empty edge, and tab through
            // them: nothing moves or grows, and every frame still fits.
            let mut states = Vec::new();
            for item in &items {
                harness.hover_at(item.center());
                harness.step();
                states.push(list_rects(&harness));
                harness.run();
                states.push(list_rects(&harness));
            }
            for label in ["Height", "Class", "Intensity"] {
                harness
                    .query_all_by_label(label)
                    .find(|node| node.accesskit_node().role() == Role::Button)
                    .unwrap()
                    .focus();
                harness.step();
                states.push(list_rects(&harness));
                harness.run();
                states.push(list_rects(&harness));
            }
            for (at, state) in states.iter().enumerate() {
                assert_eq!(state, &(items.clone(), popup), "state {at}");
            }
        }
    }

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
