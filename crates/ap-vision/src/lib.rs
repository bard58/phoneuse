//! Optional OCR so an agent can find and tap on-screen text.
//!
//! - [`TextRecognizer`]: turns a [`Frame`] into [`TextBox`]es (normalized boxes).
//! - [`text`]: upstream phoneuse helpers: [`screen_lines`] (drop the status
//!   bar and glyph noise, sort in reading order), [`format_lines`]
//!   (`"x y  text"` lines as in `mcp_server.py`), [`find_text`] (tap_on target).
//! - With the cargo feature `ocr`: [`OcrsRecognizer`] (pure-Rust `ocrs` engine)
//!   and [`models`] (download with progress, pinned SHA-256 hashes checked at
//!   download and at load time).
//!
//! Without the `ocr` feature the crate still builds; [`default_recognizer`]
//! returns [`Error::Unsupported`] with a hint.

use ap_core::{Error, Frame, Point, Result, Size};

pub mod text;

#[cfg(feature = "ocr")]
pub mod models;
#[cfg(feature = "ocr")]
mod ocr;

pub use ap_core;
#[cfg(feature = "ocr")]
pub use ocr::OcrsRecognizer;
pub use text::{
    Coords, FindOptions, STATUS_BAR, find_text, find_text_box, find_text_with, format_lines,
    screen_lines, similarity,
};

/// True when this build includes an OCR engine.
pub const OCR_ENABLED: bool = cfg!(feature = "ocr");

/// A rectangle in normalized coordinates (0..1), top-left origin.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct NormRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl NormRect {
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// From a pixel rectangle in an image of `pixels` size.
    pub fn from_pixels(left: f32, top: f32, width: f32, height: f32, pixels: Size) -> Self {
        if pixels.is_empty() {
            return Self::default();
        }
        Self::new(
            left / pixels.width,
            top / pixels.height,
            width / pixels.width,
            height / pixels.height,
        )
    }

    /// Centre point (what a tap should hit).
    pub fn center(&self) -> Point {
        Point::new(self.x + self.width / 2.0, self.y + self.height / 2.0)
    }
}

/// One recognized text line.
#[derive(Debug, Clone, PartialEq)]
pub struct TextBox {
    pub text: String,
    /// Normalized bounding box.
    pub bbox: NormRect,
    /// 0..1. Engines without a score report 1.0.
    pub confidence: f32,
}

impl TextBox {
    pub fn new(text: impl Into<String>, bbox: NormRect, confidence: f32) -> Self {
        Self {
            text: text.into(),
            bbox,
            confidence,
        }
    }

    pub fn center(&self) -> Point {
        self.bbox.center()
    }
}

/// Reads text from a screen frame.
pub trait TextRecognizer: Send + Sync {
    /// Short engine name for logs ("ocrs").
    fn name(&self) -> &str {
        "ocr"
    }

    /// All text lines in the frame, with normalized boxes.
    fn recognize(&self, frame: &Frame) -> Result<Vec<TextBox>>;
}

/// The error returned when OCR is not available in this build.
pub fn ocr_unavailable() -> Error {
    Error::unsupported(
        "OCR: this build has no OCR engine. Rebuild with `--features ocr` \
         (cargo build -p ap-vision --features ocr), then run `agentphone setup ocr` \
         to download the models",
    )
}

/// A recognizer that always fails with [`ocr_unavailable`].
#[derive(Debug, Clone, Copy, Default)]
pub struct NoOcr;

impl TextRecognizer for NoOcr {
    fn name(&self) -> &str {
        "none"
    }

    fn recognize(&self, _frame: &Frame) -> Result<Vec<TextBox>> {
        Err(ocr_unavailable())
    }
}

/// The best recognizer this build has, with models from the default model
/// directory. Without the `ocr` feature: [`Error::Unsupported`] with a hint.
/// With it, but without models: [`Error::Setup`] ("run `agentphone setup ocr`").
pub fn default_recognizer() -> Result<Box<dyn TextRecognizer>> {
    #[cfg(feature = "ocr")]
    {
        let dir = models::default_model_dir()?;
        Ok(Box::new(OcrsRecognizer::load(&dir)?))
    }
    #[cfg(not(feature = "ocr"))]
    {
        Err(ocr_unavailable())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_center_and_pixels() {
        let r = NormRect::from_pixels(100.0, 200.0, 200.0, 100.0, Size::new(1000.0, 1000.0));
        assert_eq!(r, NormRect::new(0.1, 0.2, 0.2, 0.1));
        let c = r.center();
        assert!((c.x - 0.2).abs() < 1e-6 && (c.y - 0.25).abs() < 1e-6);
        assert_eq!(
            NormRect::from_pixels(1.0, 1.0, 1.0, 1.0, Size::default()),
            NormRect::default()
        );
    }

    #[test]
    fn no_ocr_is_unsupported_with_hint() {
        let e = NoOcr.recognize(&Frame::solid(2, 2, [0; 4])).unwrap_err();
        assert!(e.is_unsupported());
        let msg = e.to_string();
        assert!(msg.contains("--features ocr") && msg.contains("agentphone setup ocr"));
    }

    #[cfg(not(feature = "ocr"))]
    #[test]
    fn default_recognizer_without_feature() {
        assert!(default_recognizer().err().unwrap().is_unsupported());
        const { assert!(!OCR_ENABLED) };
    }
}
