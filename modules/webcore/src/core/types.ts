/**
 * Domain types shared by every UI surface.
 *
 * These are lightweight, framework-agnostic shapes that mirror `metteur.proto`
 * but stay free of generated dependencies, so the UI never depends on codegen.
 * `GrpcGateway` maps between these and the generated message classes.
 */

/** Result of a gateway call; `ok` is `true` on success with `data`, else an error message. */
export type Result<T> = { ok: true; data: T } | { ok: false; error: string }

/** Wrap a value in a success result. */
export function ok<T>(data: T): Result<T> {
  return { ok: true, data }
}

/** Wrap a message in a failure result. */
export function err<T = never>(error: string): Result<T> {
  return { ok: false, error }
}

/** A workspace opened by the daemon. */
export interface WorkspaceInfo {
  path: string
  locked: boolean
}

/** A node in the workspace file explorer (the resource manager). */
export interface FileTreeNode {
  name: string
  /** Absolute path within/under the workspace root. */
  path: string
  kind: 'dir' | 'file'
  /** Present only for directories. */
  children?: FileTreeNode[]
}

/** Metadata of one filesystem entry, used by the explorer's Properties. */
export interface FileInfo {
  path: string
  isDir: boolean
  /** Length in bytes. */
  len: number
}

/** One background command of a workspace. */
export interface JobInfo {
  id: string
  command: string
  /** Working directory, relative to the workspace root. */
  cwd: string
  state: 'running' | 'exited' | 'failed' | 'killed'
  /** Process exit code; -1 when the job has none. */
  exitCode: number
  /** The run that started the job. */
  runId: string
  startedAt: number
  finishedAt: number
  outputBytes: number
  /** Trailing output, for a quick look without subscribing. */
  tail: string
}

/** A streamed job notice (`started` / `output` / `finished`). */
export interface JobNotice {
  jobId: string
  kind: 'started' | 'output' | 'finished'
  /** New output text for `output` notices. */
  chunk: string
  state: JobInfo['state']
  exitCode: number
  /** One-line summary for lifecycle notices. */
  summary: string
}

/** A live file-system change pushed from the daemon. */
export interface WatchEvent {
  /** Workspace-relative path using `/` as the separator. */
  path: string
  kind: 'created' | 'modified' | 'removed'
}

/** A sidecar resource (a saved file) inside the workspace. */
export interface FileContent {
  content: string
  /** Editor language hint derived from the extension, e.g. `json`, `blueprint`. */
  language: string
}

/** One turn in the ReAct conversation shown by the chat surface. */
export interface ChatMessage {
  id: string
  role: 'user' | 'assistant' | 'tool' | 'error' | 'notice'
  /** Role header for tool messages, e.g. the tool name. */
  actor?: string
  content: string
  /** Optional structured payload (model, tokens, tool calls). */
  detail?: Record<string, unknown>
  createdAt: number
  /** Set while this message is still streaming in. */
  pending?: boolean
  /** Reasoning that preceded the answer (streamed, then settled). */
  reasoning?: string
  /** Set while the reasoning block is still streaming. */
  reasoningPending?: boolean
  /** Observed reasoning duration, retained independently of component lifetime. */
  reasoningElapsedMs?: number
  /** Start of the current reasoning segment, in client wall-clock milliseconds. */
  reasoningStartedAt?: number
  /** Complete request duration, including first-token wait, text and tools. */
  turnElapsedMs?: number
  /** Present only while this message's request is still running. */
  turnStartedAt?: number
  /** Set on a user turn typed while the agent worked, until it is injected. */
  queued?: boolean
  /**
   * Snapshot taken before this turn ran, when versioning is available: the
   * transcript offers to roll the workspace back to it.
   */
  checkpoint?: string
}

/** Token usage reported for a completed chat turn. */
/** State of one agent task-list entry. */
export type TodoStatus = 'pending' | 'in_progress' | 'completed'

/** One entry of the agent's task list. */
export interface TodoItem {
  content: string
  status: TodoStatus
  /** Present-continuous phrasing shown while the item is in progress. */
  activeForm?: string
}

