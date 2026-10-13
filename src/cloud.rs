//! Loading COPC point clouds from local files or HTTP(S) URLs.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use copc_streaming::{ByteSource, CopcError, CopcStreamingReader, Fields, FileSource};
use glam::Vec3;

use crate::http_source::HttpSource;

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

impl PointCloud {
    /// The file's bounding box relative to `origin`, in the same space as `positions`.
    pub fn local_bounds(&self) -> (Vec3, Vec3) {
        let to_local = |p: [f64; 3]| {
            Vec3::new(
                (p[0] - self.origin[0]) as f32,
                (p[1] - self.origin[1]) as f32,
                (p[2] - self.origin[2]) as f32,
            )
        };
        (to_local(self.bounds_min), to_local(self.bounds_max))
    }
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

/// Loads the header and the root octree node (level 0) of a COPC file.
///
/// This blocks on file or network IO, so call it off the UI thread.
pub fn load_root_node(input: &str) -> Result<PointCloud, LoadError> {
    let started = Instant::now();
    let mut cloud = match Location::parse(input)? {
        Location::Http(url) => read_root_node(HttpSource::new(url))?,
        Location::File(path) => {
            let file = FileSource::open(&path).map_err(|source| LoadError::OpenFile {
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
///
/// `copc-streaming` is async, but both sources block internally, so driving
/// it with `pollster` on the caller's thread needs no async runtime.
fn read_root_node<S: ByteSource>(source: S) -> Result<PointCloud, LoadError> {
    pollster::block_on(async {
        let mut reader = CopcStreamingReader::open(source).await?;

        let las_header = reader.header().las_header();
        let total_points = las_header.number_of_points();
        let bounds = las_header.bounds();
        let bounds_min = [bounds.min.x, bounds.min.y, bounds.min.z];
        let bounds_max = [bounds.max.x, bounds.max.y, bounds.max.z];
        let copc_info = reader.copc_info();
        let [cx, cy, cz] = copc_info.center;
        let root_bounds = copc_info.root_bounds();

        // A query rather than fetching `VoxelKey::ROOT` directly, so a file
        // whose root node holds no points loads as empty instead of failing.
        let chunks = reader
            .query_chunks_to_level(&root_bounds, 0, Fields::Z)
            .await?;
        let positions = chunks
            .iter()
            .flat_map(|chunk| {
                chunk
                    .positions()
                    .expect("chunks were fetched with Fields::Z")
            })
            .map(|[x, y, z]| [(x - cx) as f32, (y - cy) as f32, (z - cz) as f32])
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
    })
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

    /// Hits the network, so it only runs with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn load_root_node_should_read_autzen_over_https() {
        let cloud =
            load_root_node("https://s3.amazonaws.com/hobu-lidar/autzen-classified.copc.laz")
                .unwrap();
        assert_eq!(cloud.positions.len(), 61_201);
    }

    /// Metashape writes 0xAABB in the laszip VLR's reserved field, which
    /// copc-reader 0.9 rejected. Hits the network, like the test above.
    #[test]
    #[ignore]
    fn load_root_node_should_read_metashape_file_with_aabb_vlr_reserved() {
        let cloud = load_root_node(
            "https://object-store.rc.nectar.org.au/v1/AUTH_4df2f67c2eed48a2aaeeed008a4bf0de/naturescan-drone/NSNSMDD001_20260425_m3m01/level2_ard/NSNSMDD001_20260425_m3m01_pc.copc.laz",
        )
        .unwrap();
        assert!(!cloud.positions.is_empty());
    }
}
