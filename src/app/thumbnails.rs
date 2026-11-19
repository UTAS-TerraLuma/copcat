//! Small thumbnails at the left of palette rows, for STAC items that have
//! one (a trial). Turn them off with [`ENABLED`]; to remove them, delete
//! this module and what refers to it (search for `thumbnails::`).
//!
//! A thumbnail is fetched only once its row is drawn on screen, a few at a
//! time, on threads that also decode and shrink it; the palette uploads it
//! as a texture when it arrives. Each URL is fetched once per session, and
//! one that can't be fetched or decoded is simply left blank.
//!
//! With a cache (the viewer has a settings directory), the shrunk picture
//! is kept on disk as a small PNG, by a hash of its URL, and read from
//! there in later sessions instead of being fetched. Failures aren't kept,
//! so they're tried again next session.

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use eframe::egui;

use crate::cache::{DiskCache, stable_hash};

/// Whether palette rows show STAC items' thumbnails.
pub(crate) const ENABLED: bool = true;

/// How many thumbnails are fetched at once.
const MAX_IN_FLIGHT: usize = 4;
/// How many requests wait their turn; older ones, for rows probably
/// scrolled past, are forgotten (and asked for again if drawn again).
const MAX_WAITING: usize = 64;
/// A thumbnail bigger than this isn't one.
const MAX_BYTES: u64 = 4 * 1024 * 1024;
/// The most pixels kept on each side: a row's slot is about 24 points
/// wide, so this is sharp at 2.5x scaling.
const KEEP_PIXELS: u32 = 64;

enum Slot {
    Waiting,
    Ready(egui::TextureHandle),
    Failed,
}

type Decoded = (String, Option<egui::ColorImage>);

/// Thumbnails fetched so far, by URL. See the module docs.
pub(crate) struct Thumbnails {
    slots: HashMap<String, Slot>,
    /// URLs to fetch, the most recently asked for last, fetched first.
    waiting: Vec<String>,
    in_flight: usize,
    send: Sender<Decoded>,
    receive: Receiver<Decoded>,
    agent: ureq::Agent,
    cache: Option<DiskCache>,
}

impl Default for Thumbnails {
    fn default() -> Self {
        let (send, receive) = mpsc::channel();
        let agent = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(10)))
            .timeout_global(Some(Duration::from_secs(20)))
            .user_agent(concat!("copcat/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        Self {
            slots: HashMap::new(),
            waiting: Vec::new(),
            in_flight: 0,
            send,
            receive,
            agent,
            cache: None,
        }
    }
}

impl Thumbnails {
    /// Keeps thumbnails in `cache` between sessions, if given.
    pub(crate) fn set_cache(&mut self, cache: Option<DiskCache>) {
        self.cache = cache;
    }

    /// Uploads the thumbnails that have arrived, and starts fetching more.
    /// Call once a frame before [`Thumbnails::get`].
    pub(crate) fn poll(&mut self, ctx: &egui::Context) {
        while let Ok((url, image)) = self.receive.try_recv() {
            self.in_flight -= 1;
            let slot = match image {
                Some(image) => Slot::Ready(ctx.load_texture(
                    format!("thumbnail {url}"),
                    image,
                    egui::TextureOptions::LINEAR,
                )),
                None => Slot::Failed,
            };
            self.slots.insert(url, slot);
        }
        self.start(ctx);
    }

    /// The thumbnail at `url`, if it has arrived; asks for it if it hasn't
    /// been.
    pub(crate) fn get(&mut self, ctx: &egui::Context, url: &str) -> Option<&egui::TextureHandle> {
        if !self.slots.contains_key(url) {
            self.slots.insert(url.to_owned(), Slot::Waiting);
            self.waiting.push(url.to_owned());
            if self.waiting.len() > MAX_WAITING {
                let forgotten = self.waiting.remove(0);
                self.slots.remove(&forgotten);
            }
            self.start(ctx);
        }
        match self.slots.get(url) {
            Some(Slot::Ready(texture)) => Some(texture),
            _ => None,
        }
    }

    fn start(&mut self, ctx: &egui::Context) {
        while self.in_flight < MAX_IN_FLIGHT {
            let Some(url) = self.waiting.pop() else {
                return;
            };
            self.in_flight += 1;
            let agent = self.agent.clone();
            let cache = self.cache.clone();
            let send = self.send.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let image = load(&agent, cache.as_ref(), &url);
                let _ = send.send((url, image));
                ctx.request_repaint();
            });
        }
    }
}

