//! The viewer's look: every colour, font, size, gap and corner radius of the
//! UI chrome, in one place. `Theme::default()` is Graphite, a dark theme with
//! rounded panels over a near-black gutter.
//!
//! This styles the chrome and the 3D view's background. The point colour
//! ramps are data, set by the renderer.

use std::path::{Path, PathBuf};

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Shadow,
    Stroke, TextStyle, Vec2,
};

/// Everything the UI's look is made of.
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    /// Shown wherever themes are listed.
    pub name: String,
    pub colors: Colors,
    pub fonts: Fonts,
    pub spacing: Spacing,
    pub shape: Shape,
}

/// Every colour the chrome uses.
#[derive(Clone, Debug, PartialEq)]
pub struct Colors {
    /// The app background behind panels, showing in the gaps between them.
    pub gutter: Color32,
    /// Panel and window backgrounds.
    pub panel: Color32,
    /// Header strips along the top of panels, and window title bars.
    pub header: Color32,
    /// Boxes grouping settings inside a panel.
    pub box_fill: Color32,
    /// The 3D view's background, behind the points.
    pub viewport: Color32,
    /// Buttons and combo boxes at rest.
    pub control: Color32,
    /// Text fields, checkboxes, radio buttons and slider rails: lighter than
    /// the panel they sit in.
    pub field: Color32,
    /// Any widget under the pointer.
    pub hover: Color32,
    /// Subtle separators, and the outline of windows.
    pub stroke: Color32,
    /// Outlines of widgets.
    pub strong_stroke: Color32,
    /// Normal text.
    pub text: Color32,
    /// Weak text: hints, notes and secondary values.
    pub text_secondary: Color32,
    /// The faintest text, such as the controls help. Only on panels and
    /// headers: it's too low in contrast on fields.
    pub text_tertiary: Color32,
    /// Pressed and selected controls, selected text, and slider fills.
    pub active: Color32,
    /// Text and marks drawn on `active`.
    pub active_ink: Color32,
    /// The accent as text or a line: hyperlinks, focus outlines, the text
    /// cursor.
    pub active_text: Color32,
    /// The thing selected in the scene, as opposed to a selected control.
    pub selection: Color32,
    /// Informational messages.
    pub info: Color32,
    /// Warnings.
    pub warn: Color32,
    /// Errors.
    pub error: Color32,
    /// The X axis, and red channels.
    pub data_red: Color32,
    /// The Y axis, and green channels.
    pub data_green: Color32,
    /// The Z axis, and blue channels.
    pub data_blue: Color32,
}

/// The two typefaces and the text sizes.
#[derive(Clone, Debug, PartialEq)]
pub struct Fonts {
    /// Labels, buttons and headings.
    pub ui: FontChoice,
    /// Numbers, coordinates and stats.
    pub monospace: FontChoice,
    /// Body text and labels, in points.
    pub body_size: f32,
    /// Button text, in points.
    pub button_size: f32,
    /// Small print, in points.
    pub small_size: f32,
    /// Headings and window titles, in points.
    pub heading_size: f32,
    /// Monospaced numbers, in points.
    pub monospace_size: f32,
}

/// A typeface, and where its file is.
#[derive(Clone, Debug, PartialEq)]
pub struct FontChoice {
    /// The family name, which also names the font to egui.
    pub family: String,
    /// A TTF or OTF file to use instead of `bundled`. A relative path is
    /// looked for next to the executable, then in the source checkout it was
    /// built from. If it can't be read, `bundled` is used instead.
    pub path: Option<PathBuf>,
    /// The font built into the executable, used when there's no `path` or it
    /// can't be read. Without either, egui's built-in font is used.
    pub bundled: Option<&'static [u8]>,
}

/// Inter, built in so installed copies have it: they ship as a lone
/// executable.
const INTER: &[u8] = include_bytes!("../assets/fonts/Inter-Regular.ttf");
/// JetBrains Mono, built in like Inter.
const JETBRAINS_MONO: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf");

/// Gaps and padding, in logical pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Spacing {
    /// Between neighbouring panels. Each panel keeps half of it as an outer
    /// margin.
    pub panel_gap: f32,
    /// Between a panel's (or window's) edge and its contents.
    pub panel_padding: f32,
    /// Between a settings box's edge and its contents.
    pub box_padding: f32,
    /// Between widgets, horizontally and vertically.
    pub item_spacing: Vec2,
    /// Between a button's edge and its text.
    pub button_padding: Vec2,
    /// The minimum height of buttons, fields and other interactive widgets.
    pub interact_height: f32,
    /// How far indented and collapsible sections step in.
    pub indent: f32,
}

