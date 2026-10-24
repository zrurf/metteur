<script setup lang="ts">
import { computed, ref } from 'vue'
import { X } from '@lucide/vue'
import { formatTokens } from '@/lib/chat/liveness'
import type { ChatContextStats, ChatUsage } from '@/core'
const props = defineProps<{ stats: ChatContextStats | null; usage: ChatUsage | null }>()
const open = ref(false),
  trigger = ref<HTMLButtonElement>()
const used = computed(() => props.stats?.tokens ?? null)
const limit = computed(() => props.stats?.limit ?? null)
const percent = computed(() =>
  used.value !== null && limit.value && limit.value > 0
    ? Math.min(100, Math.max(0, Math.round((used.value / limit.value) * 100)))
    : null,
)
const free = computed(() =>
  used.value !== null && limit.value ? Math.max(0, limit.value - used.value) : null,
)
const tone = computed(() =>
  (percent.value ?? 0) >= 95 ? 'critical' : (percent.value ?? 0) >= 80 ? 'warning' : 'normal',
)
const labels: Record<string, string> = {
  system: 'System & Tools',
  user: 'Your Messages',
  assistant: 'Agent Messages',
  tool: 'Tool Results',
}
const colors = ['var(--primary)', '#9175c7', '#4c9a89', '#d39a52']
const regions = computed(() =>
  (props.stats?.regions ?? []).map((region, index) => ({
    ...region,
    label: labels[region.region] ?? region.region,
    color: colors[index % colors.length],
    width: limit.value ? Math.min(100, Math.max(0, (region.tokens / limit.value) * 100)) : 0,
  })),
)
const cache = computed(() =>
  props.usage?.inputTokens && typeof props.usage.cachedInputTokens === 'number'
    ? Math.round((props.usage.cachedInputTokens / props.usage.inputTokens) * 100)
    : null,
)
function close() {
  open.value = false
  trigger.value?.focus()
}
</script>
<template>
  <div class="context-capacity" :data-tone="tone" @keydown.esc.stop="close">
    <button
      ref="trigger"
      type="button"
      class="chat-meter"
      aria-haspopup="dialog"
      :aria-expanded="open"
      :aria-label="
        percent === null ? 'Context: not measured' : `Context: ${percent}% of the window in use`
      "
      @click="open = !open"
    >
      <span class="context-mini-track" aria-hidden="true"
        ><span :style="{ width: `${percent ?? 0}%` }"
      /></span>
      <span>{{ percent === null ? 'Context' : `${percent}%` }}</span>
    </button>
    <template v-if="open">
      <div class="context-backdrop" @mousedown.prevent="close" />
      <section class="context-popover" role="dialog" aria-label="Context Window" tabindex="-1">
        <header>
          <h2>Context Window</h2>
          <button type="button" class="btn-icon" aria-label="Close Context Window" @click="close">
            <X :size="15" />
          </button>
        </header>
        <div class="context-total">
          <strong>{{ used === null ? 'Not Measured' : formatTokens(used) }}</strong
          ><span v-if="limit">/ {{ formatTokens(limit) }} tokens</span>
        </div>
        <p class="context-caption">
          {{
            percent === null
              ? 'Usage will appear after the next model response.'
              : `${percent}% used · ${formatTokens(free ?? 0)} available`
          }}
        </p>
        <div
          v-if="percent !== null"
          class="context-track"
          role="progressbar"
          aria-label="Context Used"
          :aria-valuenow="percent"
          :aria-valuemin="0"
          :aria-valuemax="100"
        >
          <span
            v-if="!regions.length"
            :style="{ width: `${percent}%`, background: 'var(--primary)' }"
          />
          <span
            v-for="region in regions"
            :key="region.region"
            :style="{ width: `${region.width}%`, background: region.color }"
          />
        </div>
        <dl v-if="regions.length" class="context-regions">
          <div v-for="region in regions" :key="region.region">
            <dt><i :style="{ background: region.color }" />{{ region.label }}</dt>
            <dd>{{ formatTokens(region.tokens) }}</dd>
          </div>
        </dl>
        <footer>
          <span>{{
            props.stats?.assumedLimit ? 'Window size assumed' : 'Estimated context usage'
          }}</span
          ><span v-if="cache !== null">{{ cache }}% cache hit</span>
        </footer>
        <p v-if="props.stats?.assumedLimit" class="context-assumed">
          Set this model’s window in Settings → LLM &amp; Models to replace the default.
        </p>
      </section>
    </template>
  </div>
</template>
<style scoped>
.context-capacity {
  position: relative;
  --capacity-color: var(--primary);
}
.context-capacity[data-tone='warning'] {
  --capacity-color: var(--status-warning);
}
.context-capacity[data-tone='critical'] {
  --capacity-color: var(--status-error);
}
.chat-meter {
  display: flex;
  align-items: center;
  gap: 7px;
  padding: 7px 9px;
  border-radius: 7px;
  color: var(--muted-foreground);
  font-size: 11px;
  font-variant-numeric: tabular-nums;
}
.chat-meter:hover {
  background: var(--hover);
  color: var(--foreground);
}
.context-mini-track {
  width: 26px;
  height: 4px;
  border-radius: 2px;
  background: var(--border);
  overflow: hidden;
}
.context-mini-track > span {
  display: block;
  height: 100%;
  background: var(--capacity-color);
}
.context-backdrop {
  position: fixed;
  inset: 0;
  z-index: 40;
}
.context-popover {
  position: absolute;
  right: 0;
  bottom: calc(100% + 12px);
  z-index: 41;
  width: 320px;
  max-width: calc(100vw - 32px);
  padding: 18px;
  border: 1px solid var(--border);
  border-radius: 12px;
  background: var(--background);
  box-shadow: var(--shadow-popover);
  color: var(--foreground);
}
header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
}
h2 {
  margin: 0;
  font-size: 13px;
  font-weight: 600;
}
.context-total {
  display: flex;
  gap: 7px;
  align-items: baseline;
  margin-top: 18px;
  font-variant-numeric: tabular-nums;
}
.context-total strong {
  font-size: 26px;
  font-weight: 600;
  letter-spacing: -0.03em;
}
.context-total > span,
.context-caption {
  font-size: 11px;
  color: var(--muted-foreground);
}
.context-caption {
  margin-top: 4px;
}
.context-track {
  display: flex;
  overflow: hidden;
  height: 7px;
  margin: 16px 0;
  border-radius: 4px;
  background: var(--border);
}
.context-track > span {
  flex-shrink: 1;
  height: 100%;
}
.context-regions {
  display: grid;
  gap: 10px;
  margin: 14px 0 18px;
  font-size: 12px;
}
.context-regions > div {
  display: flex;
  justify-content: space-between;
  gap: 12px;
}
dt {
  display: flex;
  align-items: center;
  gap: 8px;
  color: var(--muted-foreground);
}
dt i {
  width: 7px;
  height: 7px;
  border-radius: 2px;
}
dd {
  font-variant-numeric: tabular-nums;
}
footer {
  display: flex;
  justify-content: space-between;
  gap: 10px;
  padding-top: 12px;
  border-top: 1px solid var(--border);
  font-size: 10px;
  color: var(--subtle);
}
.context-assumed {
  margin-top: 10px;
  color: var(--subtle);
  font-size: 11px;
  line-height: 1.6;
}
</style>
