import { defineStore } from 'pinia'
import { ref, watch } from 'vue'
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
import type { ChatPhase } from '@/lib/chat/liveness'
import { useWorkspaceStore } from './workspace'

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
  const messages = ref<ChatMessage[]>([])
  const streaming = ref(false)
  const busy = ref(false)
  /** id of the persisted session, passed back on every turn. */
  const sessionId = ref('')
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
  const approval = ref<{ requestId: string; detail: string } | null>(null)
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

  // Switching workspaces resets the surface and restores the new session.
  // Immediate so a store created after the workspace was already opened still
  // loads the persisted conversation (otherwise restore would never run).
  watch(
    () => workspace.active?.path,
    async () => {
      messages.value = []
      streaming.value = false
      busy.value = false
      pendingFiles.value = []
      sessionId.value = ''
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
    { immediate: true },
  )

  /** Reload the persisted conversation of the active workspace, if any. */
  async function restore(): Promise<void> {
    const ws = workspace.active
    if (!ws) return
    const target = ws.path
    const sessions = await gateway.listChatSessions(target)
    if (!sessions.ok || sessions.data.length === 0) {
      threads.value = []
      return
    }
    threads.value = sessions.data
    const latest = sessions.data[0]
    await openThread(target, latest.sessionId, latest.title)
  }

  /** Reload the thread list without touching the open conversation. */
  async function refreshThreads(): Promise<void> {
    const ws = workspace.active
    if (!ws) return
    const sessions = await gateway.listChatSessions(ws.path)
    if (sessions.ok) threads.value = sessions.data
  }

  /** Switch the surface to an existing thread. */
  async function switchTo(threadId: string): Promise<void> {
    const ws = workspace.active
    if (!ws) return
    await openThread(ws.path, threadId, '')
  }

  /** Load one thread into the surface (empty history on failure). */
  async function openThread(wsPath: string, id: string, title: string): Promise<void> {
    if (streaming.value) await abort()
    const snap = await gateway.getChatSession(wsPath, id)
    // The user may have switched workspaces while loading; discard stale data.
    if (workspace.active?.path !== wsPath) return
    sessionId.value = id
    if (snap.ok) {
      // The transcript is the displayed conversation; the history is the
      // model's view and has no tool calls. Fall back only for sessions that
      // predate transcripts.
      messages.value = snap.data.transcript.length ? snap.data.transcript : snap.data.history
      sessionId.value = snap.data.sessionId
      todos.value = snap.data.todos ?? []
    } else {
      messages.value = []
      todos.value = []
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
      if (r.ok) addons.value = r.data.filter((a) => a.scope === 'global' || a.scopeRoot === workspace.active?.path)
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
  async function send(content: string, options?: ChatOptions, mode: 'normal' | 'retry' = 'normal'): Promise<boolean> {
    const ws = workspace.active
    const text = content.trim()
    if (!ws || !text || streaming.value || busy.value || rewinding.value || (retrying.value && mode !== 'retry')) return false
    // File references travel with the turn: the daemon learns about the
    // attached files from the message text, and the transcript shows the same
    // lines the model receives.
    const refs = mode === 'retry' ? [] : pendingFiles.value.map((f) => `@file ${f.path}`)
    const body = refs.length ? `${[...refs, text].join('\n')}` : text
    const userMessageId = `u-${Date.now()}`
    messages.value.push({
      id: userMessageId,
      role: 'user',
      content: body,
      createdAt: Date.now(),
    })
    if (mode !== 'retry') pendingFiles.value = []
    // A "new chat" turn creates a fresh thread; sending into an existing one
    // keeps its id (non-empty below).
    streamAbort = new AbortController()
    streaming.value = true
    busy.value = true
    lastUsage.value = null
    pendingTool.value = null
    turnStartedAt.value = Date.now()
    lastEventAt.value = Date.now()
    phase.value = 'thinking'
    let resolveFinished!: () => void
    const finished = new Promise<void>((resolve) => { resolveFinished = resolve })
    pendingSend = finished
    try {
      // The new turn is sent as the message itself; including it in the
      // history as well made the daemon seed a fresh session with it twice.
      const history = messages.value.slice(0, -1)
      const r = await gateway.sendChat(
        ws.path,
        body,
        history,
        (m) => upsert(m),
        options,
        (id, checkpointId) => {
          sessionId.value = id
          const user = messages.value.find((message) => message.id === userMessageId)
          if (user) user.checkpoint = checkpointId
        },
        sessionId.value,
        (usage) => {
          lastUsage.value = usage
          finishReasoning(usage.turnElapsedMs)
        },
        (next) => {
          // Live plan updates stream in while the turn runs.
          todos.value = next
        },
        streamAbort.signal,
        (progress) => {
          // Arguments of a large call stream for seconds before the tool row
          // exists; the status line carries that time.
          pendingTool.value = progress
          settleReasoningStream()
          lastEventAt.value = Date.now()
          phase.value = 'generating'
        },
        (stats) => {
          contextStats.value = stats
        },
        (request) => {
          // The turn is blocked until this is answered, so it is surfaced
          // immediately rather than queued behind the transcript.
          approval.value = request
        },
      )
      finishReasoning()
      await refreshThreads()
      if (!r.ok) {
        // Failures used to vanish here, which made a broken turn look like a
        // silent stop; the reason is part of the transcript.
        messages.value.push({
          id: `e-${Date.now()}`,
          role: 'error',
          content: r.error || 'The turn failed.',
          createdAt: Date.now(),
        })
      }
      return r.ok
    } finally {
      streamAbort = null
      phase.value = 'idle'
      finishReasoning()
      if (streaming.value) {
        for (const m of messages.value) {
          m.pending = false
          m.reasoningPending = false
        }
        streaming.value = false
      }
      busy.value = false
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
    if (retrying.value || rewinding.value || busy.value) return 'Wait for the current operation to finish before retrying.'
    const targetWorkspace = workspace.active?.path
    const targetSession = sessionId.value
    const index = messages.value.findIndex((message) => message.id === assistantId && message.role === 'assistant')
    if (index < 0) return 'The answer is no longer in this conversation.'
    let original: ChatMessage | undefined
    for (let i = index - 1; i >= 0; i--) {
      if (messages.value[i].role === 'user') { original = messages.value[i]; break }
    }
    if (!original?.checkpoint) return 'This turn has no paired context checkpoint. It cannot be retried safely without retaining the old answer.'
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
      return await send(prompt, { ...options, retry: true }, 'retry')
        ? null
        : 'The old turn was restored, but the new reply failed.'
    } finally {
      retrying.value = false
    }
  }

  /** Answers the pending approval and clears it. */
  async function respondApproval(allow: boolean): Promise<void> {
    const ws = workspace.active
    const pending = approval.value
    if (!ws || !pending) return
    approval.value = null
    await gateway.respondApproval(ws.path, pending.requestId, allow)
  }

  async function abort() {
    const ws = workspace.active
    if (!ws) return
    // Stop reading locally first: the daemon's terminal event would otherwise
    // arrive after the user asked to stop and repaint the turn.
    streamAbort?.abort()
    finishReasoning()
    streamAbort = null
    await gateway.abortChat(ws.path)
    for (const m of messages.value) m.pending = false
    streaming.value = false
    busy.value = false
    phase.value = 'idle'
    queued.value = []
  }

  /**
   * Queues a message for the running turn.
   *
   * The daemon injects it at the next model call, so the transcript can show it
   * immediately with a "queued" marker; the marker is cleared once the turn
   * reports more activity.
   */
  async function queue(text: string, attachments: FileTreeNode[] = []): Promise<boolean> {
    const ws = workspace.active
    const body = text.trim()
    if (!ws || !body || !streaming.value) return false
    const refs = attachments.map((f) => `@file ${f.path}`)
    const content = refs.length ? `${[...refs, body].join('\n')}` : body
    const id = `q-${Date.now()}`
    const r = await gateway.sendInterrupt(ws.path, content, 'Normal')
    if (!r.ok) return false
    queued.value = [...queued.value, { id, text: content, files: attachments, createdAt: Date.now() }]
    messages.value.push({
      id,
      role: 'user',
      content,
      createdAt: Date.now(),
      queued: true,
    })
    return true
  }

  /** Sends a queued-or-new message into the running turn immediately. */
  async function steer(text: string): Promise<boolean> {
    const ws = workspace.active
    const body = text.trim()
    if (!ws || !body || !streaming.value) return false
    const r = await gateway.sendInterrupt(ws.path, body, 'Urgent')
    if (!r.ok) return false
    messages.value.push({
      id: `q-${Date.now()}`,
      role: 'user',
      content: body,
      createdAt: Date.now(),
      queued: true,
    })
    return true
  }

  /** Collapse the reasoning content without stopping the answer's timer. */
  function settleReasoningStream() {
    for (const message of messages.value) message.reasoningPending = false
  }

  /** The timer includes text generation and tools; stop only at completion. */
  function finishReasoning(turnElapsedMs?: number) {
    for (const message of messages.value) {
      if (message.turnStartedAt !== undefined) {
        message.turnElapsedMs = turnElapsedMs ?? Math.max(0, Date.now() - message.turnStartedAt)
        message.turnStartedAt = undefined
      }
      if (message.reasoningStartedAt !== undefined) {
        message.reasoningElapsedMs = (message.reasoningElapsedMs ?? 0)
          + Math.max(0, Date.now() - message.reasoningStartedAt)
        message.reasoningStartedAt = undefined
      }
      message.reasoningPending = false
    }
  }

  /** Insert a message, appending deltas to an open bubble of the same id. */
  function upsert(m: ChatMessage) {
    lastEventAt.value = Date.now()
    if (m.role === 'tool' || (m.role === 'assistant' && !m.reasoningPending)) {
      settleReasoningStream()
    }
    if (m.role === 'tool') pendingTool.value = null
    const nextPhase = phaseOf(m)
    if (nextPhase) phase.value = nextPhase
    // An injected queued message has reached the model by now.
    if (m.role !== 'user' && queued.value.length) {
      queued.value = []
      for (const message of messages.value) message.queued = false
    }
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
    m.reasoningElapsedMs ??= existing.reasoningStartedAt === undefined
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
    if (streaming.value) await abort()
    messages.value = []
    sessionId.value = ''
    lastUsage.value = null
    todos.value = []
    await refreshThreads()
  }

  /** Delete one persisted thread (explicit user action). */
  async function deleteThread(id: string) {
    const ws = workspace.active
    if (!ws) return
    if (streaming.value && sessionId.value === id) await abort()
    await gateway.deleteChatSession(ws.path, id)
    if (sessionId.value === id) {
      messages.value = []
      sessionId.value = ''
      lastUsage.value = null
    }
    await refreshThreads()
  }

  /** Attach a file reference to the conversation without sending a turn
   *  (explorer "Add to Conversation"). The agent sees it on the next reply. */
  function attach(filePath: string) {
    messages.value.push({
      id: `ref-${Date.now()}`,
      role: 'user',
      content: `@file ${filePath}`,
      createdAt: Date.now(),
    })
  }

  return {
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
    phase,
    turnStartedAt,
    lastEventAt,
    pendingTool,
    queued,
    sessionId,
    threads,
    lastUsage,
    todos,
    files,
    filesLoading,
    addons,
    addonsLoading,
    pendingFiles,
    send,
    queue,
    steer,
    respondApproval,
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
