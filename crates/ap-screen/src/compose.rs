//! Put strip frames side by side in one image (upstream `_strip`).
//!
//! Upstream: one frame is shown at the phone's point size (402x874); several
//! frames at 2/3 of it (268x583 each), 6 px apart on a dark grey (40,40,40)
//! background. Three frames then cost about 625 tokens.

use ap_core::Frame;
use image::{Rgba, RgbaImage, imageops};

use crate::font;
use crate::llm::resize_exact;

/// Layout for [`compose_strip`].
#[derive(Debug, Clone, PartialEq)]
pub struct StripLayout {
    /// Longest side of a frame when the strip holds one frame. Default 874.
    pub single_max_side: u32,
    /// Longest side of each frame when there are several. Default 583 (2/3 of 874).
    pub multi_max_side: u32,
    /// Gap between frames in pixels. Default 6.
    pub gap: u32,
    /// Background (gap) colour. Default (40, 40, 40).
    pub background: [u8; 4],
    /// Label scale (5x7 font multiplier). Default 2.
    pub label_scale: u32,
    /// Label text colour. Default white.
    pub label_color: [u8; 4],
    /// Label box colour. Default black.
    pub label_background: [u8; 4],
}

impl Default for StripLayout {
    fn default() -> Self {
        Self {
            single_max_side: 874,
            multi_max_side: 583,
            gap: 6,
            background: [40, 40, 40, 255],
            label_scale: 2,
            label_color: [255, 255, 255, 255],
            label_background: [0, 0, 0, 255],
        }
    }
}

/// Compose `frames` left to right. All frames are scaled to the size of the
/// first one (fitted to the layout's max side). With `labels`, label `i` is
/// drawn at the top-left of frame `i` (e.g. `"+0.4s"`; see [`font`] for the
/// characters it can draw). Returns an empty 1x1 frame for no frames.
pub fn compose_strip(frames: &[Frame], layout: &StripLayout, labels: Option<&[String]>) -> Frame {
    let Some(first) = frames.first() else {
        return Frame::solid(1, 1, layout.background);
    };
    let max_side = if frames.len() == 1 {
        layout.single_max_side
    } else {
        layout.multi_max_side
    };
    let longest = first.width.max(first.height).max(1);
    let scale = if max_side == 0 || longest <= max_side {
        1.0
    } else {
        max_side as f64 / longest as f64
    };
    let w = ((first.width as f64 * scale).round() as u32).max(1);
    let h = ((first.height as f64 * scale).round() as u32).max(1);
    let n = frames.len() as u32;
    let mut out = RgbaImage::from_pixel(w * n + layout.gap * (n - 1), h, Rgba(layout.background));
    for (i, f) in frames.iter().enumerate() {
        let x = i as u32 * (w + layout.gap);
        let small = resize_exact(f, w, h);
        imageops::replace(&mut out, &small.image, x as i64, 0);
        if let Some(text) = labels.and_then(|l| l.get(i)).filter(|t| !t.is_empty()) {
            font::draw_label(
                &mut out,
                x as i64 + 4,
                4,
                text,
                layout.label_scale,
                layout.label_color,
                layout.label_background,
                3,
            );
        }
    }
    let mut frame = Frame::new(out);
    if let Some(last) = frames.last() {
        frame.captured_at = last.captured_at;
    }
    frame
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_like_upstream() {
        let f = Frame::solid(603, 1311, [255, 0, 0, 255]);
        let one = compose_strip(std::slice::from_ref(&f), &StripLayout::default(), None);
        assert_eq!((one.width, one.height), (402, 874));
        let three = compose_strip(
            &[f.clone(), f.clone(), f],
            &StripLayout::default(),
            Some(&["+0.1s".into(), "+0.4s".into(), "+0.8s".into()]),
        );
        assert_eq!((three.width, three.height), (268 * 3 + 12, 583));
        // Gap pixel is background, label box is black, frame is red.
        assert_eq!(three.image.get_pixel(270, 300).0, [40, 40, 40, 255]);
        assert_eq!(three.image.get_pixel(5, 5).0, [0, 0, 0, 255]);
        assert_eq!(three.image.get_pixel(100, 300).0, [255, 0, 0, 255]);
        assert_eq!(compose_strip(&[], &StripLayout::default(), None).width, 1);
    }
}
