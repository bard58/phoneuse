//! Per-device-model calibration of relative pointer motion (upstream `touch.DRAG_PTS`).
//!
//! iOS accelerates relative pointer motion by report size, and it drops motion
//! from reports that arrive less than about 8 ms apart. A drag therefore sends
//! one relative report per [`TICK`], and sizes each report from a measured table:
//! "a report of N units moves the touch P points".
//!
//! # Calibrate a new model
//!
//! [`CALIBRATION_PROCEDURE`] describes the routine. In short: open a touch test
//! page (upstream `legacy/python/touchlab/index.html`) in Safari on the phone,
//! run [`crate::HidDevice::calibration_stroke`] once per unit size, read the
//! `net dx` the page logs, and give the results to [`CalibrationProfile::fit`].

use std::time::Duration;

use ap_core::Size;
use serde::{Deserialize, Serialize};

/// One relative report per tick during a drag.
pub const TICK: Duration = Duration::from_millis(8);

/// Unit sizes the calibration routine measures (same as the upstream table).
pub const CALIBRATION_UNITS: [u8; 12] = [1, 2, 3, 4, 5, 6, 8, 10, 12, 16, 20, 28];

/// How to measure a [`CalibrationProfile`] for a new iPhone model.
pub const CALIBRATION_PROCEDURE: &str = "\
Calibration (one time per iPhone model; about 5 minutes):
1. On the iPhone: AssistiveTouch on, Dwell Control off, Auto-Lock off.
2. Serve legacy/python/touchlab/index.html (any static web server on the LAN) and open it in Safari.
   Optional: add it to the Home Screen so no browser bars move the page.
3. For each unit size U in CALIBRATION_UNITS, run HidDevice::calibration_stroke(U, 20):
   it presses at the left middle of the screen, sends 20 relative reports of (U, 0)
   at 8 ms pacing, and releases at the same place.
4. Read the last log line on the page: `net DX,DY`. DX is in CSS px = iOS points.
5. Record (U, DX, 20) for each stroke and call CalibrationProfile::fit(name, models, screen_points, &results).
6. Check: a 175 pt swipe must land within 3 pt (touchlab `net`). Save the profile in the config.
Repeat a stroke 3 times and use the median if the values differ by more than 5 %.";

/// Measured drag table for one or more iPhone models.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CalibrationProfile {
    /// Human name ("iPhone 17 Pro").
    pub name: String,
    /// Hardware model ids this profile is for ("iPhone18,1").
    pub models: Vec<String>,
    /// Logical screen size in points.
    pub screen_points: Size,
    /// `(units, points)` pairs: one relative report of `units` moves `points`,
    /// at [`TICK`] pacing. Sorted by units, points strictly increasing.
    pub drag_pts: Vec<(f32, f32)>,
    /// True when `drag_pts` was measured on this model.
    pub measured: bool,
    /// Where the values come from.
    pub source: String,
}

impl CalibrationProfile {
    /// iPhone 17 Pro, iOS 26, measured upstream 2026-11-06 (`touch.DRAG_PTS`).
    pub fn iphone_17_pro() -> Self {
        Self {
            name: "iPhone 17 Pro".into(),
            models: vec!["iPhone18,1".into()],
            screen_points: Size::new(402.0, 874.0),
            drag_pts: UPSTREAM_DRAG_PTS.to_vec(),
            measured: true,
            source: "upstream phoneuse touch.DRAG_PTS, iPhone 17 Pro, iOS 26, 2026-11-06".into(),
        }
    }

    /// iPhone 13: placeholder. The screen size is correct; the drag table is the
    /// iPhone 17 Pro table and is NOT measured. Run the calibration routine.
    pub fn iphone_13_placeholder() -> Self {
        Self {
            name: "iPhone 13 (placeholder)".into(),
            models: vec!["iPhone14,5".into()],
            screen_points: Size::new(390.0, 844.0),
            drag_pts: UPSTREAM_DRAG_PTS.to_vec(),
            measured: false,
            source: "copy of the iPhone 17 Pro table; not measured, see CALIBRATION_PROCEDURE"
                .into(),
        }
    }

    /// Built-in profiles.
    pub fn builtin() -> Vec<Self> {
        vec![Self::iphone_17_pro(), Self::iphone_13_placeholder()]
    }

    /// Profile for a hardware model id or name ("iPhone14,5", "iphone 13").
    /// Unknown models get the iPhone 17 Pro table with `measured = false`.
    pub fn for_model(model: &str) -> Self {
        let want = norm(model);
        for p in Self::builtin() {
            if p.models.iter().any(|m| norm(m) == want) || norm(&p.name).starts_with(&want) {
                return p;
            }
        }
        let mut p = Self::iphone_17_pro();
        p.name = format!("{model} (uncalibrated)");
        p.models = vec![model.to_string()];
        p.measured = false;
        p.source = "iPhone 17 Pro table; not measured for this model".into();
        p
    }

    /// Build a profile from calibration strokes `(units, net_points, reports)`.
    pub fn fit(
        name: impl Into<String>,
        models: Vec<String>,
        screen_points: Size,
        strokes: &[(u8, f32, u32)],
    ) -> Result<Self, String> {
        let mut pts: Vec<(f32, f32)> = strokes
            .iter()
            .filter(|(_, _, n)| *n > 0)
            .map(|&(u, net, n)| (f32::from(u), net.abs() / n as f32))
            .collect();
        pts.sort_by(|a, b| a.0.total_cmp(&b.0));
        pts.dedup_by(|a, b| a.0 == b.0);
        if pts.len() < 2 {
            return Err("need at least 2 strokes with different unit sizes".into());
        }
        if pts.windows(2).any(|w| w[1].1 <= w[0].1) {
            return Err(format!(
                "points per report must grow with the unit size; got {pts:?}. Repeat the strokes"
            ));
        }
        Ok(Self {
            name: name.into(),
            models,
            screen_points,
            drag_pts: pts,
            measured: true,
            source: "calibration routine".into(),
        })
    }