/// Corner radii and shadows.
#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    /// Corners of panels, windows and popups.
    pub panel_radius: u8,
    /// Corners of buttons, fields and settings boxes.
    pub widget_radius: u8,
    /// Under floating windows.
    pub window_shadow: Shadow,
    /// Under popups and menus, such as combo box lists and tooltips.
    pub popup_shadow: Shadow,
}

impl Default for Theme {
    /// Graphite.
    fn default() -> Self {
        Self {
            name: "Graphite".to_owned(),
            colors: Colors {
                gutter: Color32::from_rgb(0x0A, 0x0A, 0x0A),
                panel: Color32::from_rgb(0x26, 0x26, 0x26),
                header: Color32::from_rgb(0x1E, 0x1E, 0x1E),
                box_fill: Color32::from_rgb(0x2E, 0x2E, 0x2E),
                viewport: Color32::from_rgb(0x1F, 0x1F, 0x1F),
                control: Color32::from_rgb(0x3D, 0x3D, 0x3D),
                field: Color32::from_rgb(0x3A, 0x3A, 0x3A),
                hover: Color32::from_rgb(0x47, 0x47, 0x47),
                stroke: Color32::from_rgb(0x2F, 0x2F, 0x2F),
                strong_stroke: Color32::from_rgb(0x4A, 0x4A, 0x4A),
                text: Color32::from_rgb(0xE2, 0xE2, 0xE2),
                text_secondary: Color32::from_rgb(0xAB, 0xAB, 0xAB),
                text_tertiary: Color32::from_rgb(0x90, 0x90, 0x90),
                active: Color32::from_rgb(0x3F, 0x6F, 0xC4),
                active_ink: Color32::from_rgb(0xFF, 0xFF, 0xFF),
                active_text: Color32::from_rgb(0x7F, 0xA3, 0xE6),
                selection: Color32::from_rgb(0xF0, 0x92, 0x2E),
                info: Color32::from_rgb(0x7F, 0xA3, 0xE6),
                warn: Color32::from_rgb(0xE0, 0xA3, 0x3A),
                error: Color32::from_rgb(0xE5, 0x67, 0x5A),
                data_red: Color32::from_rgb(0xE5, 0x67, 0x5A),
                data_green: Color32::from_rgb(0x6C, 0xC0, 0x5A),
                data_blue: Color32::from_rgb(0x6E, 0xA8, 0xFE),
            },
            fonts: Fonts {
                ui: FontChoice {
                    family: "Inter".to_owned(),
                    path: None,
                    bundled: Some(INTER),
                },
                monospace: FontChoice {
                    family: "JetBrains Mono".to_owned(),
                    path: None,
                    bundled: Some(JETBRAINS_MONO),
                },
                body_size: 13.0,
                button_size: 13.0,
                small_size: 11.0,
                heading_size: 15.0,
                monospace_size: 12.0,
            },
            spacing: Spacing {
                panel_gap: 4.0,
                panel_padding: 8.0,
                box_padding: 6.0,
                item_spacing: egui::vec2(8.0, 6.0),
                button_padding: egui::vec2(8.0, 4.0),
                interact_height: 22.0,
                indent: 12.0,
            },
            shape: Shape {
                panel_radius: 6,
                widget_radius: 4,
                window_shadow: Shadow::NONE,
                popup_shadow: Shadow {
                    offset: [0, 4],
                    blur: 12,
                    spread: 0,
                    color: Color32::from_black_alpha(96),
                },
            },
        }
    }
}

/// Every built-in theme: the name a settings file gives (lowercase), and
/// how to make it. Adding a theme is one more entry.
pub const THEMES: &[(&str, MakeTheme)] = &[("graphite", Theme::default)];

/// Builds a built-in theme.
pub type MakeTheme = fn() -> Theme;

impl Theme {
    /// The built-in theme a settings file calls `name`, ignoring case.
    pub fn by_name(name: &str) -> Option<Self> {
        THEMES
            .iter()
            .find(|(known, _)| known.eq_ignore_ascii_case(name.trim()))
            .map(|(_, make)| make())
    }

    /// Styles every widget, window and panel `ctx` draws from now on. Cheap
    /// enough to call again whenever the theme changes: fonts are only
    /// reloaded when the font choices differ from the last call's.
    pub fn apply(&self, ctx: &egui::Context) {
        // Graphite is dark, and stays so whatever the OS prefers.
        ctx.set_theme(egui::Theme::Dark);
        ctx.set_style_of(egui::Theme::Dark, self.style());
        self.apply_fonts(ctx);
    }

