//! The Heights section: which height points are drawn at, and where each
//! height is read from.

use eframe::egui;

use crate::heights::{HeightKind, HeightSettings, HeightSource, SourceSetting};

use super::widgets::{choice_combo, prop_control, prop_row};
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
    prop_row(ui, "Draw at", |ui| {
        choice_combo(ui, "draw_at_height", session.shown.label(), |ui| {
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
    });
    egui::CollapsingHeader::new("Sources")
        .id_salt("height_sources")
        .show(ui, |ui| {
            for kind in HeightKind::ALL {
                // What it's set to, then where it was read from this time.
                prop_row(ui, kind.label(), |ui| {
                    let setting = draft.get_mut(kind);
                    let combo =
                        choice_combo(ui, ("height_source", kind), setting_label(setting), |ui| {
                            let options =
                                [SourceSetting::Auto, SourceSetting::From(HeightSource::Z)]
                                    .into_iter()
                                    .chain(info.extra_dimensions.iter().map(|name| {
                                        SourceSetting::From(HeightSource::Extra(name.clone()))
                                    }))
                                    .chain([SourceSetting::Absent]);
                            for option in options {
                                let label = setting_label(&option);
                                ui.selectable_value(setting, option, label);
                            }
                        });
                    let now = match &info.heights[kind] {
                        Some(source) => format!("now {source}"),
                        None => "now missing".to_owned(),
                    };
                    // Under the box, cut short if it doesn't fit.
                    ui.add(egui::Label::new(egui::RichText::new(now).weak().small()).truncate());
                    combo
                });
            }
            prop_control(ui, "reload_heights", |ui| {
                let size = egui::vec2(ui.available_width(), ui.spacing().interact_size.y);
                let response = ui
                    .add_enabled_ui(draft != applied, |ui| {
                        ui.add_sized(size, egui::Button::new("Reload with these"))
                    })
                    .inner
                    .on_hover_text("Open the cloud again, reading heights from where these say");
                if response.clicked() {
                    action = Some(HeightsAction::Reload);
                }
            });
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
