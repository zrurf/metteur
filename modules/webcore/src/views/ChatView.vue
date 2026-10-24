<script setup lang="ts">
import {
  computed,
  nextTick,
  onBeforeUnmount,
  onMounted,
  onActivated,
  onDeactivated,
  ref,
  watch,
} from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { Bug, Compass, ListChecks } from '@lucide/vue'
import { gateway } from '@/core'
import { useExecutionStore } from '@/stores/execution'
import { useChatStore } from '@/stores/chat'
import { useConfigStore } from '@/stores/config'
import { useTabsStore } from '@/stores/tabs'
import { useFeedbackStore } from '@/stores/feedback'
import { fileRoute } from '@/lib/file-token'
import { onHighlightReady, warmHighlighter } from '@/lib/markdown'
import { clearRenderCache } from '@/lib/chat/stream-render'
import ChatHeader from '@/components/chat/ChatHeader.vue'
import WorkflowReceipts from '@/components/WorkflowReceipts.vue'
import FileReferenceDialog from '@/components/FileReferenceDialog.vue'
import ChatThread from '@/components/chat/ChatThread.vue'
import ChatComposer from '@/components/chat/ChatComposer.vue'
import ChatStatusBar from '@/components/chat/ChatStatusBar.vue'
import PlanBar from '@/components/chat/PlanBar.vue'
import type { ChatOptions, FileTreeNode, LlmModelConfig } from '@/core'
import { useWorkspaceRefresh } from '@/lib/workspace-refresh'
import { useWorkspaceStore } from '@/stores/workspace'
import { useComposerInputsStore } from '@/stores/composer-inputs'
import RecoveryCopies from '@/components/RecoveryCopies.vue'
import type { RecoveryRecord } from '@/core/recovery'
import { useForegroundStore } from '@/stores/foreground'
import { useDocumentsStore } from '@/stores/documents'
import { useSurfaceNavigation } from '@/lib/surface'
import { useCodingWorkflowsStore } from '@/stores/coding-workflows'
const codingWorkflows=useCodingWorkflowsStore()
function startTemplate(){if(!workspace.active)return;void codingWorkflows.openPlan(workspace.active.path,chat.sessionId?{session:chat.sessionId,entity:chat.draftEntity}:null,chat.draft);openSurface('tasks')}

/**
 * The ReAct chat surface.
 *
 * Orchestration only: the transcript, composer, header and status line own their
 * own behaviour, and the store owns the conversation. What lives here is the
 * wiring between them — model selection, the keyboard contract, and the
 * parameter dialog.
 */
const chat = useChatStore()
const execution = useExecutionStore()
const foreground = useForegroundStore()
const { openSurface } = useSurfaceNavigation()
const blueprintBusy = computed(
  () => foreground.current.state === 'blueprint' || execution.launching || execution.active,
)
const inputBlocked = computed(
  () =>
    blueprintBusy.value ||
    chat.sessionUnavailable ||
    ['unknown', 'blocked'].includes(foreground.current.state) ||
    (foreground.current.state === 'chat' &&
      (!chat.streaming || foreground.current.session !== chat.sessionId)),
)
const foregroundLabel = computed(() =>
  chat.sessionUnavailable
    ? 'Conversation unavailable · your drafts are kept'
    : execution.launching
      ? 'Blueprint is starting'
      : foreground.current.reason,
)
const foregroundRun = computed(
  () => foreground.current.run || (execution.active ? execution.runId : null),
)
async function openSupervision() {
  const ws = workspace.active?.path,
    run = foregroundRun.value
  if (!ws || !run) return
  await execution.reconcile(run)
  if (workspace.active?.path !== ws) return
  if (execution.current?.runId !== run || !execution.connected) {
    feedback.toast(
      'error',
      'Run Unavailable',
      execution.error || 'The selected run could not be loaded.',
    )
    return
  }
  openSurface('execution')
}
async function controlBlueprint(action: 'pause' | 'cancel') {
  const run = foregroundRun.value
  if (!run) return
  await execution[action](run)
  await foreground.refresh()
}
useWorkspaceRefresh(() => execution.reconcile())
const config = useConfigStore()
const tabs = useTabsStore()
const feedback = useFeedbackStore()
const router = useRouter()
const route = useRoute()
const workspace = useWorkspaceStore(),
  inputs = useComposerInputsStore()
