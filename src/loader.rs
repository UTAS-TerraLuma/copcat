//! A background thread that keeps one COPC reader open and streams octree
//! nodes to the UI as the camera asks for them.
//!
//! The UI sends the full list of nodes it still needs, in priority order,
//! whenever that list changes. The worker replaces its queue with the newest
//! list between downloads, so nodes the camera has moved away from are dropped
//! without ever being fetched.

use std::collections::{HashSet, VecDeque};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::Instant;

use copc_streaming::{
    ByteSource, Chunk, CopcError, CopcHeader, CopcStreamingReader, Fields, FileSource,
    HierarchyEntry, VoxelKey,
};
use eframe::egui;

use crate::cloud::{self, CloudInfo, LoadError, Location};
use crate::http_source::{HttpSource, MAX_PARALLEL_REQUESTS};
use crate::lod::Octree;

/// Nodes downloaded per round trip. Matching the HTTP source's parallelism keeps
/// every connection busy, and small batches let the worker react quickly when
/// the camera moves.
const BATCH_SIZE: usize = MAX_PARALLEL_REQUESTS;

pub struct OpenedCloud {
    pub info: CloudInfo,
    pub octree: Octree,
}

/// A decoded node, or why it failed.
pub type NodeResult = Result<NodeData, String>;

pub struct NodeData {
    /// Metres relative to [`CloudInfo::origin`].
    pub positions: Vec<[f32; 3]>,
    /// 16-bit RGB plus padding, one per position, if the file has colour.
    pub colors: Option<Vec<[u16; 4]>>,
}

pub enum Event {
    /// The header and hierarchy are loaded. Always the first event on success.
    /// Boxed so the other, frequent variants don't pay for its size.
    Opened(Box<OpenedCloud>),
    /// These nodes are being downloaded now.
    Fetching(Vec<VoxelKey>),
    Node {
        key: VoxelKey,
        result: NodeResult,
    },
    /// The file couldn't be opened. The worker has stopped.
    Failed(LoadError),
}

enum Request {
    /// Replaces the download queue. Highest priority first.
    Want(Vec<VoxelKey>),
    /// The UI dropped this node, so fetch it again if it's wanted later.
    Evicted(VoxelKey),
}

/// Handle to the loader thread. Dropping it stops the thread once its
/// current batch finishes.
pub struct Loader {
    requests: Sender<Request>,
    events: Receiver<Event>,
}

impl Loader {
    pub fn spawn(source: String, ctx: egui::Context) -> Self {
        let (request_sender, request_receiver) = mpsc::channel();
        let (event_sender, event_receiver) = mpsc::channel();
        thread::spawn(move || {
            let events = Events {
                sender: event_sender,
                ctx,
            };
            run(&source, &request_receiver, &events);
        });
        Self {
            requests: request_sender,
            events: event_receiver,
        }
    }

    /// Replaces the download queue with `keys`, highest priority first.
    pub fn want(&self, keys: Vec<VoxelKey>) {
        // Sending only fails once the worker has exited, which `try_recv`
        // reports as `Disconnected`, so there's nothing more to do here.
        let _ = self.requests.send(Request::Want(keys));
    }

    /// Tells the worker the UI dropped `key`, so it may be fetched again.
    pub fn evicted(&self, key: VoxelKey) {
        let _ = self.requests.send(Request::Evicted(key));
    }

    pub fn try_recv(&self) -> Result<Event, TryRecvError> {
        self.events.try_recv()
    }
}

/// The worker's side of the event channel.
struct Events {
    sender: Sender<Event>,
    ctx: egui::Context,
}

impl Events {
    /// Returns `false` once the UI has dropped the [`Loader`], so the worker can stop.
    fn send(&self, event: Event) -> bool {
        let delivered = self.sender.send(event).is_ok();
        // egui only redraws on input, so wake it to show the news.
        self.ctx.request_repaint();
        delivered
    }
}

fn run(input: &str, requests: &Receiver<Request>, events: &Events) {
    let result = match Location::parse(input) {
        Ok(Location::Http(url)) => serve(HttpSource::new(url), input, requests, events),
        Ok(Location::File(path)) => match FileSource::open(&path) {
            Ok(file) => serve(file, input, requests, events),
            Err(source) => Err(LoadError::OpenFile { path, source }),
        },
        Err(error) => Err(error),
    };
    if let Err(error) = result {
        events.send(Event::Failed(error));
    }
}

