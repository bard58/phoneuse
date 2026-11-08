//! Meaningful-change detection, ported from phoneuse `frames.py`.
//!
//! Each frame is reduced to a coarse luma thumbnail (every 16th pixel). A frame
//! counts as a *meaningful change* when more than 0.4% of the thumbnail cells
//! differ from the thumbnail at the last meaningful change. A pointer, a blinking
//! caret or a small spinner stay under that.
//!
//! Cells that have moved continuously for more than 1 s (a playing video, an
//! animated banner) are *dynamic* and ignored, grown by 3 cells so the whole
//! video rectangle is covered. Page transitions and scroll flings finish well
//! within that time; video never does.
//!
//! Upstream received frames < 50 ms apart. Here frames come from screenshot
//! polling, and a slow device (Android `screencap`) can need 0.6-1.5 s per
//! frame. So the detector measures the gap between frames and stretches the
//! pause and dynamic thresholds to it: `pause` becomes at least 1.5 gaps and
//! `dynamic_after` at least 2 gaps. Without this, every move on a slow device
//! counts as "after a pause" and nothing ever becomes dynamic.

use std::time::{Duration, Instant};

use ap_core::Frame;

/// Thresholds for change detection and settling. Defaults are the values
/// measured upstream (phoneuse `frames.py`).
#[derive(Debug, Clone, PartialEq)]
pub struct SettleConfig {
    /// Sample every Nth pixel in both axes for the thumbnail. Default 16.
    pub thumb_step: u32,
    /// A cell differs when its luma changes by more than this (0..255). Default 12.
    pub cell_threshold: i16,
    /// A frame is a meaningful change when more than this share of the cells
    /// differ from the reference. Default 0.004 (0.4%).
    pub change_fraction: f32,
    /// A change of at least this share of the picture is *big* (a page, a scroll).
    /// Default 0.05.
    pub big_fraction: f32,
    /// A region moving continuously this long is video/animation and ignored.
    /// Default 1 s.
    pub dynamic_after: Duration,
    /// A cell still for this long has paused (pixels in a video pause now and
    /// then); its "moving since" timer restarts on its next move. Default 0.6 s.
    pub pause: Duration,
    /// Grow the dynamic mask by this many cells (~16 pt each way at 3). Default 3.
    pub dynamic_grow: u32,
    /// After this long, only big motion keeps [`settle`](crate::settle()) waiting;
    /// small changes that never stop are ambient. Default 0.4 s.
    pub ambient_after: Duration,
    /// The screen is still after no meaningful change for this long. Default 0.1 s.
    pub quiet: Duration,
    /// With a `since` count, wait up to this long for the action's first visible
    /// change (an action may change nothing). Default 0.6 s.
    pub change_wait: Duration,
    /// Give up waiting after this long and return the last frame. Default 3 s.
    pub timeout: Duration,
    /// Minimum time between two screenshots while polling. Default 50 ms
    /// (frames during an animation arrive < 50 ms apart upstream).
    pub poll_interval: Duration,
}

impl Default for SettleConfig {
    fn default() -> Self {
        Self {
            thumb_step: 16,
            cell_threshold: 12,
            change_fraction: 0.004,
            big_fraction: 0.05,
            dynamic_after: Duration::from_secs_f32(1.0),
            pause: Duration::from_secs_f32(0.6),
            dynamic_grow: 3,
            ambient_after: Duration::from_secs_f32(0.4),
            quiet: Duration::from_secs_f32(0.1),
            change_wait: Duration::from_secs_f32(0.6),
            timeout: Duration::from_secs_f32(3.0),
            poll_interval: Duration::from_millis(50),
        }
    }
}

/// Coarse luma thumbnail of a frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thumb {
    pub width: usize,
    pub height: usize,
    /// Row-major luma values (0..255) as i16 so differences do not overflow.
    pub data: Vec<i16>,
}

impl Thumb {
    /// Sample every `step`th pixel (BT.601 luma).
    pub fn from_frame(frame: &Frame, step: u32) -> Self {
        let step = step.max(1);
        let (w, h) = frame.image.dimensions();
        let tw = w.div_ceil(step) as usize;
        let th = h.div_ceil(step) as usize;
        let mut data = Vec::with_capacity(tw * th);
        for y in (0..h).step_by(step as usize) {
            for x in (0..w).step_by(step as usize) {
                data.push(luma(frame.image.get_pixel(x, y).0));
            }
        }
        Self {
            width: tw,
            height: th,
            data,
        }
    }

