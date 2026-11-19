//! Messages shown as small toasts floating over the bottom-left of the main
//! area, just above the status bar. They sit in their own foreground layer,
//! so nothing else moves when one comes or goes.
//!
//! Information goes by itself after a few seconds (paused while the pointer
//! is over the toasts); warnings and errors stay until dismissed, so there's
//! time to read them. Timing uses egui's clock, so tests can step it.

use eframe::egui;

use crate::theme::Theme;

/// How long an informational toast shows, in seconds of egui time.
pub(crate) const INFO_SECONDS: f64 = 4.0;
/// The most toasts shown at once; older ones are counted in "+N more".
pub(crate) const MAX_SHOWN: usize = 4;
/// The widest a toast's text gets before it wraps, in points.
const MAX_TEXT_WIDTH: f32 = 420.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Severity {
    /// Something done, such as a file saved. Goes by itself.
    Info,
    /// A problem worth knowing, such as a bad setting. Stays until dismissed.
    Warn,
    /// Something asked for that failed. Stays until dismissed.
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Message {
    pub(crate) text: String,
    pub(crate) severity: Severity,
    /// Seconds left before an informational toast goes; `None` stays.
    remaining: Option<f64>,
    /// Shown at least once, so the clock runs for it from then on.
    seen: bool,
    /// Tells toasts apart from frame to frame, for their widget ids.
    id: u64,
}

impl Message {
    pub(crate) fn is_warning(&self) -> bool {
        self.severity != Severity::Info
    }
}

/// The messages waiting to be read, oldest first.
#[derive(Debug, Default)]
pub(crate) struct Toasts {
    messages: Vec<Message>,
    next_id: u64,
    /// egui's time at the last frame, to count down by.
    last_time: Option<f64>,
    /// The pointer was over the toasts last frame, which pauses them.
    hovered: bool,
}

impl Toasts {
    pub(crate) fn push(&mut self, severity: Severity, text: impl Into<String>) {
        let remaining = (severity == Severity::Info).then_some(INFO_SECONDS);
        self.messages.push(Message {
            text: text.into(),
            severity,
            remaining,
            seen: false,
            id: self.next_id,
        });
        self.next_id += 1;
    }

    pub(crate) fn clear(&mut self) {
        self.messages.clear();
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// Where the shown toasts start: the newest [`MAX_SHOWN`].
    fn first_shown(&self) -> usize {
        self.messages.len().saturating_sub(MAX_SHOWN)
    }

    /// Runs the clock on by `dt` seconds for the toasts showing, dropping
    /// information whose time is up.
    fn tick(&mut self, dt: f64) {
        let first = self.first_shown();
        for message in &mut self.messages[first..] {
            if let Some(remaining) = &mut message.remaining
                && message.seen
            {
                *remaining -= dt;
            }
            message.seen = true;
        }
        self.messages
            .retain(|message| message.remaining.is_none_or(|left| left > 0.0));
    }

    /// Draws the toasts in their own layer, `bottom` points up from the
    /// foot of the window and `left` in from its left edge.
    pub(crate) fn show(&mut self, ctx: &egui::Context, theme: &Theme, left: f32, bottom: f32) {
        let now = ctx.input(|input| input.time);
        let dt = self.last_time.map_or(0.0, |last| (now - last).max(0.0));
        self.last_time = Some(now);
        // Paused while the pointer is over them. The frame after it leaves
        // skips the time spent hovering too.
        if !self.hovered {
            self.tick(dt);
        }
        if self.messages.is_empty() {
            self.hovered = false;
            return;
        }

        let first = self.first_shown();
        let mut dismissed = None;
        let area = egui::Area::new(egui::Id::new("toasts"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::LEFT_BOTTOM, egui::vec2(left, -bottom))
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = theme.spacing.item_spacing.y;
                if first > 0 {
                    theme.toast_frame().show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(format!("+{first} more"))
                                .small()
                                .color(theme.colors.text_secondary),
                        )
                        .on_hover_text("Older messages, shown as these are dismissed");
                    });
                }
                for message in &self.messages[first..] {
                    ui.push_id(message.id, |ui| {
                        if toast(ui, theme, message) {
                            dismissed = Some(message.id);
                        }
                    });
                }
                ui.ui_contains_pointer()
            });
        self.hovered = area.inner;
        if let Some(id) = dismissed {
            self.messages.retain(|message| message.id != id);
            ctx.request_repaint();
        }

        // Wake up when the next one is due to go.
        let next = self.messages[self.first_shown()..]
            .iter()
            .filter_map(|message| message.remaining)
            .reduce(f64::min);
        if let Some(next) = next
            && !self.hovered
        {
            ctx.request_repaint_after_secs(next as f32);
        }
    }
}

