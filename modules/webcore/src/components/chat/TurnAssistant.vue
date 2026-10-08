<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { Copy, RotateCcw } from '@lucide/vue'
import ReasoningBlock from './ReasoningBlock.vue'
import { renderStreaming } from '@/lib/chat/stream-render'
import type { ChatMessage } from '@/core'

/**
 * One assistant turn: the reasoning that led to the answer, the answer itself,
 * and the actions that apply to it.
 *
 * The answer is plain text on the panel surface — no bubble, no avatar — because
 * it is the point of the page. A streaming answer ends in a caret so the reader
 * can see it grow without the layout shifting.
 */
const props = defineProps<{
  readOnly?: boolean
  message: ChatMessage
  onCopy: (text: string) => void
  onRetry: (id: string) => void
}>()

const streamingText = computed(() => props.message.pending === true)
const html = computed(() => renderStreaming(props.message.content, streamingText.value))
const thinking = computed(() => props.message.reasoningPending === true)

/** Drives the reasoning block's auto-open/auto-close. */
const reasoning = ref<InstanceType<typeof ReasoningBlock>>()
watch([thinking, reasoning], ([value]) => reasoning.value?.sync(value), { flush: 'post' })

</script>

<template>
  <div class="chat-turn group/turn">
    <ReasoningBlock
      v-if="message.reasoning || message.turnStartedAt !== undefined || message.turnElapsedMs !== undefined"
      ref="reasoning"
      :text="message.reasoning ?? ''"
      :streaming="thinking"
      :elapsed-ms="message.turnElapsedMs"
      :started-at="message.turnStartedAt"
    />
    <div
      v-if="message.content"
      class="md-body text-[15px] leading-[27px] text-foreground"
      v-html="html"
    />
    <!-- Only while tokens are actually arriving: a restored answer that was
         empty (a turn that only called tools) must not blink a caret. -->
    <span v-else-if="streamingText" class="chat-caret" aria-hidden="true" />
    <div class="mt-1 flex gap-1">
      <button
        v-if="message.content"
        class="chat-action"
        type="button"
        title="Copy answer"
        aria-label="Copy answer"
        @click="onCopy(message.content)"
      >
        <Copy class="h-3 w-3" /> Copy
      </button>
      <button
        v-if="!message.pending && !readOnly"
        class="chat-action"
        type="button"
        title="Restore this turn's context and regenerate the answer"
        aria-label="Retry"
        @click="onRetry(message.id)"
      >
        <RotateCcw class="h-3 w-3" /> Retry
      </button>
    </div>
  </div>
</template>
