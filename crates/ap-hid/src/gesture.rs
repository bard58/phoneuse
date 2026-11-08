//! The AssistiveTouch gesture model, ported from upstream `touch.py`, `pointer.py`
//! and `cli.py`. Gestures are planned as pure [`Step`] lists, then executed by
//! [`crate::HidDevice`]. Positions are screen fractions (0..1).
//!
//! What iOS does with the reports (measured upstream, iPhone 17 Pro, iOS 26):
//! - The absolute pointer lands exactly where asked. Only absolute reports carry
//!   button presses.
//! - While the button is held, an absolute move ends the touch after ~90 ms (it
//!   reads as a click). So a drag = absolute press, relative motion (button 0),
//!   absolute release at the same spot.
//! - Move, wait ~0.1 s, then press. Pressing in the same instant as a jump starts
//!   the touch on the way (an edge swipe turns into a content drag).
//! - Relative motion is accelerated by report size and dropped when reports come
//!   faster than ~8 ms apart: one report per [`TICK`], sized from the
//!   [`CalibrationProfile`].

use std::time::Duration;

use crate::calibration::{CalibrationProfile, TICK};
use crate::report::{Report, char_usage, mods, usage};
use ap_core::{Error, Result};

/// Pause after moving the pointer before pressing.
pub const SETTLE: Duration = Duration::from_millis(100);
/// Pause when the pointer position is unknown (first gesture, or stale).
pub const SETTLE_UNKNOWN: Duration = Duration::from_millis(200);
/// How long a tap holds the touch.
pub const TAP_HOLD: Duration = Duration::from_millis(60);
/// Gap between the two taps of a double tap.
pub const DOUBLE_TAP_GAP: Duration = Duration::from_millis(100);
/// Pause after the press before a swipe starts moving.
pub const SWIPE_START: Duration = Duration::from_millis(20);
/// Extra time a swipe may take to catch up with its target (upstream: 1 s).
pub const SWIPE_CATCH_UP: Duration = Duration::from_secs(1);
/// Key down/up pause (upstream `HID.key`).
pub const KEY_GAP: Duration = Duration::from_millis(12);
/// Eject press length.
pub const EJECT_HOLD: Duration = Duration::from_millis(50);

/// One step of a planned gesture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Step {
    /// Send one input report.
    Send(Report),
    /// Wait this long.
    Wait(Duration),
}

/// A planned gesture: steps plus where the pointer ends.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Plan {
    pub steps: Vec<Step>,
}

impl Plan {
    pub fn send(&mut self, r: Report) {
        self.steps.push(Step::Send(r));
    }

    pub fn wait(&mut self, d: Duration) {
        if !d.is_zero() {
            self.steps.push(Step::Wait(d));
        }
    }

    /// Only the reports.
    pub fn reports(&self) -> Vec<Report> {
        self.steps
            .iter()
            .filter_map(|s| match s {
                Step::Send(r) => Some(*r),
                Step::Wait(_) => None,
            })
            .collect()
    }

    /// Sum of all waits.
    pub fn total_wait(&self) -> Duration {
        self.steps
            .iter()
            .map(|s| match s {
                Step::Wait(d) => *d,
                Step::Send(_) => Duration::ZERO,
            })
            .sum()
    }

    pub fn extend(&mut self, other: Plan) {
        self.steps.extend(other.steps);
    }
}

/// One finger (upstream `touch.Touch`). Plans reports into a [`Plan`].
#[derive(Debug, Clone)]
pub struct Touch<'a> {
    profile: &'a CalibrationProfile,
    /// Where we believe the pointer is.
    pub pos: Option<(f64, f64)>,
    target: Option<(f64, f64)>,
    down: bool,
    pub plan: Plan,
}

impl<'a> Touch<'a> {
    /// `pos`: the last absolute position sent, if known and recent.
    pub fn new(profile: &'a CalibrationProfile, pos: Option<(f64, f64)>) -> Self {
        Self {
            profile,
            pos,
            target: None,
            down: false,
            plan: Plan::default(),
        }
    }

