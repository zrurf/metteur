<script setup lang="ts">
import { computed } from 'vue'
import { Bot, ChevronRight, Circle, Play, Square } from '@lucide/vue'
import type { ExecTreeData } from '@/core'

/** The tree payload and the node id to render one row (recursive). */
const props = defineProps<{
  nodeId: string
  tree: ExecTreeData
  depth: number
}>()

const node = computed(() => props.tree.nodes.find((n) => n.id === props.nodeId))
const children = computed(() => node.value?.children ?? [])

/** The leaf kind glyph: run = root, subagent = bot, functions = layers. */
const glyph = computed(() => {
  const kind = node.value?.kind ?? ''
  if (kind === 'run') return Play
  if (kind.startsWith('subagent')) return Bot
  if (kind.startsWith('function') || kind.startsWith('node:')) return Square
  return Circle
})

/** A human label from the machine kind string. */
const kindLabel = computed(() => {
  const kind = node.value?.kind ?? ''
  const colon = kind.indexOf(':')
  return colon >= 0 ? kind.slice(0, colon) : kind
})

const tone = computed(() => {
  const status = node.value?.status ?? ''
  if (status.startsWith('failed')) return 'text-danger'
  if (status === 'done') return 'text-muted-foreground'
  return 'text-primary'
})
</script>

<template>
  <div v-if="node">
    <div class="flex items-center gap-1.5" :style="{ paddingLeft: `${depth * 14}px` }">
      <ChevronRight v-if="children.length" class="h-3 w-3 shrink-0 text-subtle" />
      <span v-else class="w-3 shrink-0" />
      <component :is="glyph" class="h-3 w-3 shrink-0 text-subtle" />
      <span class="truncate text-[11.5px] text-foreground">{{ node.label }}</span>
      <span class="shrink-0 text-[9.5px] uppercase tracking-wide text-subtle">{{ kindLabel }}</span>
      <span v-if="node.tokens" class="ml-auto shrink-0 text-[10px] tabular-nums text-subtle">
        {{ node.tokens.toLocaleString() }}
      </span>
      <span class="h-1.5 w-1.5 shrink-0 rounded-full" :class="tone" />
    </div>
    <ExecTreeRow
      v-for="child in children"
      :key="child"
      :node-id="child"
      :tree="tree"
      :depth="depth + 1"
    />
  </div>
</template>
