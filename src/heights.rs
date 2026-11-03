//! The heights points can be drawn at, and where in the file each comes from.
//!
//! A cloud can store elevation in Z, height above ground in Z (a "normalised"
//! cloud), or one in Z and the other in an extra bytes dimension: a named
//! per-point value that LAS 1.4 stores after the standard fields, described
//! by the Extra Bytes VLR.

use std::fmt;
use std::ops::{Index, IndexMut};

use clap::{Args, ValueEnum};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, ValueEnum)]
pub enum HeightKind {
    #[default]
    Elevation,
    /// Height above the ground, as made by e.g. PDAL's `filters.hag_nn`.
    #[value(name = "hag")]
    AboveGround,
}

impl HeightKind {
    pub const ALL: [Self; 2] = [Self::Elevation, Self::AboveGround];

    pub fn label(self) -> &'static str {
        match self {
            Self::Elevation => "Elevation",
            Self::AboveGround => "Height above ground",
        }
    }

    fn other(self) -> Self {
        match self {
            Self::Elevation => Self::AboveGround,
            Self::AboveGround => Self::Elevation,
        }
    }

    fn index(self) -> usize {
        match self {
            Self::Elevation => 0,
            Self::AboveGround => 1,
        }
    }
}

/// One `T` for each [`HeightKind`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PerHeight<T>(pub [T; 2]);

impl<T> PerHeight<T> {
    pub fn from_fn(mut f: impl FnMut(HeightKind) -> T) -> Self {
        Self(HeightKind::ALL.map(&mut f))
    }

    pub fn map<U>(&self, mut f: impl FnMut(HeightKind, &T) -> U) -> PerHeight<U> {
        PerHeight::from_fn(|kind| f(kind, &self[kind]))
    }
}

impl<T> Index<HeightKind> for PerHeight<T> {
    type Output = T;
    fn index(&self, kind: HeightKind) -> &T {
        &self.0[kind.index()]
    }
}

impl<T> IndexMut<HeightKind> for PerHeight<T> {
    fn index_mut(&mut self, kind: HeightKind) -> &mut T {
        &mut self.0[kind.index()]
    }
}

/// Where a height's values come from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HeightSource {
    /// The points' Z, which the COPC octree is built on.
    Z,
    /// An extra bytes dimension, by name.
    Extra(String),
}

impl fmt::Display for HeightSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Z => f.write_str("Z"),
            Self::Extra(name) => f.write_str(name),
        }
    }
}

/// Where each height is, once worked out for a file. `None` for a height
/// the file doesn't have; at least one is `Some`.
pub type HeightSources = PerHeight<Option<HeightSource>>;

/// Where the user said a height comes from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum SourceSetting {
    /// Worked out from the names of the file's extra dimensions.
    #[default]
    Auto,
    /// The file doesn't have this height.
    Absent,
    From(HeightSource),
}

impl fmt::Display for SourceSetting {
    /// As `parse_setting` reads it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => f.write_str("auto"),
            Self::Absent => f.write_str("none"),
            Self::From(HeightSource::Z) => f.write_str("z"),
            Self::From(HeightSource::Extra(name)) => f.write_str(name),
        }
    }
}

/// `z`, `none`, `auto` (any case), or else the name of an extra dimension.
fn parse_setting(text: &str) -> Result<SourceSetting, String> {
    let text = text.trim();
    Ok(match text.to_ascii_lowercase().as_str() {
        "" => return Err("expected z, none, auto or a dimension name".to_owned()),
        "auto" => SourceSetting::Auto,
        "none" => SourceSetting::Absent,
        "z" => SourceSetting::From(HeightSource::Z),
        _ => SourceSetting::From(HeightSource::Extra(text.to_owned())),
    })
}

/// Where elevation and height above ground come from, as given on the
/// command line or in the viewer.
#[derive(Args, Clone, Debug, Default, PartialEq, Eq)]
#[command(next_help_heading = "Heights")]
pub struct HeightSettings {
    /// Where elevation comes from: z, the name of an extra dimension, or none.
    /// By default, Z, unless the file has an extra dimension named like
    /// Elevation but none named like HeightAboveGround.
    #[arg(long, value_name = "z|DIMENSION|none", value_parser = parse_setting, default_value = "auto")]
    pub elevation: SourceSetting,
    /// Where height above ground comes from: z, the name of an extra
    /// dimension, or none. By default, an extra dimension named like
    /// HeightAboveGround or HAG, or else Z if elevation is elsewhere.
    #[arg(long, value_name = "z|DIMENSION|none", value_parser = parse_setting, default_value = "auto")]
    pub hag: SourceSetting,
}

