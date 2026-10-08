//! Configuration types shared between the daemon and clients.

pub mod acl;
pub mod oversight;
mod pricing;
mod layer;
pub use layer::ConfigLayer;

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

pub use acl::{AclConfig, AclRule};

/// LLM-related configuration.
///
/// The manual [`Default`] mirrors the serde defaults of every field: a config
/// built in code (tests, or a run without a loaded configuration) must behave
/// exactly like one parsed from an empty file. A derived `Default` would leave
/// every tuned scalar at zero and silently disable the features behind it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmConfig {
    /// Default model identifier.
    #[serde(default)]
    pub default_model: Option<String>,
    /// Default temperature.
    #[serde(default)]
    pub temperature: Option<f64>,
    /// Preferred model for SubAgent tool invocations.
    #[serde(default)]
    pub subagent_default_model: Option<String>,
    /// Per-model configuration, keyed by model identifier.
    ///
    /// Peak/off-peak pricing rules live with the model itself (not with the
    /// billing section, which only drives cost *display*).
    #[serde(default)]
    pub models: HashMap<String, LlmModelConfig>,
    /// Run read-only tool calls of one turn concurrently.
    #[serde(default = "default_true")]
    pub parallel_read_tools: bool,
    /// Fallback byte cap for a single tool result (`0` = unlimited).
    #[serde(default = "default_max_tool_result_bytes")]
    pub max_tool_result_bytes: u64,
    /// Tool results retained in a context before eviction (`0` = unbounded).
    #[serde(default = "default_max_tool_results")]
    pub max_tool_results: u64,
    /// Consecutive tool failures tolerated before the run aborts (`0` = off).
    #[serde(default = "default_tool_error_limit")]
    pub tool_error_limit: u32,
    /// Identical tool invocations tolerated before the run aborts (`0` = off).
    #[serde(default = "default_repeat_call_limit")]
    pub repeat_call_limit: u32,
    /// Replace reads of files modified afterwards with a stale marker.
    #[serde(default = "default_true")]
    pub stale_result_placeholders: bool,
    /// Compress when estimated tokens reach this fraction of the window
    /// (`0` disables window-driven compression).
    #[serde(default = "default_compress_at_ratio")]
    pub compress_at_ratio: f64,
    /// Token budget kept verbatim at the tail when compressing.
    #[serde(default = "default_compress_keep_tokens")]
    pub compress_keep_tokens: u64,
    /// Emit explicit prompt-cache breakpoints where the provider requires them.
    #[serde(default = "default_true")]
    pub prompt_cache: bool,
    /// Extended-thinking budget in tokens (`0` disables thinking).
    #[serde(default)]
    pub thinking_budget_tokens: u64,
    /// Anonymize thinking text; drops its signature, so providers that verify
    /// signatures reject the replayed block.
    #[serde(default)]
    pub anonymize_thinking: bool,
    /// Load a project instruction file into the harness system prompt.
    #[serde(default = "default_true")]
    pub project_instructions: bool,
    /// Project instruction file names probed at the workspace root, in order;
    /// the first one that exists wins.
    #[serde(default = "default_project_instruction_files")]
    pub project_instruction_files: Vec<String>,
    /// Byte cap for the project instruction file (`0` = no cap).
    #[serde(default = "default_project_instructions_max_bytes")]
    pub project_instructions_max_bytes: u64,
    /// Extra instructions appended after every other fragment.
    #[serde(default)]
    pub system_prompt_append: Option<String>,
    /// Warn the model when the iteration budget runs low.
    #[serde(default = "default_true")]
    pub budget_notice: bool,
    /// Extra tool-free turns granted to produce a final answer once the
    /// iteration budget is exhausted (`0` restores the old silent stop).
    #[serde(default = "default_wrap_up_iterations")]
    pub wrap_up_iterations: u32,
    /// Provider retry attempts for retryable failures (`0` disables retries).
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    /// Base delay of the exponential retry backoff.
    #[serde(default = "default_retry_base_delay_ms")]
    pub retry_base_delay_ms: u64,
    /// Upper bound of the retry backoff delay.
    #[serde(default = "default_retry_max_delay_ms")]
    pub retry_max_delay_ms: u64,
    /// Model keys tried in order once retries are exhausted.
    #[serde(default)]
    pub fallback_models: Vec<String>,
    /// Model used for context summarization; defaults to the active model.
    #[serde(default)]
    pub compress_model: Option<String>,
    /// Release the oldest tool results before falling back to LLM
    /// compression when the context crowds the window.
    #[serde(default = "default_true")]
    pub auto_release: bool,
    /// Tool results kept when the automatic release runs.
    #[serde(default = "default_auto_release_keep_results")]
    pub auto_release_keep_results: u64,
    /// Replace an earlier full read of the same paths when a file is read
    /// again.
    #[serde(default = "default_true")]
    pub dedup_reads: bool,
    /// Token cap for the conversation a SubAgent inherits.
    #[serde(default = "default_subagent_inherit_tokens")]
    pub subagent_inherit_tokens: u64,
}

fn default_true() -> bool {
    true
}

