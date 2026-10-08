<script setup lang="ts">
import { onBeforeUnmount, ref, watch } from 'vue'
import { gateway } from '@/core'
import type { Blackboard, BoardQuery, BoardValidity } from '@/core/blackboard'
import { useWorkspaceStore } from '@/stores/workspace'
import { useVersionStore } from '@/stores/version'
import { useSurfaceNavigation } from '@/lib/surface'
const props = defineProps<{ runId: string | null; connected: boolean; revision?: number }>()
const workspace = useWorkspaceStore(), versions = useVersionStore()
const { openSurface } = useSurfaceNavigation()
const board = ref<Blackboard | null>(null), error = ref(''), loading = ref(false)
const status = ref<BoardValidity | ''>(''), origin = ref<'Engine' | 'Node' | ''>(''), keyword = ref(''), entryId = ref('')
const query = ref<BoardQuery>({ last_n: 100 })
let generation = 0, disposed = false
function search() { query.value = entryId.value.trim() ? { entry_id: entryId.value.trim() } : { last_n: 100, status: status.value || undefined, origins: origin.value ? [origin.value] : [], keyword: keyword.value } }
function lookup(reference: string) { entryId.value = reference; query.value = { entry_id: entryId.value } }
function recent() { entryId.value = ''; status.value = ''; origin.value = ''; keyword.value = ''; query.value = { last_n: 100 } }
watch([() => workspace.active?.path, () => props.runId], recent, { flush: 'sync' })
watch([() => workspace.active?.path, () => props.runId, () => props.connected, () => props.revision, query], async () => {
  const ticket = ++generation, ws = workspace.active?.path, id = props.runId
  board.value = null; error.value = ''; loading.value = false
  if (!ws || !id || !props.connected) return
  loading.value = true
  try {
    const result = await gateway.getBlackboard(ws, id, query.value)
    if (disposed || ticket !== generation) return
    if (!result.ok) throw new Error(result.error)
    if (result.data.run_id !== id) throw new Error('Blackboard belongs to another run')
    board.value = result.data
  } catch (e) { if (!disposed && ticket === generation) error.value = String(e) }
  finally { if (!disposed && ticket === generation) loading.value = false }
}, { immediate: true })
onBeforeUnmount(() => { disposed = true; generation++ })
function version(id: string) { versions.select(id); openSurface('version') }
</script>
<template>
  <section class="blackboard-panel" aria-label="Run blackboard">
    <p v-if="!runId">Select a recorded blueprint run to view its blackboard.</p>
    <p v-else-if="!connected" role="status">Blackboard unavailable while disconnected.</p>
    <template v-else>
      <form class="board-filter" @submit.prevent="search">
        <select v-model="status" aria-label="Blackboard validity"><option value="">All validity</option><option>Current</option><option>Invalidated</option><option>Unverified</option></select>
        <select v-model="origin" aria-label="Blackboard origin"><option value="">All origins</option><option>Engine</option><option>Node</option></select>
        <input v-model="keyword" aria-label="Blackboard keyword" placeholder="Search summaries" />
        <input v-model="entryId" aria-label="Blackboard entry ID" placeholder="Entry ID" />
        <button type="submit">Search</button><button type="button" @click="recent">Recent</button>
      </form>
      <p v-if="loading" role="status">Loading blackboard…</p>
      <p v-if="error" role="status">Blackboard unavailable. {{ error }}</p>
      <template v-if="board">
        <article v-for="review in board.reviews" :key="review.review_id" class="board-entry"><header><strong>Supervisor review · {{ review.status }}</strong><span>Supervisor · Model opinion</span></header><p>{{ review.summary }}</p><p v-for="(note, i) in review.notes" :key="i">{{ note }}</p><code>review:{{ review.review_id }}</code><p v-for="reference in review.actual_action_refs" :key="reference">{{ reference }}</p></article>
        <p v-if="!board.available">Projection unavailable for this legacy record.</p>
        <template v-else>
          <table class="board-totals"><caption>Run {{ board.run_id }}</caption><thead><tr><th>Scope</th><th>Completed</th><th>Failed</th><th>Checks passed</th><th>Checks failed</th><th>Node time</th><th>Reported tokens</th></tr></thead><tbody><tr v-for="scope in (['current', 'historical'] as const)" :key="scope"><th>{{ scope === 'current' ? 'Current valid' : 'Historical' }}</th><td>{{ board[scope].completed }}</td><td>{{ board[scope].failed }}</td><td>{{ board[scope].passed_checks }}</td><td>{{ board[scope].failed_checks }}</td><td>{{ (board[scope].duration_ms / 1000).toFixed(1) }}s</td><td>{{ board[scope].reported_tokens.toLocaleString() }}{{ board[scope].tokens_complete ? '' : ' · partial' }}</td></tr></tbody></table>
          <p v-if="!board.entries.length">No matching entries.</p>
          <p v-if="board.truncated">Showing {{ board.entries.length }} of {{ board.matched_entries }} matching entries. Older evidence remains available by entry ID.</p>
          <article v-for="entry in board.entries" :key="entry.id" class="board-entry" :class="{ invalidated: entry.validity === 'Invalidated' }">
            <header><strong>{{ entry.event }}</strong><span class="board-badge">{{ entry.validity }}</span><span>{{ entry.origin }} · {{ entry.evidence_kind }}</span><code>{{ entry.id }}</code></header>
            <p>{{ entry.note }}</p><p v-if="entry.node_id">{{ entry.node_id }} · Attempt {{ entry.attempt }} · {{ entry.frame.join(' / ') || 'Root' }}</p>
            <pre v-if="entry.digest">{{ entry.digest }}</pre>
            <button v-if="entry.version" @click="version(entry.version.snapshot_id)">Version Flow · {{ entry.version.blob_hash.slice(0, 12) }}</button>
            <div v-if="entry.invalidates.length">Invalidates: <button v-for="reference in entry.invalidates" :key="reference" @click="lookup(reference)">{{ reference }}</button></div>
            <div class="board-evidence" v-if="entry.evidence_refs.length">Evidence: <span v-for="reference in entry.evidence_refs" :key="reference">{{ reference }} <button v-if="entry.evidence_kind === 'DeterministicCheck'" @click="lookup(reference)">Lookup evidence</button></span></div>
          </article>
        </template>
      </template>
    </template>
  </section>
