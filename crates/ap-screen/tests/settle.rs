//! Settle and strip against MockDevice and a scripted device with synthetic frames.

use std::sync::Arc;
use std::time::{Duration, Instant};

use ap_core::mock::MockDevice;
use ap_core::{
    Button, Capabilities, Device, DeviceInfo, Frame, Key, Point, Result, SwipeDirection,
    async_trait,
};
use ap_screen::{
    DEFAULT_STRIP_OFFSETS, SettleConfig, Settler, StripDedup, dedup_strip, frame_strip,
    frame_strip_timed, settle, settle_with, strip_legend,
};
use image::{Rgba, RgbaImage};

type Script = dyn Fn(Duration) -> Frame + Send + Sync;

/// A device whose screenshot is a function of the time since it was created.
struct Scripted {
    inner: MockDevice,
    t0: Instant,
    script: Arc<Script>,
    /// Time each screenshot takes (a slow device).
    delay: Duration,
}

impl Scripted {
    fn new(script: impl Fn(Duration) -> Frame + Send + Sync + 'static) -> Self {
        Self {
            inner: MockDevice::new(),
            t0: Instant::now(),
            script: Arc::new(script),
            delay: Duration::ZERO,
        }
    }

    fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }
}

#[async_trait]
impl Device for Scripted {
    fn info(&self) -> &DeviceInfo {
        self.inner.info()
    }
    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }
    async fn screenshot(&self) -> Result<Frame> {
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
        Ok((self.script)(self.t0.elapsed()))
    }
    async fn tap(&self, at: Point) -> Result<()> {
        self.inner.tap(at).await
    }
    async fn double_tap(&self, at: Point) -> Result<()> {
        self.inner.double_tap(at).await
    }
    async fn long_press(&self, at: Point, d: Duration) -> Result<()> {
        self.inner.long_press(at, d).await
    }
    async fn swipe(&self, a: Point, b: Point, d: Duration) -> Result<()> {
        self.inner.swipe(a, b, d).await
    }
    async fn scroll(&self, at: Point, dir: SwipeDirection, amount: f32) -> Result<()> {
        self.inner.scroll(at, dir, amount).await
    }
    async fn type_text(&self, text: &str) -> Result<()> {
        self.inner.type_text(text).await
    }
    async fn key(&self, key: &Key) -> Result<()> {
        self.inner.key(key).await
    }
    async fn button(&self, b: Button) -> Result<()> {
        self.inner.button(b).await
    }
    async fn launch_app(&self, id: &str) -> Result<()> {
        self.inner.launch_app(id).await
    }
}

const W: u32 = 390;
const H: u32 = 844;

/// Grey screen with a white bar whose top edge is at `y`.
fn page_at(y: u32) -> Frame {
    let mut img = RgbaImage::from_pixel(W, H, Rgba([30, 30, 30, 255]));
    for yy in y.min(H)..(y + 200).min(H) {
        for x in 0..W {
            img.put_pixel(x, yy, Rgba([240, 240, 240, 255]));
        }
    }
    Frame::new(img)
}

#[tokio::test]
async fn still_mock_settles_fast() {
    let m = MockDevice::new();
    let start = Instant::now();
    let f = settle(&m, Duration::from_secs(3), Duration::from_millis(100))
        .await
        .unwrap();
    assert_eq!((f.width, f.height), (1170, 2532));
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(m.screenshot_count() >= 2);
}

#[tokio::test]
async fn waits_for_animation_to_end() {
    // A bar slides down for 500 ms, then stops.
    let d = Scripted::new(|t| {
        let ms = t.as_millis().min(500) as u32;
        page_at(ms)
    });
    let s = settle_with(&d, SettleConfig::default()).await.unwrap();
    assert!(s.still, "{s:?}");
    assert!(s.elapsed >= Duration::from_millis(500), "{s:?}");
    assert!(s.elapsed < Duration::from_secs(2), "{s:?}");
    // The settled frame shows the final position.
    assert_eq!(s.frame.image.get_pixel(10, 550).0, [240, 240, 240, 255]);
}

