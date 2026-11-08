//! Screen geometry. Agents work in normalized coordinates so the same
//! action works on any screen size; backends convert to points or pixels.

use serde::{Deserialize, Serialize};

/// A width/height pair, in points or pixels depending on context.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

impl Size {
    pub const fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }

    /// True when either side is zero, negative or not finite.
    pub fn is_empty(&self) -> bool {
        !(self.width.is_finite()
            && self.height.is_finite()
            && self.width > 0.0
            && self.height > 0.0)
    }

    /// Width / height ratio (0 when empty).
    pub fn aspect(&self) -> f32 {
        if self.is_empty() {
            0.0
        } else {
            self.width / self.height
        }
    }

    /// Scale factor on the x axis from `self` to `other` (e.g. points to pixels).
    pub fn scale_to(&self, other: Size) -> f32 {
        if self.is_empty() {
            1.0
        } else {
            other.width / self.width
        }
    }
}

/// A position on the screen, normalized: `x` and `y` are 0..1 from the top-left corner.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const CENTER: Point = Point { x: 0.5, y: 0.5 };

    /// Create a point. Values are not clamped; see [`Point::clamped`].
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    /// True when both coordinates are finite and inside 0..=1.
    pub fn is_valid(&self) -> bool {
        (0.0..=1.0).contains(&self.x) && (0.0..=1.0).contains(&self.y)
    }

    /// Clamp both coordinates to 0..=1 (NaN becomes 0).
    pub fn clamped(self) -> Self {
        let c = |v: f32| if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) };
        Self {
            x: c(self.x),
            y: c(self.y),
        }
    }

    /// Build from absolute coordinates in a space of `size` (pixels or points).
    pub fn from_absolute(x: f32, y: f32, size: Size) -> Self {
        if size.is_empty() {
            return Self::new(0.0, 0.0);
        }
        Self::new(x / size.width, y / size.height)
    }

    /// Convert to absolute coordinates in a space of `size`.
    pub fn to_absolute(self, size: Size) -> (f32, f32) {
        (self.x * size.width, self.y * size.height)
    }

    /// Build from pixel coordinates.
    pub fn from_pixels(x: u32, y: u32, pixels: Size) -> Self {
        Self::from_absolute(x as f32, y as f32, pixels)
    }

    /// Convert to integer pixel coordinates, clamped inside the image.
    pub fn to_pixels(self, pixels: Size) -> (u32, u32) {
        let p = self.clamped();
        let max_x = (pixels.width - 1.0).max(0.0);
        let max_y = (pixels.height - 1.0).max(0.0);
        let x = (p.x * pixels.width).round().min(max_x);
        let y = (p.y * pixels.height).round().min(max_y);
        (x as u32, y as u32)
    }

    /// Build from UI points (iOS points, Android pixels).
    pub fn from_points(x: f32, y: f32, points: Size) -> Self {
        Self::from_absolute(x, y, points)
    }

    /// Convert to UI points.
    pub fn to_points(self, points: Size) -> (f32, f32) {
        self.to_absolute(points)
    }

    /// Euclidean distance in normalized units.
    pub fn distance(self, other: Point) -> f32 {
        ((self.x - other.x).powi(2) + (self.y - other.y).powi(2)).sqrt()
    }

    /// Linear interpolation; `t` = 0 gives `self`, 1 gives `other`.
    pub fn lerp(self, other: Point, t: f32) -> Point {
        Point::new(
            self.x + (other.x - self.x) * t,
            self.y + (other.y - self.y) * t,
        )
    }

    /// Offset by `(dx, dy)` normalized units, clamped.
    pub fn offset(self, dx: f32, dy: f32) -> Point {
        Point::new(self.x + dx, self.y + dy).clamped()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_round_trip() {
        let px = Size::new(1170.0, 2532.0);
        let p = Point::from_pixels(585, 1266, px);
        assert!((p.x - 0.5).abs() < 1e-6 && (p.y - 0.5).abs() < 1e-6);
        assert_eq!(p.to_pixels(px), (585, 1266));
        assert_eq!(Point::new(1.0, 1.0).to_pixels(px), (1169, 2531));
        assert_eq!(Point::new(-3.0, f32::NAN).to_pixels(px), (0, 0));
    }

    #[test]
    fn points_and_validity() {
        let pts = Size::new(390.0, 844.0);
        let (x, y) = Point::new(0.5, 0.25).to_points(pts);
        assert_eq!((x, y), (195.0, 211.0));
        assert!(Point::CENTER.is_valid());
        assert!(!Point::new(1.1, 0.0).is_valid());
        assert!(!Point::new(f32::NAN, 0.0).is_valid());
        assert_eq!(
            Point::from_absolute(1.0, 1.0, Size::default()),
            Point::new(0.0, 0.0)
        );
        assert_eq!(pts.scale_to(Size::new(1170.0, 2532.0)), 3.0);
    }

    #[test]
    fn lerp_and_offset() {
        let a = Point::new(0.0, 0.0);
        let b = Point::new(1.0, 1.0);
        assert_eq!(a.lerp(b, 0.5), Point::new(0.5, 0.5));
        assert_eq!(b.offset(0.5, -0.25), Point::new(1.0, 0.75));
        assert!((a.distance(Point::new(0.3, 0.4)) - 0.5).abs() < 1e-6);
    }
}