watch(
  () => route.query.session,
  (id) => {
    if (typeof id === 'string' && id) void chat.switchTo(id)
  },
  { immediate: true },
)
function selectSession(id: string) {
  if (route.query.session !== id) void router.replace({ query: { ...route.query, session: id } })
  else void chat.switchTo(id)
}
async function newSession() {
  await chat.clear()
  if (route.query.session) {
    const query = { ...route.query }
    delete query.session
    void router.replace({ query })
  }
}
async function deleteSession(id: string) {
  const target = workspace.active?.path
  const confirmed = await feedback.confirm({
    header: 'Delete this conversation?',
    message:
      'Delete the saved conversation. Backed-up drafts remain available in Recovery Copies and will not recreate the conversation.',
    acceptLabel: 'Delete Conversation',
    rejectLabel: 'Keep Conversation',
    danger: true,
  })
  if (!confirmed || workspace.active?.path !== target) return
  if (await chat.deleteThread(id)) {
    if (route.query.session === id) {
      const query = { ...route.query }
      delete query.session
      void router.replace({ query })
    }
  } else feedback.toast('error', 'Conversation Kept', chat.sendError)
}
const recoveryOpen = ref(false)
const draftIdentity = computed(() => JSON.stringify([workspace.active?.path, chat.draftEntity]))
const draftStatus = computed(() => {
  const input = chat.composerInput
  if (!input) return ''
  if (input.state === 'submitting') return 'Submitting…'
  if (input.state === 'unknown') return 'Receipt Unconfirmed'
  if (!input.input.raw && !input.input.selections.length) return ''
  if (input.backup?.state === 'backed_up') return 'Backed Up on Host'
  if (input.backup?.state === 'backing_up') return 'Backing Up…'
  return gateway.demo ? 'Demo Draft · This Window' : 'Not Backed Up'
})
const draftError = computed(
  () => chat.sendError || chat.composerInput?.error || chat.composerInput?.backup?.error,
)
async function restoreInput(record: RecoveryRecord) {
  const ws = workspace.active?.path,
    entity = chat.draftEntity
  if (!ws) return
  if (
    (record.input.entity !== entity && (chat.sessionId || record.input.kind !== 'new_chat')) ||
    !['chat', 'new_chat'].includes(record.input.kind)
  ) {
    feedback.toast(
      'error',
      'Different Source',
      'Open the original conversation, or copy the text into a new draft.',
    )
    return
  }
  if (!(await inputs.restore(ws, entity, record))) {
    feedback.toast(
      'error',
      'Current Input Kept',
      'Back up or reconcile the current input before replacing it.',
    )
    return
  }
  if (workspace.active?.path === ws && chat.draftEntity === entity) recoveryOpen.value = false
}
async function reconcileInput() {
  const ws = workspace.active?.path,
    entity = chat.draftEntity,
    session = chat.sessionId
  if (!ws) return
  const accepted = await inputs.reconcile(ws, entity)
  if (accepted && workspace.active?.path === ws && chat.sessionId === session && session)
    await chat.switchTo(session)
}
async function discardInput() {
  const ws = workspace.active?.path,
    entity = chat.draftEntity,
    sequence = chat.composerInput?.input.edit_sequence
  if (!ws || sequence === undefined) return
  const confirmed = await feedback.confirm({
    header: 'Discard this draft?',
    message:
      'Discard this input copy. Saved conversations and other recovery copies stay available.',
    acceptLabel: 'Discard Draft',
    danger: true,
  })
  if (!confirmed || workspace.active?.path !== ws || chat.draftEntity !== entity) return
  if (!(await inputs.discard(ws, entity, sequence)))
    feedback.toast('error', 'Draft Kept', 'The input changed or its outcome needs reconciliation.')
}

