import { defineStore } from 'pinia'
import { computed, onScopeDispose, ref, watch } from 'vue'
import { gateway } from '@/core'
import type {
  AddonInfo,
  ChatContextStats,
  ChatMessage,
  ChatOptions,
  ChatSessionInfo,
  ChatUsage,
  TodoItem,
  FileTreeNode,
} from '@/core'
import type { ChatDeliveryReceipt } from '@/core/types'
import type { ChatPhase } from '@/lib/chat/liveness'
import { useWorkspaceStore } from './workspace'
import { CONTINUATION_WAITING_MESSAGE, type TaskRecord } from '@/core/continuations'
import { usePromptResourcesStore } from './prompt-resources'
import { useWorkspaceRefresh } from '@/lib/workspace-refresh'
import { useComposerInputsStore, type ComposerSelection } from './composer-inputs'
import type { DeliverySnapshot } from '@/core/delivery'
import { useForegroundStore } from './foreground'
import { useWorkspaceViewStore } from './workspace-view'
import { isSessionId } from '@/core/workspace-view'
import { useApprovalDecisionsStore } from './approval-decisions'
import { approvalDigest, type ApprovalTarget } from '@/core/approval-decision'
import { useFeedbackStore } from './feedback'
import { useDocumentsStore } from './documents'
import { validFileReference } from '@/core/file-reference'
import { type ResultChoice } from '@/core/workflow'

/** A message typed while a turn was running, waiting to be injected. */
export interface QueuedTurn {
  id: string
  text: string
  /** Files attached to it, rendered as chips until the turn ends. */
  files: FileTreeNode[]
  createdAt: number
}

/**
 * ReAct conversation for the current workspace.
 *
 * Holds the message list and the sending state so the chat surface stays a thin
 * view. Messages are upserted by id; streamed deltas fill a bubble in place
 * (append while `pending`, replace on the final event) instead of appending
 * duplicates. Agent helpers (workspace files for the attach/@-menu and the
 * installed addons) live here so the composer stays stateless and the context
 * survives tab switches.
 *
 * The conversation is persisted daemon-side per workspace: the store restores
 * it when the workspace opens (surviving daemon restarts) and tracks the
 * session id so later turns resume the same session.
 */
