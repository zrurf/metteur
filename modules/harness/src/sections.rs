//! Static harness prompt sections.
//!
//! The texts live in `prompts/*.md` and are embedded at compile time, so a
//! released binary always carries the exact prompt revision it was built with.
//! Editing a section is a prompt change: it invalidates the stored prefix of
//! running sessions once, by design (see `HarnessPrompt::refresh`).
//!
//! Priorities decide the rendered order (higher first) and are grouped by how
//! often each section can change, so a change invalidates as little of the
//! provider prefix cache as possible: compiled-in rules first, then
//! per-session facts, then per-call instructions.

use metteur_shared::llm::SystemFragment;

/// Scope prefix of every harness-owned fragment.
pub const SCOPE_PREFIX: &str = "harness.";

/// The agent identity and operating principles.
pub const IDENTITY: &str = include_str!("prompts/identity.md");

/// Tool usage rules.
pub const TOOLS: &str = include_str!("prompts/tools.md");

/// Coding standards.
pub const CODING: &str = include_str!("prompts/coding.md");

/// Plan/todo discipline.
pub const PLAN: &str = include_str!("prompts/plan.md");

/// How to verify work, and how much reflection is expected.
pub const VERIFY: &str = include_str!("prompts/verify.md");

/// How to narrate progress so the user can follow a long run.
pub const PROGRESS: &str = include_str!("prompts/progress.md");

/// The summarization prompt for context compression.
pub const COMPRESS: &str = include_str!("prompts/compress.md");

/// Identity renders first.
pub const PRIORITY_IDENTITY: i32 = 100;
/// Tool rules follow the identity.
pub const PRIORITY_TOOLS: i32 = 90;
/// Coding standards.
pub const PRIORITY_CODING: i32 = 80;
/// Plan discipline.
pub const PRIORITY_PLAN: i32 = 70;
/// Verification discipline.
pub const PRIORITY_VERIFY: i32 = 65;
/// Progress narration.
pub const PRIORITY_PROGRESS: i32 = 60;
/// Project instructions from the workspace.
pub const PRIORITY_PROJECT: i32 = 50;
/// The environment block (workspace path, platform).
pub const PRIORITY_ENVIRONMENT: i32 = 40;
/// Date and model, the most volatile facts of the environment.
pub const PRIORITY_ENV_DATE: i32 = 39;
/// Task-level instructions written on a blueprint node.
pub const PRIORITY_NODE: i32 = 20;
/// Operator-appended instructions, rendered last.
pub const PRIORITY_APPEND: i32 = 10;

/// The sections every harness prompt carries, in render order.
pub const STATIC_SECTIONS: [(&str, i32, &str); 7] = [
    ("identity", PRIORITY_IDENTITY, IDENTITY),
    ("tools", PRIORITY_TOOLS, TOOLS),
        ("runtime_contract", 85, include_str!("prompts/runtime-contract.md")),
    ("coding", PRIORITY_CODING, CODING),
    ("plan", PRIORITY_PLAN, PLAN),
    ("verify", PRIORITY_VERIFY, VERIFY),
    ("progress", PRIORITY_PROGRESS, PROGRESS),
];

/// Builds the fragments of the constant sections.
pub fn static_fragments() -> Vec<SystemFragment> {
    STATIC_SECTIONS
        .into_iter()
        .map(|(name, priority, content)| SystemFragment {
            priority,
            scope: format!("{SCOPE_PREFIX}{name}"),
            content: content.trim_end().to_string(),
        })
        .collect()
}