    fn same_shape(&self, other: &Thumb) -> bool {
        self.width == other.width && self.height == other.height
    }

    /// Share of cells whose luma differs by more than `threshold`.
    pub fn diff_fraction(&self, other: &Thumb, threshold: i16) -> f32 {
        if !self.same_shape(other) || self.data.is_empty() {
            return 1.0;
        }
        let n = self
            .data
            .iter()
            .zip(&other.data)
            .filter(|(a, b)| (**a - **b).abs() > threshold)
            .count();
        n as f32 / self.data.len() as f32
    }
}

/// BT.601 luma of an RGBA pixel.
pub(crate) fn luma(p: [u8; 4]) -> i16 {
    ((77 * p[0] as u32 + 150 * p[1] as u32 + 29 * p[2] as u32) >> 8) as i16
}

/// Result of feeding one frame to a [`ChangeDetector`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Change {
    /// The frame changed the picture meaningfully.
    pub changed: bool,
    /// Share of cells that differ from the reference, outside dynamic regions.
    pub fraction: f32,
    /// Share of cells that moved since the previous frame.
    pub moved_fraction: f32,
    /// Share of the picture currently ignored as video/animation.
    pub dynamic_fraction: f32,
    /// The change was big (`fraction >= big_fraction`).
    pub big: bool,
    /// The detector was reset (first frame or a new size); counts as a change.
    pub reset: bool,
}

/// Stateful change detector (one per screen stream).
#[derive(Debug, Clone)]
pub struct ChangeDetector {
    cfg: SettleConfig,
    prev: Option<Thumb>,
    reference: Option<Thumb>,
    last_move: Vec<Instant>,
    moving_since: Vec<Instant>,
    dynamic_fraction: f32,
    prev_at: Option<Instant>,
    /// Smoothed time between polled frames (gaps longer than [`MAX_POLL_GAP`]
    /// are idle time between looks and do not count).
    gap: Option<Duration>,
}

/// A gap between frames longer than this is idle time (between two settles),
/// not the polling interval of a slow device.
const MAX_POLL_GAP: Duration = Duration::from_millis(2500);

impl ChangeDetector {
    pub fn new(cfg: SettleConfig) -> Self {
        Self {
            cfg,
            prev: None,
            reference: None,
            last_move: Vec::new(),
            moving_since: Vec::new(),
            dynamic_fraction: 0.0,
            prev_at: None,
            gap: None,
        }
    }

    /// Smoothed time between polled frames, once measured.
    pub fn frame_gap(&self) -> Option<Duration> {
        self.gap
    }

    /// The pause and dynamic thresholds in use, stretched to the measured
    /// frame gap: `(pause, dynamic_after)`.
    pub fn effective_thresholds(&self) -> (Duration, Duration) {
        let gap = self.gap.unwrap_or_default();
        (
            self.cfg.pause.max(gap.mul_f32(1.5)),
            self.cfg.dynamic_after.max(gap * 2),
        )
    }

    fn record_gap(&mut self, now: Instant) {
        if let Some(prev_at) = self.prev_at.replace(now) {
            let sample = now.saturating_duration_since(prev_at);
            if sample.is_zero() || sample > MAX_POLL_GAP {
                return;
            }
            // Exponential moving average, weight 0.3 for the new sample.
            self.gap = Some(match self.gap {
                Some(g) => g.mul_f32(0.7) + sample.mul_f32(0.3),
                None => sample,
            });
        }
    }

    pub fn config(&self) -> &SettleConfig {
        &self.cfg
    }

    /// Share of the picture currently ignored as video/animation.
    pub fn dynamic_fraction(&self) -> f32 {
        self.dynamic_fraction
    }

    /// Forget all state; the next frame counts as a change.
    pub fn reset(&mut self) {
        self.prev = None;
        self.reference = None;
    }

    /// Feed a frame captured at `now`.
    pub fn observe(&mut self, frame: &Frame, now: Instant) -> Change {
        self.observe_thumb(Thumb::from_frame(frame, self.cfg.thumb_step), now)
    }

