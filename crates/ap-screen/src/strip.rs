//! Frames right after an action (port of `_strip` in phoneuse `mcp_server.py`).
//!
//! Instead of waiting for the screen to settle, an action returns a few frames
//! taken shortly after it, so the agent sees what moved (a page sliding in,
//! video, a spinner) itself.

use std::time::{Duration, Instant};

use ap_core::{Device, Error, Frame, Result};
use image::imageops::{self, FilterType};

use crate::detect::luma;

/// Upstream `STRIP_AT`: +0.1, +0.4 and +0.8 s after the action.
pub const DEFAULT_STRIP_OFFSETS: [Duration; 3] = [
    Duration::from_millis(100),
    Duration::from_millis(400),
    Duration::from_millis(800),
];

/// Take one screenshot at each offset after now. If a screenshot takes longer
/// than the gap to the next offset, the next one is taken at once. Failed
/// screenshots are skipped (logged); it is an error only when all fail.
pub async fn frame_strip(device: &dyn Device, offsets: &[Duration]) -> Result<Vec<Frame>> {
    let t0 = Instant::now();
    let mut frames = Vec::with_capacity(offsets.len());
    let mut last_err = None;
    for at in offsets {
        let due = t0 + *at;
        let now = Instant::now();
        if due > now {
            tokio::time::sleep(due - now).await;
        }
        match device.screenshot().await {
            Ok(f) => frames.push(f),
            Err(e) => {
                tracing::warn!(error = %e, "strip screenshot failed");
                last_err = Some(e);
            }
        }
    }
    if frames.is_empty() {
        return Err(last_err.unwrap_or_else(|| Error::InvalidArgument("no strip offsets".into())));
    }
    Ok(frames)
}

/// How [`dedup_strip`] decides that a frame adds nothing.
#[derive(Debug, Clone, PartialEq)]
pub struct StripDedup {
    /// Width of the grey thumbnail used for comparison (height keeps aspect).
    /// Upstream 50 (50x109 on an iPhone).
    pub thumb_width: u32,
    /// Luma difference per pixel that counts. Upstream 12.
    pub cell_threshold: i16,
    /// Share of differing pixels needed to keep a frame. Upstream 0.004.
    pub change_fraction: f32,
}

impl Default for StripDedup {
    fn default() -> Self {
        Self {
            thumb_width: 50,
            cell_threshold: 12,
            change_fraction: 0.004,
        }
    }
}

fn small(f: &Frame, width: u32) -> Vec<i16> {
    let w = width.max(1);
    let h = ((f.height as u64 * w as u64) / f.width.max(1) as u64).max(1) as u32;
    imageops::resize(&f.image, w, h, FilterType::Triangle)
        .pixels()
        .map(|p| luma(p.0))
        .collect()
}

/// Indices of the frames worth showing: each kept frame differs from the one
/// kept before it. A still screen gives a single index, the *last* frame.
pub fn dedup_strip(frames: &[Frame], cfg: &StripDedup) -> Vec<usize> {
    if frames.is_empty() {
        return Vec::new();
    }
    let thumbs: Vec<Vec<i16>> = frames.iter().map(|f| small(f, cfg.thumb_width)).collect();
    let mut kept = vec![0usize];
    for i in 1..frames.len() {
        let a = &thumbs[i];
        let b = &thumbs[*kept.last().expect("non-empty")];
        let frac = if a.len() != b.len() {
            1.0
        } else {
            a.iter()
                .zip(b)
                .filter(|(x, y)| (**x - **y).abs() > cfg.cell_threshold)
                .count() as f32
                / a.len().max(1) as f32
        };
        if frac > cfg.change_fraction {
            kept.push(i);
        }
    }
    if kept.len() == 1 {
        vec![frames.len() - 1]
    } else {
        kept
    }
}

/// Upstream legend for a strip, e.g. `"frames at +0.1, +0.4 s, left to right"`,
/// or `"the screen was still (one frame)"` for a single frame.
pub fn strip_legend(offsets: &[Duration]) -> String {
    if offsets.len() <= 1 {
        return "the screen was still (one frame)".into();
    }
    let parts: Vec<String> = offsets.iter().map(|d| format_secs(*d)).collect();
    format!("frames at +{} s, left to right", parts.join(", +"))
}

/// Seconds like Python's `str(float)`: 0.1, 0.4, 1.0.
pub(crate) fn format_secs(d: Duration) -> String {
    let s = format!("{:.3}", d.as_secs_f64());
    let s = s.trim_end_matches('0');
    if s.ends_with('.') {
        format!("{s}0")
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legend_formats_like_upstream() {
        assert_eq!(
            strip_legend(&DEFAULT_STRIP_OFFSETS),
            "frames at +0.1, +0.4, +0.8 s, left to right"
        );
        assert_eq!(
            strip_legend(&[Duration::from_secs(1)]),
            "the screen was still (one frame)"
        );
        assert_eq!(format_secs(Duration::from_secs(1)), "1.0");
    }

    #[test]
    fn dedup_keeps_changes_and_last_when_still() {
        let a = Frame::solid(100, 200, [0, 0, 0, 255]);
        let b = Frame::solid(100, 200, [255, 255, 255, 255]);
        assert_eq!(
            dedup_strip(&[a.clone(), a.clone(), a.clone()], &StripDedup::default()),
            vec![2]
        );
        assert_eq!(
            dedup_strip(&[a.clone(), b.clone(), b.clone()], &StripDedup::default()),
            vec![0, 1]
        );
        assert_eq!(
            dedup_strip(&[a.clone(), b, a], &StripDedup::default()),
            vec![0, 1, 2]
        );
        assert!(dedup_strip(&[], &StripDedup::default()).is_empty());
    }
}
