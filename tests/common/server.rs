//! A tiny static file server for tests: binds 127.0.0.1 on a free port and
//! serves files under one directory, with `Range` support like an object
//! store. Standard library only, one thread per connection.
//!
//! Supports GET and HEAD; single ranges `bytes=a-b`, `bytes=a-` and
//! `bytes=-n` (206 with `Content-Range`, or 416 when unsatisfiable); a request
//! without `Range` gets the whole file with 200. Multi-range requests get 200
//! and the whole file, which RFC 9110 allows. Every response closes the
//! connection, which keeps the parsing simple.
//!
//! For fake STAC APIs: a request for a directory gets its `index.json`, and
//! `{{base}}` in a `.json` file is replaced by the server's address (such as
//! `http://127.0.0.1:50123`), so absolute links can point back at it. The
//! query is ignored when finding the file, but logged.

use std::io::{BufRead, BufReader, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

/// One request the server answered.
#[derive(Clone, Debug)]
pub struct Logged {
    pub method: String,
    pub path: String,
    /// What followed `?`, if anything.
    pub query: Option<String>,
    pub range: Option<String>,
    pub status: u16,
}

pub struct FileServer {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    log: Arc<Mutex<Vec<Logged>>>,
    thread: Option<JoinHandle<()>>,
}

impl FileServer {
    /// Serves the files under `root` until dropped.
    pub fn serve(root: impl Into<PathBuf>) -> Self {
        let root: PathBuf = root.into();
        assert!(root.is_dir(), "{} isn't a directory", root.display());
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind 127.0.0.1:0");
        let addr = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let log = Arc::new(Mutex::new(Vec::new()));
        let thread = {
            let stop = stop.clone();
            let log = log.clone();
            thread::spawn(move || {
                for stream in listener.incoming() {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(stream) = stream else { continue };
                    let root = root.clone();
                    let log = log.clone();
                    thread::spawn(move || {
                        if let Some(entry) = handle(stream, &root, addr) {
                            log.lock().unwrap().push(entry);
                        }
                    });
                }
            })
        };
        Self {
            addr,
            stop,
            log,
            thread: Some(thread),
        }
    }

    /// The URL of `path` (relative to the served directory, `/`-separated).
    pub fn url(&self, path: &str) -> String {
        format!("http://{}/{}", self.addr, path.trim_start_matches('/'))
    }

    /// Every request answered so far, in the order they finished.
    pub fn requests(&self) -> Vec<Logged> {
        self.log.lock().unwrap().clone()
    }
}

impl Drop for FileServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake the accept loop so it sees `stop`.
        let _ = TcpStream::connect(self.addr);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn handle(stream: TcpStream, root: &Path, addr: SocketAddr) -> Option<Logged> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line).ok()?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_owned();
    let target = parts.next()?.to_owned();
    let mut range = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            break;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("range")
        {
            range = Some(value.trim().to_owned());
        }
    }
    let target = target.split('#').next().unwrap_or("");
    let (path, query) = match target.split_once('?') {
        Some((path, query)) => (path.to_owned(), Some(query.to_owned())),
        None => (target.to_owned(), None),
    };
    let base = format!("http://{addr}");
    let status = respond(stream, root, &base, &method, &path, range.as_deref());
    Some(Logged {
        method,
        path,
        query,
        range,
        status,
    })
}