    pub fn is_down(&self) -> bool {
        self.down
    }

    /// Move the pointer without touching.
    pub fn hover(&mut self, fx: f64, fy: f64) {
        let (fx, fy) = (clamp(fx), clamp(fy));
        self.plan.send(Report::abs(fx, fy, false));
        self.pos = Some((fx, fy));
        self.target = self.pos;
    }

    /// Touch down at `(fx, fy)`. The pointer moves there first and gets
    /// [`SETTLE`] (or [`SETTLE_UNKNOWN`] when the start position is unknown).
    pub fn press(&mut self, fx: f64, fy: f64) {
        let (fx, fy) = (clamp(fx), clamp(fy));
        let far = match self.pos {
            None => true,
            Some((px, py)) => (px - fx).abs() > 0.002 || (py - fy).abs() > 0.002,
        };
        if far {
            let settle = if self.pos.is_some() {
                SETTLE
            } else {
                SETTLE_UNKNOWN
            };
            self.plan.send(Report::abs(fx, fy, false));
            self.plan.wait(settle);
        }
        self.plan.send(Report::abs(fx, fy, true));
        self.pos = Some((fx, fy));
        self.target = self.pos;
        self.down = true;
    }

    /// One relative report toward the target. Returns the remaining distance in
    /// points. Unlike upstream `Touch.step`, a tick with nothing to move still
    /// sends an empty report (upstream `pointer.py`, RECIPES.md): steady timing
    /// keeps iOS acceleration predictable and slow drags alive.
    fn step(&mut self) -> f32 {
        let (Some(pos), Some(target), true) = (self.pos, self.target, self.down) else {
            return 0.0;
        };
        let sp = self.profile.screen_points;
        let ex = ((target.0 - pos.0) * f64::from(sp.width)) as f32;
        let ey = ((target.1 - pos.1) * f64::from(sp.height)) as f32;
        let dist = (ex * ex + ey * ey).sqrt();
        let (ux, uy) = if dist < 1.0 {
            (0, 0)
        } else {
            self.profile.whole_units(ex, ey)
        };
        self.plan.send(Report::rel(ux, uy));
        if ux == 0 && uy == 0 {
            return dist;
        }
        let mag = ((ux * ux + uy * uy) as f32).sqrt();
        let moved = self.profile.points_for(mag);
        self.pos = Some((
            pos.0 + f64::from(moved * ux as f32 / mag) / f64::from(sp.width),
            pos.1 + f64::from(moved * uy as f32 / mag) / f64::from(sp.height),
        ));
        (dist - moved).max(0.0)
    }

    /// Lift at the target (no position change between press and release).
    pub fn release(&mut self) {
        if !self.down {
            return;
        }
        let (fx, fy) = self.target.or(self.pos).unwrap_or((0.0, 0.0));
        self.plan.send(Report::abs(fx, fy, false));
        self.down = false;
        self.pos = Some((fx, fy));
    }

    /// Tap: press, hold, release.
    pub fn tap(&mut self, fx: f64, fy: f64, hold: Duration) {
        self.press(fx, fy);
        self.plan.wait(hold);
        self.release();
    }