    /// egui's dark style with this theme's values in place of its own.
    fn style(&self) -> egui::Style {
        let mut style = egui::Style {
            visuals: self.visuals(),
            ..egui::Style::default()
        };

        let spacing = &self.spacing;
        style.spacing.item_spacing = spacing.item_spacing;
        style.spacing.button_padding = spacing.button_padding;
        style.spacing.interact_size.y = spacing.interact_height;
        style.spacing.indent = spacing.indent;
        style.spacing.window_margin = margin(spacing.panel_padding);

        let fonts = &self.fonts;
        style.text_styles = [
            (TextStyle::Small, FontId::proportional(fonts.small_size)),
            (TextStyle::Body, FontId::proportional(fonts.body_size)),
            (TextStyle::Button, FontId::proportional(fonts.button_size)),
            (TextStyle::Heading, FontId::proportional(fonts.heading_size)),
            (
                TextStyle::Monospace,
                FontId::monospace(fonts.monospace_size),
            ),
        ]
        .into();
        // Drag values and slider readouts are numbers.
        style.drag_value_text_style = TextStyle::Monospace;
        style
    }

    fn visuals(&self) -> egui::Visuals {
        let c = &self.colors;
        let widget_radius = CornerRadius::same(self.shape.widget_radius);
        let panel_radius = CornerRadius::same(self.shape.panel_radius);
        let mut visuals = egui::Visuals::dark();

        let widgets = &mut visuals.widgets;
        // Labels, separators, grid lines and indent guides.
        widgets.noninteractive.bg_fill = c.panel;
        widgets.noninteractive.weak_bg_fill = c.panel;
        widgets.noninteractive.bg_stroke = Stroke::new(1.0, c.stroke);
        widgets.noninteractive.fg_stroke.color = c.text;
        // At rest. `bg_fill` is what must be filled (checkboxes, radio
        // buttons, slider rails); `weak_bg_fill` is buttons and combo boxes.
        widgets.inactive.bg_fill = c.field;
        widgets.inactive.weak_bg_fill = c.control;
        widgets.inactive.bg_stroke = Stroke::new(1.0, c.strong_stroke);
        widgets.inactive.fg_stroke.color = c.text;
        widgets.hovered.bg_fill = c.hover;
        widgets.hovered.weak_bg_fill = c.hover;
        widgets.hovered.bg_stroke = Stroke::new(1.0, c.strong_stroke);
        widgets.hovered.fg_stroke.color = c.text;
        // Pressed, or focused by keyboard.
        widgets.active.bg_fill = c.active;
        widgets.active.weak_bg_fill = c.active;
        widgets.active.bg_stroke = Stroke::new(1.0, c.active_text);
        widgets.active.fg_stroke.color = c.active_ink;
        // A combo box with its list open. egui also fills the topmost
        // window's title bar with `weak_bg_fill`, so it's the header colour.
        widgets.open.bg_fill = c.control;
        widgets.open.weak_bg_fill = c.header;
        widgets.open.bg_stroke = Stroke::new(1.0, c.strong_stroke);
        widgets.open.fg_stroke.color = c.text;
        for widget in [
            &mut widgets.noninteractive,
            &mut widgets.inactive,
            &mut widgets.hovered,
            &mut widgets.active,
            &mut widgets.open,
        ] {
            widget.corner_radius = widget_radius;
        }

        visuals.selection.bg_fill = c.active;
        visuals.selection.stroke = Stroke::new(1.0, c.active_ink);
        visuals.slider_trailing_fill = true;
        visuals.text_cursor.stroke.color = c.active_text;
        visuals.hyperlink_color = c.active_text;
        visuals.weak_text_color = Some(c.text_secondary);
        visuals.warn_fg_color = c.warn;
        visuals.error_fg_color = c.error;

        visuals.text_edit_bg_color = Some(c.field);
        // Scroll bar troughs, progress bars and canvases.
        visuals.extreme_bg_color = c.viewport;
        // The darker rows of striped grids, which sit in settings boxes.
        visuals.faint_bg_color = c.panel;

        visuals.panel_fill = c.gutter;
        visuals.window_fill = c.panel;
        visuals.window_stroke = Stroke::new(1.0, c.stroke);
        visuals.window_corner_radius = panel_radius;
        visuals.window_shadow = self.shape.window_shadow;
        visuals.menu_corner_radius = panel_radius;
        visuals.popup_shadow = self.shape.popup_shadow;
        visuals
    }

