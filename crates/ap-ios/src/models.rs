//! iPhone model table: `ProductType` -> marketing name, screen size, newest iOS.
//!
//! Lockdown has no documented screen-size key, so `screen_points` comes from
//! this table. `pixels` is the size of a screenshot: points times the UI
//! scale. On the mini models and the 6s/7/8 Plus models the panel has fewer
//! pixels than this (iOS renders at @3x and then downsamples), but a
//! screenshot has the rendered size.
//!
//! A model that is not in this table (for example an iPhone released after
//! this code) does not cause an error. [`screen_sizes`] gives a default size,
//! and `IosDevice::connect` replaces it with the size of the first
//! screenshot (see [`sizes_from_screenshot`]). Touch input uses normalized
//! coordinates, so it does not need the screen size.
//!
//! Sources: Apple tech specs (pixels), useyourloaf.com "iPhone 17 Screen
//! Sizes" (points and scale), The Apple Wiki and the adamawolf identifier
//! list (`ProductType`), Apple iOS 26 / iOS 27 compatibility lists (newest
//! iOS). The `iPhone18,x` and `iPhone19,x` identifiers are from public device
//! databases and are not verified on a device.

use ap_core::Size;

/// One iPhone model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IphoneModel {
    /// Lockdown `ProductType`, e.g. `iPhone14,5`.
    pub product_type: &'static str,
    /// Marketing name, e.g. `iPhone 13`.
    pub name: &'static str,
    /// UI points (width, height) in portrait.
    pub points: (u16, u16),
    /// Screenshot pixels (width, height) in portrait.
    pub pixels: (u16, u16),
    /// Newest major iOS version for this model (`u16::MAX` = still updated).
    pub max_ios_major: u16,
}

impl IphoneModel {
    pub fn screen_points(&self) -> Size {
        Size::new(self.points.0 as f32, self.points.1 as f32)
    }

    pub fn screen_pixels(&self) -> Size {
        Size::new(self.pixels.0 as f32, self.pixels.1 as f32)
    }

    /// True when the model can run iOS 17.4+, the only supported route.
    pub fn can_run_supported_ios(&self) -> bool {
        self.max_ios_major >= 17
    }
}

const CURRENT: u16 = u16::MAX;

macro_rules! m {
    ($pt:literal, $name:literal, $pw:literal x $ph:literal, $xw:literal x $xh:literal, $max:expr) => {
        IphoneModel {
            product_type: $pt,
            name: $name,
            points: ($pw, $ph),
            pixels: ($xw, $xh),
            max_ios_major: $max,
        }
    };
}

