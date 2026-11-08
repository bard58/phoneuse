//! Screen capture for the model: one settled frame (`look`) or a frame strip
//! right after an action (upstream phoneuse `_strip`).

use std::time::Duration;

use ap_core::{Device, Frame, Result};
use ap_llm::{ContentBlock, ImageMediaType};
use image::{Rgba, RgbaImage};

/// What the model gets to see.
#[derive(Debug, Clone, PartialEq)]
pub struct Observation {
    /// JPEG bytes.
    pub jpeg: Vec<u8>,
    /// Size of the whole image (a strip is wider than one frame).
    pub width: u32,
    pub height: u32,
    /// Size of one frame inside the image.
    pub frame_width: u32,
    pub frame_height: u32,
    pub frames: usize,
    /// Legend for the model.
    pub text: String,
}

impl Observation {
    /// Text legend then the image, ready for a message or tool result.
    pub fn to_blocks(&self) -> Vec<ContentBlock> {
        vec![
            ContentBlock::text(self.text.clone()),
            ContentBlock::image(ImageMediaType::Jpeg, &self.jpeg),
        ]
    }
}

/// How the agent sees the screen. Replace it to add OCR text or a different
/// capture strategy.
#[async_trait::async_trait]
pub trait Observer: Send + Sync {
    /// A fresh view after the screen has settled.
    async fn look(&self, device: &dyn Device) -> Result<Observation>;

    /// The screen just after an action (may show motion).
    async fn after_action(&self, device: &dyn Device) -> Result<Observation>;
}

/// Default observer: `ap_screen::settle` for `look`, `ap_screen::frame_strip`
/// for actions. Frames that do not differ from the one before are dropped,
/// so a still screen comes back as one full-size frame.
#[derive(Debug, Clone)]
pub struct ScreenObserver {
    /// Strip capture times after the action. Upstream: 0.1, 0.4, 0.8 s.
    pub strip_offsets: Vec<Duration>,
    pub settle_timeout: Duration,
    pub settle_quiet: Duration,
    /// Longest side of a single frame sent to the model.
    pub max_side: u32,
    /// Longest side of each frame inside a multi-frame strip.
    pub strip_max_side: u32,
    pub jpeg_quality: u8,
}

impl Default for ScreenObserver {
    fn default() -> Self {
        Self {
            strip_offsets: ap_screen::DEFAULT_STRIP_OFFSETS.to_vec(),
            settle_timeout: Duration::from_secs(3),
            settle_quiet: Duration::from_millis(100),
            // ~874 px tall on an iPhone 13, like upstream (screen points).
            max_side: 900,
            strip_max_side: 600,
            jpeg_quality: 80,
        }
    }
}

impl ScreenObserver {
    /// No waiting at all (tests).
    pub fn instant() -> Self {
        Self {
            strip_offsets: vec![Duration::ZERO],
            settle_timeout: Duration::ZERO,
            settle_quiet: Duration::ZERO,
            max_side: 400,
            strip_max_side: 300,
            jpeg_quality: 60,
        }
    }

    fn single(&self, frame: &Frame, legend: &str) -> Result<Observation> {
        let f = frame.downscaled(self.max_side);
        Ok(Observation {
            jpeg: f.to_jpeg(self.jpeg_quality)?,
            width: f.width,
            height: f.height,
            frame_width: f.width,
            frame_height: f.height,
            frames: 1,
            text: format!("{legend} One frame, {}x{} px.", f.width, f.height),
        })
    }
}

#[async_trait::async_trait]
impl Observer for ScreenObserver {
    async fn look(&self, device: &dyn Device) -> Result<Observation> {
        let frame = ap_screen::settle(device, self.settle_timeout, self.settle_quiet).await?;
        self.single(&frame, "Current screen (settled).")
    }