    /// Loads the fonts, putting each in front of egui's built-in font for its
    /// family. The built-in fonts stay as fallbacks for missing glyphs, and
    /// stand in completely for a font that's neither readable nor bundled.
    fn apply_fonts(&self, ctx: &egui::Context) {
        let chosen = (self.fonts.ui.clone(), self.fonts.monospace.clone());
        let id = egui::Id::new("copcat_theme_fonts");
        let last = ctx.data_mut(|data| data.get_temp::<(FontChoice, FontChoice)>(id));
        if last.as_ref() == Some(&chosen) {
            return;
        }

        let mut definitions = FontDefinitions::default();
        for (choice, family) in [
            (&chosen.0, FontFamily::Proportional),
            (&chosen.1, FontFamily::Monospace),
        ] {
            let data = match (choice.path.as_deref().and_then(read_font), choice.bundled) {
                (Some(bytes), _) => FontData::from_owned(bytes),
                (None, Some(bytes)) => FontData::from_static(bytes),
                (None, None) => continue,
            };
            definitions
                .font_data
                .insert(choice.family.clone(), data.into());
            definitions
                .families
                .entry(family)
                .or_default()
                .insert(0, choice.family.clone());
        }
        ctx.set_fonts(definitions);
        ctx.data_mut(|data| data.insert_temp(id, chosen));
    }

    /// A panel: rounded, padded, and inset by half the gap between panels.
    pub fn panel_frame(&self) -> egui::Frame {
        egui::Frame::new()
            .fill(self.colors.panel)
            .corner_radius(self.shape.panel_radius)
            .inner_margin(margin(self.spacing.panel_padding))
            .outer_margin(self.panel_outer_margin())
    }

    /// A panel that's a header strip, such as the toolbar.
    pub fn header_frame(&self) -> egui::Frame {
        self.panel_frame().fill(self.colors.header)
    }

    /// Half the gap between panels, kept around each one.
    pub fn panel_outer_margin(&self) -> Margin {
        margin(self.spacing.panel_gap / 2.0)
    }

    /// A box grouping settings inside a panel.
    pub fn box_frame(&self) -> egui::Frame {
        egui::Frame::new()
            .fill(self.colors.box_fill)
            .corner_radius(self.shape.widget_radius)
            .inner_margin(margin(self.spacing.box_padding))
    }

    /// The 3D view's background, as the renderer takes it.
    pub fn viewport_background(&self) -> [u8; 3] {
        let c = self.colors.viewport;
        [c.r(), c.g(), c.b()]
    }

    /// The X, Y and Z axis colours.
    pub fn axis_colors(&self) -> [Color32; 3] {
        [
            self.colors.data_red,
            self.colors.data_green,
            self.colors.data_blue,
        ]
    }
}

/// The same margin on every side, rounded to whole points as egui wants.
fn margin(points: f32) -> Margin {
    Margin::same(points.round() as i8)
}

/// Reads a font file. A relative path is tried next to the executable, then
/// in the source checkout, so `cargo run` finds `assets/fonts`.
fn read_font(path: &Path) -> Option<Vec<u8>> {
    if path.is_absolute() {
        return std::fs::read(path).ok();
    }
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    exe_dir
        .into_iter()
        .chain([PathBuf::from(env!("CARGO_MANIFEST_DIR"))])
        .find_map(|dir| std::fs::read(dir.join(path)).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_outer_margin_should_be_half_the_gap() {
        assert_eq!(Theme::default().panel_outer_margin(), Margin::same(2));
    }

    #[test]
    fn bundled_fonts_should_be_truetype() {
        let fonts = Theme::default().fonts;
        for choice in [fonts.ui, fonts.monospace] {
            let bytes = choice.bundled.expect("Graphite bundles its fonts");
            assert_eq!(bytes[..4], [0, 1, 0, 0], "{} isn't a TTF", choice.family);
        }
    }

    #[test]
    fn themes_should_be_found_by_name_ignoring_case() {
        assert_eq!(Theme::by_name("Graphite").unwrap().name, "Graphite");
        assert!(Theme::by_name("solarized").is_none());
    }

    #[test]
    fn theme_names_should_be_lowercase_and_match_their_theme() {
        for (name, make) in THEMES {
            assert_eq!(*name, name.to_lowercase());
            assert_eq!(make().name.to_lowercase(), *name);
        }
    }

    #[test]
    fn missing_font_file_should_read_as_none() {
        assert!(read_font(Path::new("assets/fonts/No-Such-Font.ttf")).is_none());
    }
}
