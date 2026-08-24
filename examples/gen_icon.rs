//! One-off generator for `assets/icon-256.png` -- not part of the app
//! itself, run by hand whenever the icon design changes:
//! `cargo run --example gen_icon`.

use image::{Rgba, RgbaImage};

const SIZE: u32 = 256;
const ACCENT: [u8; 4] = [0x2f, 0x6f, 0xeb, 0xff]; // the app's --accent blue

/// Whether `(px, py)` falls inside a rounded rectangle centered at
/// `(cx, cy)` with half-extents `(hw, hh)` and corner radius `r`.
fn in_rounded_rect(px: f32, py: f32, cx: f32, cy: f32, hw: f32, hh: f32, r: f32) -> bool {
    let adx = (px - cx).abs();
    let ady = (py - cy).abs();
    if adx > hw || ady > hh {
        return false; // outside the plain bounding box
    }
    if adx <= hw - r || ady <= hh - r {
        return true; // inner cross: not near a corner at all
    }
    // Near a corner: only inside if within `r` of that corner's center.
    let cdx = adx - (hw - r);
    let cdy = ady - (hh - r);
    cdx * cdx + cdy * cdy <= r * r
}

fn in_triangle(p: (f32, f32), a: (f32, f32), b: (f32, f32), c: (f32, f32)) -> bool {
    let sign = |p1: (f32, f32), p2: (f32, f32), p3: (f32, f32)| {
        (p1.0 - p3.0) * (p2.1 - p3.1) - (p2.0 - p3.0) * (p1.1 - p3.1)
    };
    let d1 = sign(p, a, b);
    let d2 = sign(p, b, c);
    let d3 = sign(p, c, a);
    let has_neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let has_pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(has_neg && has_pos)
}

fn main() {
    let s = SIZE as f32;
    let center = (s - 1.0) / 2.0;

    // Background: a rounded-square "tile", the usual modern app-icon shape.
    let bg_half = s / 2.0 - 6.0;
    let bg_radius = s * 0.2;

    // Foreground: a speech bubble (circle body + small tail).
    let bubble_cx = s * 0.47;
    let bubble_cy = s * 0.44;
    let bubble_r = s * 0.28;
    let tail = [
        (s * 0.30, s * 0.64),
        (s * 0.18, s * 0.80),
        (s * 0.42, s * 0.66),
    ];

    let mut img = RgbaImage::new(SIZE, SIZE);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let in_bg = in_rounded_rect(px, py, center, center, bg_half, bg_half, bg_radius);
            if !in_bg {
                img.put_pixel(x, y, Rgba([0, 0, 0, 0]));
                continue;
            }
            let dx = px - bubble_cx;
            let dy = py - bubble_cy;
            let in_bubble = (dx * dx + dy * dy).sqrt() <= bubble_r
                || in_triangle((px, py), tail[0], tail[1], tail[2]);
            let pixel = if in_bubble {
                Rgba([0xff, 0xff, 0xff, 0xff])
            } else {
                Rgba(ACCENT)
            };
            img.put_pixel(x, y, pixel);
        }
    }
    img.save("assets/icon-256.png")
        .expect("failed to write assets/icon-256.png");
    println!("wrote assets/icon-256.png");
}
