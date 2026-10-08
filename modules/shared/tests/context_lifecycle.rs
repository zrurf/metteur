//! Tests for context-manager lifecycle: eviction, staleness and compression
//! boundaries, plus token estimation.

use metteur_shared::llm::token::MESSAGE_ENVELOPE_TOKENS;
use metteur_shared::llm::{
    ContentBlock, ContextManager, EvictionPolicy, Message, Role, ToolCall, ToolResult,
    ToolResultLifetime, estimate_context_tokens, estimate_message, estimate_tokens,
};

fn tool_result(id: &str, paths: &[&str], lifetime: ToolResultLifetime) -> ToolResult {
    ToolResult {
        tool_call_id: id.to_string(),
        tool: "ReadFile".to_string(),
        content: format!("content of {id}"),
        timestamp: 1,
        lifetime,
        paths: paths.iter().map(std::path::PathBuf::from).collect(),
    }
}

/// Builds an assistant message that calls one tool.
fn assistant_with_call(id: &str, name: &str) -> Message {
    Message {
        role: Role::Assistant,
        content: vec![ContentBlock::Text("working".to_string())],
        tool_calls: vec![ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments: serde_json::json!({}),
        }],
        tool_call_id: None,
    }
}

/// Asserts every tool call has exactly one matching tool response.
fn assert_pairing(context: &ContextManager) {
    let calls: Vec<&str> =
        context.messages.iter().flat_map(|m| m.tool_calls.iter().map(|c| c.id.as_str())).collect();
    let results: Vec<&str> =
        context.messages.iter().filter_map(|m| m.tool_call_id.as_deref()).collect();
    for call in &calls {
        assert!(results.contains(call), "tool call {call} lost its response");
    }
    for result in results {
        assert!(calls.contains(&result), "tool response {result} lost its call");
    }
}

#[test]
fn evicted_results_keep_the_conversation_valid() {
    let mut context = ContextManager::new_from_prompt(vec![], "go");
    for index in 0..5 {
        let id = format!("call_{index}");
        context.push_message(assistant_with_call(&id, "ReadFile"));
        context.mix_in_tool_result(tool_result(&id, &[], ToolResultLifetime::OneShot));
    }
    context.evict(EvictionPolicy::Default, 2);
    assert_eq!(context.tool_results.len(), 2);
    assert_pairing(&context);
    // Exactly three responses were replaced by the marker.
    let markers = context
        .messages
        .iter()
        .filter(|m| m.role == Role::Tool && m.text_content().contains("evicted"))
        .count();
    assert_eq!(markers, 3);
}

#[test]
fn persistent_reads_outlive_one_shot_mutations() {
    let mut context = ContextManager::new_from_prompt(vec![], "go");
    context.push_message(assistant_with_call("read_1", "ReadFile"));
    context.mix_in_tool_result(tool_result("read_1", &["src/a.rs"], ToolResultLifetime::Persistent));
    context.push_message(assistant_with_call("write_1", "WriteFile"));
    context.mix_in_tool_result(tool_result("write_1", &[], ToolResultLifetime::OneShot));
    context.evict(EvictionPolicy::Default, 1);
    // The one-shot mutation goes first even though the read is older.
    assert_eq!(context.tool_results.len(), 1);
    assert_eq!(context.tool_results[0].tool_call_id, "read_1");
}

#[test]
fn invalidate_paths_expires_reads_of_changed_files() {
    let mut context = ContextManager::new_from_prompt(vec![], "go");
    context.push_message(assistant_with_call("read_a", "ReadFile"));
    context.mix_in_tool_result(tool_result("read_a", &["src/a.rs"], ToolResultLifetime::Persistent));
    context.push_message(assistant_with_call("read_b", "ReadFile"));
    context.mix_in_tool_result(tool_result("read_b", &["src/b.rs"], ToolResultLifetime::Persistent));

    let invalidated = context.invalidate_paths(&[std::path::PathBuf::from("src/a.rs")]);
    assert_eq!(invalidated, 1);
    assert_pairing(&context);
    let messages = &context.messages;
    let a = messages.iter().find(|m| m.tool_call_id.as_deref() == Some("read_a")).unwrap();
    assert!(a.text_content().contains("stale"), "{}", a.text_content());
    let b = messages.iter().find(|m| m.tool_call_id.as_deref() == Some("read_b")).unwrap();
    assert_eq!(b.text_content(), "content of read_b");
    // The stale entry leaves the eviction pool.
    assert_eq!(context.tool_results.len(), 1);
}

