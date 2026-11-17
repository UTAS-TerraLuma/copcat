//! What the user can tweak, as plain data.

use crate::lod::LodSettings;
use crate::renderer::EdlSettings;

/// Everything the user can tweak in the floating window.
pub(crate) struct ViewSettings {
    /// Point diameter in logical pixels (scaled by the display's DPI when drawn).
    pub(crate) point_size: f32,
    pub(crate) lod: LodSettings,
    /// Draw the octree cube of each node that's wanted or being downloaded.
    pub(crate) show_bounds: bool,
    /// Also draw the cubes of nodes that are already loaded.
    pub(crate) show_loaded_bounds: bool,
    /// Preferred colouring. RGB falls back to height for files without colour.
    pub(crate) color_mode: ColorMode,
    pub(crate) edl_enabled: bool,
    pub(crate) edl: EdlSettings,
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum ColorMode {
    Height,
    Rgb,
    Classification,
}
