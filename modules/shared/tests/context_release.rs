//! Tests for model-driven context release, duplicate-read supersession and
//! text-history extraction.

use metteur_shared::llm::{
    ContentBlock, ContextManager, Message, ReleaseQuery, Role, ToolCall, ToolResult,
    ToolResultLifetime,
};

fn tool_result(tool: &str, id: &str, paths: &[&str], content: &str) -> ToolResult {
    ToolResult {
        tool_call_id: id.to_string(),
        tool: tool.to_string(),
        content: content.to_string(),
        timestamp: 1,
        lifetime: ToolResultLifetime::Persistent,
        paths: paths.iter().map(std::path::PathBuf::from).collect(),
    }
}

/// Builds a context holding `count` reads of `src/file{n}.rs`.
fn context_with_reads(count: usize) -> ContextManager {
    let mut context = ContextManager::new_from_prompt(vec![], "start");
    for index in 0..count {
        let path = format!("src/file{index}.rs");
        context.push_message(Message {
            origin: Default::default(),
            role: Role::Assistant,
            content: vec![ContentBlock::Text("reading".to_string())],
            tool_calls: vec![ToolCall {
                id: format!("call_{index}"),
                name: "ReadFile".to_string(),
                arguments: serde_json::json!({ "path": path }),
            }],
            tool_call_id: None,
        });
        context.mix_in_tool_result(tool_result(
            "ReadFile",
            &format!("call_{index}"),
            &[&path],
            &"x".repeat(100),
        ));
    }
    context
}

/// Every tool call must keep a matching tool response.
fn assert_pairing_intact(context: &ContextManager) {
    let results: Vec<&str> =
        context.messages.iter().filter_map(|m| m.tool_call_id.as_deref()).collect();
    for call in context.messages.iter().flat_map(|m| m.tool_calls.iter()) {
        assert!(results.contains(&call.id.as_str()), "call {} lost its response", call.id);
    }
}

#[test]
fn release_by_exact_path_rewrites_only_that_result() {
    let mut context = context_with_reads(3);
    let report = context.release(&ReleaseQuery {
        paths: vec!["src/file1.rs".to_string()],
        ..Default::default()
    });
    assert_eq!(report.released, 1);
    assert_eq!(report.retained, 2);
    assert_eq!(report.by_tool, vec![("ReadFile".to_string(), 1)]);
    assert!(report.freed_chars > 0);
    let messages: Vec<String> = context.messages.iter().map(|m| m.text_content()).collect();
    assert!(messages.iter().any(|text| text.contains("[released: ReadFile result")));
    assert_eq!(messages.iter().filter(|text| text.starts_with('x')).count(), 2);
    assert_pairing_intact(&context);
}

#[test]
fn release_accepts_forward_slash_patterns_for_windows_paths() {
    let mut context = ContextManager::new_from_prompt(vec![], "start");
    context.mix_in_tool_result(tool_result("ReadFile", "call_1", &[r"src\core\mod.rs"], "core"));
    context.mix_in_tool_result(tool_result("ReadFile", "call_2", &["docs/readme.md"], "docs"));
    let report = context.release(&ReleaseQuery {
        patterns: vec!["src/**/*.rs".to_string()],
        ..Default::default()
    });
    assert_eq!(report.released, 1);
    let remaining = &context.tool_results;
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].tool_call_id, "call_2");
}

#[test]
fn release_by_tool_matches_every_result_of_that_tool() {
    let mut context = ContextManager::new_from_prompt(vec![], "start");
    context.mix_in_tool_result(tool_result("Grep", "call_1", &["a.rs"], "grep one"));
    context.mix_in_tool_result(tool_result("ReadFile", "call_2", &["b.rs"], "read two"));
    context.mix_in_tool_result(tool_result("Grep", "call_3", &["c.rs"], "grep three"));
    let report = context.release(&ReleaseQuery {
        tools: vec!["Grep".to_string()],
        ..Default::default()
    });
    assert_eq!(report.released, 2);
    assert_eq!(report.retained, 1);
    assert_eq!(context.tool_results[0].tool_call_id, "call_2");
    assert_pairing_intact(&context);
}