#[test]
fn invalidate_paths_ignores_unrelated_mutations() {
    let mut context = ContextManager::new_from_prompt(vec![], "go");
    context.push_message(assistant_with_call("read_a", "ReadFile"));
    context.mix_in_tool_result(tool_result("read_a", &["src/a.rs"], ToolResultLifetime::Persistent));
    let invalidated = context.invalidate_paths(&[std::path::PathBuf::from("src/other.rs")]);
    assert_eq!(invalidated, 0);
    assert_eq!(context.tool_results.len(), 1);
}

#[test]
fn pair_safe_split_never_orphans_a_tool_response() {
    let mut context = ContextManager::default();
    context.push_message(Message::text(Role::User, "q1"));
    context.push_message(assistant_with_call("call_1", "ReadFile"));
    context.mix_in_tool_result(tool_result("call_1", &[], ToolResultLifetime::OneShot));
    context.push_message(Message::text(Role::User, "q2"));
    // The naive boundary (2) would cut between the call and its response; the
    // helper moves it left so the call sequence travels with the summary.
    assert_eq!(context.pair_safe_split(2), 1);
    // Keeping only the last message summarizes the pair as a whole, which is
    // also valid: the boundary simply lands after the tool response.
    assert_eq!(context.pair_safe_split(1), 3);
    // A boundary before the pair needs no adjustment.
    assert_eq!(context.pair_safe_split(3), 1);
    assert_eq!(context.pair_safe_split(4), 0);
}

#[test]
fn compress_keeps_the_conversation_valid() {
    let mut context = ContextManager::default();
    context.push_message(Message::text(Role::User, "q1"));
    context.push_message(assistant_with_call("call_1", "ReadFile"));
    context.mix_in_tool_result(tool_result("call_1", &[], ToolResultLifetime::Persistent));
    context.push_message(Message::text(Role::User, "q2"));
    context.compress(2, |_| Some("summary".to_string()));
    assert_eq!(context.messages[0].text_content(), "summary");
    assert_pairing(&context);
    // The call moved into the kept tail, so its result stays available.
    assert_eq!(context.tool_results.len(), 1);
}

#[test]
fn compress_prunes_results_it_absorbed() {
    let mut context = ContextManager::default();
    context.push_message(assistant_with_call("call_1", "ReadFile"));
    context.mix_in_tool_result(tool_result("call_1", &[], ToolResultLifetime::Persistent));
    context.push_message(assistant_with_call("call_2", "ReadFile"));
    context.mix_in_tool_result(tool_result("call_2", &[], ToolResultLifetime::Persistent));
    context.push_message(Message::text(Role::User, "next"));
    // Five messages; keep the last two, which carry call_2 and its response.
    context.compress(2, |_| Some("summary".to_string()));
    assert_eq!(context.tool_results.len(), 1);
    assert_eq!(context.tool_results[0].tool_call_id, "call_2");
    assert_pairing(&context);
}

#[test]
fn estimate_tokens_counts_cjk_per_character() {
    assert_eq!(estimate_tokens(""), 0);
    // Four ASCII characters approximate one token.
    assert_eq!(estimate_tokens("abcd"), 1);
    assert_eq!(estimate_tokens("abcdefgh"), 2);
    // Non-ASCII characters count individually.
    assert_eq!(estimate_tokens("中文"), 2);
    assert!(estimate_tokens("中文中文") >= 4);
}

#[test]
fn estimate_message_includes_tool_calls_and_thinking() {
    let plain = Message::text(Role::User, "hello");
    // Every message carries a wire envelope the provider bills even though it is
    // not part of the content, so the estimate is the content plus that overhead.
    assert_eq!(
        estimate_message(&plain),
        estimate_tokens("hello") + MESSAGE_ENVELOPE_TOKENS
    );

    let thinking = Message {
        role: Role::Assistant,
        content: vec![
            ContentBlock::Thinking {
                text: "reasoning here".to_string(),
                signature: None,
            },
            ContentBlock::Text("answer".to_string()),
        ],
        tool_calls: Vec::new(),
        tool_call_id: None,
    };
    assert!(estimate_message(&thinking) > estimate_tokens("answer"));

    let calling = assistant_with_call("call_1", "ReadFile");
    assert!(estimate_message(&calling) >= estimate_tokens("working") + estimate_tokens("ReadFile"));
}

