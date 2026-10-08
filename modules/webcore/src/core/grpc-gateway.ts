import type { OversightReports } from './oversight'
import type { ConciergeState, ConciergeEvent } from './concierge'
import type { Blackboard, BoardQuery } from './blackboard'
import { executionSnapshot } from './execution-view'
import { createClient, type Client } from '@connectrpc/connect'
import { createGrpcWebTransport } from '@connectrpc/connect-web'
import { ref, type Ref } from 'vue'
import type { PinKind } from './types'
import type { DaemonGateway } from './gateway'
import type {
  AddonInfo,
  Blueprint,
  NodeCatalog,
  BlueprintEdge,
  BlueprintNode,
  BlueprintPin,
  ChatMessage,
  ChatOptions,
  ChatSessionInfo,
  ChatContextStats,
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
import { err, ok } from './types'
import { readSse } from './sse'
import { Daemon } from '@/gen/metteur_pb'
import { categoryFor } from '@/lib/blueprint'
import { fromWireCatalog, fromWirePin, parseDefault, pinType, valueText, inlineValue, nodeConfiguration, type WirePin } from './node-catalog'
import { configScopeOf, configToToml, isConfigDoc, tryParseToml } from '@/lib/toml'

/** Reads a context-stats payload, tolerating a partial or unexpected shape. */
function parseContextStats(raw: unknown): ChatContextStats {
  const stats = (raw ?? {}) as Record<string, unknown>
  return {
    tokens: stats.tokens === null || stats.tokens === undefined ? null : Number(stats.tokens) || 0,
    limit: stats.limit === null || stats.limit === undefined ? null : Number(stats.limit),
    assumedLimit: stats.assumed_limit === true,
    regions: Array.isArray(stats.regions)
      ? (stats.regions as Array<{ region: string; tokens: number }>)
      : [],
  }
}

/**
 * Rebuilds the display transcript of a stored session.
 *
 * The daemon records what the conversation looked like — turns, tool calls with
 * their timing and outcome, notices, failures — which the model's context
 * cannot reproduce after compression or eviction.
 */
function parseTranscript(raw: string): ChatMessage[] {
  if (!raw) return []
  let entries: Array<Record<string, unknown>>
  try {
    entries = JSON.parse(raw)
  } catch {
    return []
  }
  if (!Array.isArray(entries)) return []
  return entries.map((entry, index) => {
    const role = String(entry.role ?? '')
    const at = Number(entry.at) || Date.now()
    const content = String(entry.content ?? '')
    if (role === 'tool') {
      return {
        id: String(entry.call_id || `t-${index}`),
        role: 'tool' as const,
        actor: String(entry.tool ?? ''),
        content,
        detail: {
          callId: String(entry.call_id ?? ''),
          summary: String(entry.summary ?? ''),
          ok: entry.ok !== false,
          elapsedMs: Number(entry.elapsed_ms) || 0,
        },
        createdAt: at,
      }
    }
    if (role === 'notice') return { id: `n-${index}`, role: 'notice' as const, content, createdAt: at }
    if (role === 'error') return { id: `e-${index}`, role: 'error' as const, content, createdAt: at }
    if (role === 'assistant') {
      return {
        id: `a-${index}`,
        role: 'assistant' as const,
        content,
        reasoning: String(entry.reasoning ?? '') || undefined,
        reasoningElapsedMs: parseReasoningDuration(entry.reasoning_elapsed_ms),
        turnElapsedMs: parseReasoningDuration(entry.turn_elapsed_ms),
        createdAt: at,
      }
    }
    return { id: `u-${index}`, role: 'user' as const, content, createdAt: at, checkpoint: String(entry.checkpoint ?? '') || undefined }
  })
}

/** Legacy transcripts have no timing; never coerce missing values to zero. */
function parseReasoningDuration(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isFinite(value) && value >= 0 ? value : undefined
}

/** Parses a `detail_json` payload, tolerating an empty or malformed value. */
function parseDetail(raw: string | undefined): Record<string, unknown> {
  if (!raw) return {}
  try {
    const parsed = JSON.parse(raw)
    return parsed && typeof parsed === 'object' ? (parsed as Record<string, unknown>) : {}
  } catch {
    return {}
  }
}

/** Maps a thrown connect error into a readable failure result. */
function toErr(e: unknown): Result<never> {
  if (e instanceof Error) return err(e.message)
  return err(String(e))
}

/** Map a UI node type to the daemon kind (`Arithmetic` wraps `Add`). */
function daemonKindOf(kind: string): string {
  return kind === 'Arithmetic' ? 'Add' : kind
}

/** Narrows the daemon's job-state string to the UI union. */
function jobStateOf(raw: string): JobInfo['state'] {
  return raw === 'running' || raw === 'exited' || raw === 'failed' || raw === 'killed'
    ? raw
    : 'failed'
}

/** Narrows the daemon's job-notice kind to the UI union. */
function jobNoticeKindOf(raw: string): JobNotice['kind'] {
  return raw === 'started' || raw === 'output' || raw === 'finished' ? raw : 'output'
}

/** Map a UI category to the daemon node-type string. */
function nodeTypeOf(kind: string): string {
  switch (kind) {
    case 'Start':
    case 'End':
      return 'Event'
    case 'Validator':
    case 'Judge':
    case 'Branch':
      return 'Control'
    case 'Add':
    case 'Subtract':
    case 'Multiply':
    case 'Divide':
      return 'Pure'
    default:
      return 'Function'
  }
}

/** Map a pin kind to the daemon pin-type string. */
function pinTypeOf(kind: PinKind): string {
  switch (kind) {
    case 'exec-in':
      return 'ExecInput'
    case 'exec-out':
      return 'ExecOutput'
    case 'data-in':
      return 'DataInput'
    case 'data-out':
      return 'DataOutput'
  }
}

/** Coerce inline values without narrowing structural types to strings. */
function coerceValue(type: string | undefined, raw: string): unknown {
  if (['number', 'float', 'int'].includes(type ?? '')) {
    const n = Number(raw)
    return Number.isFinite(n) ? n : raw
  }
  if (type === 'bool') return raw === 'true'
  if (['any', 'json'].includes(type ?? '') || type?.startsWith('list') || type?.startsWith('object')) {
    try { return JSON.parse(raw) } catch { return raw }
  }
  return raw
}

/** Serialize a UI node into the daemon `data` object from its pin values. */
function nodeData(node: BlueprintNode): Record<string, unknown> {
  const data: Record<string, unknown> = { ...node.data }
  const values = node.values ?? {}
  for (const pin of node.inputs) {
    if (pin.kind !== 'data-in') continue
    const raw = values[pin.id] ?? valueText(inlineValue(data, pin))
    // Pin values are authoritative for inline inputs; preserve unrelated configuration.
    delete data[pin.name]
    if (pin.key) delete data[pin.key]
    delete data[pin.id]
    if (raw === undefined || raw === '') continue
    data[pin.name || pin.key || pin.id] = coerceValue(pin.type, raw)
  }
  return data
}

/** Convert a domain blueprint into its protobuf form for the daemon. */
export function toProtoBlueprint(bp: Blueprint): object {
  const kept = bp.nodes.filter((n) => n.type !== 'FileReference')
  const keptIds = new Set(kept.map((n) => n.id))
  // The daemon resolves the entry against the (FileReference-filtered) node
  // list, so fall back to the first kept node when the canvas entry was dropped.
  const entryNodeId =
    bp.entryNodeId && keptIds.has(bp.entryNodeId) ? bp.entryNodeId : kept[0]?.id ?? ''
  return {
    id: bp.id,
    name: bp.name,
    entryNodeId,
    nodes: kept.map((n) => {
      const kind = daemonKindOf(n.type)
      return {
        id: n.id,
        nodeType: n.nodeType ?? nodeTypeOf(kind),
        kind,
        posX: n.position.x,
        posY: n.position.y,
        pins: [...n.inputs, ...n.outputs].map((p) => ({
          id: p.id,
          key: p.key ?? '',
          name: p.name || (p.kind === 'data-out' ? p.key ?? 'Result' : p.key ?? ''),
          pinType: pinTypeOf(p.kind),
          dataType: pinType(p.type ?? (p.kind.startsWith('exec') ? 'void' : 'any')),
          defaultJson: p.default === undefined ? '' : JSON.stringify(p.default),
          optional: p.optional ?? false, choices: p.choices ?? [], description: p.description ?? '',
        })),
        dataJson: JSON.stringify(nodeData(n)),
      }
    }),
    edges: bp.edges
      .filter((e) => keptIds.has(e.source) && keptIds.has(e.target))
      .map((e) => ({
        id: e.id,
        sourceNode: e.source,
        sourcePin: e.sourceHandle ?? '',
        targetNode: e.target,
        targetPin: e.targetHandle ?? '',
      })),
  }
}

/** Convert a protobuf blueprint into its domain form. */
export function fromProtoBlueprint(pb: {
  id: string
  name: string
  entryNodeId: string
  nodes: Array<{
    id: string
    nodeType: string
    kind: string
    posX: number
    posY: number
    pins: WirePin[]
    dataJson: string
  }>
  edges: Array<{
    id: string
    sourceNode: string
    sourcePin: string
    targetNode: string
    targetPin: string
  }>
}): Blueprint {
  const nodes: BlueprintNode[] = pb.nodes.map((n) => {
    const inputs: BlueprintPin[] = []
    const outputs: BlueprintPin[] = []
    let data: Record<string, unknown> = {}
    try {
      data = JSON.parse(n.dataJson || '{}')
    } catch {
      data = {}
    }
    const values: Record<string, string> = {}
    for (const p of n.pins) {
      const pin = fromWirePin(p)
      const kind = pin.kind
      if (kind === 'data-in') {
        const raw = inlineValue(data, p)
        if (raw !== undefined) values[p.id] = valueText(raw)
        inputs.push(pin)
      } else if (kind === 'exec-in') {
        inputs.push(pin)
      } else {
        outputs.push(pin)
      }
    }
    return {
      id: n.id,
      type: n.kind,
      nodeType: n.nodeType,
      data: nodeConfiguration(data, inputs),
      category: categoryFor(n.kind, n.nodeType),
      title: n.kind,
      position: { x: n.posX, y: n.posY },
      inputs,
      outputs,
      values: Object.keys(values).length ? values : undefined,
    }
  })
  const edges: BlueprintEdge[] = pb.edges.map((e) => ({
    id: e.id,
    source: e.sourceNode,
    sourceHandle: e.sourcePin || undefined,
    target: e.targetNode,
    targetHandle: e.targetPin || undefined,
  }))
  return { id: pb.id, name: pb.name, nodes, edges, entryNodeId: pb.entryNodeId || nodes[0]?.id }
}

/** Map a protobuf execution event into the domain shape. */
function fromProtoEvent(ev: {
  nodeId: string
  kind: string
  message: string
  detailJson: string
}): ExecutionEvent {
  let detail: Record<string, unknown> | undefined
  if (ev.detailJson) {
    try {
      detail = JSON.parse(ev.detailJson)
    } catch {
      detail = undefined
    }
  }
  return { nodeId: ev.nodeId, kind: ev.kind as ExecutionEvent['kind'], message: ev.message, detail }
}

/**
 * A data-access gateway backed by the real daemon over grpc-web.
 *
 * Talks to the Web Server Client (`metteur-web`), which proxies every call to
 * the daemon. Message and domain types are mapped at this boundary so views
 * never depend on generated code.
 */
export class GrpcGateway implements DaemonGateway {
  /** Live connection state, driven by a heartbeat ping; reactive so the UI
   *  flips to offline when the daemon (or the proxy) goes away. */
  readonly connected: Ref<boolean> = ref(true)
  readonly demo = false
  private client: Client<typeof Daemon>
  /** Same origin as the grpc-web transport, used by the SSE chat endpoint. */
  private baseUrl: string

  constructor(baseUrl: string) {
    const transport = createGrpcWebTransport({ baseUrl })
    this.client = createClient(Daemon, transport)
    this.baseUrl = baseUrl
    // Probe the daemon periodically; a network failure marks the app offline.
    setInterval(() => void this.ping(), 5000)
    void this.ping()
  }

  private async ping(): Promise<void> {
    try {
      await this.client.listWorkspaces({})
      this.connected.value = true
    } catch {
      this.connected.value = false
    }
  }

  // Workspaces ----------------------------------------------------------------
  async connect(): Promise<Result<void>> {
    return ok(undefined)
  }

  async openWorkspace(path: string): Promise<Result<WorkspaceInfo>> {
    try {
      const ws = await this.client.openWorkspace({ path })
      return ok({ path: ws.path, locked: ws.locked })
    } catch (e) {
      return toErr(e)
    }
  }

  async closeWorkspace(path: string): Promise<Result<void>> {
    try {
      await this.client.closeWorkspace({ path })
      return ok(undefined)
    } catch (e) {
      return toErr(e)
    }
  }

  async listWorkspaces(): Promise<Result<WorkspaceInfo[]>> {
    try {
      const list = await this.client.listWorkspaces({})
      return ok(list.workspaces.map((w) => ({ path: w.path, locked: w.locked })))
    } catch (e) {
      return toErr(e)
    }
  }

  // Configuration ----------------------------------------------------------------
  async getConfigState(workspacePath = ''): Promise<Result<ConfigState>> {
    try {
      const resp = await this.client.getConfig({ workspacePath })
      if (!resp.effectiveJson) return err('Update the daemon before editing configuration')
      if (!resp.overridesJson) return err('Automatic configuration migration cannot preserve the current effective values. Open Settings TOML to review and explicitly migrate or save the original format.')
      return ok({ defaults: JSON.parse(resp.defaultsJson), raw: JSON.parse(resp.configJson), overrides: JSON.parse(resp.overridesJson), effective: JSON.parse(resp.effectiveJson), legacy: resp.legacyFormat })
    } catch (e) { return toErr(e) }
  }

  async getConfig(workspacePath = ''): Promise<Result<DaemonConfig>> {
    try {
      const resp = await this.client.getConfig({ workspacePath })
      try {
        return ok(JSON.parse(resp.configJson) as DaemonConfig)
      } catch (e) {
        return err(`daemon returned invalid config: ${String(e)}`)
      }
    } catch (e) {
      return toErr(e)
    }
  }

  async setConfig(config: DaemonConfig, workspacePath = ''): Promise<Result<void>> {
    try {
      await this.client.setConfig({ workspacePath, configJson: JSON.stringify(config) })
      return ok(undefined)
    } catch (e) {
      return toErr(e)
    }
  }

  // File explorer ---------------------------------------------------------------
  async listFiles(workspacePath: string, dir: string): Promise<Result<FileTreeNode[]>> {
    try {
      // One level only: the explorer loads children lazily on expansion and
      // caches them, so large workspaces cost one RPC per opened directory
      // instead of a full recursive crawl.
      const list = await this.client.listFiles({ workspacePath, dir })
      const nodes: FileTreeNode[] = list.entries.map((e) => ({
        name: e.name,
        path: e.path,
        kind: e.isDir ? 'dir' : 'file',
      }))
      return ok(nodes)
    } catch (e) {
      return toErr(e)
    }
  }

  async readFile(workspacePath: string, filePath: string): Promise<Result<FileContent>> {
    if (isConfigDoc(filePath)) {
      const scopePath = configScopeOf(filePath) === 'user' ? '' : workspacePath
      const cfg = await this.getConfig(scopePath)
      return cfg.ok ? ok({ content: configToToml(cfg.data), language: 'toml' }) : cfg
    }
    try {
      const file = await this.client.readFile({ workspacePath, path: filePath })
      const language = filePath.endsWith('.blueprint')
        ? 'blueprint'
        : filePath.endsWith('.json')
          ? 'json'
          : 'text'
      return ok({ content: file.content, language })
    } catch (e) {
      return toErr(e)
    }
  }

  async writeFile(workspacePath: string, filePath: string, content: string): Promise<Result<void>> {
    if (isConfigDoc(filePath)) {
      const parsed = tryParseToml(content)
      if (!parsed) return err('Saved content is not valid TOML')
      const scopePath = configScopeOf(filePath) === 'user' ? '' : workspacePath
      return this.setConfig(parsed, scopePath)
    }
    try {
      await this.client.writeFile({ workspacePath, path: filePath, content })
      return ok(undefined)
    } catch (e) {
      return toErr(e)
    }
  }

  async createDir(workspacePath: string, dirPath: string): Promise<Result<void>> {
    try {
      await this.client.createDir({ workspacePath, path: dirPath })
      return ok(undefined)
    } catch (e) {
      return toErr(e)
    }
  }

  async removeFile(workspacePath: string, filePath: string): Promise<Result<void>> {
    try {
      await this.client.removeFile({ workspacePath, path: filePath })
      return ok(undefined)
    } catch (e) {
      return toErr(e)
    }
  }

  async renameFile(workspacePath: string, from: string, to: string): Promise<Result<void>> {
    try {
      await this.client.renameFile({ workspacePath, from, to })
      return ok(undefined)
    } catch (e) {
      return toErr(e)
    }
  }

  async statFile(workspacePath: string, path: string): Promise<Result<FileInfo>> {
    try {
      const info = await this.client.statFile({ workspacePath, path })
      return ok({ path: info.path, isDir: info.isDir, len: Number(info.len) })
    } catch (e) {
      return toErr(e)
    }
  }

  async revealInExplorer(workspacePath: string, path: string): Promise<Result<void>> {
    try {
      await this.client.revealInExplorer({ workspacePath, path })
      return ok(undefined)
    } catch (e) {
      return toErr(e)
    }
  }

  async watchWorkspace(
    workspacePath: string,
    onEvent: (e: WatchEvent) => void,
    signal?: AbortSignal,
  ): Promise<Result<void>> {
    try {
      for await (const ev of this.client.watchWorkspace({ workspacePath }, { signal })) {
        if (ev.kind === 'created' || ev.kind === 'modified' || ev.kind === 'removed') {
          onEvent({ path: ev.path, kind: ev.kind })
        }
      }
      return ok(undefined)
    } catch (e) {
      // Aborting the subscription is not a failure.
      if (signal?.aborted) return ok(undefined)
      return toErr(e)
    }
  }

  // Background commands (jobs) ---------------------------------------------------
  async listJobs(workspacePath: string): Promise<Result<JobInfo[]>> {
    try {
      const res = await this.client.listJobs({ workspacePath })
      return ok(
        res.jobs.map((job) => ({
          id: job.id,
          command: job.command,
          cwd: job.cwd,
          state: jobStateOf(job.state),
          exitCode: job.exitCode,
          runId: job.runId,
          startedAt: Number(job.startedAt),
          finishedAt: Number(job.finishedAt),
          outputBytes: Number(job.outputBytes),
          tail: job.tail,
        })),
      )
    } catch (e) {
      return toErr(e)
    }
  }

  async watchJobs(
    workspacePath: string,
    onEvent: (e: JobNotice) => void,
    signal?: AbortSignal,
  ): Promise<Result<void>> {
    try {
      for await (const event of this.client.watchJobs({ workspacePath }, { signal })) {
        onEvent({
          jobId: event.jobId,
          kind: jobNoticeKindOf(event.kind),
          chunk: event.chunk,
          state: jobStateOf(event.state),
          exitCode: event.exitCode,
          summary: event.summary,
        })
      }
      return ok(undefined)
    } catch (e) {
      // Aborting the subscription is not a failure.
      if (signal?.aborted) return ok(undefined)
      return toErr(e)
    }
  }

  async killJob(
    workspacePath: string,
    jobId: string,
  ): Promise<Result<{ killed: boolean; state: string }>> {
    try {
      const res = await this.client.killJob({ workspacePath, jobId })
      return ok({ killed: res.killed, state: res.state })
    } catch (e) {
      return toErr(e)
    }
  }

  async getFileAtSnapshot(
    workspacePath: string,
    path: string,
    snapshotId?: string,
  ): Promise<Result<{ found: boolean; content: string; snapshotId: string }>> {
    try {
      const res = await this.client.getFileAtSnapshot({
        workspacePath,
        path,
        snapshotId: snapshotId ?? '',
      })
      return ok({ found: res.found, content: res.content, snapshotId: res.snapshotId })
    } catch (e) {
      return toErr(e)
    }
  }

  // ReAct chat --------------------------------------------------------------------
  /**
   * Streams one chat turn over server-sent events.
   *
   * The daemon exposes the same events as a gRPC stream, but SSE is what the
   * browser can follow incrementally over plain HTTP and what an operator can
   * watch with `curl -N`. The callback contract is unchanged, so views do not
   * care which transport is in use.
   */
  async sendChat(
    workspacePath: string,
    content: string,
    history: ChatMessage[],
    onMessage: (m: ChatMessage) => void,
    options?: ChatOptions,
    onSession?: (sessionId: string, checkpointId?: string) => void,
    sessionId?: string,
    onUsage?: (usage: ChatUsage) => void,
    onTodos?: (todos: TodoItem[]) => void,
    signal?: AbortSignal,
    onProgress?: (progress: { name: string; bytes: number }) => void,
    onContext?: (stats: ChatContextStats) => void,
    /** Asks the client to decide on a sandbox approval, mid-turn. */
    onApproval?: (request: { requestId: string; detail: string }) => void,
  ): Promise<Result<void>> {
    let seq = 0
    let turnId: string | null = null
    const historyJson = JSON.stringify(
      history
        .filter((m) => m.role === 'user' || m.role === 'assistant')
        .map((m) => ({ role: m.role, content: m.content })),
    )
    const turn = () => (turnId ??= `a-${Date.now()}-${seq++}`)
    let response: Response
    try {
      response = await fetch(`${this.baseUrl}/api/chat/stream`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          workspace_path: workspacePath,
          message: content,
          history_json: historyJson,
          options_json: JSON.stringify(options ?? {}),
          session_id: sessionId ?? '',
        }),
        signal,
      })
    } catch (e) {
      return toErr(e)
    }
    if (!response.ok) {
      // A failure before the stream starts is a real HTTP error: it says the
      // request never reached the daemon, which is a different problem from a
      // stream that broke halfway.
      return err((await response.text().catch(() => '')) || `chat request failed (${response.status})`)
    }

    try {
      for await (const message of readSse(response, signal)) {
        let event: { kind?: string; content?: string; detail_json?: string }
        try {
          event = JSON.parse(message.data)
        } catch {
          continue
        }
        const detail = parseDetail(event.detail_json)
        switch (event.kind) {
          case 'session': {
            const reported = String(detail.session_id ?? '')
            if (reported) onSession?.(reported, String(detail.checkpoint_id ?? '') || undefined)
          // The session event carries the context it starts from, so the meter
          // has a size immediately (and after switching conversations).
          if (detail.context) onContext?.(parseContextStats(detail.context))
            break
          }
          case 'assistant_delta':
            onMessage({
              id: turn(),
              role: 'assistant',
              content: event.content ?? '',
              createdAt: Date.now(),
              pending: true,
            })
            break
          case 'reasoning_delta':
            onMessage({
              id: turn(),
              role: 'assistant',
              content: '',
              reasoning: event.content ?? '',
              reasoningPending: true,
              createdAt: Date.now(),
              pending: true,
            })
            break
          case 'assistant':
            onMessage({
              id: turn(),
              role: 'assistant',
              content: event.content ?? '',
              // The final reasoning settles the collapsible block; without one
              // the streamed thinking (if any) stays.
              reasoning: String(detail.reasoning ?? '') || undefined,
              reasoningElapsedMs: parseReasoningDuration(detail.reasoning_elapsed_ms),
              reasoningPending: false,
              createdAt: Date.now(),
            })
            break
          case 'tool_start':
            onMessage({
              id: `t-${String(detail.call_id ?? seq++)}`,
              role: 'tool',
              actor: String(detail.name ?? ''),
              content: '',
              detail: { callId: detail.call_id, summary: event.content ?? '', running: true },
              createdAt: Date.now(),
              pending: true,
            })
            break
          case 'tool_progress':
            onMessage({
              id: `t-${String(detail.call_id ?? '')}`,
              role: 'tool',
              content: event.content ?? '',
              detail: { callId: detail.call_id, progress: true },
              createdAt: Date.now(),
              pending: true,
            })
            break
          case 'tool':
            onMessage({
              id: `t-${String(detail.call_id ?? seq++)}`,
              role: 'tool',
              actor: String(detail.name ?? ''),
              content: event.content ?? '',
              detail: {
                callId: detail.call_id,
                ok: detail.ok !== false,
                elapsedMs: Number(detail.elapsed_ms) || 0,
              },
              createdAt: Date.now(),
            })
            break
          case 'tool_args':
            // The model is composing a call's arguments; the row appears once
            // they are complete, so report progress as a status instead.
            onProgress?.({
              name: String(detail.name ?? ''),
              bytes: Number(detail.bytes) || 0,
            })
            break
          case 'todos':
            if (Array.isArray(detail.todos)) onTodos?.(detail.todos as TodoItem[])
            break
          case 'approval':
            // A turn that needs a decision must be able to ask for one; the
            // dialog is shared with blueprint runs.
            onApproval?.({
              requestId: String(detail.request_id ?? ''),
              detail: event.content ?? '',
            })
            break
          case 'notice':
            onMessage({
              id: `n-${Date.now()}-${seq++}`,
              role: 'notice',
              content: event.content ?? '',
              createdAt: Date.now(),
            })
            break
          case 'done': {
            if (detail.context) onContext?.(parseContextStats(detail.context))
            const usage = (detail.usage ?? {}) as Record<string, unknown>
            onUsage?.({
              turnElapsedMs: parseReasoningDuration(detail.turn_elapsed_ms),
              inputTokens: Number(usage.input_tokens) || 0,
              outputTokens: Number(usage.output_tokens) || 0,
              totalTokens: Number(usage.total_tokens) || 0,
              cachedInputTokens: Number(usage.cached_input_tokens) || 0,
              cacheWriteInputTokens: Number(usage.cache_write_input_tokens) || 0,
            })
            break
          }
          case 'error':
            if (parseReasoningDuration(detail.turn_elapsed_ms) !== undefined) {
              onUsage?.({
                turnElapsedMs: parseReasoningDuration(detail.turn_elapsed_ms),
                inputTokens: 0, outputTokens: 0, totalTokens: 0,
                cachedInputTokens: 0, cacheWriteInputTokens: 0,
              })
            }
            return err(event.content || 'The turn failed.')
          default:
            break
        }
      }
      return ok(undefined)
    } catch (e) {
      return toErr(e)
    }
  }

  async sendInterrupt(
    workspacePath: string,
    message: string,
    priority: 'Normal' | 'Urgent' | 'Emergency' = 'Normal',
  ): Promise<Result<void>> {
    try {
      await this.client.sendInterrupt({ workspacePath, message, priority })
      return ok(undefined)
    } catch (e) {
      return toErr(e)
    }
  }

  async pickDirectory(): Promise<Result<string | null>> {
    try {
      const res = await fetch('/api/pick-directory', { method: 'POST' })
      if (!res.ok) return err(res.statusText || `folder picker failed (${res.status})`)
      const data = (await res.json()) as { path?: string | null }
      return ok(data.path ?? null)
    } catch (e) {
      return toErr(e)
    }
  }

  async abortChat(workspacePath: string): Promise<Result<void>> {
    try {
      await this.client.abortChat({ workspacePath })
      return ok(undefined)
    } catch (e) {
      return toErr(e)
    }
  }

  async listChatSessions(workspacePath: string): Promise<Result<ChatSessionInfo[]>> {
    try {
      const res = await this.client.listChatSessions({ workspacePath })
      return ok(
        res.sessions.map((s) => ({
          sessionId: s.sessionId,
          createdAt: Number(s.createdAt),
          updatedAt: Number(s.updatedAt),
          turns: Number(s.turns),
          title: s.title,
          messageCount: Number(s.messageCount),
        })),
      )
    } catch (e) {
      return toErr(e)
    }
  }

  async getChatSession(workspacePath: string, sessionId?: string): Promise<Result<ChatSessionSnapshot>> {
    try {
      const res = await this.client.getChatSession({ workspacePath, sessionId: sessionId ?? '' })
      let history: ChatMessage[] = []
      try {
        const entries: Array<{ role: string; content: string }> = JSON.parse(res.historyJson)
        history = entries.map((e, i) => ({
          id: `${e.role === 'user' ? 'u' : 'a'}-${i}`,
          role: e.role === 'user' ? 'user' : 'assistant',
          content: e.content,
          createdAt: Number(res.createdAt),
        }))
      } catch {
        history = []
      }
      let todos: TodoItem[] = []
      try {
        todos = JSON.parse(res.todosJson || '[]')
      } catch {
        todos = []
      }
      return ok({
        sessionId: res.sessionId,
        createdAt: Number(res.createdAt),
        history,
        transcript: parseTranscript(res.transcriptJson),
        todos,
      })
    } catch (e) {
      return toErr(e)
    }
  }

  async deleteChatSession(workspacePath: string, sessionId?: string): Promise<Result<void>> {
    try {
      await this.client.deleteChatSession({ workspacePath, sessionId: sessionId ?? '' })
      return ok(undefined)
    } catch (e) {
      return toErr(e)
    }
  }

  async rewindChat(workspacePath: string, sessionId: string, snapshotId: string): Promise<Result<ChatSessionSnapshot>> {
    try {
      const res = await this.client.rewindChat({ workspacePath, sessionId, snapshotId })
      const history = (JSON.parse(res.historyJson || '[]') as Array<{ role: string; content: string }>).map((entry, index) => ({
        id: `restored-${index}`, role: entry.role === 'user' ? 'user' as const : 'assistant' as const,
        content: entry.content, createdAt: Number(res.createdAt),
      }))
      return ok({ sessionId: res.sessionId, createdAt: Number(res.createdAt), history,
        transcript: parseTranscript(res.transcriptJson), todos: JSON.parse(res.todosJson || '[]') })
    } catch (e) {
      return toErr(e)
    }
  }

  // Blueprints -----------------------------------------------------------------
  async listNodeKinds(workspacePath = ''): Promise<Result<NodeCatalog>> {
    try {
      const kinds = await this.client.listNodeKinds({ workspacePath })
      return ok(fromWireCatalog(kinds))
    } catch (e) {
      return toErr(e)
    }
  }

  async listFunctions(workspacePath: string): Promise<Result<FunctionItem[]>> {
    try {
      const list = await this.client.listFunctions({ workspacePath })
      const items: FunctionItem[] = list.functions.map((f) => ({
        id: f.id,
        name: f.name,
        description: f.description,
        source: f.source,
        addonBinding: parseDefault(f.addonBindingJson) as Record<string, unknown> | undefined,
        filePath: f.filePath,
        inputs: (f.inputs ?? []).map((p) => ({ name: p.name, type: pinType(p.dataType), default: parseDefault(p.defaultJson), optional: p.optional, description: p.description })),
        outputs: (f.outputs ?? []).map((p) => ({ name: p.name, type: pinType(p.dataType), default: parseDefault(p.defaultJson), optional: p.optional, description: p.description })),
      }))
      return ok(items)
    } catch (e) {
      return toErr(e)
    }
  }

  async compileDsl(source: string, workspacePath = ''): Promise<Result<Blueprint>> {
    try {
      const bp = await this.client.compileDsl({ source, workspacePath })
      return ok(fromProtoBlueprint(bp))
    } catch (e) {
      return toErr(e)
    }
  }

  async importFunction(workspacePath: string, source: FunctionItem, name: string, filePath: string): Promise<Result<{ name: string; filePath: string }>> {
    try {
      const result = await this.client.saveFunction({ workspacePath, importFrom: source.name, info: { name }, filePath, expectedAddonBindingJson: JSON.stringify(source.addonBinding) })
      if (!result.info) return err('Import result unavailable')
      return ok({ name: result.info.name, filePath: result.info.filePath })
    } catch (e) { return toErr(e) }
  }

  async decompileBlueprint(
    workspacePath: string,
    blueprintOrId: Blueprint | string,
  ): Promise<Result<string>> {
    try {
      const resp =
        typeof blueprintOrId === 'string'
          ? await this.client.decompileBlueprint({ workspacePath, blueprintId: blueprintOrId })
          : await this.client.decompileBlueprint({
              workspacePath,
              blueprintId: '',
              blueprint: toProtoBlueprint(blueprintOrId),
            })
      return ok(resp.source)
    } catch (e) {
      return toErr(e)
    }
  }

  async saveBlueprint(workspacePath: string, blueprint: Blueprint, filePath = ''): Promise<Result<void>> {
    try {
      await this.client.saveBlueprint({ workspacePath, blueprint: toProtoBlueprint(blueprint), filePath, fileJson: JSON.stringify(blueprint, null, 2) })
      return ok(undefined)
    } catch (e) {
      return toErr(e)
    }
  }

  async loadBlueprint(workspacePath: string, blueprintId: string): Promise<Result<Blueprint>> {
    try {
      const bp = await this.client.loadBlueprint({ workspacePath, blueprintId })
      return ok(fromProtoBlueprint(bp))
    } catch (e) {
      return toErr(e)
    }
  }

  // Execution ----------------------------------------------------------------
  async executeBlueprint(
    workspacePath: string,
    blueprintId: string,
    onEvent: (e: ExecutionEvent) => void,
    blueprint?: Blueprint,
  ): Promise<Result<void>> {
    try {
      // The daemon verifies this canvas against the authoritative saved file.
      const blueprintJson = blueprint ? JSON.stringify(blueprint) : ''
      for await (const ev of this.client.executeBlueprint({
        workspacePath,
        blueprintId,
        blueprintJson,
      })) {
        onEvent(fromProtoEvent(ev))
      }
      return ok(undefined)
    } catch (e) {
      return toErr(e)
    }
  }

  async continueExecution(
    workspacePath: string,
    runId: string,
    onEvent: (e: ExecutionEvent) => void,
  ): Promise<Result<void>> {
    try {
      for await (const ev of this.client.continueExecution({ workspacePath, runId })) {
        onEvent(fromProtoEvent(ev))
      }
      return ok(undefined)
    } catch (e) {
      return toErr(e)
    }
  }

  async listExecutions(workspacePath: string): Promise<Result<ExecutionInfo[]>> {
    try {
      const list = await this.client.listExecutions({ workspacePath })
      return ok(
        list.executions.map((x) => ({
          runId: x.runId,
          blueprintId: x.blueprintId,
          status: x.status,
          startedAt: Number(x.startedAt),
          updatedAt: Number(x.updatedAt),
          executedNodes: x.executedNodes,
          snapshot: executionSnapshot(x.dataJson),
        })),
      )
    } catch (e) {
      return toErr(e)
    }
  }

  async getExecutionTree(workspacePath: string, runId: string): Promise<Result<ExecTreeData>> {
    try {
      const res = await this.client.getExecutionTree({ workspacePath, runId })
      return ok({
        nodes: res.nodes.map((n) => ({
          id: n.id,
          kind: n.kind,
          label: n.label,
          parent: n.parent,
          children: [...n.children],
          status: n.status,
          tokens: Number(n.tokens),
          startedAt: Number(n.startedAt),
          finishedAt: Number(n.finishedAt),
        })),
        roots: [...res.roots],
      })
    } catch (e) {
      return toErr(e)
    }
  }

  async cancel(workspacePath: string, runId = ''): Promise<Result<void>> {
    try {
      await this.client.cancelExecution({ workspacePath, runId })
      return ok(undefined)
    } catch (e) {
      return toErr(e)
    }
  }

  async pause(workspacePath: string, runId = ''): Promise<Result<void>> {
    try {
      await this.client.pauseExecution({ workspacePath, runId })
      return ok(undefined)
    } catch (e) {
      return toErr(e)
    }
  }

  async resume(workspacePath: string, runId = ''): Promise<Result<void>> {
    try {
      await this.client.resumeExecution({ workspacePath, runId })
      return ok(undefined)
    } catch (e) {
      return toErr(e)
    }
  }

  // Approvals -----------------------------------------------------------------
  async respondApproval(workspacePath: string, requestId: string, allow: boolean): Promise<Result<void>> {
    try {
      await this.client.respondApproval({ workspacePath, requestId, decision: allow ? 'AllowOnce' : 'DenyOnce' })
      return ok(undefined)
    } catch (e) {
      return toErr(e)
    }
  }

  // Versioning ------------------------------------------------------------------
  async listSnapshots(workspacePath: string): Promise<Result<SnapshotInfo[]>> {
    try {
      const list = await this.client.listSnapshots({ workspacePath })
      return ok(
        list.snapshots.map((s) => ({
          id: s.id,
          alias: s.alias || undefined,
          createdAt: Number(s.createdAt),
          message: s.description,
        })),
      )
    } catch (e) {
      return toErr(e)
    }
  }

  async createSnapshot(
    workspacePath: string,
    description: string,
    alias?: string,
  ): Promise<Result<SnapshotInfo>> {
    try {
      const s = await this.client.createSnapshot({ workspacePath, description, alias: alias ?? '' })
      return ok({
        id: s.id,
        alias: s.alias || undefined,
        createdAt: Number(s.createdAt),
        message: s.description,
      })
    } catch (e) {
      return toErr(e)
    }
  }

  async rollback(workspacePath: string, snapshotId: string, alias?: string): Promise<Result<void>> {
    try {
      await this.client.rollback({ workspacePath, snapshotId, alias: alias ?? '' })
      return ok(undefined)
    } catch (e) {
      return toErr(e)
    }
  }

  async listFileHistory(workspacePath: string, path: string): Promise<Result<FileHistoryEntry[]>> {
    try {
      const list = await this.client.getFileHistory({ workspacePath, path })
      return ok(
        list.entries.map((e) => ({
          path,
          snapshotId: e.snapshotId,
          op: e.status === 'Added' ? 'add' : e.status === 'Deleted' ? 'delete' : 'modify',
          at: Number(e.createdAt),
        })),
      )
    } catch (e) {
      return toErr(e)
    }
  }

  // Addons / usage / resources ----------------------------------------------------
  async listAddons(): Promise<Result<AddonInfo[]>> {
    try {
      const list = await this.client.listAddons({})
      return ok(
        list.addons.map((a) => ({
          id: a.id,
          name: a.name,
          version: a.version,
          description: a.description,
          enabled: a.enabled,
          scope: a.scope,
          toolCount: a.toolCount,
          fragmentCount: a.fragmentCount,
          scopeRoot: a.scopeRoot,
          fingerprint: a.fingerprint,
          status: a.status,
          error: a.error,
          requiredPermissions: a.requiredPermissions,
          grantedPermissions: a.grantedPermissions,
          hooks: a.hooks.map((hook) => ({ ...hook, completed: Number(hook.completed), failed: Number(hook.failed) })),
        })),
      )
    } catch (e) {
      return toErr(e)
    }
  }

  async setAddonEnabled(
    id: string,
    enabled: boolean,
    workspacePath = '',
  ): Promise<Result<void>> {
    try {
      await this.client.setAddonEnabled({ id, workspacePath, enabled })
      return ok(undefined)
    } catch (e) {
      return toErr(e)
    }
  }

  async listMcpServers(workspacePath?: string): Promise<Result<McpServerInfo[]>> {
    try {
      const list = await this.client.listMcpServers({ workspacePath: workspacePath ?? '' })
      return ok(list.servers.map((s) => ({ owner: s.owner, scopeRoot: s.scopeRoot, name: s.name, status: s.status, toolCount: s.toolCount, error: s.error })))
    } catch (e) {
      return toErr(e)
    }
  }

  async getBlackboard(workspacePath: string, runId: string, query: BoardQuery = {}): Promise<Result<Blackboard>> {
    try {
      const response = await this.client.getBlackboard({ workspacePath, runId, queryJson: JSON.stringify(query) })
      const value = JSON.parse(response.projectionJson) as Blackboard
      if (value.run_id !== runId || !Array.isArray(value.entries)) throw new Error('Invalid blackboard response')
      return ok(value)
    } catch (e) { return toErr(e) }
  }

  async listOversightReports(workspacePath: string, runId: string): Promise<Result<OversightReports>> {
    try {
      const response = await this.client.listOversightReports({ workspacePath, runId })
      const data = JSON.parse(response.reportsJson) as OversightReports
      if (data.run_id !== runId || !Array.isArray(data.reports)) throw new Error('Invalid oversight reports')
      return ok(data)
    } catch (e) { return toErr(e) }
  }
  async getConciergeState(workspacePath: string, runId: string, conversationId: string): Promise<Result<ConciergeState>> {
    try {
      const response = await this.client.getConciergeState({ workspacePath, runId, conversationId })
      const value = JSON.parse(response.stateJson) as ConciergeState
      if (value.run_id !== runId || value.conversation_id !== conversationId || !Array.isArray(value.messages) || !Array.isArray(value.requests)) throw new Error('Invalid concierge state')
      return ok(value)
    } catch (e) { return toErr(e) }
  }
  async sendConciergeMessage(workspacePath: string, runId: string, conversationId: string, messageId: string, message: string, onEvent: (event: ConciergeEvent) => void, signal?: AbortSignal): Promise<Result<void>> {
    try {
      for await (const event of this.client.sendConciergeMessage({ workspacePath, runId, conversationId, messageId, message }, { signal })) {
        if (event.runId !== runId || event.messageId !== messageId) throw new Error('Mismatched concierge event')
        const turn = event.kind === 'processing' ? undefined : JSON.parse(event.detailJson)
        if (turn && (turn.id !== messageId || turn.conversation_id !== conversationId)) throw new Error('Mismatched concierge turn')
        onEvent({ runId, messageId, kind: event.kind, turn })
      }
      return ok(undefined)
    } catch (e) { return toErr(e) }
  }
  async getExecutionUsage(workspacePath: string, runId: string): Promise<Result<UsageSummary>> {
    try {
      const u = await this.client.getExecutionUsage({ workspacePath, runId })
      return ok({
        oversight: u.oversightJson ? JSON.parse(u.oversightJson) : undefined,
        currency: u.currency,
        totalCostMicros: Number(u.totalCostMicros),
        models: u.models.map((m) => ({
          model: m.model,
          calls: Number(m.calls),
          inputTokens: Number(m.inputTokens),
          outputTokens: Number(m.outputTokens),
          reasoningTokens: Number(m.reasoningTokens),
          costMicros: Number(m.costMicros),
          cachedInputTokens: Number(m.cachedInputTokens),
          tokensComplete: m.tokensComplete,
          cacheComplete: m.cacheComplete,
          costComplete: m.costComplete,
          cacheWriteInputTokens: Number(m.cacheWriteInputTokens),
        })),
      })
    } catch (e) {
      return toErr(e)
    }
  }
}