#[test]
fn release_all_honors_keep_recent() {
    let mut context = context_with_reads(5);
    let report = context.release(&ReleaseQuery {
        all: true,
        keep_recent: 2,
        ..Default::default()
    });
    assert_eq!(report.released, 3);
    assert_eq!(report.retained, 2);
    // The newest two survive verbatim.
    let kept: Vec<&str> =
        context.tool_results.iter().map(|result| result.tool_call_id.as_str()).collect();
    assert_eq!(kept, vec!["call_3", "call_4"]);
    assert_pairing_intact(&context);
}

#[test]
fn release_without_matches_reports_zero() {
    let mut context = context_with_reads(2);
    let report = context.release(&ReleaseQuery {
        paths: vec!["nowhere.rs".to_string()],
        ..Default::default()
    });
    assert_eq!(report.released, 0);
    assert_eq!(report.retained, 2);
    assert!(report.by_tool.is_empty());
    assert_eq!(context.tool_results.len(), 2);
}

#[test]
fn release_ignores_malformed_patterns() {
    let mut context = context_with_reads(1);
    let report = context.release(&ReleaseQuery {
        patterns: vec!["[".to_string()],
        ..Default::default()
    });
    assert_eq!(report.released, 0, "a broken pattern must not abort the pass");
    assert_eq!(context.tool_results.len(), 1);
}

#[test]
fn notice_reports_the_outcome() {
    let mut context = context_with_reads(2);
    let report = context.release(&ReleaseQuery {
        all: true,
        keep_recent: 0,
        ..Default::default()
    });
    let notice = report.notice();
    assert!(notice.contains("released 2 tool result(s)"), "{notice}");
    assert!(notice.contains("ReadFile: 2"), "{notice}");

    let empty = context.release(&ReleaseQuery::default());
    assert!(empty.notice().contains("matched 0 tool results"));
}

#[test]
fn released_results_stop_matching_eviction() {
    let mut context = context_with_reads(4);
    context.release(&ReleaseQuery {
        all: true,
        keep_recent: 1,
        ..Default::default()
    });
    // Only the retained result can be evicted now.
    let removed = context.evict(metteur_shared::EvictionPolicy::Default, 0);
    assert_eq!(removed, 1);
    assert!(context.tool_results.is_empty());
}

#[test]
fn supersede_reads_replaces_an_earlier_full_read() {
    let mut context = ContextManager::new_from_prompt(vec![], "start");
    context.mix_in_tool_result(tool_result("ReadFile", "call_1", &["src/main.rs"], "old body"));
    let superseded =
        context.supersede_reads("ReadFile", &[std::path::PathBuf::from("src/main.rs")]);
    assert_eq!(superseded, 1);
    assert!(context.tool_results.is_empty());
    assert!(
        context.messages.iter().any(|m| m.text_content().contains("[superseded: a later read")),
        "the older read must be marked as superseded"
    );
}

#[test]
fn supersede_reads_ignores_other_tools_and_path_sets() {
    let mut context = ContextManager::new_from_prompt(vec![], "start");
    context.mix_in_tool_result(tool_result("Grep", "call_1", &["src/main.rs"], "grep hit"));
    context.mix_in_tool_result(tool_result(
        "ReadFile",
        "call_2",
        &["src/main.rs", "src/lib.rs"],
        "two files",
    ));
    // A single-file read does not supersede a multi-path grep or a wider read.
    let superseded =
        context.supersede_reads("ReadFile", &[std::path::PathBuf::from("src/main.rs")]);
    assert_eq!(superseded, 0);
    assert_eq!(context.tool_results.len(), 2);
}

