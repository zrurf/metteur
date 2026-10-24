<script setup lang="ts">
import { ref, onMounted, onBeforeUnmount } from 'vue'
import { gateway } from '@/core'
import { useRecoveryStore, type RecoveryMetadata } from '@/stores/recovery'
import type { RecoveryRecord } from '@/core/recovery'
import { vDialogFocus } from '@/components/resources/dialog-focus'
const props = defineProps<{
  workspace: string
  scope: 'project' | 'personal'
  entity?: string
  localCopies?: RecoveryRecord[]
}>()
const emit = defineEmits<{ close: []; restore: [record: RecoveryRecord] }>()
const recovery = useRecoveryStore(),
  rows = ref<RecoveryMetadata[]>([]),
  next = ref<string | null>(null),
  error = ref(''),
  loading = ref(false)
const selected = ref<{ record: RecoveryRecord; source_available: boolean } | null>(null)
let live = true
onBeforeUnmount(() => {
  live = false
})
async function load(after: string | null = null) {
  loading.value = true
  const result = await recovery.list(props.workspace, props.scope, after)
  if (!live) return
  loading.value = false
  if (!result.ok) {
    error.value = result.error
    return
  }
  const page = result.data as { items: RecoveryMetadata[]; next: string | null }
  rows.value = after ? [...rows.value, ...page.items] : page.items
  next.value = page.next
  error.value = ''
}
onMounted(() => {
  void load()
})
async function restore(row: RecoveryMetadata) {
  const result = await recovery.read(props.workspace, props.scope, row.id)
  if (!live) return
  if (!result.ok) {
    error.value = result.error
    return
  }
  if (result.data.record.discarded) {
    error.value = 'This copy was discarded. It cannot be restored.'
    return
  }
  selected.value = result.data
}
async function exportRaw(row: RecoveryMetadata) {
  const result = await gateway.resourceRequest('recovery', props.workspace, {
    scope: props.scope,
    action: { operation: 'export', id: row.id },
  })
  if (!live) return
  if (!result.ok) {
    error.value = result.error
    return
  }
  const data = result.data as { encoding: string; raw: string }
  if (data.encoding !== 'base64') {
    error.value = 'Unsupported export encoding'
    return
  }
  const bytes = Uint8Array.from(atob(data.raw), (c) => c.charCodeAt(0))
  const url = URL.createObjectURL(new Blob([bytes], { type: 'application/json' })),
    link = document.createElement('a')
  link.href = url
  link.download = `metteur-recovery-${row.id}.json`
  link.click()
  setTimeout(() => URL.revokeObjectURL(url), 1000)
}
async function copySelected() {
  if (!selected.value) return
  try {
    await navigator.clipboard.writeText(selected.value.record.input.raw)
  } catch {
    error.value = 'Could not copy. Select the text and copy it manually.'
  }
}
function sourceLabel(kind: string | null, entity: string | null) {
  if (kind === 'workflow') return 'Coding Plan'
  if (kind === 'workflow_review') return 'Delivery Review'
  return kind === 'chat' || kind === 'new_chat'
    ? 'Chat Input'
    : kind === 'supervision'
      ? 'Supervision Input'
      : entity || 'Quarantined Record'
}
function exportLocal(record: RecoveryRecord) {
  const url = URL.createObjectURL(
      new Blob([JSON.stringify(record, null, 2)], { type: 'application/json' }),
    ),
    link = document.createElement('a')
  link.href = url
  link.download = `metteur-local-input-${record.input.id}.json`
  link.click()
  setTimeout(() => URL.revokeObjectURL(url), 1000)
}
</script>
<template>
  <div
    class="fixed inset-0 z-50 grid place-items-center p-5"
    style="background: var(--overlay)"
    @keydown.esc="emit('close')"
    @mousedown.self="emit('close')"
  >
    <section
      v-dialog-focus
      role="dialog"
      aria-modal="true"
      aria-label="Recovery Copies"
      class="recovery-dialog"
    >
      <header>
        <h2>Recovery Copies</h2>
        <button class="btn" @click="emit('close')">Close</button>
      </header>
      <p>Choose a copy to review. Using it changes your draft; the saved file stays unchanged.</p>
      <p v-if="error" role="alert">{{ error }}</p>
      <template v-if="selected"
        ><strong>{{
          sourceLabel(selected.record.input.kind, selected.record.input.entity)
        }}</strong>
        <details>
          <summary>Source Details</summary>
          <p>{{ selected.record.input.entity }}</p>
          <p v-if="selected.record.input.pending_submission">
            Pending Input: {{ selected.record.input.pending_submission }}
          </p>
        </details>
        <p v-if="!selected.source_available">
          Source may be missing or unavailable. Input remains recoverable; it will not recreate the
          source automatically.
        </p>
        <pre class="whitespace-pre-wrap break-words rounded border border-border p-3 text-[13px]">{{
          selected.record.input.raw
        }}</pre>
        <div class="mt-3 flex gap-2">
          <button class="btn" @click="selected = null">Back to Copies</button
          ><button class="btn" @click="copySelected">Copy Text</button
          ><button class="btn btn-primary" @click="emit('restore', selected.record)">
            Use This Copy
          </button>
        </div></template
      ><template v-else
        ><p v-if="loading" role="status">Loading recovery copies…</p>
        <article v-for="record in localCopies ?? []" :key="`local-${record.input.id}`">
          <div>
            <strong>{{ sourceLabel(record.input.kind, record.input.entity) }}</strong>
            <p>This window · Not backed up</p>
          </div>
          <button class="btn" @click="selected = { record, source_available: false }">
            Review Copy</button
          ><button class="btn" @click="exportLocal(record)">Export</button>
        </article>
        <p v-if="!loading && !rows.length && !error">No recovery copies in this scope.</p>
        <article
          v-for="row in rows.filter((item) => !item.discarded && item.kind !== 'view_state')"
          :key="row.id"
        >
          <div>
            <strong>{{ sourceLabel(row.kind, row.entity) }}</strong>
            <p>
              {{
                row.error
                  ? 'This copy needs review'
                  : row.window_id === recovery.windowId
                    ? 'This window'
                    : 'Another window'
              }}
            </p>
            <details>
              <summary>Details</summary>
              <p>{{ row.kind }} · Edit {{ row.edit_sequence }} · Window {{ row.window_id }}</p>
              <p>{{ row.entity }}</p>
            </details>
            <p v-if="entity && row.entity !== entity">
              Different source. Copy only after checking its identity.
            </p>
          </div>
          <button class="btn" :disabled="!!row.error" @click="restore(row)">Review Copy</button
          ><button class="btn" @click="exportRaw(row)">Export</button>
        </article>
        <button v-if="next" class="btn" :disabled="loading" @click="load(next)">
          Load More
        </button></template
      >
      <details class="mt-4">
        <summary>About Recovery</summary>
        <p>
          Missing sources keep their recovery copies. Restoring input does not run a model or
          restore permissions. Prompt selections and attachments need current access and version
          checks.
        </p>
      </details>
    </section>
  </div>
</template>
<style scoped>
.recovery-dialog {
  width: min(760px, 100%);
  max-height: 85vh;
  overflow: auto;
  padding: 24px;
  background: var(--popover);
  border: 1px solid var(--border);
  border-radius: 10px;
  font-size: 14px;
  line-height: 1.7;
}
.recovery-dialog header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}
.recovery-dialog h2 {
  font-size: 18px;
  font-weight: 600;
}
.recovery-dialog p {
  color: var(--muted-foreground);
  margin: 10px 0;
}
.recovery-dialog [role='alert'] {
  color: var(--status-error);
}
.recovery-dialog article {
  display: flex;
  align-items: center;
  gap: 8px;
  border-bottom: 1px solid var(--border);
  padding: 13px 0;
  flex-wrap: wrap;
}
.recovery-dialog article > div {
  flex: 1;
  min-width: 150px;
  overflow-wrap: anywhere;
}
</style>
