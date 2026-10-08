<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import { Bug, Compass, ListChecks } from '@lucide/vue'
import ConciergePanel from '@/components/execution/ConciergePanel.vue'
import { gateway } from '@/core'
import { useExecutionStore } from '@/stores/execution'
import { useChatStore } from '@/stores/chat'
import { useWorkspaceStore } from '@/stores/workspace'
import { useConfigStore } from '@/stores/config'
import { useTabsStore } from '@/stores/tabs'
import { useFeedbackStore } from '@/stores/feedback'
import { fileRoute } from '@/lib/file-token'
import { onHighlightReady, warmHighlighter } from '@/lib/markdown'
import { clearRenderCache } from '@/lib/chat/stream-render'
import ChatHeader from '@/components/chat/ChatHeader.vue'
import ChatThread from '@/components/chat/ChatThread.vue'
import ChatComposer from '@/components/chat/ChatComposer.vue'
import ChatStatusBar from '@/components/chat/ChatStatusBar.vue'
import PlanBar from '@/components/chat/PlanBar.vue'
import type { ChatOptions, FileTreeNode, LlmModelConfig } from '@/core'

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
let runtimeTimer: ReturnType<typeof setTimeout> | undefined, runtimeDisposed = false
async function pollRuntime() {
  await execution.reconcile()
  if (!runtimeDisposed) runtimeTimer = setTimeout(pollRuntime, 1000)
}
onMounted(() => { void pollRuntime() })
onBeforeUnmount(() => { runtimeDisposed = true; clearTimeout(runtimeTimer) })
const workspace = useWorkspaceStore()
const config = useConfigStore()
const tabs = useTabsStore()
const feedback = useFeedbackStore()
const router = useRouter()

const composer = ref<InstanceType<typeof ChatComposer>>()

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
const hasModel = computed(() => modelKeys.value.length > 0)

const model = ref('')
const reasoning = ref<ChatOptions['reasoning_effort']>('medium')
const temperature = ref(0.7)
const topP = ref(0.9)
const maxTokens = ref(4096)
const parametersOpen = ref(false)

const chatOptions = computed<ChatOptions>(() => ({
  // The selection can lag the configuration (a workspace that just opened, a
  // model removed from settings); the first configured key is what the
  // selector would show anyway.
  model: model.value || modelKeys.value[0] || '',
  reasoning_effort: reasoning.value,
  temperature: temperature.value,
  top_p: topP.value,
  max_tokens: maxTokens.value,
  permission_mode: chat.permissionMode,
}))

/* Turn lifecycle -------------------------------------------------------- */

const running = computed(() => chat.streaming)

/** Sends a turn, or reports why it cannot. */
async function send(text: string): Promise<void> {
  if (!hasModel.value) {
    feedback.toast('error', 'No model configured', 'Add a model under Settings → LLM & Models.')
    openSettings()
    return
  }
  await chat.send(text, chatOptions.value)
}

/** Queues a message into the running turn (Enter while the agent works). */
async function queue(text: string): Promise<void> {
  const ok = await chat.queue(text, chat.pendingFiles ?? [])
  if (!ok) feedback.toast('error', 'Could not queue the message', 'The turn may have just ended.')
  if (chat.pendingFiles.length) for (const file of [...chat.pendingFiles]) chat.removeQueuedFile(file.path)
}

/** Sends a message into the running turn before the next model call. */
async function steer(text: string): Promise<void> {
  const ok = await chat.steer(text)
  if (!ok) feedback.toast('error', 'Could not send the message', 'The turn may have just ended.')
}

function stop(): void {
  void chat.abort()
}

function copy(text: string): void {
  void navigator.clipboard.writeText(text)
}

/** Puts a turn back in the composer for editing. */
function edit(text: string): void {
  composer.value?.setDraft(text)
}

