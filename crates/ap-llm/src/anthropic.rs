//! Claude Messages API (`POST /v1/messages`), raw HTTP over reqwest.
//!
//! Rust has no official Anthropic SDK, so this follows the documented REST
//! shape (claude-api skill, cURL reference):
//! - headers `x-api-key`, `anthropic-version: 2026-10-21`;
//! - prompt caching: a `cache_control` breakpoint on the system prompt (tools
//!   render before system, so it caches both) plus top-level automatic
//!   caching for the growing conversation;
//! - no `temperature`/`top_p`/`top_k` (rejected on API version 2026-10-21);
//! - no `thinking` field: Claude Opus 5.5 / Sonnet 5.5 / Haiku 5.5 run adaptive
//!   thinking by default; depth is set with `output_config.effort`;
//! - `tool_choice` is only ever `auto` (forced tool use is a 400 on these models);
//! - thinking blocks are echoed back unchanged (append-only history);
//! - server-side refusal fallbacks (`fallbacks: "default"`) on by default for
//!   models that support them on the Claude API.

use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::{Map, Value, json};

use crate::http::{PostJson, RetryPolicy, build_client};
use crate::types::{ContentBlock, Message, Request, Response, Role, StopReason, Usage};
use crate::{Error, Provider, Result};

pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
/// API version sent in `anthropic-version`.
pub const API_VERSION: &str = "2026-10-21";
/// Beta header for `fallbacks: "default"`.
pub const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";

const PROVIDER: &str = "anthropic";

/// Server-side refusal fallback setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Fallbacks {
    /// `fallbacks: "default"` when the model supports it and the base URL is the Claude API.
    #[default]
    Auto,
    Off,
}

#[derive(Debug, Clone)]
pub struct AnthropicOptions {
    pub api_key: String,
    /// Default model (`claude-opus-5-5`).
    pub model: String,
    /// Without `/v1` (e.g. `https://api.anthropic.com`).
    pub base_url: String,
    pub api_version: String,
    /// Whole-request timeout (non-streaming; thinking can take a while).
    pub timeout: Duration,
    pub connect_timeout: Duration,
    pub retry: RetryPolicy,
    pub fallbacks: Fallbacks,
    /// Extra `anthropic-beta` values.
    pub betas: Vec<String>,
}

impl AnthropicOptions {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            model: crate::models::CLAUDE_OPUS_5_5.into(),
            base_url: DEFAULT_BASE_URL.into(),
            api_version: API_VERSION.into(),
            timeout: Duration::from_secs(300),
            connect_timeout: Duration::from_secs(15),
            retry: RetryPolicy::default(),
            fallbacks: Fallbacks::Auto,
            betas: Vec::new(),
        }
    }
}

pub struct AnthropicProvider {
    opts: AnthropicOptions,
    client: reqwest::Client,
    url: String,
}

impl std::fmt::Debug for AnthropicProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnthropicProvider")
            .field("model", &self.opts.model)
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

impl AnthropicProvider {
    pub fn new(opts: AnthropicOptions) -> Result<Self> {
        if opts.api_key.trim().is_empty() {
            return Err(Error::Config("Anthropic API key is empty".into()));
        }
        if !crate::models::is_known(&opts.model) {
            tracing::warn!(model = %opts.model, "model id is not one of {:?}", crate::models::KNOWN);
        }
        let client = build_client(opts.timeout, opts.connect_timeout)?;
        let base = opts.base_url.trim_end_matches('/');
        let base = base.strip_suffix("/v1").unwrap_or(base);
        let url = format!("{base}/v1/messages");
        Ok(Self { opts, client, url })
    }

    fn fallbacks_for(&self, model: &str) -> bool {
        self.opts.fallbacks == Fallbacks::Auto
            && self.url.starts_with(DEFAULT_BASE_URL)
            && matches!(
                model,
                "claude-opus-5-5" | "claude-sonnet-5-5" | "claude-opus-5" | "claude-fable-5-1"
            )
    }

