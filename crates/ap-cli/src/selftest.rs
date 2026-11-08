//! `agentphone selftest`: a guided first-run check on a real phone.
//!
//! The steps run in this order: the setup check (doctor), the phone
//! connection, one screenshot, then a harmless input test. The input test
//! runs only after the owner answers yes (or gives `--yes`). It presses Home,
//! does one swipe on the home screen, and presses Home again. It never types
//! text and never taps. The swipe is sent only when there is proof that the
//! home screen shows (see [`input_steps`]). On iOS below 27 the input test is
//! skipped, because touch input needs iOS 27.
//!
//! Each step gives PASS, FAIL or SKIP with one reason and one fix. At the end
//! the command writes `selftest-report.json` and `selftest-report.txt`. The
//! report holds no secrets and no phone names. It shows only the last 4
//! characters of a device id. [`redact`] removes the ids and names of every
//! phone that discovery found.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use ap_core::{Device, DeviceInfo, Frame, Platform, Point};
use ap_screen::{SettleConfig, Settler, Thumb};
use console::style;
use serde::Serialize;

use crate::doctor::{self, Check, Status};
use crate::{config_cmd, devices, ui};

/// Every step, in order: (id, name).
pub const STEPS: [(&str, &str); 6] = [
    ("doctor", "Setup check"),
    ("connect", "Phone connection"),
    ("screenshot", "Screenshot"),
    ("home", "Press Home"),
    ("swipe", "Swipe on the home screen"),
    ("home_again", "Press Home again"),
];

/// The input test steps.
const INPUT_STEPS: [&str; 3] = ["home", "swipe", "home_again"];

/// Touch input on an iPhone needs this iOS major version or later. Below it,
/// the iPhone refuses the display stream, and `Input::open` fails.
pub const IOS_INPUT_MAJOR: u32 = 27;

/// The text that replaces a phone name in the report.
pub const NAME_MASK: &str = "[phone name]";

/// The swipe of the input test: a finger from left to right in the middle of
/// the screen. On the home screen it opens the previous page or the widgets.
/// The other direction is not used: on the lock screen it opens the camera.
pub const SWIPE_FROM: Point = Point { x: 0.25, y: 0.5 };
pub const SWIPE_TO: Point = Point { x: 0.65, y: 0.5 };
pub const SWIPE_MS: u64 = 300;

/// What the input test does. The prompt shows this text before the question.
pub const INPUT_EXPLAIN: &str = "\
Input test: agentphone presses Home, swipes one time from left to right on the
home screen, and presses Home again. It does not type text and does not tap.
Unlock the phone and go to the home screen before you start.";

pub const REPORT_JSON: &str = "selftest-report.json";
pub const REPORT_TXT: &str = "selftest-report.txt";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Outcome {
    Pass,
    Fail,
    Skip,
}

impl Outcome {
    fn label(self) -> &'static str {
        match self {
            Outcome::Pass => "PASS",
            Outcome::Fail => "FAIL",
            Outcome::Skip => "SKIP",
        }
    }
}

/// The result of one step.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Step {
    pub id: &'static str,
    pub name: &'static str,
    pub result: Outcome,
    /// One line: why this result.
    pub reason: String,
    /// One line: what to do. Shown for FAIL and SKIP.
    pub fix: Option<String>,
    /// The error text, when a call failed.
    pub error: Option<String>,
}

impl Step {
    fn new(id: &'static str, result: Outcome, reason: impl Into<String>) -> Self {
        let name = STEPS.iter().find(|(i, _)| *i == id).map_or(id, |(_, n)| *n);
        Self {
            id,
            name,
            result,
            reason: reason.into(),
            fix: None,
            error: None,
        }
    }

    pub fn pass(id: &'static str, reason: impl Into<String>) -> Self {
        Self::new(id, Outcome::Pass, reason)
    }

    pub fn fail(id: &'static str, reason: impl Into<String>) -> Self {
        Self::new(id, Outcome::Fail, reason)
    }

    pub fn skip(id: &'static str, reason: impl Into<String>) -> Self {
        Self::new(id, Outcome::Skip, reason)
    }

    pub fn with_fix(mut self, fix: impl Into<String>) -> Self {
        self.fix = Some(fix.into());
        self
    }

    pub fn with_error(mut self, e: &anyhow::Error) -> Self {
        let (problem, hint) = ui::explain(e);
        self.error = Some(problem);
        if self.fix.is_none() {
            self.fix = hint;
        }
        self
    }
}

/// The phone, without personal data: no name, only the end of the id.
/// [`Report::finish`] also removes the names from the step texts.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PhoneSummary {
    pub platform: String,
    pub os_version: String,
    pub model: String,
    pub backend: String,
    /// The last 4 characters of the UDID or serial, for example `****801E`.
    pub id: String,
    pub screen: String,
}

impl PhoneSummary {
    pub fn new(info: &DeviceInfo) -> Self {
        let unit = if info.platform == Platform::Ios {
            "pt"
        } else {
            "dp"
        };
        Self {
            platform: info.platform.to_string(),
            os_version: info.os_version.clone(),
            model: info.model.clone(),
            backend: info.backend.clone(),
            id: mask_id(&info.id),
            screen: format!(
                "{} x {} {unit} ({} x {} px)",
                info.screen_points.width,
                info.screen_points.height,
                info.screen_pixels.width,
                info.screen_pixels.height
            ),
        }
    }
}

/// The report for the developer.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Report {
    pub tool: &'static str,
    pub version: &'static str,
    /// The operating system of the computer, for example `windows x86_64`.
    pub computer: String,
    /// Seconds since 1970-01-01 UTC.
    pub created_unix: u64,
    pub phone: Option<PhoneSummary>,
    /// The owner stopped the test with Ctrl+C.
    pub stopped_by_user: bool,
    pub steps: Vec<Step>,
    /// Saved screenshots, relative to the report folder.
    pub screenshots: Vec<String>,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
}

impl Default for Report {
    fn default() -> Self {
        Self::new()
    }
}

impl Report {
    pub fn new() -> Self {
        Self {
            tool: "agentphone",
            version: env!("CARGO_PKG_VERSION"),
            computer: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
            created_unix: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
            phone: None,
            stopped_by_user: false,
            steps: Vec::new(),
            screenshots: Vec::new(),
            passed: 0,
            failed: 0,
            skipped: 0,
        }
    }

    fn count(&mut self) {
        let n = |o| self.steps.iter().filter(|s| s.result == o).count();
        (self.passed, self.failed, self.skipped) =
            (n(Outcome::Pass), n(Outcome::Fail), n(Outcome::Skip));
    }

    /// True when no step failed and the owner did not stop the test.
    pub fn ok(&self) -> bool {
        !self.stopped_by_user && !self.steps.iter().any(|s| s.result == Outcome::Fail)
    }

    /// Remove personal data from every text: known device ids, known phone
    /// names, UDID-like tokens, and the home folder path. Then update the
    /// counts.
    pub fn finish(&mut self, known_ids: &[&str], names: &[&str], home: Option<&str>) {
        let clean = |t: &str| redact(t, known_ids, names, home);
        for s in &mut self.steps {
            s.reason = clean(&s.reason);
            s.fix = s.fix.as_deref().map(clean);
            s.error = s.error.as_deref().map(clean);
        }
        self.count();
    }

