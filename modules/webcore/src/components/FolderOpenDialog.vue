<script setup lang="ts">
import { FolderOpen, X } from '@lucide/vue'
import { projectName } from '@/lib/path'

/**
 * Post-picker confirmation for "Open a folder…" (JetBrains-style): after a
 * folder is chosen (or a recent workspace is picked from the switcher), the
 * user decides where to open it — this window, a new window, or cancel.
 * Closing the current workspace is deferred to the chosen action so it never
 * happens implicitly.
 */
defineProps<{ path: string }>()
const emit = defineEmits<{ thisWindow: []; newWindow: []; cancel: [] }>()
</script>

<template>
  <Teleport to="body">
    <Transition name="folder-open">
      <div
        class="fixed inset-0 z-1100 flex items-center justify-center p-4"
        style="background: var(--overlay)"
        @mousedown.self="emit('cancel')"
      >
        <div
          class="w-full max-w-sm rounded-xl p-5"
          style="background: var(--popover); box-shadow: var(--shadow-popover); border: 1px solid var(--divider)"
        >
          <div class="flex items-start gap-3">
            <span
              class="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg"
              style="background: var(--primary-soft); color: var(--primary)"
            >
              <FolderOpen class="h-4.5 w-4.5" />
            </span>
            <div class="min-w-0 flex-1">
              <h2 class="text-[13.5px] font-semibold text-foreground">Open in a New Workspace?</h2>
              <p class="mt-1 text-[12px] leading-relaxed text-muted-foreground wrap-break-word">
                <span class="font-medium text-foreground">{{ projectName(path) }}</span>
                <span class="block truncate">{{ path }}</span>
              </p>
            </div>
            <button class="btn-icon h-6! w-6!" type="button" aria-label="Cancel" @click="emit('cancel')">
              <X class="h-3.5 w-3.5" />
            </button>
          </div>
          <div class="mt-4 flex flex-col gap-2">
            <button class="btn btn-primary h-9!" type="button" @click="emit('thisWindow')">
              This window
            </button>
            <button
              class="btn h-9!"
              type="button"
              style="background: var(--surface-muted); box-shadow: inset 0 0 0 1px var(--border)"
              @click="emit('newWindow')"
            >
              Open in a new window
            </button>
            <button
              class="btn h-9!"
              type="button"
              style="background: var(--surface-muted); box-shadow: inset 0 0 0 1px var(--border)"
              @click="emit('cancel')"
            >
              Cancel
            </button>
          </div>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.folder-open-enter-active,
.folder-open-leave-active {
  transition: opacity 140ms ease;
}
.folder-open-enter-from,
.folder-open-leave-to {
  opacity: 0;
}
</style>