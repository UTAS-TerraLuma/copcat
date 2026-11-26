//! Draws Copcat's icon: a cat's head made of points coloured by height, on a
//! dark rounded square. Writes `assets/icon.png` (256 px) and
//! `assets/icon.ico` (16 to 256 px, each a PNG).
//!
//! ```sh
//! cargo run --example make_icon
//! ```
//!
//! Everything is drawn from the shapes below with 8x8 supersampling, so the
//! output is the same on every machine.

use std::path::Path;

/// Sizes in the `.ico`. Windows picks the nearest for each place it shows
/// an icon.
const ICO_SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];
/// Below this many pixels the points blur into mush, so the cat is drawn
/// solid instead.
const SOLID_BELOW: u32 = 48;
const SUBSAMPLES: u32 = 8;

fn main() {
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");

    let png = encode_png(256, &draw(256));
    std::fs::write(assets.join("icon.png"), &png).expect("write icon.png");

    let images: Vec<(u32, Vec<u8>)> = ICO_SIZES
        .iter()
        .map(|&size| (size, encode_png(size, &draw(size))))
        .collect();
    std::fs::write(assets.join("icon.ico"), encode_ico(&images)).expect("write icon.ico");
    println!("Wrote {}", assets.display());
}

type Rgba = [f32; 4];

/// The icon at `size` pixels square, as straight (not premultiplied) RGBA.
fn draw(size: u32) -> Vec<u8> {
    let scale = 256.0 / size as f32;
    let dots = size >= SOLID_BELOW;
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for py in 0..size {
        for px in 0..size {
            // Average premultiplied colour over the subsamples.
            let mut sum = [0.0f32; 4];
            for sy in 0..SUBSAMPLES {
                for sx in 0..SUBSAMPLES {
                    let x = (px as f32 + (sx as f32 + 0.5) / SUBSAMPLES as f32) * scale;
                    let y = (py as f32 + (sy as f32 + 0.5) / SUBSAMPLES as f32) * scale;
                    let [r, g, b, a] = shade(x, y, dots);
                    sum[0] += r * a;
                    sum[1] += g * a;
                    sum[2] += b * a;
                    sum[3] += a;
                }
            }
            let n = (SUBSAMPLES * SUBSAMPLES) as f32;
            let alpha = sum[3] / n;
            let straight = |c: f32| {
                if alpha > 0.0 { c / n / alpha } else { 0.0 }
            };
            for value in [straight(sum[0]), straight(sum[1]), straight(sum[2]), alpha] {
                out.push((value.clamp(0.0, 1.0) * 255.0).round() as u8);
            }
        }
    }
    out
}

/// The colour at a point, in a 256-unit square.
fn shade(x: f32, y: f32, dots: bool) -> Rgba {
    if !in_rounded_square(x, y, 8.0, 248.0, 46.0) {
        return [0.0; 4];
    }
    // A slate background, lighter at the top.
    let t = (y - 8.0) / 240.0;
    let background = mix(hex(0x2d3b4c), hex(0x121820), t);

    if !in_cat(x, y) {
        return background;
    }
    let height = height_colour(y);
    if !dots {
        return height;
    }
    // A faint fill so the silhouette reads between the points.
    let faint = mix(background, height, 0.16);
    if in_dot(x, y) { height } else { faint }
}

/// The cat's outline: a wide head and two ears, less the eyes and nose.
fn in_cat(x: f32, y: f32) -> bool {
    let head = in_ellipse(x, y, (128.0, 152.0), (86.0, 70.0));
    let ears = [-1.0f32, 1.0].iter().any(|&side| {
        let mirror = |px: f32| 128.0 + side * (px - 128.0);
        in_triangle(
            (x, y),
            (mirror(46.0), 128.0),
            (mirror(58.0), 34.0),
            (mirror(122.0), 88.0),
        )
    });
    let eyes = [-1.0f32, 1.0]
        .iter()
        .any(|&side| in_ellipse(x, y, (128.0 + side * 34.0, 146.0), (12.0, 17.0)));
    let nose = in_triangle((x, y), (116.0, 174.0), (140.0, 174.0), (128.0, 186.0));
    (head || ears) && !eyes && !nose
}

