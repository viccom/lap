//! Protocol adapter between lap and the vision-language gateways.
//!
//! Two request/response dialects are supported: Anthropic Messages and
//! OpenAI Chat Completions. Everything here is pure: the request bodies,
//! the endpoint joining and the response parsers have no I/O, so the
//! behavior is fully covered by unit tests. The HTTP call sites in
//! `t_vl.rs` / `t_embed.rs` compose these pieces.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

// ----------------------------------------------------------------------------
// Protocol
// ----------------------------------------------------------------------------

/// The request/response dialect the gateway speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VlProtocol {
    /// Anthropic Messages: `/v1/messages`, `x-api-key` auth, top-level
    /// `system`, base64 image `source` blocks.
    #[default]
    Anthropic,
    /// OpenAI Chat Completions: `/v1/chat/completions`, `Bearer` auth,
    /// system role message, `image_url` data URLs.
    OpenAi,
}

impl VlProtocol {
    /// Parse a stored setting. Unknown and empty values fall back to
    /// Anthropic, which is what every pre-protocol configuration used.
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "openai" | "openai-chat" | "openai_chat" => VlProtocol::OpenAi,
            _ => VlProtocol::Anthropic,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            VlProtocol::Anthropic => "anthropic",
            VlProtocol::OpenAi => "openai",
        }
    }

    /// The endpoint path of the chat surface for this protocol.
    pub fn chat_endpoint(&self) -> &'static str {
        match self {
            VlProtocol::Anthropic => "/v1/messages",
            VlProtocol::OpenAi => "/v1/chat/completions",
        }
    }
}

// ----------------------------------------------------------------------------
// Endpoint joining
// ----------------------------------------------------------------------------