/// Every iPhone that runs iOS 15 or later.
pub const MODELS: &[IphoneModel] = &[
    m!("iPhone8,1", "iPhone 6s", 375 x 667, 750 x 1334, 15),
    m!("iPhone8,2", "iPhone 6s Plus", 414 x 736, 1242 x 2208, 15),
    m!("iPhone8,4", "iPhone SE (1st generation)", 320 x 568, 640 x 1136, 15),
    m!("iPhone9,1", "iPhone 7", 375 x 667, 750 x 1334, 15),
    m!("iPhone9,3", "iPhone 7", 375 x 667, 750 x 1334, 15),
    m!("iPhone9,2", "iPhone 7 Plus", 414 x 736, 1242 x 2208, 15),
    m!("iPhone9,4", "iPhone 7 Plus", 414 x 736, 1242 x 2208, 15),
    m!("iPhone10,1", "iPhone 8", 375 x 667, 750 x 1334, 16),
    m!("iPhone10,4", "iPhone 8", 375 x 667, 750 x 1334, 16),
    m!("iPhone10,2", "iPhone 8 Plus", 414 x 736, 1242 x 2208, 16),
    m!("iPhone10,5", "iPhone 8 Plus", 414 x 736, 1242 x 2208, 16),
    m!("iPhone10,3", "iPhone X", 375 x 812, 1125 x 2436, 16),
    m!("iPhone10,6", "iPhone X", 375 x 812, 1125 x 2436, 16),
    m!("iPhone11,2", "iPhone XS", 375 x 812, 1125 x 2436, 18),
    m!("iPhone11,4", "iPhone XS Max", 414 x 896, 1242 x 2688, 18),
    m!("iPhone11,6", "iPhone XS Max", 414 x 896, 1242 x 2688, 18),
    m!("iPhone11,8", "iPhone XR", 414 x 896, 828 x 1792, 18),
    m!("iPhone12,1", "iPhone 11", 414 x 896, 828 x 1792, CURRENT),
    m!("iPhone12,3", "iPhone 11 Pro", 375 x 812, 1125 x 2436, CURRENT),
    m!("iPhone12,5", "iPhone 11 Pro Max", 414 x 896, 1242 x 2688, CURRENT),
    m!("iPhone12,8", "iPhone SE (2nd generation)", 375 x 667, 750 x 1334, CURRENT),
    m!("iPhone13,1", "iPhone 12 mini", 375 x 812, 1125 x 2436, CURRENT),
    m!("iPhone13,2", "iPhone 12", 390 x 844, 1170 x 2532, CURRENT),
    m!("iPhone13,3", "iPhone 12 Pro", 390 x 844, 1170 x 2532, CURRENT),
    m!("iPhone13,4", "iPhone 12 Pro Max", 428 x 926, 1284 x 2778, CURRENT),
    m!("iPhone14,4", "iPhone 13 mini", 375 x 812, 1125 x 2436, CURRENT),
    m!("iPhone14,5", "iPhone 13", 390 x 844, 1170 x 2532, CURRENT),
    m!("iPhone14,2", "iPhone 13 Pro", 390 x 844, 1170 x 2532, CURRENT),
    m!("iPhone14,3", "iPhone 13 Pro Max", 428 x 926, 1284 x 2778, CURRENT),
    m!("iPhone14,6", "iPhone SE (3rd generation)", 375 x 667, 750 x 1334, CURRENT),
    m!("iPhone14,7", "iPhone 14", 390 x 844, 1170 x 2532, CURRENT),
    m!("iPhone14,8", "iPhone 14 Plus", 428 x 926, 1284 x 2778, CURRENT),
    m!("iPhone15,2", "iPhone 14 Pro", 393 x 852, 1179 x 2556, CURRENT),
    m!("iPhone15,3", "iPhone 14 Pro Max", 430 x 932, 1290 x 2796, CURRENT),
    m!("iPhone15,4", "iPhone 15", 393 x 852, 1179 x 2556, CURRENT),
    m!("iPhone15,5", "iPhone 15 Plus", 430 x 932, 1290 x 2796, CURRENT),
    m!("iPhone16,1", "iPhone 15 Pro", 393 x 852, 1179 x 2556, CURRENT),
    m!("iPhone16,2", "iPhone 15 Pro Max", 430 x 932, 1290 x 2796, CURRENT),
    m!("iPhone17,3", "iPhone 16", 393 x 852, 1179 x 2556, CURRENT),
    m!("iPhone17,4", "iPhone 16 Plus", 430 x 932, 1290 x 2796, CURRENT),
    m!("iPhone17,1", "iPhone 16 Pro", 402 x 874, 1206 x 2622, CURRENT),
    m!("iPhone17,2", "iPhone 16 Pro Max", 440 x 956, 1320 x 2868, CURRENT),
    m!("iPhone17,5", "iPhone 16e", 390 x 844, 1170 x 2532, CURRENT),
    m!("iPhone18,3", "iPhone 17", 402 x 874, 1206 x 2622, CURRENT),
    m!("iPhone18,1", "iPhone 17 Pro", 402 x 874, 1206 x 2622, CURRENT),
    m!("iPhone18,2", "iPhone 17 Pro Max", 440 x 956, 1320 x 2868, CURRENT),
    m!("iPhone18,4", "iPhone Air", 420 x 912, 1260 x 2736, CURRENT),
    m!("iPhone18,5", "iPhone 17e", 390 x 844, 1170 x 2532, CURRENT),
    m!("iPhone19,2", "iPhone 18 Pro", 402 x 874, 1206 x 2622, CURRENT),
    m!("iPhone19,3", "iPhone 18 Pro Max", 440 x 956, 1320 x 2868, CURRENT),
    m!("iPhone19,7", "iPhone 18 Pro Max", 440 x 956, 1320 x 2868, CURRENT),
];

