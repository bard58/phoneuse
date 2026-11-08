//! Wire types for `POST /v1/systemone`, as documented at
//! <https://docs.typesafe.ai/api.md> (checked 2026-11-08).

use std::collections::BTreeMap;

use ap_core::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Request body (`SystemOneRequest`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SystemOneRequest {
    pub model: String,
    /// Content that all questions refer to: a string, object or array.
    pub state: Value,
    /// Images, keyed by a reference string that appears in `state`,
    /// `instructions` or `criteria`. Omitted when empty (the API needs at
    /// least one entry when the field is present).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub assets: BTreeMap<String, Asset>,
    /// Questions, keyed by names that only code sees.
    pub questions: BTreeMap<String, Question>,
}

impl SystemOneRequest {
    /// True when the request needs the `TypeSafe-Danger-Zone-Images` header
    /// (it has an image asset or a Box question).
    pub fn needs_images_header(&self) -> bool {
        !self.assets.is_empty()
            || self
                .questions
                .values()
                .any(|q| matches!(q, Question::Box { .. }))
    }
}

/// One image asset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Asset {
    /// Always `"image"`.
    #[serde(rename = "type")]
    pub kind: String,
    pub format: ImageFormat,
    /// Standard base64 with padding, no line breaks, no `data:` prefix.
    pub data_base64: String,
}

impl Asset {
    pub fn image(format: ImageFormat, data_base64: String) -> Self {
        Self {
            kind: "image".into(),
            format,
            data_base64,
        }
    }
}

/// Image encoding of an asset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageFormat {
    Png,
    #[serde(alias = "jpg")]
    Jpeg,
}

/// Criteria for a Noul question: what counts as yes and as no.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NoulCriteria {
    #[serde(rename = "true")]
    pub yes: Value,
    #[serde(rename = "false")]
    pub no: Value,
}

/// A question, tagged by `type`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Question {
    /// Probability that a statement is true.
    Noul {
        instructions: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        criteria: Option<NoulCriteria>,
    },
    /// Pick one option. `criteria` maps option name to description (1..=255).
    Choice {
        instructions: Value,
        criteria: BTreeMap<String, Value>,
    },
    /// Rate on ordered levels. Position in `criteria` is the score (1..=10).
    Score {
        instructions: Value,
        criteria: Vec<Value>,
    },
    /// Locate something in an image. Needs the images header.
    Box { instructions: Value },
}

impl Question {
    pub fn noul(instructions: impl Into<Value>) -> Self {
        Question::Noul {
            instructions: instructions.into(),
            criteria: None,
        }
    }

    pub fn noul_with(
        instructions: impl Into<Value>,
        yes: impl Into<Value>,
        no: impl Into<Value>,
    ) -> Self {
        Question::Noul {
            instructions: instructions.into(),
            criteria: Some(NoulCriteria {
                yes: yes.into(),
                no: no.into(),
            }),
        }
    }

    pub fn choice<K: Into<String>, V: Into<Value>>(
        instructions: impl Into<Value>,
        options: impl IntoIterator<Item = (K, V)>,
    ) -> Self {
        Question::Choice {
            instructions: instructions.into(),
            criteria: options
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        }
    }

    pub fn score<V: Into<Value>>(
        instructions: impl Into<Value>,
        levels: impl IntoIterator<Item = V>,
    ) -> Self {
        Question::Score {
            instructions: instructions.into(),
            criteria: levels.into_iter().map(Into::into).collect(),
        }
    }

    pub fn bbox(instructions: impl Into<Value>) -> Self {
        Question::Box {
            instructions: instructions.into(),
        }
    }
}

/// Response body (`SystemOneResponse`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SystemOneResponse {
    /// The model that answered (can differ from the alias sent).
    pub model: String,
    pub answers: BTreeMap<String, Answer>,
    #[serde(default)]
    pub usage: Usage,
}

/// Token usage. Input tokens are billed, output tokens are free.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
}

/// Noul answer: probability (0..1) that the statement is true.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Noul {
    #[serde(rename = "noul")]
    pub p: f32,
}

/// Choice answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Choice {
    /// Option with the highest probability.
    pub choice: String,
    /// How concentrated the distribution is (0..1).
    pub confidence: f32,
    /// Probability of every option (sums to 1).
    pub probabilities: BTreeMap<String, f32>,
}

