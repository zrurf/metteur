//! Model-assisted approval for [`PermissionMode::Full`](super::mode::PermissionMode::Full).
//!
//! In `full` mode the user has delegated the decision for risky operations to a
//! model. The reviewer is deliberately narrow: it sees one operation, it is told
//! what the workspace is, and it answers `APPROVE` or `DENY`. Everything that
//! goes wrong — no model configured, a provider error, an answer that is neither
//! word — returns `None`, and the caller falls back to asking the user. A broken
//! approver must never become an open door.

use metteur_shared::llm::{ContextManager, Message, Role, SystemFragment};

use crate::execution::context::ExecutionContext;
use crate::llm::{LlmClient, LlmProviderConfig, ProviderKind};

/// Reviewer instructions. Written as rules rather than prose: the answer is
/// parsed, so it must stay a single word.
const REVIEWER: &str = "You review one operation an autonomous coding agent wants to run inside a \
user's workspace and decide whether it may proceed without asking the user.

Answer with exactly one word: APPROVE or DENY.

APPROVE when all of the following hold:
- the operation stays inside the workspace,
- it is not destructive outside the workspace (no writes to system paths, no \
  credential or key material, no disk/partition/registry/boot operations),
- it does not publish or transmit workspace content to a third party,
- it does not install system-wide software or change the user's environment \
  beyond the workspace.

DENY when any of those is unclear. Prefer DENY: the user can always run the \
operation themselves, and a denied step is cheap while an unwanted one is not.";

/// One operation awaiting review.
pub struct ReviewRequest<'a> {
    /// `command` or `file`.
    pub kind: &'a str,
    /// The command line, or the file path being written.
    pub subject: &'a str,
    /// Extra context for the reviewer (working directory, diff size, …).
    pub detail: &'a serde_json::Value,
}

/// Asks the configured model whether `request` may run.
///
/// Returns `None` when no verdict could be obtained.
pub async fn review(ctx: &ExecutionContext, request: &ReviewRequest<'_>) -> Option<bool> {
    let config = match &ctx.config {
        Some(config) => config.read().await.clone(),
        None => return None,
    };
    let client = match build_client(ctx, &config) {
        Some(client) => client,
        None => return None,
    };

    let prompt = format!(
        "Operation: {kind}\nSubject: {subject}\nWorkspace: {workspace}\nDetail: {detail}",
        kind = request.kind,
        subject = request.subject,
        workspace = ctx.workspace_root.display(),
        detail = request.detail,
    );
    let context = ContextManager::new_from_prompt(
        vec![SystemFragment {
            priority: 0,
            scope: "sandbox.approver".to_string(),
            content: REVIEWER.to_string(),
        }],
        prompt,
    );
    let started = std::time::Instant::now();
    let params=metteur_shared::llm::GenerationParams{max_tokens:Some(1024),..Default::default()};
    let call_id=uuid::Uuid::new_v4().to_string();
    let reserved=serde_json::to_vec(&context).ok()?.len() as u64+1024;
    if let Some(root)=&ctx.continuation{root.reserve_call(reserved).ok()?;}
    ctx.audit("llm.request",serde_json::json!({"run_id":ctx.run_id,"model":client.model()}));
    ctx.audit("llm.observation.begin",serde_json::json!({"call_id":call_id,"run_id":ctx.run_id,"role":"sandbox_reviewer","provider":client.provider(),"model":client.model(),"request":client.diagnostics(&context,&params,&[],false)}));
    let result=client.complete(&context,&params,&[]).await;
    if let Some(root)=&ctx.continuation{root.settle_call(&call_id,reserved,result.as_ref().ok().filter(|r|r.usage.tokens_reported).map(|r|r.usage.total_tokens)).ok()?;}
    let billing=crate::execution::react::billing_config(ctx).await;
    let cost=billing.as_ref().map(|(currency,tz,models)|crate::llm::diagnostics::cost(models.get(client.model()),currency,tz,result.as_ref().ok().map(|r|&r.usage))).unwrap_or(serde_json::Value::Null);
    ctx.audit("llm.observation.end",serde_json::json!({"call_id":call_id,"run_id":ctx.run_id,"role":"sandbox_reviewer","provider":client.provider(),"model":client.model(),"elapsed_ms":started.elapsed().as_millis() as u64,"ttft_ms":null,"cost":cost,"outcome":if result.is_ok(){"response"}else{"error"},"usage":result.as_ref().ok().map(|r|crate::llm::diagnostics::usage(client.provider(),&r.usage))}));
    let usage=result.as_ref().ok().map(|r|r.usage).unwrap_or_default();
    crate::execution::react::record_usage(ctx,client.as_ref(),&usage,&billing).await;
    let response=result.ok()?;
    let verdict = parse_verdict(&response.text);
    let elapsed_ms = started.elapsed().as_millis() as u64;
    ctx.audit(
        "sandbox.review",
        serde_json::json!({
            "kind": request.kind,
            "subject": request.subject,
            "model": client.model(),
            "verdict": verdict.map(|allow| if allow { "approve" } else { "deny" }),
            "elapsed_ms": elapsed_ms,
            "answer": response.text.trim(),
        }),
    );
    verdict
}

