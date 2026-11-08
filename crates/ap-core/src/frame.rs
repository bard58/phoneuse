//! A captured screen image.

use std::io::Cursor;
use std::time::SystemTime;

use image::{ImageFormat, Rgba, RgbaImage};

use crate::{Error, Result, Size};

/// One screenshot. Pixels are kept decoded as RGBA.
#[derive(Debug, Clone)]
pub struct Frame {
    pub image: RgbaImage,
    pub width: u32,
    pub height: u32,
    pub captured_at: SystemTime,
}

impl Frame {
    /// Wrap a decoded image, stamped now.
    pub fn new(image: RgbaImage) -> Self {
        let (width, height) = image.dimensions();
        Self {
            image,
            width,
            height,
            captured_at: SystemTime::now(),
        }
    }

    /// A frame of one colour (for tests and placeholders).
    pub fn solid(width: u32, height: u32, rgba: [u8; 4]) -> Self {
        Self::new(RgbaImage::from_pixel(width, height, Rgba(rgba)))
    }

    /// Decode PNG or JPEG bytes.
    pub fn from_encoded(bytes: &[u8]) -> Result<Self> {
        let img = image::load_from_memory(bytes)
            .map_err(|e| Error::Protocol(format!("cannot decode screenshot: {e}")))?;
        Ok(Self::new(img.to_rgba8()))
    }

    /// Build from raw RGBA bytes (`width * height * 4` long).
    pub fn from_rgba(width: u32, height: u32, data: Vec<u8>) -> Result<Self> {
        RgbaImage::from_raw(width, height, data)
            .map(Self::new)
            .ok_or_else(|| {
                Error::Protocol(format!("raw buffer does not match {width}x{height} RGBA"))
            })
    }

    /// Pixel size as [`Size`].
    pub fn size(&self) -> Size {
        Size::new(self.width as f32, self.height as f32)
    }

    /// Encode as PNG.
    pub fn to_png(&self) -> Result<Vec<u8>> {
        let mut out = Cursor::new(Vec::new());
        self.image
            .write_to(&mut out, ImageFormat::Png)
            .map_err(|e| Error::Other(format!("PNG encode failed: {e}")))?;
        Ok(out.into_inner())
    }

    /// Encode as JPEG with the given quality (1..=100); alpha is dropped.
    pub fn to_jpeg(&self, quality: u8) -> Result<Vec<u8>> {
        let rgb = image::DynamicImage::ImageRgba8(self.image.clone()).to_rgb8();
        let mut out = Vec::new();
        let enc =
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality.clamp(1, 100));
        rgb.write_with_encoder(enc)
            .map_err(|e| Error::Other(format!("JPEG encode failed: {e}")))?;
        Ok(out)
    }

    /// Resize so the longest side is at most `max_side` pixels (keeps aspect).
    pub fn downscaled(&self, max_side: u32) -> Frame {
        let longest = self.width.max(self.height);
        if max_side == 0 || longest <= max_side {
            return self.clone();
        }
        let scale = max_side as f32 / longest as f32;
        let w = ((self.width as f32 * scale).round() as u32).max(1);
        let h = ((self.height as f32 * scale).round() as u32).max(1);
        let image =
            image::imageops::resize(&self.image, w, h, image::imageops::FilterType::Triangle);
        Frame {
            image,
            width: w,
            height: h,
            captured_at: self.captured_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_round_trip_and_downscale() {
        let f = Frame::solid(40, 80, [10, 20, 30, 255]);
        let png = f.to_png().unwrap();
        let g = Frame::from_encoded(&png).unwrap();
        assert_eq!((g.width, g.height), (40, 80));
        assert_eq!(g.image.get_pixel(5, 5).0, [10, 20, 30, 255]);
        let s = f.downscaled(20);
        assert_eq!((s.width, s.height), (10, 20));
        assert!(!f.to_jpeg(80).unwrap().is_empty());
        assert!(Frame::from_rgba(2, 2, vec![0; 3]).is_err());
        assert!(Frame::from_encoded(b"nope").is_err());
    }
}
