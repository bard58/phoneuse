//! OpenAI-compatible `POST {base}/chat/completions` with function tools and
//! `image_url` data URIs. Works with OpenAI, OpenRouter, Ollama
//! (`http://localhost:11434/v1`) and LM Studio (`http://localhost:1234/v1`).
//!
//! Mapping from the neutral model:
//! - tool results become `role: "tool"` messages (text only); images from
//!   tool results follow in one `role: "user"` message, because most
//!   servers accept images only in user messages;
//! - thinking and opaque blocks are dropped (they belong to other providers).

use std::time::Duration;

use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use serde_json::{Map, Value, json};

use crate::http::{PostJson, RetryPolicy, build_client};
use crate::types::{ContentBlock, Message, Request, Response, Role, StopReason, Usage};
use crate::{Error, Provider, Result};

pub const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
const PROVIDER: &str = "openai";

/// Name of the output-limit field. OpenAI's own API wants
/// `max_completion_tokens`; most compatible servers want `max_tokens`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaxTokensField {
    MaxTokens,
    MaxCompletionTokens,
}

#[derive(Debug, Clone)]
pub struct OpenAiOptions {
    /// `None` for local servers without auth.
    pub api_key: Option<String>,
    pub model: String,
    /// Including `/v1`.
    pub base_url: String,
    pub timeout: Duration,
    pub connect_timeout: Duration,
    pub retry: RetryPolicy,
    /// `None` = pick by host.
    pub max_tokens_field: Option<MaxTokensField>,
    /// Extra headers (e.g. OpenRouter `HTTP-Referer`, `X-Title`).
    pub extra_headers: Vec<(String, String)>,
}

impl OpenAiOptions {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            api_key: None,
            model: model.into(),
            base_url: DEFAULT_BASE_URL.into(),
            timeout: Duration::from_secs(300),
            connect_timeout: Duration::from_secs(15),
            retry: RetryPolicy::default(),
            max_tokens_field: None,
            extra_headers: Vec::new(),
        }
    }
}

/// True for loopback hosts (local servers often need no key).
pub fn is_local(base_url: &str) -> bool {
    let rest = base_url
        .split_once("://")
        .map(|(_, r)| r)
        .unwrap_or(base_url);
    let host = rest.split(['/', '?']).next().unwrap_or_default();
    let host = if let Some(h) = host.strip_prefix('[') {
        h.split(']').next().unwrap_or_default()
    } else {
        host.rsplit_once(':').map(|(h, _)| h).unwrap_or(host)
    };
    matches!(host, "localhost" | "127.0.0.1" | "::1" | "0.0.0.0") || host.ends_with(".localhost")
}

pub struct OpenAiCompatibleProvider {
    opts: OpenAiOptions,
    client: reqwest::Client,
    url: String,
    headers: HeaderMap,
}

impl std::fmt::Debug for OpenAiCompatibleProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiCompatibleProvider")
            .field("model", &self.opts.model)
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

impl OpenAiCompatibleProvider {
    pub fn new(opts: OpenAiOptions) -> Result<Self> {
        if opts.model.trim().is_empty() {
            return Err(Error::Config("model is empty".into()));
        }
        let client = build_client(opts.timeout, opts.connect_timeout)?;
        let url = format!("{}/chat/completions", opts.base_url.trim_end_matches('/'));
        let mut headers = HeaderMap::new();
        if let Some(k) = opts.api_key.as_deref().filter(|k| !k.trim().is_empty()) {
            let mut v = HeaderValue::from_str(&format!("Bearer {}", k.trim()))
                .map_err(|_| Error::Config("API key has invalid characters".into()))?;
            v.set_sensitive(true);
            headers.insert(AUTHORIZATION, v);
        }
        for (k, v) in &opts.extra_headers {
            let name = reqwest::header::HeaderName::from_bytes(k.as_bytes())
                .map_err(|_| Error::Config(format!("bad header name {k}")))?;
            let val = HeaderValue::from_str(v)
                .map_err(|_| Error::Config(format!("bad header value for {k}")))?;
            headers.insert(name, val);
        }
        Ok(Self {
            opts,
            client,
            url,
            headers,
        })
    }