#[test]
fn estimate_context_covers_system_and_messages() {
    let context = ContextManager::new_from_prompt(
        vec![metteur_shared::llm::SystemFragment {
            priority: 0,
            scope: "test".to_string(),
            content: "you are a helpful assistant".to_string(),
        }],
        "hello world",
    );
    let total = estimate_context_tokens(&context);
    assert_eq!(
        total,
        estimate_tokens("you are a helpful assistant")
            + estimate_tokens("hello world")
            + MESSAGE_ENVELOPE_TOKENS
    );
}

#[test]
fn text_content_excludes_thinking() {
    let message = Message {
        role: Role::Assistant,
        content: vec![
            ContentBlock::Thinking {
                text: "internal reasoning".to_string(),
                signature: Some("sig".to_string()),
            },
            ContentBlock::Text("visible".to_string()),
        ],
        tool_calls: Vec::new(),
        tool_call_id: None,
    };
    assert_eq!(message.text_content(), "visible");
    assert_eq!(message.thinking_text(), "internal reasoning");
    assert!(message.has_thinking());
}

#[test]
fn thinking_signatures_survive_roundtrip_and_are_counted_once() {
    for signature in [None, Some(String::new()), Some("abcd".into()), Some("x".repeat(4096))] {
        let mut message = assistant_with_call("call_1", "ReadFile");
        let unsigned = estimate_message(&message);
        message.content.push(ContentBlock::Thinking { text: "reason".into(), signature: signature.clone() });
        message.content.push(ContentBlock::RedactedThinking { data: "opaque".into() });
        let expected = unsigned + estimate_tokens("reason") + estimate_tokens("opaque")
            + signature.as_deref().map(estimate_tokens).unwrap_or(0);
        let wire = serde_json::to_string(&message).unwrap();
        let replay: Message = serde_json::from_str(&wire).unwrap();
        assert_eq!(serde_json::to_string(&replay).unwrap(), wire);
        assert_eq!(estimate_message(&replay), expected);
        let mut context = ContextManager::default();
        context.push_message(replay.clone());
        context.push_message(replay);
        assert_eq!(estimate_context_tokens(&context), expected * 2);
    }
}

#[test]
fn usage_cache_breakdown_is_consistent() {
    let usage = metteur_shared::Usage {
        tokens_reported: true,
        cache_read_reported: true,
        input_tokens: 1000,
        cached_input_tokens: 600,
        cache_write_input_tokens: 100,
        ..Default::default()
    };
    assert_eq!(usage.uncached_input_tokens(), 300);
    assert_eq!(usage.cache_hit_rate(), Some(0.6));
    assert_eq!(metteur_shared::Usage::default().cache_hit_rate(), None);
}

#[test]
fn fragments_sort_by_priority_then_scope() {
    let make = |priority: i32, scope: &str, content: &str| metteur_shared::llm::SystemFragment {
        priority,
        scope: scope.to_string(),
        content: content.to_string(),
    };
    // Deliberately out of order, with a tie on priority.
    let context = ContextManager {
        system_fragments: vec![
            make(0, "zzz", "last"),
            make(10, "beta", "second"),
            make(10, "alpha", "first"),
            make(-5, "drop", "never"),
        ],
        ..Default::default()
    };
    let ordered: Vec<&str> =
        context.ordered_fragments().iter().map(|f| f.content.as_str()).collect();
    assert_eq!(ordered, vec!["first", "second", "last", "never"]);
    assert!(context.system_text().starts_with("first\n\nsecond"));
}

#[test]
fn fragment_order_is_independent_of_insertion_order() {
    let make = |priority: i32, scope: &str, content: &str| metteur_shared::llm::SystemFragment {
        priority,
        scope: scope.to_string(),
        content: content.to_string(),
    };
    let a = make(5, "tools", "tool rules");
    let b = make(5, "persona", "you are helpful");
    let c = make(0, "env", "workspace facts");

    let first = ContextManager {
        system_fragments: vec![a.clone(), b.clone(), c.clone()],
        ..Default::default()
    };
    // Addon loading order is not guaranteed, so the same set must render the
    // same text regardless of how it was collected.
    let second = ContextManager {
        system_fragments: vec![c, a, b],
        ..Default::default()
    };
    assert_eq!(first.system_text(), second.system_text());
}
