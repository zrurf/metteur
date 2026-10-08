//! Anthropic Messages provider (`POST /messages`).

use async_trait::async_trait;
use metteur_shared::llm::{
    ContentBlock, ContextManager, GenerationParams, Message, Role, ToolCall, ToolDefinition, Usage,
};
use serde_json::{Value as Json, json};

use crate::error::{DaemonError, DaemonResult};

use super::super::client::{
    LlmClient, LlmProviderConfig, LlmResponse, StreamDelta, ThinkingBlock, http_error,
};

/// The Anthropic API version header value.
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Cache breakpoints the API accepts per request.
///
/// Ordering is load-bearing: a change in an earlier section invalidates every
/// later one, so tools are marked before the system prompt and the conversation
/// tail comes last.
const MAX_CACHE_BREAKPOINTS: usize = 4;

/// Lifetime of a cache entry, as a JSON value so it can be dropped verbatim
/// into every `cache_control`.
///
/// The API default is 5 minutes, which a human-in-the-loop pause (blueprint
/// approval, an edit confirmation) routinely outlives — resuming then re-writes
/// the whole prefix at the 1.25x write price. One hour matches how long an
/// unattended run actually sits waiting for a decision.
fn cache_ttl() -> &'static str {
    "1h"
}

/// Builds one `cache_control` object.
fn cache_control() -> Json {
    json!({ "type": "ephemeral", "ttl": cache_ttl() })
}

/// A client for the Anthropic Messages API.
pub struct AnthropicClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
    default_params: GenerationParams,
    /// Extended-thinking token budget (`0` keeps thinking disabled).
    thinking_budget: u64,
    /// Whether to emit explicit prompt-cache breakpoints.
    prompt_cache: bool,
}

impl AnthropicClient {
    /// Creates a new Anthropic client.
    pub fn new(http: reqwest::Client, config: &LlmProviderConfig) -> Self {
        Self {
            http,
            base_url: config.base_url.trim_end_matches('/').to_string(),
            api_key: config.api_key.clone(),
            model: config.model.clone(),
            default_params: config.default_params.clone(),
            thinking_budget: config.thinking_budget_tokens,
            prompt_cache: config.prompt_cache,
        }
    }

    /// Builds the request body for a completion.
    fn build_body(
        &self,
        ctx: &ContextManager,
        params: &GenerationParams,
        tools: &[ToolDefinition],
    ) -> Json {
        let merged = merge_params(&self.default_params, params);
        // Breakpoints are planned up front so the request never exceeds the
        // API's limit when every segment is present.
        let budget = if self.prompt_cache { MAX_CACHE_BREAKPOINTS } else { 0 };
        let mutate_tools = budget >= 1;
        let mutate_system = budget >= 2;
        let rolling = budget >= 3;
        let mut body = json!({
            "model": self.model,
            "messages": build_messages(ctx, rolling),
            "max_tokens": merged.max_tokens.unwrap_or(1024),
        });
        if !ctx.system_fragments.is_empty() {
            // Anthropic caches explicitly: marking the system block keeps the
            // (large, stable) prefix cached across the turns of a conversation.
            // The canonical order is a precondition for that stability.
            let system = ctx.system_text();
            body["system"] = if mutate_system {
                // One block per fragment, not one concatenated block: the
                // fragments are ordered so that volatile content (today's date)
                // renders last, and only per-fragment blocks let a change in one
                // fragment leave the rest of the prefix cached. A single block
                // would invalidate everything behind it.
                let blocks: Vec<Json> = ctx
                    .system_fragments
                    .iter()
                    .map(|fragment| {
                        json!({
                            "type": "text",
                            "text": fragment.content,
                            "cache_control": cache_control(),
                        })
                    })
                    .collect();
                json!(blocks)
            } else {
                json!(system)
            };
        }
        // Extended thinking forbids a custom temperature (the API requires its
        // default); the parameter is dropped rather than sent and rejected.
        if self.thinking_budget > 0 {
            body["thinking"] = json!({
                "type": "enabled",
                "budget_tokens": self.thinking_budget,
            });
        } else if let Some(v) = merged.temperature {
            body["temperature"] = json!(v);
        }
        if let Some(v) = merged.top_p {
            body["top_p"] = json!(v);
        }
        if !merged.stop.is_empty() {
            body["stop_sequences"] = json!(merged.stop);
        }
        if !tools.is_empty() {
            // A cache breakpoint on the last tool extends the cached prefix to
            // cover the tool definitions as well as the system prompt.
            let mut tools_json = build_tools(tools);
            if mutate_tools
                && let Some(last) = tools_json.as_array_mut().and_then(|items| items.last_mut())
            {
                last["cache_control"] = cache_control();
            }
            body["tools"] = tools_json;
        }
        body
    }

