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
import { usePromptResourcesStore } from '@/stores/prompt-resources'
import '@/components/resources/resources.css'
import { ArrowUp, ChevronUp, FileCode, ListPlus, Square, X } from '@lucide/vue'
import ContextMenu from './ContextMenu.vue'
import ContextMeter from './ContextMeter.vue'
import ModelMenu from './ModelMenu.vue'
import PermissionMenu from './PermissionMenu.vue'
import type {
  AddonInfo,
  ChatContextStats,
  ChatOptions,
  ChatUsage,
  FileTreeNode,
  LlmModelConfig,
  TodoItem,
} from '@/core'

/**
 * The composer.
 *
 * Two rows, because they answer two different questions: the first is *what am
 * I saying* — a textarea that grows, and the attachments that travel with it —
 * and the second is *how will it run*: who approves, which model and how hard
 * it thinks, how full the context is, and how the message is delivered. The
 * second row never moves or collapses, so the send control stays where the hand
 * expects it.
 *
 * The composer stays usable while the agent works: Enter queues into the
 * running turn, `Tab` queues explicitly, `⌘/Ctrl+Enter` steers it immediately,
 * and Enter on an empty composer sends what is queued. Dropping the user's
 * words is never an option.
 */
const props = defineProps<{
  draftValue?: string
  draftIdentity?: string
  onDraftChange?: (text: string) => void
  sendBlocked?: boolean
  readOnly?: boolean
  concierge?: { placeholder: string; sendDisabled: boolean }
  files: FileTreeNode[]
  pendingFiles: FileTreeNode[]
  addons: AddonInfo[]
  todos: TodoItem[]
  models: Record<string, LlmModelConfig>
  modelKeys: string[]
  model: string
  effort: NonNullable<ChatOptions['reasoning_effort']>
  contextStats: ChatContextStats | null
  usage: ChatUsage | null
  permissionMode: 'ask' | 'sandbox' | 'full'
  /** Whether a turn is running; the send button becomes a queue button. */
  running: boolean
  /** Whether a model is configured at all. */
  ready: boolean
  onSend: (text: string, accepted?: () => void) => boolean | void | Promise<boolean | void>
  onQueue: (text: string, accepted?: () => void) => boolean | void | Promise<boolean | void>
  onSteer: (text: string, accepted?: () => void) => boolean | void | Promise<boolean | void>
  onStop: () => void
  onRemoveFile: (path: string) => void
  onAttach: (file: FileTreeNode) => void
  onAttachPlan: () => void
  onOpenPlugins: () => void
  onSelectModel: (key: string) => void
  onSelectEffort: (effort: NonNullable<ChatOptions['reasoning_effort']>) => void
  onSelectPermission: (mode: 'ask' | 'sandbox' | 'full') => void
  onOpenParameters: () => void
  onOpenSettings: () => void
}>()

const localDraft = ref('')
function readDraft() {
  return props.draftValue ?? localDraft.value
}
function writeDraft(text: string) {
  if (props.draftValue !== undefined) props.onDraftChange?.(text)
  else localDraft.value = text
}
const draft = computed({ get: readDraft, set: writeDraft })
const submitting = ref(false),
  deliveryError = ref('')
let inputVersion = 0,
  submissionTicket = 0,
  alive = true,
  composing = false,
  lastCompositionEnd = -Infinity
let surfaceActive = true
onActivated(() => {
  surfaceActive = true
})
onDeactivated(() => {
  surfaceActive = false
  menu.value = null
  composing = false
})
function startComposition() {
  composing = true
}
function endComposition() {
  composing = false
  lastCompositionEnd = performance.now()
}
const prompts = usePromptResourcesStore()
watch(
  [
    draft,
    () => props.draftIdentity,
    () => props.pendingFiles,
    () => (props.concierge ? null : prompts.selections),
    () => props.model,
  ],
  () => {
    inputVersion++
  },
  { deep: true, flush: 'sync' },
)
const textarea = ref<HTMLTextAreaElement | null>(null)
/** Which upward menu is open; only one at a time keeps the row readable. */
const menu = ref<'context' | 'model' | 'permission' | 'delivery' | null>(null)