export interface ChatUsage {
  /** Server-measured total duration supplied with a terminal event. */
  turnElapsedMs?: number
  inputTokens: number
  outputTokens: number
  totalTokens: number
  /** Input tokens served from the provider's prompt cache (0 when unreported). */
  cachedInputTokens: number
  /** Input tokens written into the provider's prompt cache. */
  cacheWriteInputTokens: number
}

/** Sampling parameters for a ReAct chat turn, adjustable from the chat panel. */
export interface ChatOptions {
  model?: string
  system?: string
  temperature?: number
  top_p?: number
  max_tokens?: number
  reasoning_effort?: 'none' | 'low' | 'medium' | 'high'
  /**
   * Who answers authorization questions for this turn.
   *
   * `ask` confirms every edit and command, `sandbox` runs workspace-internal
   * edits and policy-approved commands, and `full` has the model review the
   * operations the predictor calls risky.
   */
  permission_mode?: 'ask' | 'sandbox' | 'full'
  /** Internal marker: retry must resume the restored persisted session. */
  retry?: boolean
}

/** What the conversation occupies in the model's window, as the daemon sees it. */
export interface ChatContextStats {
  /** Estimated tokens of the whole context. */
  tokens: number | null
  /** The model's input window; assumed when the configuration states none. */
  limit: number | null
  /** Whether `limit` is a default rather than the configured window. */
  assumedLimit: boolean
  /** Per-region estimate, largest first. */
  regions: Array<{ region: string; tokens: number }>
}

/** Metadata of a persisted chat session, mirroring the daemon's record. */
export interface ChatSessionInfo {
  sessionId: string
  createdAt: number
  updatedAt: number
  turns: number
  /** First user message, truncated, for list display. */
  title: string
  messageCount: number
}

/** A persisted session's messages, loaded to restore the conversation UI. */
export interface ChatSessionSnapshot {
  /** The agent's task list at the end of the last turn. */
  todos?: TodoItem[]
  sessionId: string
  createdAt: number
  /**
   * The conversation as it was displayed, including tool calls and notices.
   *
   * Empty for sessions written before the daemon recorded a transcript; those
   * fall back to {@link history}, which only carries user/assistant text.
   */
  transcript: ChatMessage[]
  /** User/assistant text reconstructed from the model's context. */
  history: ChatMessage[]
}

/** Categories of blueprint nodes, mirroring the daemon node model. */
export type NodeCategory = 'event' | 'module' | 'flow' | 'action'

/** Available pin kinds; exec pins drive control flow, data pins carry values. */
export type PinKind = 'exec-in' | 'exec-out' | 'data-in' | 'data-out'

/** A blueprint node with UE-style exec/data pins. */
export interface BlueprintNode {
  id: string
  /** Node type name, e.g. `Start`, `CallLLM`, `Tool`. */
  type: string
  nodeType?: string
  data?: Record<string, unknown>
  category: NodeCategory
  title: string
  summary?: string
  position: { x: number; y: number }
  /** Pins grouped as input (exec-in + data-in) and output (exec-out + data-out). */
  inputs: BlueprintPin[]
  outputs: BlueprintPin[]
  /** Default values keyed by data-input pin id (Unreal-style inline editors). */
  values?: Record<string, string>
  selected?: boolean
}

/** Daemon-owned execution metadata, separate from visual presets. */
export interface NodeKindInfo {
  addonBinding?: Record<string, unknown>
  kind: string
  nodeType: string
  pins: BlueprintPin[]
  description: string
  dynamicPins: boolean
}
export interface NodeCatalog {
  kinds: string[]
  nodes: NodeKindInfo[]
  signatureVersion: number
  ready: boolean
}

/** A single pin/handle on a blueprint node. */
export interface BlueprintPin {
  /** Unique (UUID) id used as the wire handle. */
  id: string
  /** Stable semantic key assigned at creation (e.g. `x-in`, `prompt`), used
   *  for value lookup and kind-based wiring. */
  key?: string
  name: string
  kind: PinKind
  /** Pin type using the shared vocabulary: string/number/int/bool/json/object/
   *  context/choice/any plus structured forms (`list<int>`, `object{...}`). */
  type?: string
  /** Allowed values when the pin is a choice editor (e.g. reasoning effort). */
  choices?: string[]
  /** Default value used when the input has no wire. */
  default?: unknown
  /** Whether the input may stay unwired (resolves to null). */
  optional?: boolean
  /** Short human-readable description shown in the inspector. */
  description?: string
}

