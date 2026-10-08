//! OpenAI Responses provider (`POST /responses`).

use async_trait::async_trait;
use metteur_shared::llm::{
    ContextManager, GenerationParams, Message, ReasoningEffort, Role, ToolCall,
    ToolDefinition, Usage,
};
use serde_json::{Value as Json, json};

use crate::error::{DaemonError, DaemonResult};

use super::super::client::{
    LlmClient, LlmProviderConfig, LlmResponse, StreamDelta, ThinkingBlock, http_error,
};

/// A client for the OpenAI Responses API.
pub struct OpenAiResponsesClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
    default_params: GenerationParams,
}

impl OpenAiResponsesClient {
    /// Creates a new OpenAI Responses client.
    pub fn new(http: reqwest::Client, config: &LlmProviderConfig) -> Self {
        Self {
            http,
            base_url: config.base_url.trim_end_matches('/').to_string(),
            api_key: config.api_key.clone(),
            model: config.model.clone(),
            default_params: config.default_params.clone(),
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
        let mut body = json!({
            "model": self.model,
            "input": build_input(ctx),
        });
        if !ctx.system_fragments.is_empty() {
            // Canonical fragment order keeps the cached prefix stable.
            let system = ctx.system_text();
            body["instructions"] = json!(system);
        }
        if let Some(v) = merged.temperature {
            body["temperature"] = json!(v);
        }
        if let Some(v) = merged.top_p {
            body["top_p"] = json!(v);
        }
        if let Some(v) = merged.max_tokens {
            body["max_output_tokens"] = json!(v);
        }
        if let Some(effort) = merged.reasoning_effort {
            body["reasoning"] = json!({ "effort": reasoning_str(effort) });
        }
        if !tools.is_empty() {
            body["tools"] = build_tools(tools);
        }
        body
    }

    /// Sends the request and parses the response.
    async fn send(&self, body: Json) -> DaemonResult<LlmResponse> {
        let url = format!("{}/responses", self.base_url);
        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| http_error("openai responses request", e))?;

        let status = resp.status();
        let text = resp.text().await.map_err(|e| http_error("openai responses response", e))?;
        if !status.is_success() {
            return Err(DaemonError::LlmStatus {
                status: status.as_u16(),
                message: text,
            });
        }
        let parsed: Json = serde_json::from_str(&text)
            .map_err(|e| DaemonError::Llm(format!("invalid openai responses response: {e}")))?;
        parse_response(&parsed)
    }
}

#[async_trait]
impl LlmClient for OpenAiResponsesClient {
    fn provider(&self) -> &str {
        "openai-responses"
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
        let url = format!("{}/responses", self.base_url);
        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| http_error("openai responses stream request", e))?;
        let status = resp.status();
        if !status.is_success() {
            let text =
                resp.text().await.map_err(|e| http_error("openai responses stream response", e))?;
            return Err(DaemonError::LlmStatus {
                status: status.as_u16(),
                message: text,
            });
        }

        let mut stream = resp.bytes_stream();
        let mut text_out = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut usage = Usage::default();
        let mut buf = String::new();

