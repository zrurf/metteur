<script setup lang="ts">
import { computed, nextTick, provide, ref, watch } from 'vue'
import { VueFlow, useVueFlow, type Node, type Edge } from '@vue-flow/core'
import { Background } from '@vue-flow/background'
import { Crosshair, Maximize2 } from '@lucide/vue'
import type { Blueprint } from '@/core'
import type { ExecutionSnapshot, Invocation } from '@/core/execution-view'
import { blueprintEdgeVisual } from '@/lib/blueprint-edges'
import BlueprintNode from '@/components/BlueprintNode.vue'
import '@vue-flow/core/dist/style.css'
import '@vue-flow/core/dist/theme-default.css'
const props = defineProps<{ blueprint: Blueprint; snapshot?: ExecutionSnapshot; live: boolean }>()
const emit = defineEmits<{ inspect: [invocation: Invocation | null] }>()
provide('bp-pin-context', () => {})
const selected = ref(''), follow = ref(true), context = ref('')
const { fitView, setCenter } = useVueFlow('execution')
const contexts = computed(() => {
  const result = new Map<string, { scope: string; frame: string[]; label: string }>()
  result.set('', { scope: props.blueprint.id, frame: [], label: props.blueprint.name })
  for (const i of props.snapshot?.view?.invocations ?? []) {
    if (!i.frame.length) continue
    result.set(JSON.stringify([i.scope, i.frame]), { scope: i.scope, frame: i.frame, label: `${props.snapshot?.view?.graphs[i.scope]?.name ?? 'Function'} · ${i.frame.at(-1)}` })
  }
  return result
})
const frame = computed(() => contexts.value.get(context.value) ?? contexts.value.get('')!)
const graph = computed(() => context.value ? props.snapshot?.view?.graphs[frame.value.scope] ?? props.blueprint : props.blueprint)
const latest = computed(() => new Map((props.snapshot?.view?.invocations ?? []).filter(i => i.scope === frame.value.scope && JSON.stringify(i.frame) === JSON.stringify(frame.value.frame)).map(i => [i.node_id, i])))
function state(id: string) {
  const i = latest.value.get(id)
  if (!i) return props.snapshot?.view?.root ? 'Pending' : 'Unavailable'
  if (i.current === false) return 'Pending'
  if (props.snapshot?.pending.includes(id) && !context.value && i.finished_at) return 'Pending'
  if (i.status === 'Running' && !props.live) return 'Not advancing'
  return i.status
}
const nodes = computed<Node[]>(() => graph.value.nodes.map(n => ({ id: n.id, type: 'runtime', position: n.position,
  data: { ...n, kind: n.type, values: {}, status: state(n.id) === 'Running' ? 'running' : state(n.id) === 'Completed' ? 'done' : undefined, runtimeState: state(n.id),
    connectedIn: graph.value.edges.filter(e => e.target === n.id).map(e => e.targetHandle), connectedOut: graph.value.edges.filter(e => e.source === n.id).map(e => e.sourceHandle) },
})))
const traversed = computed(() => new Map((props.snapshot?.view?.edges ?? []).filter(e => e.scope === frame.value.scope && JSON.stringify(e.frame) === JSON.stringify(frame.value.frame)).map(e => [e.edge_id, e])))
const edges = computed<Edge[]>(() => graph.value.edges.map(e => {
  const candidate = traversed.value.get(e.id)
  const source = latest.value.get(e.source)
  const recorded = source?.current !== false && source?.sequence === candidate?.sequence ? candidate : undefined
  const live = props.live && !!recorded && latest.value.get(e.target)?.current !== false && latest.value.get(e.target)?.status === 'Running' && recorded.sequence <= latest.value.get(e.target)!.sequence
  const node = graph.value.nodes.find(n => n.id === e.source)
  const exec = node?.outputs.find(p => p.id === e.sourceHandle)?.kind === 'exec-out'
  const visual = blueprintEdgeVisual(exec, node?.category)
  return { ...e, ...visual, animated: live, class: `${visual.class} ${recorded ? 'is-traversed' : ''}`, style: { ...visual.style, opacity: recorded ? 1 : .35 } }
}))
async function fit() { await nextTick(); await fitView({ padding: .22 }) }
watch(() => props.snapshot?.view?.sequence, () => {
  if (!follow.value || !props.live) return
  const latestRun = props.snapshot?.view?.invocations.at(-1)
  if (!latestRun) return
  context.value = latestRun.frame.length ? JSON.stringify([latestRun.scope, latestRun.frame]) : ''
  selected.value = latestRun.node_id
  emit('inspect', latestRun)
  const position = graph.value.nodes.find(n => n.id === selected.value)?.position
  if (position) void setCenter(position.x + 100, position.y + 50, { zoom: .9 })
})
watch(context, () => { selected.value = ''; void fit() })
</script>
<template>
  <div class="graph-tools"><button aria-label="Follow execution" :aria-pressed="follow" @click="follow = !follow"><Crosshair :size="15" /></button><button aria-label="Fit graph" @click="fit"><Maximize2 :size="15" /></button><select v-if="contexts.size > 1" v-model="context" aria-label="Execution frame"><option v-for="([key, value]) in contexts" :key="key" :value="key">{{ value.label }}</option></select></div>
  <VueFlow id="execution" class="blueprint-flow execution-flow" :nodes="nodes" :edges="edges" :nodes-draggable="false" :nodes-connectable="false" :edges-updatable="false" :delete-key-code="null" :zoom-on-double-click="false" :fit-view-on-init="true" @nodes-initialized="fit" @node-click="({ node }) => { selected = node.id; emit('inspect', latest.get(node.id) ?? null) }">
    <Background :gap="20" :size="1" pattern-color="var(--grid-dot)" />
    <template #node-runtime="{ id, data }"><div class="runtime-node"><BlueprintNode readonly :id="id" :data="data" :selected="selected === id" /><div class="node-caption">{{ data.runtimeState }}</div></div></template>
  </VueFlow>
</template>
<style scoped>
.graph-tools{position:absolute;right:122px;top:12px;display:flex;gap:3px;padding:4px;border:1px solid var(--border);border-radius:8px;z-index:6;background:var(--surface-glass)}.graph-tools button{display:grid;place-items:center;width:29px;height:28px}.graph-tools [aria-pressed=true]{color:var(--primary);background:var(--primary-soft)}.graph-tools select{max-width:160px;background:var(--surface)}.runtime-node{width:210px}.runtime-node :deep(input),.runtime-node :deep(select){pointer-events:none}.node-caption{padding:8px 3px;font-size:10px;color:var(--muted-foreground)}
@media(prefers-reduced-motion:reduce){:deep(*){animation:none!important;transition:none!important}}
</style>
