//! COPC file locations, metadata and coordinate conversion.

use std::path::PathBuf;
use std::time::Duration;

use copc_streaming::{CopcError, CopcHeader};
use glam::Vec3;

/// Where a COPC file lives, parsed from the text the user pasted.
#[derive(Debug, PartialEq)]
pub enum Location {
    Http(String),
    File(PathBuf),
}

impl Location {
    /// Accepts `http(s)://` URLs, `file://` URLs and plain filesystem paths.
    /// Surrounding quotes are stripped because Windows "Copy as path" adds them.
    pub fn parse(input: &str) -> Result<Self, LoadError> {
        let input = input.trim().trim_matches('"');
        if input.is_empty() {
            return Err(LoadError::EmptyLocation);
        }
        if input.starts_with("http://") || input.starts_with("https://") {
            return Ok(Self::Http(input.to_owned()));
        }
        if input.starts_with("file://") {
            let url =
                url::Url::parse(input).map_err(|_| LoadError::InvalidFileUrl(input.to_owned()))?;
            let path = url
                .to_file_path()
                .map_err(|()| LoadError::InvalidFileUrl(input.to_owned()))?;
            return Ok(Self::File(path));
        }
        Ok(Self::File(PathBuf::from(input)))
    }
}

/// Summary of an opened COPC file, from its header.
pub struct CloudInfo {
    pub source: String,
    pub total_points: u64,
    pub bounds_min: [f64; 3],
    pub bounds_max: [f64; 3],
    /// Centre of the COPC octree. Every position on the GPU is relative to this.
    ///
    /// Georeferenced coordinates (e.g. UTM eastings of ~600,000 m) only keep
    /// ~6 cm of precision in an `f32`, which the GPU needs. Subtracting the
    /// origin in `f64` first keeps sub-millimetre precision near the site.
    pub origin: [f64; 3],
    /// Time to read the header and the whole octree hierarchy.
    pub open_time: Duration,
    /// Whether the point format stores RGB colour.
    pub has_rgb: bool,
}

impl CloudInfo {
    pub fn from_header(header: &CopcHeader, source: &str, open_time: Duration) -> Self {
        let las_header = header.las_header();
        let bounds = las_header.bounds();
        Self {
            source: source.trim().to_owned(),
            total_points: las_header.number_of_points(),
            bounds_min: [bounds.min.x, bounds.min.y, bounds.min.z],
            bounds_max: [bounds.max.x, bounds.max.y, bounds.max.z],
            origin: header.copc_info().center,
            open_time,
            has_rgb: las_header.point_format().has_color,
        }
    }

    /// The file's bounding box relative to `origin`, in the same space as the points.
    pub fn local_bounds(&self) -> (Vec3, Vec3) {
        let to_local = |p: [f64; 3]| Vec3::from_array(to_local(p, self.origin));
        (to_local(self.bounds_min), to_local(self.bounds_max))
    }
}

/// Converts a world position to `f32` metres relative to `origin`.
pub fn to_local(position: [f64; 3], origin: [f64; 3]) -> [f32; 3] {
    let [x, y, z] = position;
    let [ox, oy, oz] = origin;
    [(x - ox) as f32, (y - oy) as f32, (z - oz) as f32]
}

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("no file path or URL given")]
    EmptyLocation,
    #[error("not a valid file URL: {0}")]
    InvalidFileUrl(String),
    #[error("could not open {path}: {source}")]
    OpenFile { path: PathBuf, source: CopcError },
    #[error(transparent)]
    Copc(#[from] CopcError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_should_keep_https_urls_as_http() {
        let location = Location::parse("https://example.com/a.copc.laz").unwrap();
        assert_eq!(
            location,
            Location::Http("https://example.com/a.copc.laz".into())
        );
    }

    #[test]
    fn parse_should_strip_quotes_from_windows_copy_as_path() {
        let location = Location::parse(r#""C:\data\site 1.copc.laz""#).unwrap();
        assert_eq!(
            location,
            Location::File(PathBuf::from(r"C:\data\site 1.copc.laz"))
        );
    }

    #[cfg(windows)]
    #[test]
    fn parse_should_decode_percent_escapes_in_file_urls() {
        let location = Location::parse("file:///C:/data/site%201.copc.laz").unwrap();
        assert_eq!(
            location,
            Location::File(PathBuf::from(r"C:\data\site 1.copc.laz"))
        );
    }

    #[test]
    fn parse_should_reject_blank_input() {
        let error = Location::parse("   ").unwrap_err();
        assert_eq!(error.to_string(), "no file path or URL given");
    }

    #[test]
    fn to_local_should_keep_millimetres_at_utm_scale() {
        let [x, _, _] = to_local([600_000.123, 0.0, 0.0], [600_000.0, 0.0, 0.0]);
        assert!((x - 0.123).abs() < 1e-6, "{x}");
    }
}
