//! Opening the fixture from a file and over HTTP, through the viewer's loader.

mod common;

use std::io::Read;

use common::expected;
use common::server::{FileServer, RangeRequest, parse_range};

fn assert_matches_fixture(cloud: &copcat::CloudSummary) {
    assert_eq!(cloud.header_points, expected::POINTS);
    assert_eq!(cloud.points.len() as u64, expected::POINTS);
    assert_eq!(cloud.bounds_min, expected::BOUNDS_MIN);
    assert_eq!(cloud.bounds_max, expected::BOUNDS_MAX);
    assert!(cloud.has_rgb);
    assert!(
        cloud.nodes > 1,
        "the fixture should have a real octree, not just a root"
    );
    let classes: Vec<(u32, usize)> = cloud.class_counts().into_iter().collect();
    assert_eq!(classes, expected::CLASSES);

    // The decoded points stay inside the header's bounds (to f32 precision
    // around the octree centre) and reach its corners.
    for axis in 0..3 {
        let low = cloud
            .points
            .iter()
            .map(|p| p.position[axis])
            .fold(f64::INFINITY, f64::min);
        let high = cloud
            .points
            .iter()
            .map(|p| p.position[axis])
            .fold(f64::NEG_INFINITY, f64::max);
        assert!(
            (low - expected::BOUNDS_MIN[axis]).abs() < 0.01,
            "axis {axis}: lowest point {low}"
        );
        assert!(
            (high - expected::BOUNDS_MAX[axis]).abs() < 0.01,
            "axis {axis}: highest point {high}"
        );
    }
    assert!(cloud.points.iter().all(|p| p.rgb.is_some()));
}

#[test]
fn fixture_should_open_from_a_file() {
    let cloud = copcat::read_cloud(common::fixture().to_str().unwrap()).unwrap();
    assert_matches_fixture(&cloud);
}

#[test]
fn fixture_should_open_from_a_file_url() {
    let path = common::fixture().canonicalize().unwrap();
    let url = url::Url::from_file_path(path).unwrap();
    let cloud = copcat::read_cloud(url.as_str()).unwrap();
    assert_matches_fixture(&cloud);
}

#[test]
fn fixture_should_load_identically_from_a_file_and_over_http() {
    let server = FileServer::serve(common::data_dir());
    let from_file = copcat::read_cloud(common::fixture().to_str().unwrap()).unwrap();
    let over_http = copcat::read_cloud(&server.url("fixture.copc.laz")).unwrap();

    assert_matches_fixture(&over_http);
    // Every point, bit for bit.
    assert_eq!(from_file, over_http);

    // The viewer must have streamed byte ranges, never the whole file.
    let requests = server.requests();
    let gets: Vec<_> = requests.iter().filter(|r| r.method == "GET").collect();
    assert!(!gets.is_empty());
    for request in gets {
        assert!(request.range.is_some(), "GET without Range: {request:?}");
        assert_eq!(request.status, 206, "{request:?}");
    }
}

#[test]
fn http_source_should_report_a_missing_file() {
    let server = FileServer::serve(common::data_dir());
    let error = copcat::read_cloud(&server.url("missing.copc.laz")).unwrap_err();
    assert!(error.contains("404"), "{error}");
}

/// A server that ignores Range would send the whole file; the viewer
/// refuses rather than download a multi-gigabyte cloud.
#[test]
fn http_source_should_refuse_a_server_without_range_support() {
    use std::io::Write;
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/cloud.copc.laz", listener.local_addr().unwrap());
    let body = std::fs::read(common::fixture()).unwrap();
    let server = std::thread::spawn(move || {
        // HEAD for the size may come first; answer until the GET.
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut request = [0; 4096];
            let read = stream.read(&mut request).unwrap();
            let is_get = request[..read].starts_with(b"GET");
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(head.as_bytes()).unwrap();
            if is_get {
                let _ = stream.write_all(&body);
                break;
            }
        }
    });
    let error = copcat::read_cloud(&url).unwrap_err();
    assert!(error.contains("206"), "{error}");
    server.join().unwrap();
}

/// The file server itself, since every HTTP test relies on it.
mod file_server {
    use super::*;

    fn get(url: &str, range: Option<&str>) -> (u16, Option<String>, Vec<u8>) {
        let agent = ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                .http_status_as_error(false)
                .build(),
        );
        let mut request = agent.get(url);
        if let Some(range) = range {
            request = request.header("Range", range);
        }
        let mut response = request.call().unwrap();
        let content_range = response
            .headers()
            .get("content-range")
            .map(|v| v.to_str().unwrap().to_owned());
        let status = response.status().as_u16();
        let body = response.body_mut().read_to_vec().unwrap();
        (status, content_range, body)
    }

    #[test]
    fn should_serve_ranges_whole_files_and_404s() {
        let server = FileServer::serve(common::data_dir());
        let url = server.url("fixture.copc.laz");
        let file = std::fs::read(common::fixture()).unwrap();
        let total = file.len();

        let (status, _, body) = get(&url, None);
        assert_eq!((status, body.len()), (200, total));

        let (status, range, body) = get(&url, Some("bytes=0-3"));
        assert_eq!(status, 206);
        assert_eq!(range.unwrap(), format!("bytes 0-3/{total}"));
        assert_eq!(body, b"LASF");

        let (status, _, body) = get(&url, Some("bytes=-10"));
        assert_eq!((status, &body[..]), (206, &file[total - 10..]));

        let (status, range, _) = get(&url, Some(&format!("bytes={total}-")));
        assert_eq!(status, 416);
        assert_eq!(range.unwrap(), format!("bytes */{total}"));

        assert_eq!(get(&server.url("nope.bin"), None).0, 404);
        assert_eq!(get(&server.url("../Cargo.toml"), None).0, 404);
        assert_eq!(get(&server.url("%2e%2e/Cargo.toml"), None).0, 404);
    }

    #[test]
    fn parse_range_should_follow_rfc_9110() {
        assert_eq!(parse_range("bytes=0-9", 100), RangeRequest::Bytes(0, 9));
        assert_eq!(
            parse_range("bytes=90-200", 100),
            RangeRequest::Bytes(90, 99)
        );
        assert_eq!(parse_range("bytes=95-", 100), RangeRequest::Bytes(95, 99));
        assert_eq!(parse_range("bytes=-5", 100), RangeRequest::Bytes(95, 99));
        assert_eq!(parse_range("bytes=-500", 100), RangeRequest::Bytes(0, 99));
        assert_eq!(parse_range("bytes=100-", 100), RangeRequest::Unsatisfiable);
        assert_eq!(parse_range("bytes=-0", 100), RangeRequest::Unsatisfiable);
        assert_eq!(parse_range("bytes=5-1", 100), RangeRequest::Ignored);
        assert_eq!(parse_range("bytes=0-1,5-6", 100), RangeRequest::Ignored);
        assert_eq!(parse_range("items=0-1", 100), RangeRequest::Ignored);
    }
}
