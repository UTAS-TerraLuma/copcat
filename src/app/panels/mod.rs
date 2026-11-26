//! The settings panel's sections, each a function drawing into a `Ui`, the
//! status bar at the foot of the window, and the small widgets they share.
//!
//! Sections change settings directly where that's all there is to it, and
//! push a [`Command`](super::commands::Command) for anything the app has to
//! act on.

mod cloud_info;
mod diagnostics;
mod display;
mod heights;
mod status;
mod view;
mod widgets;

pub(crate) use cloud_info::cloud_summary;
pub(crate) use diagnostics::diagnostics_ui;
pub(crate) use display::{CloudFacts, rendering_ui, save_defaults_button};
pub(crate) use heights::{HeightsAction, heights_ui};
pub(crate) use status::{spinner, status_bar};
pub(crate) use view::view_ui;
pub(crate) use widgets::{
    Fold, NumberField, choice_combo, folding_section, prop_control, prop_row, section,
};
