import type { OversightReports } from './oversight'
import type { ConciergeState, ConciergeEvent } from './concierge'
import type { Blackboard, BoardQuery } from './blackboard'
import type { Ref } from 'vue'
import type {
  AddonInfo,
  ChatContextStats,
  Blueprint,
  NodeCatalog,
  ChatMessage,
  ChatOptions,
  ChatSessionInfo,
  ChatSessionSnapshot,
  ChatUsage,
  DaemonConfig,
  ConfigState,
  ExecTreeData,
  ExecutionEvent,
  ExecutionInfo,
  FileContent,
  FileHistoryEntry,
  FileInfo,
  FileTreeNode,
  FunctionItem,
  McpServerInfo,
  Result,
  SnapshotInfo,
  TodoItem,
  UsageSummary,
  JobInfo,
  JobNotice,
  WatchEvent,
  WorkspaceInfo,
} from './types'

/**
 * Data-access port for every UI surface.
 *
 * The app talks to the daemon through this single interface. `MockGateway`
 * implements it for development/demo; `GrpcGateway` backs it with Connect +
 * grpc-web through the Web Server Client.
 */
export interface DaemonGateway {
  /** Live daemon connection state (reactive; heartbeat-driven). */
  readonly connected: Ref<boolean>
  /** True when the in-memory demo gateway backs the app (never blocks). */
  readonly demo: boolean

  // Connection / workspace -----------------------------------------------------
  connect(): Promise<Result<void>>
  openWorkspace(path: string): Promise<Result<WorkspaceInfo>>
  closeWorkspace(path: string): Promise<Result<void>>
  listWorkspaces(): Promise<Result<WorkspaceInfo[]>>

  // Configuration --------------------------------------------------------------
  /**
   * Read one configuration layer: pass an empty string for the global layer,
   * or a workspace root path for that workspace's layer.
   */
  getConfigState(workspacePath?: string): Promise<Result<ConfigState>>
  getConfig(workspacePath?: string): Promise<Result<DaemonConfig>>
  /** Persist a configuration layer ('' = global, otherwise workspace-root). */
  setConfig(config: DaemonConfig, workspacePath?: string): Promise<Result<void>>

  // File explorer / editors ---------------------------------------------------
  listFiles(workspacePath: string, dir: string): Promise<Result<FileTreeNode[]>>
  readFile(workspacePath: string, filePath: string): Promise<Result<FileContent>>
  writeFile(workspacePath: string, filePath: string, content: string): Promise<Result<void>>
  createDir(workspacePath: string, dirPath: string): Promise<Result<void>>
  removeFile(workspacePath: string, filePath: string): Promise<Result<void>>
  renameFile(workspacePath: string, from: string, to: string): Promise<Result<void>>
  statFile(workspacePath: string, path: string): Promise<Result<FileInfo>>
  /** Opens the system file manager with the entry selected (best effort). */
  revealInExplorer(workspacePath: string, path: string): Promise<Result<void>>
  /** Subscribes to live file changes; resolves when the stream ends or aborts. */
  watchWorkspace(
    workspacePath: string,
    onEvent: (e: WatchEvent) => void,
    signal?: AbortSignal,
  ): Promise<Result<void>>

  // Background commands (jobs) --------------------------------------------------
  /** Lists the workspace's background commands (oldest first). */
  listJobs(workspacePath: string): Promise<Result<JobInfo[]>>
  /** Subscribes to job notices; resolves when the stream ends or aborts. */
  watchJobs(
    workspacePath: string,
    onEvent: (e: JobNotice) => void,
    signal?: AbortSignal,
  ): Promise<Result<void>>
  /** Terminates one background command; `killed` is false when it had ended. */
  killJob(workspacePath: string, jobId: string): Promise<Result<{ killed: boolean; state: string }>>
  /**
   * A file's content as recorded by a snapshot (empty id = the latest one).
   * `found: false` means the snapshot did not track the file.
   */
  getFileAtSnapshot(
    workspacePath: string,
    path: string,
    snapshotId?: string,
  ): Promise<Result<{ found: boolean; content: string; snapshotId: string }>>