#[test]
fn supersede_reads_requires_paths() {
    let mut context = context_with_reads(1);
    assert_eq!(context.supersede_reads("ReadFile", &[]), 0);
    assert_eq!(context.supersede_reads("", &[std::path::PathBuf::from("a.rs")]), 0);
    assert_eq!(context.tool_results.len(), 1);
}

#[test]
fn text_history_keeps_prose_and_drops_tool_traffic() {
    let mut context = ContextManager::new_from_prompt(vec![], "first question");
    context.push_message(Message::text(Role::Assistant, "first answer"));
    context.push_message(Message {
        origin: Default::default(),
        role: Role::Assistant,
        content: vec![ContentBlock::Text("calling".to_string())],
        tool_calls: vec![ToolCall {
            id: "call_1".to_string(),
            name: "ReadFile".to_string(),
            arguments: serde_json::json!({}),
        }],
        tool_call_id: None,
    });
    context.mix_in_tool_result(tool_result("ReadFile", "call_1", &["a.rs"], "content"));
    context.push_message(Message::text(Role::User, "second question"));

    let history = context.text_history(10_000);
    let texts: Vec<String> = history.iter().map(|m| m.text_content()).collect();
    assert_eq!(texts, vec!["first question", "first answer", "second question"]);
    assert!(history.iter().all(|m| m.tool_calls.is_empty() && m.tool_call_id.is_none()));
}

#[test]
fn text_history_truncates_from_the_tail() {
    let mut context = ContextManager::new_from_prompt(vec![], "x".repeat(400));
    context.push_message(Message::text(Role::Assistant, "y".repeat(400)));
    context.push_message(Message::text(Role::User, "tail"));
    // The newest message always survives; the 400-char turn above does not fit
    // the 50-token budget.
    let history = context.text_history(50);
    let texts: Vec<String> = history.iter().map(|m| m.text_content()).collect();
    assert_eq!(texts, vec!["tail"]);
}

#[test]
fn legacy_results_without_a_tool_name_still_release() {
    // Records persisted before `tool` existed deserialize with an empty name.
    let legacy: ToolResult = serde_json::from_value(serde_json::json!({
        "tool_call_id": "call_1",
        "content": "old record",
        "timestamp": 1,
        "lifetime": "OneShot",
        "paths": ["src/main.rs"],
    }))
    .unwrap();
    assert!(legacy.tool.is_empty());
    let mut context = ContextManager::new_from_prompt(vec![], "start");
    context.mix_in_tool_result(legacy);
    let report = context.release(&ReleaseQuery {
        paths: vec!["src/main.rs".to_string()],
        ..Default::default()
    });
    assert_eq!(report.released, 1);
    assert!(
        context.messages.iter().any(|m| m.text_content().contains("[released: tool result")),
        "an unnamed result still gets a marker"
    );
}