/// Opens the file, then answers requests until the UI hangs up.
///
/// Generic over the byte source so local files and HTTP share one code path.
/// `copc-streaming` is async, but both sources block internally, so driving
/// it with `pollster` on this thread needs no async runtime.
fn serve<S: ByteSource>(
    source: S,
    input: &str,
    requests: &Receiver<Request>,
    events: &Events,
) -> Result<(), LoadError> {
    let started = Instant::now();
    let reader = pollster::block_on(async {
        let mut reader = CopcStreamingReader::open(source).await?;
        // Each entry is 32 bytes, so even big clouds have a small hierarchy.
        reader.load_all_hierarchy().await?;
        Ok::<_, CopcError>(reader)
    })?;

    let info = CloudInfo::from_header(reader.header(), input, started.elapsed());
    let origin = info.origin;
    let copc_info = reader.copc_info();
    let octree = Octree::new(
        copc_info.halfsize as f32,
        copc_info.spacing as f32,
        info.local_bounds(),
        reader
            .entries()
            .map(|(key, entry)| (*key, entry.point_count))
            .collect(),
    );
    if !events.send(Event::Opened(Box::new(OpenedCloud { info, octree }))) {
        return Ok(());
    }

    let mut queue = FetchQueue::default();
    loop {
        // Sleep until there's work, then catch up on every request since, so
        // the queue reflects where the camera is now.
        if queue.is_empty() {
            match requests.recv() {
                Ok(request) => queue.apply(request),
                Err(_) => return Ok(()),
            }
        }
        loop {
            match requests.try_recv() {
                Ok(request) => queue.apply(request),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return Ok(()),
            }
        }

        let batch = queue.next_batch();
        if batch.is_empty() {
            continue;
        }
        if !events.send(Event::Fetching(batch.clone())) {
            return Ok(());
        }
        let results = fetch_nodes(&reader, &batch, origin);
        for (key, result) in batch.into_iter().zip(results) {
            if !events.send(Event::Node { key, result }) {
                return Ok(());
            }
        }
    }
}

/// The worker's download queue.
#[derive(Default)]
struct FetchQueue {
    pending: VecDeque<VoxelKey>,
    /// Nodes already fetched (or failed). The UI may ask for a node that is
    /// on its way back, because it sent the request before receiving the node;
    /// this stops it being fetched twice.
    delivered: HashSet<VoxelKey>,
}

impl FetchQueue {
    fn apply(&mut self, request: Request) {
        match request {
            Request::Want(keys) => {
                self.pending = keys
                    .into_iter()
                    .filter(|key| !self.delivered.contains(key))
                    .collect();
            }
            Request::Evicted(key) => {
                self.delivered.remove(&key);
            }
        }
    }

    fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    fn next_batch(&mut self) -> Vec<VoxelKey> {
        let count = self.pending.len().min(BATCH_SIZE);
        let batch: Vec<_> = self.pending.drain(..count).collect();
        self.delivered.extend(&batch);
        batch
    }
}

/// Downloads `keys` in one batched read, then decodes them in parallel.
/// Returns one result per key, in the same order.
fn fetch_nodes<S: ByteSource>(
    reader: &CopcStreamingReader<S>,
    keys: &[VoxelKey],
    origin: [f64; 3],
) -> Vec<NodeResult> {
    let (entries, compressed) = match download(reader, keys) {
        Ok(downloaded) => downloaded,
        Err(error) => {
            let message = error.to_string();
            return keys.iter().map(|_| Err(message.clone())).collect();
        }
    };
    let header = reader.header();
    // LAZ decoding is CPU-bound and single-threaded per node, so give each
    // node its own thread. `scope` lets the threads borrow the downloaded data.
    thread::scope(|scope| {
        let workers: Vec<_> = compressed
            .iter()
            .zip(&entries)
            .map(|(bytes, entry)| scope.spawn(move || decode_node(bytes, entry, header, origin)))
            .collect();
        workers
            .into_iter()
            .map(|worker| match worker.join() {
                Ok(result) => result.map_err(|error| error.to_string()),
                Err(_) => Err("the decoding thread panicked".to_owned()),
            })
            .collect()
    })
}