/// The configured permission mode, or the one the sandbox switch implies.
///
/// Implemented here rather than in the daemon because both the merge logic and
/// the daemon need the same answer.
pub fn effective_permission_mode(config: &SandboxConfig) -> &'static str {
    match config.mode.trim() {
        "ask" | "manual" | "confirm" => "ask",
        "sandbox" => "sandbox",
        "full" | "auto" | "bypass" => "full",
        // Unset: a disabled sandbox has no policy to consult, so nothing is
        // confirmed; an enabled one falls back to the whitelist.
        _ => {
            if config.enabled {
                "sandbox"
            } else {
                "full"
            }
        }
    }
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            default_model: None,
            temperature: None,
            subagent_default_model: None,
            models: HashMap::new(),
            parallel_read_tools: default_true(),
            max_tool_result_bytes: default_max_tool_result_bytes(),
            max_tool_results: default_max_tool_results(),
            tool_error_limit: default_tool_error_limit(),
            repeat_call_limit: default_repeat_call_limit(),
            stale_result_placeholders: default_true(),
            compress_at_ratio: default_compress_at_ratio(),
            compress_keep_tokens: default_compress_keep_tokens(),
            prompt_cache: default_true(),
            thinking_budget_tokens: 0,
            anonymize_thinking: false,
            project_instructions: default_true(),
            project_instruction_files: default_project_instruction_files(),
            project_instructions_max_bytes: default_project_instructions_max_bytes(),
            system_prompt_append: None,
            budget_notice: default_true(),
            wrap_up_iterations: default_wrap_up_iterations(),
            max_retries: default_max_retries(),
            retry_base_delay_ms: default_retry_base_delay_ms(),
            retry_max_delay_ms: default_retry_max_delay_ms(),
            fallback_models: Vec::new(),
            compress_model: None,
            auto_release: default_true(),
            auto_release_keep_results: default_auto_release_keep_results(),
            dedup_reads: default_true(),
            subagent_inherit_tokens: default_subagent_inherit_tokens(),
        }
    }
}

fn default_max_tool_result_bytes() -> u64 {
    16 * 1024
}

fn default_max_tool_results() -> u64 {
    24
}

fn default_tool_error_limit() -> u32 {
    5
}

fn default_repeat_call_limit() -> u32 {
    3
}

fn default_compress_at_ratio() -> f64 {
    0.8
}

fn default_compress_keep_tokens() -> u64 {
    8192
}

fn default_project_instruction_files() -> Vec<String> {
    vec!["METTEUR.md".to_string(), "AGENTS.md".to_string()]
}

fn default_project_instructions_max_bytes() -> u64 {
    8192
}

fn default_wrap_up_iterations() -> u32 {
    1
}

fn default_max_retries() -> u32 {
    2
}

fn default_retry_base_delay_ms() -> u64 {
    500
}

fn default_retry_max_delay_ms() -> u64 {
    8000
}

fn default_auto_release_keep_results() -> u64 {
    8
}

fn default_subagent_inherit_tokens() -> u64 {
    4096
}

/// Pricing of a single model: input/output/cache prices per million tokens.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelPricing {
    /// Cost per million input tokens.
    #[serde(default)]
    pub input_per_mtok: f64,
    /// Cost per million output tokens.
    #[serde(default)]
    pub output_per_mtok: f64,
    /// Cost per million *cache-hit* input tokens.
    #[serde(default)]
    pub cache_hit_per_mtok: f64,
    /// Cost per million tokens written into the prompt cache.
    #[serde(default)]
    pub cache_write_per_mtok: f64,
}

/// One pricing tier for "tiered" pricing: applies up to `max_input_tokens`
/// (no upper bound when `None`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PricingTier {
    /// Human label, e.g. `"<=200k"`.
    #[serde(default)]
    pub name: String,
    /// Upper bound on input tokens for this tier (`None` = unbounded, last tier).
    #[serde(default)]
    pub max_input_tokens: Option<u64>,
    #[serde(default)]
    pub prices: ModelPricing,
}

/// One peak/off-peak window with an *independent* price set.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PeakWindowPrice {
    /// 5-field cron (`min hour day month weekday`) marking the window start.
    #[serde(default)]
    pub cron: String,
    /// How long the window lasts, in minutes.
    #[serde(default)]
    pub duration_min: u64,
    #[serde(default)]
    pub prices: ModelPricing,
}

/// Pricing strategy of a model. Everything is optional; when `kind` is
/// `default` (or pricing is absent) the first `prices` table is used.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelPricingStrategy {
    /// `default` | `tiered` | `peak`
    #[serde(default = "default_pricing_kind")]
    pub kind: String,
    /// Flat prices (kind = "default").
    #[serde(default)]
    pub prices: Option<ModelPricing>,
    /// Tiered prices (kind = "tiered").
    #[serde(default)]
    pub tiers: Vec<PricingTier>,
    /// Window-external prices (kind = "peak").
    #[serde(default)]
    pub default_prices: Option<ModelPricing>,
    /// Per-window prices (kind = "peak"); each window sets its own prices.
    #[serde(default)]
    pub windows: Vec<PeakWindowPrice>,
}

fn default_pricing_kind() -> String {
    "default".to_string()
}

/// Merges a workspace boolean over a global value, treating `default` as
/// "not set" (the layers expose no presence information for scalars).
fn merge_bool(workspace: bool, global: bool, default: bool) -> bool {
    if workspace == default { global } else { workspace }
}

/// Merges a workspace `u64` over a global value, treating `default` as unset.
fn merge_u64(workspace: u64, global: u64, default: u64) -> u64 {
    if workspace == default { global } else { workspace }
}

/// Merges a workspace `u32` over a global value, treating `default` as unset.
fn merge_u32(workspace: u32, global: u32, default: u32) -> u32 {
    if workspace == default { global } else { workspace }
}