/** Prompt templates; they are inserted as text, not interpreted. */
const TEMPLATES = [
  { label: '/plan', desc: 'Break the task into a step-by-step plan' },
  { label: '/fix', desc: 'Diagnose and fix a problem in the workspace' },
  { label: '/explain', desc: 'Explain how the referenced code works' },
  { label: '/verify', desc: 'Verify the current change: build, tests, errors' },
]

/** The whitespace-delimited token being typed, for the suggestion menus. */
const token = computed(() => draft.value.split(/[\s\n]/).pop() ?? '')
const mentionQuery = computed(() =>
  token.value.startsWith('@') ? token.value.slice(1).toLowerCase() : '',
)
const commandQuery = computed(() =>
  token.value.startsWith('/') ? token.value.slice(1).toLowerCase() : '',
)
const mentionOpen = computed(() => !props.concierge && mentionQuery.value !== '')
const commandOpen = computed(() => !props.concierge && commandQuery.value !== '')

/** Flat, searchable file list for `@`. */
const flatFiles = computed(() => {
  const out: Array<{ name: string; path: string; depth: number }> = []
  const walk = (nodes: FileTreeNode[], depth: number) => {
    for (const node of nodes) {
      if (node.kind === 'file') out.push({ name: node.name, path: node.path, depth })
      if (node.children) walk(node.children, depth + 1)
    }
  }
  walk(props.files, 0)
  return out
})

const matches = computed(() => {
  const query = mentionQuery.value
  const list = query
    ? flatFiles.value.filter((f) => f.path.toLowerCase().includes(query))
    : flatFiles.value
  return list.slice(0, 40)
})

const commands = computed(() =>
  TEMPLATES.filter((t) => !commandQuery.value || t.label.slice(1).startsWith(commandQuery.value)),
)

const attached = computed(() => props.pendingFiles.map((f) => f.path))

/** Whether the composer has anything to say. */
const hasDraft = computed(() => draft.value.trim().length > 0)

function toggle(which: 'context' | 'model' | 'permission' | 'delivery'): void {
  menu.value = menu.value === which ? null : which
}

/** Replaces the token under the cursor with `text`. */
function replaceToken(text: string): void {
  draft.value = draft.value.replace(/[\s@/][^\s]*$/, '')
  draft.value = draft.value ? `${draft.value.trimEnd()} ${text}` : text
  void nextTick(() => {
    textarea.value?.focus()
    autosize()
  })
}

function pickFile(entry: { path: string; name: string }): void {
  replaceToken('')
  props.onAttach({ name: entry.name, path: entry.path, kind: 'file' })
  menu.value = null
}

function pickCommand(entry: { label: string }): void {
  replaceToken(entry.label)
  menu.value = null
}

function autosize(): void {
  const el = textarea.value
  if (!el) return
  el.style.height = 'auto'
  el.style.height = `${Math.min(el.scrollHeight, 220)}px`
}
watch(draft, () => void nextTick(autosize))

/**
 * Escape closes an open menu wherever the focus is.
 *
 * The composer's own key handler only sees keys typed into the textarea, so
 * without this an Escape pressed while a menu button has focus leaves the menu
 * (and its click-catching backdrop) in the way.
 */
function onDocumentKeydown(event: KeyboardEvent): void {
  if (!surfaceActive) return
  if (event.isComposing || composing || event.keyCode === 229) return
  if (event.key === 'Escape' && menu.value) {
    menu.value = null
    event.preventDefault()
    event.stopPropagation()
  }
}

onMounted(() => {
  autosize()
  document.addEventListener('keydown', onDocumentKeydown)
})
onBeforeUnmount(() => {
  alive = false
  document.removeEventListener('keydown', onDocumentKeydown)
})