const composer = ref<InstanceType<typeof ChatComposer>>()
let surfaceActive = true
onActivated(() => {
  surfaceActive = true
})
onDeactivated(() => {
  surfaceActive = false
})

/* Models ---------------------------------------------------------------- */

const modelTable = computed<Record<string, LlmModelConfig>>(
  () => (config.effective.llm?.models as Record<string, LlmModelConfig> | undefined) ?? {},
)
const modelKeys = computed(() => {
  const keys = Object.keys(modelTable.value)
  const fallback = config.effective.llm?.default_model
  if (fallback && keys.includes(fallback)) return [fallback, ...keys.filter((k) => k !== fallback)]
  return keys
})
const hasModel = computed(() => config.loaded && modelKeys.value.length > 0)

const model = ref('')
watch(
  [draftIdentity, () => chat.composerInput?.backup?.id],
  () => {
    const preferred = chat.composerInput?.input.selected_model
    if (preferred && modelKeys.value.includes(preferred)) model.value = preferred
  },
  { immediate: true },
)
watch([model, () => chat.draft], ([selected_model, raw]) => {
  if (workspace.active?.path && selected_model && raw)
    inputs.edit(workspace.active.path, chat.draftEntity, { selected_model })
})
watch(
  modelKeys,
  (keys) => {
    if (!keys.includes(model.value)) model.value = keys[0] ?? ''
  },
  { immediate: true },
)
const reasoning = ref<NonNullable<ChatOptions['reasoning_effort']>>('medium')
const temperature = ref(0.7)
const topP = ref(0.9)
const maxTokens = ref(4096)
const parametersOpen = ref(false)

const chatOptions = computed<ChatOptions>(() => ({
  // The selection can lag the configuration (a workspace that just opened, a
  // model removed from settings); the first configured key is what the
  // selector would show anyway.
  model: modelKeys.value.includes(model.value) ? model.value : modelKeys.value[0] || '',
  reasoning_effort: reasoning.value,
  temperature: temperature.value,
  top_p: topP.value,
  max_tokens: maxTokens.value,
  permission_mode: chat.permissionMode,
}))

/* Turn lifecycle -------------------------------------------------------- */

const running = computed(() => chat.streaming)

/** Sends a turn, or reports why it cannot. */
async function send(text: string, accepted?: () => void): Promise<boolean> {
  if (!hasModel.value) {
    feedback.toast('error', 'No model configured', 'Add a model under Settings → LLM & Models.')
    openSettings()
    return false
  }
  return chat.send(text, chatOptions.value, 'normal', accepted)
}

/** Queues a message into the running turn (Enter while the agent works). */
async function queue(text: string): Promise<boolean> {
  const ok = await chat.queue(text)
  if (!ok) feedback.toast('error', 'Input Kept', chat.sendError || 'The turn may have just ended.')
  return ok
}

async function steer(text: string): Promise<boolean> {
  const ok = await chat.steer(text)
  if (!ok) feedback.toast('error', 'Input Kept', chat.sendError || 'The turn may have just ended.')
  return ok
}

function stop(): void {
  void chat.abort()
}

function copy(text: string): void {
  void navigator.clipboard.writeText(text)
}

/** Puts a turn back in the composer for editing. */
async function edit(text: string): Promise<void> {
  const current = chat.composerInput
  if (!current) return
  await restoreInput({
    schema_version: 1,
    revision: 0,
    updated_ms: 0,
    discarded: false,
    input: { ...current.controller.capture(), raw: text, pending_submission: null, view: null },
  })
  composer.value?.focus()
}

