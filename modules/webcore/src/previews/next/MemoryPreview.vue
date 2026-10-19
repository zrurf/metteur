<script setup lang="ts">
import { computed, ref } from 'vue'
import { vDialogFocus } from './dialog-focus'

const props = defineProps<{ scenario: string }>()
const search = ref('')
const kind = ref('All entries')
const selected = ref('note-1')
const notice = ref('')
const dialog = ref(false)
const editing = ref(false)
const draft = ref('')
const entries = ref([
  { id: 'note-1', title: 'Keep parser errors actionable', kind: 'Project note', origin: 'Model opinion', validity: 'Current', revision: 3, digest: '8a91c2d4', body: 'Include the source line and a short recovery hint when parsing fails. This is a proposed convention, not an accepted rule.', source: 'Chat · Parser review', file: 'src/parser.rs · 4b910c', evidence: false },
  { id: 'rule-1', title: 'Use explicit cancellation boundaries', kind: 'Convention', origin: 'User confirmed', validity: 'Current', revision: 2, digest: '1c553d90', body: 'Check cancellation before acquiring a write lease. Explain cancellation results without promising that an external side effect was undone.', source: 'Confirmed by local user · revision 2', file: '', evidence: false },
  { id: 'check-1', title: 'Parser contract check', kind: 'Evidence', origin: 'Deterministic check', validity: 'Needs review', revision: 1, digest: '693f1b2a', body: 'The check passed for file version 4b910c. The current file has changed; this result does not establish that the current version passes.', source: 'Run 42 · invocation 7 · attempt 1', file: 'src/parser.rs · 4b910c → 9d885a', evidence: true },
  { id: 'handoff-1', title: 'Parser handoff', kind: 'Handoff', origin: 'Host handoff / model summary', validity: 'Current', revision: 1, digest: 'aac630db', body: 'The implementation is ready for focused review. The summary remains a model opinion; linked run evidence retains its original provenance.', source: 'Session 12 · turn 4', file: '', evidence: true },
])
const rows = computed(() => props.scenario === 'Empty' ? [] : entries.value.filter(e => (kind.value === 'All entries' || e.kind === kind.value) && `${e.title} ${e.body}`.toLowerCase().includes(search.value.toLowerCase())))
const entry = computed(() => rows.value.find(e => e.id === selected.value) ?? rows.value[0])
const writable = computed(() => !['Read only', 'Disconnected', 'Permission denied', 'Loading', 'Error'].includes(props.scenario))
function promote() {
  dialog.value = false
  if (!entry.value) return
  if (props.scenario === 'Conflict') { notice.value = 'Revision conflict: expected 3, actual 4. Reload and review the new content before confirming.'; return }
  entry.value.kind = 'Convention'; entry.value.origin = 'User confirmed'
  notice.value = `Fixture confirmation saved for ${entry.value.id} · revision ${entry.value.revision} · ${entry.value.digest}. No tool permissions were granted.`
}
function save() {
  if (!entry.value) return
  if (props.scenario === 'Conflict') { notice.value = 'Revision conflict. Your draft is preserved; reload before applying it.'; return }
  entry.value.body = draft.value; entry.value.revision++; entry.value.digest = 'new-fixture-digest'
  entry.value.kind = 'Project note'; entry.value.origin = 'User note'; editing.value = false
  notice.value = 'New revision saved. Any previous convention confirmation no longer applies.'
}
</script>

