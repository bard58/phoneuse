//! iPhone model table: `ProductType` -> marketing name, screen size, newest iOS.
//!
//! Lockdown has no documented screen-size key, so `screen_points` comes from
//! this table. `pixels` is the size of a screenshot (the native panel for the
//! mini models, the rendered size for the 6s/7/8 Plus models).
//!
//! Source: Apple tech specs. The iPhone 17 family identifiers (`iPhone18,x`)
//! are from public device databases and are not verified on a device.

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
    m!("iPhone13,1", "iPhone 12 mini", 375 x 812, 1080 x 2340, CURRENT),
    m!("iPhone13,2", "iPhone 12", 390 x 844, 1170 x 2532, CURRENT),
    m!("iPhone13,3", "iPhone 12 Pro", 390 x 844, 1170 x 2532, CURRENT),
    m!("iPhone13,4", "iPhone 12 Pro Max", 428 x 926, 1284 x 2778, CURRENT),
    m!("iPhone14,4", "iPhone 13 mini", 375 x 812, 1080 x 2340, CURRENT),
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
];

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

/// Screen size in (points, pixels). Unknown models fall back to the
/// iPhone 13 size; callers that need exact pixels should use a screenshot.
pub fn screen_sizes(product_type: &str) -> (Size, Size, bool) {
    match lookup(product_type) {
        Some(m) => (m.screen_points(), m.screen_pixels(), true),
        None => (Size::new(390.0, 844.0), Size::new(1170.0, 2532.0), false),
    }
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
        assert_eq!(marketing_name("iPhone99,9"), "iPhone99,9");
        assert!(!screen_sizes("iPhone99,9").2);
    }

    #[test]
    fn table_is_consistent() {
        let mut seen = std::collections::HashSet::new();
        for m in MODELS {
            assert!(seen.insert(m.product_type), "duplicate {}", m.product_type);
            assert!(m.product_type.starts_with("iPhone"));
            // Pixels are points times the scale (2 or 3), or slightly less
            // for panels that downsample (mini, Plus).
            let sx = m.pixels.0 as f32 / m.points.0 as f32;
            let sy = m.pixels.1 as f32 / m.points.1 as f32;
            assert!(
                (1.9..=3.01).contains(&sx),
                "{} x scale {sx}",
                m.product_type
            );
            assert!((sx - sy).abs() < 0.05, "{} aspect", m.product_type);
        }
        assert!(MODELS.len() >= 46);
        assert!(!lookup("iPhone10,3").unwrap().can_run_supported_ios());
        assert!(lookup("iPhone11,8").unwrap().can_run_supported_ios());
    }
}
