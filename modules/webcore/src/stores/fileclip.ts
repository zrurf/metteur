import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { useWorkspaceStore } from './workspace'

/**
 * VSCode-style file clipboard for the explorer: holds the selected paths and
 * whether they will be moved (cut) or copied. Consumed by the context-menu
 * Copy / Cut / Paste actions and cleared after a paste.
 */

export const useFileClipboardStore = defineStore('fileclip', () => {
  const workspace = useWorkspaceStore()
  const owner = ref('')
  const op = ref<'copy' | 'cut' | null>(null)
  const sources = ref<string[]>([])

  /** Whether a paste can currently be performed. */
  const active = computed(
    () => op.value !== null && sources.value.length > 0 && owner.value === workspace.active?.path,
  )

  /** Stage paths for the given operation. */
  function set(operation: 'copy' | 'cut', paths: string[]) {
    op.value = operation
    owner.value = workspace.active?.path ?? ''
    sources.value = [...paths]
  }

  function clear() {
    op.value = null
    sources.value = []
    owner.value = ''
  }

  return { op, sources, active, set, clear }
})
