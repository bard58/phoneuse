//! Text helpers ported from phoneuse `cli.py` (`screen_text`, `text_target`)
//! and `mcp_server.py` (the `"x y  text"` line format).

use ap_core::{Point, Size};

use crate::TextBox;

/// Lines whose centre is above this (time, signal, battery) are left out.
/// Upstream `STATUS_BAR`.
pub const STATUS_BAR: f32 = 0.055;

/// How OCR reads the back chevron at the top left; shown as `[back]`.
const BACK_GLYPHS: [&str; 5] = ["く", "<", "‹", "〈", "〈"];

/// Words ignored in a tap_on description ("Wi-Fi row" -> "wi-fi").
const FILLER: [&str; 10] = [
    "row", "button", "icon", "link", "tab", "cell", "item", "the", "label", "option",
];

/// Upstream reading-order row height for sorting (share of the screen).
const ROW: f32 = 0.012;

/// The screen's text lines as upstream shows them: unless `show_all`, drops
/// the status bar and lines without a letter or digit (icon glyphs OCR
/// mistakes for text); a back chevron near the top-left becomes `[back]`.
/// Sorted in rows top to bottom, then left to right.
pub fn screen_lines(boxes: &[TextBox], show_all: bool) -> Vec<TextBox> {
    let mut out: Vec<TextBox> = boxes
        .iter()
        .filter(|b| {
            show_all || (b.center().y >= STATUS_BAR && b.text.chars().any(char::is_alphanumeric))
        })
        .map(|b| {
            let mut b = b.clone();
            if BACK_GLYPHS.contains(&b.text.trim()) && b.center().x < 0.15 {
                b.text = "[back]".into();
            }
            b
        })
        .collect();
    out.sort_by(|a, b| {
        let (ca, cb) = (a.center(), b.center());
        let ra = (ca.y / ROW).round() as i64;
        let rb = (cb.y / ROW).round() as i64;
        ra.cmp(&rb).then(ca.x.total_cmp(&cb.x))
    });
    out
}

/// Coordinates used by [`format_lines`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Coords {
    /// Fractions of the screen, 3 decimals: `"0.512 0.304  Wi-Fi"` (upstream default).
    Normalized,
    /// Phone points (or pixels) of the given screen size, rounded: `"206 266  Wi-Fi"`.
    Points(Size),
}

