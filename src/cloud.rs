//! Loading COPC point clouds from local files or HTTP(S) URLs.

use std::fs::File;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use copc_reader::{
    BoundsSelection, ColumnData, ColumnSelection, CopcRangeReader, HttpRangeReader,
    LasColumnBatch, LasDimension, LodSelection, PointQuery, RangeRead,
};

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
            let url = url::Url::parse(input)
                .map_err(|_| LoadError::InvalidFileUrl(input.to_owned()))?;
            let path = url
                .to_file_path()
                .map_err(|()| LoadError::InvalidFileUrl(input.to_owned()))?;
            return Ok(Self::File(path));
        }
        Ok(Self::File(PathBuf::from(input)))
    }
}

/// The subset of a COPC file the viewer has loaded.
pub struct PointCloud {
    pub source: String,
    pub total_points: u64,
    pub bounds_min: [f64; 3],
    pub bounds_max: [f64; 3],
    /// Centre of the COPC octree. `positions` are relative to this.
    pub origin: [f64; 3],
    /// Point positions in metres, relative to `origin`.
    ///
    /// Georeferenced coordinates (e.g. UTM eastings of ~600,000 m) only keep
    /// ~6 cm of precision in an `f32`, which the GPU needs. Subtracting the
    /// origin in `f64` first keeps sub-millimetre precision near the site.
    pub positions: Vec<[f32; 3]>,
    pub load_time: Duration,
}

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("no file path or URL given")]
    EmptyLocation,
    #[error("not a valid file URL: {0}")]
    InvalidFileUrl(String),
    #[error("could not open {path}: {source}")]
    OpenFile {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error(transparent)]
    Copc(#[from] copc_core::Error),
    #[error("point data has no {0:?} column of 64-bit floats")]
    MissingColumn(LasDimension),
}

/// Loads the header and the root octree node (level 0) of a COPC file.
///
/// This blocks on file or network IO, so call it off the UI thread.
pub fn load_root_node(input: &str) -> Result<PointCloud, LoadError> {
    let started = Instant::now();
    let mut cloud = match Location::parse(input)? {
        Location::Http(url) => read_root_node(HttpRangeReader::new(url))?,
        Location::File(path) => {
            let file = File::open(&path).map_err(|source| LoadError::OpenFile {
                path: path.clone(),
                source,
            })?;
            read_root_node(file)?
        }
    };
    cloud.source = input.trim().to_owned();
    cloud.load_time = started.elapsed();
    Ok(cloud)
}

/// Generic over the byte source so local files and HTTP share one code path.
fn read_root_node<S: RangeRead>(source: S) -> Result<PointCloud, LoadError> {
    let mut reader = CopcRangeReader::open(source)?;

    let header = reader.header();
    let total_points = header.number_of_points;
    let bounds_min = [header.min_x, header.min_y, header.min_z];
    let bounds_max = [header.max_x, header.max_y, header.max_z];
    let (cx, cy, cz) = reader.copc_info().center;

    let root_only = PointQuery::new(LodSelection::Level(0), BoundsSelection::All);
    let batch = reader.read_columns(root_only, ColumnSelection::xyz())?;

    let xs = f64_column(&batch, LasDimension::X)?;
    let ys = f64_column(&batch, LasDimension::Y)?;
    let zs = f64_column(&batch, LasDimension::Z)?;
    let positions = xs
        .iter()
        .zip(ys)
        .zip(zs)
        .map(|((x, y), z)| [(x - cx) as f32, (y - cy) as f32, (z - cz) as f32])
        .collect();

    Ok(PointCloud {
        source: String::new(),
        total_points,
        bounds_min,
        bounds_max,
        origin: [cx, cy, cz],
        positions,
        load_time: Duration::ZERO,
    })
}

fn f64_column(batch: &LasColumnBatch, dimension: LasDimension) -> Result<&[f64], LoadError> {
    match batch.column(dimension) {
        Some(ColumnData::F64(values)) => Ok(values),
        _ => Err(LoadError::MissingColumn(dimension)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_should_keep_https_urls_as_http() {
        let location = Location::parse("https://example.com/a.copc.laz").unwrap();
        assert_eq!(location, Location::Http("https://example.com/a.copc.laz".into()));
    }

    #[test]
    fn parse_should_strip_quotes_from_windows_copy_as_path() {
        let location = Location::parse(r#""C:\data\site 1.copc.laz""#).unwrap();
        assert_eq!(location, Location::File(PathBuf::from(r"C:\data\site 1.copc.laz")));
    }

    #[cfg(windows)]
    #[test]
    fn parse_should_decode_percent_escapes_in_file_urls() {
        let location = Location::parse("file:///C:/data/site%201.copc.laz").unwrap();
        assert_eq!(location, Location::File(PathBuf::from(r"C:\data\site 1.copc.laz")));
    }

    #[test]
    fn parse_should_reject_blank_input() {
        let error = Location::parse("   ").unwrap_err();
        assert_eq!(error.to_string(), "no file path or URL given");
    }

    /// Hits the network, so it only runs with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn load_root_node_should_read_autzen_over_https() {
        let cloud =
            load_root_node("https://s3.amazonaws.com/hobu-lidar/autzen-classified.copc.laz")
                .unwrap();
        assert_eq!(cloud.positions.len(), 61_201);
    }
}