</template>
<style scoped>
.blackboard-panel{padding:16px;color:var(--muted-foreground);font-size:12px;overflow-wrap:anywhere;min-width:0}.board-filter{display:flex;gap:6px;flex-wrap:wrap;margin-bottom:12px}.board-filter input,.board-filter select,.board-filter button{border:1px solid var(--border);border-radius:5px;background:var(--surface);padding:5px 7px;min-width:0}.board-filter input{width:150px}.board-totals{font:inherit;width:100%;text-align:left;margin:10px 0 16px}.board-totals caption{text-align:left;padding:4px 0}.board-totals th,.board-totals td{padding:5px 8px;border-bottom:1px solid var(--border)}.board-entry{border-top:1px solid var(--border);padding:12px 0}.board-entry header{display:flex;align-items:center;gap:10px;flex-wrap:wrap}.board-entry strong{color:var(--foreground)}.board-entry p{margin:6px 0}.board-entry code{font-size:10px}.board-entry pre{white-space:pre-wrap;max-height:160px;overflow:auto;background:var(--hover);border-radius:6px;padding:8px}.board-badge{border:1px solid var(--border);padding:2px 5px;border-radius:4px}.invalidated .board-badge{color:var(--danger)}.board-entry button{color:var(--primary);padding:3px 5px}.board-evidence{font-size:11px;margin-top:6px}
</style>
