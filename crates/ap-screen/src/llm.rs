//! Prepare frames for a vision model: downscale, then JPEG.
//!
//! Upstream resized a screenshot to the phone's size in points (402x874 on an
//! iPhone 17, ~470 tokens) with Lanczos and saved JPEG quality 85.

use ap_core::{Frame, Result};
use image::imageops::{self, FilterType};

/// Options for [`encode_for_llm`].
#[derive(Debug, Clone, PartialEq)]
pub struct LlmImageOptions {
    /// Longest side in pixels; 0 keeps the size. Default 874 (upstream: phone
    /// points, 402x874).
    pub max_side: u32,
    /// JPEG quality 1..=100. Default 85 (upstream single screenshot; strips use 82).
    pub quality: u8,
}

impl Default for LlmImageOptions {
    fn default() -> Self {
        Self {
            max_side: 874,
            quality: 85,
        }
    }
}

/// An encoded image ready to send to a model.
#[derive(Debug, Clone)]
pub struct LlmImage {
    pub jpeg: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

impl LlmImage {
    pub const MIME: &'static str = "image/jpeg";
}

/// Resize so the longest side is at most `max_side` (Lanczos3, keeps aspect).
/// `max_side` 0 or a smaller frame returns a clone.
pub fn resize_max_side(frame: &Frame, max_side: u32) -> Frame {
    let longest = frame.width.max(frame.height);
    if max_side == 0 || longest <= max_side {
        return frame.clone();
    }
    let scale = max_side as f64 / longest as f64;
    let w = ((frame.width as f64 * scale).round() as u32).max(1);
    let h = ((frame.height as f64 * scale).round() as u32).max(1);
    resize_exact(frame, w, h)
}

pub(crate) fn resize_exact(frame: &Frame, w: u32, h: u32) -> Frame {
    if (frame.width, frame.height) == (w, h) {
        return frame.clone();
    }
    let mut f = Frame::new(imageops::resize(&frame.image, w, h, FilterType::Lanczos3));
    f.captured_at = frame.captured_at;
    f
}

/// Downscale and JPEG-encode a frame.
pub fn encode_for_llm(frame: &Frame, opts: &LlmImageOptions) -> Result<LlmImage> {
    let small = resize_max_side(frame, opts.max_side);
    Ok(LlmImage {
        jpeg: small.to_jpeg(opts.quality)?,
        width: small.width,
        height: small.height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downscales_and_encodes() {
        let f = Frame::solid(603, 1311, [200, 10, 10, 255]);
        let img = encode_for_llm(&f, &LlmImageOptions::default()).unwrap();
        assert_eq!((img.width, img.height), (402, 874));
        assert_eq!(&img.jpeg[..2], &[0xFF, 0xD8]);
        let same = resize_max_side(&f, 0);
        assert_eq!(same.width, 603);
    }
}