    /// Sends the request and parses the response.
    async fn send(&self, body: Json) -> DaemonResult<LlmResponse> {
        let url = format!("{}/messages", self.base_url);
        let resp = self
            .http
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&body)
            .send()
            .await
            .map_err(|e| http_error("anthropic request", e))?;

        let status = resp.status();
        let text = resp.text().await.map_err(|e| http_error("anthropic response", e))?;
        if !status.is_success() {
            return Err(DaemonError::LlmStatus {
                status: status.as_u16(),
                message: text,
            });
        }
        let parsed: Json = serde_json::from_str(&text)
            .map_err(|e| DaemonError::Llm(format!("invalid anthropic response: {e}")))?;
        parse_response(&parsed)
    }
}

#[async_trait]
impl LlmClient for AnthropicClient {
    fn provider(&self) -> &str {
        "anthropic"
    }

    fn model(&self) -> &str {
        &self.model
    }

    async fn complete(
        &self,
        ctx: &ContextManager,
        params: &GenerationParams,
        tools: &[ToolDefinition],
    ) -> DaemonResult<LlmResponse> {
        let body = self.build_body(ctx, params, tools);
        self.send(body).await
    }

    async fn stream(
        &self,
        ctx: &ContextManager,
        params: &GenerationParams,
        tools: &[ToolDefinition],
        on_delta: &mut (dyn FnMut(StreamDelta) + Send),
    ) -> DaemonResult<LlmResponse> {
        let mut body = self.build_body(ctx, params, tools);
        body["stream"] = json!(true);
        let url = format!("{}/messages", self.base_url);
        let resp = self
            .http
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&body)
            .send()
            .await
            .map_err(|e| http_error("anthropic stream request", e))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.map_err(|e| http_error("anthropic stream response", e))?;
            return Err(DaemonError::LlmStatus {
                status: status.as_u16(),
                message: text,
            });
        }

        let mut stream = resp.bytes_stream();
        let mut text_out = String::new();
        let mut thinking: Vec<ThinkingBlock> = Vec::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut usage_fields = json!({});
        let mut usage_final = false;
        let mut buf = String::new();

        use tokio_stream::StreamExt;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| http_error("anthropic stream", e))?;
            buf.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(pos) = buf.find('\n') {
                let line = buf[..pos].to_string();
                buf = buf[pos + 1..].to_string();
                let line = line.trim();
                if !line.starts_with("data:") {
                    continue;
                }
                let data = line[5..].trim();
                let event: Json = match serde_json::from_str(data) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                match event.get("type").and_then(|t| t.as_str()) {
                    Some("message_start") => {
                        if let Some(u) = event.pointer("/message/usage") { merge_usage(&mut usage_fields, u); }
                    }
                    Some("content_block_delta") => {
                        if let Some(text) = event.pointer("/delta/text").and_then(|v| v.as_str()) {
                            text_out.push_str(text);
                            on_delta(StreamDelta::Text(text.to_string()));
                        }
                        // Thinking streams as text deltas followed by a
                        // signature delta that must be preserved for replay.
                        if let Some(text) =
                            event.pointer("/delta/thinking").and_then(|v| v.as_str())
                        {
                            on_delta(StreamDelta::Reasoning(text.to_string()));
                            match thinking.last_mut() {
                                Some(block) => block.text.push_str(text),
                                None => thinking.push(ThinkingBlock {
                                    text: text.to_string(),
                                    ..ThinkingBlock::default()
                                }),
                            }
                        }
                        if let Some(signature) =
                            event.pointer("/delta/signature").and_then(|v| v.as_str())
                        {
                            match thinking.last_mut() {
                                Some(block) => block.signature = Some(signature.to_string()),
                                None => thinking.push(ThinkingBlock {
                                    signature: Some(signature.to_string()),
                                    ..ThinkingBlock::default()
                                }),
                            }
                        }
                        if let Some(input) =
                            event.pointer("/delta/partial_json").and_then(|v| v.as_str())
                            && let Some(last) = tool_calls.last_mut()
                        {
                            let current = match &last.arguments {
                                Json::String(s) => s.clone(),
                                _ => String::new(),
                            };
                            let combined = format!("{current}{input}");
                            let bytes = combined.len();
                            let name = last.name.clone();
                            last.arguments =
                                serde_json::from_str(&combined).unwrap_or(Json::String(combined));
                            on_delta(StreamDelta::ToolArgs {
                                name,
                                bytes,
                            });
                        }
                    }
                    Some("content_block_start") => {
                        // A redacted thinking block arrives whole; it is
                        // opaque and replayed verbatim.
                        if let Some(block) = event.pointer("/content_block")
                            && block.get("type").and_then(|t| t.as_str())
                                == Some("redacted_thinking")
                            && let Some(data) = block.get("data").and_then(|v| v.as_str())
                        {
                            thinking.push(ThinkingBlock {
                                redacted: Some(data.to_string()),
                                ..ThinkingBlock::default()
                            });
                        }
                        if let Some(tool_use) = event.pointer("/content_block")
                            && tool_use.get("type").and_then(|t| t.as_str()) == Some("tool_use")
                        {
                            let id = tool_use
                                .get("id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let name = tool_use
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            tool_calls.push(ToolCall {
                                id,
                                name,
                                arguments: Json::Null,
                            });
                        }
                    }
                    Some("message_delta") => {
                        if let Some(u) = event.get("usage") {
                            usage_final |= u.get("output_tokens").and_then(|v| v.as_u64()).is_some();
                            merge_usage(&mut usage_fields, u);
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(LlmResponse {
            text: text_out,
            thinking,
            tool_calls,
            usage: parse_stream_usage(&usage_fields, usage_final),
        })
    }
}

/// Converts a context into the Anthropic messages array.
fn build_messages(ctx: &ContextManager, rolling_breakpoint: bool) -> Vec<Json> {
    let mut messages: Vec<Json> = ctx.messages.iter().map(message_to_json).collect();
    if rolling_breakpoint && let Some(last) = messages.last_mut() {
        // The conversation tail is append-only, so caching it costs one write
        // now and turns every later turn's history into a cache read. Providers
        // whose cacheable minimum is not reached silently skip the write, so a
        // short conversation is unaffected.
        mark_last_block(last);
    }
    messages
}

/// Marks the final content block of a message as a cache breakpoint.
///
/// A message whose content is a plain string is rewritten into the equivalent
/// single text block, which is the only form that can carry `cache_control`.
/// Reasoning blocks are skipped: they carry a signature that must be replayed
/// verbatim, and providers reject a breakpoint on generated reasoning.
fn mark_last_block(message: &mut Json) {
    let ephemeral = cache_control();
    match message.get_mut("content") {
        Some(Json::String(text)) => {
            let text = text.clone();
            message["content"] = json!([{
                "type": "text",
                "text": text,
                "cache_control": ephemeral,
            }]);
        }
        Some(Json::Array(blocks)) => {
            let markable = blocks.iter_mut().rev().find(|block| {
                !matches!(
                    block.get("type").and_then(Json::as_str),
                    Some("thinking" | "redacted_thinking")
                )
            });
            if let Some(block) = markable {
                block["cache_control"] = ephemeral;
            }
        }
        _ => {}
    }
}

/// Converts a shared message into an Anthropic message object.
fn message_to_json(msg: &Message) -> Json {
    match msg.role {
        Role::Tool => {
            // Anthropic represents tool results as a user message with a
            // `tool_result` content block.
            let content = msg.text_content();
            json!({
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": msg.tool_call_id.clone().unwrap_or_default(),
                    "content": content,
                }]
            })
        }
        Role::Assistant if !msg.tool_calls.is_empty() => {
            // Thinking blocks must precede text and tool_use in the replay:
            // the API rejects a turn whose thinking was stripped while tool
            // calls were kept.
            let mut content: Vec<Json> =
                msg.content.iter().filter_map(block_to_json).collect();
            for call in &msg.tool_calls {
                content.push(json!({
                    "type": "tool_use",
                    "id": call.id,
                    "name": call.name,
                    "input": call.arguments,
                }));
            }
            json!({ "role": "assistant", "content": content })
        }
        _ => {
            let blocks: Vec<Json> = msg.content.iter().filter_map(block_to_json).collect();
            if blocks.len() == 1 && blocks[0].get("type").and_then(|t| t.as_str()) == Some("text") {
                json!({ "role": role_str(msg.role), "content": blocks[0]["text"] })
            } else {
                json!({ "role": role_str(msg.role), "content": blocks })
            }
        }
    }
}

/// Converts one content block into its Anthropic representation.
///
/// Returns `None` for a thinking block without payload (defensive: the API
/// rejects an empty thinking block).
fn block_to_json(block: &ContentBlock) -> Option<Json> {
    match block {
        ContentBlock::Text(text) => Some(json!({ "type": "text", "text": text })),
        ContentBlock::Thinking {
            text,
            signature,
        } => {
            if text.is_empty() {
                return None;
            }
            let mut value = json!({ "type": "thinking", "thinking": text });
            if let Some(signature) = signature {
                value["signature"] = json!(signature);
            }
            Some(value)
        }
        ContentBlock::RedactedThinking {
            data,
        } => Some(json!({ "type": "redacted_thinking", "data": data })),
    }
}

fn role_str(role: Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "user",
    }
}

