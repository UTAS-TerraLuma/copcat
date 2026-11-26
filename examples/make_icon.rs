//! Draws Copcat's icon: three points, coloured like a height ramp, on a dark
//! rounded square. Writes `assets/icon.png` (256 px) and `assets/icon.ico`
//! (16 to 256 px, each a PNG).
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
const SUBSAMPLES: u32 = 8;

/// The points, in a 256-unit square: centre, radius and colour. Low and
/// blue at the left, up to amber at the top, like points coloured by
/// height.
const DOTS: [((f32, f32), f32, u32); 3] = [
    ((82.0, 168.0), 34.0, 0x3d6fe0),
    ((174.0, 168.0), 34.0, 0x22b5a6),
    ((128.0, 88.0), 34.0, 0xffc845),
];

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
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for py in 0..size {
        for px in 0..size {
            // Average premultiplied colour over the subsamples.
            let mut sum = [0.0f32; 4];
            for sy in 0..SUBSAMPLES {
                for sx in 0..SUBSAMPLES {
                    let x = (px as f32 + (sx as f32 + 0.5) / SUBSAMPLES as f32) * scale;
                    let y = (py as f32 + (sy as f32 + 0.5) / SUBSAMPLES as f32) * scale;
                    let [r, g, b, a] = shade(x, y);
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
fn shade(x: f32, y: f32) -> Rgba {
    if !in_rounded_square(x, y, 8.0, 248.0, 46.0) {
        return [0.0; 4];
    }
    for ((cx, cy), radius, colour) in DOTS {
        if (x - cx).powi(2) + (y - cy).powi(2) <= radius * radius {
            return hex(colour);
        }
    }
    // A slate background, lighter at the top.
    let t = (y - 8.0) / 240.0;
    mix(hex(0x2d3b4c), hex(0x121820), t)
}

fn in_rounded_square(x: f32, y: f32, min: f32, max: f32, radius: f32) -> bool {
    if x < min || x > max || y < min || y > max {
        return false;
    }
    let cx = x.clamp(min + radius, max - radius);
    let cy = y.clamp(min + radius, max - radius);
    (x - cx).powi(2) + (y - cy).powi(2) <= radius * radius
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
