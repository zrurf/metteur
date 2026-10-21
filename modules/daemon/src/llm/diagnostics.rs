//! Content-free request diagnostics. Keyed fingerprints are comparable only
//! within the same host process; neither secrets nor raw prompts enter logs.
use metteur_shared::llm::{ContextManager, Usage};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::OnceLock;
static KEY: OnceLock<Option<[u8; 32]>> = OnceLock::new();
fn key() -> Option<&'static [u8; 32]> {
    KEY.get_or_init(|| {
        let mut key = [0; 32];
        getrandom::fill(&mut key).ok()?;
        Some(key)
    })
    .as_ref()
}
fn mac(key: &[u8], bytes: &[u8]) -> String {
    let mut inner = [0x36u8; 64];
    let mut outer = [0x5cu8; 64];
    let hashed;
    let key = if key.len() > 64 {
        hashed = Sha256::digest(key);
        hashed.as_slice()
    } else {
        key
    };
    for (i, k) in key.iter().enumerate() {
        inner[i] ^= k;
        outer[i] ^= k;
    }
    let mut first = Sha256::new();
    first.update(inner);
    first.update(bytes);
    let mut last = Sha256::new();
    last.update(outer);
    last.update(first.finalize());
    last.finalize().iter().map(|b| format!("{b:02x}")).collect()
}
fn fingerprint(value: &Value) -> Option<String> {
    Some(mac(key()?, &serde_json::to_vec(value).ok()?))
}
fn markers(value: &Value) -> usize {
    fn one(v: &Value) -> usize {
        usize::from(v.pointer("/cache_control/type").and_then(Value::as_str) == Some("ephemeral"))
    }
    fn blocks(v: Option<&Value>) -> usize {
        v.and_then(Value::as_array).map(|xs| xs.iter().map(one).sum()).unwrap_or(0)
    }
    one(value)
        + blocks(value.get("tools"))
        + blocks(value.get("system"))
        + value
            .get("messages")
            .and_then(Value::as_array)
            .map(|xs| xs.iter().map(|m| blocks(m.get("content"))).sum::<usize>())
            .unwrap_or(0)
}
pub fn request(endpoint: &str, body: &Value, context: &ContextManager) -> Value {
    let fragments:Vec<_>=context.ordered_fragments().into_iter().take(64).map(|f|json!({"category":if f.scope.starts_with("harness."){"harness"}else{"context"},"scope_digest":fingerprint(&json!(f.scope)),"content_digest":fingerprint(&json!(f.content)),"bytes":f.content.len(),"priority":f.priority})).collect();
    let roles:Vec<_>=context.messages.iter().rev().take(128).map(|m|json!({"role":m.role,"origin":m.origin,"tool_call_count":m.tool_calls.len(),"has_thinking":m.content.iter().any(|c|matches!(c,metteur_shared::llm::ContentBlock::Thinking{..}|metteur_shared::llm::ContentBlock::RedactedThinking{..}))})).collect();
    let prefix: Vec<_> = context
        .ordered_fragments()
        .into_iter()
        .take_while(|f| f.priority >= 40)
        .map(|f| json!({"scope":f.scope,"priority":f.priority,"content":f.content}))
        .collect();
    let tools = body.get("tools").cloned().unwrap_or(Value::Null);
    let system =
        body.get("system").or_else(|| body.get("instructions")).cloned().unwrap_or_else(|| {
            // Chat Completions embeds system entries in the message array.
            Value::Array(
                body.get("messages")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter(|m| {
                        matches!(
                            m.get("role").and_then(Value::as_str),
                            Some("system" | "developer")
                        )
                    })
                    .cloned()
                    .collect(),
            )
        });
    json!({"schema_version":1,"provenance":"host_derived","fingerprint_kind":"HMAC-SHA256","key_scope":"process_lifetime","key_id":key().map(|k|mac(k,b"diagnostic-key-id")),"request_digest":fingerprint(body),"endpoint_digest":fingerprint(&json!(endpoint)),"system_digest":fingerprint(&system),"stable_fragment_prefix_digest":fingerprint(&json!(prefix)),"tools_digest":fingerprint(&tools),"fragment_versions":fragments,"fragment_count":context.system_fragments.len(),"fragment_versions_truncated":context.system_fragments.len()>64,"message_count":context.messages.len(),"message_roles_truncated":context.messages.len()>128,"recent_message_roles_reverse":roles,"tool_count":tools.as_array().map_or(0,Vec::len),"cache_breakpoints":markers(body),"stream":body.get("stream").and_then(Value::as_bool).unwrap_or(false),"cache_condition":"unknown_unless_declared_by_eval","provider_cache_hit":"unknown_until_usage_receipt"})
}
pub fn usage(provider: &str, u: &Usage) -> Value {
    let (input, output, read, write) = match provider {
        "anthropic" => (
            "input_tokens + cache_read_input_tokens + cache_creation_input_tokens",
            "output_tokens",
            "cache_read_input_tokens",
            "cache_creation_input_tokens",
        ),
        "openai-responses" => (
            "input_tokens",
            "output_tokens",
            "input_tokens_details.cached_tokens",
            "input_tokens_details.cache_write_tokens",
        ),
        "openai-chat" => (
            "prompt_tokens",
            "completion_tokens",
            "prompt_tokens_details.cached_tokens",
            "prompt_tokens_details.cache_write_tokens",
        ),
        _ => ("unknown", "unknown", "unknown", "unknown"),
    };
    let consistent =
        u.cached_input_tokens.saturating_add(u.cache_write_input_tokens) <= u.input_tokens;
    json!({"normalized_input":{"value":u.tokens_reported.then_some(u.input_tokens),"source":input,"kind":if provider=="anthropic"{"host_derived_from_reported"}else{"provider_reported"}},
 "output":{"value":u.tokens_reported.then_some(u.output_tokens),"source":output},
 "cache_read":{"value":(u.cache_read_reported&&consistent).then_some(u.cached_input_tokens),"source":read},
 "cache_write":{"value":(u.cache_write_reported&&consistent).then_some(u.cache_write_input_tokens),"source":write},
 "cache_counter_consistent":consistent,"known_cache_read_input":(u.tokens_reported&&u.cache_read_reported&&consistent).then_some(u.input_tokens),
 "coverage_unit":"per_call; aggregate only known-input receipts in hit-rate denominator"})
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keyed_diagnostics_preserve_structure_without_content() {
        assert_eq!(
            mac(&[0x0b; 20], b"Hi There"),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        let mut ctx = ContextManager::new_from_prompt(vec![], "");
        ctx.system_fragments.push(metteur_shared::llm::SystemFragment {
            priority: 1,
            scope: "secret-scope".into(),
            content: "private instruction".into(),
        });
        let body = json!({"system":"private instruction","messages":[{"role":"user","content":"sensitive-value"}],"tools":[]});
        let a = request("https://secret:credential@example.test/?token=hidden", &body, &ctx);
        let b = request("https://secret:credential@example.test/?token=hidden", &body, &ctx);
        assert_eq!(a, b);
        let text = a.to_string();
        for s in [
            "credential",
            "sensitive-value",
            "private instruction",
            "secret-scope",
            "hidden",
            "example.test",
        ] {
            assert!(!text.contains(s));
        }
        let mut changed = body.clone();
        changed["messages"][0]["content"] = json!("changed");
        let c = request("https://secret:credential@example.test/?token=hidden", &changed, &ctx);
        assert_ne!(a["request_digest"], c["request_digest"]);
        assert_eq!(a["system_digest"], c["system_digest"]);
    }
    #[test]
    fn missing_and_invalid_cache_are_not_reported_as_zero() {
        let mut u = Usage {
            tokens_reported: true,
            input_tokens: 100,
            output_tokens: 1,
            total_tokens: 101,
            ..Default::default()
        };
        assert!(usage("openai-chat", &u)["cache_write"]["value"].is_null());
        u.cache_read_reported = true;
        u.cache_write_reported = true;
        assert_eq!(usage("openai-chat", &u)["cache_write"]["value"], 0);
        u.cached_input_tokens = 90;
        u.cache_write_input_tokens = 20;
        assert!(usage("openai-chat", &u)["cache_read"]["value"].is_null());
    }
}

/// A configured estimate, never the provider invoice. Missing/ambiguous rates
/// or unknown cache partitions keep the cost unknown.
pub fn cost(
    model: Option<&metteur_shared::config::LlmModelConfig>,
    currency: &str,
    timezone: &str,
    usage: Option<&Usage>,
) -> Value {
    let Some(u) = usage else {
        return Value::Null;
    };
    let rates = model.and_then(|m| {
        crate::llm::billing::effective_pricing_at(m, u.input_tokens, timezone, chrono::Utc::now())
    });
    crate::llm::billing::cost(rates.as_ref(),currency,u).map(|c|json!({"micros":c.micros,"currency":c.currency,"source":"configured_estimate_not_invoice"})).unwrap_or(Value::Null)
}