    async fn after_action(&self, device: &dyn Device) -> Result<Observation> {
        let frames = ap_screen::frame_strip(device, &self.strip_offsets).await?;
        let offsets: Vec<Duration> = self.strip_offsets.clone();
        let (kept, times) = dedup(&frames, &offsets);
        if kept.len() <= 1 {
            // Still screen: the last frame, full size.
            let last = frames.last().unwrap_or(&kept[0]);
            return self.single(last, "Screen after the action (it was still).");
        }
        let scaled: Vec<Frame> = kept
            .iter()
            .map(|f| f.downscaled(self.strip_max_side))
            .collect();
        let (fw, fh) = (scaled[0].width, scaled[0].height);
        let strip = compose(&scaled, 6);
        let times = times
            .iter()
            .map(|t| format!("+{:.1}", t.as_secs_f32()))
            .collect::<Vec<_>>()
            .join(", ");
        Ok(Observation {
            jpeg: strip.to_jpeg(self.jpeg_quality)?,
            width: strip.width,
            height: strip.height,
            frame_width: fw,
            frame_height: fh,
            frames: scaled.len(),
            text: format!(
                "Screen after the action: {n} frames at {times} s, left to right, each {fw}x{fh} px, \
                 6 px gaps. The screen was still changing; the rightmost frame is the newest. \
                 Coordinates refer to one frame, not the whole strip.",
                n = scaled.len()
            ),
        })
    }
}

/// Grey thumbnail, ~50 px wide, for change detection.
fn thumb(f: &Frame) -> image::GrayImage {
    let w = 50u32;
    let h = ((f.height as f32 * w as f32 / f.width.max(1) as f32).round() as u32).max(1);
    let small = image::imageops::thumbnail(&f.image, w, h);
    image::DynamicImage::ImageRgba8(small).to_luma8()
}

fn changed(a: &image::GrayImage, b: &image::GrayImage) -> bool {
    if a.dimensions() != b.dimensions() {
        return true;
    }
    let total = a.len().max(1) as f32;
    let diff = a
        .as_raw()
        .iter()
        .zip(b.as_raw())
        .filter(|(x, y)| (i16::from(**x) - i16::from(**y)).abs() > 12)
        .count() as f32;
    diff / total > 0.004
}

/// Keep frames that differ from the last kept one (upstream thresholds).
fn dedup(frames: &[Frame], offsets: &[Duration]) -> (Vec<Frame>, Vec<Duration>) {
    let mut kept = Vec::new();
    let mut times = Vec::new();
    let mut last: Option<image::GrayImage> = None;
    for (i, f) in frames.iter().enumerate() {
        let t = thumb(f);
        if last.as_ref().is_none_or(|l| changed(l, &t)) {
            kept.push(f.clone());
            times.push(offsets.get(i).copied().unwrap_or_default());
            last = Some(t);
        }
    }
    (kept, times)
}

/// Frames side by side on a dark background.
fn compose(frames: &[Frame], gap: u32) -> Frame {
    let w = frames[0].width;
    let h = frames.iter().map(|f| f.height).max().unwrap_or(1);
    let n = frames.len() as u32;
    let mut canvas = RgbaImage::from_pixel(w * n + gap * (n - 1), h, Rgba([40, 40, 40, 255]));
    for (i, f) in frames.iter().enumerate() {
        let x = i64::from(i as u32 * (w + gap));
        image::imageops::overlay(&mut canvas, &f.image, x, 0);
    }
    Frame::new(canvas)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedup_and_compose() {
        let a = Frame::solid(100, 200, [0, 0, 0, 255]);
        let b = Frame::solid(100, 200, [255, 255, 255, 255]);
        let offs = [
            Duration::from_millis(100),
            Duration::from_millis(400),
            Duration::from_millis(800),
        ];
        let (k, t) = dedup(&[a.clone(), a.clone(), b.clone()], &offs);
        assert_eq!(k.len(), 2);
        assert_eq!(t, vec![offs[0], offs[2]]);
        let s = compose(&k, 6);
        assert_eq!((s.width, s.height), (206, 200));
        assert_eq!(s.image.get_pixel(103, 10).0, [40, 40, 40, 255]);
        assert_eq!(s.image.get_pixel(150, 10).0, [255, 255, 255, 255]);
    }
}
