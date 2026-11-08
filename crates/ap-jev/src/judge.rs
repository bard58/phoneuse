//! High-level screen judgments. Every call checks the [`Privacy`] guard
//! before it builds a request.

use std::collections::BTreeMap;

use ap_core::{Error, Frame, Point, Result};
use serde_json::Value;

use crate::client::JevClient;
use crate::image::ImageAsset;
use crate::privacy::Privacy;
use crate::types::{Answers, Asset, Choice, Question, Usage};

/// Thresholds for [`ScreenJudge`]. Tune them on real screens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JudgeConfig {
    /// Noul probability at or above which a flag is `true`.
    pub flag_threshold: f32,
    /// Minimum Choice confidence for [`TargetPick::accepted`].
    pub pick_confidence: f32,
    /// Minimum probability of the picked option for [`TargetPick::accepted`].
    pub pick_probability: f32,
    /// Minimum "is it present" probability for [`ScreenJudge::locate`].
    pub present_threshold: f32,
    /// Minimum Box confidence for [`ScreenJudge::locate`] (ignored when null).
    pub box_confidence: f32,
}

impl Default for JudgeConfig {
    fn default() -> Self {
        Self {
            flag_threshold: 0.5,
            pick_confidence: 0.7,
            pick_probability: 0.6,
            present_threshold: 0.6,
            box_confidence: 0.3,
        }
    }
}

/// Raw probabilities behind a [`ScreenJudgment`].
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ScreenProbabilities {
    pub settled: f32,
    pub locked: f32,
    pub keyboard_visible: f32,
    pub alert_visible: f32,
    pub sensitive: f32,
}

/// Result of [`ScreenJudge::judge`].
#[derive(Debug, Clone, PartialEq)]
pub struct ScreenJudgment {
    /// Fully loaded and still: no spinner, skeleton or transition.
    pub settled: bool,
    /// Lock screen or passcode prompt.
    pub locked: bool,
    /// On-screen keyboard is shown.
    pub keyboard_visible: bool,
    /// A dialog, alert, permission prompt or sheet covers the app.
    pub alert_visible: bool,
    /// Probability that `last_action` is hard to undo (0 when no action).
    pub action_irreversible: f32,
    /// Probability that the screen shows private data (passwords, bank
    /// details, private messages, health data).
    pub sensitive: f32,
    pub probabilities: ScreenProbabilities,
    pub model: String,
    pub usage: Usage,
}

/// Candidates for [`ScreenJudge::pick_target`].
#[derive(Debug, Clone, Copy)]
pub struct PickInput<'a> {
    /// Screenshot that shows the candidates (optional).
    pub frame: Option<&'a Frame>,
    /// Text candidates, for example OCR lines or UI element labels.
    pub candidates: &'a [String],
}

/// Result of [`ScreenJudge::pick_target`].
#[derive(Debug, Clone, PartialEq)]
pub struct TargetPick {
    /// The raw Choice. Option names are candidate indexes and `"none"`.
    pub choice: Choice,
    /// Index into `candidates`, or `None` when Jev picked "none".
    pub index: Option<usize>,
    /// True when a candidate was picked with enough confidence. When
    /// false, ask the main LLM instead.
    pub accepted: bool,
}

const NONE_OPTION: &str = "none";
/// Choice allows at most 255 options; one is "none".
pub const MAX_CANDIDATES: usize = 254;

/// High-level Jev checks for phone screens.
#[derive(Debug)]
pub struct ScreenJudge {
    client: JevClient,
    privacy: Privacy,
    config: JudgeConfig,
}

impl ScreenJudge {
    pub fn new(client: JevClient, privacy: Privacy) -> Self {
        Self {
            client,
            privacy,
            config: JudgeConfig::default(),
        }
    }

    pub fn with_config(mut self, config: JudgeConfig) -> Self {
        self.config = config;
        self
    }