/// Points on a jittered grid, like a LiDAR scan seen from above.
fn in_dot(x: f32, y: f32) -> bool {
    const SPACING: f32 = 12.0;
    const RADIUS: f32 = 4.4;
    let (ci, cj) = ((x / SPACING).floor() as i32, (y / SPACING).floor() as i32);
    for j in cj - 1..=cj + 1 {
        for i in ci - 1..=ci + 1 {
            let (jx, jy) = jitter(i, j);
            let cx = (i as f32 + 0.5) * SPACING + jx;
            let cy = (j as f32 + 0.5) * SPACING + jy;
            if (x - cx).powi(2) + (y - cy).powi(2) <= RADIUS * RADIUS {
                return true;
            }
        }
    }
    false
}

/// A fixed offset of up to 2.5 units for grid cell `(i, j)`.
fn jitter(i: i32, j: i32) -> (f32, f32) {
    let mut h = (i as u32).wrapping_mul(0x9e37_79b1) ^ (j as u32).wrapping_mul(0x85eb_ca77);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    let unit = |bits: u32| (bits & 0xffff) as f32 / 65535.0 * 2.0 - 1.0;
    (unit(h) * 2.5, unit(h >> 16) * 2.5)
}

/// Blue at the chin up to amber at the ear tips, like a height ramp.
fn height_colour(y: f32) -> Rgba {
    let t = ((222.0 - y) / (222.0 - 34.0)).clamp(0.0, 1.0);
    let stops = [
        (0.0, hex(0x3d6fe0)),
        (0.38, hex(0x22b5a6)),
        (0.68, hex(0x8fd14f)),
        (1.0, hex(0xffc845)),
    ];
    for pair in stops.windows(2) {
        let ((t0, c0), (t1, c1)) = (pair[0], pair[1]);
        if t <= t1 {
            return mix(c0, c1, (t - t0) / (t1 - t0));
        }
    }
    stops[stops.len() - 1].1
}

fn in_rounded_square(x: f32, y: f32, min: f32, max: f32, radius: f32) -> bool {
    if x < min || x > max || y < min || y > max {
        return false;
    }
    let cx = x.clamp(min + radius, max - radius);
    let cy = y.clamp(min + radius, max - radius);
    (x - cx).powi(2) + (y - cy).powi(2) <= radius * radius
}

fn in_ellipse(x: f32, y: f32, centre: (f32, f32), radii: (f32, f32)) -> bool {
    ((x - centre.0) / radii.0).powi(2) + ((y - centre.1) / radii.1).powi(2) <= 1.0
}

fn in_triangle(p: (f32, f32), a: (f32, f32), b: (f32, f32), c: (f32, f32)) -> bool {
    let cross = |u: (f32, f32), v: (f32, f32), w: (f32, f32)| {
        (v.0 - u.0) * (w.1 - u.1) - (v.1 - u.1) * (w.0 - u.0)
    };
    let (d1, d2, d3) = (cross(a, b, p), cross(b, c, p), cross(c, a, p));
    let negative = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let positive = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(negative && positive)
}

fn hex(rgb: u32) -> Rgba {
    let channel = |shift: u32| ((rgb >> shift) & 0xff) as f32 / 255.0;
    [channel(16), channel(8), channel(0), 1.0]
}

fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let t = t.clamp(0.0, 1.0);
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

fn encode_png(size: u32, rgba: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, size, size);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::High);
    let mut writer = encoder.write_header().expect("PNG header");
    writer.write_image_data(rgba).expect("PNG data");
    writer.finish().expect("PNG end");
    bytes
}

/// An `.ico` whose images are all PNGs, which Windows has read since Vista.
fn encode_ico(images: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let count = u16::try_from(images.len()).expect("too many images");
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes()); // reserved
    out.extend_from_slice(&1u16.to_le_bytes()); // type: icon
    out.extend_from_slice(&count.to_le_bytes());
    let mut offset = 6 + 16 * images.len();
    for (size, png) in images {
        // 256 is written as 0.
        let side = if *size >= 256 { 0 } else { *size as u8 };
        out.extend_from_slice(&[side, side, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes()); // colour planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        out.extend_from_slice(&(png.len() as u32).to_le_bytes());
        out.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += png.len();
    }
    for (_, png) in images {
        out.extend_from_slice(png);
    }
    out
}