/// A complete model definition: connection, advanced options and pricing.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LlmModelConfig {
    /// Human-readable display name (defaults to the model ID when empty).
    #[serde(default)]
    pub display_name: String,
    /// `openai-chat` | `openai-responses` | `anthropic`.
    #[serde(default)]
    pub api_type: String,
    /// API endpoint URL.
    #[serde(default)]
    pub api_endpoint: String,
    /// Model ID sent in requests (required).
    #[serde(default)]
    pub model_id: String,
    /// API key used to authenticate.
    #[serde(default)]
    pub api_key: String,
    /// Input context window limit in tokens (advanced, optional).
    #[serde(default)]
    pub context_window_input: Option<u64>,
    /// Output context window limit in tokens (advanced, optional).
    #[serde(default)]
    pub context_window_output: Option<u64>,
    /// Whether the model accepts image inputs (advanced, optional).
    #[serde(default)]
    pub supports_vision: bool,
    /// Replay stored reasoning blocks in later requests (advanced, optional).
    ///
    /// DeepSeek-family models require their `reasoning_content` back verbatim
    /// once tools are in play, and replaying it also keeps the assistant turn
    /// byte-identical for prefix caching. Strict OpenAI-compatible endpoints
    /// reject the extra field, so the default is off and the daemon turns it on
    /// for model ids that name a DeepSeek model.
    #[serde(default)]
    pub replay_reasoning: Option<bool>,
    /// Pricing strategy (all prices optional).
    #[serde(default)]
    pub pricing: ModelPricingStrategy,
}

/// Billing configuration — only cost *display*, not per-model pricing rules.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BillingConfig {
    /// Currency code used for cost reporting (e.g. `"USD"`).
    #[serde(default)]
    pub currency: String,
    /// IANA time zone name used for cost reporting (default UTC).
    #[serde(default)]
    pub timezone: String,
}

/// Anonymization configuration.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AnonymizeConfig {
    /// Whether sensitive values are redacted before entering the LLM context.
    #[serde(default)]
    pub enabled: bool,
    /// Additional regex patterns treated as secrets.
    #[serde(default)]
    pub extra_patterns: Vec<String>,
}

/// Command sandbox configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SandboxConfig {
    /// Whether the sandbox is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Default permission mode: `ask`, `sandbox` or `full`.
    ///
    /// `ask` confirms every edit and command, `sandbox` runs edits inside the
    /// workspace and policy-approved commands without asking, and `full` sends
    /// the operations the predictor calls risky to a model before running them
    /// (falling back to the user when the model cannot answer). A client may
    /// override this per conversation.
    ///
    /// Empty — the default — derives the mode from `enabled`, so a
    /// configuration written before modes existed keeps its behaviour: a
    /// disabled sandbox means nothing asks, an enabled one means the whitelist
    /// decides.
    #[serde(default)]
    pub mode: String,
    /// Model key used for automatic approval in `full` mode.
    ///
    /// Empty selects the workspace's default model, which keeps the feature
    /// usable without extra configuration; a small model is usually enough.
    #[serde(default)]
    pub approver_model: Option<String>,
    /// Commands allowed without user approval (glob patterns).
    #[serde(default)]
    pub whitelist: Vec<String>,
    /// Commands always requiring approval even if whitelisted elsewhere.
    #[serde(default)]
    pub blacklist: Vec<String>,
    /// Seconds before an unanswered approval request is denied (0 = default).
    #[serde(default)]
    pub approval_timeout_secs: u64,
    /// Seconds before a spawned command is killed (0 = default).
    #[serde(default)]
    pub command_timeout_secs: u64,
}

/// Mirrors the serde defaults of every field.
///
/// A derived `Default` would leave `mode` empty, and an empty mode is not a
/// mode: a configuration built in code must behave exactly like one parsed from
/// an empty file (see the note on [`LlmConfig`]'s `Default`).
impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: String::new(),
            approver_model: None,
            whitelist: Vec::new(),
            blacklist: Vec::new(),
            approval_timeout_secs: 0,
            command_timeout_secs: 0,
        }
    }
}

/// Transport configuration for one MCP server connection.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// `"stdio"` or `"http"`.
    #[serde(default)]
    pub transport: String,
    /// Process argv for the stdio transport.
    #[serde(default)]
    pub command: Vec<String>,
    /// Extra environment variables for the stdio transport.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Server URL for the http transport.
    #[serde(default)]
    pub url: Option<String>,
    /// Whether this server is connected.
    #[serde(default)]
    pub enabled: bool,
}

/// MCP host configuration.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct McpConfig {
    /// Registered MCP servers keyed by alias.
    #[serde(default)]
    pub servers: HashMap<String, McpServerConfig>,
    /// Per-call timeout in seconds (0 = built-in default of 30).
    #[serde(default)]
    pub call_timeout_secs: u64,
}

/// Addon runtime configuration.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AddonConfig {
    /// Default plugin function call timeout in milliseconds
    /// (0 = built-in default of 30_000).
    #[serde(default)]
    pub call_timeout_ms: u64,
    /// Require a valid `signature.toml` for every installed addon.
    ///
    /// When disabled, unsigned packages are accepted (useful during local
    /// development).
    #[serde(default = "default_require_signature")]
    pub require_signature: bool,
    /// Base64-encoded Ed25519 public keys trusted for package signatures.
    ///
    /// Empty means any well-formed self-contained signature is accepted.
    #[serde(default)]
    pub signing_keys: Vec<String>,
}

fn default_require_signature() -> bool {
    true
}

