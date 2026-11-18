//! The Heights section: which height points are drawn at, and where each
//! height is read from.

use eframe::egui;

use crate::heights::{HeightKind, HeightSettings, HeightSource, SourceSetting};

use super::widgets::{choice_combo, text_width};
use crate::app::session::Session;

/// A change asked for in the Heights section.
pub(crate) enum HeightsAction {
    Show(HeightKind),
    /// Open the cloud again, reading heights from where the section now says.
    Reload,
}

/// The Heights section: which height the points are drawn at, and where in
/// the file each height comes from.
pub(crate) fn heights_ui(
    ui: &mut egui::Ui,
    session: &Session,
    draft: &mut HeightSettings,
    applied: &HeightSettings,
) -> Option<HeightsAction> {
    let info = &session.info;
    let mut action = None;
    ui.horizontal(|ui| {
        let label = ui.weak("Draw at");
        let labels = HeightKind::ALL.map(HeightKind::label);
        choice_combo(ui, "draw_at_height", session.shown.label(), labels, |ui| {
            for kind in HeightKind::ALL {
                let available = info.heights[kind].is_some();
                let response = ui
                    .add_enabled(
                        available,
                        egui::Button::selectable(session.shown == kind, kind.label()),
                    )
                    .on_disabled_hover_text("Not in this file. Choose where it is under Sources.");
                if response.clicked() && session.shown != kind {
                    action = Some(HeightsAction::Show(kind));
                }
            }
        })
        .labelled_by(label.id);
    });
    egui::CollapsingHeader::new("Sources")
        .id_salt("height_sources")
        .show(ui, |ui| {
            egui::Grid::new("height_sources_grid")
                .num_columns(2)
                .show(ui, |ui| {
                    for kind in HeightKind::ALL {
                        let label = ui.weak(kind.label());
                        // What it's set to, then where it was read from this
                        // time.
                        ui.horizontal_wrapped(|ui| {
                            let setting = draft.get_mut(kind);
                            // Narrower than usual if that's all there's room for.
                            let width = ui.available_width().min(ui.spacing().combo_width);
                            let combo = egui::ComboBox::from_id_salt(("height_source", kind))
                                .selected_text(setting_label(setting))
                                .width(width)
                                .show_ui(ui, |ui| {
                                    let options =
                                        [SourceSetting::Auto, SourceSetting::From(HeightSource::Z)]
                                            .into_iter()
                                            .chain(info.extra_dimensions.iter().map(|name| {
                                                SourceSetting::From(HeightSource::Extra(
                                                    name.clone(),
                                                ))
                                            }))
                                            .chain([SourceSetting::Absent]);
                                    for option in options {
                                        let label = setting_label(&option);
                                        ui.selectable_value(setting, option, label);
                                    }
                                });
                            let _ = combo.response.labelled_by(label.id);
                            let now = match &info.heights[kind] {
                                Some(source) => format!("now {source}"),
                                None => "now missing".to_owned(),
                            };
                            // Kept on one line: under the box if it doesn't fit beside it,
                            // and cut short if it doesn't fit there either.
                            let now = egui::RichText::new(now).weak();
                            if text_width(ui, now.clone(), egui::TextStyle::Body)
                                > ui.available_width()
                            {
                                ui.end_row();
                            }
                            ui.add(egui::Label::new(now).truncate());
                        });
                        ui.end_row();
                    }
                });
            let response = ui
                .add_enabled(draft != applied, egui::Button::new("Reload with these"))
                .on_hover_text("Open the cloud again, reading heights from where these say");
            if response.clicked() {
                action = Some(HeightsAction::Reload);
            }
        });
    action
}

pub(crate) fn setting_label(setting: &SourceSetting) -> String {
    match setting {
        SourceSetting::Auto => "Auto".to_owned(),
        SourceSetting::Absent => "None".to_owned(),
        SourceSetting::From(source) => source.to_string(),
    }
}