/// Score answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Score {
    /// Probability-weighted level (can be fractional).
    pub score: f32,
    pub confidence: f32,
    /// Level index (as a string) to its description.
    #[serde(default)]
    pub legend: BTreeMap<String, Value>,
    /// Level index (as a string) to probability.
    #[serde(default)]
    pub probabilities: BTreeMap<String, f32>,
}

/// Box edges as fractions (0..1) of the image, from the top-left corner.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BoxCoords {
    pub top: f32,
    pub bottom: f32,
    pub left: f32,
    pub right: f32,
}

impl BoxCoords {
    /// Centre of the box as a normalized point.
    pub fn center(&self) -> ap_core::Point {
        ap_core::Point::new(
            (self.left + self.right) / 2.0,
            (self.top + self.bottom) / 2.0,
        )
        .clamped()
    }
}

/// Box answer. A box is always returned, even when the target is absent:
/// ask a separate Noul for presence.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BoxAnswer {
    #[serde(rename = "box")]
    pub bbox: BoxCoords,
    #[serde(default)]
    pub confidence: Option<f32>,
}

/// One answer, tagged by `type` (same as its question).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Answer {
    Noul(Noul),
    Choice(Choice),
    Score(Score),
    Box(BoxAnswer),
    /// A type this crate does not know yet.
    #[serde(other)]
    Unknown,
}

/// All answers of one request, with typed accessors.
#[derive(Debug, Clone, PartialEq)]
pub struct Answers {
    pub model: String,
    pub usage: Usage,
    pub answers: BTreeMap<String, Answer>,
}

impl From<SystemOneResponse> for Answers {
    fn from(r: SystemOneResponse) -> Self {
        Self {
            model: r.model,
            usage: r.usage,
            answers: r.answers,
        }
    }
}

impl Answers {
    fn get(&self, name: &str) -> Result<&Answer> {
        self.answers
            .get(name)
            .ok_or_else(|| Error::protocol(format!("Jev answer '{name}' is missing")))
    }

    fn wrong(name: &str, want: &str) -> Error {
        Error::protocol(format!("Jev answer '{name}' is not a {want}"))
    }

    pub fn noul(&self, name: &str) -> Result<Noul> {
        match self.get(name)? {
            Answer::Noul(n) => Ok(*n),
            _ => Err(Self::wrong(name, "noul")),
        }
    }

    pub fn choice(&self, name: &str) -> Result<&Choice> {
        match self.get(name)? {
            Answer::Choice(c) => Ok(c),
            _ => Err(Self::wrong(name, "choice")),
        }
    }

    pub fn score(&self, name: &str) -> Result<&Score> {
        match self.get(name)? {
            Answer::Score(s) => Ok(s),
            _ => Err(Self::wrong(name, "score")),
        }
    }

