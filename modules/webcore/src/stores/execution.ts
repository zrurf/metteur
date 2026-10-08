import { blueprintApproval } from '@/core/blueprint-approval'
import { defineStore } from 'pinia'
import { computed, ref, watch } from 'vue'
import { gateway } from '@/core'
import type {
  ApprovalRequest,
  Blueprint,
  ContextRegion,
  TodoItem,
  ExecStatus,
  ExecTreeData,
  ExecutionEvent,
  NodeAudit,
  ExecutionInfo,
} from '@/core'
import { useWorkspaceStore } from './workspace'

/** App-facing actions derived from an execution event. */
export interface EventLine {
  nodeId: string
  kind: ExecutionEvent['kind']
  message: string
  detail?: Record<string, unknown>
}

/**
 * Execution surface.
 *
 * Subscribes to the gateway stream, keeps the newest events plus the lifecycle
 * status, and surfaces a pending approval as a modal payload for the user to
 * allow/deny. The active run id is captured from `ListExecutions` after a run
 * starts so a suspended run can be resumed.
 */
export const useExecutionStore = defineStore('execution', () => {
  const workspace = useWorkspaceStore()
  const blueprint = ref<Blueprint | null>(null)
  const sourcePath = ref('')
  const error = ref('')
  const launching = ref(false)
  const current = ref<ExecutionInfo | null>(null)
  const controlBusy = ref(false)
  const connected = ref(false)
  let epoch = 0, query = 0
  const seen = new Map<string, number>()
  const streamActive = ref(false)
  let priorRuns = new Set<string>()
  const active = computed(() => !!current.value?.snapshot?.runtime?.active || streamActive.value && running.value)
  watch(() => workspace.active?.path, () => {
    epoch++; query++; seen.clear(); streamActive.value = false; priorRuns = new Set()
    blueprint.value = null; sourcePath.value = ''; error.value = ''; status.value = 'idle'
    events.value = []; nodeAudits.value = new Map(); clearApprovals(); runId.value = null
    current.value = null; connected.value = false; launching.value = false; controlBusy.value = false
    contextUsage.value = null; contextByNode.value = new Map(); tree.value = null; todos.value = []
  }, { flush: 'sync' })
  const status = ref<ExecStatus>('idle')
  const events = ref<EventLine[]>([])
  const approval = ref<ApprovalRequest | null>(null)
  const approvalQueue: ApprovalRequest[] = []
  let approvalRevision = 0
  function clearApprovals() { approval.value = null; approvalQueue.length = 0; approvalRevision++ }
  function enqueueApproval(request: ApprovalRequest) {
    approvalRevision++
    if (!approval.value || approval.value.id === request.id) approval.value = request
    else if (!approvalQueue.some(item => item.id === request.id)) approvalQueue.push(request)
  }
  /** Most recent context region breakdown (from `context` events). */
  const contextUsage = ref<ContextRegion[] | null>(null)
  const contextNode = ref<string | null>(null)
  /** Per-node context region breakdowns, keyed by node id. */
  const contextByNode = ref<Map<string, ContextRegion[]>>(new Map())
  /** The id of the run belonging to the current execution, if known. */
  const runId = ref<string | null>(null)
  /** The agent execution tree of the latest run (loaded on finish). */
  const tree = ref<ExecTreeData | null>(null)
  /** The agent's task list reported by the running blueprint. */
  const todos = ref<TodoItem[]>([])
  /** Live per-node audit facts, fed by started/finished/node_data/context. */
  const nodeAudits = ref<Map<string, NodeAudit>>(new Map())

  const running = computed(() => status.value === 'running' || status.value === 'paused')
  const lastEvent = computed(() => events.value[events.value.length - 1])
  /** Node whose `started` has no matching `finished` yet. */
  const runningNodeId = computed(() => {
    for (const [id, a] of nodeAudits.value) {
      if (a.startedAt && !a.finishedAt) return id
    }
    return null
  })
  const contextTotal = computed(() =>
    (contextUsage.value ?? []).reduce((sum, r) => sum + r.tokens, 0),
  )

  /** Context breakdown of one node: its own snapshot, else the latest. */
  function contextOf(nodeId: string | null): ContextRegion[] | null {
    if (nodeId) {
      const own = contextByNode.value.get(nodeId)
      if (own) return own
    }
    return contextUsage.value
  }

  function ensureAudit(nodeId: string): NodeAudit {
    let a = nodeAudits.value.get(nodeId)
    if (!a) {
      a = { startedAt: 0, finishedAt: 0, outputs: {}, tokens: 0, message: '' }
      nodeAudits.value.set(nodeId, a)
    }
    return a
  }

  function push(ev: EventLine, limit = 500) {
      if (ev.kind === 'approval_request') {
        const detail = (ev.detail ?? {}) as Record<string, unknown>
        const requestType = String(detail.request_type ?? 'sandbox')
        const plan = blueprintApproval(detail)
        const title =
          requestType === 'circuit_tripped'
            ? 'Circuit breaker: allow auto-replan?'
            : requestType === 'replan_proposal'
              ? 'Approve revised plan'
              : 'Approve shell command'
        enqueueApproval({
          id: String(detail.requestId ?? detail.approvalId ?? ev.message ?? ''),
          title: plan?.title ?? title,
          tool: String(detail.tool ?? ''),
          command: plan?.command ?? ev.message,
          detail: plan?.detail ?? (detail.command ? String(detail.command) : ev.message),
          requestType,
        })
      }
    events.value.push(ev)
    const audit = ev.nodeId ? ensureAudit(ev.nodeId) : null
    if (ev.kind === 'started' && audit) audit.startedAt = Date.now()
    if (ev.kind === 'finished' && audit) audit.finishedAt = Date.now()
    if (ev.kind === 'node_data' && audit && ev.detail && typeof ev.detail === 'object') {
      audit.outputs = { ...audit.outputs, ...(ev.detail.outputs ?? {}) }
    }
    if (ev.kind === 'message' && audit) {
      const summary = ev.detail?.summary
      audit.message = typeof summary === 'string' ? summary : ev.message
    }
    if (ev.kind === 'todos' && Array.isArray(ev.detail?.todos)) {
      todos.value = ev.detail.todos as TodoItem[]
    }
    if (ev.kind === 'context' && Array.isArray(ev.detail?.regions)) {
      const regions = (ev.detail.regions as unknown as ContextRegion[]).map((r) => ({
        region: r.region,
        chars: Number(r.chars) || 0,
        tokens: Number(r.tokens) || 0,
      }))
      contextUsage.value = regions
      contextNode.value = ev.nodeId
      if (ev.nodeId) contextByNode.value.set(ev.nodeId, regions)
      if (audit) {
        audit.tokens = regions.reduce((sum, r) => sum + r.tokens, 0)
      }
    }
    if (events.value.length > limit) events.value.splice(0, events.value.length - limit)
  }

  async function reconcile() {
    const ws = workspace.active?.path, generation = epoch, ticket = ++query, approvalTicket = approvalRevision
    if (!ws || launching.value) return
    try {
      const result = await gateway.listExecutions(ws)
      if (generation !== epoch || ticket !== query) return
      if (!result.ok) throw new Error(result.error)
      connected.value = true
      const candidates = result.data.filter(r => !streamActive.value || runId.value || !priorRuns.has(r.runId))
      const item = candidates.find(r => r.snapshot?.runtime?.active) ?? candidates.find(r => r.runId === runId.value) ?? candidates.at(-1)
      if (!item) return
      if (current.value?.runId === item.runId && current.value.updatedAt > item.updatedAt) return
      current.value = item; runId.value = item.runId
      const runtime = item.snapshot?.runtime
      status.value = runtime?.active ? runtime.pause_requested ? 'paused' : 'running'
        : ({ Running: 'running', Suspended: 'suspended', RecoveryRequired: 'suspended', Completed: 'finished', Cancelled: 'cancelled', Failed: 'failed' } as Record<string, ExecStatus>)[item.status] ?? 'unknown'
      if (item.snapshot?.view?.root) blueprint.value = item.snapshot.view.root
      else if (!streamActive.value) blueprint.value = null
      sourcePath.value = item.snapshot?.blueprint_version?.blueprint_uri ?? sourcePath.value
      if (item.snapshot?.error) error.value = item.snapshot.error
      if (approvalTicket === approvalRevision) {
        if (!runtime?.active) clearApprovals()
        else if (Array.isArray(runtime.pending_approval_ids)) {
          const valid = new Set(runtime.pending_approval_ids)
          for (let i = approvalQueue.length - 1; i >= 0; i--) if (!valid.has(approvalQueue[i]!.id)) approvalQueue.splice(i, 1)
          if (approval.value && !valid.has(approval.value.id)) approval.value = approvalQueue.shift() ?? null
        }
      }
    } catch (e) {
      if (generation !== epoch || ticket !== query) return
      connected.value = false; error.value = String(e)
    }
  }

  async function run(blueprintId: string, graph?: Blueprint, filePath = '') {
    const ws = workspace.active
    if (!ws || running.value || launching.value) return
    const generation = ++epoch
    launching.value = true
    error.value = ''
    try {
      const runsBefore = await gateway.listExecutions(ws.path)
      if (generation !== epoch) return
      if (!runsBefore.ok) throw new Error(runsBefore.error)
      if (runsBefore.data.some(r => r.snapshot?.runtime?.active || r.status === 'Running')) {
        error.value = 'A run is already active in this workspace.'; launching.value = false; await reconcile(); return
      }
      priorRuns = new Set(runsBefore.data.map(r => r.runId))
    } catch (e) { if (generation === epoch) error.value = String(e); return }
    finally { if (generation === epoch) launching.value = false }
    blueprint.value = graph ? JSON.parse(JSON.stringify(graph)) : null
    sourcePath.value = filePath
    current.value = null; connected.value = true; streamActive.value = true
    events.value = []
    seen.clear()
    contextUsage.value = null
    contextNode.value = null
    contextByNode.value = new Map()
    nodeAudits.value = new Map()
    tree.value = null
    todos.value = []
    status.value = 'running'
    runId.value = null
    // Assert the canvas matches the authoritative file saved before Run.
    let result
    try { result = await gateway.executeBlueprint(
      ws.path,
      blueprintId,
      (ev) => {
      if (generation !== epoch) return
      const incomingRun = ev.detail?.run_id
      if (typeof incomingRun === 'string') {
        if (runId.value && runId.value !== incomingRun) return
        runId.value = incomingRun
        const key = `${incomingRun}:${ev.detail?.stream_id}`
        const sequence = Number(ev.detail?.sequence)
        if (Number.isFinite(sequence)) {
          if (sequence <= (seen.get(key) ?? -1)) return
          seen.set(key, sequence)
        }
      }
      push(ev)

      },
      graph,
    ) } catch (e) { result = { ok: false as const, error: String(e) } }
    if (generation !== epoch) return
    clearApprovals()
    if (!result.ok && !['cancelled'].includes(status.value)) {
      status.value = 'failed'
      error.value = result.error
      push({ nodeId: '', kind: 'message', message: result.error })
    }
    await reconcile()
    streamActive.value = false
    if (!current.value && result.ok && status.value === 'running') status.value = 'unknown'
  }

  async function respond(allow: boolean) {
    const ws = workspace.active?.path, request = approval.value, generation = epoch
    if (!ws || !request || controlBusy.value) return
    controlBusy.value = true
    try {
      const result = await gateway.respondApproval(ws, request.id, allow)
      if (generation !== epoch) return
      if (!result.ok) throw new Error(result.error)
      approvalRevision++
      if (approval.value?.id === request.id) approval.value = approvalQueue.shift() ?? null
    } catch (e) { if (generation === epoch) error.value = String(e) }
    finally { if (generation === epoch) controlBusy.value = false }
  }
  async function control(action: 'pause' | 'resume' | 'cancel') {
    const ws = workspace.active?.path, generation = epoch
    if (!ws || !runId.value || controlBusy.value) return
    controlBusy.value = true; error.value = ''
    try {
      const result = await gateway[action](ws, runId.value ?? undefined)
      if (generation !== epoch) return
      if (!result.ok) throw new Error(result.error)
      await reconcile()
    } catch (e) { if (generation === epoch) error.value = String(e) }
    finally { if (generation === epoch) controlBusy.value = false }
  }
  // Live Continue only releases the pause flag. Checkpoint recovery is explicit.
  const pause = () => control('pause')
  const resume = () => control('resume')
  const cancel = () => control('cancel')
  async function recover() {
    const ws = workspace.active?.path, id = runId.value, generation = epoch
    if (!ws || !id || active.value || controlBusy.value) return
    controlBusy.value = true
    try {
      const result = await gateway.continueExecution(ws, id, ev => { if (generation === epoch) push(ev) })
      if (generation !== epoch) return
      if (!result.ok) { status.value = 'failed'; push({ nodeId: '', kind: 'message', message: result.error }); error.value = result.error }
      await reconcile()
    } finally { if (generation === epoch) controlBusy.value = false }
  }

  return {
    blueprint, sourcePath, error, launching, reconcile, current, connected, active, controlBusy, recover,
    status,
    events,
    approval,
    contextUsage,
    contextNode,
    contextByNode,
    contextOf,
    contextTotal,
    runId,
    tree,
    todos,
    running,
    lastEvent,
    nodeAudits,
    runningNodeId,
    run,
    respond,
    pause,
    resume,
    cancel,
  }
})
