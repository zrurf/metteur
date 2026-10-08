<script setup lang="ts">
import { onBeforeUnmount, ref, watch } from 'vue'
import { gateway } from '@/core'
import type { OversightReview, OversightReports, ProposalTarget, ProposalVersion } from '@/core/oversight'
import type { BoardEntry } from '@/core/blackboard'
import { useWorkspaceStore } from '@/stores/workspace'
import OversightDiagnostic from './OversightDiagnostic.vue'
import OversightProposal from './OversightProposal.vue'
const closing = ref<OversightReports['closing']>(null)
const props = defineProps<{ runId: string | null; connected: boolean }>()
const emit = defineEmits<{ inspect: [nodeId: string, scope: string]; target: [target: ProposalTarget]; version: [target: ProposalVersion] }>()
const workspace = useWorkspaceStore(), reports = ref<OversightReview[] | null>(null), error = ref(''), evidence = ref<BoardEntry | null>(null)
let generation = 0, disposed = false, timer: ReturnType<typeof setTimeout> | undefined
watch([() => workspace.active?.path, () => props.runId, () => props.connected], () => {
  const ticket = ++generation, ws = workspace.active?.path, run = props.runId
  clearTimeout(timer); closing.value = null; reports.value = null; error.value = ''; evidence.value = null
  if (!ws || !run || !props.connected) return
  async function refresh() {
    const result = await gateway.listOversightReports(ws!, run!)
    if (disposed || ticket !== generation) return
    if (result.ok && result.data.run_id === run) { closing.value = result.data.closing ?? null; reports.value = result.data.reports.filter(r => !r.run_id || r.run_id === run); error.value = '' } else { closing.value = null; reports.value = null; error.value = result.ok ? 'Report identity does not match this run.' : result.error }
    timer = setTimeout(refresh, 1500)
  }
  void refresh()
}, { immediate: true })
onBeforeUnmount(() => { disposed = true; generation++; clearTimeout(timer) })
async function lookup(entry: string) {
  const ws = workspace.active?.path, run = props.runId, ticket = generation
  if (!ws || !run || !props.connected) return
  const result = await gateway.getBlackboard(ws, run, { entry_id: entry })
  if (disposed || ticket !== generation) return
  if (result.ok) evidence.value = result.data.entries[0] ?? null
  else error.value = result.error
}
</script>
<template>
  <section class="reviews" aria-label="Supervisor reviews">
    <p v-if="!runId">Select a recorded run.</p><p v-else-if="!connected">Reviews unavailable while disconnected.</p>
    <p v-else-if="error" role="alert">{{ error }}</p><p v-else-if="!reports">Loading reviews…</p><p v-else-if="!reports.length">No supervisor reviews recorded.</p>
    <aside v-if="closing" class="review"><strong>Cancellation requested</strong><p>Recorded checkpoint: {{ closing.checkpoint_status }}</p><p v-if="closing.recovery_required">Final cancellation outcome is unavailable; this run cannot resume.</p><p v-if="closing.checkpoint_error">{{ closing.checkpoint_error }}</p></aside>
    <article v-for="report in reports" :key="report.review_id" class="review">
      <header><strong>{{ report.status.replaceAll('_', ' ') }}</strong><span v-if="report.status === 'completed' && report.verdict">{{ report.verdict.replaceAll('_', ' ') }}</span></header>
      <p v-if="report.circuit_node">Circuit node: {{ report.circuit_node }}</p>
      <p v-if="report.model_verdict && report.verdict !== report.model_verdict">Model conclusion: {{ report.model_verdict }}</p>
      <p v-for="(disposition, i) in report.human_dispositions" :key="`d${i}`">Human disposition: {{ disposition.action.replaceAll('_', ' ') }}</p>
      <p>{{ report.summary }}</p><small>Triggers: {{ report.triggers.join(', ') }} · Model: {{ report.work.model || 'Unavailable' }}</small>
      <OversightDiagnostic :detail="report.diagnostic" :legacy="['failed', 'timed_out', 'budget_exhausted', 'cancelled'].includes(report.status)" />
      <p v-for="(note, i) in report.work.notes" :key="i">Model opinion: {{ note }}</p>
      <p v-for="(answer, i) in report.work.answers" :key="`a${i}`">Response: {{ answer }}</p>
      <p v-if="!report.usage?.length">No provider call recorded.</p>
      <p v-for="call in report.usage" :key="call.id">{{ call.model }} · {{ call.charged.toLocaleString() }} {{ call.state === 'reported' ? 'tokens' : 'reserved tokens · usage unknown' }}<span v-if="call.accounting_version !== 1"> · Historical budget accounting · cost unavailable</span><span v-else-if="call.cost_micros != null"> · {{ call.cost_micros / 1000000 }} {{ call.currency }}</span><span v-else> · Cost unavailable</span></p>
      <OversightProposal v-for="proposal in report.proposals" :key="proposal.proposal_id" :proposal="proposal" :run-id="runId" @target="emit('target', $event)" @version="emit('version', $event)" />
      <div v-if="report.cancel_result"><strong>Cancellation outcome</strong><p>{{ report.cancel_result.rollback_requested ? report.cancel_result.error ? 'File rollback incomplete' : 'Recorded file rollback completed' : 'File rollback disabled' }}<span v-if="report.cancel_result.restored_operations !== null"> · {{ report.cancel_result.restored_operations }} operation(s) restored</span></p><p v-if="report.cancel_result.error">{{ report.cancel_result.error }}</p><p v-for="(file, i) in report.cancel_result.files" :key="i">{{ file.path }} · {{ file.phase }}</p><small>Approved blueprint changes and shell/network effects are retained.</small></div>
      <details><summary>Evidence and request lineage</summary><code>review:{{ report.review_id }}</code><p v-for="id in report.source_request_ids" :key="id">Request: {{ id }}</p>
        <div v-for="item in report.work.evidence" :key="item.entry_id"><button @click="lookup(item.entry_id)">Read evidence · {{ item.entry_id }}</button><button v-if="item.node_id && item.scope" @click="emit('inspect', item.node_id, item.scope)">Inspect node</button></div>
        <p v-for="reference in report.actual_action_refs" :key="reference">{{ reference }}</p>
      </details>
    </article>
    <aside v-if="evidence" class="review"><strong>{{ evidence.event }} · {{ evidence.validity }}</strong><p>{{ evidence.note }}</p><pre>{{ evidence.digest }}</pre><button @click="evidence = null">Close evidence</button></aside>
  </section>
</template>
<style scoped>
.reviews{padding:16px;overflow:auto;flex:1;font-size:13px;color:var(--muted-foreground);line-height:1.6;overflow-wrap:anywhere}.review{padding:14px;margin-bottom:12px;border:1px solid var(--border);border-radius:10px;background:linear-gradient(125deg,var(--primary-soft),transparent 75%)}header{display:flex;gap:12px;color:var(--foreground)}p{margin:8px 0;white-space:pre-wrap}small,code{font-size:11px}button{color:var(--primary);padding:4px}pre{white-space:pre-wrap;max-height:200px;overflow:auto}[role=alert]{color:var(--danger)}
</style>