    /// Finger down at `from`, move to `to` over `duration`, wait `hold_end`, lift.
    /// The path is planned per tick (k = i / n), not from the wall clock, so the
    /// plan is deterministic; if a report round trip is slower than [`TICK`] the
    /// swipe takes longer than `duration`.
    pub fn swipe(
        &mut self,
        from: (f64, f64),
        to: (f64, f64),
        duration: Duration,
        hold_end: Duration,
    ) {
        let (from, to) = ((clamp(from.0), clamp(from.1)), (clamp(to.0), clamp(to.1)));
        self.press(from.0, from.1);
        self.plan.wait(SWIPE_START);
        let n = ticks(duration).max(1);
        let limit = n + ticks(SWIPE_CATCH_UP);
        for i in 1..=limit {
            let k = (f64::from(i) / f64::from(n)).min(1.0);
            self.target = Some((from.0 + (to.0 - from.0) * k, from.1 + (to.1 - from.1) * k));
            let left = self.step();
            // Upstream stops at < 1 pt left, which can idle for the whole catch-up
            // second when 1..3.6 pt remain (no report is small enough). Stop as
            // soon as no whole report fits; the release lands on the target.
            if k >= 1.0 && left < self.profile.min_points().max(1.0) {
                break;
            }
            self.plan.wait(TICK);
        }
        self.plan.wait(hold_end);
        self.release();
    }

    /// Calibration stroke: press, `count` relative reports of `(units, 0)` at
    /// [`TICK`] pacing, release where the press was.
    pub fn calibration_stroke(&mut self, at: (f64, f64), units: u8, count: u32) {
        self.press(at.0, at.1);
        self.plan.wait(SWIPE_START);
        for _ in 0..count {
            self.plan.send(Report::rel(i32::from(units), 0));
            self.plan.wait(TICK);
        }
        self.release();
    }
}

fn ticks(d: Duration) -> u32 {
    (d.as_secs_f64() / TICK.as_secs_f64()).ceil() as u32
}

fn clamp(v: f64) -> f64 {
    if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) }
}

// ---------- keyboard ----------

/// Press and release one key (upstream `HID.key`).
pub fn key_press(plan: &mut Plan, code: u8, modifiers: u8) {
    plan.send(Report::key(code, modifiers));
    plan.wait(KEY_GAP);
    plan.send(Report::KEYS_UP);
    plan.wait(KEY_GAP);
}

/// Type US-layout text; `\n` presses Enter.
pub fn type_text(plan: &mut Plan, text: &str) -> Result<()> {
    for c in text.chars() {
        let (code, m) = char_usage(c).ok_or_else(|| {
            Error::InvalidArgument(format!(
                "cannot type {c:?}: the HID keyboard is US layout, ASCII only"
            ))
        })?;
        key_press(plan, code, m);
    }
    Ok(())
}

/// Eject: shows or hides the on-screen keyboard while a hardware keyboard is connected.
pub fn eject(plan: &mut Plan) {
    plan.send(Report::Consumer { eject: true });
    plan.wait(EJECT_HOLD);
    plan.send(Report::Consumer { eject: false });
}

// ---------- system actions (upstream cli.py recipes, verified on iOS 26) ----------

/// Pauses between the steps of "open app". Upstream waits for the screen to
/// settle instead; the dongle has no screen, so these are fixed.
pub const OPEN_APP_HOME_WAIT: Duration = Duration::from_millis(600);
pub const OPEN_APP_SEARCH_WAIT: Duration = Duration::from_millis(700);
pub const OPEN_APP_TYPE_WAIT: Duration = Duration::from_millis(900);

/// Home: Cmd+H.
pub fn home(plan: &mut Plan) {
    key_press(plan, usage::H, mods::GUI);
}

/// Open an app by its name: Cmd+H, Cmd+Space, type the name, Enter.
pub fn open_app(plan: &mut Plan, name: &str) -> Result<()> {
    home(plan);
    plan.wait(OPEN_APP_HOME_WAIT);
    key_press(plan, usage::SPACE, mods::GUI);
    plan.wait(OPEN_APP_SEARCH_WAIT);
    type_text(plan, name)?;
    plan.wait(OPEN_APP_TYPE_WAIT);
    key_press(plan, usage::ENTER, 0);
    Ok(())
}

/// Named system gestures (upstream `navigate`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemGesture {
    /// Edge swipe from x = 0.
    Back,
    AppSwitcher,
    ControlCenter,
    Notifications,
    /// Close Control Center, Notification Center, the switcher, or go home from an app.
    Dismiss,
}

