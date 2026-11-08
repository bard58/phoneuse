//! Image preparation. The API never resizes: each side must be within
//! 280..=1568 px, or the whole request fails.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use ap_core::{Frame, Result};
use base64::Engine as _;

use crate::types::{Asset, ImageFormat};

/// Smallest side the API accepts, in pixels.
pub const MIN_SIDE: u32 = 280;
/// Largest side the API accepts, in pixels.
pub const MAX_SIDE: u32 = 1568;
/// JPEG quality used for screenshots.
pub const JPEG_QUALITY: u8 = 85;

/// Target size for an image of `width` x `height`.
///
/// The aspect ratio is kept when possible. For extreme ratios (over 5.6:1)
/// one side is clamped, which stretches the image a little. Stretching is
/// used instead of padding because it keeps normalized Box coordinates
/// correct for the original frame.
pub fn target_size(width: u32, height: u32) -> (u32, u32) {
    let (w, h) = (width.max(1) as f64, height.max(1) as f64);
    let longest = w.max(h);
    let shortest = w.min(h);
    let mut scale = 1.0;
    if longest > MAX_SIDE as f64 {
        scale = MAX_SIDE as f64 / longest;
    }
    if shortest * scale < MIN_SIDE as f64 {
        scale = MIN_SIDE as f64 / shortest;
    }
    let side = |v: f64| ((v * scale).round() as u32).clamp(MIN_SIDE, MAX_SIDE);
    (side(w), side(h))
}

/// Return a copy of `frame` resized into the accepted size range
/// (unchanged when it already fits).
pub fn prepare_frame(frame: &Frame) -> Frame {
    let (w, h) = target_size(frame.width, frame.height);
    if (w, h) == (frame.width, frame.height) {
        return frame.clone();
    }
    let image = image::imageops::resize(&frame.image, w, h, image::imageops::FilterType::Triangle);
    Frame {
        image,
        width: w,
        height: h,
        captured_at: frame.captured_at,
    }
}

/// A prepared image with a fresh reference id.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageAsset {
    /// Reference string. Put it in `state` where the image belongs.
    pub id: String,
    pub asset: Asset,
}

impl ImageAsset {
    /// Resize `frame` and encode it as baseline JPEG.
    pub fn from_frame(frame: &Frame) -> Result<Self> {
        let jpeg = prepare_frame(frame).to_jpeg(JPEG_QUALITY)?;
        Ok(Self {
            id: new_asset_id(),
            asset: Asset::image(
                ImageFormat::Jpeg,
                base64::engine::general_purpose::STANDARD.encode(jpeg),
            ),
        })
    }
}

/// A new reference id, unique within the process and unlikely to occur in
/// normal text (the docs recommend a new id per image per request).
pub fn new_asset_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("ap-img-{nanos:x}-{n:x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn in_range(w: u32, h: u32) -> bool {
        (MIN_SIDE..=MAX_SIDE).contains(&w) && (MIN_SIDE..=MAX_SIDE).contains(&h)
    }

    #[test]
    fn target_sizes() {
        // iPhone 13 screenshot: scaled down, aspect kept.
        let (w, h) = target_size(1170, 2532);
        assert!(in_range(w, h));
        assert_eq!(h, 1568);
        assert_eq!(w, 725);
        // Already fits.
        assert_eq!(target_size(390, 844), (390, 844));
        // Too small: scaled up.
        let (w, h) = target_size(100, 200);
        assert_eq!((w, h), (280, 560));
        // Extreme ratio: clamped on both sides.
        let (w, h) = target_size(10_000, 100);
        assert!(in_range(w, h));
        assert_eq!((w, h), (1568, 280));
        assert!(in_range(target_size(0, 0).0, target_size(0, 0).1));
    }

    #[test]
    fn prepare_and_encode() {
        let f = Frame::solid(1170, 2532, [200, 10, 10, 255]);
        let p = prepare_frame(&f);
        assert_eq!((p.width, p.height), (725, 1568));
        assert_eq!(p.image.dimensions(), (725, 1568));
        let small = Frame::solid(300, 300, [0, 0, 0, 255]);
        assert_eq!(prepare_frame(&small).width, 300);

        let a = ImageAsset::from_frame(&Frame::solid(50, 100, [1, 2, 3, 255])).unwrap();
        assert_eq!(a.asset.format, ImageFormat::Jpeg);
        assert_eq!(a.asset.kind, "image");
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&a.asset.data_base64)
            .unwrap();
        let back = Frame::from_encoded(&bytes).unwrap();
        assert_eq!((back.width, back.height), (280, 560));
        assert_ne!(a.id, ImageAsset::from_frame(&small).unwrap().id);
    }
}
