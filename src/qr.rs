//! Renders a URL as a QR-code PNG data URI for each native window to
//! display.
//!
//! Builds the image ourselves from `qrcode`'s bit matrix rather than using
//! its optional "image" feature, keeping the dependency footprint small
//! and explicit (same reasoning as the sibling Python prototype's choice
//! to draw the matrix directly instead of pulling in a heavier rendering
//! path).

use base64::Engine;
use image::{GrayImage, Luma};
use qrcode::{EcLevel, QrCode};

const MODULE_PX: u32 = 6;
const QUIET_ZONE_MODULES: u32 = 4;

/// Returns a `data:image/png;base64,...` URI encoding `data`'s QR code,
/// ready to drop straight into an `<img src="...">`.
pub fn build_data_uri(data: &str) -> String {
    let code = QrCode::with_error_correction_level(data.as_bytes(), EcLevel::M)
        .expect("QR encoding of a short URL should never fail");
    let colors = code.to_colors();
    let size = code.width() as u32;
    let total = (size + 2 * QUIET_ZONE_MODULES) * MODULE_PX;

    let mut img = GrayImage::from_pixel(total, total, Luma([255]));
    for y in 0..size {
        for x in 0..size {
            if colors[(y * size + x) as usize].select(true, false) {
                let px0 = (x + QUIET_ZONE_MODULES) * MODULE_PX;
                let py0 = (y + QUIET_ZONE_MODULES) * MODULE_PX;
                for dy in 0..MODULE_PX {
                    for dx in 0..MODULE_PX {
                        img.put_pixel(px0 + dx, py0 + dy, Luma([0]));
                    }
                }
            }
        }
    }

    let mut png_bytes = Vec::new();
    img.write_to(
        &mut std::io::Cursor::new(&mut png_bytes),
        image::ImageFormat::Png,
    )
    .expect("encoding an in-memory PNG should never fail");
    let encoded = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
    format!("data:image/png;base64,{encoded}")
}
