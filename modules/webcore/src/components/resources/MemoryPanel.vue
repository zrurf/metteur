<script setup lang="ts">
import MoreActions from './MoreActions.vue'
import PageHeader from '@/components/PageHeader.vue'
import InlineHelp from '@/components/resources/InlineHelp.vue'
import { computed, ref, watch } from 'vue'
import { vDialogFocus } from './dialog-focus'
import { gateway } from '@/core'
import { useWorkspaceStore } from '@/stores/workspace'
import type { MemoryEntry, MemoryPage } from '@/core/resources'
import './resources.css'
const workspace = useWorkspaceStore()
const search = ref(''),
  kind = ref('All entries'),
  selected = ref(''),
  notice = ref('')
const dialog = ref(false),
  editing = ref(false),
  draft = ref(''),
  busy = ref(false),
  failed = ref(false)
const mutating = ref(false),
  sourceEvidence = ref('')
const raw = ref<MemoryEntry[]>([]),
  next = ref<string | null>(null)
const evidence = ref<(MemoryEntry & { evidenceLabel: string })[]>([])
const draftEntry = ref<MemoryEntry | null>(null)
let generation = 0,
  editGeneration = 0,
  sourceGeneration = 0,
  mutationGeneration = 0
const origins: Record<string, string> = {
  model_opinion: 'Model opinion',
  user_note: 'User note',
  user_convention: 'User confirmed',
  handoff: 'Host handoff / model summary',
  execution_evidence: 'Deterministic check',
}
const kinds: Record<string, string> = {
  user_convention: 'Convention',
  handoff: 'Handoff',
  execution_evidence: 'Evidence',
}
function displayEntry(item: MemoryEntry & { evidenceLabel?: string }) {
  return {
    id: item.reference.id,
    title: item.content.title,
    kind: kinds[item.content.origin] ?? 'Project note',
    origin: item.evidenceLabel ?? origins[item.content.origin] ?? item.content.origin,
    validity: item.content.validity === 'needs_review' ? 'Needs review' : item.content.validity,
    revision: item.reference.revision,
    digest: item.reference.content_digest,
    body: item.content.body,
    source: item.content.source ?? `Author: ${item.content.author}`,
    file: item.content.files.map((f) => `${f.path} · ${f.digest}`).join(', '),
    evidence: ['handoff', 'execution_evidence'].includes(item.content.origin),
    raw: item,
  }
}
const entries = computed(() => [...raw.value, ...evidence.value].map(displayEntry))
const rows = computed(() =>
  entries.value.filter(
    (e) =>
      (kind.value === 'All entries' || e.kind === kind.value) &&
      `${e.title} ${e.body}`.toLowerCase().includes(search.value.toLowerCase()),
  ),
)
const entry = computed(() =>
  editing.value && draftEntry.value
    ? displayEntry(draftEntry.value)
    : (rows.value.find((e) => e.id === selected.value) ?? rows.value[0]),
)
const scenario = computed(() =>
  !gateway.connected.value
    ? 'Disconnected'
    : busy.value
      ? 'Loading'
      : failed.value
        ? 'Error'
        : 'Ready',
)
const writable = computed(
  () => gateway.connected.value && !busy.value && !failed.value && !mutating.value,
)
async function load(append = false) {
  const path = workspace.active?.path
  if (!path) return
  const ticket = ++generation
  busy.value = true
  const result = await gateway.resourceRequest('queryProjectMemory', path, {
    operation: 'query',
    query: { limit: 50, after: append ? next.value : null },
  })
  if (ticket !== generation || workspace.active?.path !== path) return
  busy.value = false
  failed.value = !result.ok
  if (!result.ok) {
    notice.value = result.error
    return
  }
  const page = result.data as MemoryPage
  raw.value = append ? [...raw.value, ...page.entries] : page.entries
  next.value = page.next
  if (!append) {
    const runs = await gateway.listExecutions(path)
    const records: typeof evidence.value = []
    if (runs.ok)
      for (const run of runs.data.slice(-3)) {
        const projection = await gateway.resourceRequest('queryProjectMemory', path, {
          operation: 'evidence',
          run_id: run.runId,
          entry_id: null,
        })
        if (projection.ok) {
          const data = projection.data as {
            entries: {
              id: string
              event: string
              note: string
              digest?: string
              evidence_kind: string
              validity: string
              attempt?: number
              version?: unknown
            }[]
          }
          for (const item of data.entries)
            records.push({
              reference: {
                scope: { scope: 'project', owner: path },
                id: `evidence:${run.runId}:${item.id}`,
                revision: item.attempt ?? 1,
                content_digest: item.digest ?? 'Unavailable',
              },
              content: {
                title: item.event,
                body: item.note,
                origin: 'execution_evidence',
                validity:
                  item.validity === 'Invalidated' ? 'needs_review' : item.validity.toLowerCase(),
                author: 'host',
                source: `run:${run.runId}`,
                files: [],
                confirmation: null,
              },
              evidenceLabel: item.evidence_kind,
            })
        }
      }
    if (ticket === generation && workspace.active?.path === path) evidence.value = records
  }
}
watch(
  () => workspace.active?.path,
  () => {
    generation++
    editGeneration++
    sourceGeneration++
    mutationGeneration++
    raw.value = []
    evidence.value = []
    draftEntry.value = null
    editing.value = false
    dialog.value = false
    sourceEvidence.value = ''
    notice.value = ''
    mutating.value = false
    busy.value = false
    void load()
  },
  { immediate: true, flush: 'sync' },
)
watch(
  () => entry.value?.id,
  () => {
    sourceGeneration++
    sourceEvidence.value = ''
    dialog.value = false
  },
  { flush: 'sync' },
)
function selectEntry(id: string) {
  editGeneration++
  selected.value = id
  editing.value = false
  draftEntry.value = null
  notice.value = ''
  sourceGeneration++
  sourceEvidence.value = ''
}
function cancelEdit() {
  editGeneration++
  editing.value = false
}
function cancelConfirmation() {
  dialog.value = false
  notice.value = 'Confirmation cancelled. No convention was created.'
}
function editNote() {
  if (!entry.value || !writable.value) return
  editGeneration++
  draftEntry.value = entry.value.raw
  draft.value = entry.value.body
  editing.value = true
}
async function mutate(
  method: 'writeProjectNote' | 'promoteProjectConvention' | 'updateProjectMemoryStatus',
  payload: unknown,
  revision = entry.value?.revision,
) {
  const path = workspace.active?.path
  if (revision === undefined || !path || !writable.value) return false
  const ticket = generation
  const mutation = ++mutationGeneration
  mutating.value = true
  let result
  try {
    result = await gateway.resourceRequest(method, path, payload, revision)
  } finally {
    if (mutation === mutationGeneration) mutating.value = false
  }
  if (ticket !== generation || workspace.active?.path !== path) return false
  if (!result.ok) {
    notice.value = result.error
    return false
  }
  const updated = result.data as MemoryEntry
  raw.value = raw.value.map((row) => (row.reference.id === updated.reference.id ? updated : row))
  return updated
}
async function promote() {
  dialog.value = false
  if (!entry.value) return
  if (
    await mutate('promoteProjectConvention', {
      id: entry.value.id,
      content_digest: entry.value.digest,
    })
  )
    notice.value =
      'This exact revision was confirmed as a project convention. No tool permissions were granted.'
}
async function save() {
  const item = draftEntry.value,
    edit = editGeneration,
    body = draft.value
  if (!editing.value || !item) return
  const updated = await mutate(
    'writeProjectNote',
    {
      id: item.reference.id,
      title: item.content.title,
      body,
      files: item.content.files.map((f) => f.path),
    },
    item.reference.revision,
  )
  if (updated && edit === editGeneration && editing.value) {
    draftEntry.value = updated
    editing.value = draft.value !== body
    notice.value = editing.value
      ? 'Saved this revision. Your newer draft is still unsaved.'
      : 'New revision saved. Any previous convention confirmation no longer applies.'
  }
}
async function archive() {
  if (
    entry.value &&
    (await mutate('updateProjectMemoryStatus', { id: entry.value.id, validity: 'archived' }))
  )
    notice.value = 'Archived. Existing references retain their identity and revision.'
}
async function showSource() {
  const source = entry.value?.raw.content.source
  const path = workspace.active?.path
  if (!source || !path) {
    notice.value = 'No linked execution or conversation is available for this note.'
    return
  }
  const ticket = generation,
    sourceTicket = ++sourceGeneration,
    id = entry.value?.id
  sourceEvidence.value = ''
  if (source.startsWith('run:')) {
    const result = await gateway.resourceRequest('queryProjectMemory', path, {
      operation: 'evidence',
      run_id: source.slice(4),
      entry_id: null,
    })
    if (
      ticket === generation &&
      sourceTicket === sourceGeneration &&
      workspace.active?.path === path &&
      entry.value?.id === id
    ) {
      if (result.ok) sourceEvidence.value = JSON.stringify(result.data, null, 2)
      else notice.value = 'Source run is unavailable in this workspace.'
    }
  } else {
    notice.value = `Source reference: ${source}. The original source retains its role and evidence classification.`
  }
}
</script>