/// Merges default params with request params (request wins).
fn merge_params(defaults: &GenerationParams, params: &GenerationParams) -> GenerationParams {
    GenerationParams {
        temperature: params.temperature.or(defaults.temperature),
        top_p: params.top_p.or(defaults.top_p),
        max_tokens: params.max_tokens.or(defaults.max_tokens),
        stop: if params.stop.is_empty() {
            defaults.stop.clone()
        } else {
            params.stop.clone()
        },
        reasoning_effort: params.reasoning_effort.or(defaults.reasoning_effort),
        seed: params.seed.or(defaults.seed),
        presence_penalty: params.presence_penalty.or(defaults.presence_penalty),
        frequency_penalty: params.frequency_penalty.or(defaults.frequency_penalty),
    }
}

/// Parses an Anthropic response into a shared response.
fn parse_response(parsed: &Json) -> DaemonResult<LlmResponse> {
    let mut text = String::new();
    let mut thinking: Vec<ThinkingBlock> = Vec::new();
    let mut tool_calls = Vec::new();
    if let Some(content) = parsed.get("content").and_then(|c| c.as_array()) {
        for block in content {
            match block.get("type").and_then(|t| t.as_str()) {
                Some("text") => {
                    if let Some(t) = block.get("text").and_then(|v| v.as_str()) {
                        text.push_str(t);
                    }
                }
                Some("thinking") => {
                    if let Some(t) = block.get("thinking").and_then(|v| v.as_str()) {
                        thinking.push(ThinkingBlock {
                            text: t.to_string(),
                            signature: block
                                .get("signature")
                                .and_then(|v| v.as_str())
                                .map(str::to_string),
                            redacted: None,
                        });
                    }
                }
                Some("redacted_thinking") => {
                    if let Some(data) = block.get("data").and_then(|v| v.as_str()) {
                        thinking.push(ThinkingBlock {
                            text: String::new(),
                            signature: None,
                            redacted: Some(data.to_string()),
                        });
                    }
                }
                Some("tool_use") => {
                    let id = block.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let name = block.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let args = block.get("input").cloned().unwrap_or(Json::Null);
                    tool_calls.push(ToolCall {
                        id,
                        name,
                        arguments: args,
                    });
                }
                _ => {}
            }
        }
    }
    let usage = parsed.get("usage").map(parse_usage).unwrap_or_default();
    Ok(LlmResponse {
        text,
        thinking,
        tool_calls,
        usage,
    })
}