    /// Feed a precomputed thumbnail captured at `now`.
    #[allow(clippy::needless_range_loop)] // parallel per-cell arrays
    pub fn observe_thumb(&mut self, thumb: Thumb, now: Instant) -> Change {
        self.record_gap(now);
        let prev = self.prev.replace(thumb.clone());
        let (prev, reference) = match (prev, self.reference.as_ref()) {
            (Some(p), Some(r)) if p.same_shape(&thumb) && r.same_shape(&thumb) => (p, r),
            _ => {
                let n = thumb.data.len();
                self.reference = Some(thumb);
                self.last_move = vec![now; n];
                self.moving_since = vec![now; n];
                self.dynamic_fraction = 0.0;
                return Change {
                    changed: true,
                    fraction: 1.0,
                    moved_fraction: 1.0,
                    dynamic_fraction: 0.0,
                    big: true,
                    reset: true,
                };
            }
        };
        let (pause, dynamic_after) = self.effective_thresholds();
        let cfg = &self.cfg;
        let (w, h) = (thumb.width, thumb.height);
        let n = thumb.data.len();

        // Per cell: when it last moved, and since when it has moved without a pause.
        let mut moved_count = 0usize;
        for i in 0..n {
            if (thumb.data[i] - prev.data[i]).abs() > cfg.cell_threshold {
                moved_count += 1;
                if now.duration_since(self.last_move[i]) > pause {
                    self.moving_since[i] = now;
                }
                self.last_move[i] = now;
            }
        }
        let mut dynamic: Vec<bool> = (0..n)
            .map(|i| {
                now.duration_since(self.moving_since[i]) > dynamic_after
                    && now.duration_since(self.last_move[i]) < pause
            })
            .collect();
        // Grow (4-neighbourhood) so a whole video rectangle is covered.
        for _ in 0..cfg.dynamic_grow {
            let d = dynamic.clone();
            for y in 0..h {
                for x in 0..w {
                    let i = y * w + x;
                    if d[i] {
                        continue;
                    }
                    dynamic[i] = (y > 0 && d[i - w])
                        || (y + 1 < h && d[i + w])
                        || (x > 0 && d[i - 1])
                        || (x + 1 < w && d[i + 1]);
                }
            }
        }
        let differs = (0..n)
            .filter(|&i| {
                !dynamic[i] && (thumb.data[i] - reference.data[i]).abs() > cfg.cell_threshold
            })
            .count();
        let fraction = differs as f32 / n.max(1) as f32;
        let dynamic_fraction = dynamic.iter().filter(|d| **d).count() as f32 / n.max(1) as f32;
        self.dynamic_fraction = dynamic_fraction;
        let changed = fraction > cfg.change_fraction;
        let big = changed && fraction >= cfg.big_fraction;
        if changed {
            // Keep the old reference under dynamic regions.
            let mut r = reference.clone();
            for i in 0..n {
                if !dynamic[i] {
                    r.data[i] = thumb.data[i];
                }
            }
            self.reference = Some(r);
        }
        Change {
            changed,
            fraction,
            moved_fraction: moved_count as f32 / n.max(1) as f32,
            dynamic_fraction,
            big,
            reset: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    fn frame_with_box(w: u32, h: u32, bx: (u32, u32, u32, u32), v: u8) -> Frame {
        let mut img = RgbaImage::from_pixel(w, h, Rgba([20, 20, 20, 255]));
        for y in bx.1..bx.1 + bx.3 {
            for x in bx.0..bx.0 + bx.2 {
                img.put_pixel(x, y, Rgba([v, v, v, 255]));
            }
        }
        Frame::new(img)
    }

    #[test]
    fn thumb_size_and_luma() {
        let f = Frame::solid(100, 33, [255, 255, 255, 255]);
        let t = Thumb::from_frame(&f, 16);
        assert_eq!((t.width, t.height), (7, 3));
        assert!(t.data.iter().all(|v| *v == 255));
        assert_eq!(luma([0, 0, 0, 255]), 0);
    }

    #[test]
    fn small_changes_are_ignored_big_ones_count() {
        let mut d = ChangeDetector::new(SettleConfig::default());
        let t0 = Instant::now();
        let base = frame_with_box(320, 640, (0, 0, 0, 0), 0);
        assert!(d.observe(&base, t0).reset);
        assert!(!d.observe(&base, t0 + Duration::from_millis(50)).changed);
        // A caret: 1 cell of 20x40 = 800 cells (0.125%), below 0.4%.
        let caret = frame_with_box(320, 640, (160, 160, 4, 16), 250);
        assert!(!d.observe(&caret, t0 + Duration::from_millis(100)).changed);
        // A page: half the screen.
        let page = frame_with_box(320, 640, (0, 0, 320, 320), 250);
        let c = d.observe(&page, t0 + Duration::from_millis(150));
        assert!(c.changed && c.big && c.fraction > 0.4);
        // Same picture again: no change against the new reference.
        assert!(!d.observe(&page, t0 + Duration::from_millis(200)).changed);
    }

    #[test]
    fn continuous_motion_becomes_dynamic() {
        let mut d = ChangeDetector::new(SettleConfig::default());
        let t0 = Instant::now();
        // A 64x64 "video" flickering every 50 ms.
        let a = frame_with_box(320, 640, (64, 64, 64, 64), 250);
        let b = frame_with_box(320, 640, (64, 64, 64, 64), 100);
        d.observe(&a, t0);
        let mut last = None;
        for i in 1..=40u64 {
            let f = if i % 2 == 0 { &a } else { &b };
            last = Some(d.observe(f, t0 + Duration::from_millis(50 * i)));
        }
        let c = last.unwrap();
        assert!(!c.changed, "video after 2 s must not count: {c:?}");
        assert!(c.dynamic_fraction > 0.0);
        // Elsewhere a real change still counts.
        let mut big = frame_with_box(320, 640, (64, 64, 64, 64), 250);
        for y in 400..600 {
            for x in 0..320 {
                big.image.put_pixel(x, y, Rgba([255, 255, 255, 255]));
            }
        }
        assert!(d.observe(&big, t0 + Duration::from_millis(2050)).changed);
    }

    #[test]
    fn slow_polling_still_finds_video() {
        // Android screencap: one frame every 800 ms, longer than `pause`.
        let mut d = ChangeDetector::new(SettleConfig::default());
        let t0 = Instant::now();
        let a = frame_with_box(320, 640, (64, 64, 64, 64), 250);
        let b = frame_with_box(320, 640, (64, 64, 64, 64), 100);
        d.observe(&a, t0);
        let mut last = None;
        for i in 1..=5u64 {
            let f = if i % 2 == 0 { &a } else { &b };
            last = Some(d.observe(f, t0 + Duration::from_millis(800 * i)));
        }
        let c = last.unwrap();
        assert!(c.dynamic_fraction > 0.0, "video at 0.8 s polling: {c:?}");
        assert!(!c.changed, "video must not count as a change: {c:?}");
        let gap = d.frame_gap().unwrap();
        assert!(gap.abs_diff(Duration::from_millis(800)) < Duration::from_millis(5));
        let (pause, dynamic_after) = d.effective_thresholds();
        assert!(
            pause >= Duration::from_millis(1199) && dynamic_after >= Duration::from_millis(1599)
        );
    }

    #[test]
    fn idle_gaps_do_not_stretch_thresholds() {
        let mut d = ChangeDetector::new(SettleConfig::default());
        let t0 = Instant::now();
        let a = frame_with_box(320, 640, (0, 0, 0, 0), 0);
        d.observe(&a, t0);
        d.observe(&a, t0 + Duration::from_millis(50));
        // 10 s idle between two looks.
        d.observe(&a, t0 + Duration::from_secs(10));
        assert_eq!(d.frame_gap(), Some(Duration::from_millis(50)));
        assert_eq!(
            d.effective_thresholds(),
            (
                SettleConfig::default().pause,
                SettleConfig::default().dynamic_after
            )
        );
    }

    #[test]
    fn one_page_change_after_idle_is_not_dynamic() {
        // A cell that moved in the last look and again after a long idle
        // restarts its "moving since" timer: a single page change counts.
        let mut d = ChangeDetector::new(SettleConfig::default());
        let t0 = Instant::now();
        let a = frame_with_box(320, 640, (0, 0, 320, 320), 250);
        let b = frame_with_box(320, 640, (0, 0, 320, 320), 50);
        d.observe(&a, t0);
        for i in 1..=3u64 {
            d.observe(&a, t0 + Duration::from_millis(800 * i));
        }
        let c = d.observe(&b, t0 + Duration::from_secs(8));
        assert!(c.changed && c.big && c.dynamic_fraction == 0.0, "{c:?}");
    }
}