    fn headers(&self, fallbacks: bool) -> Result<HeaderMap> {
        let mut h = HeaderMap::new();
        let key = HeaderValue::from_str(self.opts.api_key.trim())
            .map_err(|_| Error::Config("API key has invalid characters".into()))?;
        let mut key = key;
        key.set_sensitive(true);
        h.insert(HeaderName::from_static("x-api-key"), key);
        h.insert(
            HeaderName::from_static("anthropic-version"),
            HeaderValue::from_str(&self.opts.api_version)
                .map_err(|_| Error::Config("bad api_version".into()))?,
        );
        let mut betas: Vec<&str> = self.opts.betas.iter().map(String::as_str).collect();
        if fallbacks && !betas.contains(&FALLBACK_BETA) {
            betas.push(FALLBACK_BETA);
        }
        if !betas.is_empty() {
            h.insert(
                HeaderName::from_static("anthropic-beta"),
                HeaderValue::from_str(&betas.join(","))
                    .map_err(|_| Error::Config("bad beta name".into()))?,
            );
        }
        Ok(h)
    }

    /// The JSON body for a request (public for tests and debugging).
    pub fn build_body(&self, req: &Request) -> Result<Value> {
        if req.messages.is_empty() {
            return Err(Error::InvalidRequest("no messages".into()));
        }
        let model = req.model.as_deref().unwrap_or(&self.opts.model);
        let mut body = Map::new();
        body.insert("model".into(), json!(model));
        body.insert("max_tokens".into(), json!(req.max_tokens));
        let ephemeral = json!({"type": "ephemeral"});

        let mut tools: Vec<Value> = req
            .tools
            .iter()
            .map(|t| {
                json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": t.input_schema,
                })
            })
            .collect();
        if !req.system.is_empty() {
            let mut sys = json!({"type": "text", "text": req.system});
            if req.cache {
                sys["cache_control"] = ephemeral.clone();
            }
            body.insert("system".into(), json!([sys]));
        } else if req.cache
            && let Some(last) = tools.last_mut()
        {
            last["cache_control"] = ephemeral.clone();
        }
        if !tools.is_empty() {
            body.insert("tools".into(), Value::Array(tools));
            if !req.parallel_tool_calls {
                body.insert(
                    "tool_choice".into(),
                    json!({"type": "auto", "disable_parallel_tool_use": true}),
                );
            }
        }
        body.insert(
            "messages".into(),
            Value::Array(req.messages.iter().map(message_to_wire).collect()),
        );
        if let Some(e) = req.effort {
            body.insert("output_config".into(), json!({"effort": e.as_str()}));
        }
        if req.cache {
            // Automatic breakpoint on the last cacheable block: the conversation tail.
            body.insert("cache_control".into(), ephemeral);
        }
        if self.fallbacks_for(model) {
            body.insert("fallbacks".into(), json!("default"));
        }
        Ok(Value::Object(body))
    }
}

fn message_to_wire(m: &Message) -> Value {
    let role = match m.role {
        Role::User => "user",
        Role::Assistant => "assistant",
    };
    let content: Vec<Value> = m.content.iter().filter_map(block_to_wire).collect();
    json!({"role": role, "content": content})
}

fn block_to_wire(b: &ContentBlock) -> Option<Value> {
    Some(match b {
        ContentBlock::Text { text } => {
            if text.is_empty() {
                return None; // the API rejects empty text blocks
            }
            json!({"type": "text", "text": text})
        }
        ContentBlock::Image { media_type, data } => json!({
            "type": "image",
            "source": {"type": "base64", "media_type": media_type.as_str(), "data": data},
        }),
        ContentBlock::ToolUse { id, name, input } => {
            json!({"type": "tool_use", "id": id, "name": name, "input": input})
        }
        ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => {
            let inner: Vec<Value> = content.iter().filter_map(block_to_wire).collect();
            let mut v =
                json!({"type": "tool_result", "tool_use_id": tool_use_id, "content": inner});
            if *is_error {
                v["is_error"] = json!(true);
            }
            v
        }
        ContentBlock::Thinking {
            thinking,
            signature,
        } => {
            let mut v = json!({"type": "thinking", "thinking": thinking});
            if let Some(s) = signature {
                v["signature"] = json!(s);
            }
            v
        }
        ContentBlock::Opaque { provider, value } => {
            if provider != PROVIDER {
                return None;
            }
            value.clone()
        }
    })
}