/// One language server definition.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LspLanguageConfig {
    /// Language identifier (e.g. `"rust"`).
    #[serde(default)]
    pub id: String,
    /// Process argv launching the language server.
    #[serde(default)]
    pub command: Vec<String>,
    /// File extensions routed to this language (without the dot).
    #[serde(default)]
    pub extensions: Vec<String>,
}

/// LSP-related configuration.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LspConfig {
    /// Whether LSP integration is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Language server definitions.
    #[serde(default)]
    pub languages: Vec<LspLanguageConfig>,
    /// Merge window for repeated document syncs of the same file, in
    /// milliseconds (`0` syncs every revision).
    ///
    /// Agents edit in bursts; merging intermediate revisions keeps the
    /// language server from re-checking text that is about to change again.
    /// Explicit diagnostic requests always bypass the window.
    #[serde(default = "default_lsp_debounce_ms")]
    pub debounce_ms: u64,
    /// Run a diagnostics pass when a node finishes mutating files.
    #[serde(default)]
    pub check_on_node_end: bool,
}

/// Default document sync merge window.
fn default_lsp_debounce_ms() -> u64 {
    300
}

/// Versioning configuration.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct VersioningConfig {
    /// Whether automatic snapshotting is enabled.
    #[serde(default)]
    pub auto_snapshot: bool,
}

/// How the daemon registers itself to start with the host OS.
///
/// Deserialized from either a boolean (legacy `true`/`false`) or one of the
/// strings `off`/`login`/`service`, so existing configs stay valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AutostartMode {
    /// Do not register anything; a client starts the daemon on demand.
    #[default]
    Off,
    /// Register a per-user login auto-start entry.
    Login,
    /// Register the daemon as a managed OS service (Windows SCM). On other
    /// platforms this mode is unsupported.
    Service,
}

impl Serialize for AutostartMode {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let s = match self {
            AutostartMode::Off => "off",
            AutostartMode::Login => "login",
            AutostartMode::Service => "service",
        };
        serializer.serialize_str(s)
    }
}

impl<'de> Deserialize<'de> for AutostartMode {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;

        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = AutostartMode;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a boolean or one of `off`/`login`/`service`")
            }

            fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Self::Value, E> {
                Ok(if v {
                    AutostartMode::Login
                } else {
                    AutostartMode::Off
                })
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                match v.to_ascii_lowercase().as_str() {
                    "off" | "none" | "disabled" | "false" => Ok(AutostartMode::Off),
                    "login" | "auto" | "true" | "on" => Ok(AutostartMode::Login),
                    "service" => Ok(AutostartMode::Service),
                    _ => Err(E::custom(format!("invalid autostart mode: {v}"))),
                }
            }
        }

        deserializer.deserialize_any(Visitor)
    }
}

/// Execution-engine configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutionConfig {
    /// Consecutive validator/judge failures that trip the circuit breaker
    /// (`0` disables the breaker).
    #[serde(default)]
    pub circuit_break_after: u32,
    /// Default total attempts (including the first) for validators that do
    /// not set an explicit `data.retry.max_attempts`. `1` disables retry.
    #[serde(default = "default_validation_max_attempts")]
    pub validation_max_attempts: u32,
    /// Maximum body entries of one ForEach loop (`0` disables the guard).
    #[serde(default = "default_foreach_max_iterations")]
    pub foreach_max_iterations: u32,
    /// Default timeout of a blocking command in seconds (`0` = no limit).
    #[serde(default)]
    pub command_timeout_secs: u64,
    /// Output bytes retained per job (`0` = the built-in default).
    #[serde(default = "default_job_output_max_bytes")]
    pub job_output_max_bytes: u64,
    /// Trailing output lines shown for a job (`0` = the built-in default).
    #[serde(default = "default_job_tail_lines")]
    pub job_tail_lines: u64,
    /// Whether a ReAct turn parks until a running job finishes instead of
    /// ending, so the engine wakes the model with the result.
    #[serde(default = "default_true")]
    pub job_auto_wake: bool,
    /// Whether cancelling a run rolls its recorded file mutations back.
    ///
    /// Command side effects are outside the WAL and are never undone.
    #[serde(default = "default_true")]
    pub rollback_on_cancel: bool,
}

/// The manual [`Default`] mirrors the serde defaults field by field: a config
/// built in code must behave exactly like one parsed from an empty file.
impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            circuit_break_after: 0,
            validation_max_attempts: default_validation_max_attempts(),
            foreach_max_iterations: default_foreach_max_iterations(),
            command_timeout_secs: 0,
            job_output_max_bytes: default_job_output_max_bytes(),
            job_tail_lines: default_job_tail_lines(),
            job_auto_wake: true,
            rollback_on_cancel: true,
        }
    }
}

/// Default retained output per job.
fn default_job_output_max_bytes() -> u64 {
    256 * 1024
}

/// Default trailing lines shown for a job.
fn default_job_tail_lines() -> u64 {
    80
}

/// Default validator retry budget.
fn default_validation_max_attempts() -> u32 {
    1
}

/// Default ForEach iteration guard.
fn default_foreach_max_iterations() -> u32 {
    1000
}

/// Daemon process configuration.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DaemonConfig {
    /// How the daemon registers itself to start with the host OS.
    #[serde(default)]
    pub autostart: AutostartMode,
    /// Whether the daemon stays quiet until a client sends `WAKE`.
    #[serde(default)]
    pub wake: bool,
    /// Listen address overriding the CLI default when present.
    #[serde(default)]
    pub listen_addr: Option<String>,
}

