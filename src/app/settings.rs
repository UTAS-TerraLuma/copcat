//! What the user can tweak, as plain data with defaults, gathered in
//! [`Settings`]. Nothing here knows about egui or the open cloud, so it can
//! be saved and loaded as it is.
//!
//! [`LodSettings`] and [`EdlSettings`] are defined beside the code that uses
//! them (the command line shares them) and re-exported here.

pub(crate) use crate::lod::LodSettings;
use crate::renderer::AdaptiveSize;
pub(crate) use crate::renderer::EdlSettings;
use crate::renderer::{DEFAULT_FILL, DEFAULT_MAX_POINT_PX, DEFAULT_MIN_POINT_PX};

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
    /// Size points by their spacing on screen, or all the same.
    pub(crate) point_size_mode: PointSizeMode,
    /// In Adaptive mode: how big points are drawn for their spacing.
    pub(crate) adaptive_size: AdaptiveSizeSettings,
    /// In Fixed mode: point diameter in logical pixels (scaled by the
    /// display's DPI when drawn).
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
            point_size_mode: PointSizeMode::default(),
            adaptive_size: AdaptiveSizeSettings::default(),
            point_size: 2.0,
            color_mode: ColorMode::default(),
            edl_enabled: true,
            edl: EdlSettings::default(),
            lod: LodSettings::default(),
        }
    }
}

impl ViewSettings {
    /// The level of detail to load for a view at `pixels_per_point`. In
    /// Adaptive mode, no finer than points can be drawn: the smallest size
    /// sets how close together they're worth loading.
    pub(crate) fn lod_for_view(&self, pixels_per_point: f32) -> LodSettings {
        match self.point_size_mode {
            PointSizeMode::Fixed => self.lod,
            PointSizeMode::Adaptive => {
                let size = &self.adaptive_size;
                self.lod
                    .no_finer_than(AdaptiveSize::finest_useful_spacing_px(
                        size.fill,
                        size.min_px * pixels_per_point,
                    ))
            }
        }
    }
}

/// How big points are drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum PointSizeMode {
    /// By how far apart they are on screen, so the surface looks filled
    /// close up and far away.
    Adaptive,
    /// All the same size in pixels.
    #[default]
    Fixed,
}

impl PointSizeMode {
    pub(crate) const ALL: [Self; 2] = [Self::Fixed, Self::Adaptive];

    /// A short name, for the drop-down.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Adaptive => "Adaptive",
            Self::Fixed => "Fixed",
        }
    }
}

/// Adaptive point sizes, in logical pixels (scaled by the display's DPI
/// when drawn). See [`AdaptiveSize`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct AdaptiveSizeSettings {
    /// Diameter as a multiple of the points' spacing on screen.
    pub(crate) fill: f32,
    pub(crate) min_px: f32,
    pub(crate) max_px: f32,
}

impl Default for AdaptiveSizeSettings {
    fn default() -> Self {
        Self {
            fill: DEFAULT_FILL,
            min_px: DEFAULT_MIN_POINT_PX,
            max_px: DEFAULT_MAX_POINT_PX,
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

    /// A short name, for the drop-down.
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
    fn default_settings_should_draw_points_at_a_fixed_size() {
        assert_eq!(
            Settings::default().view.point_size_mode,
            PointSizeMode::Fixed
        );
    }

    #[test]
    fn lod_for_view_should_load_no_finer_than_the_smallest_point_in_adaptive_mode() {
        let mut view = ViewSettings {
            point_size_mode: PointSizeMode::Adaptive,
            ..ViewSettings::default()
        };
        view.lod.target_spacing_px = 1.0;
        view.adaptive_size.min_px = 1.5;
        view.adaptive_size.fill = 1.5;
        // 2 × 1.5 logical px × 2 px per point / 1.5.
        assert_eq!(view.lod_for_view(2.0).target_spacing_px, 4.0);
        view.point_size_mode = PointSizeMode::Fixed;
        assert_eq!(view.lod_for_view(2.0), view.lod);
    }

    #[test]
    fn color_mode_labels_should_be_unique() {
        let labels: std::collections::HashSet<_> =
            ColorMode::ALL.iter().map(|mode| mode.label()).collect();
        assert_eq!(labels.len(), ColorMode::ALL.len());
    }
}
