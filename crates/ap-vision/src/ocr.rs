//! OCR with the pure-Rust `ocrs` engine (rten inference, ONNX models).

use std::path::Path;

use ap_core::{Error, Frame, Result, Size};
use ocrs::{ImageSource, OcrEngine, OcrEngineParams, TextItem};
use rten::Model;

use crate::models::{DETECTION_MODEL, RECOGNITION_MODEL};
use crate::{NormRect, TextBox, TextRecognizer};

/// Words further apart than this many line heights become separate boxes, so
/// a settings row ("Wi-Fi ... HomeNet") gives two tap targets, like upstream
/// RapidOCR does.
const SPLIT_GAP: f32 = 1.5;

/// `ocrs` recognizer. Build in release mode: debug builds of `ocrs`/`rten`
/// are very slow.
pub struct OcrsRecognizer {
    engine: OcrEngine,
}

impl std::fmt::Debug for OcrsRecognizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OcrsRecognizer").finish_non_exhaustive()
    }
}

fn other(what: &str, e: impl std::fmt::Display) -> Error {
    Error::Other(format!("{what}: {e}"))
}

impl OcrsRecognizer {
    /// Load the two models from `dir` (see [`crate::models`]).
    pub fn load(dir: &Path) -> Result<Self> {
        let det = dir.join(DETECTION_MODEL);
        let rec = dir.join(RECOGNITION_MODEL);
        if !det.is_file() || !rec.is_file() {
            return Err(Error::setup(
                format!("OCR models not found in {}", dir.display()),
                "run `agentphone setup ocr` to download them (about 12 MB)",
            ));
        }
        Self::from_files(&det, &rec)
    }

    /// Load from explicit model paths (`.onnx`).
    pub fn from_files(detection: &Path, recognition: &Path) -> Result<Self> {
        let detection_model =
            Model::load_file(detection).map_err(|e| other("cannot load OCR detection model", e))?;
        let recognition_model = Model::load_file(recognition)
            .map_err(|e| other("cannot load OCR recognition model", e))?;
        let engine = OcrEngine::new(OcrEngineParams {
            detection_model: Some(detection_model),
            recognition_model: Some(recognition_model),
            ..Default::default()
        })
        .map_err(|e| other("cannot start OCR engine", e))?;
        Ok(Self { engine })
    }
}

impl TextRecognizer for OcrsRecognizer {
    fn name(&self) -> &str {
        "ocrs"
    }

    fn recognize(&self, frame: &Frame) -> Result<Vec<TextBox>> {
        let src = ImageSource::from_bytes(frame.image.as_raw(), frame.image.dimensions())
            .map_err(|e| other("bad image for OCR", e))?;
        let input = self
            .engine
            .prepare_input(src)
            .map_err(|e| other("OCR preprocess", e))?;
        let words = self
            .engine
            .detect_words(&input)
            .map_err(|e| other("OCR detection", e))?;
        let lines = self.engine.find_text_lines(&input, &words);
        let texts = self
            .engine
            .recognize_text(&input, &lines)
            .map_err(|e| other("OCR recognition", e))?;
        let size = frame.size();
        let mut out = Vec::new();
        for line in texts.into_iter().flatten() {
            split_line(&line, size, &mut out);
        }
        Ok(out)
    }
}

/// Split one recognized line at wide gaps between words.
fn split_line(line: &ocrs::TextLine, size: Size, out: &mut Vec<TextBox>) {
    let line_h = line.bounding_rect().height().max(1) as f32;
    let mut group: Vec<(String, [i32; 4])> = Vec::new();
    let flush = |group: &mut Vec<(String, [i32; 4])>, out: &mut Vec<TextBox>| {
        if group.is_empty() {
            return;
        }
        let text = group
            .iter()
            .map(|(t, _)| t.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let l = group.iter().map(|(_, r)| r[0]).min().unwrap_or(0);
        let t = group.iter().map(|(_, r)| r[1]).min().unwrap_or(0);
        let r = group.iter().map(|(_, r)| r[2]).max().unwrap_or(0);
        let b = group.iter().map(|(_, r)| r[3]).max().unwrap_or(0);
        group.clear();
        if text.trim().is_empty() {
            return;
        }
        out.push(TextBox::new(
            text,
            NormRect::from_pixels(l as f32, t as f32, (r - l) as f32, (b - t) as f32, size),
            1.0,
        ));
    };
    for word in line.words() {
        let rect = word.bounding_rect();
        let r = [
            rect.left(),
            rect.top(),
            rect.left() + rect.width(),
            rect.top() + rect.height(),
        ];
        if let Some((_, last)) = group.last()
            && (r[0] - last[2]) as f32 > SPLIT_GAP * line_h
        {
            flush(&mut group, out);
        }
        group.push((word.to_string(), r));
    }
    flush(&mut group, out);
}
