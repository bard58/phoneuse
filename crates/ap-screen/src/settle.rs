//! Wait until the screen is still (port of `Frames.settle` in phoneuse `frames.py`).
//!
//! Upstream watched a continuous video stream. Here frames come from
//! [`Device::screenshot`] polling, so "still" means: the newest screenshot was
//! taken at least `quiet` after the last meaningful change, with no meaningful
//! change in between.
//!
//! Measured upstream: an action's first visible change comes ~0.1-0.15 s after
//! the input ends, animations run 0.4-0.7 s (scroll flings ~1-2.5 s), so 0.1 s
//! without a meaningful change means the animation ended.

use std::time::{Duration, Instant};

use ap_core::{Device, Frame, Result};

use crate::detect::{Change, ChangeDetector, SettleConfig};

/// Outcome of a settle.
#[derive(Clone)]
pub struct Settled {
    /// The newest frame.
    pub frame: Frame,
    /// Meaningful-change count after settling.
    pub seq: u64,
    /// The screen was still (false: timed out while it kept changing).
    pub still: bool,
    /// Returned because only small, never-ending changes remained (tiles
    /// cycling, clips swapping) after `ambient_after`.
    pub ambient: bool,
    /// Share of the picture ignored as video/animation.
    pub ignored: f32,
    /// Time spent in this settle.
    pub elapsed: Duration,
    /// Screenshots taken in this settle.
    pub captures: u32,
}

impl std::fmt::Debug for Settled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Settled")
            .field(
                "frame",
                &format_args!("{}x{}", self.frame.width, self.frame.height),
            )
            .field("seq", &self.seq)
            .field("still", &self.still)
            .field("ambient", &self.ambient)
            .field("ignored", &self.ignored)
            .field("elapsed", &self.elapsed)
            .field("captures", &self.captures)
            .finish()
    }
}

/// Keeps change-detection state across actions, like the upstream frames service.
///
/// Typical use: `let n = s.seq();` before an action, then
/// `s.settle(device, Some(n)).await` after it, so settle first waits (up to
/// `change_wait`) for the action's first visible change.
#[derive(Debug, Clone)]
pub struct Settler {
    detector: ChangeDetector,
    seq: u64,
    frame_at: Option<Instant>,
    big_at: Option<Instant>,
    last: Option<(Frame, Instant)>,
}

impl Default for Settler {
    fn default() -> Self {
        Self::new(SettleConfig::default())
    }
}

impl Settler {
    pub fn new(cfg: SettleConfig) -> Self {
        Self {
            detector: ChangeDetector::new(cfg),
            seq: 0,
            frame_at: None,
            big_at: None,
            last: None,
        }
    }

    pub fn config(&self) -> &SettleConfig {
        self.detector.config()
    }

    /// Meaningful picture changes seen so far.
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// The newest frame, if any.
    pub fn last_frame(&self) -> Option<&Frame> {
        self.last.as_ref().map(|(f, _)| f)
    }

    /// Feed a frame captured at `now` (for callers that have their own frames).
    pub fn observe(&mut self, frame: Frame, now: Instant) -> Change {
        let c = self.detector.observe(&frame, now);
        if c.changed {
            self.seq += 1;
            self.frame_at = Some(now);
            if c.big {
                self.big_at = Some(now);
            }
        }
        self.last = Some((frame, now));
        c
    }

    /// Take one screenshot, no sooner than `poll_interval` after the last one.
    pub async fn capture(&mut self, device: &dyn Device) -> Result<Change> {
        if let Some((_, at)) = &self.last {
            let next = *at + self.config().poll_interval;
            let now = Instant::now();
            if next > now {
                tokio::time::sleep(next - now).await;
            }
        }
        let frame = device.screenshot().await?;
        Ok(self.observe(frame, Instant::now()))
    }

    /// Wait until the screen is still, using the configured `quiet`,
    /// `change_wait` and `timeout`. With `since` (a [`seq`](Self::seq) taken
    /// before an action), first wait for a change after it. Returns at once
    /// when nothing happens. Always takes at least one fresh screenshot.
    pub async fn settle(&mut self, device: &dyn Device, since: Option<u64>) -> Result<Settled> {
        let cfg = self.config().clone();
        let start = Instant::now();
        let mut captures = 0u32;
        self.capture(device).await?;
        captures += 1;
        if let Some(since) = since {
            while self.seq <= since && start.elapsed() < cfg.change_wait.min(cfg.timeout) {
                self.capture(device).await?;
                captures += 1;
            }
        }
        let mut still = false;
        let mut ambient = false;
        loop {
            let (_, last_at) = self.last.as_ref().expect("captured above");
            let last_at = *last_at;
            let idle = self
                .frame_at
                .map_or(Duration::MAX, |t| last_at.saturating_duration_since(t));
            if idle >= cfg.quiet {
                still = true;
                break;
            }
            // After `ambient_after`, only big motion keeps us waiting.
            let since_big = self
                .big_at
                .map_or(Duration::MAX, |t| last_at.saturating_duration_since(t));
            if last_at.saturating_duration_since(start) > cfg.ambient_after
                && since_big >= cfg.quiet
            {
                still = true;
                ambient = true;
                break;
            }
            if start.elapsed() >= cfg.timeout {
                break;
            }
            self.capture(device).await?;
            captures += 1;
        }
        let elapsed = start.elapsed();
        tracing::debug!(seq = self.seq, still, ambient, captures, ?elapsed, "settle");
        let frame = self
            .last
            .as_ref()
            .map(|(f, _)| f.clone())
            .expect("captured");
        Ok(Settled {
            frame,
            seq: self.seq,
            still,
            ambient,
            ignored: self.detector.dynamic_fraction(),
            elapsed,
            captures,
        })
    }
}

/// Wait until the screen is still for `quiet_period` (at most `timeout`) and
/// return the newest frame. On timeout the newest frame is returned anyway,
/// and this function cannot say so: callers that report the screen state to
/// a model must use [`settle_with`] and check [`Settled::still`].
pub async fn settle(
    device: &dyn Device,
    timeout: Duration,
    quiet_period: Duration,
) -> Result<Frame> {
    let cfg = SettleConfig {
        timeout,
        quiet: quiet_period,
        ..SettleConfig::default()
    };
    Ok(settle_with(device, cfg).await?.frame)
}

/// Like [`settle()`] with every threshold configurable, returning details.
pub async fn settle_with(device: &dyn Device, cfg: SettleConfig) -> Result<Settled> {
    Settler::new(cfg).settle(device, None).await
}
