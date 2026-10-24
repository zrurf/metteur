<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch } from 'vue'

/**
 * Side-by-side comparison of a file's snapshot content against the working
 * copy, using Monaco's diff editor.
 *
 * Both sides are read-only: this is a reading surface for "what changed".
 * Editing happens in the normal editor pane, which sits next to this one when
 * the split view is open.
 */
const props = defineProps<{
  /** Path shown in the header, for the reader's orientation. */
  filePath: string
  /** The recorded (baseline) text. */
  original: string
  /** The current text. */
  modified: string
  /** Snapshot the baseline came from, shown in the footer. */
  snapshotId?: string
  comparisonLabel?: string
}>()

const host = ref<HTMLElement | null>(null)
let editor: { dispose: () => void } | null = null
let model: unknown = null
let originalModel: unknown = null

/** Creates the diff editor once Monaco is available. */
async function mount() {
  if (!host.value) return
  // Monaco is loaded on demand, exactly like the editor pane does.
  const monaco = (await import('@/lib/monaco')).default
  if (!host.value) return
  const original = monaco.editor.createModel(props.original)
  const modified = monaco.editor.createModel(props.modified)
  originalModel = original
  model = modified
  editor = monaco.editor.createDiffEditor(host.value, {
    readOnly: true,
    automaticLayout: true,
    renderSideBySide: true,
    originalEditable: false,
    minimap: { enabled: false },
    scrollBeyondLastLine: false,
    fontSize: 13,
  })
  ;(
    editor as unknown as {
      setModel: (value: { original: unknown; modified: unknown }) => void
    }
  ).setModel({ original, modified })
}

onMounted(() => {
  void mount()
})

onBeforeUnmount(() => {
  editor?.dispose()
  ;(originalModel as { dispose?: () => void } | null)?.dispose?.()
  ;(model as { dispose?: () => void } | null)?.dispose?.()
  editor = null
  originalModel = null
  model = null
})

// Refreshing the content (a new snapshot, or an edit in the other pane) swaps
// the text without rebuilding the editor.
watch(
  () => [props.original, props.modified] as const,
  ([original, modified]) => {
    ;(originalModel as { setValue?: (text: string) => void } | null)?.setValue?.(original)
    ;(model as { setValue?: (text: string) => void } | null)?.setValue?.(modified)
  },
)
</script>

<template>
  <div class="flex h-full min-h-0 flex-col">
    <div ref="host" class="min-h-0 flex-1" />
    <p class="shrink-0 border-t border-divider px-3 py-1 text-[13px] text-subtle">
      <span class="text-foreground/70">{{ filePath }}</span>
      <template v-if="comparisonLabel"> · {{ comparisonLabel }}</template>
      <template v-else> · snapshot {{ snapshotId || '(none)' }} → working copy</template>
    </p>
  </div>
</template>