#[tokio::test]
async fn video_region_does_not_block_settle() {
    // Static page with a flickering "video" box in the middle, forever.
    let d = Scripted::new(|t| {
        let mut f = page_at(600);
        let v = if (t.as_millis() / 40) % 2 == 0 {
            250
        } else {
            60
        };
        for y in 200..330 {
            for x in 100..290 {
                f.image.put_pixel(x, y, Rgba([v, v, v, 255]));
            }
        }
        f
    });
    let s = settle_with(&d, SettleConfig::default()).await.unwrap();
    // Small never-ending change: ambient return well before the 3 s limit.
    assert!(s.still, "{s:?}");
    assert!(s.elapsed < Duration::from_millis(2500), "{s:?}");
}

#[tokio::test]
async fn since_waits_for_first_change() {
    // Nothing until 300 ms, then the page jumps.
    let d = Scripted::new(|t| {
        if t < Duration::from_millis(300) {
            page_at(0)
        } else {
            page_at(400)
        }
    });
    let mut s = Settler::default();
    s.capture(&d).await.unwrap();
    let before = s.seq();
    let r = s.settle(&d, Some(before)).await.unwrap();
    assert!(r.seq > before, "{r:?}");
    assert!(r.still);
    assert_eq!(r.frame.image.get_pixel(10, 450).0, [240, 240, 240, 255]);
}

#[tokio::test]
async fn timeout_returns_last_frame() {
    // Keeps scrolling the whole screen: never still, always big.
    let d = Scripted::new(|t| page_at((t.as_millis() as u32 * 2) % H));
    let cfg = SettleConfig {
        timeout: Duration::from_millis(400),
        ..SettleConfig::default()
    };
    let s = settle_with(&d, cfg).await.unwrap();
    assert!(!s.still, "{s:?}");
    assert!(s.elapsed < Duration::from_millis(1500), "{s:?}");
}

#[tokio::test]
async fn strip_takes_frames_at_offsets() {
    let m = MockDevice::new();
    let start = Instant::now();
    let frames = frame_strip(&m, &DEFAULT_STRIP_OFFSETS).await.unwrap();
    assert_eq!(frames.len(), 3);
    assert!(start.elapsed() >= Duration::from_millis(800));
    // A still screen collapses to the last frame.
    assert_eq!(dedup_strip(&frames, &StripDedup::default()), vec![2]);

    let no_shots = MockDevice::new().with_capabilities(Capabilities::NONE);
    assert!(frame_strip(&no_shots, &[Duration::ZERO]).await.is_err());
}

#[tokio::test]
async fn timed_strip_reports_real_capture_times() {
    // Each screenshot takes 500 ms, longer than the 0.3 s gaps in the offsets.
    let d = Scripted::new(|_| page_at(0)).with_delay(Duration::from_millis(500));
    let frames = frame_strip_timed(&d, &DEFAULT_STRIP_OFFSETS).await.unwrap();
    let times: Vec<Duration> = frames.iter().map(|(_, t)| *t).collect();
    assert_eq!(times.len(), 3);
    // Midpoints: ~0.35 s, ~0.85 s, ~1.35 s, not 0.1 / 0.4 / 0.8 s.
    assert!(times[0] >= Duration::from_millis(340), "{times:?}");
    assert!(times[1] >= Duration::from_millis(840), "{times:?}");
    assert!(times[2] >= Duration::from_millis(1340), "{times:?}");
    assert!(times.windows(2).all(|w| w[0] < w[1]), "{times:?}");
    assert_ne!(
        strip_legend(&times),
        strip_legend(&DEFAULT_STRIP_OFFSETS),
        "the legend must use the real times"
    );
}

#[tokio::test]
async fn slow_polling_settles_on_video_across_looks() {
    // Each screenshot takes 800 ms. A static page with a "video" box that
    // changes on every frame: the detector must learn it is video.
    let d = Scripted::new(|t| {
        let mut f = page_at(600);
        let v = if (t.as_millis() / 800) % 2 == 0 {
            250
        } else {
            60
        };
        for y in 200..330 {
            for x in 100..290 {
                f.image.put_pixel(x, y, Rgba([v, v, v, 255]));
            }
        }
        f
    })
    .with_delay(Duration::from_millis(800));
    let mut s = Settler::default();
    let first = s.settle(&d, None).await.unwrap();
    let second = s.settle(&d, None).await.unwrap();
    assert!(
        second.ignored > 0.0 || first.ignored > 0.0,
        "video never ignored: {first:?} {second:?}"
    );
    assert!(second.still, "{second:?}");
}