/** Regenerates against the original pre-turn context, not the old answer. */
async function retry(assistantId: string): Promise<void> {
  if (inputBlocked.value) {
    feedback.toast('error', 'Chat Input Unavailable', foregroundLabel.value)
    return
  }
  if (!hasModel.value) {
    openSettings()
    return
  }
  if (chat.busy || chat.rewinding || chat.retrying) return
  const confirmed = await feedback.confirm({
    header: 'Retry this turn?',
    message:
      'Restore files and context to before the original request, discard this answer and all later turns, then generate a new answer automatically. Your input draft will be kept.',
    acceptLabel: 'Retry',
    danger: true,
  })
  if (!confirmed) return
  const error = await chat.retry(assistantId, chatOptions.value)
  if (error !== null) feedback.toast('error', 'Retry failed', error)
}

function openFile(path: string | undefined): void {
  if (!path) return
  tabs.openFile(path)
  void router.push(fileRoute(path))
}

function openSettings(): void {
  void router.push('/settings')
}

/**
 * Rolls the workspace back to the checkpoint a turn started from.
 *
 * Restoring rewrites files, so it is confirmed first and reported afterwards;
 * both the visible conversation and the model's context return to that point.
 */
async function restore(snapshotId: string): Promise<void> {
  const confirmed = await feedback.confirm({
    header: 'Restore files and conversation?',
    message:
      'Files will return to the state before this turn. This message and all later messages, tool results and model context will be removed. Any running reply will be stopped.',
    acceptLabel: 'Restore',
    danger: true,
  })
  if (!confirmed) return
  const error = await chat.rewind(snapshotId)
  if (error === null)
    feedback.toast(
      'success',
      'Conversation restored',
      'Files and model context are back to the state before that turn.',
    )
  else feedback.toast('error', 'Restore failed', error)
}

const referenceBusy = ref(false),
  fileReferenceTarget = ref<{ workspace: string; entity: string; file: FileTreeNode } | null>(null)
let referenceEpoch = 0
watch([() => workspace.active?.path, () => chat.draftEntity], () => {
  referenceEpoch++
  referenceBusy.value = false
  fileReferenceTarget.value = null
})
function finishReference() {
  fileReferenceTarget.value = null
  referenceBusy.value = false
}
function selectReferencedFile(file: FileTreeNode) {
  const target = fileReferenceTarget.value
  if (target && workspace.active?.path === target.workspace && chat.draftEntity === target.entity)
    chat.queueFile(file)
  finishReference()
}
function reviewReferencedFile() {
  const path = fileReferenceTarget.value?.file.path
  finishReference()
  if (path) openFile(path)
}
async function attach(file: FileTreeNode) {
  const ws = workspace.active?.path,
    entity = chat.draftEntity
  if (!ws || referenceBusy.value) return
  const ticket = ++referenceEpoch
  referenceBusy.value = true
  const doc = useDocumentsStore().peek(ws, file.path)
  if (doc && useDocumentsStore().dirty(doc)) {
    fileReferenceTarget.value = { workspace: ws, entity, file }
    return
  }
  try {
    const result = await gateway.readFile(ws, file.path, true)
    if (ticket !== referenceEpoch || workspace.active?.path !== ws || chat.draftEntity !== entity)
      return
    if (!result.ok || !result.data.reference) {
      feedback.toast(
        'error',
        'File Reference Unavailable',
        result.ok ? 'The host did not confirm a saved version.' : result.error,
      )
      return
    }
    chat.queueFile({ ...file, reference: result.data.reference })
  } catch (cause) {
    if (ticket === referenceEpoch)
      feedback.toast('error', 'File Reference Unavailable', String(cause))
  } finally {
    if (ticket === referenceEpoch) referenceBusy.value = false
  }
}
watch(
  () => chat.requestedAttachment,
  (target) => {
    if (!target) return
    chat.requestedAttachment = null
    if (target.workspace !== workspace.active?.path || target.entity !== chat.draftEntity) return
    void attach({
      kind: 'file',
      path: target.path,
      name: target.path.split('/').at(-1) || target.path,
    })
  },
  { immediate: true },
)