/// Join a user-entered gateway base URL with an endpoint path such as
/// `/v1/messages` or `/v1/models`.
///
/// The base may be a bare host (`http://gw:3425`), a base ending in `/v1`
/// (possibly behind a reverse-proxy prefix like `http://gw/proxy/v1`), or the
/// full endpoint itself. The rules, applied in order:
///
/// 1. the base already ends with the endpoint → returned unchanged;
/// 2. the base's last path segment is `v1` → only the endpoint's suffix is
///    appended, so the prefix survives;
/// 3. otherwise the full endpoint path is appended.
///
/// The result must parse as an http(s) URL; anything else is an error.
pub fn join_endpoint(base: &str, endpoint: &str) -> Result<String, String> {
    let base = base.trim().trim_end_matches('/');
    if base.is_empty() {
        return Err("网关地址为空".to_string());
    }
    let endpoint_path = endpoint.trim().trim_start_matches('/');
    if endpoint_path.is_empty() {
        return Err("端点路径为空".to_string());
    }
    let joined = if base.ends_with(&format!("/{endpoint_path}")) || base == endpoint_path {
        base.to_string()
    } else {
        let suffix = endpoint_path.strip_prefix("v1/").unwrap_or(endpoint_path);
        let last_segment = base.rsplit('/').next().unwrap_or("");
        if last_segment == "v1" {
            format!("{base}/{suffix}")
        } else {
            format!("{base}/{endpoint_path}")
        }
    };
    let url = reqwest::Url::parse(&joined).map_err(|e| format!("网关地址无效: {e}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("网关地址必须是 http 或 https".to_string());
    }
    Ok(joined)
}

// ----------------------------------------------------------------------------
// Authentication headers
// ----------------------------------------------------------------------------

/// The auth headers of one protocol. Anthropic always needs the key; OpenAI
/// works without one (local gateways) and only sends `Bearer` when a key is
/// set, so the key never goes out as an empty header.
pub fn auth_headers(protocol: VlProtocol, api_key: &str) -> Vec<(&'static str, String)> {
    let key = api_key.trim();
    match protocol {
        VlProtocol::Anthropic => vec![
            ("x-api-key", key.to_string()),
            (
                "anthropic-version",
                crate::t_common::VL_ANTHROPIC_VERSION.to_string(),
            ),
        ],
        VlProtocol::OpenAi => {
            if key.is_empty() {
                Vec::new()
            } else {
                vec![("authorization", format!("Bearer {key}"))]
            }
        }
    }
}

// ----------------------------------------------------------------------------
// Request bodies
// ----------------------------------------------------------------------------

/// The body of one vision chat call.
///
/// Anthropic carries the base64 payload as a `source` block and the system
/// prompt at the top level; OpenAI carries a data URL `image_url` part and
/// the system prompt as a leading role message.
pub fn build_vl_body(
    protocol: VlProtocol,
    model: &str,
    max_tokens: usize,
    system: Option<&str>,
    prompt: &str,
    image_base64: &str,
    media_type: &str,
) -> Value {
    match protocol {
        VlProtocol::Anthropic => {
            let mut body = json!({
                "model": model,
                "max_tokens": max_tokens,
                "messages": [{
                    "role": "user",
                    "content": [
                        {
                            "type": "image",
                            "source": {
                                "type": "base64",
                                "media_type": media_type,
                                "data": image_base64,
                            }
                        },
                        { "type": "text", "text": prompt }
                    ]
                }]
            });
            if let Some(system) = system.filter(|value| !value.is_empty()) {
                body["system"] = json!(system);
            }
            body
        }
        VlProtocol::OpenAi => {
            let mut messages = Vec::new();
            if let Some(system) = system.filter(|value| !value.is_empty()) {
                messages.push(json!({ "role": "system", "content": system }));
            }
            messages.push(json!({
                "role": "user",
                "content": [
                    {
                        "type": "image_url",
                        "image_url": {
                            "url": format!("data:{media_type};base64,{image_base64}")
                        }
                    },
                    { "type": "text", "text": prompt }
                ]
            }));
            json!({
                "model": model,
                "max_tokens": max_tokens,
                "messages": messages,
            })
        }
    }
}

/// The body of one text-only chat call (the search rerank).
///
/// The Anthropic body disables thinking explicitly; the OpenAI body has no
/// thinking field at all — sending one would be an Anthropic-only extension.
pub fn build_text_body(
    protocol: VlProtocol,
    model: &str,
    max_tokens: usize,
    system: &str,
    content: &[Value],
) -> Value {
    match protocol {
        VlProtocol::Anthropic => json!({
            "model": model,
            "max_tokens": max_tokens,
            "temperature": 0,
            "thinking": { "type": "disabled" },
            "system": system,
            "messages": [{ "role": "user", "content": content }],
        }),
        VlProtocol::OpenAi => json!({
            "model": model,
            "max_tokens": max_tokens,
            "temperature": 0,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": content },
            ],
        }),
    }
}

// ----------------------------------------------------------------------------
// Response parsing
// ----------------------------------------------------------------------------

/// Token usage of one response, protocol-neutral.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChatUsage {
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub thinking_tokens: Option<i64>,
}

/// The protocol-neutral result of one chat response.
#[derive(Debug, Clone, Default)]
pub struct ChatParse {
    pub text: String,
    pub thinking: Option<String>,
    pub model: Option<String>,
    pub request_id: Option<String>,
    pub stop_reason: Option<String>,
    pub usage: ChatUsage,
}