#[test]
fn text_history_merges_adjacent_turns_of_one_role() {
    // Dropping the tool traffic can leave two user turns in a row — the
    // parent's prompt and the sub-agent task. Providers require alternating
    // roles, so they have to become one turn.
    let mut context = ContextManager::new_from_prompt(vec![], "first");
    context.push_message(Message {
        origin: Default::default(),
        role: Role::Assistant,
        content: vec![ContentBlock::Text("calling".to_string())],
        tool_calls: vec![ToolCall {
            id: "call_1".to_string(),
            name: "ReadFile".to_string(),
            arguments: serde_json::json!({}),
        }],
        tool_call_id: None,
    });
    context.mix_in_tool_result(tool_result("ReadFile", "call_1", &["a.rs"], "content"));
    context.push_message(Message::text(Role::User, "second"));

    let history = context.text_history(10_000);
    assert_eq!(history.len(), 1, "{history:?}");
    assert_eq!(history[0].role, Role::User);
    assert_eq!(history[0].text_content(), "first

second");
}

#[test]
fn text_history_keeps_alternating_roles_untouched() {
    let mut context = ContextManager::new_from_prompt(vec![], "question");
    context.push_message(Message::text(Role::Assistant, "answer"));
    context.push_message(Message::text(Role::User, "follow-up"));
    let history = context.text_history(10_000);
    let pairs: Vec<(Role, String)> =
        history.iter().map(|m| (m.role, m.text_content())).collect();
    assert_eq!(
        pairs,
        vec![
            (Role::User, "question".to_string()),
            (Role::Assistant, "answer".to_string()),
            (Role::User, "follow-up".to_string()),
        ]
    );
}

#[test]
fn repair_tool_pairing_folds_a_stranded_assistant_text() {
    // The shape an older daemon wrote: the assistant text as its own message
    // between the tool call and its result, which providers reject.
    let mut context = ContextManager::new_from_prompt(vec![], "start");
    context.push_message(Message {
        origin: Default::default(),
        role: Role::Assistant,
        content: vec![ContentBlock::Text("let me look".to_string())],
        tool_calls: vec![ToolCall {
            id: "call_1".to_string(),
            name: "ReadFile".to_string(),
            arguments: serde_json::json!({ "path": "a.rs" }),
        }],
        tool_call_id: None,
    });
    context.push_message(Message::text(Role::Assistant, "let me look"));
    context.push_message(Message {
        origin: Default::default(),
        role: Role::Tool,
        content: vec![ContentBlock::Text("file body".to_string())],
        tool_calls: Vec::new(),
        tool_call_id: Some("call_1".to_string()),
    });

    let repaired = context.repair_tool_pairing();
    assert_eq!(repaired, 1);
    // The prompt, the calling turn and its result.
    assert_eq!(context.messages.len(), 3);
    assert_eq!(context.messages[1].tool_calls.len(), 1);
    assert_eq!(context.messages[1].text_content(), "let me look");
    assert_eq!(context.messages[2].role, Role::Tool);
}

#[test]
fn repair_tool_pairing_keeps_unalike_text() {
    // Text that the calling turn does not carry is preserved, only moved.
    let mut context = ContextManager::new_from_prompt(vec![], "start");
    context.push_message(Message {
        origin: Default::default(),
        role: Role::Assistant,
        content: vec![ContentBlock::Text("calling".to_string())],
        tool_calls: vec![ToolCall {
            id: "call_1".to_string(),
            name: "ReadFile".to_string(),
            arguments: serde_json::json!({}),
        }],
        tool_call_id: None,
    });
    context.push_message(Message::text(Role::Assistant, "additional context"));
    context.push_message(Message {
        origin: Default::default(),
        role: Role::Tool,
        content: vec![ContentBlock::Text("body".to_string())],
        tool_calls: Vec::new(),
        tool_call_id: Some("call_1".to_string()),
    });

    assert_eq!(context.repair_tool_pairing(), 1);
    assert_eq!(context.messages.len(), 3);
    let text = context.messages[1].text_content();
    assert!(text.contains("calling"), "{text}");
    assert!(text.contains("additional context"), "{text}");
}

#[test]
fn repair_tool_pairing_leaves_valid_conversations_alone() {
    let mut context = ContextManager::new_from_prompt(vec![], "start");
    context.push_message(Message {
        origin: Default::default(),
        role: Role::Assistant,
        content: vec![ContentBlock::Text("working".to_string())],
        tool_calls: vec![ToolCall {
            id: "call_1".to_string(),
            name: "ReadFile".to_string(),
            arguments: serde_json::json!({}),
        }],
        tool_call_id: None,
    });
    context.mix_in_tool_result(tool_result("ReadFile", "call_1", &["a.rs"], "body"));
    context.push_message(Message::text(Role::Assistant, "done"));

    let before = context.messages.clone();
    assert_eq!(context.repair_tool_pairing(), 0);
    assert_eq!(context.messages, before);
    // An assistant text turn that is *not* wedged into a tool batch stays put.
    assert_eq!(context.messages.len(), 4);
}