        use tokio_stream::StreamExt;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| http_error("openai responses stream", e))?;
            buf.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(pos) = buf.find('\n') {
                let line = buf[..pos].to_string();
                buf = buf[pos + 1..].to_string();
                let line = line.trim();
                if !line.starts_with("data:") {
                    continue;
                }
                let data = line[5..].trim();
                if data == "[DONE]" {
                    continue;
                }
                let event: Json = match serde_json::from_str(data) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                match event.get("type").and_then(|t| t.as_str()) {
                    Some("response.output_text.delta") => {
                        if let Some(text) = event.get("delta").and_then(|v| v.as_str()) {
                            text_out.push_str(text);
                            on_delta(StreamDelta::Text(text.to_string()));
                        }
                    }
                    Some("response.output_item.added") => {
                        // A function call item is created before its argument
                        // deltas arrive.
                        if let Some(item) = event.get("item")
                            && item.get("type").and_then(|t| t.as_str()) == Some("function_call")
                        {
                            let id = item
                                .get("call_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let name =
                                item.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                            tool_calls.push(ToolCall {
                                id,
                                name,
                                arguments: Json::Null,
                            });
                        }
                    }
                    Some("response.reasoning_summary_text.delta") => {
                        if let Some(text) = event.get("delta").and_then(|v| v.as_str()) {
                            on_delta(StreamDelta::Reasoning(text.to_string()));
                        }
                    }
                    Some("response.function_call_arguments.delta") => {
                        if let Some(input) = event.get("delta").and_then(|v| v.as_str())
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
                    Some("response.completed") => {
                        if let Some(u) = event.pointer("/response/usage") {
                            usage = parse_usage(u);
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(LlmResponse {
            text: text_out,
            thinking: Vec::new(),
            tool_calls,
            usage,
        })
    }
}

/// Converts a context into the Responses `input` array.
fn build_input(ctx: &ContextManager) -> Vec<Json> {
    ctx.messages.iter().flat_map(message_to_items).collect()
}

/// Converts a shared message into one or more Responses input items.
///
/// An assistant message with tool calls expands into the function call items
/// followed by the message item that references them.
fn message_to_items(msg: &Message) -> Vec<Json> {
    // Reasoning items are not replayed: the Responses API uses opaque
    // encrypted content for that, which this client does not store.
    let content = msg.text_content();
    match msg.role {
        Role::Tool => vec![json!({
            "type": "function_call_output",
            "call_id": msg.tool_call_id.clone().unwrap_or_default(),
            "output": content,
        })],
        Role::Assistant if !msg.tool_calls.is_empty() => {
            let mut items: Vec<Json> = Vec::new();
            let mut call_ids: Vec<Json> = Vec::new();
            for call in &msg.tool_calls {
                call_ids.push(json!(call.id));
                items.push(json!({
                    "type": "function_call",
                    "call_id": call.id,
                    "name": call.name,
                    "arguments": call.arguments.to_string(),
                }));
            }
            items.push(json!({
                "type": "message",
                "role": "assistant",
                "content": [{ "type": "output_text", "text": content }],
                "call_ids": call_ids,
            }));
            items
        }
        _ => vec![json!({
            "type": "message",
            "role": role_str(msg.role),
            "content": [{ "type": "input_text", "text": content }],
        })],
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

fn reasoning_str(effort: ReasoningEffort) -> &'static str {
    match effort {
        ReasoningEffort::None => "none",
        ReasoningEffort::Low => "low",
        ReasoningEffort::Medium => "medium",
        ReasoningEffort::High => "high",
    }
}

/// Parses a Responses response into a shared response.
fn parse_response(parsed: &Json) -> DaemonResult<LlmResponse> {
    let mut text = String::new();
    let mut thinking: Vec<ThinkingBlock> = Vec::new();
    let mut tool_calls = Vec::new();
    if let Some(output) = parsed.get("output").and_then(|o| o.as_array()) {
        for item in output {
            match item.get("type").and_then(|t| t.as_str()) {
                Some("message") => {
                    if let Some(content) = item.get("content").and_then(|c| c.as_array()) {
                        for block in content {
                            if let Some(t) = block.get("text").and_then(|v| v.as_str()) {
                                text.push_str(t);
                            }
                        }
                    }
                }
                // Reasoning arrives as a summary of its text parts; it is
                // shown and audited but never replayed.
                Some("reasoning") => {
                    let mut parts = Vec::new();
                    if let Some(summary) = item.get("summary").and_then(|s| s.as_array()) {
                        for part in summary {
                            if let Some(t) = part.get("text").and_then(|v| v.as_str()) {
                                parts.push(t.to_string());
                            }
                        }
                    }
                    let joined = parts.join("
");
                    if !joined.trim().is_empty() {
                        thinking.push(ThinkingBlock {
                            text: joined,
                            ..ThinkingBlock::default()
                        });
                    }
                }
                Some("function_call") => {
                    let id = item.get("call_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let args = item
                        .get("arguments")
                        .and_then(|v| v.as_str())
                        .and_then(|s| serde_json::from_str(s).ok())
                        .unwrap_or(Json::Null);
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

/// Parses a Responses usage object.
///
/// Like the Chat API, `input_tokens` already contains cache-served tokens;
/// `input_tokens_details.cached_tokens` is the breakdown.
fn parse_usage(u: &Json) -> Usage {
    let input = u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
    let output = u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
    let reasoning =
        u.pointer("/output_tokens_details/reasoning_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
    let cached =
        u.pointer("/input_tokens_details/cached_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
    Usage {
        tokens_reported: u.get("input_tokens").and_then(|v| v.as_u64()).is_some()
            && u.get("output_tokens").and_then(|v| v.as_u64()).is_some(),
        cache_read_reported: u.pointer("/input_tokens_details/cached_tokens").and_then(|v| v.as_u64()).is_some(),
        input_tokens: input,
        output_tokens: output,
        reasoning_tokens: reasoning,
        total_tokens: input + output,
        cached_input_tokens: cached,
        cache_write_input_tokens: 0,
    }
}

/// Builds the tools array for a request.
pub(crate) fn build_tools(definitions: &[ToolDefinition]) -> Json {
    Json::Array(
        definitions
            .iter()
            .map(|d| {
                json!({
                    "type": "function",
                    "name": d.name,
                    "description": d.description,
                    "parameters": d.parameters,
                })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use metteur_shared::llm::SystemFragment;

    #[test]
    fn builds_input_with_system_and_tool() {
        let mut ctx = ContextManager::new_from_prompt(
            vec![SystemFragment {
                priority: 0,
                scope: "test".to_string(),
                content: "You are helpful.".to_string(),
            }],
            "hello",
        );
        ctx.mix_in_tool_result(metteur_shared::llm::ToolResult {
            tool: "ReadFile".to_string(),
            tool_call_id: "call_1".to_string(),
            content: "42".to_string(),
            timestamp: 0,
            lifetime: metteur_shared::llm::ToolResultLifetime::OneShot,
            paths: Vec::new(),
        });
        let input = build_input(&ctx);
        assert_eq!(input.len(), 2);
        assert_eq!(input[1]["type"], "function_call_output");
        assert_eq!(input[1]["call_id"], "call_1");
    }

    #[test]
    fn parses_response_with_function_call() {
        let json = json!({
            "output": [
                { "type": "function_call", "call_id": "call_1", "name": "ReadFile", "arguments": "{\"path\":\"a.txt\"}" }
            ],
            "usage": { "input_tokens": 10, "output_tokens": 5 }
        });
        let resp = parse_response(&json).unwrap();
        assert_eq!(resp.tool_calls.len(), 1);
        assert_eq!(resp.tool_calls[0].name, "ReadFile");
        assert_eq!(resp.usage.input_tokens, 10);
    }

    #[test]
    fn usage_exposes_cached_tokens() {
        let usage = parse_usage(&json!({
            "input_tokens": 500,
            "output_tokens": 10,
            "input_tokens_details": { "cached_tokens": 400 }
        }));
        assert_eq!(usage.input_tokens, 500);
        assert_eq!(usage.cached_input_tokens, 400);
        assert_eq!(usage.uncached_input_tokens(), 100);
    }

    #[test]
    fn reasoning_summaries_are_collected() {
        let json = json!({
            "output": [
                { "type": "reasoning", "summary": [{ "type": "summary_text", "text": "thought" }] },
                { "type": "message", "content": [{ "type": "output_text", "text": "answer" }] }
            ],
            "usage": { "input_tokens": 1, "output_tokens": 1 }
        });
        let resp = parse_response(&json).unwrap();
        assert_eq!(resp.text, "answer");
        assert_eq!(resp.thinking.len(), 1);
        assert_eq!(resp.thinking[0].text, "thought");
    }

    #[test]
    fn applies_reasoning_effort() {
        let client = OpenAiResponsesClient::new(
            reqwest::Client::new(),
            &LlmProviderConfig::new(
                super::super::super::ProviderKind::OpenAiResponses,
                "https://api.openai.com",
                "key",
                "gpt-5",
            ),
        );
        let ctx = ContextManager::new_from_prompt(vec![], "hi");
        let params = GenerationParams {
            reasoning_effort: Some(ReasoningEffort::High),
            ..Default::default()
        };
        let body = client.build_body(&ctx, &params, &[]);
        assert_eq!(body["reasoning"]["effort"], "high");
    }
    #[test]
    fn usage_distinguishes_missing_cache_from_zero() {
        let mut raw = json!({"input_tokens": 100, "output_tokens": 10});
        assert_eq!(parse_usage(&raw).cache_hit_rate(), None);
        raw["input_tokens_details"] = json!({"cached_tokens": 0});
        assert_eq!(parse_usage(&raw).cache_hit_rate(), Some(0.0));
        assert_eq!(parse_usage(&json!({})).cache_hit_rate(), None);
    }

}
