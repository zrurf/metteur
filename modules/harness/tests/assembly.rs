//! Assembly behaviour: which fragments a run carries, in what order, and what
//! they contain.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use metteur_harness::{EnvFacts, HarnessPrompt, sections};
use metteur_shared::config::{Config, LlmConfig, LlmModelConfig};
use metteur_shared::llm::{ContextManager, SystemFragment};

static SEQ: AtomicU32 = AtomicU32::new(0);

/// A unique temporary workspace root for one test.
fn temp_root(tag: &str) -> PathBuf {
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "metteur-harness-{tag}-{}-{seq}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).unwrap();
    root
}

/// Facts with a fixed date, so assertions never depend on the clock.
fn facts(root: &std::path::Path) -> EnvFacts {
    EnvFacts::new(root.to_path_buf(), "2026-01-02")
}

/// Rendered fragment scopes, in provider order.
fn scopes(fragments: &[SystemFragment]) -> Vec<String> {
    let context = ContextManager {
        system_fragments: fragments.to_vec(),
        ..Default::default()
    };
    context.ordered_fragments().into_iter().map(|f| f.scope.clone()).collect()
}

fn with_llm(llm: LlmConfig) -> Config {
    Config {
        llm,
        ..Default::default()
    }
}

#[test]
fn sections_render_in_priority_order() {
    let root = temp_root("order");
    let fragments = HarnessPrompt::fragments_for(&facts(&root), &Config::default());
    assert_eq!(
        scopes(&fragments),
        vec![
            "harness.identity",
            "harness.tools",
            "harness.runtime_contract",
            "harness.coding",
            "harness.plan",
            "harness.verify",
            "harness.progress",
            "harness.env.base",
            "harness.env.date",
        ]
    );
    assert!(fragments.iter().all(metteur_harness::is_harness_fragment));
    assert!(fragments.iter().all(|f| !f.content.trim().is_empty()));
}

#[test]
fn environment_reports_the_workspace_and_the_date_separately() {
    let root = temp_root("env");
    let fragments = HarnessPrompt::fragments_for(&facts(&root), &Config::default());
    let base = fragments.iter().find(|f| f.scope == "harness.env.base").expect("base fragment");
    assert!(base.content.contains(&root.display().to_string()));
    assert!(base.content.contains(std::env::consts::OS));
    // The volatile half must not leak into the stable one: that is what keeps a
    // date rollover from invalidating the whole cached prefix.
    assert!(!base.content.contains("2026-01-02"));

    let date = fragments.iter().find(|f| f.scope == "harness.env.date").expect("date fragment");
    assert!(date.content.contains("2026-01-02"));
}

#[test]
fn the_configured_model_is_named_in_the_date_fragment() {
    let root = temp_root("model");
    let config = with_llm(LlmConfig {
        default_model: Some("deepseek".to_string()),
        models: HashMap::from([(
            "deepseek".to_string(),
            LlmModelConfig {
                api_type: "anthropic".to_string(),
                ..Default::default()
            },
        )]),
        ..Default::default()
    });
    let facts = facts(&root).with_model(Some("deepseek (anthropic)".to_string()));
    let fragments = HarnessPrompt::fragments_for(&facts, &config);
    let date = fragments.iter().find(|f| f.scope == "harness.env.date").unwrap();
    assert!(date.content.contains("model: deepseek (anthropic)"), "{}", date.content);
}

#[test]
fn project_instructions_load_from_metteur_md_first() {
    let root = temp_root("project");
    std::fs::write(root.join("AGENTS.md"), "agents rules").unwrap();
    std::fs::write(root.join("METTEUR.md"), "metteur rules").unwrap();
    let fragments = HarnessPrompt::fragments_for(&facts(&root), &Config::default());
    let project = fragments.iter().find(|f| f.scope == "harness.project").expect("project");
    assert!(project.content.contains("metteur rules"));
    assert!(project.content.contains("source=\"METTEUR.md\""));
    assert!(!project.content.contains("agents rules"));
    // Project instructions are per-session facts: they render before the
    // environment so an environment change cannot invalidate them.
    let order = scopes(&fragments);
    let project_at = order.iter().position(|s| s == "harness.project").unwrap();
    let env_at = order.iter().position(|s| s == "harness.env.base").unwrap();
    assert!(project_at < env_at);
}

#[test]
fn project_instructions_fall_back_and_can_be_disabled() {
    let root = temp_root("project-fallback");
    std::fs::write(root.join("AGENTS.md"), "agents rules").unwrap();
    let fragments = HarnessPrompt::fragments_for(&facts(&root), &Config::default());
    let project = fragments.iter().find(|f| f.scope == "harness.project").unwrap();
    assert!(project.content.contains("agents rules"));

    let disabled = with_llm(LlmConfig {
        project_instructions: false,
        ..Default::default()
    });
    let fragments = HarnessPrompt::fragments_for(&facts(&root), &disabled);
    assert!(!fragments.iter().any(|f| f.scope == "harness.project"));
}

#[test]
fn project_instructions_are_truncated_at_the_byte_cap() {
    let root = temp_root("project-cap");
    std::fs::write(root.join("METTEUR.md"), "z".repeat(500)).unwrap();
    let config = with_llm(LlmConfig {
        project_instructions_max_bytes: 64,
        ..Default::default()
    });
    let fragments = HarnessPrompt::fragments_for(&facts(&root), &config);
    let project = fragments.iter().find(|f| f.scope == "harness.project").unwrap();
    assert!(project.content.contains("truncated"));
    assert!(project.content.len() < 300, "{}", project.content.len());
}