    pub fn privacy(&self) -> &Privacy {
        &self.privacy
    }

    pub fn config(&self) -> &JudgeConfig {
        &self.config
    }

    pub fn client(&self) -> &JevClient {
        &self.client
    }

    async fn ask_screen(
        &self,
        frame: Option<&Frame>,
        mut state: serde_json::Map<String, Value>,
        questions: BTreeMap<String, Question>,
    ) -> Result<Answers> {
        self.privacy.check()?;
        let mut assets = BTreeMap::<String, Asset>::new();
        if let Some(f) = frame {
            let img = ImageAsset::from_frame(f)?;
            state.insert("screen".into(), Value::String(img.id.clone()));
            assets.insert(img.id, img.asset);
        }
        self.client
            .ask(Value::Object(state), assets, questions)
            .await
    }

    /// Settled, locked, keyboard, alert, sensitive data and (when given)
    /// irreversibility of `last_action`, in one batched request.
    pub async fn judge(&self, frame: &Frame, last_action: Option<&str>) -> Result<ScreenJudgment> {
        let mut state = serde_json::Map::new();
        let questions = judge_questions(last_action);
        if let Some(a) = last_action {
            state.insert("action".into(), Value::String(a.to_string()));
        }
        let answers = self.ask_screen(Some(frame), state, questions).await?;
        judgment_from_answers(&answers, last_action.is_some(), self.config.flag_threshold)
    }

    /// Probability (0..1) that `action_description` on this screen is hard
    /// or impossible to undo. For the confirm-before-irreversible gate.
    pub async fn is_irreversible(&self, frame: &Frame, action_description: &str) -> Result<f32> {
        let mut state = serde_json::Map::new();
        state.insert("action".into(), Value::String(action_description.into()));
        let mut questions = BTreeMap::new();
        questions.insert("irreversible".to_string(), irreversible_question());
        let answers = self.ask_screen(Some(frame), state, questions).await?;
        Ok(answers.noul("irreversible")?.p)
    }

    /// Pick the candidate that best serves `goal`. Fast path: when
    /// [`TargetPick::accepted`] is false, fall back to the main LLM.
    pub async fn pick_target(&self, input: PickInput<'_>, goal: &str) -> Result<TargetPick> {
        let question = pick_question(input.candidates, input.frame.is_some())?;
        let mut state = serde_json::Map::new();
        state.insert("goal".into(), Value::String(goal.into()));
        let mut questions = BTreeMap::new();
        questions.insert("target".to_string(), question);
        let answers = self.ask_screen(input.frame, state, questions).await?;
        pick_from_choice(
            answers.choice("target")?.clone(),
            input.candidates.len(),
            &self.config,
        )
    }

    /// Find `description` on the screen and return its centre.
    /// `None` when Jev thinks it is absent or the box is weak.
    #[doc = "experimental"]
    pub async fn locate(&self, frame: &Frame, description: &str) -> Result<Option<Point>> {
        let mut state = serde_json::Map::new();
        state.insert("target".into(), Value::String(description.into()));
        let answers = self
            .ask_screen(Some(frame), state, locate_questions())
            .await?;
        locate_from_answers(&answers, &self.config)
    }
}

fn irreversible_question() -> Question {
    Question::noul_with(
        "The phone agent is about to do `action` on `screen`. Is this action hard or impossible to undo?",
        "It sends, posts, pays, buys, deletes, submits, confirms, signs, calls, or changes an account or security setting.",
        "It only navigates, opens, scrolls, types into a field without submitting, or can be cancelled or undone.",
    )
}