  // ReAct chat -----------------------------------------------------------------
  /**
   * Send a chat turn. Assistant replies arrive as `onMessage` updates: deltas
   * come with a stable id and `pending` set (append to the open bubble), the
   * final turn carries the full text without `pending` (replace the bubble).
   * `onSession` reports the persisted session id created or resumed, and
   *  `onUsage` the token usage of a completed turn.
   */
  sendChat(
    workspacePath: string,
    content: string,
    history: ChatMessage[],
    onMessage: (m: ChatMessage) => void,
    options?: ChatOptions,
    onSession?: (sessionId: string, checkpointId?: string) => void,
    sessionId?: string,
    onUsage?: (usage: ChatUsage) => void,
    onTodos?: (todos: TodoItem[]) => void,
    /** Aborts the stream locally; the caller still asks the daemon to stop. */
    signal?: AbortSignal,
    /** Reports a tool call whose arguments are still being written. */
    onProgress?: (progress: { name: string; bytes: number }) => void,
    /** Reports what the conversation occupies once the turn ends. */
    onContext?: (stats: ChatContextStats) => void,
    /** Asks the client to decide on a sandbox approval, mid-turn. */
    onApproval?: (request: { requestId: string; detail: string }) => void,
  ): Promise<Result<void>>
  abortChat(workspacePath: string): Promise<Result<void>>
  /**
   * Injects a message into the running turn.
   *
   * `Normal` lands at the next model call (a queued message), `Urgent` before
   * the next request of the loop (steering), `Emergency` aborts it.
   */
  sendInterrupt(
    workspacePath: string,
    message: string,
    priority?: 'Normal' | 'Urgent' | 'Emergency',
  ): Promise<Result<void>>
  /**
   * Opens the host's folder dialog.
   *
   * Implemented by the web server (`POST /api/pick-directory`), so the demo
   * gateway answers without a round trip.
   */
  pickDirectory(): Promise<Result<string | null>>
  /** List the workspace's persisted chat sessions (newest first). */
  listChatSessions(workspacePath: string): Promise<Result<ChatSessionInfo[]>>
  /** Load a session's history for UI restore (empty id = latest, NotFound when absent). */
  getChatSession(workspacePath: string, sessionId?: string): Promise<Result<ChatSessionSnapshot>>
  rewindChat(workspacePath: string, sessionId: string, snapshotId: string): Promise<Result<ChatSessionSnapshot>>
  /** Delete one session (empty id = latest; stops a running chat first). */
  deleteChatSession(workspacePath: string, sessionId?: string): Promise<Result<void>>

  // Blueprints -----------------------------------------------------------------
  listNodeKinds(workspacePath?: string): Promise<Result<NodeCatalog>>
  listFunctions(workspacePath: string): Promise<Result<FunctionItem[]>>
  importFunction(workspacePath: string, source: FunctionItem, name: string, filePath: string): Promise<Result<{ name: string; filePath: string }>>
  compileDsl(source: string, workspacePath?: string): Promise<Result<Blueprint>>
  decompileBlueprint(workspacePath: string, blueprintOrId: Blueprint | string): Promise<Result<string>>
  saveBlueprint(workspacePath: string, blueprint: Blueprint, filePath?: string): Promise<Result<void>>
  loadBlueprint(workspacePath: string, blueprintId: string): Promise<Result<Blueprint>>

  // Execution ------------------------------------------------------------------
  executeBlueprint(
    workspacePath: string,
    blueprintId: string,
    onEvent: (e: ExecutionEvent) => void,
    /** The canvas being edited; sent so Run executes what the user sees even
     *  when the daemon mirror of the blueprint is stale. */
    blueprint?: Blueprint,
  ): Promise<Result<void>>
  continueExecution(
    workspacePath: string,
    runId: string,
    onEvent: (e: ExecutionEvent) => void,
  ): Promise<Result<void>>
  listExecutions(workspacePath: string): Promise<Result<ExecutionInfo[]>>
  getExecutionTree(workspacePath: string, runId: string): Promise<Result<ExecTreeData>>
  cancel(workspacePath: string, runId?: string): Promise<Result<void>>
  pause(workspacePath: string, runId?: string): Promise<Result<void>>
  resume(workspacePath: string, runId?: string): Promise<Result<void>>

  // Approvals ------------------------------------------------------------------
  respondApproval(workspacePath: string, requestId: string, allow: boolean): Promise<Result<void>>

  // Versioning -----------------------------------------------------------------
  listSnapshots(workspacePath: string): Promise<Result<SnapshotInfo[]>>
  createSnapshot(
    workspacePath: string,
    description: string,
    alias?: string,
  ): Promise<Result<SnapshotInfo>>
  rollback(workspacePath: string, snapshotId: string, alias?: string): Promise<Result<void>>
  listFileHistory(workspacePath: string, path: string): Promise<Result<FileHistoryEntry[]>>

  // Addons / usage / resources --------------------------------------------------
  listAddons(): Promise<Result<AddonInfo[]>>
  /** Toggle an addon. `workspacePath` selects the workspace scope; empty means
   *  the global scope. */
  setAddonEnabled(id: string, enabled: boolean, workspacePath?: string): Promise<Result<void>>
  listMcpServers(workspacePath?: string): Promise<Result<McpServerInfo[]>>
  getBlackboard(workspacePath: string, runId: string, query?: BoardQuery): Promise<Result<Blackboard>>
  listOversightReports(workspacePath: string, runId: string): Promise<Result<OversightReports>>
  getConciergeState(workspacePath: string, runId: string, conversationId: string): Promise<Result<ConciergeState>>
  sendConciergeMessage(workspacePath: string, runId: string, conversationId: string, messageId: string, message: string, onEvent: (event: ConciergeEvent) => void, signal?: AbortSignal): Promise<Result<void>>
  getExecutionUsage(workspacePath: string, runId: string): Promise<Result<UsageSummary>>
}