/// The thumbnail at `url`, from the cache if it's there, else fetched,
/// shrunk and kept in the cache.
fn load(agent: &ureq::Agent, cache: Option<&DiskCache>, url: &str) -> Option<egui::ColorImage> {
    let name = format!("{}.png", stable_hash(url));
    if let Some(image) = cache
        .and_then(|cache| cache.read(&name))
        .and_then(|bytes| decode(&bytes))
    {
        return Some(image);
    }
    let small = shrink(&fetch(agent, url)?)?;
    if let Some(cache) = cache {
        let mut png = Vec::new();
        if small
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .is_ok()
        {
            cache.write(&name, &png);
        }
    }
    Some(color_image(&small))
}

fn fetch(agent: &ureq::Agent, url: &str) -> Option<Vec<u8>> {
    agent
        .get(url)
        .call()
        .ok()?
        .body_mut()
        .with_config()
        .limit(MAX_BYTES)
        .read_to_vec()
        .ok()
}

/// A PNG or JPEG, shrunk to at most [`KEEP_PIXELS`] on each side.
pub(crate) fn decode(bytes: &[u8]) -> Option<egui::ColorImage> {
    shrink(bytes).map(|small| color_image(&small))
}

fn shrink(bytes: &[u8]) -> Option<image::RgbaImage> {
    let image = image::load_from_memory(bytes).ok()?;
    // `thumbnail` would also enlarge a small one.
    let small = if image.width() > KEEP_PIXELS || image.height() > KEEP_PIXELS {
        image.thumbnail(KEEP_PIXELS, KEEP_PIXELS)
    } else {
        image
    };
    Some(small.to_rgba8())
}

fn color_image(small: &image::RgbaImage) -> egui::ColorImage {
    let size = [small.width() as usize, small.height() as usize];
    egui::ColorImage::from_rgba_unmultiplied(size, small.as_raw())
}

/// The size of the slot at the left of each row: as tall as the row's text,
/// so rows are no taller with thumbnails than without.
pub(crate) fn slot_size(ui: &egui::Ui) -> egui::Vec2 {
    let height = ui.text_style_height(&egui::TextStyle::Button);
    egui::vec2(height * 1.5, height)
}

/// Paints `texture` in `slot`, as big as fits without changing its shape.
pub(crate) fn paint(ui: &egui::Ui, slot: egui::Rect, texture: &egui::TextureHandle) {
    let size = texture.size_vec2();
    let scale = (slot.width() / size.x).min(slot.height() / size.y);
    let rect = egui::Rect::from_center_size(slot.center(), size * scale);
    let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    ui.painter()
        .image(texture.id(), rect, uv, egui::Color32::WHITE);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_should_read_the_fixture_png_at_its_size() {
        let bytes = include_bytes!("../../tests/data/stac/static/site-a/site-a-thumbnail.png");
        let image = decode(bytes).expect("a PNG");
        assert_eq!(image.size, [24, 16]);
    }

    #[test]
    fn decode_should_shrink_a_big_jpeg_keeping_its_shape() {
        let big = image::RgbImage::from_pixel(400, 200, image::Rgb([200, 100, 50]));
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgb8(big)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Jpeg,
            )
            .unwrap();
        let image = decode(&bytes).expect("a JPEG");
        assert_eq!(image.size, [64, 32]);
    }

    #[test]
    fn a_thumbnail_should_be_kept_shrunk_and_read_back_without_fetching() {
        let temp = tempfile::tempdir().unwrap();
        let cache = DiskCache::new(
            temp.path().to_path_buf(),
            crate::cache::THUMBNAIL_LIMITS,
            crate::cache::system_clock(),
        );
        let big = image::RgbImage::from_pixel(400, 200, image::Rgb([200, 100, 50]));
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgb8(big)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/big.png", server.local_addr().unwrap());
        let serve = std::thread::spawn(move || {
            use std::io::{Read, Write};
            let (mut stream, _) = server.accept().unwrap();
            let mut request = [0; 1024];
            let _ = stream.read(&mut request);
            let head = format!(
                "HTTP/1.1 200 OK
Content-Length: {}
Connection: close

",
                bytes.len()
            );
            stream.write_all(head.as_bytes()).unwrap();
            stream.write_all(&bytes).unwrap();
        });
        let agent = Thumbnails::default().agent;
        let fetched = load(&agent, Some(&cache), &url).expect("fetched");
        serve.join().unwrap();
        assert_eq!(fetched.size, [64, 32]);
        let kept = cache.read(&format!("{}.png", stable_hash(&url))).unwrap();
        assert!(kept.len() < 4096, "{} bytes", kept.len());
        // The server has gone: this comes from the cache.
        let cached = load(&agent, Some(&cache), &url).expect("cached");
        assert_eq!(cached.size, [64, 32]);
    }

    #[test]
    fn decode_should_refuse_what_isnt_a_picture() {
        assert!(decode(b"<html>not found</html>").is_none());
    }
}
