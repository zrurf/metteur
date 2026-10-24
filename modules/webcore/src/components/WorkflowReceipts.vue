<script setup lang="ts">
import { ref, watch } from 'vue'
import { gateway } from '@/core'
import { useWorkspaceStore } from '@/stores/workspace'
import {
  workflowQuery,
  checkWorkflowReceipt,
  localWorkflowReceipts,
  type WorkflowReceipt,
} from '@/core/workflow-receipts'
const workspace = useWorkspaceStore(),
  rows = ref<WorkflowReceipt[]>([]),
  error = ref(''),
  loading = ref(false),
  next = ref<string | null>(null)
let generation = 0
watch(
  () => workspace.active?.path,
  () => {
    generation++
    loading.value = false
    rows.value = []
    error.value = ''
    next.value = null
  },
)
async function load(after: string | null = null) {
  const ws = workspace.active?.path,
    ticket = generation
  if (!ws || loading.value) return
  loading.value = true
  try {
    const result = await workflowQuery(gateway, ws, { operation: 'list', after })
    if (ticket !== generation || ws !== workspace.active?.path) return
    if (!result.ok) {
      error.value = result.error
      return
    }
    const data = result.data as { items: WorkflowReceipt[]; next: string | null }
    const all = new Map((after ? rows.value : []).map((r) => [r.id, r]))
    for (const row of data.items) all.set(row.id, row)
    for (const { workspace: source, receipt } of localWorkflowReceipts.values())
      if (source === ws && !all.has(receipt.id)) all.set(receipt.id, receipt)
    rows.value = [...all.values()]
    next.value = data.next
    error.value = ''
  } finally {
    if (ticket === generation) loading.value = false
  }
}
async function check(row: WorkflowReceipt) {
  const ws = workspace.active?.path,
    ticket = generation
  if (!ws) return
  const result = await checkWorkflowReceipt(gateway, ws, row)
  if (ticket !== generation || ws !== workspace.active?.path) return
  if (result.ok) rows.value = rows.value.map((r) => (r.id === row.id ? result.data : r))
  else error.value = result.error
}
const label = (row: WorkflowReceipt) =>
  row.operation === 'temporary_input'
    ? 'Temporary input'
    : row.operation === 'stop'
      ? 'Stop request'
      : row.operation === 'pause'
        ? 'Pause request'
        : 'Resume request'
const phase = (row: WorkflowReceipt) =>
  ({
    prepared: 'Not submitted · check before continuing',
    claimed: 'Outcome unconfirmed',
    accepted: 'Request received',
    queued: 'Received · waiting for a model boundary',
    applied: 'Consumed · node committed',
    unconsumed: 'Run ended · input was not consumed',
    unknown: 'Outcome unconfirmed · do not resubmit blindly',
    retired: 'Closed · not applied',
  })[row.phase]
</script>
<template>
  <details
    v-if="!gateway.demo"
    class="workflow-receipts"
    @toggle="($event.target as HTMLDetailsElement).open && load()"
  >
    <summary>Control &amp; Input Receipts</summary>
    <p>
      Receipt checks never repeat an action. Execution status shows when a received control takes
      effect.
    </p>
    <button class="btn" :disabled="loading" @click="load()">Refresh Receipts</button>
    <p v-if="error" role="alert">{{ error }}</p>
    <p v-if="!loading && !rows.length && !error">No receipts in this workspace.</p>
    <article v-for="row in rows" :key="row.id">
      <strong>{{ label(row) }}</strong>
      <p>{{ phase(row) }}</p>
      <button
        v-if="['prepared', 'claimed', 'unknown', 'queued'].includes(row.phase)"
        class="btn"
        @click="check(row)"
      >
        Check Original Receipt
      </button>
      <details>
        <summary>Source &amp; Content</summary>
        <p>{{ row.target }}</p>
        <code>{{ row.id }}</code>
        <pre v-if="row.body">{{ row.body }}</pre>
      </details>
    </article>
    <button v-if="next" class="btn" :disabled="loading" @click="load(next)">More Receipts</button>
  </details>
</template>
<style scoped>
.workflow-receipts {
  font-size: 13px;
  line-height: 1.6;
  padding: 8px 12px;
  overflow-wrap: anywhere;
}
.workflow-receipts > summary {
  cursor: pointer;
  color: var(--muted-foreground);
}
.workflow-receipts article {
  padding: 10px 0;
  border-bottom: 1px solid var(--divider);
}
.workflow-receipts pre {
  white-space: pre-wrap;
  max-height: 18rem;
  overflow: auto;
}
.workflow-receipts p {
  margin: 6px 0;
}
.workflow-receipts code {
  font-size: 13px;
}
</style>
