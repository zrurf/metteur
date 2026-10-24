<script setup lang="ts">
import { computed, onMounted, onBeforeUnmount, ref } from 'vue'
import { gateway, type FileContent, type FileTreeNode } from '@/core'
import { useDocumentsStore } from '@/stores/documents'
import { vDialogFocus } from './resources/dialog-focus'
const props = defineProps<{ workspace: string; file: FileTreeNode }>()
const emit = defineEmits<{ close: []; selected: [file: FileTreeNode]; review: [] }>()
const documents = useDocumentsStore(),
  saved = ref<FileContent | null>(null),
  error = ref(''),
  busy = ref(false)
let disposed = false
onBeforeUnmount(() => {
  disposed = true
})
const canSave = computed(() => {
  const doc = documents.peek(props.workspace, props.file.path)
  return !!doc?.loaded && !doc.saving && !doc.readOnlyReason
})
async function capture() {
  busy.value = true
  try {
    const result = await gateway.readFile(props.workspace, props.file.path, true)
    if (disposed) return
    if (!result.ok || !result.data.reference) {
      saved.value = null
      error.value = result.ok ? 'The host could not freeze this saved version.' : result.error
      return
    }
    saved.value = result.data
    error.value = ''
  } catch (cause) {
    if (!disposed) error.value = String(cause)
  } finally {
    busy.value = false
  }
}
onMounted(capture)
function useSaved() {
  if (saved.value?.reference) emit('selected', { ...props.file, reference: saved.value.reference })
}
async function saveThenReference() {
  if (busy.value || !canSave.value) return
  const doc = documents.get(props.workspace, props.file.path),
    content = doc.content
  if (new TextEncoder().encode(content).length > 65536) {
    error.value =
      'Saved references support at most 64 KiB. Keep the draft and use an explicit path for larger files.'
    return
  }
  busy.value = true
  error.value = ''
  try {
    if (!(await documents.save(props.workspace, props.file.path))) {
      error.value = doc.error
      return
    }
    const result = await gateway.readFile(props.workspace, props.file.path, true)
    if (disposed) return
    if (!result.ok || !result.data.reference) {
      error.value = result.ok
        ? 'The file was saved, but its reference was not confirmed.'
        : result.error
      return
    }
    if (result.data.content !== content) {
      error.value =
        'The saved file changed before its reference was captured. Review the latest version.'
      return
    }
    emit('selected', { ...props.file, reference: result.data.reference })
  } catch (cause) {
    if (!disposed) error.value = String(cause)
  } finally {
    busy.value = false
  }
}
</script>
<template>
  <Teleport to="body"
    ><div
      class="fixed inset-0 z-50 grid place-items-center p-4"
      style="background: var(--overlay)"
      @mousedown.self="emit('close')"
      @keydown.esc.stop="emit('close')"
    >
      <section
        v-dialog-focus
        role="dialog"
        aria-modal="true"
        aria-label="Reference File"
        class="reference-dialog"
      >
        <header>
          <h2>Reference File</h2>
          <button class="btn" @click="emit('close')">Cancel</button>
        </header>
        <p>{{ file.path }} has unsaved changes. Choose the content this message references.</p>
        <p>Later tool reads use the file version present at read time.</p>
        <p v-if="busy" role="status">Preparing the saved version…</p>
        <p v-if="error" role="alert" class="text-danger">{{ error }}</p>
        <details v-if="saved?.reference">
          <summary>Saved Version &amp; Content</summary>
          <p>Snapshot: {{ saved.reference.snapshotId }}</p>
          <p>SHA256: {{ saved.reference.digest }}</p>
          <pre>{{ saved.content }}</pre>
        </details>
        <footer>
          <button class="btn" :disabled="busy || !saved?.reference" @click="useSaved">
            Use This Saved Version</button
          ><button class="btn btn-primary" :disabled="busy || !canSave" @click="saveThenReference">
            Save Then Reference
          </button>
        </footer>
        <div v-if="error" class="flex gap-2">
          <button class="btn" :disabled="busy" @click="capture">Review Latest</button
          ><button class="btn" :disabled="busy" @click="emit('review')">Open File</button>
        </div>
      </section>
    </div></Teleport
  >
</template>
<style scoped>
.reference-dialog {
  width: min(600px, 95vw);
  max-height: 88vh;
  overflow: auto;
  background: var(--popover);
  border: 1px solid var(--divider);
  border-radius: 12px;
  padding: 20px;
  display: flex;
  flex-direction: column;
  gap: 14px;
  font-size: 13px;
  line-height: 1.6;
  overflow-wrap: anywhere;
  box-shadow: var(--shadow-popover);
}
.reference-dialog header,
.reference-dialog footer {
  display: flex;
  align-items: center;
  gap: 10px;
}
.reference-dialog h2 {
  font-size: 15px;
  font-weight: 600;
  flex: 1;
}
.reference-dialog pre {
  white-space: pre-wrap;
  max-height: 20rem;
  overflow: auto;
  border: 1px solid var(--divider);
  padding: 10px;
  margin-top: 10px;
}
</style>
