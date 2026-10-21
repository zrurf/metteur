<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { Loader2 } from '@lucide/vue'
import {
  cacheHitRate,
  effectivePhase,
  formatTokens,
  isSlow,
  phaseLabel,
  type ChatPhase,
} from '@/lib/chat/liveness'
import type { ChatUsage } from '@/core'

/**
 * What the agent is doing and its token usage.
 *
 * One line that answers the question a spinner cannot: which phase the run is
 * in and how many tokens it has spent. Reasoning duration is shown only in the
 * reasoning block, not duplicated above the composer.
 */
const props = defineProps<{
  waitingForApproval?: boolean
  phase: ChatPhase
  lastEventAt: number
  usage: ChatUsage | null
  /** Tool call whose arguments are still being written. */
  pendingTool: { name: string; bytes: number } | null
}>()

const now = ref(Date.now())
const timer = setInterval(() => (now.value = Date.now()), 1000)
onBeforeUnmount(() => clearInterval(timer))

const active = computed(() => props.waitingForApproval || props.phase !== 'idle' || props.pendingTool !== null)
const sinceLastEvent = computed(() => now.value - props.lastEventAt)
const shown = computed(() =>
  props.pendingTool ? 'generating' : effectivePhase(props.phase, sinceLastEvent.value),
)
/** Human-readable size of the arguments composed so far. */
const pendingLabel = computed(() => {
  const pending = props.pendingTool
  if (!pending) return ''
  const kb = pending.bytes / 1024
  const size = kb < 1 ? `${pending.bytes} B` : `${kb.toFixed(1)} KB`
  return `Preparing ${pending.name || 'a tool call'} · ${size}`
})
const slow = computed(() => active.value && !props.waitingForApproval && isSlow(sinceLastEvent.value))

const tokens = computed(() => {
  const usage = props.usage
  if (!usage) return ''
  const cache = cacheHitRate(usage.cachedInputTokens, usage.inputTokens)
  const parts = [
    `${formatTokens(usage.inputTokens)} in`,
    `${formatTokens(usage.outputTokens)} out`,
  ]
  if (cache) parts.push(`${cache} cached`)
  return parts.join(' · ')
})

defineExpose({ reset: () => (now.value = Date.now()) })
watch(active, () => (now.value = Date.now()))
</script>

<template>
  <div v-if="active || tokens" class="chat-status" role="status">
    <template v-if="active">
      <Loader2 class="h-3 w-3 shrink-0 animate-spin text-status-running" />
      <span class="chat-status-label">{{ waitingForApproval ? 'Waiting for your approval' : pendingLabel || phaseLabel(shown) }}</span>
      <span v-if="slow" class="text-subtle">· still working, the model is slow</span>
    </template>
    <span v-if="tokens" class="ml-auto font-mono tabular-nums text-subtle">{{ tokens }}</span>
  </div>
</template>