    /// The plain text report.
    pub fn text(&self) -> String {
        let mut out = String::from("agentphone selftest report\n");
        out.push_str(&format!("version:  {}\n", self.version));
        out.push_str(&format!("computer: {}\n", self.computer));
        match &self.phone {
            Some(p) => {
                out.push_str(&format!("phone:    {} {}\n", p.platform, p.os_version));
                out.push_str(&format!("model:    {}\n", p.model));
                out.push_str(&format!("screen:   {}\n", p.screen));
                out.push_str(&format!("backend:  {}\n", p.backend));
                out.push_str(&format!("id:       {}\n", p.id));
            }
            None => out.push_str("phone:    not connected\n"),
        }
        out.push('\n');
        for s in &self.steps {
            out.push_str(&step_text(s, false));
        }
        out.push('\n');
        out.push_str(&self.summary());
        out.push('\n');
        out
    }

    /// One line: the counts and the result.
    pub fn summary(&self) -> String {
        let mut s = format!(
            "{} passed, {} failed, {} skipped.",
            self.passed, self.failed, self.skipped
        );
        if self.stopped_by_user {
            s.push_str(" Stopped with Ctrl+C.");
        }
        s
    }
}

/// `****` and the last 4 characters of an id.
pub fn mask_id(id: &str) -> String {
    let chars: Vec<char> = id.chars().collect();
    if chars.len() <= 4 {
        return "****".into();
    }
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("****{tail}")
}

/// A token that looks like a UDID or a long hardware serial: 12 or more
/// characters, only hex digits and dashes, and 8 or more digits.
fn looks_like_udid(t: &str) -> bool {
    t.len() >= 12
        && t.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
        && t.chars().filter(|c| c.is_ascii_digit()).count() >= 8
}

/// A phone name that is personal. The default names ("iPhone") and very
/// short names are not, and a replacement would change normal words.
fn is_personal_name(name: &str) -> bool {
    let n = name.trim();
    n.chars().count() >= 3
        && !["iphone", "ipad", "android", "phone"]
            .iter()
            .any(|g| n.eq_ignore_ascii_case(g))
}

/// Remove personal data from a text. See [`Report::finish`].
pub fn redact(text: &str, known_ids: &[&str], names: &[&str], home: Option<&str>) -> String {
    let mut t = text.to_string();
    if let Some(h) = home.filter(|h| h.len() > 3) {
        t = t.replace(h, "~");
    }
    // The longest name first: "Ozan's iPhone 13" before "Ozan's iPhone".
    let mut names: Vec<&str> = names
        .iter()
        .map(|n| n.trim())
        .filter(|n| is_personal_name(n))
        .collect();
    names.sort_by_key(|n| std::cmp::Reverse(n.len()));
    for n in names {
        t = t.replace(n, NAME_MASK);
    }
    for id in known_ids.iter().filter(|i| i.len() > 4) {
        t = t.replace(id, &mask_id(id));
    }
    let mut out = String::with_capacity(t.len());
    let mut token = String::new();
    let flush = |token: &mut String, out: &mut String| {
        if looks_like_udid(token) {
            out.push_str(&mask_id(token));
        } else {
            out.push_str(token);
        }
        token.clear();
    };
    for c in t.chars() {
        if c.is_ascii_alphanumeric() || c == '-' {
            token.push(c);
        } else {
            flush(&mut token, &mut out);
            out.push(c);
        }
    }
    flush(&mut token, &mut out);
    out
}

/// The home folder path of the user, for [`redact`].
fn home_dir() -> Option<String> {
    std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .ok()
}

/// The text of one step: the result, the name and the reason, then the fix
/// and the error on their own lines.
pub fn step_text(s: &Step, color: bool) -> String {
    let label = s.result.label();
    let label = if color {
        let st = match s.result {
            Outcome::Pass => style(label).green().bold(),
            Outcome::Fail => style(label).red().bold(),
            Outcome::Skip => style(label).yellow().bold(),
        };
        st.force_styling(true).to_string()
    } else {
        label.to_string()
    };
    let mut out = format!("{label}  {}: {}\n", s.name, s.reason);
    if s.result != Outcome::Pass
        && let Some(f) = &s.fix
    {
        out.push_str(&format!("      Fix: {f}\n"));
    }
    if let Some(e) = &s.error {
        out.push_str(&format!("      Error: {e}\n"));
    }
    out
}

/// The report under construction, and where the step lines go.
pub struct Session {
    pub report: Report,
    /// Ids of every phone seen in this run. They are not in the report:
    /// [`Report::finish`] removes them from the texts.
    pub ids: Vec<String>,
    /// Names of every phone seen in this run, for the same purpose.
    pub names: Vec<String>,
    /// Print each step when it completes.
    pub echo: bool,
    /// Print to stderr (stdout carries the JSON with `--json`).
    pub to_stderr: bool,
    pub color: bool,
}

impl Session {
    /// A session that prints nothing (unit tests).
    #[cfg(test)]
    pub fn quiet() -> Self {
        Self {
            report: Report::new(),
            ids: Vec::new(),
            names: Vec::new(),
            echo: false,
            to_stderr: false,
            color: false,
        }
    }

    fn say(&self, text: &str) {
        if !self.echo {
            return;
        }
        if self.to_stderr {
            eprint!("{text}");
        } else {
            print!("{text}");
        }
    }

    pub fn add(&mut self, s: Step) {
        self.say(&step_text(&s, self.color));
        self.report.steps.push(s);
        self.report.count();
    }

    /// Remember the id and the name of a phone, so that the report does
    /// not show them. A name that is the same as the model is not personal.
    pub fn remember(&mut self, info: &DeviceInfo) {
        if !info.id.is_empty() {
            self.ids.push(info.id.clone());
        }
        if info.name != info.model {
            self.names.push(info.name.clone());
        }
    }

    pub fn has(&self, id: &str) -> bool {
        self.report.steps.iter().any(|s| s.id == id)
    }

    /// Add a SKIP for every step that has no result yet.
    pub fn skip_rest(&mut self, reason: &str, fix: &str) {
        for (id, _) in STEPS {
            if !self.has(id) {
                self.add(Step::skip(id, reason).with_fix(fix));
            }
        }
    }
}

/// The setup check step from the doctor checks. A FAIL check is a blocker.
pub fn doctor_step(checks: &[Check]) -> Step {
    let fails: Vec<&Check> = checks.iter().filter(|c| c.status == Status::Fail).collect();
    let warns = checks.iter().filter(|c| c.status == Status::Warn).count();
    // An Android check name holds the serial: use a general name.
    let name = |c: &Check| {
        if c.id.starts_with("android_") {
            "Android phone".to_string()
        } else {
            c.name.clone()
        }
    };
    if fails.is_empty() {
        let reason = match warns {
            0 => "no problems".to_string(),
            w => format!("no blockers, {w} warning(s) (`agentphone doctor` shows them)"),
        };
        return Step::pass("doctor", reason);
    }
    let names: Vec<String> = fails.iter().map(|c| name(c)).collect();
    let error = fails
        .iter()
        .map(|c| format!("{}: {}", name(c), c.detail))
        .collect::<Vec<_>>()
        .join(" | ");
    let mut s = Step::fail(
        "doctor",
        format!("{} blocker(s): {}", fails.len(), names.join(", ")),
    )
    .with_fix(
        fails[0]
            .fix
            .clone()
            .unwrap_or_else(|| "run `agentphone setup`".into()),
    );
    s.error = Some(error);
    s
}