/// Extra dimension names, normalised by `normalise`, that auto picks.
const ELEVATION_NAMES: [&str; 5] = ["elevation", "altitude", "zorig", "zoriginal", "originalz"];
const ABOVE_GROUND_NAMES: [&str; 3] = ["heightaboveground", "hag", "normalizedheight"];

/// Lower case letters and digits only, so `Height_Above_Ground` matches.
fn normalise(name: &str) -> String {
    name.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

impl HeightSettings {
    pub fn get(&self, kind: HeightKind) -> &SourceSetting {
        match kind {
            HeightKind::Elevation => &self.elevation,
            HeightKind::AboveGround => &self.hag,
        }
    }

    pub fn get_mut(&mut self, kind: HeightKind) -> &mut SourceSetting {
        match kind {
            HeightKind::Elevation => &mut self.elevation,
            HeightKind::AboveGround => &mut self.hag,
        }
    }

    /// Where each height is in a file whose readable extra dimensions are
    /// called `dimensions`.
    pub fn resolve(&self, dimensions: &[String]) -> Result<HeightSources, String> {
        for kind in HeightKind::ALL {
            if let SourceSetting::From(HeightSource::Extra(name)) = self.get(kind)
                && !dimensions.contains(name)
            {
                let have = if dimensions.is_empty() {
                    "it has none".to_owned()
                } else {
                    format!("it has {}", dimensions.join(", "))
                };
                return Err(format!(
                    "the file has no numeric extra dimension called {name} ({have})"
                ));
            }
        }
        let found = PerHeight::from_fn(|kind| {
            let names: &[&str] = match kind {
                HeightKind::Elevation => &ELEVATION_NAMES,
                HeightKind::AboveGround => &ABOVE_GROUND_NAMES,
            };
            dimensions
                .iter()
                .find(|name| names.contains(&normalise(name).as_str()))
                .cloned()
        });
        let z_holds = self.z_holds(&found)?;
        let sources = PerHeight::from_fn(|kind| match self.get(kind) {
            SourceSetting::Absent => None,
            SourceSetting::From(source) => Some(source.clone()),
            SourceSetting::Auto if z_holds == Some(kind) => Some(HeightSource::Z),
            SourceSetting::Auto => found[kind].clone().map(HeightSource::Extra),
        });
        if sources.0.iter().all(Option::is_none) {
            return Err("there's no height to draw the points at: set --elevation or --hag".into());
        }
        Ok(sources)
    }

    /// Which height Z holds. `found` is the extra dimension auto would pick
    /// for each height.
    fn z_holds(&self, found: &PerHeight<Option<String>>) -> Result<Option<HeightKind>, String> {
        let is_z = |kind| *self.get(kind) == SourceSetting::From(HeightSource::Z);
        match HeightKind::ALL.map(is_z) {
            [true, true] => return Err("Z can't be both elevation and height above ground".into()),
            [true, false] => return Ok(Some(HeightKind::Elevation)),
            [false, true] => return Ok(Some(HeightKind::AboveGround)),
            [false, false] => {}
        }
        // A height that's elsewhere (or missing) leaves Z to the other.
        for kind in HeightKind::ALL {
            if *self.get(kind) != SourceSetting::Auto
                && *self.get(kind.other()) == SourceSetting::Auto
            {
                return Ok(Some(kind.other()));
            }
        }
        if self.elevation != SourceSetting::Auto {
            // Both are set, and neither to Z.
            return Ok(None);
        }
        // A normalised cloud that kept its elevation in an extra dimension.
        let flipped =
            found[HeightKind::Elevation].is_some() && found[HeightKind::AboveGround].is_none();
        Ok(Some(if flipped {
            HeightKind::AboveGround
        } else {
            HeightKind::Elevation
        }))
    }
}

/// One dimension described by the Extra Bytes VLR.
#[derive(Clone, Debug, PartialEq)]
pub struct ExtraDimension {
    pub name: String,
    /// Where its value starts in each point's extra bytes.
    byte_offset: usize,
    /// `None` for types we can't read as a single number: undocumented
    /// bytes, and the deprecated 2 and 3 element arrays.
    kind: Option<NumberKind>,
    scale: f64,
    offset: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NumberKind {
    U8,
    I8,
    U16,
    I16,
    U32,
    I32,
    U64,
    I64,
    F32,
    F64,
}

impl NumberKind {
    /// LAS 1.4 data types 1 to 10, in order.
    const BY_TYPE: [Self; 10] = [
        Self::U8,
        Self::I8,
        Self::U16,
        Self::I16,
        Self::U32,
        Self::I32,
        Self::U64,
        Self::I64,
        Self::F32,
        Self::F64,
    ];

    fn size(self) -> usize {
        match self {
            Self::U8 | Self::I8 => 1,
            Self::U16 | Self::I16 => 2,
            Self::U32 | Self::I32 | Self::F32 => 4,
            Self::U64 | Self::I64 | Self::F64 => 8,
        }
    }

    /// Reads a little-endian value from the start of `bytes`, which must be
    /// at least `size` long.
    fn read(self, bytes: &[u8]) -> f64 {
        // Padded with zeros past `size`, which each type below ignores.
        let mut padded = [0; 8];
        padded[..self.size()].copy_from_slice(&bytes[..self.size()]);
        let [b0, b1, b2, b3, ..] = padded;
        match self {
            Self::U8 => f64::from(b0),
            Self::I8 => f64::from(b0 as i8),
            Self::U16 => f64::from(u16::from_le_bytes([b0, b1])),
            Self::I16 => f64::from(i16::from_le_bytes([b0, b1])),
            Self::U32 => f64::from(u32::from_le_bytes([b0, b1, b2, b3])),
            Self::I32 => f64::from(i32::from_le_bytes([b0, b1, b2, b3])),
            Self::F32 => f64::from(f32::from_le_bytes([b0, b1, b2, b3])),
            Self::U64 => u64::from_le_bytes(padded) as f64,
            Self::I64 => i64::from_le_bytes(padded) as f64,
            Self::F64 => f64::from_le_bytes(padded),
        }
    }
}

/// The Extra Bytes VLR's user ID and record ID.
pub const EXTRA_BYTES_USER_ID: &str = "LASF_Spec";
pub const EXTRA_BYTES_RECORD_ID: u16 = 4;
const DESCRIPTOR_LEN: usize = 192;

/// The dimensions an Extra Bytes VLR's `data` describes, in the order they're
/// stored in each point.
pub fn parse_extra_bytes(data: &[u8]) -> Vec<ExtraDimension> {
    let mut byte_offset = 0;
    data.as_chunks::<DESCRIPTOR_LEN>()
        .0
        .iter()
        .map(|descriptor| {
            let data_type = descriptor[2];
            let options = descriptor[3];
            let name_bytes = &descriptor[4..36];
            let name_len = name_bytes.iter().position(|&b| b == 0).unwrap_or(32);
            let name = String::from_utf8_lossy(&name_bytes[..name_len])
                .trim()
                .to_owned();
            let f64_at =
                |at: usize| f64::from_le_bytes(descriptor[at..at + 8].try_into().expect("8 bytes"));
            // Scale and offset are only used when their option bits are set.
            let scale = if options & 0b1000 != 0 {
                f64_at(112)
            } else {
                1.0
            };
            let offset = if options & 0b1_0000 != 0 {
                f64_at(136)
            } else {
                0.0
            };
            let (kind, size) = match data_type {
                // Undocumented bytes: the options field is their count.
                0 => (None, usize::from(options)),
                1..=10 => {
                    let kind = NumberKind::BY_TYPE[usize::from(data_type - 1)];
                    (Some(kind), kind.size())
                }
                // Deprecated arrays of 2 (11 to 20) or 3 (21 to 30) numbers.
                11..=30 => {
                    let count = if data_type <= 20 { 2 } else { 3 };
                    let base = NumberKind::BY_TYPE[usize::from((data_type - 1) % 10)];
                    (None, base.size() * count)
                }
                _ => (None, 0),
            };
            let dimension = ExtraDimension {
                name,
                byte_offset,
                kind,
                scale,
                offset,
            };
            byte_offset += size;
            dimension
        })
        .collect()
}

impl ExtraDimension {
    /// Whether `read` works: it's a single number.
    pub fn is_number(&self) -> bool {
        self.kind.is_some()
    }

    /// This dimension's value in one point's `extra_bytes`, scaled and
    /// offset. `None` if it isn't a number or the bytes are too short.
    pub fn read(&self, extra_bytes: &[u8]) -> Option<f64> {
        let kind = self.kind?;
        let bytes = extra_bytes.get(self.byte_offset..self.byte_offset + kind.size())?;
        Some(kind.read(bytes) * self.scale + self.offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor(name: &str, data_type: u8, scale: Option<f64>) -> Vec<u8> {
        let mut bytes = vec![0; DESCRIPTOR_LEN];
        bytes[2] = data_type;
        bytes[4..4 + name.len()].copy_from_slice(name.as_bytes());
        if let Some(scale) = scale {
            bytes[3] |= 0b1000;
            bytes[112..120].copy_from_slice(&scale.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn parse_extra_bytes_should_place_each_dimension_after_the_last() {
        let data = [
            descriptor("Flag", 1, None),
            descriptor("HeightAboveGround", 9, None),
        ]
        .concat();
        let dimensions = parse_extra_bytes(&data);
        assert_eq!(dimensions[1].name, "HeightAboveGround");
        let extra_bytes = [&[7][..], &12.5_f32.to_le_bytes()].concat();
        assert_eq!(dimensions[1].read(&extra_bytes), Some(12.5));
    }

    #[test]
    fn read_should_apply_the_scale() {
        let dimensions = parse_extra_bytes(&descriptor("HAG", 4, Some(0.01)));
        let value = dimensions[0].read(&(-250_i16).to_le_bytes()).unwrap();
        assert!((value + 2.5).abs() < 1e-9, "{value}");
    }

    #[test]
    fn read_should_skip_undocumented_bytes_and_arrays() {
        let mut undocumented = descriptor("Blob", 0, None);
        undocumented[3] = 3;
        let data = [
            undocumented,
            descriptor("Pair", 13, None),
            descriptor("Z_orig", 10, None),
        ]
        .concat();
        let dimensions = parse_extra_bytes(&data);
        assert!(!dimensions[0].is_number() && !dimensions[1].is_number());
        // 3 undocumented bytes, then two u16s.
        let extra_bytes = [&[0; 7][..], &401.25_f64.to_le_bytes()].concat();
        assert_eq!(dimensions[2].read(&extra_bytes), Some(401.25));
    }

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    fn settings(elevation: &str, hag: &str) -> HeightSettings {
        HeightSettings {
            elevation: parse_setting(elevation).unwrap(),
            hag: parse_setting(hag).unwrap(),
        }
    }

    fn extra(name: &str) -> Option<HeightSource> {
        Some(HeightSource::Extra(name.to_owned()))
    }

    #[test]
    fn resolve_should_take_z_as_elevation_by_default() {
        let sources = settings("auto", "auto").resolve(&[]).unwrap();
        assert_eq!(sources, PerHeight([Some(HeightSource::Z), None]));
    }

    #[test]
    fn resolve_should_find_height_above_ground_by_name() {
        let sources = settings("auto", "auto")
            .resolve(&names(&["Height_Above_Ground"]))
            .unwrap();
        assert_eq!(
            sources,
            PerHeight([Some(HeightSource::Z), extra("Height_Above_Ground")])
        );
    }

    #[test]
    fn resolve_should_read_a_lone_elevation_dimension_as_a_normalised_cloud() {
        let sources = settings("auto", "auto")
            .resolve(&names(&["Elevation"]))
            .unwrap();
        assert_eq!(
            sources,
            PerHeight([extra("Elevation"), Some(HeightSource::Z)])
        );
    }

    #[test]
    fn resolve_should_give_z_to_the_other_height_when_one_is_elsewhere() {
        let sources = settings("none", "auto").resolve(&[]).unwrap();
        assert_eq!(sources, PerHeight([None, Some(HeightSource::Z)]));
        let sources = settings("auto", "Canopy")
            .resolve(&names(&["Canopy"]))
            .unwrap();
        assert_eq!(sources, PerHeight([Some(HeightSource::Z), extra("Canopy")]));
    }

    #[test]
    fn resolve_should_reject_a_missing_dimension() {
        let error = settings("auto", "HAG")
            .resolve(&names(&["Other"]))
            .unwrap_err();
        assert!(
            error.contains("no numeric extra dimension called HAG"),
            "{error}"
        );
    }

    #[test]
    fn resolve_should_reject_z_for_both() {
        assert!(settings("z", "z").resolve(&[]).is_err());
    }

    #[test]
    fn resolve_should_reject_no_height_at_all() {
        assert!(settings("none", "none").resolve(&[]).is_err());
    }

    #[test]
    fn settings_should_print_as_they_parse() {
        for text in ["auto", "none", "z", "HeightAboveGround"] {
            assert_eq!(parse_setting(text).unwrap().to_string(), text);
        }
    }
}