    fn max_tokens_field(&self) -> MaxTokensField {
        self.opts.max_tokens_field.unwrap_or_else(|| {
            if self.opts.base_url.contains("api.openai.com") {
                MaxTokensField::MaxCompletionTokens
            } else {
                MaxTokensField::MaxTokens
            }
        })
    }

    /// The JSON body for a request (public for tests and debugging).
    pub fn build_body(&self, req: &Request) -> Result<Value> {
        if req.messages.is_empty() {
            return Err(Error::InvalidRequest("no messages".into()));
        }
        let mut msgs = Vec::new();
        if !req.system.is_empty() {
            msgs.push(json!({"role": "system", "content": req.system}));
        }
        for m in &req.messages {
            messages_to_wire(m, &mut msgs);
        }
        let mut body = Map::new();
        body.insert(
            "model".into(),
            json!(req.model.as_deref().unwrap_or(&self.opts.model)),
        );
        body.insert("messages".into(), Value::Array(msgs));
        let field = match self.max_tokens_field() {
            MaxTokensField::MaxTokens => "max_tokens",
            MaxTokensField::MaxCompletionTokens => "max_completion_tokens",
        };
        body.insert(field.into(), json!(req.max_tokens));
        if !req.tools.is_empty() {
            let tools: Vec<Value> = req
                .tools
                .iter()
                .map(|t| {
                    json!({"type": "function", "function": {
                        "name": t.name, "description": t.description, "parameters": t.input_schema,
                    }})
                })
                .collect();
            body.insert("tools".into(), Value::Array(tools));
            if !req.parallel_tool_calls {
                body.insert("parallel_tool_calls".into(), json!(false));
            }
        }
        Ok(Value::Object(body))
    }
}

fn data_uri(media_type: &str, data: &str) -> String {
    format!("data:{media_type};base64,{data}")
}

fn image_part(media_type: &str, data: &str) -> Value {
    json!({"type": "image_url", "image_url": {"url": data_uri(media_type, data)}})
}

fn messages_to_wire(m: &Message, out: &mut Vec<Value>) {
    match m.role {
        Role::Assistant => {
            let text: Vec<&str> = m
                .content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } if !text.is_empty() => Some(text.as_str()),
                    _ => None,
                })
                .collect();
            let calls: Vec<Value> = m
                .content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::ToolUse { id, name, input } => Some(json!({
                        "id": id, "type": "function",
                        "function": {"name": name, "arguments": input.to_string()},
                    })),
                    _ => None,
                })
                .collect();
            let mut v = json!({"role": "assistant"});
            v["content"] = if text.is_empty() {
                Value::Null
            } else {
                json!(text.join("\n"))
            };
            if !calls.is_empty() {
                v["tool_calls"] = Value::Array(calls);
            }
            out.push(v);
        }
        Role::User => {
            let mut parts = Vec::new();
            let mut result_images = Vec::new();
            for b in &m.content {
                match b {
                    ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } => {
                        let mut text: Vec<String> = Vec::new();
                        if *is_error {
                            text.push("error:".into());
                        }
                        for c in content {
                            match c {
                                ContentBlock::Text { text: t } => text.push(t.clone()),
                                ContentBlock::Image { media_type, data } => {
                                    result_images.push(image_part(media_type.as_str(), data));
                                    text.push("[screen image in the next user message]".into());
                                }
                                _ => {}
                            }
                        }
                        out.push(json!({
                            "role": "tool", "tool_call_id": tool_use_id, "content": text.join("\n"),
                        }));
                    }
                    ContentBlock::Text { text } if !text.is_empty() => {
                        parts.push(json!({"type": "text", "text": text}));
                    }
                    ContentBlock::Image { media_type, data } => {
                        parts.push(image_part(media_type.as_str(), data));
                    }
                    _ => {}
                }
            }
            if !result_images.is_empty() {
                let mut all = vec![json!({"type": "text", "text": "Screen after the tool call:"})];
                all.extend(result_images);
                all.extend(parts);
                parts = all;
            }
            if !parts.is_empty() {
                out.push(json!({"role": "user", "content": parts}));
            }
        }
    }
}

