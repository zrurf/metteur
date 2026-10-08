//! LLM client abstraction and provider factory.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use metteur_shared::llm::{ContextManager, GenerationParams, ToolCall, ToolDefinition, Usage};

use crate::error::{DaemonError, DaemonResult};

/// The result of a single LLM completion.
#[derive(Debug, Clone)]
pub struct LlmResponse {
    /// The generated text (empty if the model only made tool calls).
    pub text: String,
    /// Reasoning blocks produced alongside the answer.
    pub thinking: Vec<ThinkingBlock>,
    /// Tool calls requested by the model.
    pub tool_calls: Vec<ToolCall>,
    /// Token usage for the request.
    pub usage: Usage,
}

/// A reasoning block returned by a thinking-capable model.
///
/// Providers differ in how reasoning may be fed back: Anthropic verifies a
/// signature over the text and requires verbatim replay, DeepSeek can require
/// replayed `reasoning_content`, OpenAI's Responses API summarizes instead of
/// returning raw reasoning. The daemon stores the block either way so the UI
/// and audit trail can show it; each provider decides what to send back.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ThinkingBlock {
    /// The reasoning text as shown to the user.
    pub text: String,
    /// Provider signature binding `text` (Anthropic).
    pub signature: Option<String>,
    /// Opaque redacted payload replayed verbatim (Anthropic).
    pub redacted: Option<String>,
}

/// One streamed delta from a provider.
///
/// Reasoning text is kept separate from the answer: it is displayed to the
/// user but is not part of the assistant turn, and providers differ in whether
/// it may be replayed at all (see [`ThinkingBlock`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamDelta {
    /// Assistant answer text.
    Text(String),
    /// Reasoning/thinking text.
    Reasoning(String),
    /// A tool call the model is still writing arguments for.
    ///
    /// Emitted per chunk with the accumulated size, so a client can show that a
    /// large edit is being composed instead of going silent until the call
    /// executes (the tool itself only appears when its arguments are complete).
    ToolArgs {
        /// Tool name, as soon as the provider named the call.
        name: String,
        /// Argument bytes received for this call so far.
        bytes: usize,
    },
}

/// A client for a single LLM provider and model.
#[async_trait]
pub trait LlmClient: Send + Sync {
    /// The provider name (e.g. `openai-chat`, `anthropic`, `openai-responses`).
    fn provider(&self) -> &str;

    /// The model identifier.
    fn model(&self) -> &str;

    /// Performs a non-streaming completion.
    async fn complete(
        &self,
        ctx: &ContextManager,
        params: &GenerationParams,
        tools: &[ToolDefinition],
    ) -> DaemonResult<LlmResponse>;

    /// Performs a streaming completion, invoking `on_delta` for each delta as
    /// it arrives.
    async fn stream(
        &self,
        ctx: &ContextManager,
        params: &GenerationParams,
        tools: &[ToolDefinition],
        on_delta: &mut (dyn FnMut(StreamDelta) + Send),
    ) -> DaemonResult<LlmResponse>;
}

/// The kind of LLM provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    /// OpenAI Chat Completions (`/chat/completions`).
    OpenAiChat,
    /// Anthropic Messages (`/messages`).
    Anthropic,
    /// OpenAI Responses (`/responses`).
    OpenAiResponses,
}

/// Configuration for a single LLM provider.
#[derive(Debug, Clone)]
pub struct LlmProviderConfig {
    /// The provider kind.
    pub kind: ProviderKind,
    /// The base URL (without the API path).
    pub base_url: String,
    /// The API key.
    pub api_key: String,
    /// The model identifier.
    pub model: String,
    /// Default generation parameters applied to every request.
    pub default_params: GenerationParams,
    /// Extended-thinking budget in tokens (`0` disables thinking).
    pub thinking_budget_tokens: u64,
    /// Whether the provider should emit explicit prompt-cache breakpoints.
    pub prompt_cache: bool,
    /// Whether stored reasoning is replayed in later requests.
    pub replay_reasoning: bool,
}