/// Questions sent by [`ScreenJudge::judge`].
pub fn judge_questions(last_action: Option<&str>) -> BTreeMap<String, Question> {
    let mut q = BTreeMap::new();
    q.insert(
        "settled".into(),
        Question::noul_with(
            "Is `screen` fully loaded and still?",
            "Content is complete. No spinner, progress bar, loading placeholder, or half-finished transition.",
            "A spinner, progress bar, blank loading area, skeleton, or mid-animation transition is visible.",
        ),
    );
    q.insert(
        "locked".into(),
        Question::noul(
            "Does `screen` show the phone lock screen, a passcode keypad, or a Face ID / fingerprint prompt?",
        ),
    );
    q.insert(
        "keyboard_visible".into(),
        Question::noul("Is an on-screen keyboard visible in `screen`?"),
    );
    q.insert(
        "alert_visible".into(),
        Question::noul(
            "Does a dialog, alert, permission prompt, or modal sheet cover the app in `screen`?",
        ),
    );
    q.insert(
        "sensitive".into(),
        Question::noul(
            "Does `screen` show private data: passwords, card or bank details, private messages, health data, or personal photos?",
        ),
    );
    if last_action.is_some() {
        q.insert("irreversible".into(), irreversible_question());
    }
    q
}

/// Build a [`ScreenJudgment`] from the answers to [`judge_questions`].
pub fn judgment_from_answers(
    answers: &Answers,
    with_action: bool,
    threshold: f32,
) -> Result<ScreenJudgment> {
    let p = ScreenProbabilities {
        settled: answers.noul("settled")?.p,
        locked: answers.noul("locked")?.p,
        keyboard_visible: answers.noul("keyboard_visible")?.p,
        alert_visible: answers.noul("alert_visible")?.p,
        sensitive: answers.noul("sensitive")?.p,
    };
    let action_irreversible = if with_action {
        answers.noul("irreversible")?.p
    } else {
        0.0
    };
    Ok(ScreenJudgment {
        settled: p.settled >= threshold,
        locked: p.locked >= threshold,
        keyboard_visible: p.keyboard_visible >= threshold,
        alert_visible: p.alert_visible >= threshold,
        action_irreversible,
        sensitive: p.sensitive,
        probabilities: p,
        model: answers.model.clone(),
        usage: answers.usage,
    })
}

/// The Choice question for [`ScreenJudge::pick_target`].
pub fn pick_question(candidates: &[String], with_screen: bool) -> Result<Question> {
    if candidates.is_empty() {
        return Err(Error::InvalidArgument(
            "pick_target needs at least one candidate".into(),
        ));
    }
    if candidates.len() > MAX_CANDIDATES {
        return Err(Error::InvalidArgument(format!(
            "pick_target accepts at most {MAX_CANDIDATES} candidates, got {}",
            candidates.len()
        )));
    }
    let mut options: Vec<(String, Value)> = candidates
        .iter()
        .enumerate()
        .map(|(i, c)| (i.to_string(), Value::String(c.clone())))
        .collect();
    options.push((
        NONE_OPTION.into(),
        Value::String("None of the other options helps reach the goal.".into()),
    ));
    let instructions = if with_screen {
        "Which on-screen element in `screen` should the phone agent tap next to make progress on `goal`?"
    } else {
        "Which on-screen element should the phone agent tap next to make progress on `goal`?"
    };
    Ok(Question::choice(instructions, options))
}

/// Map the Choice answer to a [`TargetPick`].
pub fn pick_from_choice(choice: Choice, count: usize, cfg: &JudgeConfig) -> Result<TargetPick> {
    let index = if choice.choice == NONE_OPTION {
        None
    } else {
        let i: usize = choice.choice.parse().map_err(|_| {
            Error::protocol(format!("Jev picked unknown option '{}'", choice.choice))
        })?;
        if i >= count {
            return Err(Error::protocol(format!("Jev picked option {i} of {count}")));
        }
        Some(i)
    };
    let p = choice
        .probabilities
        .get(&choice.choice)
        .copied()
        .unwrap_or(0.0);
    let accepted =
        index.is_some() && choice.confidence >= cfg.pick_confidence && p >= cfg.pick_probability;
    Ok(TargetPick {
        choice,
        index,
        accepted,
    })
}