/**
 * Hands the current plan to the turn.
 *
 * The plan is already in the agent's context when it wrote it; attaching it as
 * a message is what makes it explicit for a turn that follows a compaction.
 */
function attachPlan(): void {
  const plan = chat.todos
  if (!plan.length) return
  const lines = plan.map((todo, i) => `${i + 1}. [${todo.status}] ${todo.content}`)
  chat.draft = `${chat.draft ? chat.draft + '\n\n' : ''}Current plan:
${lines.join('\n')}
\n`
  composer.value?.focus()
}

/* Keyboard ------------------------------------------------------------- */

/**
 * Global keys for the surface: `Esc` interrupts, `Ctrl+O` folds every detail
 * back in, and `Ctrl+K` focuses the composer from anywhere in the page.
 */
function onKeydown(event: KeyboardEvent): void {
  if (!surfaceActive) return
  if (event.defaultPrevented || event.isComposing || event.keyCode === 229 || event.repeat) return
  const mod = event.ctrlKey || event.metaKey
  if (event.key === 'Escape' && running.value) {
    // The draft is otherwise lost on interrupt, which is the opposite of what
    // "stop" should cost the user.
    event.preventDefault()
    stop()
    return
  }
  if (!mod) return
  if (event.key.toLowerCase() === 'k') {
    event.preventDefault()
    composer.value?.focus()
    return
  }
  if (event.key.toLowerCase() === 'o') {
    event.preventDefault()
    // Collapsing everything is a re-render, not a state change: tool rows and
    // reasoning blocks keep their own state, so a remount resets them.
    rerender.value += 1
  }
}

/** Bumped to remount the transcript (folds every expanded detail). */
const rerender = ref(0)

onMounted(() => {
  void chat.loadFiles()
  void chat.loadAddons()
  window.addEventListener('keydown', onKeydown)
  warmHighlighter()
  onHighlightReady(() => {
    clearRenderCache()
    rerender.value += 1
  })
  void nextTick(() => composer.value?.focus())
})
onBeforeUnmount(() => window.removeEventListener('keydown', onKeydown))

const starterPrompts = [
  {
    label: 'Explain this workspace',
    description: 'Find your way around the code.',
    icon: Compass,
    text: 'Explain how this workspace is organised and what the main entry points are.',
  },
  {
    label: 'Find and fix a bug',
    description: 'Turn a problem into a solution.',
    icon: Bug,
    text: 'Find the most likely bug in the modified files and fix it.',
  },
  {
    label: 'Plan a change',
    description: 'Make the next step a clear one.',
    icon: ListChecks,
    text: 'Plan how to add a new feature end to end, then implement the first step.',
  },
]
</script>

