<script setup lang="ts">
import { computed, onBeforeUnmount, ref } from 'vue'
import FileDiffView from './FileDiffView.vue'
import { useDocumentsStore, type DocumentReview } from '@/stores/documents'
import { vDialogFocus } from './resources/dialog-focus'
const props = defineProps<{ review: DocumentReview }>()
const emit = defineEmits<{ close: []; saved: [workspace: string, path: string] }>()
const documents = useDocumentsStore(),
  reviewed = ref(props.review),
  busy = ref(false),
  error = ref(''),
  compareBase = ref(false),
  copyPath = ref(''),
  copying = ref(false)
let disposed = false
onBeforeUnmount(() => {
  disposed = true
})
const original = computed(() =>
  compareBase.value ? reviewed.value.baseline : reviewed.value.latest,
)
async function refresh() {
  busy.value = true
  try {
    const current = reviewed.value
    if (!(await documents.sync(current.workspace, current.path))) {
      error.value =
        documents.get(current.workspace, current.path).error ||
        'The latest saved file could not be read.'
      return
    }
    const latest = documents.review(current.workspace, current.path)
    if (latest && !disposed) {
      reviewed.value = latest
      error.value = ''
      compareBase.value = false
    }
  } finally {
    busy.value = false
  }
}
async function apply(action: 'save' | 'disk' | 'copy') {
  if (busy.value) return
  const snapshot = reviewed.value
  busy.value = true
  error.value = ''
  try {
    const ok =
      action === 'save'
        ? await documents.saveReviewed(snapshot)
        : action === 'disk'
          ? await documents.useReviewedDisk(snapshot)
          : await documents.saveCopy(snapshot, copyPath.value.trim())
    if (disposed) return
    if (!ok) {
      error.value =
        documents.get(snapshot.workspace, snapshot.path).error ||
        'The original draft was kept. Review the latest versions.'
      return
    }
    if (action !== 'disk')
      emit('saved', snapshot.workspace, action === 'copy' ? copyPath.value.trim() : snapshot.path)
    emit('close')
  } catch (cause) {
    if (!disposed) error.value = String(cause)
  } finally {
    busy.value = false
  }
}
</script>
<template>
  <Teleport to="body">
    <div
      class="fixed inset-0 z-50 grid place-items-center p-4"
      style="background: var(--overlay)"
      @mousedown.self="emit('close')"
      @keydown.esc.stop="emit('close')"
    >
      <section
        v-dialog-focus
        role="dialog"
        aria-modal="true"
        aria-label="Compare Changes"
        class="conflict-dialog"
      >
        <header>
          <div>
            <h2>Compare Changes</h2>
            <p class="truncate">{{ reviewed.path }}</p>
          </div>
          <button class="btn" @click="emit('close')">Keep Editing</button>
        </header>
        <p>
          Your draft is on the right. Saving replaces only the version reviewed here; the previous
          saved version remains recoverable.
        </p>
        <div class="flex flex-wrap items-center gap-2">
          <button class="btn" :aria-pressed="!compareBase" @click="compareBase = false">
            Latest Saved</button
          ><button class="btn" :aria-pressed="compareBase" @click="compareBase = true">
            Common Baseline</button
          ><button class="btn ml-auto" :disabled="busy" @click="refresh">Review Latest</button>
        </div>
        <div class="comparison">
          <FileDiffView
            :file-path="reviewed.path"
            :original="original"
            :modified="reviewed.local"
            :comparison-label="`${compareBase ? 'Common baseline' : 'Latest saved'} → your reviewed draft`"
          />
        </div>
        <p v-if="error" role="alert" class="text-danger">{{ error }}</p>
        <div v-if="copying" class="flex flex-wrap items-center gap-2">
          <label for="conflict-copy-path">New file path</label
          ><input
            id="conflict-copy-path"
            v-model="copyPath"
            class="input min-w-0 flex-1"
            placeholder="path/to/new-file.ts"
          /><button class="btn" :disabled="busy || !copyPath.trim()" @click="apply('copy')">
            Save New Copy
          </button>
        </div>
        <footer>
          <button v-if="!reviewed.deleted" class="btn" :disabled="busy" @click="apply('disk')">
            Use Saved Version
          </button>
          <button
            v-if="!reviewed.resource"
            class="btn"
            :disabled="busy"
            @click="copying = !copying"
          >
            Keep Both
          </button>
          <button class="btn btn-primary ml-auto" :disabled="busy" @click="apply('save')">
            {{ busy ? 'Working…' : 'Save Reviewed Draft' }}
          </button>
        </footer>
      </section>
    </div>
  </Teleport>
</template>
<style scoped>
.conflict-dialog {
  width: min(1120px, 95vw);
  max-height: 92vh;
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 20px;
  border: 1px solid var(--divider);
  border-radius: 12px;
  background: var(--popover);
  color: var(--foreground);
  font-size: 13px;
  line-height: 1.6;
  overflow: auto;
  box-shadow: var(--shadow-popover);
}
.conflict-dialog header,
.conflict-dialog footer {
  display: flex;
  align-items: center;
  gap: 10px;
}
.conflict-dialog header > div {
  min-width: 0;
  flex: 1;
}
.conflict-dialog h2 {
  font-size: 15px;
  font-weight: 600;
}
.conflict-dialog header p {
  color: var(--muted-foreground);
}
.comparison {
  height: clamp(240px, 48vh, 600px);
  min-height: 200px;
  border: 1px solid var(--divider);
}
</style>