impl LlmProviderConfig {
    /// Creates a config from a provider kind, base URL, API key and model.
    pub fn new(
        kind: ProviderKind,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            base_url: base_url.into(),
            api_key: api_key.into(),
            model: model.into(),
            default_params: GenerationParams::default(),
            thinking_budget_tokens: 0,
            prompt_cache: true,
            replay_reasoning: false,
        }
    }

    /// Sets the extended-thinking budget.
    pub fn with_thinking_budget(mut self, budget_tokens: u64) -> Self {
        self.thinking_budget_tokens = budget_tokens;
        self
    }

    /// Sets whether explicit prompt-cache breakpoints are emitted.
    pub fn with_prompt_cache(mut self, enabled: bool) -> Self {
        self.prompt_cache = enabled;
        self
    }

    /// Sets whether stored reasoning is replayed in later requests.
    pub fn with_reasoning_replay(mut self, enabled: bool) -> Self {
        self.replay_reasoning = enabled;
        self
    }

    /// Applies provider settings shared by main execution and bounded oversight.
    /// This does not copy tools, retry policies, fallback models or authority.
    pub fn with_model_settings(
        self,
        defaults: &metteur_shared::config::LlmConfig,
        model: Option<&metteur_shared::config::LlmModelConfig>,
    ) -> Self {
        let replay = model.and_then(|cfg| cfg.replay_reasoning)
            .unwrap_or_else(|| Self::is_deepseek_model(&self.model));
        self.with_thinking_budget(defaults.thinking_budget_tokens)
            .with_prompt_cache(defaults.prompt_cache)
            .with_reasoning_replay(replay)
    }

    /// Whether a model id names a DeepSeek-family model.
    ///
    /// Those endpoints require the chain of thought they produced to come back
    /// verbatim once tools are involved, and reject a request without it on
    /// some routes. Other OpenAI-compatible endpoints may not accept the field
    /// at all, which is why this is not a blanket default.
    pub fn is_deepseek_model(model: &str) -> bool {
        model.to_ascii_lowercase().contains("deepseek")
    }
}

/// Creates [`LlmClient`] instances for the supported providers.
#[derive(Clone, Default)]
pub struct LlmClientFactory {
    http: reqwest::Client,
    /// When set, every `create` call returns this client instead of building a
    /// provider. Tests use it to script model responses without HTTP.
    override_client: Option<Arc<dyn LlmClient>>,
    /// When set, `create` returns the client registered for the requested model
    /// id. Tests use it to script several models at once (fallback chains).
    override_by_model: HashMap<String, Arc<dyn LlmClient>>,
}

impl LlmClientFactory {
    /// Creates a factory with a shared HTTP client.
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::new(),
            override_client: None,
            override_by_model: HashMap::new(),
        }
    }

    /// Creates a factory that always returns `client`.
    pub fn with_override(client: Arc<dyn LlmClient>) -> Self {
        Self {
            http: reqwest::Client::new(),
            override_client: Some(client),
            override_by_model: HashMap::new(),
        }
    }

    /// Creates a factory that returns a scripted client per model id.
    pub fn with_model_overrides(clients: HashMap<String, Arc<dyn LlmClient>>) -> Self {
        Self {
            http: reqwest::Client::new(),
            override_client: None,
            override_by_model: clients,
        }
    }

    /// Creates a client for the given provider config.
    pub fn create(&self, config: &LlmProviderConfig) -> DaemonResult<Arc<dyn LlmClient>> {
        if let Some(client) = self.override_by_model.get(&config.model) {
            return Ok(client.clone());
        }
        if let Some(client) = &self.override_client {
            return Ok(client.clone());
        }
        let client: Arc<dyn LlmClient> = match config.kind {
            ProviderKind::OpenAiChat => Arc::new(
                crate::llm::provider::openai_chat::OpenAiChatClient::new(self.http.clone(), config),
            ),
            ProviderKind::Anthropic => Arc::new(
                crate::llm::provider::anthropic::AnthropicClient::new(self.http.clone(), config),
            ),
            ProviderKind::OpenAiResponses => {
                Arc::new(crate::llm::provider::openai_responses::OpenAiResponsesClient::new(
                    self.http.clone(),
                    config,
                ))
            }
        };
        Ok(client)
    }
}

/// Maps an HTTP error into a daemon error.
pub(crate) fn http_error(context: &str, err: reqwest::Error) -> DaemonError {
    DaemonError::LlmTransport(format!("{context}: {err}"))
}