#[test]
fn project_instruction_names_cannot_escape_the_workspace() {
    let root = temp_root("project-escape");
    std::fs::write(root.join("METTEUR.md"), "workspace rules").unwrap();
    // A repository can write its own configuration; a traversal attempt must be
    // ignored instead of pulling an arbitrary file into the prompt.
    let config = with_llm(LlmConfig {
        project_instruction_files: vec![
            "../outside.md".to_string(),
            "sub/dir.md".to_string(),
            "..".to_string(),
            "METTEUR.md".to_string(),
        ],
        ..Default::default()
    });
    let fragments = HarnessPrompt::fragments_for(&facts(&root), &config);
    let project = fragments.iter().find(|f| f.scope == "harness.project").unwrap();
    assert!(project.content.contains("workspace rules"));

    let hostile = with_llm(LlmConfig {
        project_instruction_files: vec!["../outside.md".to_string()],
        ..Default::default()
    });
    let fragments = HarnessPrompt::fragments_for(&facts(&root), &hostile);
    assert!(!fragments.iter().any(|f| f.scope == "harness.project"));
}

#[test]
fn system_prompt_append_renders_last() {
    let root = temp_root("append");
    let config = with_llm(LlmConfig {
        system_prompt_append: Some("Always check the changelog first.".to_string()),
        ..Default::default()
    });
    let fragments = HarnessPrompt::fragments_for(&facts(&root), &config);
    assert_eq!(scopes(&fragments).last().map(String::as_str), Some("harness.append"));
}

#[test]
fn refresh_replaces_harness_fragments_and_keeps_foreign_ones() {
    let root = temp_root("refresh");
    let mut context = ContextManager::new_from_prompt(
        vec![
            SystemFragment {
                priority: 0,
                scope: "addon.custom".to_string(),
                content: "addon text".to_string(),
            },
            SystemFragment {
                priority: 50,
                scope: "harness.project".to_string(),
                content: "stale project rules".to_string(),
            },
        ],
        "hi",
    );
    let fragments = HarnessPrompt::fragments_for(&facts(&root), &Config::default());
    assert!(HarnessPrompt::refresh(&mut context, fragments.clone()));
    assert!(context.system_fragments.iter().any(|f| f.scope == "addon.custom"));
    assert!(!context.system_fragments.iter().any(|f| f.content == "stale project rules"));
    assert!(context.system_fragments.iter().any(|f| f.scope == "harness.identity"));

    // A second refresh with identical content must not touch the context: the
    // stored prefix has to stay byte-identical for the provider cache.
    let before = context.system_fragments.clone();
    assert!(!HarnessPrompt::refresh(&mut context, fragments));
    assert_eq!(context.system_fragments, before);
}

#[test]
fn refresh_detects_new_project_instructions() {
    let root = temp_root("refresh-project");
    let mut context = ContextManager::new_from_prompt(vec![], "hi");
    let fragments = HarnessPrompt::fragments_for(&facts(&root), &Config::default());
    assert!(HarnessPrompt::refresh(&mut context, fragments));

    std::fs::write(root.join("METTEUR.md"), "new rule: use tabs").unwrap();
    let fragments = HarnessPrompt::fragments_for(&facts(&root), &Config::default());
    assert!(HarnessPrompt::refresh(&mut context, fragments));
    assert!(
        context.system_fragments.iter().any(|f| f.content.contains("use tabs")),
        "editing the project file must reach the next prompt"
    );
}

#[test]
fn refresh_normalizes_drifted_fragment_priorities() {
    // A session stored by an older prompt revision may carry the same text with
    // a different priority; the rendered prompt would interleave differently
    // with addon fragments, so the refresh must not skip it.
    let root = temp_root("refresh-priority");
    let mut context = ContextManager::new_from_prompt(
        vec![
            SystemFragment {
                priority: 0,
                scope: "harness.identity".to_string(),
                content: sections::IDENTITY.trim_end().to_string(),
            },
            SystemFragment {
                priority: 55,
                scope: "addon.custom".to_string(),
                content: "addon text".to_string(),
            },
        ],
        "hi",
    );
    let fragments = HarnessPrompt::fragments_for(&facts(&root), &Config::default());
    assert!(HarnessPrompt::refresh(&mut context, fragments.clone()));
    let identity = context
        .system_fragments
        .iter()
        .find(|f| f.scope == "harness.identity")
        .expect("identity fragment");
    assert_eq!(identity.priority, sections::PRIORITY_IDENTITY);
    assert!(!HarnessPrompt::refresh(&mut context, fragments));
}

#[test]
fn apply_installs_the_harness_in_place_of_older_copies() {
    let root = temp_root("apply");
    let mut context = ContextManager::new_from_prompt(
        vec![SystemFragment {
            priority: 0,
            scope: "harness.identity".to_string(),
            content: "an older identity".to_string(),
        }],
        "hi",
    );
    let fragments = HarnessPrompt::fragments_for(&facts(&root), &Config::default());
    HarnessPrompt::apply(&mut context, fragments);
    let identity: Vec<&SystemFragment> = context
        .system_fragments
        .iter()
        .filter(|f| f.scope == "harness.identity")
        .collect();
    assert_eq!(identity.len(), 1, "the harness must not be duplicated");
    assert_eq!(identity[0].content, sections::IDENTITY.trim_end());
}
