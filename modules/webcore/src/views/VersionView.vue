<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { FileClock, GitBranch, GitCommitVertical, History, RotateCcw } from '@lucide/vue'
import { useFeedbackStore } from '@/stores/feedback'
import { useWorkspaceStore } from '@/stores/workspace'
import { useVersionStore } from '@/stores/version'
import { usePanelStore } from '@/stores/panel'
import VersionSidebar from '@/components/VersionSidebar.vue'
import type { SnapshotInfo } from '@/core'

const workspace = useWorkspaceStore()
const version = useVersionStore()
const panel = usePanelStore()
const feedback = useFeedbackStore()
const fileInput = ref(version.selectedFile ?? 'src/api.ts')
const snapDesc = ref('')

/** Newest snapshot first, used for the `HEAD-n` chip on the detail card. */
const sortedSnapshots = computed(() => [...version.snapshots].sort((a, b) => b.createdAt - a.createdAt))
/** Snapshot whose details are shown in the editor pane. */
const selected = computed(() => version.snapshots.find((s) => s.id === version.selectedId))

onMounted(async () => {
  await workspace.refresh()
  if (workspace.active) await version.refresh()
})

/** This view owns the left rail: pop it in on mount. The rail is *not*
 *  cleared on unmount — full-page surfaces (Chat / Audit / Execution) keep
 *  whatever sidebar is open, exactly like the Explorer. */
onMounted(() => panel.show('version', VersionSidebar, 'Version Flow'))

function format(ts: number): string {
  return new Intl.DateTimeFormat(undefined, { timeStyle: 'short', dateStyle: 'medium' }).format(ts)
}

async function handlePickFile() {
  await version.pickFile(fileInput.value.trim())
}

async function handleCreate() {
  if (await version.create(snapDesc.value)) snapDesc.value = ''
}

async function handleRollback(snapshot: SnapshotInfo) {
  const ok = await feedback.confirm({
    header: 'Confirm rollback',
    message: `Roll back to ${snapshot.alias ?? snapshot.id}?`,
    acceptLabel: 'Roll back',
    rejectLabel: 'Cancel',
    danger: true,
  })
  if (ok) await version.rollback(snapshot.id)
}

function opTone(op: string): string {
  switch (op) {
    case 'add':
      return 'text-emerald-600 dark:text-emerald-400'
    case 'delete':
      return 'text-danger'
    default:
      return 'text-muted-foreground'
  }
}
</script>

<template>
  <div class="flex h-full flex-col">
    <!-- Editor toolbar -->
    <div class="flex h-11 shrink-0 items-center gap-2 border-b border-divider px-3">
      <GitBranch class="h-4 w-4 text-muted-foreground" />
      <span class="text-[13px] font-medium">Version Flow</span>
      <span class="chip">{{ version.snapshots.length }} snapshot(s)</span>

      <div class="ml-auto flex items-center gap-1.5">
        <input
          v-model="snapDesc"
          class="input h-8 w-48 text-[12px]!"
          placeholder="Snapshot description…"
          aria-label="Snapshot description"
          @keydown.enter="handleCreate"
        />
        <button class="btn btn-primary h-8!" type="button" title="Create snapshot" @click="handleCreate">
          <GitCommitVertical class="h-4 w-4" />
        </button>
        <button class="btn-icon" type="button" title="Refresh" aria-label="Refresh" @click="version.refresh()">
          <RotateCcw class="h-4 w-4" />
        </button>
      </div>
    </div>

    <!-- Detail + file history -->
    <div class="min-h-0 flex-1 overflow-y-auto">
      <div class="mx-auto max-w-2xl space-y-5 p-5">
        <p v-if="version.error" role="alert">{{ version.error }}</p>
        <section v-if="version.reference" class="panel p-4 break-all" aria-label="Referenced blueprint version">
          <strong>{{ version.reference.label }}</strong>
          <p>Run: {{ version.reference.runId }}</p><p v-if="version.reference.proposalId">Proposal: {{ version.reference.proposalId }}</p>
          <p>{{ version.reference.version.blueprint_uri }}</p><code>{{ version.reference.version.blob_hash }}</code>
          <p v-if="!selected">Referenced snapshot unavailable: {{ version.reference.version.snapshot_id }}</p>
        </section>
        <!-- Selected snapshot detail -->
        <section v-if="selected" class="panel p-4">
          <div class="flex items-start gap-3">
            <span
              class="mt-0.5 flex h-8 w-8 shrink-0 items-center justify-center rounded-lg"
              :style="{ background: selected.alias ? 'var(--primary-soft)' : 'var(--surface-muted)' }"
            >
              <GitCommitVertical class="h-4 w-4" :style="{ color: 'var(--primary)' }" />
            </span>
            <div class="min-w-0 flex-1">
              <div class="flex items-center gap-2">
                <span class="font-mono text-[13px] font-semibold">{{ selected.id }}</span>
                <span v-if="selected.alias" class="chip" style="color: var(--primary)">{{ selected.alias }}</span>
                <span class="chip">HEAD-{{ sortedSnapshots.findIndex((s) => s.id === (selected?.id ?? '')) + 1 }}</span>
              </div>
              <p class="mt-1 text-[12.5px] text-foreground">{{ selected.message }}</p>
              <p class="mt-0.5 text-[11px] text-muted-foreground">{{ format(selected.createdAt) }}</p>
            </div>
            <button class="btn btn-danger-outline shrink-0" type="button" @click="handleRollback(selected)">
              <RotateCcw class="h-4 w-4" /> Roll back
            </button>
          </div>
        </section>
        <p v-else class="panel px-4 py-6 text-center text-[12.5px] text-muted-foreground">
          Select a snapshot.
        </p>

        <!-- File history -->
        <div>
          <h1 class="flex items-center gap-2 text-[15px] font-semibold">
            <FileClock class="h-4 w-4 text-muted-foreground" /> File history
          </h1>
          <p class="mt-0.5 text-[12px] text-muted-foreground">Inspect how a file changed across snapshots.</p>
        </div>

        <form class="flex gap-2" @submit.prevent="handlePickFile">
          <input
            v-model="fileInput"
            class="input min-w-0 flex-1"
            placeholder="File path within workspace, e.g. src/api.ts"
            type="text"
            :aria-label="'File path'"
          />
          <button class="btn btn-primary" type="submit" :disabled="!fileInput.trim()">Inspect</button>
        </form>

        <section class="panel divide-y divide-divider">
          <div v-for="(h, i) in version.history" :key="i" class="px-4 py-3">
            <div class="flex items-center gap-2">
              <History class="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
              <span class="min-w-0 flex-1 truncate font-mono text-[12px]">{{ h.path }}</span>
              <span class="chip" :class="opTone(h.op)">{{ h.op }} | {{ h.snapshotId }}</span>
            </div>
            <pre
              v-if="h.diff"
              class="mt-2 overflow-x-auto rounded-md bg-surface-muted p-2 font-mono text-[11px] leading-5 text-muted-foreground"
            >{{ h.diff }}</pre>
          </div>

          <p v-if="version.history.length === 0" class="px-4 py-6 text-center text-[12.5px] text-muted-foreground">
            Pick a file to see its timeline.
          </p>
        </section>
      </div>
    </div>
  </div>
</template>
