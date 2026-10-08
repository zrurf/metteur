<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { gateway, type UsageSummary } from '@/core'
import { useWorkspaceStore } from '@/stores/workspace'
import { executionUsage } from '@/core/execution-usage'
const props = defineProps<{ runId: string | null; connected: boolean }>()
const workspace = useWorkspaceStore()
const summary = ref<UsageSummary | null>(null), error = ref(''), loading = ref(false)
let generation = 0, disposed = false, timer: ReturnType<typeof setTimeout> | undefined
watch([() => workspace.active?.path, () => props.runId, () => props.connected], () => {
  const ticket = ++generation, ws = workspace.active?.path, id = props.runId
  clearTimeout(timer); summary.value = null; error.value = ''; loading.value = false
  if (!ws || !id || !props.connected) return
  async function refresh() {
    loading.value = !summary.value
    try {
      const result = await gateway.getExecutionUsage(ws!, id!)
      if (disposed || ticket !== generation) return
      if (!result.ok) throw new Error(result.error)
      summary.value = result.data; error.value = ''
    } catch (e) {
      if (disposed || ticket !== generation) return
      summary.value = null; error.value = String(e)
    } finally {
      if (!disposed && ticket === generation) { loading.value = false; timer = setTimeout(refresh, 1000) }
    }
  }
  void refresh()
}, { immediate: true })
onBeforeUnmount(() => { disposed = true; generation++; clearTimeout(timer) })
const metrics = computed(() => executionUsage(summary.value))
const rate = computed(() => metrics.value.cacheHitRate === null ? null : metrics.value.cacheHitRate * 100)
const number = (value: number | null) => value === null ? 'Unavailable' : value.toLocaleString()
const cost = computed(() => metrics.value.estimatedCost === null ? 'Unavailable' : `${summary.value?.currency} ${metrics.value.estimatedCost.toFixed(6)}`)
const oversight = computed(() => summary.value?.oversight)
function callerUsage(caller: 'supervisor' | 'concierge') {
  const value = oversight.value
  if (!value) return 'Unavailable'
  const calls = value.calls.filter(c => c.caller === caller)
  if (!calls.length) return caller === 'concierge' && !value.concierge_available ? 'Not configured' : 'No calls'
  const tokens = calls.reduce((sum, c) => sum + c.charged, 0)
  return `${tokens.toLocaleString()} tokens${calls.some(c => c.state !== 'reported') ? ' (estimated / reserved)' : ''}`
}
function callerCost(caller: 'supervisor' | 'concierge') {
  const calls = oversight.value?.calls.filter(c => c.caller === caller)
  if (!calls?.length) return ''
  if (calls.some(c => c.accounting_version !== 1 || c.cost_micros === null || c.currency !== summary.value?.currency)) return 'Estimated cost unavailable'
  return `${summary.value?.currency} ${(calls.reduce((sum, c) => sum + (c.cost_micros ?? 0), 0) / 1e6).toFixed(6)}`
}
</script>
<template>
  <div class="usage-panel">
    <p v-if="loading" role="status">Loading usage…</p>
    <p v-if="error" role="status">Usage unavailable. {{ error }}</p>
    <div class="usage-grid">
      <section><span>Input tokens</span><strong>{{ number(metrics.input) }}</strong></section>
      <section><span>Output tokens</span><strong>{{ number(metrics.output) }}</strong></section>
      <section><span>Estimated cost</span><strong>{{ cost }}</strong></section>
      <section class="cache-card"><span>Cache hit rate</span><strong>{{ rate === null ? 'Unavailable' : `${Number(rate.toFixed(1))}%` }}</strong><div class="meter" role="progressbar" aria-label="Cache hit rate" :aria-valuenow="rate ?? undefined" :aria-valuetext="rate === null ? 'Unavailable' : undefined" :aria-valuemin="0" :aria-valuemax="100"><i :style="{ width: `${rate ?? 0}%` }" /></div></section>
      <section><span>Oversight token budget</span><strong>{{ oversight ? `${oversight.charged.toLocaleString()} / ${oversight.limit.toLocaleString()}` : 'Unavailable' }}</strong><span v-if="oversight?.exhausted">Exhausted</span><span v-else-if="oversight?.warning">Budget warning</span></section>
      <p v-if="oversight?.calls.some(c => c.accounting_version !== 1)">Historical accounting: original budget charges are retained; legacy costs may include reasoning twice.</p>
      <section v-for="caller in (['supervisor', 'concierge'] as const)" :key="caller"><span>{{ caller === 'supervisor' ? 'Supervisor' : 'Concierge' }}</span><strong>{{ callerUsage(caller) }}</strong><span>{{ callerCost(caller) }}</span></section>
    </div>
  </div>
</template>
<style scoped>
.usage-panel{padding:18px;color:var(--muted-foreground)}.usage-panel>p{margin:0 0 12px}.usage-grid{display:grid;grid-template-columns:repeat(3,minmax(150px,1fr));gap:14px}.usage-grid section{padding:14px;border:1px solid var(--border);border-radius:9px;background:var(--surface)}.usage-grid span{display:block;font-size:11px;margin-bottom:8px}.usage-grid strong{font-size:16px;font-weight:600;color:var(--foreground)}.meter{height:5px;border-radius:4px;background:var(--hover);overflow:hidden;margin-top:12px}.meter i{display:block;height:100%;background:var(--primary);border-radius:4px}@media(max-width:900px){.usage-grid{grid-template-columns:repeat(2,minmax(120px,1fr))}}
</style>