/// Writes the response and returns its status.
fn respond(
    mut stream: TcpStream,
    root: &Path,
    base: &str,
    method: &str,
    path: &str,
    range: Option<&str>,
) -> u16 {
    let mut send = |status: u16, reason: &str, headers: &[(&str, String)], body: &[u8]| {
        let mut head = format!("HTTP/1.1 {status} {reason}\r\nConnection: close\r\n");
        for (name, value) in headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str("\r\n");
        let _ = stream.write_all(head.as_bytes());
        if method != "HEAD" {
            let _ = stream.write_all(body);
        }
        let _ = stream.flush();
        let _ = stream.shutdown(Shutdown::Write);
        status
    };
    if method != "GET" && method != "HEAD" {
        return send(
            405,
            "Method Not Allowed",
            &[("Allow", "GET, HEAD".into())],
            b"",
        );
    }
    let Some(file) = resolve(root, path) else {
        return send(404, "Not Found", &[("Content-Length", "0".into())], b"");
    };
    let Ok(mut bytes) = std::fs::read(&file) else {
        return send(404, "Not Found", &[("Content-Length", "0".into())], b"");
    };
    if file.extension().is_some_and(|e| e == "json") {
        bytes = String::from_utf8_lossy(&bytes)
            .replace("{{base}}", base)
            .into_bytes();
    }
    let total = bytes.len() as u64;
    let content_type = ("Content-Type", content_type(&file).to_owned());
    let accept = ("Accept-Ranges", "bytes".to_owned());
    match range.map(|r| parse_range(r, total)) {
        None | Some(RangeRequest::Ignored) => send(
            200,
            "OK",
            &[content_type, accept, ("Content-Length", total.to_string())],
            &bytes,
        ),
        Some(RangeRequest::Unsatisfiable) => send(
            416,
            "Range Not Satisfiable",
            &[
                accept,
                ("Content-Range", format!("bytes */{total}")),
                ("Content-Length", "0".into()),
            ],
            b"",
        ),
        Some(RangeRequest::Bytes(first, last)) => {
            let body = &bytes[first as usize..=last as usize];
            send(
                206,
                "Partial Content",
                &[
                    content_type,
                    accept,
                    ("Content-Range", format!("bytes {first}-{last}/{total}")),
                    ("Content-Length", body.len().to_string()),
                ],
                body,
            )
        }
    }
}

/// The file `path` names under `root`, or a directory's `index.json`,
/// refusing anything that climbs out.
fn resolve(root: &Path, path: &str) -> Option<PathBuf> {
    let decoded = percent_decode(path)?;
    let mut file = root.to_path_buf();
    for component in Path::new(decoded.trim_start_matches('/')).components() {
        match component {
            Component::Normal(part) => file.push(part),
            Component::CurDir => {}
            _ => return None,
        }
    }
    if file.is_dir() {
        file.push("index.json");
    }
    file.is_file().then_some(file)
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn content_type(file: &Path) -> &'static str {
    match file.extension().and_then(|e| e.to_str()) {
        Some("json") => "application/json",
        Some("html") => "text/html; charset=utf-8",
        Some("txt" | "md") => "text/plain; charset=utf-8",
        Some("png") => "image/png",
        Some("tif" | "tiff") => "image/tiff",
        _ => "application/octet-stream",
    }
}

#[derive(Debug, PartialEq)]
pub enum RangeRequest {
    /// Inclusive first and last byte.
    Bytes(u64, u64),
    Unsatisfiable,
    /// Not a single byte range we understand: send the whole file.
    Ignored,
}

pub fn parse_range(header: &str, total: u64) -> RangeRequest {
    let Some(spec) = header.trim().strip_prefix("bytes=") else {
        return RangeRequest::Ignored;
    };
    if spec.contains(',') {
        return RangeRequest::Ignored;
    }
    let Some((first, last)) = spec.trim().split_once('-') else {
        return RangeRequest::Ignored;
    };
    let parse = |s: &str| s.trim().parse::<u64>().ok();
    let (first, last) = match (first.trim().is_empty(), last.trim().is_empty()) {
        // bytes=-n: the last n bytes.
        (true, false) => match parse(last) {
            Some(0) => return RangeRequest::Unsatisfiable,
            Some(n) => (total.saturating_sub(n), total.saturating_sub(1)),
            None => return RangeRequest::Ignored,
        },
        // bytes=a-
        (false, true) => match parse(first) {
            Some(a) => (a, total.saturating_sub(1)),
            None => return RangeRequest::Ignored,
        },
        (false, false) => match (parse(first), parse(last)) {
            (Some(a), Some(b)) if a <= b => (a, b.min(total.saturating_sub(1))),
            _ => return RangeRequest::Ignored,
        },
        (true, true) => return RangeRequest::Ignored,
    };
    if total == 0 || first >= total {
        RangeRequest::Unsatisfiable
    } else {
        RangeRequest::Bytes(first, last)
    }
}