fn block_from_wire(v: Value) -> ContentBlock {
    let ty = v.get("type").and_then(Value::as_str).unwrap_or_default();
    match ty {
        "text" => ContentBlock::Text {
            text: v
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        },
        "tool_use" => ContentBlock::ToolUse {
            id: str_field(&v, "id"),
            name: str_field(&v, "name"),
            input: v.get("input").cloned().unwrap_or_else(|| json!({})),
        },
        "thinking" => ContentBlock::Thinking {
            thinking: str_field(&v, "thinking"),
            signature: v
                .get("signature")
                .and_then(Value::as_str)
                .map(str::to_string),
        },
        _ => ContentBlock::Opaque {
            provider: PROVIDER.into(),
            value: v,
        },
    }
}

fn str_field(v: &Value, k: &str) -> String {
    v.get(k)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn is_fallback(b: &ContentBlock) -> bool {
    matches!(b, ContentBlock::Opaque { value, .. }
        if value.get("type").and_then(Value::as_str) == Some("fallback"))
}

/// After a mid-output fallback, blocks before the last `fallback` marker came
/// from the model that declined: keep only their text (skill: "Echoing
/// fallback turns back"). The marker itself is dropped.
fn sanitize_fallback(content: Vec<ContentBlock>) -> Vec<ContentBlock> {
    let Some(last) = content.iter().rposition(is_fallback) else {
        return content;
    };
    content
        .into_iter()
        .enumerate()
        .filter(|(i, b)| *i > last || (*i < last && matches!(b, ContentBlock::Text { .. })))
        .map(|(_, b)| b)
        .collect()
}

pub(crate) fn parse_response(body: &str) -> Result<Response> {
    let v: Value = serde_json::from_str(body).map_err(|e| Error::Decode(e.to_string()))?;
    let content = match v.get("content") {
        Some(Value::Array(a)) => a.iter().cloned().map(block_from_wire).collect(),
        _ => return Err(Error::Decode("response has no content array".into())),
    };
    let u = v.get("usage").cloned().unwrap_or_default();
    let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
    let usage = Usage {
        input_tokens: n("input_tokens"),
        output_tokens: n("output_tokens"),
        cache_creation_input_tokens: n("cache_creation_input_tokens"),
        cache_read_input_tokens: n("cache_read_input_tokens"),
    };
    let stop_details = v.get("stop_details").filter(|d| !d.is_null()).map(|d| {
        let cat = d
            .get("category")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        match d.get("explanation").and_then(Value::as_str) {
            Some(e) => format!("{cat}: {e}"),
            None => cat.to_string(),
        }
    });
    Ok(Response {
        id: str_field(&v, "id"),
        model: str_field(&v, "model"),
        content: sanitize_fallback(content),
        stop_reason: StopReason::from_anthropic(v.get("stop_reason").and_then(Value::as_str)),
        usage,
        stop_details,
    })
}

pub(crate) fn parse_error(status: u16, body: &str) -> (String, String) {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let e = v.get("error");
    let kind = e
        .and_then(|e| e.get("type"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("http_{status}"));
    let message = e
        .and_then(|e| e.get("message"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| body.chars().take(300).collect());
    (kind, message)
}

#[async_trait::async_trait]
impl Provider for AnthropicProvider {
    fn name(&self) -> &str {
        PROVIDER
    }

    fn default_model(&self) -> &str {
        &self.opts.model
    }

    async fn complete(&self, request: Request) -> Result<Response> {
        let body = self.build_body(&request)?;
        let model = body["model"].as_str().unwrap_or_default().to_string();
        let headers = self.headers(self.fallbacks_for(&model))?;
        let raw = PostJson {
            client: &self.client,
            url: &self.url,
            headers: &headers,
            body: &body,
            policy: &self.opts.retry,
            timeout: self.opts.timeout,
            provider: PROVIDER,
            parse_error,
        }
        .send()
        .await?;
        let resp = parse_response(&raw.body)?;
        tracing::debug!(
            model = %resp.model,
            stop = ?resp.stop_reason,
            input = resp.usage.input_tokens,
            output = resp.usage.output_tokens,
            cache_read = resp.usage.cache_read_input_tokens,
            cache_write = resp.usage.cache_creation_input_tokens,
            "anthropic response"
        );
        Ok(resp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_server::{Reply, TestServer};
    use crate::types::{Effort, ImageMediaType, ToolDef};

    fn provider(base: &str) -> AnthropicProvider {
        let mut o = AnthropicOptions::new("sk-test");
        o.base_url = base.into();
        o.retry = RetryPolicy {
            max_retries: 2,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
        };
        AnthropicProvider::new(o).unwrap()
    }

    fn sample_request() -> Request {
        let mut r = Request::new(
            "You drive a phone.",
            vec![
                Message::user(vec![
                    ContentBlock::text("Open Settings"),
                    ContentBlock::image(ImageMediaType::Jpeg, b"jpegbytes"),
                ]),
                Message::assistant(vec![
                    ContentBlock::Thinking {
                        thinking: String::new(),
                        signature: Some("sig1".into()),
                    },
                    ContentBlock::text(""),
                    ContentBlock::ToolUse {
                        id: "toolu_1".into(),
                        name: "tap".into(),
                        input: json!({"x": 0.5, "y": 0.5}),
                    },
                ]),
                Message::user(vec![ContentBlock::tool_result(
                    "toolu_1",
                    vec![
                        ContentBlock::text("ok"),
                        ContentBlock::image(ImageMediaType::Png, b"png"),
                    ],
                    false,
                )]),
            ],
        );
        r.tools = vec![ToolDef {
            name: "tap".into(),
            description: "Tap".into(),
            input_schema: json!({"type": "object", "properties": {}}),
        }];
        r.effort = Some(Effort::Medium);
        r.parallel_tool_calls = false;
        r.max_tokens = 4096;
        r
    }

    #[test]
    fn body_shape() {
        let p = provider(DEFAULT_BASE_URL);
        let b = p.build_body(&sample_request()).unwrap();
        assert_eq!(b["model"], "claude-opus-5-5");
        assert_eq!(b["max_tokens"], 4096);
        assert_eq!(b["system"][0]["cache_control"]["type"], "ephemeral");
        assert_eq!(b["cache_control"]["type"], "ephemeral");
        assert_eq!(b["output_config"]["effort"], "medium");
        assert_eq!(b["tool_choice"]["type"], "auto");
        assert_eq!(b["tool_choice"]["disable_parallel_tool_use"], true);
        assert_eq!(b["fallbacks"], "default");
        assert!(b.get("thinking").is_none() && b.get("temperature").is_none());
        let m = &b["messages"];
        assert_eq!(m[0]["content"][1]["source"]["media_type"], "image/jpeg");
        assert_eq!(m[0]["content"][1]["source"]["data"], "anBlZ2J5dGVz");
        // thinking echoed, empty text dropped
        assert_eq!(m[1]["content"][0]["type"], "thinking");
        assert_eq!(m[1]["content"][0]["signature"], "sig1");
        assert_eq!(m[1]["content"].as_array().unwrap().len(), 2);
        let tr = &m[2]["content"][0];
        assert_eq!(tr["type"], "tool_result");
        assert!(tr.get("is_error").is_none());
        assert_eq!(tr["content"][1]["type"], "image");
    }

    #[test]
    fn no_fallbacks_for_haiku_or_proxy() {
        let p = provider("http://127.0.0.1:1");
        let b = p.build_body(&sample_request()).unwrap();
        assert!(b.get("fallbacks").is_none());
        let p = provider(DEFAULT_BASE_URL);
        let mut r = sample_request();
        r.model = Some("claude-haiku-5-5".into());
        assert!(p.build_body(&r).unwrap().get("fallbacks").is_none());
    }

    #[test]
    fn parse_tool_use_and_usage() {
        let r = parse_response(
            r#"{"id":"msg_1","type":"message","role":"assistant","model":"claude-opus-5-5",
            "content":[{"type":"thinking","thinking":"","signature":"abc"},
                       {"type":"text","text":"Tapping."},
                       {"type":"tool_use","id":"toolu_9","name":"tap","input":{"x":0.1,"y":0.2}}],
            "stop_reason":"tool_use","stop_details":null,
            "usage":{"input_tokens":10,"output_tokens":20,"cache_creation_input_tokens":300,"cache_read_input_tokens":4000}}"#,
        )
        .unwrap();
        assert_eq!(r.stop_reason, StopReason::ToolUse);
        assert_eq!(r.usage.cache_read_input_tokens, 4000);
        assert_eq!(r.usage.total_input(), 4310);
        assert_eq!(r.tool_uses()[0].1, "tap");
        assert_eq!(r.text(), "Tapping.");
        assert!(r.stop_details.is_none());
    }

    #[test]
    fn fallback_blocks_are_sanitized() {
        let r = parse_response(
            r#"{"id":"m","model":"claude-opus-4-8","stop_reason":"end_turn","usage":{},
            "content":[{"type":"thinking","thinking":"","signature":"x"},
                       {"type":"text","text":"partial"},
                       {"type":"tool_use","id":"t0","name":"tap","input":{}},
                       {"type":"fallback","from":{"model":"claude-opus-5-5"},"to":{"model":"claude-opus-4-8"}},
                       {"type":"text","text":"rest"}]}"#,
        )
        .unwrap();
        assert_eq!(r.content.len(), 2);
        assert_eq!(r.text(), "partial\nrest");
    }

    #[test]
    fn refusal_details() {
        let r = parse_response(
            r#"{"id":"m","model":"x","content":[],"stop_reason":"refusal",
            "stop_details":{"type":"refusal","category":"cyber","explanation":"no"},"usage":{}}"#,
        )
        .unwrap();
        assert_eq!(r.stop_reason, StopReason::Refusal);
        assert_eq!(r.stop_details.as_deref(), Some("cyber: no"));
    }

    #[tokio::test]
    async fn retries_overloaded_then_succeeds() {
        let ok = r#"{"id":"msg_2","model":"claude-opus-5-5","content":[{"type":"text","text":"hi"}],
                     "stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":2}}"#;
        let srv = TestServer::start(vec![
            Reply::json(
                529,
                r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
            )
            .header("retry-after", "0"),
            Reply::json(200, ok),
        ])
        .await;
        let p = provider(&srv.url());
        let r = p
            .complete(Request::new("sys", vec![Message::user_text("hello")]))
            .await
            .unwrap();
        assert_eq!(r.text(), "hi");
        let reqs = srv.requests();
        assert_eq!(reqs.len(), 2);
        assert_eq!(reqs[0].path, "/v1/messages");
        assert_eq!(reqs[0].header("x-api-key"), Some("sk-test"));
        assert_eq!(reqs[0].header("anthropic-version"), Some(API_VERSION));
        // Not the Claude API host, so no fallback beta.
        assert_eq!(reqs[0].header("anthropic-beta"), None);
        let body: Value = serde_json::from_str(&reqs[1].body).unwrap();
        assert_eq!(body["messages"][0]["content"][0]["text"], "hello");
    }

    #[tokio::test]
    async fn bad_request_is_not_retried() {
        let srv = TestServer::start(vec![
            Reply::json(
                400,
                r#"{"type":"error","error":{"type":"invalid_request_error","message":"bad"}}"#,
            )
            .header("request-id", "req_1"),
        ])
        .await;
        let p = provider(&srv.url());
        let e = p
            .complete(Request::new("", vec![Message::user_text("x")]))
            .await
            .unwrap_err();
        assert!(!e.is_retryable());
        assert_eq!(e.status(), Some(400));
        match e {
            Error::Api {
                kind, request_id, ..
            } => {
                assert_eq!(kind, "invalid_request_error");
                assert_eq!(request_id.as_deref(), Some("req_1"));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(srv.requests().len(), 1);
    }

    #[tokio::test]
    async fn gives_up_after_max_retries() {
        let srv = TestServer::start(vec![
            Reply::json(500, "{}"),
            Reply::json(503, "{}"),
            Reply::json(
                429,
                r#"{"error":{"type":"rate_limit_error","message":"slow"}}"#,
            ),
        ])
        .await;
        let p = provider(&srv.url());
        let e = p
            .complete(Request::new("", vec![Message::user_text("x")]))
            .await
            .unwrap_err();
        assert_eq!(e.status(), Some(429));
        assert_eq!(srv.requests().len(), 3);
    }

    #[test]
    fn empty_key_rejected() {
        assert!(AnthropicProvider::new(AnthropicOptions::new(" ")).is_err());
    }
}