/// Fetches the compressed bytes of every node in one `read_ranges` call, which
/// the HTTP source spreads across parallel requests.
fn download<S: ByteSource>(
    reader: &CopcStreamingReader<S>,
    keys: &[VoxelKey],
) -> Result<(Vec<HierarchyEntry>, Vec<Vec<u8>>), CopcError> {
    let entries = keys
        .iter()
        .map(|key| {
            reader
                .get(key)
                .cloned()
                .ok_or(CopcError::NodeNotFound(*key))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let ranges: Vec<_> = entries
        .iter()
        .map(|entry| (entry.offset, u64::from(entry.byte_size)))
        .collect();
    let compressed = pollster::block_on(reader.source().read_ranges(&ranges))?;
    Ok((entries, compressed))
}

fn decode_node(
    compressed: &[u8],
    entry: &HierarchyEntry,
    header: &CopcHeader,
    origin: [f64; 3],
) -> Result<NodeData, CopcError> {
    // Only the layers we draw: skipping the others saves most of the decode time.
    let has_rgb = header.las_header().point_format().has_color;
    let fields = if has_rgb {
        Fields::Z | Fields::RGB
    } else {
        Fields::Z
    };
    let chunk = Chunk::decompress(
        compressed,
        entry,
        header.laz_vlr(),
        header.las_header(),
        fields,
    )?;
    let positions = chunk
        .positions()
        .ok_or(CopcError::PartialDecode(chunk.fields))?
        .map(|position| cloud::to_local(position, origin))
        .collect();
    // `rgb()` is `None` when the format has no colour, which is what we want.
    // The fourth channel pads to the GPU's Unorm16x4 vertex format.
    let colors = chunk
        .rgb()
        .map(|rgb| rgb.map(|(r, g, b)| [r, g, b, u16::MAX]).collect());
    Ok(NodeData { positions, colors })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn key(level: i32, x: i32) -> VoxelKey {
        VoxelKey {
            level,
            x,
            y: 0,
            z: 0,
        }
    }

    #[test]
    fn fetch_queue_should_take_batches_in_priority_order() {
        let mut queue = FetchQueue::default();
        let wanted: Vec<_> = (0..20).map(|x| key(5, x)).collect();
        queue.apply(Request::Want(wanted.clone()));
        assert_eq!(queue.next_batch(), wanted[..BATCH_SIZE]);
    }

    #[test]
    fn fetch_queue_should_drop_keys_no_longer_wanted() {
        let mut queue = FetchQueue::default();
        queue.apply(Request::Want(vec![key(1, 0), key(1, 1)]));
        queue.apply(Request::Want(vec![key(1, 1)]));
        assert_eq!(queue.next_batch(), vec![key(1, 1)]);
    }

    #[test]
    fn fetch_queue_should_skip_keys_already_delivered() {
        let mut queue = FetchQueue::default();
        queue.apply(Request::Want(vec![key(1, 0)]));
        queue.next_batch();
        // The UI asked again before it received the node.
        queue.apply(Request::Want(vec![key(1, 0), key(1, 1)]));
        assert_eq!(queue.next_batch(), vec![key(1, 1)]);
    }

    #[test]
    fn fetch_queue_should_refetch_evicted_keys() {
        let mut queue = FetchQueue::default();
        queue.apply(Request::Want(vec![key(1, 0)]));
        queue.next_batch();
        queue.apply(Request::Evicted(key(1, 0)));
        queue.apply(Request::Want(vec![key(1, 0)]));
        assert_eq!(queue.next_batch(), vec![key(1, 0)]);
    }

    /// Opens `source` and fetches its root node through a real loader thread.
    fn fetch_root(source: &str) -> NodeData {
        let loader = Loader::spawn(source.to_owned(), egui::Context::default());
        loop {
            match loader.events.recv_timeout(Duration::from_secs(120)) {
                Ok(Event::Opened(_)) => loader.want(vec![VoxelKey::ROOT]),
                Ok(Event::Fetching(_)) => {}
                Ok(Event::Node { result, .. }) => return result.unwrap(),
                Ok(Event::Failed(error)) => panic!("{error}"),
                Err(error) => panic!("{error}"),
            }
        }
    }

    /// Hits the network, so it only runs with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn loader_should_read_autzen_root_over_https() {
        let positions =
            fetch_root("https://s3.amazonaws.com/hobu-lidar/autzen-classified.copc.laz").positions;
        assert_eq!(positions.len(), 61_201);
    }

    /// Metashape writes 0xAABB in the laszip VLR's reserved field, which
    /// copc-reader 0.9 rejected. Hits the network, like the test above.
    #[test]
    #[ignore]
    fn loader_should_read_metashape_file_with_aabb_vlr_reserved() {
        let node = fetch_root(
            "https://object-store.rc.nectar.org.au/v1/AUTH_4df2f67c2eed48a2aaeeed008a4bf0de/naturescan-drone/NSNSMDD001_20260425_m3m01/level2_ard/NSNSMDD001_20260425_m3m01_pc.copc.laz",
        );
        assert!(!node.positions.is_empty());
    }
}