/** A directed connection between two node pins. */
export interface BlueprintEdge {
  id: string
  source: string
  sourceHandle?: string
  target: string
  targetHandle?: string
  label?: string
}

/** A saved blueprint graph. */
export interface Blueprint {
  id: string
  name: string
  nodes: BlueprintNode[]
  edges: BlueprintEdge[]
  entryNodeId?: string
}

/** Execution lifecycle observed by the UI. */
export type ExecStatus = 'idle' | 'running' | 'paused' | 'finished' | 'cancelled' | 'failed' | 'unknown' | 'suspended'

/** A region of the live LLM context, reported by a CallLLM node. */
export interface ContextRegion {
  region: string
  chars: number
  tokens: number
}

/** A streamed execution event, aligned with `ExecutionEvent` in the proto. */
export interface ExecutionEvent {
  nodeId: string
  kind:
    | 'started'
    | 'finished'
    | 'node_data'
    | 'message'
    | 'approval_request'
    | 'context'
    | 'error'
    | 'todos'
  message: string
  /** Structured payload for `approval_request` / `message` / `context`. */
  detail?: Record<string, unknown>
}

/** A pending sandbox approval surfaced during execution. */
export interface ApprovalRequest {
  id: string
  title: string
  tool: string
  command?: string
  detail: string
  /** sandbox | circuit_tripped | replan_proposal | blueprint_save */
  requestType?: string
}

/** A version snapshot with an optional human alias. */
export interface SnapshotInfo {
  id: string
  alias?: string
  createdAt: number
  message: string
}

/** Entry of the file timeline for a snapshot. */
export interface FileHistoryEntry {
  path: string
  snapshotId: string
  op: 'add' | 'modify' | 'delete'
  at: number
  /** Optional unified diff for `modify`. */
  diff?: string
}

/** An installed addon. */
export interface AddonInfo {
  id: string
  name: string
  version: string
  description: string
  enabled: boolean
  scope: string
  toolCount: number
  fragmentCount: number
  scopeRoot?: string
  fingerprint?: string
  status?: string
  error?: string
  requiredPermissions?: string[]
  grantedPermissions?: string[]
  hooks?: Array<{ name: string; event: string; scopeRoot: string; eventId: string; status: string; completed: number; failed: number; error: string }>
}

/** Aggregated token/cost usage for an execution scope. Missing completeness fields are unknown. */
export interface OversightUsage {
  limit: number
  charged: number
  warning: boolean
  exhausted: boolean
  concierge_available: boolean
  calls: Array<{ caller: 'supervisor' | 'concierge'; charged: number; state: string; cost_micros: number | null; currency: string; accounting_version?: number }>
}
export interface UsageSummary {
  oversight?: OversightUsage
  currency: string
  totalCostMicros: number
  models: Array<{
    model: string
    calls: number
    inputTokens: number
    outputTokens: number
    reasoningTokens: number
    costMicros: number
    /** Input tokens served from the provider's prompt cache. */
    cachedInputTokens: number
    tokensComplete?: boolean
    cacheComplete?: boolean
    costComplete?: boolean
    /** Input tokens written into the provider's prompt cache. */
    cacheWriteInputTokens: number
  }>
}

/** An execution run recorded by the daemon. */
export interface ExecutionInfo {
  snapshot?: import('./execution-view').ExecutionSnapshot
  runId: string
  blueprintId: string
  /** Running | Suspended | RecoveryRequired | Completed | Cancelled | Failed */
  status: string
  startedAt: number
  updatedAt: number
  executedNodes: number
}

/** A registered MCP server surfaced by the daemon. */
export interface McpServerInfo {
  owner?: string
  scopeRoot?: string
  name: string
  /** Connected | Failed | Disabled */
  status: string
  toolCount: number
  error: string
}

/** One signature pin of a blueprint function. */
export interface FnPinInfo {
  default?: unknown
  optional?: boolean
  description?: string
  name: string
  /** string | number | int | bool | list | object */
  type: string
}

/** A callable blueprint function from the daemon's function library. */
export interface FunctionItem {
  addonBinding?: Record<string, unknown>
  filePath?: string
  id: string
  name: string
  description: string
  /** builtin | global | workspace */
  source: string
  inputs: FnPinInfo[]
  outputs: FnPinInfo[]
}