export const useChatStore = defineStore('chat', () => {
  const workspace = useWorkspaceStore()
  const foreground = useForegroundStore()
  const views = useWorkspaceViewStore()
  const decisions = useApprovalDecisionsStore()
  const feedback = useFeedbackStore()
  const inputs = useComposerInputsStore(),
    prompts = usePromptResourcesStore()
  const messages = ref<ChatMessage[]>([])
  const streaming = ref(false)
  const busy = ref(false)
  const sendError = ref('')
  const requestedAttachment = ref<{ workspace: string; entity: string; path: string } | null>(null)
  /** id of the persisted session, passed back on every turn. */
  const sessionId = ref('')
  const sessionUnavailable = ref(false)
  const activeTurn = ref('')
  /** Aborts the local event stream; the daemon is stopped separately. */
  let streamAbort: AbortController | null = null
  let pendingSend: Promise<void> | null = null
  const rewinding = ref(false)
  const retrying = ref(false)

  /** All persisted threads, newest first. */
  const threads = ref<ChatSessionInfo[]>([])
  /** Token usage of the most recently completed turn. */
  const lastUsage = ref<ChatUsage | null>(null)
  /** What the conversation occupies in the model's window. */
  const contextStats = ref<ChatContextStats | null>(null)
  /** Who answers authorization questions for this conversation. */
  const permissionMode = ref<'ask' | 'sandbox' | 'full'>('sandbox')
  /** A sandbox approval the running turn is waiting for. */
  const approval = ref<{
    requestId: string
    detail: string
    contentDigest?: string
    target?: ApprovalTarget
  } | null>(null)
  const approvalSubmitting = ref(false),
    approvalError = ref('')
  const approvalDecision = computed(() =>
    approval.value && workspace.active?.path
      ? decisions.read(workspace.active.path, approval.value.requestId)
      : null,
  )
  /** The agent's task list for the open conversation. */
  const todos = ref<TodoItem[]>([])
  /** What the agent is doing right now, derived from the event stream. */
  const phase = ref<ChatPhase>('idle')
  /** When the current turn started; drives the elapsed timers. */
  const turnStartedAt = ref(0)
  /** Timestamp of the last event of the current turn. */
  const lastEventAt = ref(0)
  /** Messages typed while the agent worked, injected into the running turn. */
  const queued = ref<QueuedTurn[]>([])
  /** Tool call whose arguments are still being written, with their size. */
  const pendingTool = ref<{ name: string; bytes: number } | null>(null)

  // Files / addons driving the composer's attach, @-mention and plugin toggles.
  const files = ref<FileTreeNode[]>([])
  const filesLoading = ref(false)
  const addons = ref<AddonInfo[]>([])
  const addonsLoading = ref(false)
  /** Files queued in the composer (chips) but not yet sent as a turn. */
  const pendingFiles = ref<FileTreeNode[]>([])
  const pendingResults = ref<ResultChoice[]>([])
  const draftEntity = computed(() => {
    return inputs.entity(workspace.active?.path ?? '', sessionId.value)
  })
  const composerInput = computed(() =>
    workspace.active?.path ? inputs.read(workspace.active.path, draftEntity.value) : null,
  )
  const draft = computed({
    get: () => composerInput.value?.input.raw ?? '',
    set: (raw: string) => {
      if (workspace.active?.path) inputs.edit(workspace.active.path, draftEntity.value, { raw })
    },
  })
  let changingDraft = false
  watch(
    [() => workspace.active?.path, draftEntity, () => composerInput.value?.backup?.id],
    () => {
      const current = composerInput.value
      changingDraft = true
      const chosen = current ? inputs.selections(current.input) : []
      pendingFiles.value = chosen.flatMap((item) => (item.kind === 'file' ? [item.file] : []))
      pendingResults.value = chosen.flatMap((item) => (item.kind === 'result' ? [item.result] : []))
      prompts.selections = chosen.flatMap((item) =>
        item.kind === 'prompt' ? [item.selection] : [],
      )
      changingDraft = false
    },
    { immediate: true, flush: 'sync' },
  )
  watch(
    [pendingFiles, pendingResults, () => prompts.selections],
    () => {
      if (changingDraft || !workspace.active?.path) return
      const selections: ComposerSelection[] = [
        ...pendingFiles.value.map((file) => ({ kind: 'file' as const, file })),
        ...prompts.selections.map((selection) => ({ kind: 'prompt' as const, selection })),
        ...pendingResults.value.map((result) => ({ kind: 'result' as const, result })),
      ]
      // Keep existing mixed-source order, replacing payloads only at the same identity.
      const key = (item: ComposerSelection) =>
        item.kind === 'file'
          ? 'file:' + item.file.path
          : item.kind === 'result'
            ? 'result:' + JSON.stringify(item.result.reference)
            : 'prompt:' + JSON.stringify(item.selection.snapshot.reference)
      const previous = inputs.currentSelections(workspace.active.path, draftEntity.value)
      const ordered = previous.flatMap((item) => {
        const index = selections.findIndex((candidate) => key(candidate) === key(item))
        return index < 0 ? [] : selections.splice(index, 1)
      })
      inputs.edit(workspace.active.path, draftEntity.value, {
        selections: [...ordered, ...selections],
      })
    },
    { deep: true, flush: 'sync' },
  )
  function rotateDraft() {
    if (workspace.active?.path) inputs.rotateNewChat(workspace.active.path)
  }
  const following = ref<{ workspace: string; session: string; task: string } | null>(null)
  let followEpoch = 0,
    conversationEpoch = 0
  function cancelFollow() {
    followEpoch++
  }
  function followTask(task: unknown) {
    if (typeof task === 'string' && task && workspace.active?.path && sessionId.value)
      following.value = { workspace: workspace.active.path, session: sessionId.value, task }
  }
  async function refreshContinuation(epoch: number) {
    const target = following.value
    if (!target) return
    const current = () =>
      epoch === followEpoch &&
      following.value === target &&
      workspace.active?.path === target.workspace &&
      sessionId.value === target.session &&
      !streaming.value &&
      !busy.value &&
      !rewinding.value &&
      !retrying.value &&
      !queued.value.length
    if (!current()) return
    {
      const result = await gateway.resourceRequest('queryContinuations', target.workspace, {
        operation: 'read',
        id: target.task,
      })
      if (!current() || !result.ok) return
      const record = result.data as TaskRecord
      if (record.task.id !== target.task || record.task.session !== target.session) {
        following.value = null
        return
      }
      const snapshot = await gateway.getChatSession(target.workspace, target.session)
      if (!current() || !snapshot.ok || snapshot.data.sessionId !== target.session) return
      const remote = snapshot.data.transcript.length
        ? snapshot.data.transcript
        : snapshot.data.history
      messages.value = remote.filter((message) => message.detail?.continuation_task !== target.task)
      todos.value = snapshot.data.todos ?? []
      // Session snapshots do not carry the resumed turn's usage/context receipt.
      lastUsage.value = null
      contextStats.value = null
      const state = record.task.state
      if (state !== 'completed') {
        const status: Record<string, string> = {
          waiting: CONTINUATION_WAITING_MESSAGE,
          wait_preparing: 'Task is preparing its saved wait.',
          ready: 'Task is ready for its authorized continuation.',
          dispatching: 'Task is continuing from its owned result.',
          active: 'Task is continuing from its owned result.',
          paused: 'Task is paused. Manage it in Tasks.',
          stopped: 'Task is stopped. No automatic replay will occur.',
          needs_attention: 'Task needs attention. Review it in Tasks.',
        }
        messages.value.push({
          id: `waiting-${target.task}`,
          role: 'notice',
          createdAt: Date.now(),
          content: status[state] ?? 'Task status requires review in Tasks.',
          detail: { continuation_task: target.task },
        })
      }
      if (
        ![
          'waiting',
          'wait_preparing',
          'ready',
          'dispatching',
          'active',
          'paused',
          'needs_attention',
        ].includes(state)
      )
        following.value = null
    }
  }
  onScopeDispose(cancelFollow)
  useWorkspaceRefresh(
    () => refreshContinuation(followEpoch),
    [
      following,
      streaming,
      busy,
      rewinding,
      retrying,
      () => queued.value.length,
      sessionId,
      () => gateway.connected.value,
    ],
  )
  // Switching workspaces resets the surface and restores the new session.
  // Immediate so a store created after the workspace was already opened still
  // loads the persisted conversation (otherwise restore would never run).
  watch(
    () => workspace.active?.path,
    async () => {
      conversationEpoch++
      streamAbort?.abort()
      streamAbort = null
      following.value = null
      messages.value = []
      streaming.value = false
      busy.value = false
      sessionId.value = ''
      sessionUnavailable.value = false
      activeTurn.value = ''
      permissionMode.value = 'sandbox'
      approval.value = null
      approvalSubmitting.value = false
      approvalError.value = ''
      sendError.value = ''
      lastUsage.value = null
      contextStats.value = null
      todos.value = []
      phase.value = 'idle'
      turnStartedAt.value = 0
      lastEventAt.value = 0
      queued.value = []
      pendingTool.value = null
      if (workspace.active?.path) await restore()
    },
    { immediate: true, flush: 'sync' },
  )

  /** Reload the persisted conversation of the active workspace, if any. */
  async function restore(): Promise<void> {
    const ws = workspace.active
    if (!ws) return
    const target = ws.path
    const epoch = conversationEpoch
    busy.value = true
    const view = await views.ready(target)
    if (workspace.active?.path !== target || epoch !== conversationEpoch) return
    if (!gateway.demo && views.forWorkspace(target).error) {
      busy.value = false
      sessionUnavailable.value = true
      sendError.value =
        'The saved view is unavailable. Your drafts were kept. Select a conversation explicitly or start a new one.'
      return
    }
    const sessions = await gateway.listChatSessions(target)
    if (workspace.active?.path !== target || epoch !== conversationEpoch) return
    if (!sessions.ok) {
      busy.value = false
      sessionUnavailable.value = true
      sendError.value =
        'Conversations could not be loaded. Your drafts were kept. Reconnect or select a conversation explicitly.'
      return
    }
    threads.value = sessions.data
    if (view.session === '') {
      busy.value = false
      return
    }
    const selected = view.session ? { sessionId: view.session, title: '' } : sessions.data[0]
    if (selected) await openThread(target, selected.sessionId, selected.title)
    else busy.value = false
  }

  /** Reload the thread list without touching the open conversation. */
  async function refreshThreads(): Promise<void> {
    const ws = workspace.active
    if (!ws) return
    const sessions = await gateway.listChatSessions(ws.path)
    if (workspace.active?.path === ws.path && sessions.ok) threads.value = sessions.data
  }

  /** Switch the surface to an existing thread. */
  async function switchTo(threadId: string): Promise<void> {
    const ws = workspace.active
    if (!ws) return
    await openThread(ws.path, threadId, '')
  }

  /** A missing selected source stays unavailable; never substitute another thread. */
  async function openThread(wsPath: string, id: string, title: string): Promise<void> {
    if (!gateway.demo && !isSessionId(id)) {
      sendError.value = 'The conversation identity is invalid. Select a valid conversation.'
      sessionUnavailable.value = true
      return
    }
    const epoch = ++conversationEpoch
    approval.value = null
    approvalError.value = ''
    approvalSubmitting.value = false
    following.value = null
    streamAbort?.abort()
    streamAbort = null
    streaming.value = false
    busy.value = true
    phase.value = 'idle'
    activeTurn.value = ''
    const snap = await gateway.getChatSession(wsPath, id)
    // The user may have switched workspaces while loading; discard stale data.
    if (workspace.active?.path !== wsPath || epoch !== conversationEpoch) return
    sessionId.value = id
    if (isSessionId(id)) views.edit(wsPath, { session: id })
    busy.value = false
    sessionUnavailable.value = !snap.ok || snap.data.sessionId !== id
    if (snap.ok && !sessionUnavailable.value) {
      sendError.value = ''
      // The transcript is the displayed conversation; the history is the
      // model's view and has no tool calls. Fall back only for sessions that
      // predate transcripts.
      messages.value = snap.data.transcript.length ? snap.data.transcript : snap.data.history
      sessionId.value = snap.data.sessionId
      todos.value = snap.data.todos ?? []
      followTask(
        messages.value.findLast((message) => typeof message.detail?.continuation_task === 'string')
          ?.detail?.continuation_task,
      )
    } else {
      messages.value = []
      todos.value = []
      sendError.value =
        'This conversation is missing or unavailable. Its drafts were kept; select another conversation or start a new one.'
    }
    if (title) {
      const info = threads.value.find((t) => t.sessionId === id)
      if (info) info.title = title
    }
  }

  /** Load the workspace file list into the attach / @-mention menu. */
  async function loadFiles(): Promise<void> {
    const ws = workspace.active
    if (!ws) return
    filesLoading.value = true
    try {
      const r = await gateway.listFiles(ws.path, '')
      if (r.ok) files.value = r.data
    } finally {
      filesLoading.value = false
    }
  }

  /** Load the installed addons (plugin toggles in the action bar). */
  async function loadAddons(): Promise<void> {
    addonsLoading.value = true
    try {
      const r = await gateway.listAddons()
      if (r.ok)
        addons.value = r.data.filter(
          (a) => a.scope === 'global' || a.scopeRoot === workspace.active?.path,
        )
    } finally {
      addonsLoading.value = false
    }
  }

  /** Persist a plugin switch and refresh the list. */
  async function toggleAddon(id: string, enabled: boolean): Promise<void> {
    const target = addons.value.find((a) => a.id === id)
    if (!target || (target.scope === 'workspace' && !target.scopeRoot)) return
    const r = await gateway.setAddonEnabled(id, enabled, target.scopeRoot ?? '')
    if (r.ok) await loadAddons()
  }

  /** Queue a file chip in the composer (deduped by path). */
  function queueFile(f: FileTreeNode): void {
    if (f.kind !== 'file' || pendingFiles.value.some((x) => x.path === f.path)) return
    pendingFiles.value = [...pendingFiles.value, f]
  }
  function contextOrder(chosen: ComposerSelection[]): NonNullable<ChatOptions['context_order']> {
    let file = 0,
      prompt = 0,
      result = 0
    return chosen.flatMap<NonNullable<ChatOptions['context_order']>[number]>((item) =>
      item.kind === 'prompt'
        ? [{ kind: 'prompt', index: prompt++ }]
        : item.kind === 'result'
          ? [{ kind: 'result', index: result++ }]
          : item.file.reference
            ? [{ kind: 'file', index: file++ }]
            : [],
    )
  }

  /** Drop a queued chip. */
  function removeQueuedFile(path: string): void {
    pendingFiles.value = pendingFiles.value.filter((x) => x.path !== path)
  }

  /**
   * Appends a turn and streams the reply.
   *
   * Before the turn runs, the daemon captures a paired file/context checkpoint:
   * it is what the transcript's "Restore" action rolls back to, and without it
   * an agent's edits cannot be undone from the conversation.
   */
  async function send(
    content: string,
    options?: ChatOptions,
    mode: 'normal' | 'retry' = 'normal',
    onAccepted?: () => void,
  ): Promise<boolean> {
    const ws = workspace.active?.path,
      targetSession = sessionId.value,
      entity = draftEntity.value
    if (
      !ws ||
      sessionUnavailable.value ||
      !content.trim() ||
      streaming.value ||
      busy.value ||
      rewinding.value ||
      (retrying.value && mode !== 'retry')
    )
      return false
    const input = inputs.read(ws, entity).controller,
      source = input.capture(),
      chosen = mode === 'retry' ? [] : inputs.selections(source)
    if (mode !== 'retry' && chosen.length !== source.selections.length) {
      sendError.value =
        'Some recovered references need review. Keep or copy the input before sending.'
      return false
    }
    if (
      !gateway.demo &&
      chosen.some(
        (item) => item.kind === 'file' && !validFileReference(item.file.reference, item.file.path),
      )
    ) {
      sendError.value =
        'Review recovered file references and choose their saved versions before sending.'
      return false
    }
    const refs = chosen.flatMap((item) =>
        item.kind === 'file' ? [`@file ${item.file.path}`] : [],
      ),
      body = refs.length ? [...refs, content].join('\n') : content,
      epoch = ++conversationEpoch,
      current = () => workspace.active?.path === ws && epoch === conversationEpoch,
      aborter = new AbortController(),
      history = JSON.parse(JSON.stringify(messages.value)) as ChatMessage[]
    const snapshot: DeliverySnapshot = {
      workspace: ws,
      session: targetSession,
      newSession: targetSession ? undefined : entity,
      message: body,
      history,
      options: {
        ...options,
        ...(mode !== 'retry'
          ? {
              file_references: chosen.flatMap((item) =>
                item.kind === 'file' && item.file.reference ? [item.file.reference] : [],
              ),
              context_order: contextOrder(chosen),
              result_references: chosen.flatMap((item) =>
                item.kind === 'result' ? [item.result.reference] : [],
              ),
            }
          : {}),
      },
    }
    const fixedFileReferences = JSON.parse(JSON.stringify(snapshot.options.file_references ?? []))
    const fixedResultReferences = JSON.parse(
      JSON.stringify(snapshot.options.result_references ?? []),
    )
    let received = false,
      executionOk = false
    const receivedInput = (receipt?: ChatDeliveryReceipt) => {
      if (received) return
      received = true
      if (!current()) return
      following.value = null
      if (receipt) {
        sessionId.value = receipt.session
        if (isSessionId(receipt.session)) views.edit(ws, { session: receipt.session })
        activeTurn.value = receipt.turn
      }
      const id = `u-${receipt?.id ?? crypto.randomUUID()}`
      if (!messages.value.some((message) => message.id === id))
        messages.value.push({
          id,
          role: 'user',
          content: body,
          createdAt: receipt?.accepted_ms ?? Date.now(),
          checkpoint: receipt?.checkpoint ?? undefined,
          fileReferences: fixedFileReferences,
          resultReferences: fixedResultReferences,
          contextOrder: snapshot.options.context_order,
        })
      streaming.value = true
      lastUsage.value = null
      pendingTool.value = null
      turnStartedAt.value = Date.now()
      lastEventAt.value = Date.now()
      phase.value = 'thinking'
      if (mode !== 'retry') {
        // Only the exact submitted version is consumed by its input owner.
        // A newer draft and its references stay bound to this conversation.
        const remaining = inputs.selections(input.primary.input)
        changingDraft = true
        pendingFiles.value = remaining.flatMap((item) => (item.kind === 'file' ? [item.file] : []))
        pendingResults.value = remaining.flatMap((item) =>
          item.kind === 'result' ? [item.result] : [],
        )
        prompts.selections = remaining.flatMap((item) =>
          item.kind === 'prompt' ? [item.selection] : [],
        )
        changingDraft = false
      }
      onAccepted?.()
    }
    streamAbort = aborter
    sendError.value = ''
    busy.value = true
    let resolveFinished!: () => void
    const finished = new Promise<void>((resolve) => {
      resolveFinished = resolve
    })
    pendingSend = finished
    const run = async (
      fixed: DeliverySnapshot,
      onReceipt: (receipt: ChatDeliveryReceipt) => void,
    ) => {
      if (!current() || aborter.signal.aborted)
        return { ok: false as const, error: 'Conversation changed before submission.' }
      if (
        !(await foreground.permits(mode === 'retry' ? 'retry' : 'send', targetSession)) ||
        !current() ||
        aborter.signal.aborted
      )
        return {
          ok: false as const,
          error: foreground.current.reason || 'Workspace is unavailable for ordinary chat input.',
        }
      const ids: string[] = []
      // Re-select each original reference against current access and version.
      // Restored snapshot IDs are input, never an authorization token.
      for (const item of chosen) {
        if (item.kind !== 'prompt') continue
        const reference = item.selection.snapshot.reference
        const read = await gateway.resourceRequest('queryPromptResources', ws, {
          operation: 'read',
          scope: reference.scope.scope,
          id: reference.id,
        })
        if (!current() || aborter.signal.aborted)
          return { ok: false as const, error: 'Conversation changed before submission.' }
        const latest = read.ok ? (read.data as { reference: typeof reference }).reference : null
        if (
          !latest ||
          latest.revision !== reference.revision ||
          latest.content_digest !== reference.content_digest
        )
          return {
            ok: false as const,
            error: read.ok
              ? 'A selected prompt changed. Review and refresh it before sending.'
              : read.error,
          }
        const selected = await prompts.select(latest, { kind: 'chat' })
        ids.push(selected.id)
      }
      if (!current() || aborter.signal.aborted)
        return { ok: false as const, error: 'Conversation changed before submission.' }
      const result = await gateway.sendChat(
        ws,
        fixed.message,
        fixed.history,
        (m) => {
          if (current() && (received || gateway.demo)) upsert(m)
        },
        { ...fixed.options, prompt_snapshots: ids },
        (id, checkpoint) => {
          if (!current() || (!gateway.demo && !received)) return
          if (gateway.demo) {
            if (mode !== 'retry') input.acceptLocal(source)
            receivedInput()
          }
          sessionId.value = id
          if (isSessionId(id)) views.edit(ws, { session: id })
          const user = messages.value.findLast((message) => message.role === 'user')
          if (user && checkpoint) user.checkpoint = checkpoint
        },
        fixed.session,
        (usage) => {
          if (current()) {
            lastUsage.value = usage
            finishReasoning(usage.turnElapsedMs)
          }
        },
        (next) => {
          if (current()) todos.value = next
        },
        aborter.signal,
        (progress) => {
          if (current()) {
            pendingTool.value = progress
            settleReasoningStream()
            lastEventAt.value = Date.now()
            phase.value = 'generating'
          }
        },
        (stats) => {
          if (current()) contextStats.value = stats
        },
        (request) => {
          if (current()) approval.value = request
        },
        onReceipt,
      )
      executionOk = result.ok
      if (current()) {
        finishReasoning()
        if (!result.ok)
          messages.value.push({
            id: `e-${crypto.randomUUID()}`,
            role: 'error',
            content: result.error || 'The turn failed.',
            createdAt: Date.now(),
          })
      }
      return result
    }
    try {
      const accepted = gateway.demo
        ? (await run(snapshot, () => {})).ok
        : await inputs.submit(ws, entity, snapshot, run, {
            source,
            mode: mode === 'retry' ? 'retry' : 'send',
            isCurrent: () => current() && !aborter.signal.aborted,
            accepted: receivedInput,
          })
      if (current()) await refreshThreads()
      return mode === 'retry' ? accepted && executionOk : accepted
    } finally {
      if (current()) {
        if (streamAbort === aborter) streamAbort = null
        phase.value = 'idle'
        finishReasoning()
        for (const message of messages.value) {
          message.pending = false
          message.reasoningPending = false
        }
        streaming.value = false
        busy.value = false
      }
      if (pendingSend === finished) pendingSend = null
      resolveFinished()
    }
  }

  /** Rewind files, persisted model context and display transcript as one operation. */
  async function rewind(snapshotId: string): Promise<string | null> {
    const ws = workspace.active
    if (!ws || !sessionId.value) return 'No active conversation to restore.'
    if (rewinding.value) return 'A restore is already in progress.'
    const targetSession = sessionId.value
    rewinding.value = true
    try {
      const pending = pendingSend
      if (streaming.value) await abort()
      if (pending) await pending
      busy.value = true
      if (!(await useDocumentsStore().protectWorkspaceDrafts(ws.path)))
        return 'Current file drafts could not be protected. Keep or export them before restoring.'
      if (workspace.active?.path !== ws.path || sessionId.value !== targetSession)
        return 'Conversation changed before restoring files.'
      const result = await gateway.rewindChat(ws.path, targetSession, snapshotId)
      if (!result.ok) return result.error
      workspace.invalidateTree(ws.path)
      // A different workspace/thread may have been opened while the RPC ran.
      if (workspace.active?.path !== ws.path || sessionId.value !== targetSession) return null
      messages.value = result.data.transcript.length ? result.data.transcript : result.data.history
      sessionId.value = result.data.sessionId
      todos.value = result.data.todos ?? []
      queued.value = []
      pendingTool.value = null
      approval.value = null
      contextStats.value = null
      lastUsage.value = null
      phase.value = 'idle'
      turnStartedAt.value = 0
      await refreshThreads()
      return null
    } catch (error) {
      return error instanceof Error ? error.message : String(error)
    } finally {
      rewinding.value = false
      busy.value = false
    }
  }

  /** Restore the selected answer's starting state, then regenerate automatically. */
  async function retry(assistantId: string, options?: ChatOptions): Promise<string | null> {
    if (retrying.value || rewinding.value || busy.value)
      return 'Wait for the current operation to finish before retrying.'
    const targetWorkspace = workspace.active?.path
    const targetSession = sessionId.value
    if (!(await foreground.permits('retry', targetSession)))
      return foreground.current.reason || 'Workspace availability is unconfirmed.'
    if (retrying.value || rewinding.value || busy.value)
      return 'Another conversation operation is in progress.'
    if (workspace.active?.path !== targetWorkspace || sessionId.value !== targetSession)
      return 'Conversation changed before retry.'
    const index = messages.value.findIndex(
      (message) => message.id === assistantId && message.role === 'assistant',
    )
    if (index < 0) return 'The answer is no longer in this conversation.'
    let original: ChatMessage | undefined
    for (let i = index - 1; i >= 0; i--) {
      if (messages.value[i].role === 'user') {
        original = messages.value[i]
        break
      }
    }
    if (!original?.checkpoint)
      return 'This turn has no paired context checkpoint. It cannot be retried safely without retaining the old answer.'
    const prompt = original.content
    const checkpoint = original.checkpoint
    retrying.value = true
    try {
      const error = await rewind(checkpoint)
      if (error !== null) return error
      if (workspace.active?.path !== targetWorkspace || sessionId.value !== targetSession) {
        return 'Conversation changed during retry; the prompt was not sent to another conversation.'
      }
      // Send directly; never seed from the discarded messages or consume the
      // unrelated draft/attachments currently waiting in the composer.
      // Mark this as a session-bound retry. The daemon will reject a retry
      // without a restored session instead of seeding a new context from the
      // display history (which could reintroduce the discarded answer).
      return (await send(
        prompt,
        {
          ...options,
          file_references: original.fileReferences ?? [],
          result_references: original.resultReferences ?? [],
          context_order: original.contextOrder,
          retry: true,
        },
        'retry',
      ))
        ? null
        : 'The old turn was restored, but the new reply failed.'
    } finally {
      retrying.value = false
    }
  }

  /** Clear only a receipt for this exact pending approval; failures remain visible. */
  async function respondApproval(allow: boolean): Promise<void> {
    const ws = workspace.active?.path,
      pending = approval.value,
      session = sessionId.value,
      turn = activeTurn.value,
      epoch = conversationEpoch
    if (!ws || !pending || approvalSubmitting.value) return
    approvalSubmitting.value = true
    approvalError.value = ''
    const current = () =>
      workspace.active?.path === ws &&
      conversationEpoch === epoch &&
      sessionId.value === session &&
      activeTurn.value === turn &&
      approval.value?.requestId === pending.requestId
    try {
      if (gateway.demo) {
        const result = await gateway.respondApproval(ws, pending.requestId, allow)
        if (!current()) return
        if (result.ok) approval.value = null
        else approvalError.value = result.error
        return
      }
      const digest = pending.contentDigest || (await approvalDigest(pending.detail))
      if (!current()) return
      const entry = decisions.entry({
        workspace: ws,
        id: pending.requestId,
        target: pending.target ?? { kind: 'chat', session, turn },
        content_digest: digest,
      })
      const accepted = await entry.respond(allow)
      if (!current()) return
      approvalError.value = entry.error
      if (accepted || entry.state === 'retired') {
        approval.value = null
        if (entry.error) {
          sendError.value = entry.error
          feedback.toast('error', 'Decision Receipt', entry.error)
        }
      }
    } catch (error) {
      if (current()) approvalError.value = String(error)
    } finally {
      if (workspace.active?.path === ws && conversationEpoch === epoch)
        approvalSubmitting.value = false
    }
  }
  async function reconcileApproval() {
    const ws = workspace.active?.path,
      pending = approval.value,
      epoch = conversationEpoch,
      session = sessionId.value,
      turn = activeTurn.value
    if (!ws || !pending || approvalSubmitting.value) return
    approvalSubmitting.value = true
    try {
      const digest = pending.contentDigest || (await approvalDigest(pending.detail))
      if (
        workspace.active?.path !== ws ||
        epoch !== conversationEpoch ||
        sessionId.value !== session ||
        activeTurn.value !== turn ||
        approval.value?.requestId !== pending.requestId
      )
        return
      const entry = decisions.entry({
        workspace: ws,
        id: pending.requestId,
        target: pending.target ?? { kind: 'chat', session, turn },
        content_digest: digest,
      })
      const accepted = await entry.reconcile()
      if (
        workspace.active?.path !== ws ||
        epoch !== conversationEpoch ||
        approval.value?.requestId !== pending.requestId
      )
        return
      approvalError.value = entry.error
      if (accepted || entry.state === 'retired') {
        approval.value = null
        if (entry.error) {
          sendError.value = entry.error
          feedback.toast('error', 'Decision Receipt', entry.error)
        }
      }
    } catch (error) {
      if (workspace.active?.path === ws && epoch === conversationEpoch)
        approvalError.value = String(error)
    } finally {
      if (workspace.active?.path === ws && epoch === conversationEpoch)
        approvalSubmitting.value = false
    }
  }
  async function stopApproval() {
    const ws = workspace.active?.path,
      pending = approval.value
    const target = pending?.target
    if (
      !target ||
      target.kind !== 'chat' ||
      !target.session ||
      !target.turn ||
      (target.session === sessionId.value && target.turn === activeTurn.value)
    )
      return abort()
    if (!ws || !pending) return
    const result = await gateway.abortChat(ws, {
      kind: 'chat',
      sessionId: target.session,
      turnId: target.turn,
    })
    if (workspace.active?.path !== ws || approval.value?.requestId !== pending.requestId) return
    if (result.ok) approval.value = null
    else approvalError.value = result.error
  }

  async function abort() {
    const ws = workspace.active
    if (!ws) return
    const target = { kind: 'chat' as const, sessionId: sessionId.value, turnId: activeTurn.value }
    const epoch = ++conversationEpoch
    // Stop reading locally first: the daemon's terminal event would otherwise
    // arrive after the user asked to stop and repaint the turn.
    streamAbort?.abort()
    finishReasoning()
    streamAbort = null
    const result =
      gateway.demo || (target.sessionId && target.turnId)
        ? await gateway.abortChat(ws.path, target)
        : {
            ok: false as const,
            error:
              'The chat turn is unconfirmed. Reconcile the input receipt before issuing a targeted Stop.',
          }
    if (workspace.active?.path !== ws.path || epoch !== conversationEpoch) return
    if (!result.ok) sendError.value = result.error
    else approval.value = null
    for (const m of messages.value) m.pending = false
    streaming.value = false
    busy.value = false
    phase.value = 'idle'
    void refreshLiveInputs()
  }

  /** Both entry points freeze the same input and root turn. Receiving a body
   * is independent of whether the turn later reaches another model call. */
  async function submitLive(text: string, mode: 'queue' | 'steer'): Promise<boolean> {
    const ws = workspace.active?.path,
      session = sessionId.value,
      turn = activeTurn.value,
      entity = draftEntity.value,
      epoch = conversationEpoch
    if (!ws || !text.trim() || !streaming.value || !session || (!gateway.demo && !turn))
      return false
    const input = inputs.read(ws, entity).controller,
      source = input.capture(),
      chosen = inputs.selections(source)
    if (!input.canSubmit || chosen.length !== source.selections.length) {
      sendError.value = 'Review or reconcile the current input before submitting.'
      return false
    }
    if (
      !gateway.demo &&
      chosen.some(
        (item) => item.kind === 'file' && !validFileReference(item.file.reference, item.file.path),
      )
    ) {
      sendError.value =
        'Review recovered file references and choose their saved versions before submitting.'
      return false
    }
    const current = () =>
      workspace.active?.path === ws && epoch === conversationEpoch && sessionId.value === session
    const refs = chosen.flatMap((item) => (item.kind === 'file' ? [`@file ${item.file.path}`] : []))
    const body = refs.length ? [...refs, text].join('\n') : text
    const snapshot: DeliverySnapshot = {
      workspace: ws,
      session,
      message: body,
      history: [],
      options: {
        file_references: chosen.flatMap((item) =>
          item.kind === 'file' && item.file.reference ? [item.file.reference] : [],
        ),
        context_order: contextOrder(chosen),
        result_references: chosen.flatMap((item) =>
          item.kind === 'result' ? [item.result.reference] : [],
        ),
      },
      live: { mode, turn },
    }
    const accepted = (receipt?: ChatDeliveryReceipt) => {
      if (!current()) return
      const id = receipt?.id ?? crypto.randomUUID()
      if (!messages.value.some((message) => message.id === `q-${id}`)) {
        queued.value.push({
          id,
          text: body,
          files: chosen.flatMap((item) => (item.kind === 'file' ? [item.file] : [])),
          createdAt: Date.now(),
        })
        messages.value.push({
          id: `q-${id}`,
          role: 'user',
          content: body,
          createdAt: Date.now(),
          queued: true,
          inputStatus: receipt ? 'queued' : undefined,
        })
      }
      changingDraft = true
      const remaining = inputs.selections(input.primary.input)
      pendingFiles.value = remaining.flatMap((item) => (item.kind === 'file' ? [item.file] : []))
      pendingResults.value = remaining.flatMap((item) =>
        item.kind === 'result' ? [item.result] : [],
      )
      prompts.selections = remaining.flatMap((item) =>
        item.kind === 'prompt' ? [item.selection] : [],
      )
      changingDraft = false
      void refreshLiveInputs()
    }
    const run = async (
      fixed: DeliverySnapshot,
      onReceipt: (receipt: ChatDeliveryReceipt) => void,
    ) => {
      if (
        !current() ||
        !streaming.value ||
        !(await foreground.permits(mode, session)) ||
        foreground.current.turn !== turn ||
        !current()
      )
        return { ok: false as const, error: 'The targeted turn changed. Your input is kept.' }
      const ids: string[] = []
      for (const item of chosen) {
        if (item.kind !== 'prompt') continue
        const reference = item.selection.snapshot.reference
        const read = await gateway.resourceRequest('queryPromptResources', ws, {
          operation: 'read',
          scope: reference.scope.scope,
          id: reference.id,
        })
        const latest = read.ok ? (read.data as { reference: typeof reference }).reference : null
        if (
          !current() ||
          !latest ||
          latest.revision !== reference.revision ||
          latest.content_digest !== reference.content_digest
        )
          return {
            ok: false as const,
            error: 'A selected prompt changed or is unavailable. Review it before submitting.',
          }
        ids.push((await prompts.select(latest, { kind: 'chat' })).id)
      }
      if (!current())
        return { ok: false as const, error: 'Conversation changed before submission.' }
      if (gateway.demo)
        return gateway.sendInterrupt(ws, body, mode === 'queue' ? 'Normal' : 'Urgent', {
          kind: 'chat',
          sessionId: session,
          turnId: turn,
        })
      const result = await gateway.resourceRequest('chatDelivery', ws, {
        operation: 'live_submit',
        id: fixed.options.delivery!.id,
        session,
        turn,
        mode,
        message: fixed.message,
        prompt_snapshots: ids,
        ...(fixed.options.file_references?.length || fixed.options.result_references?.length
          ? {
              file_references: fixed.options.file_references,
              result_references: fixed.options.result_references,
              context_order: fixed.options.context_order,
            }
          : {}),
      })
      if (result.ok) onReceipt(result.data as ChatDeliveryReceipt)
      return result
    }
    if (gateway.demo) {
      const result = await gateway.sendInterrupt(ws, body, mode === 'queue' ? 'Normal' : 'Urgent', {
        kind: 'chat',
        sessionId: session,
        turnId: turn,
      })
      if (result.ok && input.acceptLocal(source)) accepted()
      return result.ok
    }
    const ok = await inputs.submit(ws, entity, snapshot, run, {
      source,
      mode,
      isCurrent: current,
      accepted,
    })
    if (current() && !ok)
      sendError.value = input.error || 'Input outcome is unconfirmed. Keep its original identity.'
    return ok
  }
  function queue(text: string, _attachments: FileTreeNode[] = []) {
    return submitLive(text, 'queue')
  }
  function steer(text: string) {
    return submitLive(text, 'steer')
  }
  async function refreshLiveInputs() {
    const ws = workspace.active?.path,
      epoch = conversationEpoch,
      ids = queued.value.map((item) => item.id).slice(0, 64)
    if (!ws || gateway.demo || !gateway.connected.value || !ids.length) return
    const result = await gateway.resourceRequest('chatDelivery', ws, {
      operation: 'live_receipts',
      ids,
    })
    if (!result.ok || workspace.active?.path !== ws || epoch !== conversationEpoch) return
    const data = result.data as { items: Array<{ receipt: ChatDeliveryReceipt | null }> }
    for (const entry of data.items ?? []) {
      const receipt = entry.receipt
      if (
        !receipt ||
        receipt.schema_version !== 1 ||
        !receipt.live ||
        receipt.session !== sessionId.value ||
        !ids.includes(receipt.id)
      )
        continue
      const status = (
        {
          live_queued: 'queued',
          live_claimed: 'claimed',
          live_applied: 'applied',
          live_unconsumed: 'unconsumed',
          live_unknown: 'unknown',
        } as Partial<Record<ChatDeliveryReceipt['phase'], ChatMessage['inputStatus']>>
      )[receipt.phase]
      if (!status) continue
      const found = messages.value.find((message) => message.id === `q-${receipt.id}`)
      if (found) {
        found.inputStatus = status
        found.queued = status === 'queued' || status === 'claimed'
      }
      if (['applied', 'unconsumed', 'unknown'].includes(status))
        queued.value = queued.value.filter((item) => item.id !== receipt.id)
    }
  }
  useWorkspaceRefresh(refreshLiveInputs, [streaming, () => gateway.connected.value])

  /** Collapse the reasoning content without stopping the answer's timer. */
  function settleReasoningStream() {
    for (const message of messages.value) {
      message.reasoningPending = false
      if (message.reasoningStartedAt !== undefined) {
        message.reasoningElapsedMs =
          (message.reasoningElapsedMs ?? 0) + Math.max(0, Date.now() - message.reasoningStartedAt)
        message.reasoningStartedAt = undefined
      }
    }
  }

  /** The timer includes text generation and tools; stop only at completion. */
  function finishReasoning(turnElapsedMs?: number) {
    for (const message of messages.value) {
      if (message.turnStartedAt !== undefined) {
        message.turnElapsedMs = turnElapsedMs ?? Math.max(0, Date.now() - message.turnStartedAt)
        message.turnStartedAt = undefined
      }
      if (message.reasoningStartedAt !== undefined) {
        message.reasoningElapsedMs =
          (message.reasoningElapsedMs ?? 0) + Math.max(0, Date.now() - message.reasoningStartedAt)
        message.reasoningStartedAt = undefined
      }
      message.reasoningPending = false
    }
  }

  /** Insert a message, appending deltas to an open bubble of the same id. */
  function upsert(m: ChatMessage) {
    if (m.role === 'notice') followTask(m.detail?.continuation_task)
    lastEventAt.value = Date.now()
    if (m.role === 'tool' || (m.role === 'assistant' && !m.reasoningPending)) {
      settleReasoningStream()
    }
    if (m.role === 'tool') pendingTool.value = null
    const nextPhase = phaseOf(m)
    if (nextPhase) phase.value = nextPhase

    const idx = messages.value.findIndex((x) => x.id === m.id)
    if (idx < 0) {
      if (m.role === 'assistant') m.turnStartedAt = turnStartedAt.value
      if (m.reasoningPending) m.reasoningStartedAt = Date.now()
      messages.value.push(m)
      return
    }
    const existing = messages.value[idx]
    if (existing.pending && m.pending) {
      // Deltas append into the open bubble. Text and reasoning stream on
      // separate channels, so each accumulates into its own field.
      existing.content += m.content
      if (m.reasoning) existing.reasoning = (existing.reasoning ?? '') + m.reasoning
      if (m.reasoningPending) {
        existing.reasoningStartedAt ??= Date.now()
        existing.reasoningPending = true
      }
      return
    }
    // A final event replaces the bubble; reasoning streamed earlier survives
    // when the provider does not repeat it in the final payload. A tool call
    // keeps the progress it already reported when the result arrives empty.
    if (!m.reasoning && existing.reasoning) m.reasoning = existing.reasoning
    if (m.role === 'tool' && !m.content) m.content = existing.content
    m.reasoningPending = false
    // An assistant final-text event is not the end of the request: tools or
    // terminal bookkeeping may still follow. Only done/error/abort freezes it.
    m.turnStartedAt = existing.turnStartedAt
    m.turnElapsedMs ??= existing.turnElapsedMs
    m.reasoningElapsedMs ??=
      existing.reasoningStartedAt === undefined
        ? existing.reasoningElapsedMs
        : (existing.reasoningElapsedMs ?? 0) + Math.max(0, Date.now() - existing.reasoningStartedAt)
    messages.value[idx] = m
  }

  /** The phase a message implies, or `null` when it says nothing about it. */
  function phaseOf(m: ChatMessage): ChatPhase | null {
    if (m.role === 'tool') return m.pending ? 'tool' : 'waiting'
    if (m.role === 'assistant') {
      if (m.reasoningPending) return 'thinking'
      if (m.pending) return 'generating'
      return 'waiting'
    }
    return null
  }

  /** Start a fresh conversation without touching the previous thread.
   *
   * The prior thread stays on disk and reappears in the session list; deleting
   * one is an explicit action (`deleteThread`). */
  async function clear() {
    if (streaming.value || busy.value) await abort()
    conversationEpoch++
    following.value = null
    messages.value = []
    sessionId.value = ''
    sessionUnavailable.value = false
    sendError.value = ''
    approval.value = null
    approvalError.value = ''
    approvalSubmitting.value = false
    if (workspace.active?.path) views.edit(workspace.active.path, { session: '' })
    rotateDraft()
    lastUsage.value = null
    todos.value = []
    await refreshThreads()
  }

  /** Delete one persisted thread (explicit user action). */
  async function deleteThread(id: string) {
    const ws = workspace.active
    if (!ws) return
    const input = inputs.read(ws.path, id)
    if ((input.input.raw || input.input.selections.length) && !(await inputs.backup(ws.path, id))) {
      sendError.value =
        'The conversation was kept because its draft is not backed up. Keep or export the input before deleting it.'
      return false
    }
    if (streaming.value && sessionId.value === id) await abort()
    const result = await gateway.deleteChatSession(ws.path, id)
    if (!result.ok) {
      if (workspace.active?.path === ws.path)
        sendError.value = `Conversation deletion failed: ${result.error}`
      return false
    }
    if (workspace.active?.path !== ws.path) return true
    if (sessionId.value === id) {
      messages.value = []
      sessionId.value = ''
      sessionUnavailable.value = false
      sendError.value = ''
      views.edit(ws.path, { session: '' })
      lastUsage.value = null
    }
    await refreshThreads()
    return true
  }

  /** Explorer attachment follows the same saved-version choice as the composer. */
  function attach(filePath: string) {
    const ws = workspace.active?.path
    if (ws) requestedAttachment.value = { workspace: ws, entity: draftEntity.value, path: filePath }
  }
  function queueResult(choice: ResultChoice) {
    if (
      pendingResults.value.some(
        (item) => JSON.stringify(item.reference) === JSON.stringify(choice.reference),
      )
    )
      return
    pendingResults.value = [...pendingResults.value, JSON.parse(JSON.stringify(choice))]
  }
  function removeResult(index: number) {
    pendingResults.value = pendingResults.value.filter((_, i) => i !== index)
  }

  return {
    pendingResults,
    queueResult,
    removeResult,
    requestedAttachment,
    messages,
    rewind,
    rewinding,
    retry,
    retrying,
    streaming,
    busy,
    contextStats,
    permissionMode,
    approval,
    approvalSubmitting,
    approvalDecision,
    approvalError,
    phase,
    turnStartedAt,
    lastEventAt,
    pendingTool,
    queued,
    sessionId,
    sessionUnavailable,
    activeTurn,
    threads,
    lastUsage,
    todos,
    files,
    filesLoading,
    addons,
    addonsLoading,
    pendingFiles,
    draft,
    draftEntity,
    composerInput,
    sendError,
    send,
    queue,
    steer,
    respondApproval,
    reconcileApproval,
    stopApproval,
    abort,
    clear,
    deleteThread,
    restore,
    switchTo,
    refreshThreads,
    attach,
    loadFiles,
    loadAddons,
    toggleAddon,
    queueFile,
    removeQueuedFile,
  }
})