/// Streaming usage updates replace reported fields without erasing prior counters.
fn merge_usage(total: &mut Json, delta: &Json) {
    if let (Some(total), Some(delta)) = (total.as_object_mut(), delta.as_object()) {
        total.extend(delta.iter().map(|(key, value)| (key.clone(), value.clone())));
    }
}

/// Parses an Anthropic usage object.
///
/// Anthropic reports cache traffic *outside* `input_tokens`, so the total is
/// the sum of the three counters; this provider normalizes to the shared
/// convention where `input_tokens` includes the cached parts.
fn parse_stream_usage(total: &Json, finalized: bool) -> Usage {
    let mut usage = parse_usage(total);
    usage.tokens_reported &= finalized;
    usage
}

fn parse_usage(u: &Json) -> Usage {
    let fresh = u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
    let output = u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
    let cache_read = u.get("cache_read_input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
    let cache_write = u.get("cache_creation_input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
    let input = fresh + cache_read + cache_write;
    Usage {
        tokens_reported: u.get("input_tokens").and_then(|v| v.as_u64()).is_some()
            && u.get("output_tokens").and_then(|v| v.as_u64()).is_some()
            && u.get("cache_read_input_tokens").and_then(|v| v.as_u64()).is_some()
            && u.get("cache_creation_input_tokens").and_then(|v| v.as_u64()).is_some(),
        cache_read_reported: u.pointer("/cache_read_input_tokens").and_then(|v| v.as_u64()).is_some(),
        input_tokens: input,
        output_tokens: output,
        reasoning_tokens: 0,
        total_tokens: input + output,
        cached_input_tokens: cache_read,
        cache_write_input_tokens: cache_write,
    }
}

/// Builds the tools array for a request.
pub(crate) fn build_tools(definitions: &[ToolDefinition]) -> Json {
    Json::Array(
        definitions
            .iter()
            .map(|d| {
                json!({
                    "name": d.name,
                    "description": d.description,
                    "input_schema": d.parameters,
                })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_messages_with_tool_result() {
        let mut ctx = ContextManager::new_from_prompt(vec![], "hello");
        ctx.mix_in_tool_result(metteur_shared::llm::ToolResult {
            tool: "ReadFile".to_string(),
            tool_call_id: "toolu_1".to_string(),
            content: "42".to_string(),
            timestamp: 0,
            lifetime: metteur_shared::llm::ToolResultLifetime::OneShot,
            paths: Vec::new(),
        });
        let messages = build_messages(&ctx, false);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[1]["content"][0]["type"], "tool_result");
        assert_eq!(messages[1]["content"][0]["tool_use_id"], "toolu_1");
    }

    #[test]
    fn parses_response_with_tool_use() {
        let json = json!({
            "content": [
                { "type": "text", "text": "Let me check." },
                { "type": "tool_use", "id": "toolu_1", "name": "ReadFile", "input": { "path": "a.txt" } }
            ],
            "usage": { "input_tokens": 10, "output_tokens": 5 }
        });
        let resp = parse_response(&json).unwrap();
        assert_eq!(resp.text, "Let me check.");
        assert_eq!(resp.tool_calls.len(), 1);
        assert_eq!(resp.tool_calls[0].name, "ReadFile");
        assert_eq!(resp.usage.input_tokens, 10);
    }

    #[test]
    fn requires_max_tokens() {
        let client = AnthropicClient::new(
            reqwest::Client::new(),
            &LlmProviderConfig::new(
                super::super::super::ProviderKind::Anthropic,
                "https://api.anthropic.com",
                "key",
                "claude-sonnet-4",
            ),
        );
        let ctx = ContextManager::new_from_prompt(vec![], "hi");
        let body = client.build_body(&ctx, &GenerationParams::default(), &[]);
        assert!(body["max_tokens"].as_u64().unwrap() > 0);
    }

    #[test]
    fn system_prompt_carries_a_cache_breakpoint() {
        let client = AnthropicClient::new(
            reqwest::Client::new(),
            &LlmProviderConfig::new(
                super::super::super::ProviderKind::Anthropic,
                "https://api.anthropic.com",
                "key",
                "claude-sonnet-4",
            ),
        );
        let ctx = ContextManager::new_from_prompt(
            vec![metteur_shared::llm::SystemFragment {
                priority: 0,
                scope: "test".to_string(),
                content: "system text".to_string(),
            }],
            "hi",
        );
        let body = client.build_body(&ctx, &GenerationParams::default(), &[]);
        assert_eq!(body["system"][0]["type"], "text");
        assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn cache_breakpoints_can_be_disabled() {
        let client = AnthropicClient::new(
            reqwest::Client::new(),
            &LlmProviderConfig::new(
                super::super::super::ProviderKind::Anthropic,
                "https://api.anthropic.com",
                "key",
                "claude-sonnet-4",
            )
            .with_prompt_cache(false),
        );
        let ctx = ContextManager::new_from_prompt(
            vec![metteur_shared::llm::SystemFragment {
                priority: 0,
                scope: "test".to_string(),
                content: "system text".to_string(),
            }],
            "hi",
        );
        let body = client.build_body(&ctx, &GenerationParams::default(), &[]);
        // Without breakpoints the system field stays a plain string.
        assert!(body["system"].is_string(), "{}", body["system"]);
    }

    /// A rolling breakpoint on the conversation tail is what turns the history
    /// of a long session into cache reads instead of cache writes.
    #[test]
    fn the_conversation_tail_carries_a_cache_breakpoint() {
        let client = AnthropicClient::new(
            reqwest::Client::new(),
            &LlmProviderConfig::new(
                super::super::super::ProviderKind::Anthropic,
                "https://api.anthropic.com",
                "key",
                "claude-sonnet-4",
            ),
        );
        let mut ctx = ContextManager::new_from_prompt(vec![], "hi");
        ctx.push_message(metteur_shared::llm::Message::text(
            metteur_shared::llm::Role::Assistant,
            "answer",
        ));
        let body = client.build_body(&ctx, &GenerationParams::default(), &[]);
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.last().unwrap()["content"][0]["type"], "text");
        assert_eq!(
            messages.last().unwrap()["content"][0]["cache_control"]["type"],
            "ephemeral"
        );
        // Earlier messages stay untouched.
        assert!(messages[0]["content"].is_string());
    }

    /// Reasoning blocks must not carry a breakpoint: the API rejects a
    /// `cache_control` on generated thinking.
    #[test]
    fn a_trailing_thinking_block_is_not_marked() {
        let mut message = json!({
            "role": "assistant",
            "content": [
                { "type": "thinking", "thinking": "hmm", "signature": "sig" },
                { "type": "text", "text": "answer" }
            ]
        });
        mark_last_block(&mut message);
        assert!(message["content"][0].get("cache_control").is_none());
        assert_eq!(message["content"][1]["cache_control"]["type"], "ephemeral");

        // A message that is nothing but reasoning stays unmarked.
        let mut thinking_only = json!({
            "role": "assistant",
            "content": [{ "type": "thinking", "thinking": "hmm", "signature": "sig" }]
        });
        mark_last_block(&mut thinking_only);
        assert!(thinking_only["content"][0].get("cache_control").is_none());
    }

    #[test]
    fn thinking_budget_enables_thinking_and_drops_temperature() {
        let client = AnthropicClient::new(
            reqwest::Client::new(),
            &LlmProviderConfig::new(
                super::super::super::ProviderKind::Anthropic,
                "https://api.anthropic.com",
                "key",
                "claude-sonnet-4",
            )
            .with_thinking_budget(4096),
        );
        let ctx = ContextManager::new_from_prompt(vec![], "hi");
        let params = GenerationParams {
            temperature: Some(0.7),
            ..Default::default()
        };
        let body = client.build_body(&ctx, &params, &[]);
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["thinking"]["budget_tokens"], 4096);
        // The API rejects a custom temperature while thinking is enabled.
        assert!(body.get("temperature").is_none(), "{body}");
    }

    #[test]
    fn thinking_blocks_are_parsed_with_signatures() {
        let json = json!({
            "content": [
                { "type": "thinking", "thinking": "let me reason", "signature": "sig-1" },
                { "type": "redacted_thinking", "data": "opaque" },
                { "type": "text", "text": "answer" }
            ],
            "usage": { "input_tokens": 1, "output_tokens": 1 }
        });
        let resp = parse_response(&json).unwrap();
        assert_eq!(resp.thinking.len(), 2);
        assert_eq!(resp.thinking[0].text, "let me reason");
        assert_eq!(resp.thinking[0].signature.as_deref(), Some("sig-1"));
        assert_eq!(resp.thinking[1].redacted.as_deref(), Some("opaque"));
        assert_eq!(resp.text, "answer");
        // Reasoning never leaks into the user-facing text.
        assert!(!resp.text.contains("reason"));
    }

    #[test]
    fn thinking_blocks_are_replayed_before_tool_use() {
        let msg = Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Thinking {
                    text: "reasoning".to_string(),
                    signature: Some("sig-1".to_string()),
                },
                ContentBlock::Text("calling".to_string()),
            ],
            tool_calls: vec![ToolCall {
                id: "toolu_1".to_string(),
                name: "ReadFile".to_string(),
                arguments: json!({ "path": "a.txt" }),
            }],
            tool_call_id: None,
        };
        let json = message_to_json(&msg);
        let content = json["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "thinking");
        assert_eq!(content[0]["thinking"], "reasoning");
        assert_eq!(content[0]["signature"], "sig-1");
        assert_eq!(content[1]["type"], "text");
        assert_eq!(content[2]["type"], "tool_use");
    }

    #[test]
    fn usage_normalizes_anthropic_cache_counters() {
        let usage = parse_usage(&json!({
            "input_tokens": 100,
            "output_tokens": 20,
            "cache_read_input_tokens": 900,
            "cache_creation_input_tokens": 50
        }));
        // Anthropic reports cache traffic outside `input_tokens`.
        assert_eq!(usage.input_tokens, 1050);
        assert_eq!(usage.cached_input_tokens, 900);
        assert_eq!(usage.cache_write_input_tokens, 50);
        assert_eq!(usage.uncached_input_tokens(), 100);
    }
    #[test]
    fn usage_distinguishes_missing_cache_from_zero() {
        let mut raw = json!({"input_tokens": 100, "output_tokens": 10});
        assert_eq!(parse_usage(&raw).cache_hit_rate(), None);
        raw["cache_read_input_tokens"] = json!(0);
        raw["cache_creation_input_tokens"] = json!(20);
        assert_eq!(parse_usage(&raw).cache_hit_rate(), Some(0.0));
        assert_eq!(parse_usage(&json!({})).cache_hit_rate(), None);
    }

    #[test]
    fn streaming_output_updates_preserve_input_and_cache_counters() {
        let mut total = json!({});
        merge_usage(&mut total, &json!({"input_tokens": 100, "cache_read_input_tokens": 800, "cache_creation_input_tokens": 100, "output_tokens": 1}));
        merge_usage(&mut total, &json!({"output_tokens": 50}));
        merge_usage(&mut total, &json!({"output_tokens": 50}));
        assert_eq!(parse_stream_usage(&total, false).cache_hit_rate(), None);
        let usage = parse_stream_usage(&total, true);
        assert_eq!(usage.input_tokens, 1000);
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.cache_hit_rate(), Some(0.8));
        assert_eq!(usage.uncached_input_tokens(), 100);
    }

}