/// Parse an OpenAI Chat Completions response body.
///
/// `choices[0].message.content` may be a plain string or an array of parts;
/// `reasoning_content`/`reasoning` carry the thinking text. `finish_reason`
/// is normalized onto the Anthropic names the caller already handles
/// (`max_tokens` for `length`).
pub fn parse_openai_chat(body: &str) -> Result<ChatParse, String> {
    let value: Value =
        serde_json::from_str(body).map_err(|e| format!("网关响应不是合法 JSON: {e}"))?;
    let choice = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .ok_or_else(|| "网关响应缺少 choices 字段".to_string())?;
    let message = choice
        .get("message")
        .ok_or_else(|| "网关响应缺少 message 字段".to_string())?;

    let mut text_parts: Vec<String> = Vec::new();
    match message.get("content") {
        Some(Value::String(text)) => text_parts.push(text.clone()),
        Some(Value::Array(parts)) => {
            for part in parts {
                if let Some(text) = part
                    .get("text")
                    .and_then(Value::as_str)
                    .filter(|text| !text.trim().is_empty())
                {
                    text_parts.push(text.to_string());
                }
            }
        }
        _ => {}
    }

    let thinking = ["reasoning_content", "reasoning"]
        .iter()
        .find_map(|key| message.get(*key).and_then(Value::as_str))
        .filter(|text| !text.trim().is_empty())
        .map(str::to_string);

    let usage_value = value.get("usage");
    let thinking_tokens = usage_value
        .and_then(|u| u.get("completion_tokens_details"))
        .and_then(|d| d.get("reasoning_tokens"))
        .and_then(Value::as_i64);

    let stop_reason =
        choice
            .get("finish_reason")
            .and_then(Value::as_str)
            .map(|reason| match reason {
                "length" => "max_tokens".to_string(),
                other => other.to_string(),
            });

    Ok(ChatParse {
        text: text_parts.join(""),
        thinking,
        model: value
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_string),
        request_id: value.get("id").and_then(Value::as_str).map(str::to_string),
        stop_reason,
        usage: ChatUsage {
            input_tokens: usage_value
                .and_then(|u| u.get("prompt_tokens"))
                .and_then(Value::as_i64),
            output_tokens: usage_value
                .and_then(|u| u.get("completion_tokens"))
                .and_then(Value::as_i64),
            cache_read_tokens: usage_value
                .and_then(|u| u.get("prompt_tokens_details"))
                .and_then(|d| d.get("cached_tokens"))
                .and_then(Value::as_i64),
            thinking_tokens,
        },
    })
}

// ----------------------------------------------------------------------------
// Model discovery
// ----------------------------------------------------------------------------

/// Whether a listed model accepts image input. `Unknown` keeps models whose
/// provider lists neither `modalities.input` nor `capabilities` visible
/// instead of silently filtering them out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VlCapability {
    Supported,
    Unsupported,
    Unknown,
}

/// One entry of the gateway model list.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VlModelInfo {
    pub id: String,
    pub display_name: Option<String>,
    pub vision: VlCapability,
    pub context_length: Option<i64>,
    pub max_output_tokens: Option<i64>,
    pub reasoning: Option<bool>,
}

/// The outcome of a model-list request. `auth_required` marks a 401 answer:
/// the list is unusable but the chat surface may still work (an OpenAI
/// gateway without a key), so the frontend offers manual entry instead of
/// treating the gateway as unconfigured.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VlModelList {
    pub models: Vec<VlModelInfo>,
    pub auth_required: bool,
}