/// Default size for a `ProductType` that is not in [`MODELS`] (iPhone 13).
/// It is a placeholder until the first screenshot gives the real size.
pub const FALLBACK_POINTS: Size = Size::new(390.0, 844.0);
/// Screenshot pixels that go with [`FALLBACK_POINTS`].
pub const FALLBACK_PIXELS: Size = Size::new(1170.0, 2532.0);

/// Look up a model by `ProductType` (case-insensitive).
pub fn lookup(product_type: &str) -> Option<&'static IphoneModel> {
    let pt = product_type.trim();
    MODELS
        .iter()
        .find(|m| m.product_type.eq_ignore_ascii_case(pt))
}

/// Marketing name, or the raw `ProductType` when unknown.
pub fn marketing_name(product_type: &str) -> String {
    lookup(product_type)
        .map(|m| m.name.to_string())
        .unwrap_or_else(|| product_type.to_string())
}

/// Screen size in (points, pixels, known). For a `ProductType` that is not
/// in the table, `known` is false and the size is [`FALLBACK_POINTS`] /
/// [`FALLBACK_PIXELS`]. Replace it with [`sizes_from_screenshot`] when a
/// screenshot is available.
pub fn screen_sizes(product_type: &str) -> (Size, Size, bool) {
    match lookup(product_type) {
        Some(m) => (m.screen_points(), m.screen_pixels(), true),
        None => (FALLBACK_POINTS, FALLBACK_PIXELS, false),
    }
}