/// The connection step for a connected phone.
pub fn connect_step(info: &DeviceInfo) -> Step {
    let p = PhoneSummary::new(info);
    Step::pass(
        "connect",
        format!(
            "{} {}, model {}, screen {}, backend {}",
            p.platform, p.os_version, p.model, p.screen, p.backend
        ),
    )
}

fn save_png(dir: &Path, name: &str, f: &Frame) -> Result<()> {
    let png = f.to_png()?;
    let path = dir.join(name);
    std::fs::write(&path, png)
        .map_err(|e| anyhow::anyhow!("cannot write {}: {e}", path.display()))?;
    Ok(())
}

/// Take a screenshot and save it as `screenshot.png` in `dir`.
pub async fn screenshot_step(s: &mut Session, d: &dyn Device, dir: &Path) -> Option<Frame> {
    let frame = match d.screenshot().await {
        Ok(f) => f,
        Err(e) => {
            let fix = if e.is_unsupported() {
                "this backend cannot take screenshots. Send the report to the developer"
            } else {
                "unlock the phone, then run `agentphone selftest` again"
            };
            s.add(
                Step::fail("screenshot", "the phone did not send a screenshot")
                    .with_fix(fix)
                    .with_error(&e.into()),
            );
            return None;
        }
    };
    let name = "screenshot.png";
    match save_png(dir, name, &frame) {
        Ok(()) => {
            s.report.screenshots.push(name.into());
            s.add(Step::pass(
                "screenshot",
                format!(
                    "{} x {} px, saved as {}",
                    frame.width,
                    frame.height,
                    dir.join(name).display()
                ),
            ));
        }
        Err(e) => s.add(
            Step::fail(
                "screenshot",
                "the screenshot came, but the file write failed",
            )
            .with_fix("give a folder that you can write to with `--out DIR`")
            .with_error(&e),
        ),
    }
    Some(frame)
}

/// Run the input test or not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputPlan {
    Run,
    Skip { reason: String, fix: String },
}

/// Decide about the input test. `answer` is the answer to the question, or
/// `None` when nobody asked.
pub fn input_plan(yes: bool, terminal: bool, answer: Option<bool>) -> InputPlan {
    if yes {
        return InputPlan::Run;
    }
    if !terminal {
        return InputPlan::Skip {
            reason: "no terminal for the question".into(),
            fix: "run `agentphone selftest --yes` to run the input test".into(),
        };
    }
    match answer {
        Some(true) => InputPlan::Run,
        _ => InputPlan::Skip {
            reason: "you answered no".into(),
            fix: "run `agentphone selftest` again and answer y".into(),
        },
    }
}

/// Share of the screen that differs between two frames (ap-screen thumbnails).
pub fn changed_fraction(a: &Frame, b: &Frame, cfg: &SettleConfig) -> f32 {
    Thumb::from_frame(a, cfg.thumb_step)
        .diff_fraction(&Thumb::from_frame(b, cfg.thumb_step), cfg.cell_threshold)
}

/// The result of one input action.
enum Seen {
    /// The action failed.
    Error(anyhow::Error),
    /// The action was sent. `None`: no screenshot, so the change is unknown.
    /// `Some((big, fraction))`: `big` is true when the screen before and
    /// after the action differs by at least `big_fraction`.
    Sent(Option<(bool, f32)>),
}

/// Send one action, wait until the screen is still, save a screenshot.
async fn act(
    s: &mut Session,
    d: &dyn Device,
    settler: &mut Option<Settler>,
    dir: &Path,
    shot: &str,
    action: impl std::future::Future<Output = ap_core::Result<()>>,
) -> Seen {
    let before = settler.as_ref().and_then(|t| t.last_frame().cloned());
    let since = settler.as_ref().map(Settler::seq);
    if let Err(e) = action.await {
        return Seen::Error(e.into());
    }
    let Some(t) = settler.as_mut() else {
        return Seen::Sent(None);
    };
    let settled = match t.settle(d, since).await {
        Ok(st) => st,
        Err(e) => {
            tracing::debug!("selftest: settle failed: {e}");
            *settler = None;
            return Seen::Sent(None);
        }
    };
    if save_png(dir, shot, &settled.frame).is_ok() {
        s.report.screenshots.push(shot.into());
    }
    let cfg = t.config().clone();
    let fraction = before.map_or(1.0, |b| changed_fraction(&b, &settled.frame, &cfg));
    // Only a big difference counts. A clock tick, a live widget or a Live
    // Activity also increments `seq` and can pass `change_fraction`, but it
    // does not prove that the action arrived.
    Seen::Sent(Some((is_big_change(fraction, &cfg), fraction)))
}

/// True when `fraction` of the screen changed is enough to prove that an
/// action arrived: a new page or a new app, not a small ambient change.
pub fn is_big_change(fraction: f32, cfg: &SettleConfig) -> bool {
    fraction >= cfg.big_fraction
}

/// True for the bundle id or package name of a home screen app.
pub fn is_home_app(id: &str) -> bool {
    let id = id.trim().to_ascii_lowercase();
    id == "com.apple.springboard" || id == "com.miui.home" || id.contains("launcher")
}

/// The iOS version of an iPhone that cannot get touch input, or `None`.
/// An unknown version gives `None`: the input test then shows the result.
pub fn ios_without_input(info: &DeviceInfo) -> Option<&str> {
    if info.platform != Platform::Ios {
        return None;
    }
    let major = info
        .os_version
        .trim()
        .split('.')
        .next()?
        .parse::<u32>()
        .ok()?;
    (major < IOS_INPUT_MAJOR).then_some(info.os_version.as_str())
}

/// The SKIP plan for an iPhone below iOS 27, or `None`.
pub fn input_gate(info: &DeviceInfo) -> Option<InputPlan> {
    let v = ios_without_input(info)?;
    Some(InputPlan::Skip {
        reason: format!(
            "touch input needs iOS {IOS_INPUT_MAJOR} or later, and this iPhone has iOS {v}"
        ),
        fix: format!(
            "update the iPhone to iOS {IOS_INPUT_MAJOR} or later (a release build, not an early beta). Screenshots can work without it"
        ),
    })
}

/// Asks the owner if the home screen shows. `None` when the prompt fails.
pub type AskHome = fn() -> Option<bool>;

fn percent(f: f32) -> String {
    format!("{:.0}% of the screen", (f * 100.0).clamp(0.0, 100.0))
}

const AFTER_FAIL: &str = "fix the FAIL step above, then run `agentphone selftest` again";

const NO_CHANGE_FIX: &str = "unlock the phone, go to the home screen, then run `agentphone selftest` again. If the screen still does not change, send the report to the developer";

const NO_HOME_FIX: &str = "open an app on the phone (for example Settings), then run `agentphone selftest` again. Home must then change the screen";