    /// Smallest motion (points) one report can make.
    pub fn min_points(&self) -> f32 {
        self.drag_pts.first().map_or(1.0, |p| p.1)
    }

    /// Report size (units, may be fractional) that moves `points` in one report.
    /// Capped at the largest measured size; the rest goes in later ticks.
    pub fn units_for(&self, points: f32) -> f32 {
        let t = &self.drag_pts;
        if points <= 0.0 || t.is_empty() {
            return 0.0;
        }
        let i = t.partition_point(|p| p.1 < points);
        if i == 0 {
            return points / t[0].1 * t[0].0;
        }
        if i >= t.len() {
            return t[t.len() - 1].0;
        }
        let ((u0, p0), (u1, p1)) = (t[i - 1], t[i]);
        u0 + (u1 - u0) * (points - p0) / (p1 - p0)
    }

    /// Points moved by a report of `units` (inverse of [`Self::units_for`]).
    pub fn points_for(&self, units: f32) -> f32 {
        let t = &self.drag_pts;
        if units <= 0.0 || t.is_empty() {
            return 0.0;
        }
        let i = t.partition_point(|p| p.0 < units);
        if i == 0 {
            return units * t[0].1 / t[0].0;
        }
        if i >= t.len() {
            let (u, p) = t[t.len() - 1];
            return p * units / u;
        }
        let ((u0, p0), (u1, p1)) = (t[i - 1], t[i]);
        p0 + (p1 - p0) * (units - u0) / (u1 - u0)
    }

    /// Whole report `(ux, uy)` that moves at most `(px, py)` points. Sizes round
    /// down, so the touch never gets ahead of the requested position (overshoot
    /// and return shows as a vibrating pointer in slow drags).
    pub fn whole_units(&self, px: f32, py: f32) -> (i32, i32) {
        let dist = (px * px + py * py).sqrt();
        if dist < self.min_points() {
            return (0, 0);
        }
        let u = self.units_for(dist);
        let mut ux = (u * px / dist) as i32; // truncate toward zero
        let mut uy = (u * py / dist) as i32;
        if ux == 0 && uy == 0 {
            ux = if px.abs() >= py.abs() {
                if px > 0.0 { 1 } else { -1 }
            } else {
                0
            };
            uy = if py.abs() > px.abs() {
                if py > 0.0 { 1 } else { -1 }
            } else {
                0
            };
        }
        (ux, uy)
    }
}

impl Default for CalibrationProfile {
    fn default() -> Self {
        Self::iphone_17_pro()
    }
}

/// Upstream `DRAG_PTS` (iPhone 17 Pro, iOS 26).
const UPSTREAM_DRAG_PTS: [(f32, f32); 12] = [
    (1.0, 3.6),
    (2.0, 6.9),
    (3.0, 12.1),
    (4.0, 17.6),
    (5.0, 25.6),
    (6.0, 33.3),
    (8.0, 46.6),
    (10.0, 62.1),
    (12.0, 76.3),
    (16.0, 104.1),
    (20.0, 141.8),
    (28.0, 199.7),
];

fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == ',')
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn table_lookup_matches_upstream() {
        let p = CalibrationProfile::iphone_17_pro();
        assert!(close(p.units_for(3.6), 1.0));
        assert!(close(p.units_for(1.8), 0.5));
        assert!(close(p.units_for(9.5), 2.5)); // halfway 6.9..12.1
        assert!(close(p.units_for(500.0), 28.0));
        assert!(close(p.points_for(2.5), 9.5));
        assert!(close(p.points_for(56.0), 399.4));
        assert_eq!(p.whole_units(2.0, 0.0), (0, 0));
        assert_eq!(p.whole_units(4.0, 0.0), (1, 0));
        assert_eq!(p.whole_units(0.0, -4.0), (0, -1));
        // 100 pt to the right: units_for(100) = 12 + 4*(23.7/27.8) = 15.41 -> 15
        assert_eq!(p.whole_units(100.0, 0.0), (15, 0));
        assert_eq!(p.whole_units(1000.0, 0.0), (28, 0));
    }

    #[test]
    fn profiles() {
        assert!(CalibrationProfile::for_model("iPhone18,1").measured);
        let p13 = CalibrationProfile::for_model("iphone14,5");
        assert_eq!(p13.screen_points, Size::new(390.0, 844.0));
        assert!(!p13.measured);
        assert_eq!(
            CalibrationProfile::for_model("iPhone 13").models[0],
            "iPhone14,5"
        );
        assert!(!CalibrationProfile::for_model("iPhone99,9").measured);
        let fit = CalibrationProfile::fit(
            "x",
            vec![],
            Size::new(390.0, 844.0),
            &[(2, 140.0, 20), (1, 70.0, 20)],
        )
        .unwrap();
        assert_eq!(fit.drag_pts, vec![(1.0, 3.5), (2.0, 7.0)]);
        assert!(
            CalibrationProfile::fit("x", vec![], Size::default(), &[(1, 9.0, 1), (2, 8.0, 1)])
                .is_err()
        );
    }
}