/// Screen size in (points, pixels) from the size of a screenshot, for a model
/// that is not in the table. The result is in portrait. The UI scale is @3x
/// when the short side is 1000 px or more, and @2x below that. Every iPhone
/// from the iPhone 6s on follows this rule (@2x models have a short side of
/// 640 to 828 px, @3x models 1125 px or more).
///
/// Returns `None` when the size is empty or too small for an iPhone screen.
pub fn sizes_from_screenshot(pixels: Size) -> Option<(Size, Size)> {
    if pixels.is_empty() {
        return None;
    }
    let short = pixels.width.min(pixels.height).round();
    let long = pixels.width.max(pixels.height).round();
    if short < 320.0 {
        return None;
    }
    let scale = if short >= 1000.0 { 3.0 } else { 2.0 };
    Some((
        Size::new((short / scale).round(), (long / scale).round()),
        Size::new(short, long),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iphone_13() {
        let m = lookup("iPhone14,5").unwrap();
        assert_eq!(m.name, "iPhone 13");
        assert_eq!(m.points, (390, 844));
        assert_eq!(m.pixels, (1170, 2532));
        assert!(m.can_run_supported_ios());
        assert_eq!(lookup("iphone14,5").unwrap().name, "iPhone 13");
        assert_eq!(lookup(" iPhone14,5 ").unwrap().name, "iPhone 13");
    }

    #[test]
    fn iphone_13_family() {
        let m = lookup("iPhone14,4").unwrap();
        assert_eq!(m.name, "iPhone 13 mini");
        assert_eq!(m.points, (375, 812));
        assert_eq!(m.pixels, (1125, 2436));
        for pt in ["iPhone14,2", "iPhone14,5"] {
            let (p, x, known) = screen_sizes(pt);
            assert!(known, "{pt}");
            assert_eq!(p, Size::new(390.0, 844.0), "{pt}");
            assert_eq!(x, Size::new(1170.0, 2532.0), "{pt}");
        }
        assert_eq!(lookup("iPhone14,3").unwrap().points, (428, 926));
    }

    /// Every iPhone that can run iOS 17.4 or later has an entry.
    #[test]
    fn every_ios_17_4_iphone_is_known() {
        let ids = [
            "iPhone11,2",
            "iPhone11,4",
            "iPhone11,6",
            "iPhone11,8",
            "iPhone12,1",
            "iPhone12,3",
            "iPhone12,5",
            "iPhone12,8",
            "iPhone13,1",
            "iPhone13,2",
            "iPhone13,3",
            "iPhone13,4",
            "iPhone14,2",
            "iPhone14,3",
            "iPhone14,4",
            "iPhone14,5",
            "iPhone14,6",
            "iPhone14,7",
            "iPhone14,8",
            "iPhone15,2",
            "iPhone15,3",
            "iPhone15,4",
            "iPhone15,5",
            "iPhone16,1",
            "iPhone16,2",
            "iPhone17,1",
            "iPhone17,2",
            "iPhone17,3",
            "iPhone17,4",
            "iPhone17,5",
            "iPhone18,1",
            "iPhone18,2",
            "iPhone18,3",
            "iPhone18,4",
            "iPhone18,5",
            "iPhone19,2",
            "iPhone19,3",
            "iPhone19,7",
        ];
        for id in ids {
            let m = lookup(id).unwrap_or_else(|| panic!("{id} missing"));
            assert!(m.can_run_supported_ios(), "{id}");
        }
        // XS, XS Max and XR stop at iOS 18. iPhone 11 and SE 2 run iOS 27.
        assert_eq!(lookup("iPhone11,8").unwrap().max_ios_major, 18);
        assert_eq!(lookup("iPhone12,8").unwrap().max_ios_major, CURRENT);
        assert_eq!(lookup("iPhone18,5").unwrap().name, "iPhone 17e");
        assert_eq!(lookup("iPhone18,4").unwrap().points, (420, 912));
    }

    #[test]
    fn unknown_model_falls_back() {
        let (p, x, known) = screen_sizes("iPhone99,1");
        assert!(!known);
        assert_eq!((p, x), (FALLBACK_POINTS, FALLBACK_PIXELS));
        assert_eq!(marketing_name("iPhone99,1"), "iPhone99,1");
        assert!(!screen_sizes("").2);
        // The first screenshot gives the real size, in portrait.
        let (p, x) = sizes_from_screenshot(Size::new(1206.0, 2622.0)).unwrap();
        assert_eq!((p, x), (Size::new(402.0, 874.0), Size::new(1206.0, 2622.0)));
        let (p, _) = sizes_from_screenshot(Size::new(2868.0, 1320.0)).unwrap();
        assert_eq!(p, Size::new(440.0, 956.0));
        // @2x screens.
        let (p, _) = sizes_from_screenshot(Size::new(828.0, 1792.0)).unwrap();
        assert_eq!(p, Size::new(414.0, 896.0));
        assert!(sizes_from_screenshot(Size::new(2.0, 2.0)).is_none());
        assert!(sizes_from_screenshot(Size::new(f32::NAN, 10.0)).is_none());
        assert!(sizes_from_screenshot(Size::default()).is_none());
    }

    /// The screenshot rule gives the table size for every model in the table.
    #[test]
    fn screenshot_rule_matches_table() {
        for m in MODELS {
            let (p, x) = sizes_from_screenshot(m.screen_pixels()).unwrap();
            assert_eq!(p, m.screen_points(), "{}", m.product_type);
            assert_eq!(x, m.screen_pixels(), "{}", m.product_type);
        }
    }

    #[test]
    fn table_is_consistent() {
        let mut seen = std::collections::HashSet::new();
        for m in MODELS {
            assert!(seen.insert(m.product_type), "duplicate {}", m.product_type);
            assert!(m.product_type.starts_with("iPhone"));
            // Screenshot pixels are points times the scale, exactly 2 or 3.
            let s = m.pixels.0 / m.points.0;
            assert!(s == 2 || s == 3, "{} scale", m.product_type);
            assert_eq!(m.pixels.0, m.points.0 * s, "{} width", m.product_type);
            assert_eq!(m.pixels.1, m.points.1 * s, "{} height", m.product_type);
        }
        assert!(MODELS.len() >= 51);
        assert!(!lookup("iPhone10,3").unwrap().can_run_supported_ios());
        assert!(lookup("iPhone11,8").unwrap().can_run_supported_ios());
    }
}
