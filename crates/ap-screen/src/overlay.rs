//! Debug overlays: a marker at a point, and a labelled coordinate grid that
//! helps a model pick normalized coordinates.

use ap_core::{Frame, Point};
use image::RgbaImage;

use crate::font;

/// Style for [`draw_marker`].
#[derive(Debug, Clone, PartialEq)]
pub struct MarkerStyle {
    /// Ring radius as a share of the frame's shorter side. Default 0.03.
    pub radius: f32,
    /// Line thickness in pixels. Default 3.
    pub thickness: u32,
    /// Default red.
    pub color: [u8; 4],
}

impl Default for MarkerStyle {
    fn default() -> Self {
        Self {
            radius: 0.03,
            thickness: 3,
            color: [255, 0, 0, 255],
        }
    }
}

fn blend(img: &mut RgbaImage, x: i64, y: i64, c: [u8; 4]) {
    if x < 0 || y < 0 || x >= img.width() as i64 || y >= img.height() as i64 {
        return;
    }
    let p = img.get_pixel_mut(x as u32, y as u32);
    let a = c[3] as u32;
    for (dst, src) in p.0.iter_mut().zip(c).take(3) {
        *dst = ((src as u32 * a + *dst as u32 * (255 - a)) / 255) as u8;
    }
    p.0[3] = 255;
}

/// Draw a ring with a crosshair centred on `at` (normalized; clamped).
pub fn draw_marker(frame: &mut Frame, at: Point, style: &MarkerStyle) {
    let (cx, cy) = at.to_pixels(frame.size());
    let (cx, cy) = (cx as i64, cy as i64);
    let r = ((frame.width.min(frame.height) as f32 * style.radius).round() as i64).max(2);
    let t = style.thickness.max(1) as i64;
    let img = &mut frame.image;
    let (r_in, r_out) = ((r - t / 2 - t % 2) as f64, (r + t / 2) as f64);
    for y in cy - r - t..=cy + r + t {
        for x in cx - r - t..=cx + r + t {
            let d = (((x - cx).pow(2) + (y - cy).pow(2)) as f64).sqrt();
            let on_ring = d >= r_in && d <= r_out;
            let on_cross = ((x - cx).abs() < t.div_euclid(2) + 1 && (y - cy).abs() <= r / 2)
                || ((y - cy).abs() < t.div_euclid(2) + 1 && (x - cx).abs() <= r / 2);
            if on_ring || on_cross {
                blend(img, x, y, style.color);
            }
        }
    }
}

/// Options for [`draw_grid`].
#[derive(Debug, Clone, PartialEq)]
pub struct GridOptions {
    /// Lines per axis: 10 draws lines at 0.1, 0.2, ... 0.9. Default 10.
    pub divisions: u32,
    /// Line colour (alpha-blended). Default semi-transparent magenta.
    pub color: [u8; 4],
    /// Line thickness in pixels. Default 1.
    pub thickness: u32,
    /// Write the coordinate ("0.3") next to each line. Default true.
    pub labels: bool,
    /// Label scale (5x7 font multiplier). Default 2.
    pub label_scale: u32,
}

impl Default for GridOptions {
    fn default() -> Self {
        Self {
            divisions: 10,
            color: [255, 0, 255, 140],
            thickness: 1,
            labels: true,
            label_scale: 2,
        }
    }
}

/// Draw a normalized coordinate grid: vertical lines labelled with x along
/// the top edge, horizontal lines labelled with y along the left edge.
pub fn draw_grid(frame: &mut Frame, opts: &GridOptions) {
    let n = opts.divisions.max(1);
    let (w, h) = (frame.width as i64, frame.height as i64);
    let t = opts.thickness.max(1) as i64;
    let img = &mut frame.image;
    for k in 1..n {
        let x = w * k as i64 / n as i64;
        let y = h * k as i64 / n as i64;
        for yy in 0..h {
            for dx in 0..t {
                blend(img, x + dx - t / 2, yy, opts.color);
            }
        }
        for xx in 0..w {
            for dy in 0..t {
                if (x - t / 2..x - t / 2 + t).contains(&xx) {
                    continue; // already blended at the crossing
                }
                blend(img, xx, y + dy - t / 2, opts.color);
            }
        }
    }
    if opts.labels {
        let digits = if n <= 10 {
            1
        } else if n <= 100 {
            2
        } else {
            3
        };
        let solid = [opts.color[0], opts.color[1], opts.color[2], 255];
        for k in 1..n {
            let label = format!("{:.*}", digits, k as f64 / n as f64);
            let x = w * k as i64 / n as i64;
            let y = h * k as i64 / n as i64;
            let s = opts.label_scale;
            font::draw_label(
                img,
                x + 2,
                2,
                &label,
                s,
                [255, 255, 255, 255],
                [0, 0, 0, 200],
                1,
            );
            font::draw_label(img, 2, y + 2, &label, s, solid, [0, 0, 0, 200], 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_and_grid_change_pixels() {
        let mut f = Frame::solid(200, 400, [0, 0, 0, 255]);
        draw_marker(&mut f, Point::new(0.5, 0.5), &MarkerStyle::default());
        assert_eq!(f.image.get_pixel(100, 200).0, [255, 0, 0, 255]);
        assert_eq!(f.image.get_pixel(0, 0).0, [0, 0, 0, 255]);
        // Marker near a corner must not panic.
        draw_marker(&mut f, Point::new(1.0, 0.0), &MarkerStyle::default());

        let mut g = Frame::solid(200, 400, [0, 0, 0, 255]);
        draw_grid(
            &mut g,
            &GridOptions {
                labels: false,
                color: [255, 255, 255, 255],
                ..GridOptions::default()
            },
        );
        assert_eq!(g.image.get_pixel(100, 300).0, [255, 255, 255, 255]); // x = 0.5
        assert_eq!(g.image.get_pixel(150, 200).0, [255, 255, 255, 255]); // y = 0.5
        assert_eq!(g.image.get_pixel(105, 205).0, [0, 0, 0, 255]);
        draw_grid(&mut g, &GridOptions::default());
    }
}