/// Questions sent by [`ScreenJudge::locate`].
pub fn locate_questions() -> BTreeMap<String, Question> {
    let mut q = BTreeMap::new();
    q.insert(
        "present".into(),
        Question::noul("Is `target` visible in `screen`?"),
    );
    q.insert("where".into(), Question::bbox("`target` in `screen`"));
    q
}

/// Map the answers to [`locate_questions`] to a point.
pub fn locate_from_answers(answers: &Answers, cfg: &JudgeConfig) -> Result<Option<Point>> {
    if answers.noul("present")?.p < cfg.present_threshold {
        return Ok(None);
    }
    let b = answers.bbox("where")?;
    if b.confidence.is_some_and(|c| c < cfg.box_confidence) {
        return Ok(None);
    }
    Ok(Some(b.bbox.center()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::Transport;
    use crate::privacy::PrivacyConfig;
    use crate::types::{SystemOneRequest, SystemOneResponse};
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    /// Records requests and returns a canned response.
    struct Fake {
        response: Value,
        seen: Mutex<Vec<SystemOneRequest>>,
    }

    #[async_trait]
    impl Transport for Fake {
        async fn send(&self, request: &SystemOneRequest) -> Result<SystemOneResponse> {
            self.seen.lock().unwrap().push(request.clone());
            serde_json::from_value(self.response.clone()).map_err(Error::protocol)
        }
    }

    fn judge_with(response: Value) -> (ScreenJudge, Arc<Fake>) {
        let fake = Arc::new(Fake {
            response,
            seen: Mutex::new(Vec::new()),
        });
        let client = JevClient::with_transport("jev-latest", fake.clone());
        (
            ScreenJudge::new(client, Privacy::new(PrivacyConfig::default())),
            fake,
        )
    }

    fn noul(p: f32) -> Value {
        json!({"type": "noul", "noul": p})
    }

    #[tokio::test]
    async fn judge_batches_one_request() {
        let (j, fake) = judge_with(json!({
            "model": "jev-1.14.0",
            "answers": {
                "settled": noul(0.9), "locked": noul(0.02), "keyboard_visible": noul(0.7),
                "alert_visible": noul(0.1), "sensitive": noul(0.05), "irreversible": noul(0.95)
            },
            "usage": {"input_tokens": 900, "output_tokens": 30}
        }));
        let frame = Frame::solid(1170, 2532, [255, 255, 255, 255]);

        // No consent yet: refused, nothing sent.
        let err = j.judge(&frame, Some("tap Send")).await.unwrap_err();
        assert!(matches!(err, Error::Refused(_)));
        assert!(fake.seen.lock().unwrap().is_empty());

        j.privacy().grant_consent();
        j.privacy()
            .set_foreground_app(Some("com.apple.Preferences".into()));
        let r = j.judge(&frame, Some("tap Send")).await.unwrap();
        assert!(r.settled && !r.locked && r.keyboard_visible && !r.alert_visible);
        assert!((r.action_irreversible - 0.95).abs() < 1e-6);
        assert_eq!(r.usage.input_tokens, 900);

        let seen = fake.seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 1);
        let req = &seen[0];
        assert!(req.needs_images_header());
        assert_eq!(req.questions.len(), 6);
        let id = req.state["screen"].as_str().unwrap();
        assert!(req.assets.contains_key(id));
        assert_eq!(req.state["action"], "tap Send");

        // Blocked app: refused.
        j.privacy()
            .set_foreground_app(Some("com.apple.MobileSMS".into()));
        assert!(matches!(
            j.is_irreversible(&frame, "tap Send").await,
            Err(Error::Refused(_))
        ));
    }

    #[test]
    fn judge_without_action_skips_question() {
        let q = judge_questions(None);
        assert!(!q.contains_key("irreversible"));
        let a: Answers = serde_json::from_value::<SystemOneResponse>(json!({
            "model": "m",
            "answers": {"settled": noul(0.4), "locked": noul(0.8), "keyboard_visible": noul(0.0),
                        "alert_visible": noul(0.5), "sensitive": noul(0.1)}
        }))
        .unwrap()
        .into();
        let r = judgment_from_answers(&a, false, 0.5).unwrap();
        assert!(!r.settled && r.locked && r.alert_visible);
        assert_eq!(r.action_irreversible, 0.0);
        assert!(judgment_from_answers(&a, true, 0.5).is_err());
    }

    #[tokio::test]
    async fn pick_target_text_only() {
        let (j, fake) = judge_with(json!({
            "model": "m",
            "answers": {"target": {"type": "choice", "choice": "1", "confidence": 0.9,
                "probabilities": {"0": 0.05, "1": 0.9, "none": 0.05}}}
        }));
        j.privacy().grant_consent();
        j.privacy()
            .set_foreground_app(Some("com.apple.Preferences".into()));
        let c = vec!["Cancel".to_string(), "Wi-Fi".to_string()];
        let pick = j
            .pick_target(
                PickInput {
                    frame: None,
                    candidates: &c,
                },
                "turn on Wi-Fi",
            )
            .await
            .unwrap();
        assert_eq!(pick.index, Some(1));
        assert!(pick.accepted);
        let req = &fake.seen.lock().unwrap()[0];
        assert!(req.assets.is_empty() && !req.needs_images_header());
        match &req.questions["target"] {
            Question::Choice { criteria, .. } => {
                assert_eq!(criteria.len(), 3);
                assert_eq!(criteria["1"], "Wi-Fi");
                assert!(criteria.contains_key("none"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn pick_thresholds_and_errors() {
        let cfg = JudgeConfig::default();
        let mk = |choice: &str, conf: f32, p: f32| Choice {
            choice: choice.into(),
            confidence: conf,
            probabilities: [(choice.to_string(), p)].into_iter().collect(),
        };
        assert!(
            !pick_from_choice(mk("0", 0.4, 0.5), 2, &cfg)
                .unwrap()
                .accepted
        );
        let none = pick_from_choice(mk("none", 0.99, 0.99), 2, &cfg).unwrap();
        assert!(none.index.is_none() && !none.accepted);
        assert!(pick_from_choice(mk("5", 0.9, 0.9), 2, &cfg).is_err());
        assert!(pick_from_choice(mk("x", 0.9, 0.9), 2, &cfg).is_err());
        assert!(pick_question(&[], false).is_err());
        assert!(pick_question(&vec!["a".to_string(); 255], false).is_err());
    }

    #[test]
    fn locate_logic() {
        let cfg = JudgeConfig::default();
        let mk = |present: f32, conf: Value| -> Answers {
            serde_json::from_value::<SystemOneResponse>(json!({
                "model": "m",
                "answers": {
                    "present": noul(present),
                    "where": {"type": "box", "box": {"top": 0.1, "bottom": 0.3, "left": 0.2, "right": 0.6}, "confidence": conf}
                }
            }))
            .unwrap()
            .into()
        };
        let p = locate_from_answers(&mk(0.9, json!(0.8)), &cfg)
            .unwrap()
            .unwrap();
        assert!((p.x - 0.4).abs() < 1e-6 && (p.y - 0.2).abs() < 1e-6);
        assert!(
            locate_from_answers(&mk(0.2, json!(0.9)), &cfg)
                .unwrap()
                .is_none()
        );
        assert!(
            locate_from_answers(&mk(0.9, json!(0.1)), &cfg)
                .unwrap()
                .is_none()
        );
        assert!(
            locate_from_answers(&mk(0.9, json!(null)), &cfg)
                .unwrap()
                .is_some()
        );
        assert!(matches!(locate_questions()["where"], Question::Box { .. }));
    }
}
