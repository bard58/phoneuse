//! Coordinate conversion and gesture paths.
//!
//! CoreDevice UniversalHID touch reports use a normalized `u16` space:
//! 0 = left/top, 65535 = right/bottom, the same for every screen size. An
//! ap-core [`Point`] is already normalized (0..1), so the conversion does not
//! need the screen size.

use std::time::Duration;

use ap_core::{Point, SwipeDirection};

/// Largest HID coordinate.
pub const HID_MAX: u16 = u16::MAX;

/// Interval between touch samples in a drag (about 60 Hz).
pub const SAMPLE_INTERVAL: Duration = Duration::from_millis(16);

/// Convert a normalized point to HID coordinates. Out-of-range values and NaN
/// are clamped.
pub fn to_hid(p: Point) -> (u16, u16) {
    let p = p.clamped();
    let n = |v: f32| (f64::from(v) * f64::from(HID_MAX)).round() as u16;
    (n(p.x), n(p.y))
}

/// Convert HID coordinates back to a normalized point.
pub fn from_hid(x: u16, y: u16) -> Point {
    let m = f32::from(HID_MAX);
    Point::new(f32::from(x) / m, f32::from(y) / m)
}

/// Number of touch samples for a drag of `duration` (at least 2).
pub fn steps_for(duration: Duration) -> u32 {
    let n = duration.as_millis() / SAMPLE_INTERVAL.as_millis();
    n.clamp(2, 10_000) as u32
}

/// Contact positions for a drag from `from` to `to` in `steps` samples. The
/// first sample is `from` and the last is `to` (so `steps + 1` points).
pub fn drag_path(from: Point, to: Point, steps: u32) -> Vec<(u16, u16)> {
    let steps = steps.max(1);
    (0..=steps)
        .map(|i| to_hid(from.lerp(to, i as f32 / steps as f32)))
        .collect()
}

/// Start and end of a scroll gesture. The finger moves in `direction` (the
/// ap-core [`SwipeDirection`] convention) by `amount` of the screen, centered
/// on `at`. `amount` is clamped to 0.05..=1 (NaN gives 0.5).
pub fn scroll_points(at: Point, direction: SwipeDirection, amount: f32) -> (Point, Point) {
    let amount = if amount.is_finite() {
        amount.clamp(0.05, 1.0)
    } else {
        0.5
    };
    let (dx, dy) = direction.vector();
    let half = amount / 2.0;
    (
        at.offset(-dx * half, -dy * half),
        at.offset(dx * half, dy * half),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hid_conversion() {
        assert_eq!(to_hid(Point::new(0.0, 0.0)), (0, 0));
        assert_eq!(to_hid(Point::new(1.0, 1.0)), (65535, 65535));
        assert_eq!(to_hid(Point::CENTER), (32768, 32768));
        assert_eq!(to_hid(Point::new(-1.0, f32::NAN)), (0, 0));
        assert_eq!(to_hid(Point::new(2.0, 1.5)), (65535, 65535));
        // iPhone 13: (195, 211) pt of 390x844.
        let p = Point::from_points(195.0, 211.0, ap_core::Size::new(390.0, 844.0));
        assert_eq!(to_hid(p), (32768, 16384));
        let back = from_hid(32768, 16384);
        assert!((back.x - 0.5).abs() < 1e-4 && (back.y - 0.25).abs() < 1e-4);
    }

    #[test]
    fn paths() {
        assert_eq!(steps_for(Duration::from_millis(300)), 18);
        assert_eq!(steps_for(Duration::ZERO), 2);
        let p = drag_path(Point::new(0.0, 1.0), Point::new(0.0, 0.0), 4);
        assert_eq!(p.len(), 5);
        assert_eq!(p[0], (0, 65535));
        assert_eq!(p[2], (0, 32768));
        assert_eq!(p[4], (0, 0));
    }

    #[test]
    fn scroll() {
        let (a, b) = scroll_points(Point::CENTER, SwipeDirection::Up, 0.5);
        assert_eq!(a, Point::new(0.5, 0.75));
        assert_eq!(b, Point::new(0.5, 0.25));
        let (a, b) = scroll_points(Point::CENTER, SwipeDirection::Right, f32::NAN);
        assert_eq!((a.x, b.x), (0.25, 0.75));
    }
}