/// One toast; returns whether its close button was clicked.
fn toast(ui: &mut egui::Ui, theme: &Theme, message: &Message) -> bool {
    let colors = &theme.colors;
    let color = match message.severity {
        Severity::Info => colors.text,
        Severity::Warn => colors.warn,
        Severity::Error => colors.error,
    };
    let prefix = if message.is_warning() {
        "Settings: "
    } else {
        ""
    };
    let mut close = false;
    theme.toast_frame().show(ui, |ui| {
        ui.horizontal_top(|ui| {
            ui.scope(|ui| {
                ui.set_max_width(MAX_TEXT_WIDTH);
                let text = egui::RichText::new(format!("{prefix}{}", message.text)).color(color);
                ui.add(egui::Label::new(text).wrap());
            });
            close = close_button(ui, theme).clicked();
        });
    });
    close
}

/// A small "×", drawn thin so it doesn't outweigh the message.
fn close_button(ui: &mut egui::Ui, theme: &Theme) -> egui::Response {
    let side = theme.fonts.body_size + 1.0;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Dismiss"));
    let colors = &theme.colors;
    let color = if response.hovered() {
        ui.painter()
            .rect_filled(rect, theme.shape.widget_radius, colors.hover);
        colors.text
    } else {
        colors.text_secondary
    };
    let cross = rect.shrink(side * 0.3);
    let stroke = egui::Stroke::new(1.25, color);
    let painter = ui.painter();
    painter.line_segment([cross.left_top(), cross.right_bottom()], stroke);
    painter.line_segment([cross.right_top(), cross.left_bottom()], stroke);
    response.on_hover_text("Dismiss")
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::*;

    fn harness(toasts: Toasts) -> Harness<'static, Toasts> {
        let theme = Theme::default();
        Harness::builder()
            .with_size(egui::vec2(800.0, 400.0))
            .build_ui_state(
                move |ui, toasts: &mut Toasts| {
                    theme.apply(ui.ctx());
                    ui.label("Behind");
                    toasts.show(ui.ctx(), &theme, 10.0, 30.0);
                },
                toasts,
            )
    }

    fn shown(harness: &Harness<'_, Toasts>) -> Vec<String> {
        harness
            .query_all_by_role(egui::accesskit::Role::Label)
            .filter_map(|node| node.value())
            .filter(|text| text != "Behind")
            .collect()
    }

    fn now(harness: &Harness<'_, Toasts>) -> f64 {
        harness.ctx.input(|input| input.time)
    }

    /// Runs `seconds` of egui time, a step (a quarter second) at a time.
    fn wait(harness: &mut Harness<'_, Toasts>, seconds: f64) {
        let until = now(harness) + seconds - 0.01;
        while now(harness) < until {
            harness.step();
        }
    }

    #[test]
    fn information_should_go_after_about_four_seconds() {
        let mut harness = harness(Toasts::default());
        harness.run_ok();
        harness.state_mut().push(Severity::Info, "Saved it");
        harness.step();
        assert_eq!(shown(&harness), ["Saved it"]);
        wait(&mut harness, INFO_SECONDS - 0.5);
        assert_eq!(shown(&harness), ["Saved it"], "still there just before");
        wait(&mut harness, 1.0);
        assert!(shown(&harness).is_empty(), "{:?}", shown(&harness));
        assert!(harness.state().is_empty());
    }

    #[test]
    fn information_should_stay_while_hovered() {
        let mut harness = harness(Toasts::default());
        harness.run_ok();
        harness.state_mut().push(Severity::Info, "Saved it");
        // The first frame sizes the toast; the second puts it in place.
        harness.step();
        harness.step();
        let rect = harness.get_by_label("Saved it").rect();
        harness.hover_at(rect.center());
        wait(&mut harness, 3.0 * INFO_SECONDS);
        assert_eq!(shown(&harness), ["Saved it"], "paused while hovered");
        harness.hover_at(egui::pos2(400.0, 50.0));
        // The time hovered doesn't count: it has what was left when the
        // pointer arrived, about three and a half seconds.
        wait(&mut harness, INFO_SECONDS - 1.0);
        assert_eq!(shown(&harness), ["Saved it"]);
        wait(&mut harness, 1.0);
        assert!(shown(&harness).is_empty(), "{:?}", shown(&harness));
    }

    #[test]
    fn warnings_should_stay_until_dismissed() {
        let mut toasts = Toasts::default();
        toasts.push(Severity::Warn, "bad setting");
        toasts.push(Severity::Error, "couldn't save");
        let mut harness = harness(toasts);
        harness.step();
        wait(&mut harness, 5.0 * INFO_SECONDS);
        assert_eq!(
            shown(&harness),
            ["Settings: bad setting", "Settings: couldn't save"]
        );
        // Each has its own close button.
        let dismiss: Vec<_> = harness.query_all_by_label("Dismiss").collect();
        assert_eq!(dismiss.len(), 2);
        dismiss[0].click();
        harness.step();
        harness.step();
        assert_eq!(shown(&harness), ["Settings: couldn't save"]);
    }

    #[test]
    fn toasts_should_stack_up_from_the_bottom_newest_lowest() {
        let mut toasts = Toasts::default();
        for n in 1..=6 {
            toasts.push(Severity::Warn, format!("problem {n}"));
        }
        let mut harness = harness(toasts);
        harness.step();
        harness.step();
        assert_eq!(
            shown(&harness),
            [
                "+2 more",
                "Settings: problem 3",
                "Settings: problem 4",
                "Settings: problem 5",
                "Settings: problem 6"
            ]
        );
        let top = |label: &str| harness.get_by_label(label).rect().top();
        assert!(top("+2 more") < top("Settings: problem 3"));
        assert!(top("Settings: problem 5") < top("Settings: problem 6"));
        // Just above where it was asked to sit: 30 points up from the foot.
        let newest = harness.get_by_label("Settings: problem 6").rect();
        assert!((360.0..370.0).contains(&newest.bottom()), "{newest:?}");
        assert!(newest.left() > 10.0 && newest.left() < 25.0, "{newest:?}");

        // Dismissing the newest brings back the oldest hidden one.
        harness
            .query_all_by_label("Dismiss")
            .last()
            .unwrap()
            .click();
        harness.step();
        harness.step();
        assert_eq!(shown(&harness)[0], "+1 more");
        assert_eq!(shown(&harness)[1], "Settings: problem 2");
    }

    #[test]
    fn hidden_information_should_wait_its_turn() {
        let mut toasts = Toasts::default();
        toasts.push(Severity::Info, "first");
        for n in 1..=MAX_SHOWN {
            toasts.push(Severity::Warn, format!("problem {n}"));
        }
        let mut harness = harness(toasts);
        harness.step();
        wait(&mut harness, 2.0 * INFO_SECONDS);
        harness
            .query_all_by_label("Dismiss")
            .last()
            .unwrap()
            .click();
        harness.step();
        harness.step();
        assert!(shown(&harness).contains(&"first".to_owned()));
    }

    #[test]
    fn long_text_should_wrap_to_a_compact_width() {
        let mut toasts = Toasts::default();
        let long = format!(
            "Saved the theme to C:\\{}\\settings.json",
            "folder\\".repeat(40)
        );
        toasts.push(Severity::Info, long.clone());
        toasts.push(Severity::Info, "Short");
        let mut harness = harness(toasts);
        harness.step();
        harness.step();
        let long_rect = harness.get_by_label(&long).rect();
        let short_rect = harness.get_by_label("Short").rect();
        assert!(long_rect.width() <= MAX_TEXT_WIDTH + 0.5, "{long_rect:?}");
        assert!(long_rect.height() > 2.0 * short_rect.height(), "wrapped");
        assert!(
            short_rect.width() < 60.0,
            "as wide as its text: {short_rect:?}"
        );
    }
}