<template>
  <section aria-label="Project Memory">
    <PageHeader title="Project Memory"
      ><template #help
        ><InlineHelp
          text="Notes, conventions and evidence, with their original source and version." /></template
      ><button class="btn btn-outline" @click="load()">Reload</button></PageHeader
    >
    <div class="pv-toolbar">
      <label class="pv-search"
        ><span class="sr-only">Search memory</span
        ><input v-model="search" class="input" placeholder="Search notes and evidence" /></label
      ><label
        ><span class="sr-only">Type</span
        ><select v-model="kind" class="input">
          <option>All entries</option>
          <option>Project note</option>
          <option>Convention</option>
          <option>Evidence</option>
          <option>Handoff</option>
        </select></label
      >
    </div>
    <p v-if="scenario === 'Loading'" role="status" class="pv-callout">Loading project memory…</p>
    <p v-else-if="scenario === 'Error'" role="alert" class="pv-callout">
      Memory is unavailable. Your draft is preserved.
      <button class="btn btn-outline" @click="load()">Retry</button>
    </p>
    <template v-else>
      <p v-if="scenario === 'Disconnected'" class="pv-callout" role="status">
        Disconnected · Showing the last loaded snapshot. Editing and confirmation are disabled.
      </p>
      <div class="pv-split">
        <aside aria-label="Memory entries" class="pv-list">
          <button
            v-for="row in rows"
            :key="row.id"
            class="pv-row"
            :class="{ selected: entry?.id === row.id }"
            @click="selectEntry(row.id)"
          >
            <span class="pv-muted">{{ row.kind }} · r{{ row.revision }}</span
            ><strong>{{ row.title }}</strong
            ><span :class="row.validity === 'Needs review' ? 'pv-warn' : 'pv-muted'"
              >{{ row.origin }} · {{ row.validity }}</span
            >
          </button>
          <p v-if="!rows.length" class="pv-empty">No matching entries.</p>
        </aside>
        <article v-if="entry" class="pv-detail">
          <div class="pv-between">
            <span class="pv-badge">{{ entry.origin }}</span
            ><span class="pv-muted">Revision {{ entry.revision }}</span>
          </div>
          <h2>{{ entry.title }}</h2>
          <p class="pv-callout" v-if="entry.validity === 'Needs review'">
            Needs review · Applies to an earlier file version.
          </p>
          <label v-if="editing"
            >Note body<textarea v-model="draft" class="input pv-editor" />
          </label>
          <p v-else class="pv-body">{{ entry.body }}</p>
          <details class="pv-disclosure">
            <summary>Source details</summary>
            <dl class="pv-facts">
              <dt>Source</dt>
              <dd>
                <button class="pv-link" @click="showSource">{{ entry.source }} ↗</button>
              </dd>
              <dt>Content digest</dt>
              <dd>
                <code>{{ entry.digest }}</code>
              </dd>
              <template v-if="entry.file"
                ><dt>File version</dt>
                <dd>{{ entry.file }}</dd></template
              >
              <dt>Trust</dt>
              <dd>
                {{
                  entry.evidence
                    ? 'Host-owned reference. Not editable as a note.'
                    : 'Current does not mean verified. A convention grants no capabilities.'
                }}
              </dd>
            </dl>
            <pre v-if="sourceEvidence" class="pv-source-output">{{ sourceEvidence }}</pre>
          </details>
          <div class="pv-actions" v-if="!entry.evidence">
            <button class="btn btn-primary" v-if="editing" :disabled="!writable" @click="save">
              Save new revision
            </button>
            <button class="btn btn-outline" v-if="editing" @click="cancelEdit">Cancel edit</button>
            <button
              class="btn btn-primary"
              v-if="entry.kind !== 'Convention' && !editing"
              :disabled="!writable"
              @click="dialog = true"
            >
              Pin as convention
            </button>
            <MoreActions v-if="!editing"
              ><button :disabled="!writable" @click="editNote">Edit note</button
              ><button :disabled="!writable" @click="archive">Archive</button></MoreActions
            >
          </div>
        </article>
        <div v-else class="pv-empty">Select a memory entry to inspect its source.</div>
      </div>
    </template>
    <button v-if="next" class="btn btn-outline" @click="load(true)">Load more</button>
    <p v-if="notice" class="pv-callout" role="status">{{ notice }}</p>
    <div v-if="dialog && entry" v-dialog-focus class="pv-overlay" @keydown.esc="dialog = false">
      <section role="dialog" aria-modal="true" aria-labelledby="confirm-title" class="pv-modal">
        <h2 id="confirm-title">Pin This Exact Revision?</h2>
        <p>{{ entry.title }}</p>
        <p class="pv-body">{{ entry.body }}</p>
        <code>{{ entry.id }} · r{{ entry.revision }} · {{ entry.digest }}</code>
        <p class="pv-muted">
          This becomes a project convention. It grants no tool permissions. Future edits require a
          new confirmation.
        </p>
        <div class="pv-actions">
          <button class="btn btn-outline" autofocus @click="cancelConfirmation">Cancel</button
          ><button class="btn btn-primary" @click="promote">Confirm convention</button>
        </div>
      </section>
    </div>
  </section>
</template>