impl SystemGesture {
    /// `(from, to, duration, hold_end)` as in upstream cli.py.
    pub fn path(self) -> ((f64, f64), (f64, f64), Duration, Duration) {
        let ms = Duration::from_millis;
        match self {
            Self::Back => ((0.0, 0.5), (0.8, 0.5), ms(350), Duration::ZERO),
            Self::AppSwitcher => ((0.5, 1.0), (0.5, 0.62), ms(450), ms(600)),
            Self::ControlCenter => ((0.88, 0.0), (0.88, 0.45), ms(600), Duration::ZERO),
            Self::Notifications => ((0.15, 0.0), (0.15, 0.7), ms(600), Duration::ZERO),
            Self::Dismiss => ((0.5, 1.0), (0.5, 0.15), ms(120), Duration::ZERO),
        }
    }
}

/// Well-known Apple bundle ids and the name Spotlight finds them by.
const BUNDLE_NAMES: &[(&str, &str)] = &[
    ("com.apple.mobilesafari", "Safari"),
    ("com.apple.Preferences", "Settings"),
    ("com.apple.MobileSMS", "Messages"),
    ("com.apple.mobilemail", "Mail"),
    ("com.apple.mobileslideshow", "Photos"),
    ("com.apple.camera", "Camera"),
    ("com.apple.Maps", "Maps"),
    ("com.apple.mobilenotes", "Notes"),
    ("com.apple.mobilecal", "Calendar"),
    ("com.apple.mobiletimer", "Clock"),
    ("com.apple.AppStore", "App Store"),
    ("com.apple.mobilephone", "Phone"),
    ("com.apple.Music", "Music"),
    ("com.apple.weather", "Weather"),
    ("com.apple.reminders", "Reminders"),
    ("com.apple.DocumentsApp", "Files"),
    ("com.apple.MobileAddressBook", "Contacts"),
    ("com.apple.calculator", "Calculator"),
    ("com.apple.Health", "Health"),
    ("com.apple.Passbook", "Wallet"),
    ("com.apple.shortcuts", "Shortcuts"),
    ("com.apple.facetime", "FaceTime"),
];