/** Regenerates against the original pre-turn context, not the old answer. */
async function retry(assistantId: string): Promise<void> {
  if (!hasModel.value) { openSettings(); return }
  if (chat.busy || chat.rewinding || chat.retrying) return
  const confirmed = await feedback.confirm({
    header: 'Retry this turn?',
    message: 'Restore files and context to before the original request, discard this answer and all later turns, then generate a new answer automatically. Your input draft will be kept.',
    acceptLabel: 'Retry', danger: true,
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
  if (error === null) feedback.toast('success', 'Conversation restored', 'Files and model context are back to the state before that turn.')
  else feedback.toast('error', 'Restore failed', error)
}

function attach(file: FileTreeNode): void {
  chat.queueFile(file)
}

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
  composer.value?.setDraft(
    `Current plan:
${lines.join('\n')}
\n
`,
  )
}

/* Keyboard ------------------------------------------------------------- */

/**
 * Global keys for the surface: `Esc` interrupts, `Ctrl+O` folds every detail
 * back in, and `Ctrl+K` focuses the composer from anywhere in the page.
 */
function onKeydown(event: KeyboardEvent): void {
  if (execution.active) return
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
  { label: 'Explain this workspace', description: 'Find your way around the code.', icon: Compass, text: 'Explain how this workspace is organised and what the main entry points are.' },
  { label: 'Find and fix a bug', description: 'Turn a problem into a solution.', icon: Bug, text: 'Find the most likely bug in the modified files and fix it.' },
  { label: 'Plan a change', description: 'Make the next step a clear one.', icon: ListChecks, text: 'Plan how to add a new feature end to end, then implement the first step.' },
]
</script>

<template>
  <div v-if="execution.active" class="chat-surface flex h-full min-w-0 flex-col bg-background"><ConciergePanel :run-id="execution.runId" :connected="execution.connected && gateway.connected.value" /></div>
  <div v-else class="chat-surface flex h-full min-w-0 flex-col bg-background">
    <ChatHeader
      :threads="chat.threads"
      :session-id="chat.sessionId"
      :active-title="chat.threads.find((t) => t.sessionId === chat.sessionId)?.title || 'New chat'"
      :addons="chat.addons"
      :addons-loading="chat.addonsLoading"
      :busy="chat.busy || chat.retrying || chat.rewinding"
      :on-select-session="(id) => void chat.switchTo(id)"
      :on-delete-session="(id) => void chat.deleteThread(id)"
      :on-new-session="() => void chat.clear()"
      :on-toggle-addon="(id, enabled) => void chat.toggleAddon(id, enabled)"
    />

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
          <h2>What should we work on?</h2>
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
          :phase="chat.phase"
          :last-event-at="chat.lastEventAt"
          :usage="chat.lastUsage"
          :pending-tool="chat.pendingTool"
        />
        <PlanBar :todos="chat.todos" />
        <ChatComposer
          ref="composer"
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
          :on-send="(text) => void send(text)"
          :on-queue="(text) => void queue(text)"
          :on-steer="(text) => void steer(text)"
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
        <p class="chat-composer-hint">
          <span><kbd>@</kbd> files <span aria-hidden="true">·</span> <kbd>/</kbd> templates</span>
          <span>Enter to send <span aria-hidden="true">·</span> Shift+Enter for a new line</span>
        </p>
      </div>
    </div>

    <!-- Generation parameters -->
    <div v-if="parametersOpen" class="fixed inset-0 z-50 grid place-items-center" style="background: var(--overlay)" @mousedown.self="parametersOpen = false">
      <div class="chat-menu w-80 p-3!">
        <p class="mb-2 text-[13px] font-semibold">Generation parameters</p>
        <div class="space-y-2">
          <label class="flex items-center justify-between text-[12px] text-muted-foreground">
            Reasoning
            <select v-model="reasoning" class="input h-6 w-28! px-1.5! text-[11px]!">
              <option value="none">none</option>
              <option value="low">low</option>
              <option value="medium">medium</option>
              <option value="high">high</option>
            </select>
          </label>
          <label class="flex items-center justify-between text-[12px] text-muted-foreground">
            Temperature
            <input v-model.number="temperature" type="number" step="0.1" min="0" max="2" class="input h-6 w-28! px-1.5! text-[11px]!" />
          </label>
          <label class="flex items-center justify-between text-[12px] text-muted-foreground">
            Top P
            <input v-model.number="topP" type="number" step="0.05" min="0" max="1" class="input h-6 w-28! px-1.5! text-[11px]!" />
          </label>
          <label class="flex items-center justify-between text-[12px] text-muted-foreground">
            Max tokens
            <input v-model.number="maxTokens" type="number" step="256" min="256" class="input h-6 w-28! px-1.5! text-[11px]!" />
          </label>
        </div>
        <div class="mt-3 flex justify-end">
          <button class="btn btn-primary h-7! px-3! text-[12px]" type="button" @click="parametersOpen = false">
            Done
          </button>
        </div>
      </div>
    </div>
  </div>
</template>