/// One line per box: `"x y  text"` (box centre, two spaces before the text),
/// exactly as upstream `mcp_server.py` prints them.
pub fn format_lines(boxes: &[TextBox], coords: Coords) -> String {
    boxes
        .iter()
        .map(|b| {
            let c = b.center();
            match coords {
                Coords::Normalized => format!("{:.3} {:.3}  {}", c.x, c.y, b.text),
                Coords::Points(size) => {
                    let (x, y) = c.to_points(size);
                    format!("{:.0} {:.0}  {}", x, y, b.text)
                }
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Options for [`find_text_with`].
#[derive(Debug, Clone, PartialEq)]
pub struct FindOptions {
    /// Allow a close but inexact match (OCR misreads a character now and
    /// then). Default true.
    pub fuzzy: bool,
    /// Ignore lines whose centre is above this. Default [`STATUS_BAR`].
    pub min_y: f32,
    /// Fuzzy: minimum similarity. Default 0.85 (upstream).
    pub min_ratio: f32,
    /// Fuzzy: the best match must beat the second by this much. Default 0.1.
    pub margin: f32,
}

impl Default for FindOptions {
    fn default() -> Self {
        Self {
            fuzzy: true,
            min_y: STATUS_BAR,
            min_ratio: 0.85,
            margin: 0.1,
        }
    }
}

/// Centre of the text line that best matches `query`, or `None`. See
/// [`find_text_with`].
pub fn find_text(boxes: &[TextBox], query: &str, fuzzy: bool) -> Option<Point> {
    find_text_box(
        boxes,
        query,
        &FindOptions {
            fuzzy,
            ..FindOptions::default()
        },
    )
    .map(TextBox::center)
}

/// Like [`find_text`] with all options.
pub fn find_text_with(boxes: &[TextBox], query: &str, opts: &FindOptions) -> Option<Point> {
    find_text_box(boxes, query, opts).map(TextBox::center)
}

/// The matching box (upstream `text_target`). Case-insensitive; quotes and
/// filler words ("row", "button", ...) in `query` are ignored. Order:
/// 1. the first line equal to the query (in reading order);
/// 2. the only line starting with it;
/// 3. if `fuzzy`, the single closest line when it is close (similarity
///    >= `min_ratio`, and `margin` ahead of the next best).
///
/// Unlike a grounding model, this never guesses when the text is not on screen.
pub fn find_text_box<'a>(
    boxes: &'a [TextBox],
    query: &str,
    opts: &FindOptions,
) -> Option<&'a TextBox> {
    let cleaned = query.to_lowercase().replace('"', "");
    let words: Vec<&str> = cleaned
        .split_whitespace()
        .filter(|w| !FILLER.contains(w))
        .collect();
    if words.is_empty() {
        return None;
    }
    let want = words.join(" ");
    let mut lines: Vec<(&TextBox, String)> = boxes
        .iter()
        .filter(|b| b.center().y >= opts.min_y)
        .map(|b| (b, b.text.trim().to_lowercase()))
        .collect();
    lines.sort_by(|(a, _), (b, _)| {
        let (ca, cb) = (a.center(), b.center());
        ((ca.y / ROW).round() as i64)
            .cmp(&((cb.y / ROW).round() as i64))
            .then(ca.x.total_cmp(&cb.x))
    });
    if let Some((b, _)) = lines.iter().find(|(_, t)| *t == want) {
        return Some(b);
    }
    let starts: Vec<&TextBox> = lines
        .iter()
        .filter(|(_, t)| t.starts_with(&want))
        .map(|(b, _)| *b)
        .collect();
    if starts.len() == 1 {
        return Some(starts[0]);
    }
    if !opts.fuzzy {
        return None;
    }
    let mut scored: Vec<(f32, &TextBox)> = lines
        .iter()
        .map(|(b, t)| (similarity(&want, t), *b))
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    match scored.as_slice() {
        [(best, b), rest @ ..]
            if *best >= opts.min_ratio
                && rest
                    .first()
                    .is_none_or(|(second, _)| *second < best - opts.margin) =>
        {
            Some(b)
        }
        _ => None,
    }
}

/// Similarity 0..1 like Python's `difflib.SequenceMatcher(None, a, b).ratio()`
/// (Ratcliff/Obershelp: 2 * matches / total length), over characters.
pub fn similarity(a: &str, b: &str) -> f32 {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let total = a.len() + b.len();
    if total == 0 {
        return 1.0;
    }
    2.0 * matching_chars(&a, &b) as f32 / total as f32
}

fn matching_chars(a: &[char], b: &[char]) -> usize {
    // Longest common substring, earliest in `a`, then earliest in `b` (difflib order).
    let (mut best_i, mut best_j, mut best_k) = (0, 0, 0);
    let mut prev = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![0usize; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            if ca == cb {
                let k = prev[j] + 1;
                cur[j + 1] = k;
                if k > best_k {
                    best_k = k;
                    best_i = i + 1 - k;
                    best_j = j + 1 - k;
                }
            }
        }
        prev = cur;
    }
    if best_k == 0 {
        return 0;
    }
    best_k
        + matching_chars(&a[..best_i], &b[..best_j])
        + matching_chars(&a[best_i + best_k..], &b[best_j + best_k..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NormRect;

    fn tb(text: &str, cx: f32, cy: f32) -> TextBox {
        TextBox::new(text, NormRect::new(cx - 0.05, cy - 0.01, 0.1, 0.02), 1.0)
    }

    fn screen() -> Vec<TextBox> {
        vec![
            tb("9:41", 0.1, 0.02),
            tb("General", 0.3, 0.40),
            tb("Wi-Fi", 0.3, 0.20),
            tb("く", 0.05, 0.08),
            tb("<", 0.9, 0.08),
            tb("iMessage", 0.3, 0.50),
            tb("Messages", 0.3, 0.60),
            tb("Bluetooth", 0.3, 0.30),
            tb("On", 0.8, 0.30),
            tb("•••", 0.5, 0.70),
        ]
    }

    #[test]
    fn difflib_ratio() {
        // Values from Python's difflib.
        assert!((similarity("imessage", "message") - 0.9333333).abs() < 1e-5);
        assert!((similarity("abcd", "bcde") - 0.75).abs() < 1e-6);
        assert_eq!(similarity("", ""), 1.0);
        assert_eq!(similarity("abc", "xyz"), 0.0);
    }

    #[test]
    fn lines_and_format() {
        let lines = screen_lines(&screen(), false);
        let texts: Vec<&str> = lines.iter().map(|b| b.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "[back]",
                "Wi-Fi",
                "Bluetooth",
                "On",
                "General",
                "iMessage",
                "Messages"
            ]
        );
        let out = format_lines(&lines[1..3], Coords::Normalized);
        assert_eq!(out, "0.300 0.200  Wi-Fi\n0.300 0.300  Bluetooth");
        let pts = format_lines(&lines[1..2], Coords::Points(Size::new(400.0, 800.0)));
        assert_eq!(pts, "120 160  Wi-Fi");
        assert_eq!(screen_lines(&screen(), true).len(), 10);
    }

    #[test]
    fn find_like_text_target() {
        let s = screen();
        // Exact, case-insensitive, filler words ignored.
        let p = find_text(&s, "\"wi-fi\" row", false).unwrap();
        assert!((p.y - 0.2).abs() < 1e-6);
        // Unique prefix.
        assert!((find_text(&s, "blue", false).unwrap().y - 0.3).abs() < 1e-6);
        // Ambiguous prefix "me"? Only "messages" starts with it -> found.
        assert!((find_text(&s, "mess", false).unwrap().y - 0.6).abs() < 1e-6);
        // Status bar is ignored.
        assert!(find_text(&s, "9:41", true).is_none());
        // Fuzzy: OCR misread "Genera1".
        assert!(find_text(&s, "genera1", false).is_none());
        assert!((find_text(&s, "genera1", true).unwrap().y - 0.4).abs() < 1e-6);
        // Fuzzy but ambiguous: "message" is close to both iMessage and Messages.
        assert!(find_text(&s, "messag", true).is_some()); // unique prefix "messages"
        assert!(find_text(&s, "nothing here", true).is_none());
        assert!(find_text(&s, "the button", true).is_none());
    }
}
