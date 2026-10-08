<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue'
import { ArrowDown } from '@lucide/vue'
import TurnUser from './TurnUser.vue'
import TurnAssistant from './TurnAssistant.vue'
import ToolCallCard from './ToolCallCard.vue'
import ToolCallGroup from './ToolCallGroup.vue'
import type { ChatMessage } from '@/core'

/**
 * The conversation transcript.
 *
 * Rendering decisions that make a long run readable:
 * - consecutive tool calls collapse into one group, and a lone call renders as a
 *   row without a header repeating it;
 * - the view follows new content only while the reader is already at the bottom,
 *   and offers a jump button instead of yanking the viewport;
 * - auto-follow uses the scroll container's size changes rather than a timer, so
 *   a growing answer does not fight a manual scroll.
 */
const props = defineProps<{
  messages: ChatMessage[]
  readOnly?: boolean
  onOpenFile: (path: string) => void
  onCopy: (text: string) => void
  onEdit: (text: string) => void
  onRetry: (id: string) => void
  onRestore: (snapshotId: string) => void
}>()

/** A transcript row: one message, or a run of tool calls. */
type Row =
  | { kind: 'message'; message: ChatMessage }
  | { kind: 'tools'; messages: ChatMessage[]; key: string }

const rows = computed<Row[]>(() => {
  const out: Row[] = []
  let run: ChatMessage[] = []
  const flush = () => {
    if (!run.length) return
    const first = run[0]
    out.push({ kind: 'tools', messages: run, key: `g-${first.id}` })
    run = []
  }
  for (const message of props.messages) {
    if (message.role === 'tool') {
      run.push(message)
      continue
    }
    flush()
    out.push({ kind: 'message', message })
  }
  flush()
  return out
})

const scroller = ref<HTMLElement | null>(null)
const atBottom = ref(true)

function onScroll(): void {
  updateAtBottom()
}

/**
 * Whether the view is pinned to the newest message.
 *
 * Content that does not overflow counts as "at the bottom" — otherwise a short
 * conversation would show the jump button forever, because there is nothing to
 * scroll and therefore no scroll event to correct the state.
 */
function updateAtBottom(): void {
  const el = scroller.value
  if (!el) return
  const overflow = el.scrollHeight - el.clientHeight
  // A small slack keeps "at the bottom" true while a streamed line grows.
  atBottom.value = overflow <= 4 || el.scrollTop >= overflow - 48
}

function scrollToBottom(smooth = false): void {
  nextTick(() => {
    const el = scroller.value
    if (!el) return
    // The welcome screen is not a transcript: keep its heading in view even
    // when the starter cards overflow a short pane.
    const top = props.messages.length ? Math.max(0, el.scrollHeight - el.clientHeight) : 0
    if (Math.abs(el.scrollTop - top) < 1) return
    el.scrollTo({ top, behavior: smooth ? 'smooth' : 'auto' })
  })
}

// Content growth is observed, not polled: a ResizeObserver on the inner list
// fires exactly when the transcript changes size.
const inner = ref<HTMLElement | null>(null)
let observer: ResizeObserver | undefined
let scrollerObserver: ResizeObserver | undefined
onBeforeUnmount(() => {
  observer?.disconnect()
  scrollerObserver?.disconnect()
})
watch(
  inner,
  (el) => {
    observer?.disconnect()
    if (!el) return
    observer = new ResizeObserver(() => {
      updateAtBottom()
      if (atBottom.value) scrollToBottom()
    })
    observer.observe(el)
  },
  { immediate: true },
)
// The scroller's own height changes when the pane is resized, when a panel
// opens, or when it first gets a size — all of which decide whether content
// overflows at all.
watch(
  scroller,
  (el) => {
    scrollerObserver?.disconnect()
    if (!el) return
    scrollerObserver = new ResizeObserver(() => updateAtBottom())
    scrollerObserver.observe(el)
  },
  { immediate: true },
)

watch(
  () => props.messages.length,
  () => {
    // A new turn always pulls the view down, even if the reader had scrolled up.
    atBottom.value = true
    scrollToBottom()
  },
  { immediate: true },
)

defineExpose({ scrollToBottom })
</script>

<template>
  <div
    ref="scroller"
    class="relative min-h-0 flex-1 overflow-y-auto"
    role="log"
    aria-live="polite"
    aria-label="Conversation"
    @scroll.passive="onScroll"
  >
    <div ref="inner" class="chat-column chat-thread-inner">
      <slot v-if="!messages.length" name="empty" />
      <template v-for="row in rows" :key="row.kind === 'tools' ? row.key : row.message.id">
        <div v-if="row.kind === 'tools'" class="chat-activity pl-0.5">
          <ToolCallGroup
            v-if="row.messages.length > 1"
            :messages="row.messages"
            :on-open-file="onOpenFile"
          />
          <ToolCallCard
            v-else
            :message="row.messages[0]"
            :on-open-file="onOpenFile"
          />
        </div>
        <template v-else>
          <div v-if="row.message.role === 'error'" class="flex justify-center">
            <div class="chat-error-turn">{{ row.message.content }}</div>
          </div>
          <p v-else-if="row.message.role === 'notice'" class="chat-notice">
            {{ row.message.content }}
          </p>
          <TurnUser
            v-else-if="row.message.role === 'user'"
            :read-only="readOnly"
            :content="row.message.content"
            :created-at="row.message.createdAt"
            :queued="row.message.queued === true"
            :checkpoint="row.message.checkpoint"
            :on-open-file="onOpenFile"
            :on-copy="onCopy"
            :on-edit="onEdit"
            :on-restore="onRestore"
          />
          <TurnAssistant
            v-else
            :read-only="readOnly"
            :message="row.message"
            :on-copy="onCopy"
            :on-retry="onRetry"
          />
        </template>
      </template>
      <slot name="footer" />
    </div>

    <button
      v-if="!atBottom"
      class="glass sticky bottom-4 left-1/2 z-10 flex h-8 -translate-x-1/2 items-center gap-1.5 rounded-full px-3 text-[11.5px] text-foreground shadow-popover"
      type="button"
      @click="scrollToBottom(true)"
    >
      <ArrowDown class="h-3.5 w-3.5" /> New messages
    </button>
  </div>
</template>
