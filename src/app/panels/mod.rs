//! The settings panel's sections, each a function drawing into a `Ui`, and
//! the small widgets they share.

mod cloud_info;
mod display;
mod heights;
mod view;
mod widgets;

pub(crate) use cloud_info::{cloud_summary, streaming_stats};
pub(crate) use display::settings_ui;
pub(crate) use heights::{HeightsAction, heights_ui};
pub(crate) use view::view_ui;
pub(crate) use widgets::section;