/** Enter sends, queues or delivers the queue; Shift+Enter breaks the line. */
function onKeydown(event: KeyboardEvent): void {
  if (event.isComposing || event.keyCode === 229 || composing) return
  if (['Enter', 'Tab'].includes(event.key) && performance.now() - lastCompositionEnd < 60) return
  if (event.repeat && ['Enter', 'Tab'].includes(event.key)) {
    event.preventDefault()
    return
  }
  if (event.key === 'Escape') {
    if (menu.value) {
      menu.value = null
      event.preventDefault()
      event.stopPropagation()
    } else if (mentionOpen.value || commandOpen.value) {
      draft.value = draft.value.replace(/[\s@/][^\s]*$/, '')
      event.preventDefault()
      event.stopPropagation()
    }
    return
  }
  if (event.key === 'Tab' && props.running && hasDraft.value) {
    // Tab is the explicit "queue this" key while a turn runs.
    event.preventDefault()
    submit('queue')
    return
  }
  if (event.key !== 'Enter' || event.shiftKey) return
  event.preventDefault()
  if (!props.ready || props.sendBlocked || submitting.value || props.readOnly) return
  if (props.running && !hasDraft.value) {
    // Enter on an empty composer sends what was queued, so holding Enter keeps
    // work moving instead of looking like the input was swallowed.
    props.onSteer('')
    return
  }
  submit(event.metaKey || event.ctrlKey ? 'steer' : 'auto')
}

/** Delivers the draft according to the delivery mode. */
async function submit(mode: 'auto' | 'queue' | 'steer'): Promise<void> {
  const text = draft.value,
    version = inputVersion,
    identity = props.draftIdentity
  if (
    !text.trim() ||
    !props.ready ||
    props.sendBlocked ||
    props.readOnly ||
    submitting.value ||
    composing ||
    (props.concierge?.sendDisabled && mode !== 'steer')
  )
    return
  const ticket = ++submissionTicket
  submitting.value = true
  deliveryError.value = ''
  let acknowledged = false
  const accepted = () => {
    if (acknowledged) return
    acknowledged = true
    if (!alive || props.draftIdentity !== identity) return
    // Controlled drafts are cleared by their versioned owner. The standalone
    // supervision composer still compares the exact local input version.
    if (
      props.draftValue === undefined &&
      inputVersion === version &&
      draft.value === text &&
      textarea.value?.value === text.replace(/\r\n?/g, '\n')
    )
      draft.value = ''
    if (submissionTicket === ticket) submitting.value = false
    void nextTick(autosize)
  }
  try {
    const action = props.concierge
      ? mode === 'steer'
        ? props.onSteer
        : props.onSend
      : !props.running
        ? props.onSend
        : mode === 'steer'
          ? props.onSteer
          : props.onQueue
    if ((await action(text, accepted)) === true) accepted()
  } catch (error) {
    if (alive && props.draftIdentity === identity) deliveryError.value = String(error)
  } finally {
    if (alive && submissionTicket === ticket) submitting.value = false
  }
}

/** Handlers that close the menu they were opened from. */
function pickContextFile(file: { path: string; name: string }): void {
  pickFile(file)
}
function attachPlan(): void {
  props.onAttachPlan()
  menu.value = null
}
function openPlugins(): void {
  props.onOpenPlugins()
  menu.value = null
}
function openParameters(): void {
  props.onOpenParameters()
  menu.value = null
}
function openSettings(): void {
  props.onOpenSettings()
  menu.value = null
}
function selectPermission(mode: 'ask' | 'sandbox' | 'full'): void {
  props.onSelectPermission(mode)
  menu.value = null
}
function deliver(mode: 'queue' | 'steer'): void {
  submit(mode)
  menu.value = null
}
function stopAndClose(): void {
  props.onStop()
  menu.value = null
}

const placeholder = computed(() =>
  props.concierge
    ? props.concierge.placeholder
    : !props.ready
      ? 'Configure a model to start chatting'
      : props.running
        ? 'Queue a message for this turn — Tab to queue, ⌘/Ctrl+Enter to send it now'
        : 'Describe a task — @ to reference a file, / for a template',
)