/// Parse a `/v1/models` response body.
///
/// Two field shapes occur in practice: the magpie gateway reports
/// `modalities.input` + `context_length`, the vl-gw reports
/// `capabilities` + `context_window`. Both are read; models carrying
/// neither shape stay in the list with `Unknown` vision capability.
pub fn parse_models_body(body: &str) -> Result<Vec<VlModelInfo>, String> {
    let value: Value =
        serde_json::from_str(body).map_err(|e| format!("模型列表不是合法 JSON: {e}"))?;
    let models = value
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| "模型列表缺少 data 字段".to_string())?;
    let mut parsed = Vec::with_capacity(models.len());
    for model in models {
        let Some(id) = model.get("id").and_then(Value::as_str) else {
            continue;
        };
        let modalities = model
            .get("modalities")
            .and_then(|m| m.get("input"))
            .and_then(Value::as_array);
        let capabilities = model.get("capabilities").and_then(Value::as_array);
        let vision = if let Some(inputs) = modalities {
            let accepts = inputs.iter().any(|item| item.as_str() == Some("image"));
            if accepts {
                VlCapability::Supported
            } else {
                VlCapability::Unsupported
            }
        } else if let Some(items) = capabilities {
            let accepts = items.iter().any(|item| item.as_str() == Some("vision"));
            if accepts {
                VlCapability::Supported
            } else {
                VlCapability::Unsupported
            }
        } else {
            VlCapability::Unknown
        };
        parsed.push(VlModelInfo {
            id: id.to_string(),
            display_name: model
                .get("display_name")
                .or_else(|| model.get("name"))
                .and_then(Value::as_str)
                .map(str::to_string),
            vision,
            context_length: ["context_length", "context_window"]
                .iter()
                .find_map(|key| model.get(*key).and_then(Value::as_i64)),
            max_output_tokens: model.get("max_output_tokens").and_then(Value::as_i64),
            reasoning: model.get("reasoning").and_then(Value::as_bool),
        });
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------
    // protocol parsing
    // ------------------------------------------------------------------

    #[test]
    fn protocol_parsing_defaults_to_anthropic() {
        assert_eq!(VlProtocol::parse(""), VlProtocol::Anthropic);
        assert_eq!(VlProtocol::parse("anthropic"), VlProtocol::Anthropic);
        assert_eq!(VlProtocol::parse(" Anthropic "), VlProtocol::Anthropic);
        assert_eq!(VlProtocol::parse("nonsense"), VlProtocol::Anthropic);
        assert_eq!(VlProtocol::parse("openai"), VlProtocol::OpenAi);
        assert_eq!(VlProtocol::parse("OpenAI"), VlProtocol::OpenAi);
        assert_eq!(VlProtocol::default(), VlProtocol::Anthropic);
        assert_eq!(VlProtocol::OpenAi.as_str(), "openai");
        assert_eq!(VlProtocol::Anthropic.chat_endpoint(), "/v1/messages");
        assert_eq!(VlProtocol::OpenAi.chat_endpoint(), "/v1/chat/completions");
    }

    // ------------------------------------------------------------------
    // endpoint joining
    // ------------------------------------------------------------------

    #[test]
    fn join_appends_the_full_path_to_a_bare_host() {
        assert_eq!(
            join_endpoint("http://gw:3425", "/v1/messages").unwrap(),
            "http://gw:3425/v1/messages"
        );
        assert_eq!(
            join_endpoint("http://gw:3425/", "/v1/models").unwrap(),
            "http://gw:3425/v1/models"
        );
    }

    #[test]
    fn join_appends_only_the_suffix_to_a_v1_base() {
        assert_eq!(
            join_endpoint("http://gw:3425/v1", "/v1/messages").unwrap(),
            "http://gw:3425/v1/messages"
        );
        assert_eq!(
            join_endpoint("http://gw:3425/v1/", "/v1/chat/completions").unwrap(),
            "http://gw:3425/v1/chat/completions"
        );
    }

    #[test]
    fn join_keeps_a_reverse_proxy_prefix() {
        assert_eq!(
            join_endpoint("http://gw:3425/proxy/v1", "/v1/chat/completions").unwrap(),
            "http://gw:3425/proxy/v1/chat/completions"
        );
    }

    #[test]
    fn join_does_not_duplicate_a_full_endpoint() {
        assert_eq!(
            join_endpoint("http://gw:3425/v1/messages", "/v1/messages").unwrap(),
            "http://gw:3425/v1/messages"
        );
        assert_eq!(
            join_endpoint("http://gw:3425/v1/chat/completions", "/v1/chat/completions").unwrap(),
            "http://gw:3425/v1/chat/completions"
        );
    }

    #[test]
    fn join_rejects_empty_and_non_http_bases() {
        assert!(join_endpoint("", "/v1/messages").is_err());
        assert!(join_endpoint("   ", "/v1/messages").is_err());
        assert!(join_endpoint("not a url", "/v1/messages").is_err());
        assert!(join_endpoint("ftp://gw", "/v1/messages").is_err());
        assert!(join_endpoint("http://gw", "").is_err());
    }

    // ------------------------------------------------------------------
    // auth headers
    // ------------------------------------------------------------------

    #[test]
    fn auth_headers_follow_the_protocol() {
        let anthropic = auth_headers(VlProtocol::Anthropic, "k1");
        assert_eq!(anthropic[0], ("x-api-key", "k1".to_string()));
        assert_eq!(anthropic[1].0, "anthropic-version");

        let openai = auth_headers(VlProtocol::OpenAi, "k1");
        assert_eq!(openai, vec![("authorization", "Bearer k1".to_string())]);

        // An OpenAI gateway works without a key: no empty header goes out.
        assert!(auth_headers(VlProtocol::OpenAi, "").is_empty());
        assert!(!auth_headers(VlProtocol::Anthropic, "").is_empty());
    }

    // ------------------------------------------------------------------
    // request bodies
    // ------------------------------------------------------------------

    #[test]
    fn vl_body_shapes_per_protocol() {
        let anthropic = build_vl_body(
            VlProtocol::Anthropic,
            "m",
            1024,
            Some("sys"),
            "prompt",
            "AAAA",
            "image/jpeg",
        );
        assert_eq!(anthropic["system"], json!("sys"));
        assert_eq!(
            anthropic["messages"][0]["content"][0]["source"]["data"],
            json!("AAAA")
        );
        assert_eq!(anthropic["max_tokens"], json!(1024));

        let openai = build_vl_body(
            VlProtocol::OpenAi,
            "m",
            1024,
            Some("sys"),
            "prompt",
            "AAAA",
            "image/jpeg",
        );
        assert_eq!(openai["messages"][0]["role"], json!("system"));
        assert_eq!(openai["messages"][0]["content"], json!("sys"));
        assert_eq!(
            openai["messages"][1]["content"][0]["image_url"]["url"],
            json!("data:image/jpeg;base64,AAAA")
        );
        assert!(openai.get("system").is_none());
    }

    #[test]
    fn vl_body_omits_an_empty_system() {
        for protocol in [VlProtocol::Anthropic, VlProtocol::OpenAi] {
            let body = build_vl_body(protocol, "m", 10, Some(""), "p", "AA", "image/png");
            // Neither protocol carries an empty system anywhere.
            assert!(!body.to_string().contains("\"system\""));
        }
    }

    #[test]
    fn text_body_omits_the_thinking_field_on_openai() {
        let content = vec![json!({"type": "text", "text": "hi"})];
        let anthropic = build_text_body(VlProtocol::Anthropic, "m", 100, "sys", &content);
        assert_eq!(anthropic["thinking"]["type"], json!("disabled"));
        assert_eq!(anthropic["system"], json!("sys"));

        let openai = build_text_body(VlProtocol::OpenAi, "m", 100, "sys", &content);
        assert!(openai.get("thinking").is_none());
        assert!(openai.get("system").is_none());
        assert_eq!(openai["messages"][0]["role"], json!("system"));
    }

    // ------------------------------------------------------------------
    // OpenAI response parsing
    // ------------------------------------------------------------------

    #[test]
    fn openai_chat_parses_a_plain_string_content() {
        let body = r#"{
            "id": "chatcmpl-1",
            "model": "qwen-vl",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "{\"caption\":\"猫\"}" },
                "finish_reason": "stop"
            }],
            "usage": { "prompt_tokens": 10, "completion_tokens": 5 }
        }"#;
        let parsed = parse_openai_chat(body).unwrap();
        assert_eq!(parsed.text, "{\"caption\":\"猫\"}");
        assert_eq!(parsed.model.as_deref(), Some("qwen-vl"));
        assert_eq!(parsed.request_id.as_deref(), Some("chatcmpl-1"));
        assert_eq!(parsed.stop_reason.as_deref(), Some("stop"));
        assert_eq!(parsed.usage.input_tokens, Some(10));
        assert_eq!(parsed.usage.output_tokens, Some(5));
        assert!(parsed.thinking.is_none());
    }

    #[test]
    fn openai_chat_parses_part_arrays_reasoning_and_length() {
        let body = r#"{
            "model": "qwen-vl",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": [
                        { "type": "text", "text": "part one " },
                        { "type": "text", "text": "part two" }
                    ],
                    "reasoning_content": "thinking hard"
                },
                "finish_reason": "length"
            }],
            "usage": {
                "prompt_tokens": 7,
                "completion_tokens": 3,
                "prompt_tokens_details": { "cached_tokens": 4 },
                "completion_tokens_details": { "reasoning_tokens": 2 }
            }
        }"#;
        let parsed = parse_openai_chat(body).unwrap();
        assert_eq!(parsed.text, "part one part two");
        assert_eq!(parsed.thinking.as_deref(), Some("thinking hard"));
        // `length` is normalized onto the Anthropic name the caller checks.
        assert_eq!(parsed.stop_reason.as_deref(), Some("max_tokens"));
        assert_eq!(parsed.usage.cache_read_tokens, Some(4));
        assert_eq!(parsed.usage.thinking_tokens, Some(2));
    }

    #[test]
    fn openai_chat_rejects_broken_shapes() {
        assert!(parse_openai_chat("not json").is_err());
        assert!(parse_openai_chat("{}").is_err());
        assert!(parse_openai_chat(r#"{"choices":[]}"#).is_err());
    }

    // ------------------------------------------------------------------
    // model list parsing
    // ------------------------------------------------------------------

    #[test]
    fn models_read_the_magpie_shape() {
        let body = r#"{"data": [
            { "id": "vl-model", "display_name": "VL Model",
              "modalities": { "input": ["text", "image"] },
              "context_length": 131072, "max_output_tokens": 8192,
              "reasoning": true },
            { "id": "text-model",
              "modalities": { "input": ["text"] },
              "context_length": 4096 }
        ]}"#;
        let models = parse_models_body(body).unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "vl-model");
        assert_eq!(models[0].display_name.as_deref(), Some("VL Model"));
        assert_eq!(models[0].vision, VlCapability::Supported);
        assert_eq!(models[0].context_length, Some(131072));
        assert_eq!(models[0].max_output_tokens, Some(8192));
        assert_eq!(models[0].reasoning, Some(true));
        assert_eq!(models[1].vision, VlCapability::Unsupported);
        assert_eq!(models[1].max_output_tokens, None);
    }

    #[test]
    fn models_read_the_vl_gw_shape() {
        let body = r#"{"data": [
            { "id": "gw-vision", "capabilities": ["vision", "tools"],
              "context_window": 32768 },
            { "id": "gw-text", "capabilities": ["tools"], "context_window": 8192 }
        ]}"#;
        let models = parse_models_body(body).unwrap();
        assert_eq!(models[0].vision, VlCapability::Supported);
        assert_eq!(models[0].context_length, Some(32768));
        assert_eq!(models[1].vision, VlCapability::Unsupported);
    }

    #[test]
    fn models_without_any_capability_field_stay_unknown() {
        let body = r#"{"data": [ { "id": "mystery" } ]}"#;
        let models = parse_models_body(body).unwrap();
        assert_eq!(models[0].vision, VlCapability::Unknown);
        assert!(models[0].context_length.is_none());
    }

    #[test]
    fn models_rejects_broken_shapes() {
        assert!(parse_models_body("{}").is_err());
        assert!(parse_models_body("nope").is_err());
    }
}