/// The input test: Home, one swipe, Home again. Uses only these three
/// actions. `before` is the screenshot of the screenshot step.
///
/// The swipe is sent only when there is proof that the home screen shows:
/// Home changed the screen, the backend reports a home screen app, or the
/// owner answers yes to `ask_home`. Without proof, the swipe and the second
/// Home are skipped, because a swipe in an app or on the lock screen can do
/// an action (mark as read, archive, open a notification).
pub async fn input_steps(
    s: &mut Session,
    d: &dyn Device,
    plan: &InputPlan,
    before: Option<Frame>,
    cfg: SettleConfig,
    dir: &Path,
    ask_home: Option<AskHome>,
) {
    let gate = input_gate(d.info());
    if let InputPlan::Skip { reason, fix } = gate.as_ref().unwrap_or(plan) {
        for id in INPUT_STEPS {
            s.add(Step::skip(id, reason.clone()).with_fix(fix.clone()));
        }
        return;
    }
    let mut settler = before.map(|f| {
        let mut t = Settler::new(cfg);
        t.observe(f, std::time::Instant::now());
        t
    });

    // 1. Home. The phone can be on the home screen already, so no change is
    //    not a failure here. But only a change proves that the home screen
    //    shows now.
    let home_proof;
    let step = match act(s, d, &mut settler, dir, "after-home.png", d.home()).await {
        Seen::Error(e) => {
            s.add(
                Step::fail("home", "the phone did not accept the Home button")
                    .with_fix("run `agentphone doctor`, then send the report to the developer")
                    .with_error(&e),
            );
            for id in ["swipe", "home_again"] {
                s.add(
                    Step::skip(id, "Home failed, so the swipe was not sent").with_fix(AFTER_FAIL),
                );
            }
            return;
        }
        Seen::Sent(None) => {
            home_proof = false;
            Step::pass(
                "home",
                "sent; no screenshot, so the screen change is unknown",
            )
        }
        Seen::Sent(Some((true, f))) => {
            home_proof = true;
            Step::pass("home", format!("sent, the screen changed ({})", percent(f)))
        }
        Seen::Sent(Some((false, _))) => {
            home_proof = false;
            Step::pass(
                "home",
                "sent, the screen did not change (the phone was on the home screen, or the press did not arrive)",
            )
        }
    };
    s.add(step);

    // Proof that the home screen shows, before the swipe.
    let mut on_home =
        home_proof || matches!(d.foreground_app().await, Ok(Some(app)) if is_home_app(&app));
    if !on_home && let Some(ask) = ask_home {
        on_home = tokio::task::spawn_blocking(ask)
            .await
            .ok()
            .flatten()
            .unwrap_or(false);
    }
    if !on_home {
        let (reason, fix) = if ask_home.is_some() {
            (
                "you did not confirm the home screen, so the swipe was not sent",
                "go to the home screen, then run `agentphone selftest` again",
            )
        } else {
            (
                "no proof that the home screen shows, so the swipe was not sent",
                NO_HOME_FIX,
            )
        };
        for id in ["swipe", "home_again"] {
            s.add(Step::skip(id, reason).with_fix(fix));
        }
        return;
    }

    // 2. Swipe from left to right. On the home screen this must change the screen.
    let swipe = d.swipe(SWIPE_FROM, SWIPE_TO, Duration::from_millis(SWIPE_MS));
    let step = match act(s, d, &mut settler, dir, "after-swipe.png", swipe).await {
        Seen::Error(e) => Step::fail("swipe", "the phone did not accept the swipe")
            .with_fix("run `agentphone doctor`, then send the report to the developer")
            .with_error(&e),
        Seen::Sent(None) => Step::pass(
            "swipe",
            "sent; no screenshot, so the screen change is unknown",
        ),
        Seen::Sent(Some((true, f))) => Step::pass(
            "swipe",
            format!("sent, the screen changed ({})", percent(f)),
        ),
        Seen::Sent(Some((false, _))) => {
            Step::fail("swipe", "sent, but the screen did not change").with_fix(NO_CHANGE_FIX)
        }
    };
    let swipe_failed = step.result == Outcome::Fail;
    s.add(step);

    // 3. Home again: it goes back to a known screen. When the swipe ends on
    //    the first home page, Home does nothing there, so no change is not a
    //    failure.
    let result = act(s, d, &mut settler, dir, "after-home-again.png", d.home()).await;
    let step = match result {
        Seen::Error(e) => Step::fail("home_again", "the phone did not accept the Home button")
            .with_fix("run `agentphone doctor`, then send the report to the developer")
            .with_error(&e),
        _ if swipe_failed => Step::skip(
            "home_again",
            "sent, but the swipe did not change the screen, so this result tells nothing",
        )
        .with_fix(NO_CHANGE_FIX),
        Seen::Sent(None) => Step::pass(
            "home_again",
            "sent; no screenshot, so the screen change is unknown",
        ),
        Seen::Sent(Some((true, f))) => Step::pass(
            "home_again",
            format!("sent, the screen changed back ({})", percent(f)),
        ),
        Seen::Sent(Some((false, _))) => Step::pass(
            "home_again",
            "sent, the screen did not change (the swipe can end on the first home page, where Home does nothing)",
        ),
    };
    s.add(step);
}

/// Write the JSON and the text report into `dir`. Returns the JSON path.
pub fn write_report(dir: &Path, report: &Report) -> Result<PathBuf> {
    std::fs::create_dir_all(dir)
        .map_err(|e| anyhow::anyhow!("cannot create {}: {e}", dir.display()))?;
    let json = dir.join(REPORT_JSON);
    std::fs::write(&json, serde_json::to_string_pretty(report)? + "\n")
        .map_err(|e| anyhow::anyhow!("cannot write {}: {e}", json.display()))?;
    let txt = dir.join(REPORT_TXT);
    std::fs::write(&txt, report.text())
        .map_err(|e| anyhow::anyhow!("cannot write {}: {e}", txt.display()))?;
    Ok(json)
}

/// Process exit code: 0 when every step that ran passed.
pub fn exit_code(report: &Report) -> u8 {
    if report.ok() { 0 } else { 1 }
}

/// Ask if the home screen shows, when Home gave no proof. `None` when the
/// prompt fails. The default is no.
fn ask_home_screen() -> Option<bool> {
    dialoguer::Confirm::with_theme(&dialoguer::theme::ColorfulTheme::default())
        .with_prompt("Home did not change the screen. Is the home screen showing on the phone now?")
        .default(false)
        .interact()
        .ok()
}

/// Ask the input question. `None` when the prompt fails.
fn ask_input() -> Option<bool> {
    eprintln!("\n{INPUT_EXPLAIN}");
    dialoguer::Confirm::with_theme(&dialoguer::theme::ColorfulTheme::default())
        .with_prompt("Start the input test?")
        .default(false)
        .interact()
        .ok()
}

/// All steps. Each result goes into `s` when it is known, so a Ctrl+C keeps
/// the results so far. The connected phone goes into `slot`.
async fn run_steps(
    s: &mut Session,
    cfg_path: &Path,
    wanted: Option<&str>,
    out: &Path,
    yes: bool,
    slot: &mut Option<Box<dyn Device>>,
) {
    // 1. Setup check.
    let (checks, found) = doctor::gather_with_devices(cfg_path, wanted).await;
    for info in &found {
        s.remember(info);
    }
    let step = doctor_step(&checks);
    let blocked = step.result == Outcome::Fail;
    s.add(step);
    if blocked {
        s.say(&format!("\n{}", doctor::render(&checks, s.color)));
        s.skip_rest("the setup check found a blocker", AFTER_FAIL);
        return;
    }

    // 2. Connection.
    let cfg = match config_cmd::load(cfg_path) {
        Ok(c) => c,
        Err(e) => {
            s.add(Step::fail("connect", "cannot read the config file").with_error(&e));
            s.skip_rest("no phone connection", AFTER_FAIL);
            return;
        }
    };
    match devices::connect(&cfg, wanted).await {
        Ok(d) => {
            s.remember(d.info());
            s.report.phone = Some(PhoneSummary::new(d.info()));
            s.add(connect_step(d.info()));
            *slot = Some(d);
        }
        Err(e) => {
            s.add(
                Step::fail("connect", "agentphone cannot connect to the phone")
                    .with_fix("unlock the phone, then run `agentphone doctor`")
                    .with_error(&e),
            );
            s.skip_rest("no phone connection", AFTER_FAIL);
            return;
        }
    }
    let d = slot.as_deref().expect("connected above");

    // 3. Screenshot.
    if let Err(e) = std::fs::create_dir_all(out) {
        s.add(
            Step::fail("screenshot", "cannot make the selftest folder")
                .with_fix("give a folder that you can write to with `--out DIR`")
                .with_error(&anyhow::anyhow!("{}: {e}", out.display())),
        );
        s.skip_rest("no folder for the screenshots", AFTER_FAIL);
        return;
    }
    let before = screenshot_step(s, d, out).await;

    // 4. Input test, only after a yes. Below iOS 27 there is no question.
    let terminal = ui::interactive();
    let plan = match input_gate(d.info()) {
        Some(skip) => skip,
        None => {
            let answer = if !yes && terminal {
                tokio::task::spawn_blocking(ask_input).await.ok().flatten()
            } else {
                None
            };
            input_plan(yes, terminal, answer)
        }
    };
    // With `--yes` there is no question about the home screen either.
    let ask_home: Option<AskHome> = (!yes && terminal).then_some(ask_home_screen as AskHome);
    input_steps(s, d, &plan, before, SettleConfig::default(), out, ask_home).await;
}