/// The Spotlight search text for a bundle id or app name. A HID keyboard can
/// only search by name: known Apple bundle ids map to their names, other
/// bundle ids use their last segment, and names pass through.
pub fn app_search_name(bundle_or_name: &str) -> String {
    let s = bundle_or_name.trim();
    if let Some((_, n)) = BUNDLE_NAMES.iter().find(|(b, _)| b.eq_ignore_ascii_case(s)) {
        return (*n).to_string();
    }
    let looks_like_bundle = s.contains('.') && !s.contains(' ') && s.split('.').count() >= 3;
    if looks_like_bundle {
        return s.rsplit('.').next().unwrap_or(s).to_string();
    }
    s.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p() -> CalibrationProfile {
        CalibrationProfile::iphone_17_pro()
    }

    #[test]
    fn tap_from_unknown_position() {
        let prof = p();
        let mut t = Touch::new(&prof, None);
        t.tap(0.5, 0.25, TAP_HOLD);
        assert_eq!(
            t.plan.steps,
            vec![
                Step::Send(Report::abs(0.5, 0.25, false)),
                Step::Wait(SETTLE_UNKNOWN),
                Step::Send(Report::abs(0.5, 0.25, true)),
                Step::Wait(TAP_HOLD),
                Step::Send(Report::abs(0.5, 0.25, false)),
            ]
        );
        assert_eq!(t.pos, Some((0.5, 0.25)));
    }

    #[test]
    fn tap_at_known_positions() {
        let prof = p();
        let mut t = Touch::new(&prof, Some((0.1, 0.1)));
        t.press(0.5, 0.5);
        assert_eq!(t.plan.steps[1], Step::Wait(SETTLE));
        let mut t = Touch::new(&prof, Some((0.5, 0.5)));
        t.press(0.501, 0.5);
        assert_eq!(
            t.plan.steps,
            vec![Step::Send(Report::abs(0.501, 0.5, true))]
        );
    }

    #[test]
    fn swipe_is_abs_press_rel_moves_abs_release() {
        let prof = p();
        let mut t = Touch::new(&prof, Some((0.5, 0.7)));
        t.swipe(
            (0.5, 0.7),
            (0.5, 0.3),
            Duration::from_millis(350),
            Duration::ZERO,
        );
        let r = t.plan.reports();
        // The pointer is already there: no hover, no settle.
        assert_eq!(r[0], Report::abs(0.5, 0.7, true));
        assert_eq!(*r.last().unwrap(), Report::abs(0.5, 0.3, false));
        let middle = &r[1..r.len() - 1];
        assert!(!middle.is_empty());
        let mut total_dy = 0.0;
        for m in middle {
            match *m {
                Report::Mouse {
                    buttons,
                    dx,
                    dy,
                    wheel,
                } => {
                    assert_eq!((buttons, dx, wheel), (0, 0, 0));
                    assert!(dy <= 0, "never moves back");
                    total_dy += prof.points_for(f32::from(dy.unsigned_abs()));
                }
                other => panic!("absolute report while held: {other:?}"),
            }
        }
        // 0.4 * 874 pt = 349.6 pt requested; lands within 1 pt short (round down).
        assert!((348.0..=349.6).contains(&total_dy), "moved {total_dy}");
        // One report per tick: every relative report is followed by an 8 ms wait,
        // except the last one before the release.
        let waits = t
            .plan
            .steps
            .iter()
            .filter(|s| **s == Step::Wait(TICK))
            .count();
        assert_eq!(waits, middle.len() - 1);
    }

    #[test]
    fn edge_swipe_settles_before_press() {
        let prof = p();
        let mut t = Touch::new(&prof, Some((0.5, 0.5)));
        let (f, to, d, h) = SystemGesture::Back.path();
        t.swipe(f, to, d, h);
        assert_eq!(t.plan.steps[0], Step::Send(Report::abs(0.0, 0.5, false)));
        assert_eq!(t.plan.steps[1], Step::Wait(SETTLE));
        assert_eq!(t.plan.steps[2], Step::Send(Report::abs(0.0, 0.5, true)));
    }

    #[test]
    fn keyboard_plans() {
        let mut plan = Plan::default();
        home(&mut plan);
        assert_eq!(
            plan.steps,
            vec![
                Step::Send(Report::key(usage::H, mods::GUI)),
                Step::Wait(KEY_GAP),
                Step::Send(Report::KEYS_UP),
                Step::Wait(KEY_GAP),
            ]
        );
        let mut plan = Plan::default();
        open_app(&mut plan, "Settings").unwrap();
        let r = plan.reports();
        assert_eq!(r[0], Report::key(usage::H, mods::GUI));
        assert_eq!(r[2], Report::key(usage::SPACE, mods::GUI));
        assert_eq!(r[4], Report::key(22, mods::SHIFT)); // 'S'
        assert_eq!(r[r.len() - 2], Report::key(usage::ENTER, 0));
        assert_eq!(r.len(), 2 * (2 + 8 + 1));
        assert!(type_text(&mut Plan::default(), "ğ").is_err());
        let mut plan = Plan::default();
        eject(&mut plan);
        assert_eq!(
            plan.reports(),
            vec![
                Report::Consumer { eject: true },
                Report::Consumer { eject: false }
            ]
        );
    }

    #[test]
    fn app_names() {
        assert_eq!(app_search_name("com.apple.Preferences"), "Settings");
        assert_eq!(app_search_name("com.example.SuperApp"), "SuperApp");
        assert_eq!(app_search_name("App Store"), "App Store");
        assert_eq!(app_search_name("Settings"), "Settings");
    }
}