    pub fn bbox(&self, name: &str) -> Result<BoxAnswer> {
        match self.get(name)? {
            Answer::Box(b) => Ok(*b),
            _ => Err(Self::wrong(name, "box")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_matches_docs_example() {
        let mut questions = BTreeMap::new();
        questions.insert(
            "complaint".to_string(),
            Question::noul("Is this message a complaint?"),
        );
        let req = SystemOneRequest {
            model: "jev-latest".into(),
            state: json!("Your order arrived damaged. I would like a replacement."),
            assets: BTreeMap::new(),
            questions,
        };
        assert_eq!(
            serde_json::to_value(&req).unwrap(),
            json!({
                "model": "jev-latest",
                "state": "Your order arrived damaged. I would like a replacement.",
                "questions": {
                    "complaint": {"type": "noul", "instructions": "Is this message a complaint?"}
                }
            })
        );
        assert!(!req.needs_images_header());
    }

    #[test]
    fn image_and_box_request_shape() {
        let mut req = SystemOneRequest {
            model: "jev-latest".into(),
            state: json!({"shelf": "7e2d9b41-shelf"}),
            assets: BTreeMap::new(),
            questions: BTreeMap::new(),
        };
        req.questions
            .insert("mug".into(), Question::bbox("the coffee mug in `shelf`"));
        assert!(req.needs_images_header());
        req.assets.insert(
            "7e2d9b41-shelf".into(),
            Asset::image(ImageFormat::Png, "iVBORw0KGgo=".into()),
        );
        req.questions.insert(
            "dept".into(),
            Question::choice(
                "Which team?",
                [("returns", "Exchanges"), ("billing", "Charges")],
            ),
        );
        req.questions.insert(
            "ok".into(),
            Question::noul_with("Is it ok?", "It is fine", json!(null)),
        );
        req.questions.insert(
            "urgency".into(),
            Question::score("How urgent?", ["low", "high"]),
        );
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(
            v["assets"]["7e2d9b41-shelf"],
            json!({"type": "image", "format": "png", "data_base64": "iVBORw0KGgo="})
        );
        assert_eq!(
            v["questions"]["mug"],
            json!({"type": "box", "instructions": "the coffee mug in `shelf`"})
        );
        assert_eq!(v["questions"]["dept"]["criteria"]["returns"], "Exchanges");
        assert_eq!(
            v["questions"]["ok"]["criteria"],
            json!({"true": "It is fine", "false": null})
        );
        assert_eq!(
            v["questions"]["urgency"]["criteria"],
            json!(["low", "high"])
        );
        let back: SystemOneRequest = serde_json::from_value(v).unwrap();
        assert_eq!(back, req);
    }

    #[test]
    fn parses_docs_response_examples() {
        let noul: SystemOneResponse = serde_json::from_value(json!({
            "model": "jev-latest",
            "answers": {"complaint": {"type": "noul", "noul": 0.97}},
            "usage": {"input_tokens": 110, "output_tokens": 10}
        }))
        .unwrap();
        let a = Answers::from(noul);
        assert!((a.noul("complaint").unwrap().p - 0.97).abs() < 1e-6);
        assert_eq!(a.usage.input_tokens, 110);
        assert!(a.choice("complaint").is_err());
        assert!(a.noul("missing").is_err());

        let choice: Answers = serde_json::from_value::<SystemOneResponse>(json!({
            "model": "jev-1.13.0",
            "answers": {"department": {
                "type": "choice", "choice": "returns", "confidence": 1.0,
                "probabilities": {"shipping": 0.0, "returns": 1.0, "billing": 0.0}
            }},
            "usage": {"input_tokens": 328, "output_tokens": 34}
        }))
        .unwrap()
        .into();
        let c = choice.choice("department").unwrap();
        assert_eq!(c.choice, "returns");
        assert_eq!(c.probabilities["returns"], 1.0);

        let bx: Answers = serde_json::from_value::<SystemOneResponse>(json!({
            "model": "jev-1.14.0",
            "answers": {"mug": {
                "type": "box",
                "box": {"top": 0.25, "bottom": 0.82, "left": 0.44, "right": 0.84},
                "confidence": 0.92
            }},
            "usage": {"input_tokens": 290, "output_tokens": 21}
        }))
        .unwrap()
        .into();
        let b = bx.bbox("mug").unwrap();
        assert_eq!(b.confidence, Some(0.92));
        let c = b.bbox.center();
        assert!((c.x - 0.64).abs() < 1e-6 && (c.y - 0.535).abs() < 1e-6);
    }

    #[test]
    fn parses_score_null_confidence_and_unknown() {
        let r: SystemOneResponse = serde_json::from_value(json!({
            "model": "jev-latest",
            "answers": {
                "u": {"type": "score", "score": 1.4, "confidence": 0.6,
                      "legend": {"0": "low", "1": "mid", "2": "high"},
                      "probabilities": {"0": 0.1, "1": 0.4, "2": 0.5}},
                "b": {"type": "box", "box": {"top": 0, "bottom": 1, "left": 0, "right": 1}, "confidence": null},
                "x": {"type": "future_kind", "value": 3}
            }
        }))
        .unwrap();
        let a = Answers::from(r);
        let s = a.score("u").unwrap();
        assert_eq!(s.legend["2"], "high");
        assert_eq!(s.probabilities["1"], 0.4);
        assert_eq!(a.bbox("b").unwrap().confidence, None);
        assert_eq!(a.answers["x"], Answer::Unknown);
        assert_eq!(a.usage, Usage::default());
    }
}
