//! What the user can tweak, as plain data with defaults, gathered in
//! [`Settings`]. Nothing here knows about egui or the open cloud, so it can
//! be saved and loaded as it is.
//!
//! [`LodSettings`] and [`EdlSettings`] are defined beside the code that uses
//! them (the command line shares them) and re-exported here.

pub(crate) use crate::lod::LodSettings;
pub(crate) use crate::renderer::EdlSettings;

/// Every user setting the viewer has.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Settings {
    pub(crate) view: ViewSettings,
    pub(crate) map: MapStyleSettings,
    pub(crate) export: ExportSettings,
    pub(crate) diagnostics: DiagnosticsSettings,
}

/// How the points are drawn and how much detail is loaded.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ViewSettings {
    /// Point diameter in logical pixels (scaled by the display's DPI when drawn).
    pub(crate) point_size: f32,
    /// Preferred colouring. RGB falls back to height for files without colour.
    pub(crate) color_mode: ColorMode,
    pub(crate) edl_enabled: bool,
    pub(crate) edl: EdlSettings,
    pub(crate) lod: LodSettings,
}

impl Default for ViewSettings {
    fn default() -> Self {
        Self {
            point_size: 2.0,
            color_mode: ColorMode::default(),
            edl_enabled: true,
            edl: EdlSettings::default(),
            lod: LodSettings::default(),
        }
    }
}

/// How points are drawn on the subcloud map: a small top-down overview, so
/// its own small points and light shading rather than the 3D view's. The
/// colouring follows the 3D view's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MapStyleSettings {
    /// Point diameter in logical pixels.
    pub(crate) point_size: f32,
    pub(crate) edl_enabled: bool,
    pub(crate) edl: EdlSettings,
}

impl Default for MapStyleSettings {
    fn default() -> Self {
        Self {
            point_size: 1.5,
            edl_enabled: true,
            // Enough to pick out roofs and tree crowns from above.
            edl: EdlSettings {
                strength: 0.5,
                radius_px: 1.0,
            },
        }
    }
}

/// How points are coloured.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum ColorMode {
    Height,
    /// The file's RGB, or height for files without colour.
    #[default]
    Rgb,
    /// By LAS classification.
    Classification,
}

impl ColorMode {
    pub(crate) const ALL: [Self; 3] = [Self::Height, Self::Rgb, Self::Classification];

    /// A short name, for the radio buttons.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Height => "Height",
            Self::Rgb => "RGB",
            Self::Classification => "Class",
        }
    }
}

/// The Save image section's settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ExportSettings {
    /// Keep `size` the same as the 3D view's. Otherwise the view shows a
    /// frame around what will be in the image.
    pub(crate) match_view: bool,
    /// Width and height in pixels.
    pub(crate) size: [u32; 2],
}

impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            match_view: true,
            size: [1920, 1080],
        }
    }
}

/// The Diagnostics section, for seeing how streaming is going.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct DiagnosticsSettings {
    /// Show the Diagnostics section in the settings panel.
    pub(crate) visible: bool,
    /// Draw the octree cube of each node that's wanted or being downloaded.
    pub(crate) show_bounds: bool,
    /// Also draw the cubes of nodes that are already loaded.
    pub(crate) show_loaded_bounds: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_settings_should_colour_by_rgb_with_eye_dome_lighting() {
        let view = Settings::default().view;
        assert_eq!(view.color_mode, ColorMode::Rgb);
        assert!(view.edl_enabled);
    }

    #[test]
    fn default_settings_should_hide_diagnostics() {
        assert_eq!(
            Settings::default().diagnostics,
            DiagnosticsSettings {
                visible: false,
                show_bounds: false,
                show_loaded_bounds: false,
            }
        );
    }

    #[test]
    fn default_export_should_match_the_view() {
        assert!(Settings::default().export.match_view);
    }

    #[test]
    fn color_mode_labels_should_be_unique() {
        let labels: std::collections::HashSet<_> =
            ColorMode::ALL.iter().map(|mode| mode.label()).collect();
        assert_eq!(labels.len(), ColorMode::ALL.len());
    }
}