/** Per-node audit data collected live during an execution. */
export interface NodeAudit {
  /** Epoch millis when the node started (0 = unknown). */
  startedAt: number
  finishedAt: number
  /** Data output values captured by `node_data` events. */
  outputs: Record<string, unknown>
  /** Total input+output tokens reported via `context` events. */
  tokens: number
  /** Latest message produced by the node. */
  message: string
}

/** One node of the agent execution tree. */
export interface ExecTreeNode {
  id: string
  /** run | node:<kind> | subagent | function:<name> */
  kind: string
  label: string
  parent: string
  children: string[]
  /** running | done | failed:<reason> */
  status: string
  tokens: number
  startedAt: number
  finishedAt: number
}

/** The agent execution tree of one run. */
export interface ExecTreeData {
  nodes: ExecTreeNode[]
  roots: string[]
}

// --- Layered configuration (mirrors `metteur_shared::config::Config`) --------
//
// The daemon persists configuration in *layers*: a global layer
// (`~/.metteur/config.toml`) and a per-workspace layer
// (`<workspace>/.metteur/config.toml`). The effective value is the workspace
// layer merged over the global layer per key (VSCode user/workspace model).
// Field names use the daemon's snake_case JSON form.

/** Pricing of a single model, expressed as cost per million tokens. */
export interface ModelPricing {
  input_per_mtok?: number
  output_per_mtok?: number
}

/** LLM-related settings. */
export interface LlmSettings {
  /** Default model identifier. */
  default_model?: string | null
  /** Default temperature. */
  temperature?: number | null
  /** Preferred model for SubAgent tool invocations. */
  subagent_default_model?: string | null
  /** Complete per-model definitions keyed by model ID. */
  models?: Record<string, LlmModelConfig>
}

/** A complete model definition: connection, advanced options and pricing. */
export interface LlmModelConfig {
  /** Human-readable display name (defaults to the model ID when empty). */
  display_name?: string
  /** `openai-chat` | `openai-responses` | `anthropic`. */
  api_type?: string
  /** API endpoint URL. */
  api_endpoint?: string
  /** Model ID sent in requests. */
  model_id?: string
  /** API key used to authenticate. */
  api_key?: string
  /** Input context window limit in tokens (advanced, optional). */
  context_window_input?: number | null
  /** Output context window limit in tokens (advanced, optional). */
  context_window_output?: number | null
  /** Whether the model accepts image inputs (advanced, optional). */
  supports_vision?: boolean
  /** Pricing strategy (all prices optional). */
  pricing?: ModelPricingStrategy
}

/** Prices of a model: input / output / cache-hit input / cache-write, per 1M. */
export interface ModelPricing {
  input_per_mtok?: number
  output_per_mtok?: number
  /** Cache-hit input, per 1M tokens. */
  cache_hit_per_mtok?: number
  /** Tokens written into the prompt cache, per 1M tokens. */
  cache_write_per_mtok?: number
}

/** One pricing tier ("tiered"): applies up to `max_input_tokens`. */
export interface PricingTier {
  /** Human label, e.g. `"<=200k"`. */
  name?: string
  /** Input-token upper bound for this tier (unset = unbounded, last tier). */
  max_input_tokens?: number | null
  prices?: ModelPricing
}

/** One peak/off-peak window with an independent price set. */
export interface PeakWindowPrice {
  /** 5-field cron (`min hour day month weekday`) marking the window start. */
  cron?: string
  /** How long the window lasts, in minutes. */
  duration_min?: number
  /** Independent prices for this window. */
  prices?: ModelPricing
}

/** Pricing strategy of a model. */
export interface ModelPricingStrategy {
  /** `default` | `tiered` | `peak`. */
  kind?: 'default' | 'tiered' | 'peak'
  /** Flat prices (kind = "default"). */
  prices?: ModelPricing
  /** Tiered prices (kind = "tiered"). */
  tiers?: PricingTier[]
  /** Window-external prices (kind = "peak"). */
  default_prices?: ModelPricing
  /** Per-window prices (kind = "peak"); each window sets its own prices. */
  windows?: PeakWindowPrice[]
}

