<script setup lang="ts">
import { computed } from 'vue'
import type { OversightProposal, ProposalTarget, ProposalVersion } from '@/core/oversight'
import OversightDiagnostic from './OversightDiagnostic.vue'
const props = defineProps<{ proposal: OversightProposal; runId: string | null }>()
const emit = defineEmits<{ target: [target: ProposalTarget]; version: [target: ProposalVersion] }>()
const bound = computed(() => !!props.runId && props.proposal.run_id === props.runId)
const base = computed(() => props.proposal.binding?.base ?? props.proposal.binding?.expected?.base)
const labels: Record<string, string> = { awaiting_confirmation: 'Awaiting your confirmation', approved_pending_apply: 'Confirmed · not applied', applied: 'Applied', rejected: 'Rejected', failed: 'Failed', closed_unhandled: 'Closed · unhandled' }
const kinds: Record<string, string> = { blueprint_edits: 'Blueprint changes', PauseRun: 'Pause run', CancelRun: 'Cancel run' }
const status = computed(() => props.proposal.state === 'approved_pending_apply' && props.proposal.result_version
  ? 'Committed version · application acknowledgement pending'
  : labels[props.proposal.state] ?? props.proposal.state.replaceAll('_', ' '))
const changes = computed(() => {
  const before = props.proposal.binding?.before ?? [], after = props.proposal.binding?.after ?? []
  const rows: Array<{ node: string; field: string; before: string; after: string }> = []
  const show = (v: unknown) => v === undefined ? 'Not recorded' : JSON.stringify(v, null, 2)
  function diff(node: string, field: string, a: unknown, b: unknown) {
    if (JSON.stringify(a) === JSON.stringify(b)) return
    if (Array.isArray(a) && Array.isArray(b)) {
      for (let index = 0; index < Math.max(a.length, b.length); index++) {
        const item = b[index] ?? a[index], label = item && typeof item === 'object' ? item.key ?? item.name ?? index : index
        diff(node, `${field}[${label}]`, a[index], b[index])
      }
    } else if (a && b && typeof a === 'object' && typeof b === 'object' && !Array.isArray(a) && !Array.isArray(b)) {
      const left = a as Record<string, unknown>, right = b as Record<string, unknown>
      for (const key of new Set([...Object.keys(left), ...Object.keys(right)])) diff(node, field ? `${field}.${key}` : key, left[key], right[key])
    } else rows.push({ node, field, before: show(a), after: show(b) })
  }
  for (const id of new Set([...before.map(n => n.id), ...after.map(n => n.id)])) {
    const a = before.find(n => n.id === id), b = after.find(n => n.id === id)
    diff(id, '', a, b)
  }
  return rows
})
</script>
<template>
  <article class="proposal-card" :aria-label="`Proposal ${proposal.proposal_id}`">
    <header><strong>{{ kinds[proposal.kind ?? ''] ?? proposal.kind ?? 'Recorded proposal' }}</strong><span>{{ status }}</span></header>
    <p>{{ proposal.binding?.summary || proposal.reason }}</p>
    <p>Source: {{ proposal.source_request_ids?.length ? 'Concierge request → supervisor' : proposal.binding?.source === 'supervisor' ? 'Supervisor' : 'Recorded source unavailable' }}</p>
    <blockquote v-for="request in proposal.original_requests" :key="request.request_id"><small>{{ request.source }} · {{ request.request_id }}</small><p>{{ request.original_text }}</p></blockquote>
    <p v-if="proposal.source_request_ids?.length && !proposal.original_requests?.length">Original request details unavailable.</p>
    <p v-if="proposal.decision_source">Decision: {{ proposal.decision_source === 'delegated' ? 'existing user delegation' : proposal.decision_source === 'human' ? 'human confirmation' : 'unavailable' }}</p>
    <p v-else>{{ proposal.state === 'awaiting_confirmation' ? 'Decision pending' : 'Decision unavailable' }}</p>
    <p v-if="proposal.state === 'awaiting_confirmation'">Confirm or reject this specific action in the independent approval dialog. Viewing this card grants no permission.</p>
    <p v-if="proposal.reason && proposal.binding?.summary">{{ proposal.reason }}</p>
    <nav v-if="proposal.binding?.affected_nodes?.length" aria-label="Affected nodes">
      <button v-for="id in proposal.binding.affected_nodes" :key="id" :disabled="!bound || !proposal.binding.scope" @click="emit('target', { proposal, nodeId: id })">Inspect node · {{ id }}</button>
    </nav>
    <div v-if="changes.length" class="proposal-diff" tabindex="0" aria-label="Proposed field changes"><table><thead><tr><th>Node / field</th><th>Before</th><th>After</th></tr></thead><tbody><tr v-for="(change, index) in changes" :key="index"><th>{{ change.node }}<br>{{ change.field }}</th><td><pre>{{ change.before }}</pre></td><td><pre>{{ change.after }}</pre></td></tr></tbody></table></div>
    <p v-else-if="proposal.kind === 'blueprint_edits'">Recorded field changes unavailable.</p>
    <details v-if="proposal.binding?.expected"><summary>Control scope</summary><pre class="control-scope">{{ JSON.stringify(proposal.binding.expected, null, 2) }}</pre><p>{{ proposal.binding.rollback_notice }}</p></details>
    <nav aria-label="Proposal versions">
      <button v-if="base" :disabled="!bound" @click="emit('version', { proposal, version: base, label: 'Base version' })">Base version · {{ base.blob_hash.slice(0, 12) }}</button><span v-else>Base version unavailable</span>
      <button v-if="proposal.result_version" :disabled="!bound" @click="emit('version', { proposal, version: proposal.result_version, label: 'Result version' })">Result version · {{ proposal.result_version.blob_hash.slice(0, 12) }}</button>
      <span v-else>{{ proposal.state === 'applied' ? proposal.kind === 'blueprint_edits' ? 'Result version unavailable' : 'Control action · no blueprint version created' : 'No committed result version recorded' }}</span>
    </nav>
    <OversightDiagnostic :detail="proposal.diagnostic" :legacy="['failed', 'rejected', 'closed_unhandled'].includes(proposal.state)" />
    <details><summary>Proposal references</summary><p>Proposal: {{ proposal.proposal_id }}</p><p>Review: {{ proposal.review_id || 'Unavailable' }}</p><p>Run: {{ proposal.run_id || 'Unavailable' }}</p><p v-for="reference in proposal.result_refs" :key="reference">{{ reference }}</p></details>
  </article>
</template>
<style scoped>
.proposal-card{margin:12px 0;padding:12px;border:1px solid var(--border);border-radius:8px;background:linear-gradient(125deg,var(--primary-soft),transparent 85%);font-size:13px;line-height:1.6;overflow-wrap:anywhere;color:var(--foreground);min-width:0}header,nav{display:flex;flex-wrap:wrap;gap:8px;align-items:center}header span,small,details,nav span{color:var(--muted-foreground);font-size:11px}p{margin:8px 0;white-space:pre-wrap}blockquote{border-left:2px solid var(--primary);margin:8px 0;padding-left:10px;max-height:180px;overflow:auto}button{color:var(--primary);text-align:left;padding:4px 0}button:disabled{opacity:.5;cursor:default}nav{margin:10px 0}.proposal-diff{max-height:300px;overflow:auto;border:1px solid var(--border);border-radius:6px}table{border-collapse:collapse;width:100%;font-size:11px}th,td{padding:8px;vertical-align:top;text-align:left;border-bottom:1px solid var(--border);min-width:100px}th{font-weight:500}pre{white-space:pre-wrap;overflow-wrap:anywhere;margin:0;min-width:100px;max-width:400px}.control-scope{max-height:240px;overflow:auto}details{margin-top:8px}
</style>