/// Reads `APPROVE` / `DENY` out of a model answer.
///
/// The first recognizable word wins, so a model that explains itself before
/// answering still works; an answer with neither word is not a decision.
fn parse_verdict(answer: &str) -> Option<bool> {
    for word in answer.split(|ch: char| !ch.is_ascii_alphabetic()) {
        match word.to_ascii_uppercase().as_str() {
            "APPROVE" | "APPROVED" | "ALLOW" => return Some(true),
            "DENY" | "DENIED" | "REJECT" | "REJECTED" => return Some(false),
            _ => {}
        }
    }
    None
}

/// Builds the client used for review.
///
/// The workspace's default model is used unless `[sandbox] approver_model`
/// names another one; a small model is usually enough for a verdict.
fn build_client(
    ctx: &ExecutionContext,
    config: &metteur_shared::config::Config,
) -> Option<std::sync::Arc<dyn LlmClient>> {
    let key = config
        .sandbox
        .approver_model
        .as_deref()
        .filter(|key| !key.is_empty())
        .or(config.llm.default_model.as_deref())
        .filter(|key| !key.is_empty())?;
    let model = config.llm.models.get(key)?;
    let kind = match model.api_type.as_str() {
        "anthropic" => ProviderKind::Anthropic,
        "openai-responses" => ProviderKind::OpenAiResponses,
        _ => ProviderKind::OpenAiChat,
    };
    let model_id =
        if model.model_id.is_empty() { key.to_string() } else { model.model_id.clone() };
    let provider = LlmProviderConfig::new(kind, model.api_endpoint.clone(), model.api_key.clone(), model_id)
        .with_reasoning_replay(
            model
                .replay_reasoning
                .unwrap_or_else(|| LlmProviderConfig::is_deepseek_model(key)),
        );
    ctx.llm_factory.create(&provider).ok()
}

/// A message shown to the model when an operation was denied.
pub fn denial_note(kind: &str, subject: &str) -> Message {
    Message::text(
        Role::User,
        format!(
            "[engine] the sandbox denied this {kind} and it did not run: {subject}\n\
             Do not retry it unchanged. Either explain why it is needed and ask the user, \
             or accomplish the goal another way."
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_verdict_from_an_answer() {
        assert_eq!(parse_verdict("APPROVE"), Some(true));
        assert_eq!(parse_verdict("deny"), Some(false));
        assert_eq!(parse_verdict("Deny. This writes outside the workspace."), Some(false));
        assert_eq!(parse_verdict("I will approve this one: APPROVE"), Some(true));
        assert_eq!(parse_verdict("maybe later"), None);
        assert_eq!(parse_verdict(""), None);
    }
}
