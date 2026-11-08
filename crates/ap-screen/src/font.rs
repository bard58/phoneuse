//! A tiny 5x7 bitmap font for labels on debug images (no font files needed).
//!
//! Covers `0-9`, `A-Z` (lower case is drawn as upper case) and `+ - . , : / ( ) = %`
//! and space. Other characters are drawn as `?`.

use image::{Rgba, RgbaImage};

const W: u32 = 5;
const H: u32 = 7;

fn glyph(c: char) -> [u8; 7] {
    match c.to_ascii_uppercase() {
        '0' => [
            0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110,
        ],
        '1' => [
            0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        '2' => [
            0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111,
        ],
        '3' => [
            0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110,
        ],
        '4' => [
            0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010,
        ],
        '5' => [
            0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110,
        ],
        '6' => [
            0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110,
        ],
        '7' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000,
        ],
        '8' => [
            0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110,
        ],
        '9' => [
            0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100,
        ],
        'A' => [
            0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'B' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110,
        ],
        'C' => [
            0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110,
        ],
        'D' => [
            0b11100, 0b10010, 0b10001, 0b10001, 0b10001, 0b10010, 0b11100,
        ],
        'E' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
        ],
        'F' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'G' => [
            0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01111,
        ],
        'H' => [
            0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'I' => [
            0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        'J' => [
            0b00111, 0b00010, 0b00010, 0b00010, 0b00010, 0b10010, 0b01100,
        ],
        'K' => [
            0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001,
        ],
        'L' => [
            0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111,
        ],
        'M' => [
            0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001,
        ],
        'N' => [
            0b10001, 0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001,
        ],
        'O' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'P' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'Q' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101,
        ],
        'R' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001,
        ],
        'S' => [
            0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        'T' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'U' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'V' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100,
        ],
        'W' => [
            0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b10101, 0b01010,
        ],
        'X' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001,
        ],
        'Y' => [
            0b10001, 0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100,
        ],
        'Z' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111,
        ],
        '+' => [
            0b00000, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0b00000,
        ],
        '-' => [
            0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000,
        ],
        '.' => [
            0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b01100, 0b01100,
        ],
        ',' => [
            0b00000, 0b00000, 0b00000, 0b00000, 0b01100, 0b00100, 0b01000,
        ],
        ':' => [
            0b00000, 0b01100, 0b01100, 0b00000, 0b01100, 0b01100, 0b00000,
        ],
        '/' => [
            0b00000, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b00000,
        ],
        '(' => [
            0b00010, 0b00100, 0b01000, 0b01000, 0b01000, 0b00100, 0b00010,
        ],
        ')' => [
            0b01000, 0b00100, 0b00010, 0b00010, 0b00010, 0b00100, 0b01000,
        ],
        '=' => [
            0b00000, 0b00000, 0b11111, 0b00000, 0b11111, 0b00000, 0b00000,
        ],
        '%' => [
            0b11000, 0b11001, 0b00010, 0b00100, 0b01000, 0b10011, 0b00011,
        ],
        ' ' => [0; 7],
        _ => [
            0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b00000, 0b00100,
        ],
    }
}

/// Size in pixels of `text` drawn at `scale` (1 px gap between glyphs).
pub fn text_size(text: &str, scale: u32) -> (u32, u32) {
    let n = text.chars().count() as u32;
    if n == 0 {
        return (0, 0);
    }
    let s = scale.max(1);
    ((n * (W + 1) - 1) * s, H * s)
}

/// Draw `text` with its top-left corner at (`x`, `y`). Pixels outside the
/// image are skipped.
pub fn draw_text(img: &mut RgbaImage, x: i64, y: i64, text: &str, scale: u32, color: [u8; 4]) {
    let s = scale.max(1) as i64;
    let (iw, ih) = (img.width() as i64, img.height() as i64);
    for (k, c) in text.chars().enumerate() {
        let gx = x + k as i64 * (W as i64 + 1) * s;
        for (row, bits) in glyph(c).iter().enumerate() {
            for col in 0..W as i64 {
                if bits & (1 << (W as i64 - 1 - col)) == 0 {
                    continue;
                }
                for dy in 0..s {
                    for dx in 0..s {
                        let px = gx + col * s + dx;
                        let py = y + row as i64 * s + dy;
                        if (0..iw).contains(&px) && (0..ih).contains(&py) {
                            img.put_pixel(px as u32, py as u32, Rgba(color));
                        }
                    }
                }
            }
        }
    }
}

/// Draw `text` on a filled box with `pad` pixels around it.
#[allow(clippy::too_many_arguments)]
pub fn draw_label(
    img: &mut RgbaImage,
    x: i64,
    y: i64,
    text: &str,
    scale: u32,
    color: [u8; 4],
    background: [u8; 4],
    pad: u32,
) {
    let (tw, th) = text_size(text, scale);
    let p = pad as i64;
    let (iw, ih) = (img.width() as i64, img.height() as i64);
    for py in y..y + th as i64 + 2 * p {
        for px in x..x + tw as i64 + 2 * p {
            if (0..iw).contains(&px) && (0..ih).contains(&py) {
                img.put_pixel(px as u32, py as u32, Rgba(background));
            }
        }
    }
    draw_text(img, x + p, y + p, text, scale, color);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_inside_bounds() {
        assert_eq!(text_size("+0.4s", 2), (58, 14));
        assert_eq!(text_size("", 2), (0, 0));
        let mut img = RgbaImage::from_pixel(20, 10, Rgba([0, 0, 0, 255]));
        draw_text(&mut img, -3, 1, "1x?", 1, [255, 0, 0, 255]);
        assert!(img.pixels().any(|p| p.0 == [255, 0, 0, 255]));
        draw_label(&mut img, 15, 5, "W", 3, [1, 1, 1, 255], [9, 9, 9, 255], 2);
    }
}