pub(crate) fn parse_response(body: &str) -> Result<Response> {
    let v: Value = serde_json::from_str(body).map_err(|e| Error::Decode(e.to_string()))?;
    let choice = v
        .get("choices")
        .and_then(|c| c.get(0))
        .ok_or_else(|| Error::Decode("response has no choices".into()))?;
    let msg = choice.get("message").cloned().unwrap_or_default();
    let mut content = Vec::new();
    match msg.get("content") {
        Some(Value::String(s)) if !s.is_empty() => content.push(ContentBlock::text(s.clone())),
        Some(Value::Array(parts)) => {
            for p in parts {
                if let Some(t) = p.get("text").and_then(Value::as_str) {
                    content.push(ContentBlock::text(t));
                }
            }
        }
        _ => {}
    }
    if let Some(Value::Array(calls)) = msg.get("tool_calls") {
        for (i, c) in calls.iter().enumerate() {
            let f = c.get("function").cloned().unwrap_or_default();
            let args = f.get("arguments");
            let input = match args {
                Some(Value::String(s)) if s.trim().is_empty() => json!({}),
                Some(Value::String(s)) => {
                    serde_json::from_str(s).unwrap_or_else(|_| json!({"_invalid_json": s}))
                }
                Some(o @ Value::Object(_)) => o.clone(), // Ollama sometimes sends an object
                _ => json!({}),
            };
            content.push(ContentBlock::ToolUse {
                id: c
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("call_{i}")),
                name: f
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                input,
            });
        }
    }
    let has_tools = content
        .iter()
        .any(|b| matches!(b, ContentBlock::ToolUse { .. }));
    let finish = choice.get("finish_reason").and_then(Value::as_str);
    let stop_reason = match finish {
        Some("tool_calls") | Some("function_call") => StopReason::ToolUse,
        Some("length") => StopReason::MaxTokens,
        Some("content_filter") => StopReason::Refusal,
        _ if has_tools => StopReason::ToolUse,
        Some("stop") | None => StopReason::EndTurn,
        Some(o) => StopReason::Other(o.to_string()),
    };
    let u = v.get("usage").cloned().unwrap_or_default();
    let prompt = u.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0);
    let cached = u
        .get("prompt_tokens_details")
        .and_then(|d| d.get("cached_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .min(prompt);
    let usage = Usage {
        input_tokens: prompt - cached,
        output_tokens: u
            .get("completion_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: cached,
    };
    let refusal = msg
        .get("refusal")
        .and_then(Value::as_str)
        .map(str::to_string);
    Ok(Response {
        id: v
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .into(),
        model: v
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .into(),
        content,
        stop_reason: if refusal.is_some() && !has_tools {
            StopReason::Refusal
        } else {
            stop_reason
        },
        usage,
        stop_details: refusal,
    })
}

fn parse_error(status: u16, body: &str) -> (String, String) {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let e = v.get("error");
    let kind = e
        .and_then(|e| e.get("type").or_else(|| e.get("code")))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("http_{status}"));
    let message = e
        .and_then(|e| e.get("message"))
        .and_then(Value::as_str)
        .or_else(|| e.and_then(Value::as_str)) // Ollama: {"error": "text"}
        .map(str::to_string)
        .unwrap_or_else(|| body.chars().take(300).collect());
    (kind, message)
}

#[async_trait::async_trait]
impl Provider for OpenAiCompatibleProvider {
    fn name(&self) -> &str {
        PROVIDER
    }

    fn default_model(&self) -> &str {
        &self.opts.model
    }

    async fn complete(&self, request: Request) -> Result<Response> {
        let body = self.build_body(&request)?;
        let raw = PostJson {
            client: &self.client,
            url: &self.url,
            headers: &self.headers,
            body: &body,
            policy: &self.opts.retry,
            timeout: self.opts.timeout,
            provider: PROVIDER,
            parse_error,
        }
        .send()
        .await?;
        parse_response(&raw.body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_server::{Reply, TestServer};
    use crate::types::{ImageMediaType, ToolDef};

    fn req() -> Request {
        let mut r = Request::new(
            "sys",
            vec![
                Message::user(vec![
                    ContentBlock::text("go"),
                    ContentBlock::image(ImageMediaType::Jpeg, b"abc"),
                ]),
                Message::assistant(vec![
                    ContentBlock::Thinking {
                        thinking: "t".into(),
                        signature: None,
                    },
                    ContentBlock::ToolUse {
                        id: "call_1".into(),
                        name: "tap".into(),
                        input: json!({"x": 0.5}),
                    },
                ]),
                Message::user(vec![ContentBlock::tool_result(
                    "call_1",
                    vec![
                        ContentBlock::text("ok"),
                        ContentBlock::image(ImageMediaType::Jpeg, b"img"),
                    ],
                    false,
                )]),
            ],
        );
        r.tools = vec![ToolDef {
            name: "tap".into(),
            description: "Tap".into(),
            input_schema: json!({"type": "object"}),
        }];
        r.parallel_tool_calls = false;
        r
    }

    #[test]
    fn body_shape() {
        let mut o = OpenAiOptions::new("gpt-test");
        o.api_key = Some("k".into());
        let p = OpenAiCompatibleProvider::new(o).unwrap();
        let b = p.build_body(&req()).unwrap();
        assert_eq!(b["max_completion_tokens"], 16000);
        assert_eq!(b["parallel_tool_calls"], false);
        assert_eq!(b["tools"][0]["function"]["name"], "tap");
        let m = b["messages"].as_array().unwrap();
        assert_eq!(m[0]["role"], "system");
        assert_eq!(
            m[1]["content"][1]["image_url"]["url"],
            "data:image/jpeg;base64,YWJj"
        );
        assert_eq!(m[2]["content"], Value::Null);
        assert_eq!(
            m[2]["tool_calls"][0]["function"]["arguments"],
            r#"{"x":0.5}"#
        );
        assert_eq!(m[3]["role"], "tool");
        assert_eq!(m[3]["tool_call_id"], "call_1");
        assert_eq!(m[4]["role"], "user");
        assert_eq!(m[4]["content"][1]["type"], "image_url");
        assert_eq!(m.len(), 5);
    }

    #[test]
    fn local_hosts() {
        assert!(is_local("http://localhost:11434/v1"));
        assert!(is_local("http://127.0.0.1:1234/v1"));
        assert!(is_local("http://[::1]:8080/v1"));
        assert!(!is_local("https://openrouter.ai/api/v1"));
    }

    #[test]
    fn parse_tool_calls() {
        let r = parse_response(
            r#"{"id":"c1","model":"m","choices":[{"index":0,"finish_reason":"tool_calls",
            "message":{"role":"assistant","content":null,"tool_calls":[
              {"id":"call_a","type":"function","function":{"name":"tap","arguments":"{\"x\":0.2,\"y\":0.3}"}},
              {"type":"function","function":{"name":"done","arguments":"not json"}}]}}],
            "usage":{"prompt_tokens":100,"completion_tokens":7,"prompt_tokens_details":{"cached_tokens":40}}}"#,
        )
        .unwrap();
        assert_eq!(r.stop_reason, StopReason::ToolUse);
        let t = r.tool_uses();
        assert_eq!(t[0], ("call_a", "tap", &json!({"x": 0.2, "y": 0.3})));
        assert_eq!(t[1].0, "call_1");
        assert!(t[1].2.get("_invalid_json").is_some());
        assert_eq!(r.usage.input_tokens, 60);
        assert_eq!(r.usage.cache_read_input_tokens, 40);
    }

    #[tokio::test]
    async fn round_trip_against_local_server() {
        let srv = TestServer::start(vec![
            Reply::json(503, r#"{"error":"loading model"}"#),
            Reply::json(
                200,
                r#"{"id":"x","model":"llava","choices":[{"finish_reason":"stop","message":{"content":"hello"}}]}"#,
            ),
        ])
        .await;
        let mut o = OpenAiOptions::new("llava");
        o.base_url = format!("{}/v1", srv.url());
        o.retry = RetryPolicy {
            max_retries: 1,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(2),
        };
        let p = OpenAiCompatibleProvider::new(o).unwrap();
        let r = p
            .complete(Request::new("s", vec![Message::user_text("hi")]))
            .await
            .unwrap();
        assert_eq!(r.text(), "hello");
        assert_eq!(r.stop_reason, StopReason::EndTurn);
        let reqs = srv.requests();
        assert_eq!(reqs[1].path, "/v1/chat/completions");
        assert_eq!(reqs[1].header("authorization"), None);
        let body: Value = serde_json::from_str(&reqs[1].body).unwrap();
        assert_eq!(body["max_tokens"], 16000);
    }
}