/// `agentphone selftest`.
pub async fn command(
    cfg_path: &Path,
    wanted: Option<&str>,
    out: &Path,
    yes: bool,
    json: bool,
) -> Result<u8> {
    let mut s = Session {
        report: Report::new(),
        ids: Vec::new(),
        names: Vec::new(),
        echo: true,
        to_stderr: json,
        color: !json && console::colors_enabled(),
    };
    s.say(&format!(
        "agentphone selftest {}. Press Ctrl+C to stop.\n\n",
        env!("CARGO_PKG_VERSION")
    ));
    let mut slot: Option<Box<dyn Device>> = None;
    let ctrl_c = async {
        if tokio::signal::ctrl_c().await.is_err() {
            std::future::pending::<()>().await;
        }
    };
    let stopped = tokio::select! {
        _ = run_steps(&mut s, cfg_path, wanted, out, yes, &mut slot) => false,
        _ = ctrl_c => true,
    };
    if stopped {
        if let Some(d) = &slot {
            d.cancel();
        }
        s.say("\n");
        s.report.stopped_by_user = true;
        s.skip_rest("stopped with Ctrl+C", "run `agentphone selftest` again");
    }

    if let Some(d) = &slot {
        s.remember(d.info());
    }
    let mut ids = std::mem::take(&mut s.ids);
    ids.extend(wanted.map(str::to_string));
    if let Ok(cfg) = config_cmd::load(cfg_path) {
        ids.extend(cfg.default_device);
    }
    let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
    let names: Vec<&str> = s.names.iter().map(String::as_str).collect();
    let home = home_dir();
    s.report.finish(&ids, &names, home.as_deref());
    let code = exit_code(&s.report);

    s.say(&format!("\n{}\n", s.report.summary()));
    match write_report(out, &s.report) {
        Ok(path) => {
            if json {
                ui::json(&s.report)?;
            }
            s.say(&format!(
                "Report files:\n  {}\n  {}\nSend {REPORT_JSON} to the developer.\n",
                path.display(),
                out.join(REPORT_TXT).display(),
            ));
        }
        Err(e) => {
            ui::print_error(&e);
            if json {
                ui::json(&s.report)?;
            }
        }
    }
    if stopped {
        // The input question can still wait on stdin in a blocking thread,
        // and the runtime waits for it on shutdown. Exit now.
        use std::io::Write;
        let _ = std::io::stdout().flush();
        let _ = std::io::stderr().flush();
        std::process::exit(code.into());
    }
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ap_core::mock::MockDevice;
    use ap_core::{Button, Capabilities, Gesture, Key, Size, SwipeDirection};
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A phone whose screen colour follows the actions it got. `moves[i]` is
    /// false when action `i` (0 = Home, 1 = swipe, 2 = Home again) arrives
    /// but does not change the screen. `ambient` changes one thumbnail cell
    /// (1/32 of the screen) on every screenshot, like a clock tick. `app` is
    /// the foreground app that the backend reports.
    struct Screen {
        inner: MockDevice,
        moves: [bool; 3],
        ambient: bool,
        app: Option<&'static str>,
        shots: AtomicUsize,
    }

    impl Screen {
        fn new(swipe_moves: bool) -> Self {
            Self::with_moves([true, swipe_moves, true])
        }

        fn with_moves(moves: [bool; 3]) -> Self {
            let m = MockDevice::new();
            let mut info = m.info().clone();
            info.id = "00008110-001A2B3C4D5E801E".into();
            info.os_version = "27.0".into();
            info.screen_pixels = Size::new(64.0, 128.0);
            Self {
                inner: m.with_info(info),
                moves,
                ambient: false,
                app: None,
                shots: AtomicUsize::new(0),
            }
        }

        fn calls(&self) -> Vec<Gesture> {
            self.inner.calls()
        }
    }

    #[async_trait]
    impl Device for Screen {
        fn info(&self) -> &DeviceInfo {
            self.inner.info()
        }
        fn capabilities(&self) -> Capabilities {
            self.inner.capabilities()
        }
        async fn screenshot(&self) -> ap_core::Result<Frame> {
            let n = (0..self.inner.calls().len())
                .filter(|i| self.moves.get(*i).copied().unwrap_or(true))
                .count();
            let v = (40 + 60 * (n % 4)) as u8;
            let s = self.info().screen_pixels;
            let mut f = Frame::solid(s.width as u32, s.height as u32, [v, v, v, 255]);
            let shot = self.shots.fetch_add(1, Ordering::SeqCst);
            if self.ambient && shot % 2 == 1 {
                // The thumbnail samples pixel (0, 0) for its first cell.
                let c = v.wrapping_add(100);
                f.image.get_pixel_mut(0, 0).0 = [c, c, c, 255];
            }
            Ok(f)
        }
        async fn tap(&self, at: Point) -> ap_core::Result<()> {
            self.inner.tap(at).await
        }
        async fn double_tap(&self, at: Point) -> ap_core::Result<()> {
            self.inner.double_tap(at).await
        }
        async fn long_press(&self, at: Point, d: Duration) -> ap_core::Result<()> {
            self.inner.long_press(at, d).await
        }
        async fn swipe(&self, a: Point, b: Point, d: Duration) -> ap_core::Result<()> {
            self.inner.swipe(a, b, d).await
        }
        async fn scroll(&self, at: Point, dir: SwipeDirection, amt: f32) -> ap_core::Result<()> {
            self.inner.scroll(at, dir, amt).await
        }
        async fn type_text(&self, t: &str) -> ap_core::Result<()> {
            self.inner.type_text(t).await
        }
        async fn key(&self, k: &Key) -> ap_core::Result<()> {
            self.inner.key(k).await
        }
        async fn button(&self, b: Button) -> ap_core::Result<()> {
            self.inner.button(b).await
        }
        async fn launch_app(&self, id: &str) -> ap_core::Result<()> {
            self.inner.launch_app(id).await
        }
        async fn foreground_app(&self) -> ap_core::Result<Option<String>> {
            Ok(self.app.map(str::to_string))
        }
    }

    fn fast() -> SettleConfig {
        SettleConfig {
            quiet: Duration::from_millis(10),
            change_wait: Duration::from_millis(40),
            ambient_after: Duration::from_millis(30),
            timeout: Duration::from_millis(300),
            poll_interval: Duration::from_millis(2),
            ..SettleConfig::default()
        }
    }

    /// A mock phone with iOS 27, so that the input test runs.
    fn mock27(m: MockDevice) -> MockDevice {
        let mut info = m.info().clone();
        info.os_version = "27.0".into();
        m.with_info(info)
    }

    fn yes() -> Option<bool> {
        Some(true)
    }

    fn no() -> Option<bool> {
        Some(false)
    }

    async fn device_part(
        d: &dyn Device,
        plan: &InputPlan,
        dir: &Path,
        ask: Option<AskHome>,
    ) -> Session {
        let mut s = Session::quiet();
        s.add(Step::pass("doctor", "no problems"));
        s.add(connect_step(d.info()));
        s.report.phone = Some(PhoneSummary::new(d.info()));
        let before = screenshot_step(&mut s, d, dir).await;
        input_steps(&mut s, d, plan, before, fast(), dir, ask).await;
        s
    }

    fn results(s: &Session) -> Vec<(&str, Outcome)> {
        s.report.steps.iter().map(|x| (x.id, x.result)).collect()
    }

    fn has_swipe(calls: &[Gesture]) -> bool {
        calls.iter().any(|g| matches!(g, Gesture::Swipe { .. }))
    }

    #[tokio::test]
    async fn all_steps_pass_and_only_harmless_input_is_sent() {
        let d = Screen::new(true);
        let dir = tempfile::tempdir().unwrap();
        let mut s = device_part(&d, &InputPlan::Run, dir.path(), None).await;
        use Outcome::Pass;
        assert_eq!(
            results(&s),
            vec![
                ("doctor", Pass),
                ("connect", Pass),
                ("screenshot", Pass),
                ("home", Pass),
                ("swipe", Pass),
                ("home_again", Pass)
            ],
            "{:#?}",
            s.report.steps
        );
        // Only Home, one swipe, Home: no tap, no text, no app launch.
        let calls = d.calls();
        assert_eq!(calls.len(), 3, "{calls:?}");
        assert_eq!(
            calls[0],
            Gesture::Button {
                button: Button::Home
            }
        );
        assert!(
            matches!(calls[1], Gesture::Swipe { from, to, .. } if from == SWIPE_FROM && to == SWIPE_TO)
        );
        assert_eq!(
            calls[2],
            Gesture::Button {
                button: Button::Home
            }
        );
        for f in [
            "screenshot.png",
            "after-home.png",
            "after-swipe.png",
            "after-home-again.png",
        ] {
            assert!(dir.path().join(f).exists(), "{f}");
        }
        s.report.finish(&[], &[], None);
        assert!(s.report.ok());
        assert_eq!(exit_code(&s.report), 0);
        assert_eq!(
            (s.report.passed, s.report.failed, s.report.skipped),
            (6, 0, 0)
        );
    }

    #[tokio::test]
    async fn a_swipe_without_effect_fails() {
        let d = Screen::new(false);
        let dir = tempfile::tempdir().unwrap();
        let mut s = device_part(&d, &InputPlan::Run, dir.path(), None).await;
        let r = results(&s);
        assert_eq!(r[3], ("home", Outcome::Pass));
        assert_eq!(r[4], ("swipe", Outcome::Fail));
        assert_eq!(r[5], ("home_again", Outcome::Skip));
        assert!(
            s.report.steps[4]
                .fix
                .as_deref()
                .unwrap()
                .contains("home screen")
        );
        s.report.finish(&[], &[], None);
        assert_eq!(exit_code(&s.report), 1);
    }

    /// A clock tick or a live widget during the wait is not a swipe result.
    #[tokio::test]
    async fn an_ambient_change_does_not_pass_the_swipe() {
        let mut d = Screen::new(false);
        d.ambient = true;
        let dir = tempfile::tempdir().unwrap();
        let s = device_part(&d, &InputPlan::Run, dir.path(), None).await;
        let r = results(&s);
        assert_eq!(r[3], ("home", Outcome::Pass), "{:#?}", s.report.steps);
        assert_eq!(r[4], ("swipe", Outcome::Fail), "{:#?}", s.report.steps);
        // The small change is real, but it is below `big_fraction`.
        let cfg = fast();
        assert!(1.0 / 32.0 > cfg.change_fraction);
        assert!(!is_big_change(1.0 / 32.0, &cfg));
        assert!(is_big_change(0.5, &cfg));
    }

    /// Home failed: the swipe is never sent.
    #[tokio::test]
    async fn a_home_error_skips_the_swipe() {
        let m = mock27(MockDevice::new().with_capabilities(Capabilities {
            screenshot: true,
            ..Capabilities::NONE
        }));
        let mut info = m.info().clone();
        info.screen_pixels = Size::new(32.0, 64.0);
        let m = m.with_info(info);
        let dir = tempfile::tempdir().unwrap();
        let s = device_part(&m, &InputPlan::Run, dir.path(), Some(yes)).await;
        let r = results(&s);
        assert_eq!(r[3], ("home", Outcome::Fail));
        assert_eq!(r[4], ("swipe", Outcome::Skip));
        assert_eq!(r[5], ("home_again", Outcome::Skip));
        assert!(
            s.report.steps[3]
                .error
                .as_deref()
                .unwrap()
                .contains("cannot do this")
        );
        assert!(s.report.steps[4].reason.contains("Home failed"));
        assert!(!has_swipe(&m.calls()), "{:?}", m.calls());
    }

    /// Home did not change the screen and nobody can confirm the home
    /// screen: the swipe is not sent.
    #[tokio::test]
    async fn no_proof_of_the_home_screen_skips_the_swipe() {
        let d = Screen::with_moves([false, true, true]);
        let dir = tempfile::tempdir().unwrap();
        let mut s = device_part(&d, &InputPlan::Run, dir.path(), None).await;
        let r = results(&s);
        assert_eq!(r[3], ("home", Outcome::Pass));
        assert_eq!(r[4], ("swipe", Outcome::Skip));
        assert_eq!(r[5], ("home_again", Outcome::Skip));
        assert!(s.report.steps[4].reason.contains("no proof"));
        assert!(
            s.report.steps[4]
                .fix
                .as_deref()
                .unwrap()
                .contains("open an app")
        );
        assert_eq!(d.calls().len(), 1, "{:?}", d.calls());
        s.report.finish(&[], &[], None);
        assert_eq!(exit_code(&s.report), 0);

        // The owner answers no: the swipe is not sent.
        let d = Screen::with_moves([false, true, true]);
        let s = device_part(&d, &InputPlan::Run, dir.path(), Some(no)).await;
        assert_eq!(results(&s)[4], ("swipe", Outcome::Skip));
        assert!(s.report.steps[4].reason.contains("did not confirm"));
        assert!(!has_swipe(&d.calls()));

        // The owner answers yes: the swipe is sent.
        let d = Screen::with_moves([false, true, true]);
        let s = device_part(&d, &InputPlan::Run, dir.path(), Some(yes)).await;
        assert_eq!(results(&s)[4], ("swipe", Outcome::Pass));
        assert_eq!(d.calls().len(), 3);
    }

    /// The backend reports the launcher: that is proof of the home screen.
    #[tokio::test]
    async fn a_home_screen_app_is_proof() {
        let mut d = Screen::with_moves([false, true, true]);
        d.app = Some("com.sec.android.app.launcher");
        let dir = tempfile::tempdir().unwrap();
        let s = device_part(&d, &InputPlan::Run, dir.path(), None).await;
        assert_eq!(results(&s)[4], ("swipe", Outcome::Pass));
        assert!(has_swipe(&d.calls()));

        let mut d = Screen::with_moves([false, true, true]);
        d.app = Some("com.google.android.gm");
        let s = device_part(&d, &InputPlan::Run, dir.path(), None).await;
        assert_eq!(results(&s)[4], ("swipe", Outcome::Skip));
        assert!(!has_swipe(&d.calls()));

        assert!(is_home_app("com.apple.springboard"));
        assert!(is_home_app("com.google.android.apps.nexuslauncher"));
        assert!(is_home_app("com.miui.home"));
        assert!(!is_home_app("com.apple.mobilemail"));
    }

    /// The swipe ends on the first home page, where Home does nothing.
    #[tokio::test]
    async fn home_again_without_change_passes() {
        let d = Screen::with_moves([true, true, false]);
        let dir = tempfile::tempdir().unwrap();
        let mut s = device_part(&d, &InputPlan::Run, dir.path(), None).await;
        let r = results(&s);
        assert_eq!(r[4], ("swipe", Outcome::Pass));
        assert_eq!(r[5], ("home_again", Outcome::Pass));
        assert!(s.report.steps[5].reason.contains("first home page"));
        s.report.finish(&[], &[], None);
        assert_eq!(exit_code(&s.report), 0);
    }

    /// Below iOS 27 the input test is skipped and sends nothing.
    #[tokio::test]
    async fn ios_below_27_skips_the_input_test() {
        let m = MockDevice::new();
        assert_eq!(m.info().os_version, "26.0");
        let dir = tempfile::tempdir().unwrap();
        let s = device_part(&m, &InputPlan::Run, dir.path(), Some(yes)).await;
        let skips: Vec<&Step> = s.report.steps[3..].iter().collect();
        assert!(skips.iter().all(|x| x.result == Outcome::Skip));
        assert!(skips[0].reason.contains("iOS 27"), "{}", skips[0].reason);
        assert!(skips[0].reason.contains("26.0"), "{}", skips[0].reason);
        assert!(m.calls().is_empty());

        let mut info = m.info().clone();
        assert!(input_gate(&info).is_some());
        for v in ["27.0", "27.0.1", "28.1", "", "beta"] {
            info.os_version = v.into();
            assert!(input_gate(&info).is_none(), "{v}");
        }
        info.os_version = "17.4".into();
        assert!(input_gate(&info).is_some());
        info.platform = Platform::Android;
        info.os_version = "14".into();
        assert!(input_gate(&info).is_none());
    }

    #[tokio::test]
    async fn skipped_input_sends_nothing() {
        let d = Screen::new(true);
        let dir = tempfile::tempdir().unwrap();
        let plan = input_plan(false, false, None);
        let mut s = device_part(&d, &plan, dir.path(), Some(yes)).await;
        assert!(d.calls().is_empty());
        let skips: Vec<&Step> = s
            .report
            .steps
            .iter()
            .filter(|x| x.result == Outcome::Skip)
            .collect();
        assert_eq!(skips.len(), 3);
        assert!(skips[0].fix.as_deref().unwrap().contains("--yes"));
        s.report.finish(&[], &[], None);
        // SKIP is not a failure.
        assert_eq!(exit_code(&s.report), 0);
    }

    #[tokio::test]
    async fn no_screenshots_means_unknown_change() {
        let m = mock27(MockDevice::new().with_capabilities(Capabilities {
            screenshot: false,
            ..Capabilities::ALL
        }));
        let dir = tempfile::tempdir().unwrap();
        // Without screenshots, only the owner can confirm the home screen.
        let s = device_part(&m, &InputPlan::Run, dir.path(), Some(yes)).await;
        let r = results(&s);
        assert_eq!(r[2], ("screenshot", Outcome::Fail));
        assert!(s.report.steps[2].error.is_some());
        assert_eq!(r[4], ("swipe", Outcome::Pass));
        assert!(s.report.steps[4].reason.contains("unknown"));
        assert_eq!(m.calls().len(), 3);

        let m = mock27(MockDevice::new().with_capabilities(Capabilities {
            screenshot: false,
            ..Capabilities::ALL
        }));
        let s = device_part(&m, &InputPlan::Run, dir.path(), None).await;
        assert_eq!(results(&s)[4], ("swipe", Outcome::Skip));
        assert_eq!(m.calls().len(), 1);
    }

    #[tokio::test]
    async fn a_swipe_error_is_a_failure() {
        struct NoSwipe(Screen);
        #[async_trait]
        impl Device for NoSwipe {
            fn info(&self) -> &DeviceInfo {
                self.0.info()
            }
            fn capabilities(&self) -> Capabilities {
                self.0.capabilities()
            }
            async fn screenshot(&self) -> ap_core::Result<Frame> {
                self.0.screenshot().await
            }
            async fn tap(&self, at: Point) -> ap_core::Result<()> {
                self.0.tap(at).await
            }
            async fn double_tap(&self, at: Point) -> ap_core::Result<()> {
                self.0.double_tap(at).await
            }
            async fn long_press(&self, at: Point, d: Duration) -> ap_core::Result<()> {
                self.0.long_press(at, d).await
            }
            async fn swipe(&self, _: Point, _: Point, _: Duration) -> ap_core::Result<()> {
                Err(ap_core::Error::Unsupported("swipe".into()))
            }
            async fn scroll(&self, at: Point, d: SwipeDirection, a: f32) -> ap_core::Result<()> {
                self.0.scroll(at, d, a).await
            }
            async fn type_text(&self, t: &str) -> ap_core::Result<()> {
                self.0.type_text(t).await
            }
            async fn key(&self, k: &Key) -> ap_core::Result<()> {
                self.0.key(k).await
            }
            async fn button(&self, b: Button) -> ap_core::Result<()> {
                self.0.button(b).await
            }
            async fn launch_app(&self, id: &str) -> ap_core::Result<()> {
                self.0.launch_app(id).await
            }
        }
        let d = NoSwipe(Screen::new(true));
        let dir = tempfile::tempdir().unwrap();
        let s = device_part(&d, &InputPlan::Run, dir.path(), None).await;
        let r = results(&s);
        assert_eq!(r[3], ("home", Outcome::Pass));
        assert_eq!(r[4], ("swipe", Outcome::Fail));
        assert!(s.report.steps[4].error.is_some());
    }

    #[test]
    fn input_plan_rules() {
        assert_eq!(input_plan(true, false, None), InputPlan::Run);
        assert_eq!(input_plan(false, true, Some(true)), InputPlan::Run);
        // The default answer is no.
        assert!(matches!(
            input_plan(false, true, Some(false)),
            InputPlan::Skip { .. }
        ));
        assert!(matches!(
            input_plan(false, true, None),
            InputPlan::Skip { .. }
        ));
        match input_plan(false, false, Some(true)) {
            InputPlan::Skip { reason, .. } => assert!(reason.contains("terminal")),
            p => panic!("{p:?}"),
        }
    }

    #[test]
    fn doctor_blockers_stop_the_test() {
        let ok = vec![
            Check::new("Config", "config", "Config file", Status::Ok, "x"),
            Check::new("OCR", "ocr", "OCR", Status::Warn, "not in this build"),
        ];
        let s = doctor_step(&ok);
        assert_eq!(s.result, Outcome::Pass);
        assert!(s.reason.contains("1 warning"));
        let bad = vec![
            Check::new(
                "iPhone",
                "usbmuxd",
                "Apple Mobile Device service",
                Status::Fail,
                "not running",
            )
            .with_fix("install the Apple Devices app"),
            Check::new(
                "Android",
                "android_unauthorized",
                "Phone R5CX123456",
                Status::Fail,
                "not allowed yet",
            ),
        ];
        let s = doctor_step(&bad);
        assert_eq!(s.result, Outcome::Fail);
        assert_eq!(s.fix.as_deref(), Some("install the Apple Devices app"));
        assert!(s.reason.starts_with("2 blocker(s)"));
        let all = format!("{} {}", s.reason, s.error.as_deref().unwrap());
        assert!(!all.contains("R5CX123456"), "{all}");
    }

    /// Phone names and Android serials in error texts do not go into the
    /// report: the paired check, two connected phones, Wi-Fi.
    #[test]
    fn names_and_android_serials_are_removed() {
        let mut s = Session::quiet();
        let mut ios = MockDevice::new().info().clone();
        ios.id = "00008110-001A2B3C4D5E801E".into();
        ios.name = "Ozan's iPhone".into();
        let mut android = ios.clone();
        android.id = "R5CX123456".into();
        android.name = "Galaxy of Ozan".into();
        android.platform = Platform::Android;
        android.model = "SM-S921B".into();
        let mut plain = android.clone();
        plain.id = "emulator-5554".into();
        plain.name = "Pixel 8".into();
        plain.model = "Pixel 8".into();
        for i in [&ios, &android, &plain] {
            s.remember(i);
        }
        // A name that is the model is not personal.
        assert_eq!(s.names, vec!["Ozan's iPhone", "Galaxy of Ozan"]);

        let paired = Check::new(
            "iPhone",
            "paired",
            "Pairing",
            Status::Fail,
            "Ozan's iPhone does not trust this computer",
        );
        s.add(doctor_step(&[paired]));
        s.add(
            Step::fail("connect", "agentphone cannot connect to the phone").with_error(
                &anyhow::Error::new(ap_core::Error::setup(
                    "more than one phone is connected: R5CX123456 (Galaxy of Ozan, android), 00008110-001A2B3C4D5E801E (Ozan's iPhone, ios)",
                    "choose one",
                )),
            ),
        );
        s.add(Step::fail(
            "screenshot",
            "Ozan's iPhone is not reachable over Wi-Fi",
        ));
        let ids: Vec<&str> = s.ids.iter().map(String::as_str).collect();
        let names: Vec<&str> = s.names.iter().map(String::as_str).collect();
        s.report.finish(&ids, &names, None);
        let text = s.report.text();
        let json = serde_json::to_string(&s.report).unwrap();
        for all in [&text, &json] {
            for secret in ["Ozan", "R5CX123456", "00008110-001A2B3C4D5E801E"] {
                assert!(!all.contains(secret), "{secret} in {all}");
            }
        }
        assert!(
            text.contains("[phone name] does not trust this computer"),
            "{text}"
        );
        assert!(text.contains("****3456 ([phone name], android)"), "{text}");

        // Default names stay: they are not personal and are normal words.
        assert_eq!(
            redact("unlock the iPhone", &[], &["iPhone", "ab"], None),
            "unlock the iPhone"
        );
    }

    #[test]
    fn skip_rest_fills_missing_steps() {
        let mut s = Session::quiet();
        s.add(Step::pass("doctor", "no problems"));
        s.add(Step::fail("connect", "no phone"));
        s.skip_rest("no phone connection", AFTER_FAIL);
        let ids: Vec<&str> = s.report.steps.iter().map(|x| x.id).collect();
        assert_eq!(ids, STEPS.iter().map(|(i, _)| *i).collect::<Vec<_>>());
        assert_eq!(s.report.skipped, 4);
        s.report.stopped_by_user = true;
        assert!(!s.report.ok());
    }

    #[test]
    fn ids_and_paths_are_removed() {
        assert_eq!(mask_id("00008110-001A2B3C4D5E801E"), "****801E");
        assert_eq!(mask_id("abc"), "****");
        let t = redact(
            "no phone with id \"R5CX9Z\" at C:\\Users\\Ozan\\x; udid 00008110-001A2B3C4D5E801E, hash 0123456789abcdef0123456789abcdef01234567, model iPhone14,5",
            &["R5CX9Z"],
            &[],
            Some("C:\\Users\\Ozan"),
        );
        assert_eq!(
            t,
            "no phone with id \"****CX9Z\" at ~\\x; udid ****801E, hash ****4567, model iPhone14,5"
        );
        // Normal words and short numbers stay.
        assert_eq!(
            redact("iOS 26.0, 1170 x 2532 px", &[], &[], None),
            "iOS 26.0, 1170 x 2532 px"
        );
    }

    #[test]
    fn report_files() {
        let d = Screen::new(true);
        let mut r = Report::new();
        r.phone = Some(PhoneSummary::new(d.info()));
        r.steps.push(Step::pass("doctor", "no problems"));
        r.steps.push(connect_step(d.info()));
        r.steps.push(
            Step::fail("screenshot", "the phone did not send a screenshot")
                .with_fix("unlock the phone")
                .with_error(&anyhow::Error::new(ap_core::Error::NotConnected(
                    "lost 00008110-001A2B3C4D5E801E".into(),
                ))),
        );
        r.steps
            .push(Step::skip("home", "you answered no").with_fix("answer y"));
        r.finish(&[&d.info().id], &[&d.info().name], None);
        assert_eq!((r.passed, r.failed, r.skipped), (2, 1, 1));
        assert!(!r.ok());

        let dir = tempfile::tempdir().unwrap();
        let json = write_report(&dir.path().join("sub"), &r).unwrap();
        assert!(json.ends_with(REPORT_JSON));
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&json).unwrap()).unwrap();
        assert_eq!(v["tool"], "agentphone");
        assert_eq!(v["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(v["phone"]["model"], "iPhone14,5");
        assert_eq!(v["phone"]["os_version"], "27.0");
        assert_eq!(v["phone"]["id"], "****801E");
        assert_eq!(v["steps"][2]["result"], "FAIL");
        assert_eq!(v["steps"][3]["result"], "SKIP");
        let txt = std::fs::read_to_string(dir.path().join("sub").join(REPORT_TXT)).unwrap();
        for f in [&json, &dir.path().join("sub").join(REPORT_TXT)] {
            let all = std::fs::read_to_string(f).unwrap();
            assert!(!all.contains("00008110-001A2B3C4D5E801E"), "{all}");
            assert!(
                !all.contains("Mock iPhone"),
                "the phone name is personal: {all}"
            );
        }
        assert!(txt.contains("PASS  Setup check: no problems\n"), "{txt}");
        assert!(txt.contains("FAIL  Screenshot: the phone did not send a screenshot\n      Fix: unlock the phone\n      Error: the phone is not connected: lost ****801E\n"), "{txt}");
        assert!(
            txt.contains("SKIP  Press Home: you answered no\n      Fix: answer y\n"),
            "{txt}"
        );
        assert!(txt.contains("model:    iPhone14,5\n"), "{txt}");
        assert!(txt.ends_with("2 passed, 1 failed, 1 skipped.\n"), "{txt}");
    }
}
