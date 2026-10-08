<script setup lang="ts">
import type { OversightDiagnostic } from '@/core/oversight'
defineProps<{ detail?: OversightDiagnostic | null; legacy?: boolean }>()
const bounded = (text: string | null | undefined, size = 320) => (text ?? '').slice(0, size)
const label = (text: string) => text === 'unknown_legacy' ? 'Unknown legacy' : bounded(text, 64).replaceAll('_', ' ')
</script>

<template>
  <aside v-if="detail || legacy" class="diagnostic" aria-label="Failure diagnostic">
    <template v-if="detail">
      <strong>{{ label(detail.category) }}</strong> · {{ label(detail.stage) }}
      <span v-if="detail.http_status"> · HTTP {{ detail.http_status }}</span>
      <p>{{ bounded(detail.message) }}</p>
      <details><summary>Diagnostic references</summary>
        <p>Run: {{ bounded(detail.run_id, 96) }} · Review: {{ bounded(detail.review_id, 96) }}</p>
        <p v-if="detail.proposal_id">Proposal: {{ bounded(detail.proposal_id, 96) }}</p>
        <p v-if="detail.call_id">Call: {{ bounded(detail.call_id, 96) }}</p>
      </details>
    </template>
    <template v-else>Unknown legacy: no failure category was recorded.</template>
  </aside>
</template>

<style scoped>
.diagnostic{margin:8px 0;padding:8px 10px;border-left:2px solid var(--border);font-size:12px;overflow-wrap:anywhere;max-height:240px;overflow:auto}p{margin:4px 0;white-space:pre-wrap}strong{color:var(--foreground)}details{font-size:11px}
</style>