/** Billing settings: only cost *display*, not per-model pricing rules. */
export interface BillingSettings {
  /** Currency code used for cost reporting, e.g. `USD`. */
  currency?: string
  /** IANA time zone name used for cost reporting. */
  timezone?: string
}

/** Anonymization settings. */
export interface AnonymizeSettings {
  /** Whether sensitive values are redacted before entering the LLM context. */
  enabled?: boolean
  /** Additional regex patterns treated as secrets. */
  extra_patterns?: string[]
}

/** Command sandbox settings. */
export interface SandboxSettings {
  /** Whether the command sandbox is enabled. */
  enabled?: boolean
  /** Commands allowed without approval (glob patterns). */
  whitelist?: string[]
  /** Commands always requiring approval. */
  blacklist?: string[]
  /** Seconds before an unanswered approval is denied (0 = default). */
  approval_timeout_secs?: number
  /** Seconds before a spawned command is killed (0 = default). */
  command_timeout_secs?: number
}

/** Transport settings for one MCP server connection. */
export interface McpServerSettings {
  /** `stdio` or `http`. */
  transport: string
  /** Process argv for the stdio transport. */
  command: string[]
  /** Extra environment variables. */
  env?: Record<string, string>
  /** Server URL for the http transport. */
  url?: string | null
  /** Whether this server is connected. */
  enabled?: boolean
}

/** MCP host settings. */
export interface McpSettings {
  /** Registered MCP servers keyed by alias. */
  servers?: Record<string, McpServerSettings>
  /** Per-call timeout in seconds (0 = built-in default of 30). */
  call_timeout_secs?: number
}

/** Addon runtime settings. */
export interface AddonSettings {
  /** Default plugin function call timeout in milliseconds. */
  call_timeout_ms?: number
  /** Require a valid `signature.toml` for every installed addon. */
  require_signature?: boolean
  /** Base64-encoded Ed25519 public keys trusted for package signatures. */
  signing_keys?: string[]
}

/** One language server definition. */
export interface LspLanguageSettings {
  /** Language identifier, e.g. `rust`. */
  id: string
  /** Process argv launching the language server. */
  command: string[]
  /** File extensions routed to this language (without the dot). */
  extensions: string[]
}

/** LSP-related settings. */
export interface LspSettings {
  /** Whether LSP integration is enabled. */
  enabled?: boolean
  /** Language server definitions. */
  languages?: LspLanguageSettings[]
}

/** Versioning settings. */
export interface VersioningSettings {
  /** Whether automatic snapshotting is enabled. */
  auto_snapshot?: boolean
}

/** Execution-engine settings. */
export interface ExecutionSettings {
  /** Consecutive validator/judge failures tripping the circuit breaker. */
  circuit_break_after?: number
}

/** Daemon process settings. */
export interface DaemonSettings {
  /** How the daemon registers itself to start with the host OS. */
  autostart?: 'off' | 'login' | 'service'
  /** Whether the daemon stays quiet until a client sends `WAKE`. */
  wake?: boolean
  /** Listen address overriding the CLI default when present. */
  listen_addr?: string | null
}

/** ACL rule (kept opaque; edited via the JSON view). */
export interface AclRule {
  [key: string]: unknown
}

/** ACL settings. */
export interface AclSettings {
  rules?: AclRule[]
}

/**
 * The merged daemon configuration.
 *
 * Mirrors `metteur_shared::config::Config` in its JSON form. One layer is
 * persisted per `getConfig`/`setConfig` call; `extra` holds future
 * Addon-provided keys.
 */
export interface DaemonConfig {
  acl?: AclSettings
  llm?: LlmSettings
  billing?: BillingSettings
  anonymize?: AnonymizeSettings
  sandbox?: SandboxSettings
  mcp?: McpSettings
  addon?: AddonSettings
  lsp?: LspSettings
  versioning?: VersioningSettings
  execution?: ExecutionSettings
  daemon?: DaemonSettings
  /** Extension keys for future Addon use. */
  [key: string]: unknown
}

/** Raw file plus the daemon's compatible editing layer and runtime values. */
export interface ConfigState {
  defaults: DaemonConfig
  raw: DaemonConfig
  overrides: DaemonConfig
  effective: DaemonConfig
  legacy: boolean
}