<template>
  <section aria-label="Project memory preview">
    <header class="pv-heading"><div><p class="pv-eyebrow">PROJECT / PARSER WORKSPACE</p><h1>Project memory</h1><p class="pv-muted">Notes, conventions and evidence — with their original source and version.</p></div><span class="pv-badge">PV-01 · r1</span></header>
    <div class="pv-toolbar"><label class="pv-search">Search memory<input v-model="search" class="input" placeholder="Search notes and evidence" /></label><label>Type<select v-model="kind" class="input"><option>All entries</option><option>Project note</option><option>Convention</option><option>Evidence</option><option>Handoff</option></select></label></div>
    <p v-if="scenario === 'Loading'" role="status" class="pv-callout">Loading project memory…</p>
    <p v-else-if="scenario === 'Error'" role="alert" class="pv-callout">Memory is unavailable. Your draft is preserved. <button class="btn" @click="notice = 'Fixture retry requested.'">Retry</button></p>
    <template v-else>
      <p v-if="scenario === 'Disconnected'" class="pv-callout" role="status">Disconnected · Showing the last loaded snapshot. Editing and confirmation are disabled.</p>
      <p v-if="scenario === 'Read only' || scenario === 'Permission denied'" class="pv-callout">{{ scenario }} · You can inspect source and validity. Changes are unavailable.</p>
      <div class="pv-split">
        <aside aria-label="Memory entries" class="pv-list">
          <p class="pv-eyebrow">{{ rows.length }} ENTRIES</p>
          <button v-for="row in rows" :key="row.id" class="pv-row" :class="{ selected: entry?.id === row.id }" @click="selected = row.id; editing = false; notice = ''"><span class="pv-muted">{{ row.kind }} · r{{ row.revision }}</span><strong>{{ row.title }}</strong><span :class="row.validity === 'Needs review' ? 'pv-warn' : 'pv-muted'">{{ row.origin }} · {{ row.validity }}</span></button>
          <p v-if="!rows.length" class="pv-empty">No matching entries.<br />Try another filter or add a project note.</p>
        </aside>
        <article v-if="entry" class="pv-detail">
          <div class="pv-between"><span class="pv-badge">{{ entry.origin }}</span><span class="pv-muted">Revision {{ entry.revision }}</span></div>
          <h2>{{ entry.title }}</h2><p class="pv-callout" v-if="entry.validity === 'Needs review'">Needs review · Applies to an earlier file version.</p>
          <label v-if="editing">Note body<textarea v-model="draft" class="input pv-editor" /></label><p v-else class="pv-body">{{ entry.body }}</p>
          <dl class="pv-facts"><dt>Source</dt><dd><button class="pv-link" @click="notice = `Fixture source: ${entry.source}. Production navigation will open only the matching workspace and record.`">{{ entry.source }} ↗</button></dd><dt>Content digest</dt><dd><code>{{ entry.digest }}</code></dd><template v-if="entry.file"><dt>File version</dt><dd>{{ entry.file }}</dd></template><dt>Trust</dt><dd>{{ entry.evidence ? 'Host-owned reference. Not editable as a note.' : 'Current does not mean verified. A convention grants no capabilities.' }}</dd></dl>
          <div class="pv-actions" v-if="!entry.evidence">
            <button class="btn" v-if="!editing" :disabled="!writable" @click="draft = entry.body; editing = true">Edit note</button>
            <button class="btn btn-primary" v-if="editing" :disabled="!writable" @click="save">Save new revision</button>
            <button class="btn" v-if="editing" @click="editing = false">Cancel edit</button>
            <button class="btn btn-primary" v-if="entry.kind !== 'Convention' && !editing" :disabled="!writable" @click="dialog = true">Pin as convention</button>
            <button class="btn" :disabled="!writable" @click="entry.validity = 'Archived'; notice = 'Archived. Existing references retain their original identity and revision.'">Archive</button>
          </div>
        </article>
        <div v-else class="pv-empty">Select a memory entry to inspect its source.</div>
      </div>
    </template>
    <p v-if="notice" class="pv-callout" role="status">{{ notice }}</p>
    <div v-if="dialog && entry" v-dialog-focus class="pv-overlay" @keydown.esc="dialog = false"><section role="dialog" aria-modal="true" aria-labelledby="confirm-title" class="pv-modal"><h2 id="confirm-title">Pin this exact revision?</h2><p>{{ entry.title }}</p><p class="pv-body">{{ entry.body }}</p><code>{{ entry.id }} · r{{ entry.revision }} · {{ entry.digest }}</code><p class="pv-muted">This becomes a project convention. It grants no tool permissions. Future edits require a new confirmation.</p><div class="pv-actions"><button class="btn" autofocus @click="dialog = false; notice = 'Confirmation cancelled. No convention was created.'">Cancel</button><button class="btn btn-primary" @click="promote">Confirm convention</button></div></section></div>
  </section>
</template>