defineExpose({
  focus: () => textarea.value?.focus(),
  setDraft: (text: string) => {
    draft.value = text
    void nextTick(() => textarea.value?.focus())
  },
})
</script>

<template>
  <div class="chat-composer" @dragover.prevent @drop="!concierge && prompts.dropChat($event)">
    <p v-if="deliveryError" role="alert" class="px-3 py-2 text-[13px] text-danger">
      {{ deliveryError }}
    </p>
    <div v-if="!concierge && prompts.selections.length" class="chat-composer-chips">
      <span v-for="(item, index) in prompts.selections" :key="item.snapshot.id" class="pv-chip"
        ><span :title="item.snapshot.body"
          >{{ item.snapshot.reference.scope.scope }} / {{ item.snapshot.name }} · r{{
            item.snapshot.reference.revision
          }}</span
        ><span v-if="item.status !== 'current'">{{ item.status }}</span
        ><button v-if="item.status === 'changed'" class="pv-link" @click="prompts.refresh(index)">
          Refresh selection</button
        ><button
          :aria-label="`Remove selected prompt ${index + 1}`"
          @click="prompts.selections.splice(index, 1)"
        >
          ×
        </button></span
      >
    </div>
    <p v-if="!concierge && prompts.error" role="alert" class="pv-callout">{{ prompts.error }}</p>
    <div v-if="pendingFiles.length" class="chat-composer-chips">
      <span v-for="file in pendingFiles" :key="file.path" class="chat-chip">
        <FileCode class="h-3 w-3 shrink-0" />
        <span
          class="max-w-60 truncate text-[13px]"
          :title="
            file.reference
              ? `Saved version ${file.reference.snapshotId} · SHA256 ${file.reference.digest}`
              : 'Path only; review the file before sending'
          "
          >{{ file.path }} · {{ file.reference ? 'Saved' : 'Path only' }}</span
        >
        <button
          class="ml-0.5 rounded hover:text-foreground"
          type="button"
          :aria-label="`Remove ${file.path}`"
          @click="onRemoveFile(file.path)"
        >
          <X class="h-3 w-3" />
        </button>
      </span>
    </div>

    <div class="chat-composer-input">
      <div
        v-if="mentionOpen || commandOpen"
        class="chat-menu absolute bottom-full left-2 mb-2 max-h-64 w-80 overflow-y-auto"
      >
        <template v-if="mentionOpen">
          <p class="chat-menu-head">Reference a file</p>
          <p v-if="!matches.length" class="px-2 py-1.5 text-[12.5px] text-subtle">No matches</p>
          <button
            v-for="entry in matches"
            :key="entry.path"
            class="chat-menu-item"
            type="button"
            @mousedown.prevent="pickFile(entry)"
          >
            <FileCode class="h-4 w-4 shrink-0 text-subtle" />
            <span class="min-w-0 flex-1 truncate" :style="{ paddingLeft: entry.depth * 10 + 'px' }">
              {{ entry.name }}
            </span>
            <span v-if="attached.includes(entry.path)" class="shrink-0 text-[11px] text-subtle">
              attached
            </span>
          </button>
        </template>
        <template v-else>
          <p class="chat-menu-head">Templates</p>
          <p v-if="!commands.length" class="px-2 py-1.5 text-[12.5px] text-subtle">No match</p>
          <button
            v-for="entry in commands"
            :key="entry.label"
            class="chat-menu-item"
            type="button"
            @mousedown.prevent="pickCommand(entry)"
          >
            <span class="w-16 shrink-0 font-mono text-[12.5px] text-primary">{{
              entry.label
            }}</span>
            <span class="min-w-0 flex-1 truncate text-muted-foreground">{{ entry.desc }}</span>
          </button>
        </template>
      </div>

      <textarea
        ref="textarea"
        v-model="draft"
        rows="1"
        class="chat-composer-textarea"
        :placeholder="placeholder"
        :readonly="readOnly"
        aria-label="Message"
        @keydown="onKeydown"
        @compositionstart="startComposition"
        @compositionend="endComposition"
      />
    </div>

    <div class="chat-composer-controls">
      <div v-if="!concierge" class="chat-composer-options flex min-w-0 items-center gap-1">
        <ContextMenu
          :files="files"
          :addons="addons"
          :todos="todos"
          :attached-paths="attached"
          :open="menu === 'context'"
          :on-toggle="() => toggle('context')"
          :on-pick-file="pickContextFile"
          :on-attach-plan="attachPlan"
          :on-open-plugins="openPlugins"
          :on-remove="onRemoveFile"
        />
        <PermissionMenu
          :mode="permissionMode"
          :open="menu === 'permission'"
          :on-toggle="() => toggle('permission')"
          :on-select="selectPermission"
        />
        <ModelMenu
          :models="models"
          :model-keys="modelKeys"
          :model="model"
          :effort="effort"
          :open="menu === 'model'"
          :on-toggle="() => toggle('model')"
          :on-select-model="onSelectModel"
          :on-select-effort="onSelectEffort"
          :on-open-parameters="openParameters"
          :on-open-settings="openSettings"
        />
      </div>

      <span v-else class="text-[12px] text-muted-foreground">Concierge</span>
      <div class="flex shrink-0 items-center gap-1.5">
        <ContextMeter v-if="!concierge" :stats="contextStats" :usage="usage" />

        <div class="relative flex items-center">
          <!-- One control: the action on the left, its alternatives behind the
               chevron on the right. Two separate buttons here read as two send
               buttons. -->
          <div class="chat-deliver-group">
            <button
              v-if="running"
              class="chat-deliver"
              type="button"
              title="Queue this message for the running turn (Tab)"
              aria-label="Queue message"
              :disabled="!ready || !hasDraft || sendBlocked || submitting || readOnly"
              @click="submit('queue')"
            >
              <ListPlus class="h-4 w-4" />
            </button>
            <button
              v-else
              class="chat-send"
              type="button"
              :disabled="
                !ready ||
                !hasDraft ||
                concierge?.sendDisabled ||
                sendBlocked ||
                submitting ||
                readOnly
              "
              title="Send (Enter)"
              aria-label="Send"
              @click="submit('auto')"
            >
              <ArrowUp class="h-4 w-4" />
            </button>
            <button
              class="chat-deliver-more"
              type="button"
              :aria-expanded="menu === 'delivery'"
              aria-label="Delivery options"
              @click="toggle('delivery')"
            >
              <ChevronUp class="h-3.5 w-3.5" />
            </button>
          </div>

          <div v-if="menu === 'delivery'" class="chat-menu chat-menu-up chat-delivery-menu w-64">
            <p class="chat-menu-head">Delivery</p>
            <button
              v-if="!concierge"
              class="chat-menu-item"
              type="button"
              :disabled="!ready || sendBlocked || readOnly || submitting || !hasDraft"
              @click="deliver('queue')"
            >
              <span class="font-medium">Queue</span>
              <span class="text-[12px] text-subtle">after the current step</span>
            </button>
            <button
              class="chat-menu-item"
              type="button"
              :disabled="!ready || sendBlocked || readOnly || submitting || !hasDraft"
              @click="deliver('steer')"
            >
              <span class="font-medium">{{ concierge ? 'Urgent message' : 'Send now' }}</span>
              <span class="text-[12px] text-subtle">before the next model call</span>
            </button>
            <button
              class="chat-menu-item"
              type="button"
              :disabled="!!concierge && !ready"
              @click="stopAndClose"
            >
              <Square class="h-4 w-4 shrink-0 text-status-error" />
              <span class="font-medium">Stop and take over</span>
            </button>
          </div>
        </div>

        <button
          v-if="running"
          class="chat-stop"
          type="button"
          title="Stop the turn (Esc)"
          aria-label="Stop"
          @click="onStop"
        >
          <Square class="h-3.5 w-3.5" />
        </button>
      </div>
    </div>

    <div v-if="menu" class="fixed inset-0 z-30" @mousedown="menu = null" />
  </div>
</template>