<template>
  <div class="chat-surface flex h-full min-w-0 flex-col bg-background">
    <ChatHeader
      :threads="chat.threads"
      :session-id="chat.sessionId"
      :active-title="
        chat.sessionUnavailable
          ? 'Unavailable conversation'
          : chat.threads.find((t) => t.sessionId === chat.sessionId)?.title || 'New chat'
      "
      :addons="chat.addons"
      :addons-loading="chat.addonsLoading"
      :busy="chat.busy || chat.retrying || chat.rewinding"
      :on-select-session="selectSession"
      :on-delete-session="(id) => void deleteSession(id)"
      :on-new-session="() => void newSession()"
      :on-toggle-addon="(id, enabled) => void chat.toggleAddon(id, enabled)"
    />

    <div
      v-if="inputBlocked"
      class="flex flex-wrap items-center gap-3 border-b border-border px-6 py-3 text-[14px]"
      data-chat-availability
    >
      <span role="status" class="mr-auto">{{
        foregroundLabel || 'Workspace availability is unconfirmed'
      }}</span>
      <template v-if="blueprintBusy">
        <button class="btn" :disabled="!foregroundRun" @click="openSupervision">
          Open Supervision
        </button>
        <button
          class="btn"
          :disabled="!foregroundRun || execution.controlBusy"
          @click="controlBlueprint('pause')"
        >
          Pause
        </button>
        <button
          class="btn"
          :disabled="!foregroundRun || execution.stopping"
          @click="controlBlueprint('cancel')"
        >
          Stop
        </button>
      </template>
      <button v-else class="btn" @click="foreground.refresh">Check State</button>
      <details v-if="foreground.error || execution.error">
        <summary>Details</summary>
        <p class="max-w-lg break-words text-[13px]">{{ foreground.error || execution.error }}</p>
      </details>
    </div>

    <WorkflowReceipts />
    <div class="px-3 py-1 text-[13px]"><button class="btn" :disabled="!workspace.active" @click="startTemplate">Start from template</button></div>
    <ChatThread
      :key="rerender"
      :messages="chat.messages"
      :on-open-file="openFile"
      :on-copy="copy"
      :on-edit="edit"
      :on-retry="(id) => void retry(id)"
      :on-restore="(id) => void restore(id)"
    >
      <template #empty>
        <div class="chat-welcome">
          <h2>What Should We Work On?</h2>
          <p class="chat-welcome-description">
            Ask about your code, fix a bug, or build something new.
          </p>
          <div v-if="hasModel" class="chat-starters">
            <button
              v-for="starter in starterPrompts"
              :key="starter.label"
              class="chat-starter"
              type="button"
              @click="edit(starter.text)"
            >
              <span class="chat-starter-top" aria-hidden="true">
                <component :is="starter.icon" class="h-4.5 w-4.5" />
              </span>
              <span class="chat-starter-title">{{ starter.label }}</span>
            </button>
          </div>
        </div>
      </template>
    </ChatThread>

    <div class="chat-composer-dock shrink-0">
      <div class="chat-column chat-column-composer">
        <ChatStatusBar
          :waiting-for-approval="!!chat.approval"
          :phase="chat.phase"
          :last-event-at="chat.lastEventAt"
          :usage="chat.lastUsage"
          :pending-tool="chat.pendingTool"
        />
        <PlanBar :todos="chat.todos" />
        <div
          v-if="chat.pendingResults.length"
          class="flex flex-wrap gap-2 px-4 py-2 text-[13px]"
          aria-label="Selected saved results"
        >
          <span
            v-for="(result, index) in chat.pendingResults"
            :key="JSON.stringify(result.reference)"
            class="rounded border border-divider px-2 py-1"
          >
            {{ result.label }} · Saved result
            <button :aria-label="`Remove result ${result.label}`" @click="chat.removeResult(index)">
              ×
            </button>
          </span>
        </div>
        <ChatComposer
          ref="composer"
          :draft-value="chat.draft"
          :draft-identity="draftIdentity"
          :on-draft-change="(text) => (chat.draft = text)"
          :send-blocked="
            inputBlocked ||
            referenceBusy ||
            (chat.busy && !chat.streaming) ||
            !chat.composerInput?.canSubmit
          "
          :files="chat.files"
          :pending-files="chat.pendingFiles"
          :addons="chat.addons"
          :todos="chat.todos"
          :models="modelTable"
          :model-keys="modelKeys"
          :model="model"
          :effort="reasoning"
          :context-stats="chat.contextStats"
          :usage="chat.lastUsage"
          :permission-mode="chat.permissionMode"
          :running="running"
          :ready="hasModel && !chat.rewinding && !chat.retrying"
          :on-send="send"
          :on-queue="queue"
          :on-steer="steer"
          :on-stop="stop"
          :on-remove-file="(path) => chat.removeQueuedFile(path)"
          :on-attach="attach"
          :on-attach-plan="attachPlan"
          :on-open-plugins="() => openSettings()"
          :on-select-model="(key) => (model = key)"
          :on-select-effort="(key) => (reasoning = key)"
          :on-select-permission="(mode) => (chat.permissionMode = mode)"
          :on-open-parameters="() => (parametersOpen = true)"
          :on-open-settings="openSettings"
        />
        <div
          class="flex items-center gap-3 py-2 text-[13px] text-muted-foreground"
          data-chat-draft-status
        >
          <span v-if="draftStatus" role="status">{{ draftStatus }}</span>
          <button class="cursor-pointer hover:text-foreground" @click="recoveryOpen = true">
            Recovery Copies
          </button>
          <button
            v-if="chat.composerInput?.state === 'unknown'"
            class="btn"
            @click="reconcileInput"
          >
            Check Receipt
          </button>
          <details v-if="chat.draft || draftError" class="relative">
            <summary class="cursor-pointer">
              Draft Options{{ draftError ? ' · Needs Review' : '' }}
            </summary>
            <div
              class="absolute bottom-7 right-0 z-20 w-72 rounded border border-border bg-popover p-3 shadow-lg"
            >
              <p v-if="draftError" role="alert" class="mb-3 break-words">{{ draftError }}</p>
              <button
                v-if="chat.draft"
                class="btn"
                @click="inputs.backup(workspace.active?.path ?? '', chat.draftEntity)"
              >
                Back Up
              </button>
              <button
                v-if="chat.draft"
                class="btn"
                :disabled="!chat.composerInput?.canSubmit"
                @click="discardInput"
              >
                Discard Draft
              </button>
            </div>
          </details>
        </div>
      </div>
    </div>
    <RecoveryCopies
      v-if="recoveryOpen"
      :key="draftIdentity"
      :workspace="workspace.active?.path ?? ''"
      scope="project"
      :entity="chat.draftEntity"
      :local-copies="inputs.localCopies(workspace.active?.path ?? '')"
      @close="recoveryOpen = false"
      @restore="restoreInput"
    />

    <!-- Generation parameters -->
    <div
      v-if="parametersOpen"
      class="fixed inset-0 z-50 grid place-items-center"
      style="background: var(--overlay)"
      @mousedown.self="parametersOpen = false"
    >
      <div class="chat-menu w-80 p-3!">
        <p class="mb-2 text-[13px] font-semibold">Generation parameters</p>
        <div class="space-y-2">
          <label class="flex items-center justify-between text-[13px] text-muted-foreground">
            Reasoning
            <select v-model="reasoning" class="input h-8 w-28! px-1.5! text-[13px]!">
              <option value="none">none</option>
              <option value="low">low</option>
              <option value="medium">medium</option>
              <option value="high">high</option>
            </select>
          </label>
          <label class="flex items-center justify-between text-[13px] text-muted-foreground">
            Temperature
            <input
              v-model.number="temperature"
              type="number"
              step="0.1"
              min="0"
              max="2"
              class="input h-8 w-28! px-1.5! text-[13px]!"
            />
          </label>
          <label class="flex items-center justify-between text-[13px] text-muted-foreground">
            Top P
            <input
              v-model.number="topP"
              type="number"
              step="0.05"
              min="0"
              max="1"
              class="input h-8 w-28! px-1.5! text-[13px]!"
            />
          </label>
          <label class="flex items-center justify-between text-[13px] text-muted-foreground">
            Max tokens
            <input
              v-model.number="maxTokens"
              type="number"
              step="256"
              min="256"
              class="input h-8 w-28! px-1.5! text-[13px]!"
            />
          </label>
        </div>
        <div class="mt-3 flex justify-end">
          <button
            class="btn btn-primary h-7! px-3! text-[13px]"
            type="button"
            @click="parametersOpen = false"
          >
            Done
          </button>
        </div>
      </div>
    </div>
    <FileReferenceDialog
      v-if="fileReferenceTarget"
      :workspace="fileReferenceTarget.workspace"
      :file="fileReferenceTarget.file"
      @close="finishReference"
      @selected="selectReferencedFile"
      @review="reviewReferencedFile"
    />
  </div>
</template>