/// The merged daemon configuration.
///
/// A workspace config overrides the global config on a per-key basis. The
/// `extra` field is reserved for future Addon-provided keys.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Config {
    /// Access control rules.
    #[serde(default)]
    pub acl: AclConfig,
    /// LLM settings.
    #[serde(default)]
    pub llm: LlmConfig,
    /// Billing settings.
    #[serde(default)]
    pub billing: BillingConfig,
    /// Anonymization settings.
    #[serde(default)]
    pub anonymize: AnonymizeConfig,
    /// Sandbox settings.
    #[serde(default)]
    pub sandbox: SandboxConfig,
    /// MCP host settings.
    #[serde(default)]
    pub mcp: McpConfig,
    /// Addon runtime settings.
    #[serde(default)]
    pub addon: AddonConfig,
    /// LSP settings.
    #[serde(default)]
    pub lsp: LspConfig,
    /// Versioning settings.
    #[serde(default)]
    pub versioning: VersioningConfig,
    /// Execution-engine settings.
    #[serde(default)]
    pub execution: ExecutionConfig,
    /// Daemon process settings.
    #[serde(default)]
    pub daemon: DaemonConfig,
    /// Extension keys for future Addon use.
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl Config {
    /// Merges `workspace` over `self`, returning a new config.
    ///
    /// Non-default fields from `workspace` replace the corresponding fields in
    /// `self`; collection-valued sections are replaced wholesale when
    /// non-empty; `extra` keys are merged with the workspace taking precedence.
    pub fn merge(&self, workspace: &Config) -> Config {
        Config {
            acl: if workspace.acl.rules.is_empty() {
                self.acl.clone()
            } else {
                workspace.acl.clone()
            },
            llm: LlmConfig {
                default_model: workspace
                    .llm
                    .default_model
                    .clone()
                    .or_else(|| self.llm.default_model.clone()),
                temperature: workspace.llm.temperature.or(self.llm.temperature),
                subagent_default_model: workspace
                    .llm
                    .subagent_default_model
                    .clone()
                    .or_else(|| self.llm.subagent_default_model.clone()),
                models: if workspace.llm.models.is_empty() {
                    self.llm.models.clone()
                } else {
                    workspace.llm.models.clone()
                },
                // Scalars cannot distinguish "unset" from "default", so a
                // workspace layer only overrides when it differs from the
                // built-in default (matching the boolean handling above).
                parallel_read_tools: merge_bool(
                    workspace.llm.parallel_read_tools,
                    self.llm.parallel_read_tools,
                    default_true(),
                ),
                max_tool_result_bytes: merge_u64(
                    workspace.llm.max_tool_result_bytes,
                    self.llm.max_tool_result_bytes,
                    default_max_tool_result_bytes(),
                ),
                max_tool_results: merge_u64(
                    workspace.llm.max_tool_results,
                    self.llm.max_tool_results,
                    default_max_tool_results(),
                ),
                tool_error_limit: merge_u32(
                    workspace.llm.tool_error_limit,
                    self.llm.tool_error_limit,
                    default_tool_error_limit(),
                ),
                repeat_call_limit: merge_u32(
                    workspace.llm.repeat_call_limit,
                    self.llm.repeat_call_limit,
                    default_repeat_call_limit(),
                ),
                stale_result_placeholders: merge_bool(
                    workspace.llm.stale_result_placeholders,
                    self.llm.stale_result_placeholders,
                    default_true(),
                ),
                compress_at_ratio: if workspace.llm.compress_at_ratio == default_compress_at_ratio() {
                    self.llm.compress_at_ratio
                } else {
                    workspace.llm.compress_at_ratio
                },
                compress_keep_tokens: merge_u64(
                    workspace.llm.compress_keep_tokens,
                    self.llm.compress_keep_tokens,
                    default_compress_keep_tokens(),
                ),
                prompt_cache: merge_bool(
                    workspace.llm.prompt_cache,
                    self.llm.prompt_cache,
                    default_true(),
                ),
                thinking_budget_tokens: if workspace.llm.thinking_budget_tokens == 0 {
                    self.llm.thinking_budget_tokens
                } else {
                    workspace.llm.thinking_budget_tokens
                },
                anonymize_thinking: workspace.llm.anonymize_thinking,
                project_instructions: merge_bool(
                    workspace.llm.project_instructions,
                    self.llm.project_instructions,
                    default_true(),
                ),
                project_instruction_files: if workspace.llm.project_instruction_files
                    == default_project_instruction_files()
                {
                    self.llm.project_instruction_files.clone()
                } else {
                    workspace.llm.project_instruction_files.clone()
                },
                project_instructions_max_bytes: merge_u64(
                    workspace.llm.project_instructions_max_bytes,
                    self.llm.project_instructions_max_bytes,
                    default_project_instructions_max_bytes(),
                ),
                system_prompt_append: workspace
                    .llm
                    .system_prompt_append
                    .clone()
                    .or_else(|| self.llm.system_prompt_append.clone()),
                budget_notice: merge_bool(
                    workspace.llm.budget_notice,
                    self.llm.budget_notice,
                    default_true(),
                ),
                wrap_up_iterations: merge_u32(
                    workspace.llm.wrap_up_iterations,
                    self.llm.wrap_up_iterations,
                    default_wrap_up_iterations(),
                ),
                max_retries: merge_u32(
                    workspace.llm.max_retries,
                    self.llm.max_retries,
                    default_max_retries(),
                ),
                retry_base_delay_ms: merge_u64(
                    workspace.llm.retry_base_delay_ms,
                    self.llm.retry_base_delay_ms,
                    default_retry_base_delay_ms(),
                ),
                retry_max_delay_ms: merge_u64(
                    workspace.llm.retry_max_delay_ms,
                    self.llm.retry_max_delay_ms,
                    default_retry_max_delay_ms(),
                ),
                fallback_models: if workspace.llm.fallback_models.is_empty() {
                    self.llm.fallback_models.clone()
                } else {
                    workspace.llm.fallback_models.clone()
                },
                compress_model: workspace
                    .llm
                    .compress_model
                    .clone()
                    .or_else(|| self.llm.compress_model.clone()),
                auto_release: merge_bool(
                    workspace.llm.auto_release,
                    self.llm.auto_release,
                    default_true(),
                ),
                auto_release_keep_results: merge_u64(
                    workspace.llm.auto_release_keep_results,
                    self.llm.auto_release_keep_results,
                    default_auto_release_keep_results(),
                ),
                dedup_reads: merge_bool(
                    workspace.llm.dedup_reads,
                    self.llm.dedup_reads,
                    default_true(),
                ),
                subagent_inherit_tokens: merge_u64(
                    workspace.llm.subagent_inherit_tokens,
                    self.llm.subagent_inherit_tokens,
                    default_subagent_inherit_tokens(),
                ),
            },
            billing: BillingConfig {
                currency: if workspace.billing.currency.is_empty() {
                    self.billing.currency.clone()
                } else {
                    workspace.billing.currency.clone()
                },
                timezone: if workspace.billing.timezone.is_empty() {
                    self.billing.timezone.clone()
                } else {
                    workspace.billing.timezone.clone()
                },
            },
            anonymize: AnonymizeConfig {
                enabled: workspace.anonymize.enabled || self.anonymize.enabled,
                extra_patterns: if workspace.anonymize.extra_patterns.is_empty() {
                    self.anonymize.extra_patterns.clone()
                } else {
                    workspace.anonymize.extra_patterns.clone()
                },
            },
            sandbox: SandboxConfig {
                enabled: workspace.sandbox.enabled || self.sandbox.enabled,
                // An overridden mode is what the workspace asked for; the
                // default value never shadows the global layer.
                mode: if workspace.sandbox.mode.trim().is_empty() {
                    self.sandbox.mode.clone()
                } else {
                    workspace.sandbox.mode.clone()
                },
                approver_model: workspace
                    .sandbox
                    .approver_model
                    .clone()
                    .or_else(|| self.sandbox.approver_model.clone()),
                whitelist: if workspace.sandbox.whitelist.is_empty() {
                    self.sandbox.whitelist.clone()
                } else {
                    workspace.sandbox.whitelist.clone()
                },
                blacklist: if workspace.sandbox.blacklist.is_empty() {
                    self.sandbox.blacklist.clone()
                } else {
                    workspace.sandbox.blacklist.clone()
                },
                approval_timeout_secs: if workspace.sandbox.approval_timeout_secs != 0 {
                    workspace.sandbox.approval_timeout_secs
                } else {
                    self.sandbox.approval_timeout_secs
                },
                command_timeout_secs: if workspace.sandbox.command_timeout_secs != 0 {
                    workspace.sandbox.command_timeout_secs
                } else {
                    self.sandbox.command_timeout_secs
                },
            },
            mcp: McpConfig {
                servers: if workspace.mcp.servers.is_empty() {
                    self.mcp.servers.clone()
                } else {
                    workspace.mcp.servers.clone()
                },
                call_timeout_secs: if workspace.mcp.call_timeout_secs != 0 {
                    workspace.mcp.call_timeout_secs
                } else {
                    self.mcp.call_timeout_secs
                },
            },
            addon: AddonConfig {
                call_timeout_ms: if workspace.addon.call_timeout_ms != 0 {
                    workspace.addon.call_timeout_ms
                } else {
                    self.addon.call_timeout_ms
                },
                require_signature: self.addon.require_signature,
                signing_keys: self.addon.signing_keys.clone(),
            },
            lsp: LspConfig {
                enabled: workspace.lsp.enabled || self.lsp.enabled,
                languages: if workspace.lsp.languages.is_empty() {
                    self.lsp.languages.clone()
                } else {
                    workspace.lsp.languages.clone()
                },
                debounce_ms: merge_u64(
                    workspace.lsp.debounce_ms,
                    self.lsp.debounce_ms,
                    default_lsp_debounce_ms(),
                ),
                check_on_node_end: workspace.lsp.check_on_node_end,
            },
            versioning: VersioningConfig {
                auto_snapshot: workspace.versioning.auto_snapshot || self.versioning.auto_snapshot,
            },
            execution: ExecutionConfig {
                circuit_break_after: if workspace.execution.circuit_break_after != 0 {
                    workspace.execution.circuit_break_after
                } else {
                    self.execution.circuit_break_after
                },
                validation_max_attempts: if workspace.execution.validation_max_attempts != 1 {
                    workspace.execution.validation_max_attempts
                } else {
                    self.execution.validation_max_attempts
                },
                foreach_max_iterations: if workspace.execution.foreach_max_iterations != 1000 {
                    workspace.execution.foreach_max_iterations
                } else {
                    self.execution.foreach_max_iterations
                },
                command_timeout_secs: if workspace.execution.command_timeout_secs != 0 {
                    workspace.execution.command_timeout_secs
                } else {
                    self.execution.command_timeout_secs
                },
                job_output_max_bytes: merge_u64(
                    workspace.execution.job_output_max_bytes,
                    self.execution.job_output_max_bytes,
                    default_job_output_max_bytes(),
                ),
                job_tail_lines: merge_u64(
                    workspace.execution.job_tail_lines,
                    self.execution.job_tail_lines,
                    default_job_tail_lines(),
                ),
                job_auto_wake: merge_bool(
                    workspace.execution.job_auto_wake,
                    self.execution.job_auto_wake,
                    default_true(),
                ),
                rollback_on_cancel: merge_bool(
                    workspace.execution.rollback_on_cancel,
                    self.execution.rollback_on_cancel,
                    default_true(),
                ),
            },
            daemon: DaemonConfig {
                autostart: if workspace.daemon.autostart != AutostartMode::Off {
                    workspace.daemon.autostart
                } else {
                    self.daemon.autostart
                },
                wake: workspace.daemon.wake || self.daemon.wake,
                listen_addr: workspace
                    .daemon
                    .listen_addr
                    .clone()
                    .or_else(|| self.daemon.listen_addr.clone()),
            },
            extra: {
                let mut extra = self.extra.clone();
                extra.extend(workspace.extra.clone());
                extra
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autostart_mode_accepts_legacy_bool_and_strings() {
        assert_eq!(
            serde_json::from_value::<AutostartMode>(serde_json::json!(true)).unwrap(),
            AutostartMode::Login
        );
        assert_eq!(
            serde_json::from_value::<AutostartMode>(serde_json::json!(false)).unwrap(),
            AutostartMode::Off
        );
        assert_eq!(
            serde_json::from_value::<AutostartMode>(serde_json::json!("service")).unwrap(),
            AutostartMode::Service
        );
    }

    #[test]
    fn autostart_mode_round_trips_to_string() {
        let v = serde_json::to_value(AutostartMode::Service).unwrap();
        assert_eq!(v, serde_json::json!("service"));
    }

    #[test]
    fn workspace_overrides_global() {
        let global = Config {
            llm: LlmConfig {
                default_model: Some("gpt-4".to_string()),
                temperature: Some(0.7),
                subagent_default_model: None,
                models: HashMap::new(),
                ..Default::default()
            },
            ..Default::default()
        };
        let workspace = Config {
            llm: LlmConfig {
                default_model: Some("claude".to_string()),
                temperature: None,
                subagent_default_model: None,
                models: HashMap::new(),
                ..Default::default()
            },
            ..Default::default()
        };
        let merged = global.merge(&workspace);
        assert_eq!(merged.llm.default_model.as_deref(), Some("claude"));
        assert_eq!(merged.llm.temperature, Some(0.7));
    }

    #[test]
    fn extra_keys_merge() {
        let global = Config {
            extra: HashMap::from([("a".to_string(), serde_json::json!(1))]),
            ..Default::default()
        };
        let workspace = Config {
            extra: HashMap::from([("b".to_string(), serde_json::json!(2))]),
            ..Default::default()
        };
        let merged = global.merge(&workspace);
        assert_eq!(merged.extra.len(), 2);
    }

    #[test]
    fn sandbox_lists_replace_wholesale() {
        let global = Config {
            sandbox: SandboxConfig {
                enabled: false,
                mode: "sandbox".to_string(),
                approver_model: None,
                whitelist: vec!["cargo *".to_string()],
                blacklist: vec!["rm *".to_string()],
                approval_timeout_secs: 0,
                command_timeout_secs: 30,
            },
            ..Default::default()
        };
        let workspace = Config {
            sandbox: SandboxConfig {
                enabled: true,
                mode: "sandbox".to_string(),
                approver_model: None,
                whitelist: vec!["node *".to_string()],
                blacklist: Vec::new(),
                approval_timeout_secs: 120,
                command_timeout_secs: 0,
            },
            ..Default::default()
        };
        let merged = global.merge(&workspace);
        assert_eq!(merged.sandbox.whitelist, vec!["node *".to_string()]);
        // Empty workspace list falls back to the global list.
        assert_eq!(merged.sandbox.blacklist, vec!["rm *".to_string()]);
        assert_eq!(merged.sandbox.approval_timeout_secs, 120);
        assert_eq!(merged.sandbox.command_timeout_secs, 30);
    }

    #[test]
    fn billing_and_llm_models_merge() {
        let global = Config {
            llm: LlmConfig {
                default_model: Some("gpt-4".to_string()),
                temperature: Some(0.7),
                subagent_default_model: None,
                models: HashMap::from([(
                    "deepseek".to_string(),
                    LlmModelConfig {
                        api_type: "openai-chat".to_string(),
                        api_endpoint: "https://api.deepseek.com".to_string(),
                        model_id: "deepseek-v3".to_string(),
                        api_key: String::new(),
                        pricing: ModelPricingStrategy {
                            kind: "default".to_string(),
                            prices: Some(ModelPricing {
                                input_per_mtok: 1.0,
                                output_per_mtok: 5.0,
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                )]),
                ..Default::default()
            },
            billing: BillingConfig {
                currency: "USD".to_string(),
                timezone: "UTC".to_string(),
            },
            ..Default::default()
        };
        let workspace = Config {
            billing: BillingConfig {
                currency: String::new(),
                timezone: "Asia/Shanghai".to_string(),
            },
            ..Default::default()
        };
        let merged = global.merge(&workspace);
        assert_eq!(merged.billing.currency, "USD");
        assert_eq!(merged.billing.timezone, "Asia/Shanghai");
        // Workspace has no models → the global model table is preserved.
        assert_eq!(merged.llm.models["deepseek"].model_id, "deepseek-v3");
    }

    #[test]
    fn mcp_servers_and_lsp_languages_replace() {
        let mut servers = HashMap::new();
        servers.insert(
            "fs".to_string(),
            McpServerConfig {
                transport: "stdio".to_string(),
                command: vec!["mcp-fs".to_string()],
                env: HashMap::new(),
                url: None,
                enabled: true,
            },
        );
        let global = Config {
            mcp: McpConfig {
                servers,
                call_timeout_secs: 0,
            },
            lsp: LspConfig {
                enabled: false,
                languages: vec![LspLanguageConfig {
                    id: "rust".to_string(),
                    command: vec!["rust-analyzer".to_string()],
                    extensions: vec!["rs".to_string()],
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        let mut ws_servers = HashMap::new();
        ws_servers.insert(
            "git".to_string(),
            McpServerConfig {
                transport: "stdio".to_string(),
                command: vec!["mcp-git".to_string()],
                env: HashMap::new(),
                url: None,
                enabled: true,
            },
        );
        let workspace = Config {
            mcp: McpConfig {
                servers: ws_servers,
                call_timeout_secs: 60,
            },
            addon: AddonConfig {
                call_timeout_ms: 10_000,
                require_signature: false,
                signing_keys: Vec::new(),
            },
            lsp: LspConfig {
                enabled: true,
                languages: Vec::new(),
                ..Default::default()
            },
            ..Default::default()
        };
        let merged = global.merge(&workspace);
        assert!(merged.mcp.servers.contains_key("git"));
        assert!(!merged.mcp.servers.contains_key("fs"));
        assert_eq!(merged.mcp.call_timeout_secs, 60);
        assert_eq!(merged.addon.call_timeout_ms, 10_000);
        assert_eq!(merged.lsp.languages.len(), 1);
        assert!(merged.lsp.enabled);
    }

    #[test]
    fn deserializes_mcp_timeout_and_addon_section() {
        let raw = r#"
[mcp]
call_timeout_secs = 45
[addon]
call_timeout_ms = 15000
"#;
        let cfg: Config = toml::from_str(raw).expect("parses toml");
        assert_eq!(cfg.mcp.call_timeout_secs, 45);
        assert_eq!(cfg.addon.call_timeout_ms, 15_000);
    }

    #[test]
    fn deserializes_new_sections_from_toml() {
        let raw = r#"
[billing]
currency = "USD"
timezone = "Asia/Shanghai"

[llm.models.deepseek]
display_name = "DeepSeek"
api_type = "openai-chat"
api_endpoint = "https://api.deepseek.com"
model_id = "deepseek-v4-pro"
api_key = "sk-test"

[llm.models.deepseek.pricing]
kind = "peak"
[llm.models.deepseek.pricing.default_prices]
input_per_mtok = 1.0
output_per_mtok = 5.0
[[llm.models.deepseek.pricing.windows]]
cron = "0 9-11 * * 1-5"
duration_min = 180
[llm.models.deepseek.pricing.windows.prices]
input_per_mtok = 3.0
output_per_mtok = 12.0
cache_hit_per_mtok = 0.3

[sandbox]
enabled = true
whitelist = ["cargo *"]
blacklist = ["format*"]

[anonymize]
enabled = true

[execution]
circuit_break_after = 5

[mcp.servers.filesystem]
transport = "stdio"
command = ["mcp-server-fs"]
enabled = true

[[lsp.languages]]
id = "rust"
command = ["rust-analyzer"]
extensions = ["rs"]
"#;
        let cfg: Config = toml::from_str(raw).expect("parses toml");
        assert_eq!(cfg.billing.timezone, "Asia/Shanghai");
        let ds = &cfg.llm.models["deepseek"];
        assert_eq!(ds.api_type, "openai-chat");
        assert_eq!(ds.pricing.kind, "peak");
        assert_eq!(ds.pricing.windows.len(), 1);
        assert_eq!(ds.pricing.windows[0].duration_min, 180);
        assert_eq!(ds.pricing.windows[0].prices.input_per_mtok, 3.0);
        assert_eq!(ds.pricing.default_prices.as_ref().unwrap().output_per_mtok, 5.0);
        assert!(cfg.sandbox.enabled);
        assert!(cfg.anonymize.enabled);
        assert_eq!(cfg.execution.circuit_break_after, 5);
        assert!(cfg.mcp.servers.contains_key("filesystem"));
        assert_eq!(cfg.lsp.languages[0].id, "rust");
    }

    #[test]
    fn execution_section_merges_by_field() {
        let global = Config {
            execution: ExecutionConfig {
                circuit_break_after: 3,
                validation_max_attempts: 2,
                foreach_max_iterations: 100,
                ..Default::default()
            },
            ..Default::default()
        };
        let workspace = Config {
            execution: ExecutionConfig {
                circuit_break_after: 0,
                validation_max_attempts: 1,
                foreach_max_iterations: 1000,
                ..Default::default()
            },
            ..Default::default()
        };
        let merged = global.merge(&workspace);
        assert_eq!(merged.execution.circuit_break_after, 3);
        assert_eq!(merged.execution.validation_max_attempts, 2);
        assert_eq!(merged.execution.foreach_max_iterations, 100);
        let workspace = Config {
            execution: ExecutionConfig {
                circuit_break_after: 7,
                validation_max_attempts: 5,
                foreach_max_iterations: 50,
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(global.merge(&workspace).execution.circuit_break_after, 7);
        assert_eq!(global.merge(&workspace).execution.validation_max_attempts, 5);
        assert_eq!(global.merge(&workspace).execution.foreach_max_iterations, 50);
    }
}
