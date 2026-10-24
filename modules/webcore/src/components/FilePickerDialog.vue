<script setup lang="ts">
import { onMounted, onUnmounted, ref, watch } from 'vue'
import { vDialogFocus } from '@/components/resources/dialog-focus'
import { ArrowUp, File, Folder, X } from '@lucide/vue'
import { gateway } from '@/core'
import type { FileTreeNode } from '@/core'
import { dirOf } from '@/lib/path'

/** Confirmed selection payload. */
export interface FilePick {
  /** Current directory (workspace-relative, `''` = root). */
  dir: string
  /** Path of the picked file (mode `open`). */
  filePath?: string
  /** Entered file name (mode `save`). */
  name?: string
}

/**
 * Workspace file/directory picker.
 *
 * `open` mode selects an existing file; `save` mode picks a target directory
 * and enters a file name. Both navigate the workspace tree one level at a time
 * through the daemon `ListFiles` RPC.
 */
const props = withDefaults(
  defineProps<{
    open: boolean
    mode: 'open' | 'save'
    title: string
    workspacePath: string
    /** Extension filter, e.g. `['.mbp']` (case-insensitive). */
    extensions?: string[]
  }>(),
  { extensions: () => [] },
)

const emit = defineEmits<{
  (e: 'close'): void
  (e: 'confirm', payload: FilePick): void
}>()

const dir = ref('')
const entries = ref<FileTreeNode[]>([])
const loading = ref(false)
const error = ref('')
/** Picked file path (mode `open`). */
const selected = ref('')
/** Entered file name (mode `save`). */
const name = ref('')
let generation = 0

async function load(dirPath: string) {
  const ticket = ++generation,
    workspace = props.workspacePath
  entries.value = []
  selected.value = ''
  dir.value = dirPath
  loading.value = true
  error.value = ''
  try {
    const r = await gateway.listFiles(workspace, dirPath)
    if (ticket !== generation || props.workspacePath !== workspace) return
    if (r.ok) {
      // Directories first, then files (extensions filter applies to files).
      const dirs = r.data.filter((e) => e.kind === 'dir')
      const files = r.data.filter(
        (e) =>
          e.kind === 'file' &&
          (props.extensions.length === 0 ||
            props.extensions.some((x) => e.name.toLowerCase().endsWith(x.toLowerCase()))),
      )
      entries.value = [...dirs, ...files]
    } else {
      error.value = r.error
      entries.value = []
    }
  } finally {
    if (ticket === generation) loading.value = false
  }
}

function enter(entry: FileTreeNode) {
  if (entry.kind === 'dir') {
    selected.value = ''
    void load(entry.path)
  } else {
    selected.value = entry.path
  }
}

function goUp() {
  load(dirOf(dir.value))
}

function confirm() {
  emit('confirm', {
    dir: dir.value,
    filePath: props.mode === 'open' ? selected.value : undefined,
    name: props.mode === 'save' ? name.value.trim() : undefined,
  })
}

/** Directory display label: `''` shows as `Workspace root`. */
const dirLabel = () => (dir.value === '' ? 'Workspace root' : dir.value)

watch([() => props.open, () => props.workspacePath], ([open]) => {
  if (open) {
    name.value = ''
    selected.value = ''
    void load('')
  }
})

onMounted(() => {
  if (props.open) void load('')
})
onUnmounted(() => {
  generation++
})
</script>

<template>
  <Teleport to="body">
    <div
      v-if="open"
      class="fixed inset-0 z-50 grid place-items-center bg-black/40 p-4"
      @click.self="emit('close')"
    >
      <div
        v-dialog-focus
        role="dialog"
        aria-modal="true"
        :aria-label="title"
        tabindex="-1"
        class="glass flex max-h-[75vh] w-120 max-w-full flex-col overflow-hidden rounded-xl border border-border shadow-2xl"
        @keydown.esc.stop="emit('close')"
      >
        <header class="flex items-center gap-2 border-b border-border px-3 py-2">
          <h2 class="truncate text-[13px] font-semibold text-foreground">{{ title }}</h2>
          <button
            class="ml-auto grid h-6 w-6 place-items-center rounded-md text-subtle transition-colors hover:bg-hover hover:text-foreground"
            type="button"
            title="Close"
            @click="emit('close')"
          >
            <X class="h-3.5 w-3.5" />
          </button>
        </header>

        <!-- Location bar -->
        <div class="flex items-center gap-1.5 border-b border-border px-3 py-1.5">
          <button
            class="grid h-6 w-6 place-items-center rounded-md text-subtle transition-colors hover:bg-hover hover:text-foreground disabled:opacity-40"
            type="button"
            title="Parent folder"
            :disabled="dir === ''"
            @click="goUp"
          >
            <ArrowUp class="h-3.5 w-3.5" />
          </button>
          <span class="truncate text-[12px] text-subtle">{{ dirLabel() }}</span>
        </div>

        <!-- Entry list -->
        <div class="min-h-0 flex-1 overflow-y-auto px-2 py-1.5">
          <p v-if="loading" class="px-2 py-1 text-[12px] text-subtle">Loading…</p>
          <p v-else-if="error" class="px-2 py-1 text-[12px] text-danger">{{ error }}</p>
          <p v-else-if="entries.length === 0" class="px-2 py-1 text-[12px] text-subtle">
            Empty folder
          </p>
          <button
            type="button"
            v-for="entry in entries"
            :key="entry.path"
            class="flex w-full cursor-pointer items-center gap-2 rounded-md px-2 py-1.5 text-left transition-colors"
            :class="
              selected === entry.path
                ? 'bg-accent text-accent-foreground'
                : 'text-foreground hover:bg-hover'
            "
            @click="enter(entry)"
            @dblclick="
              entry.kind === 'dir' ? enter(entry) : props.mode === 'open' ? confirm() : undefined
            "
          >
            <Folder v-if="entry.kind === 'dir'" class="h-4 w-4 shrink-0 text-[#e2a13c]" />
            <File v-else class="h-4 w-4 shrink-0 text-subtle" />
            <span class="truncate text-[12.5px]">{{ entry.name }}</span>
          </button>
        </div>

        <!-- Save-mode file name -->
        <div v-if="mode === 'save'" class="border-t border-border px-3 py-2">
          <input
            v-model="name"
            class="input h-7! w-full text-[12px]"
            :placeholder="extensions.length ? `file name (${extensions.join(', ')})` : 'file name'"
            @keyup.enter="name.trim() && confirm()"
          />
        </div>

        <!-- Actions -->
        <footer class="flex items-center justify-end gap-2 border-t border-border px-3 py-2">
          <button
            class="btn btn-outline h-7! px-3 text-[12px]"
            type="button"
            @click="emit('close')"
          >
            Cancel
          </button>
          <button
            class="btn btn-primary h-7! px-3 text-[12px] rounded-md"
            type="button"
            :disabled="mode === 'open' ? !selected : !name.trim()"
            @click="confirm"
          >
            {{ mode === 'save' ? 'Save' : 'Import' }}
          </button>
        </footer>
      </div>
    </div>
  </Teleport>
</template>
