<script setup lang="ts">
import { computed, onMounted, onBeforeUnmount, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import {
  Eye,
  FileCode2,
  GitBranch,
  GitCompare,
  Redo2,
  Save,
  TriangleAlert,
  Undo2,
  Workflow,
} from '@lucide/vue'
import { gateway } from '@/core'
import { useWorkspaceStore } from '@/stores/workspace'
import { useTabsStore } from '@/stores/tabs'
import { useRightPanelStore } from '@/stores/right-panel'
import { useFileWatchStore } from '@/stores/filewatch'
import { useFeedbackStore } from '@/stores/feedback'
import CodeEditor from '@/components/CodeEditor.vue'
import BlueprintView from './BlueprintView.vue'
import FileVersionPanel from '@/components/FileVersionPanel.vue'
import { dirOf } from '@/lib/path'
import { fileRoute, pathByFileParam } from '@/lib/file-token'
import { renderMarkdown } from '@/lib/markdown'
import FileDiffView from '@/components/FileDiffView.vue'
import DocumentConflictDialog from '@/components/DocumentConflictDialog.vue'
import { wurl } from '@/lib/workspace-url'
import { useDocumentsStore, type DocumentReview } from '@/stores/documents'
import { usePromptResourcesStore } from '@/stores/prompt-resources'
import { useSurfaceNavigation } from '@/lib/surface'
import RecoveryCopies from '@/components/RecoveryCopies.vue'
import { useRecoveryStore } from '@/stores/recovery'
import type { RecoveryRecord } from '@/core/recovery'
import { hash } from 'ohash'

const route = useRoute()
const router = useRouter()
const workspace = useWorkspaceStore()
const tabs = useTabsStore()
const rightPanel = useRightPanelStore()
const fileWatch = useFileWatchStore()
const feedback = useFeedbackStore()
const prompts = usePromptResourcesStore(),
  { openSurface } = useSurfaceNavigation()
async function usePromptInChat() {
  const resource = buffer.value.resource
  if (!resource || dirty.value) return
  try {
    await prompts.addChat(resource.reference)
    openSurface('chat')
  } catch (cause) {
    feedback.toast('error', 'Could Not Add Prompt', String(cause))
  }
}

/** An explicit path renders this pane as the split view's right-hand side. */
const props = defineProps<{ filePath?: string }>()

/** The route-driven path comes from the `?f=` hash, resolved via the persisted
 *  hash→path map — never from raw URL bytes (no path traversal surface). */
const routePath = computed(() => pathByFileParam(String(route.query.f ?? '')))
const filePath = computed(() => (props.filePath || routePath.value).replaceAll('\\', '/'))
const isBlueprint = computed(() => filePath.value.endsWith('.blueprint'))
/** A `.mbp` file is a text-drawn blueprint (the DSL); a toolbar action can
 *  compile it straight into a visual blueprint file. */
const isDsl = computed(() => filePath.value.toLowerCase().endsWith('.mbp'))

/**
 * Identity of the edited file for the shared Monaco model.
 *
 * Scoped by workspace so two workspaces never share a model, and identical for
 * the same file opened in both panes so both panes edit one buffer.
 */
const modelId = computed(() =>
  filePath.value ? `${workspace.active?.path ?? ''}::${filePath.value}` : '',
)

const documents = useDocumentsStore()
const recovery = useRecoveryStore()
const recoveryOpen = ref(false)
const conflictReview = ref<DocumentReview | null>(null)
const backup = computed(() =>
  recovery.status(
    workspace.active?.path ?? '',
    documents.recoveryKind(filePath.value),
    filePath.value,
  ),
)
const backupLabel = computed(() => {
  if (!backup.value) return 'No Unsaved Recovery Copy'
  if (backup.value.state === 'backed_up')
    return gateway.connected.value ? 'Backed Up on Host' : 'Backed Up on Host · Offline'
  if (backup.value.state === 'backing_up') return 'Backing Up…'
  if (backup.value.state === 'unsupported') return 'Recovery Unavailable'
  if (backup.value.state === 'unknown') return 'Backup Unconfirmed'
  if (backup.value.state === 'conflict') return 'Review Recovery Conflict'
  if (/capacity|storage.full/i.test(backup.value.error)) return 'Not Backed Up · Storage Full'
  return gateway.connected.value ? 'Not Backed Up' : 'Not Backed Up · Offline'
})
async function restoreRecovery(record: RecoveryRecord) {
  const target = workspace.active?.path,
    path = filePath.value
  if (
    !target ||
    record.input.entity !== path ||
    record.input.kind !== documents.recoveryKind(path)
  ) {
    feedback.toast(
      'error',
      'Different Recovery Source',
      'Open the matching source or export this copy for manual recovery.',
    )
    return
  }
  const doc = documents.get(target, path)
  const localRevision = doc.revision
  if (documents.dirty(doc) && !(await recovery.flush(target, documents.recoveryKind(path), path))) {
    feedback.toast(
      'error',
      'Current Draft Not Backed Up',
      'Keep or export the current input before replacing it with a recovery copy.',
    )
    return
  }
  await documents.sync(target, path)
  if (workspace.active?.path !== target || filePath.value !== path) return
  if (doc.revision !== localRevision) {
    feedback.toast(
      'error',
      'Newer Draft Kept',
      'Your input changed while the recovery copy was loading. Review it before replacing it.',
    )
    return
  }
  const view = record.input.view as { baseline_content?: unknown } | null
  if (
    typeof view?.baseline_content === 'string' &&
    `xxh3:${hash(view.baseline_content)}` === record.input.baseline_identity
  )
    doc.baseline = view.baseline_content
  recovery.fork(target, documents.recoveryKind(path), path, record)
  documents.edit(target, path, record.input.raw)
  doc.conflict = doc.deleted || (doc.disk !== doc.baseline && doc.disk !== doc.content)
  recoveryOpen.value = false
}
async function discardRecovery() {
  const target = workspace.active?.path,
    path = filePath.value
  if (!target) return
  const localRevision = documents.get(target, path).revision
  const confirmed = await feedback.confirm({
    header: 'Discard This Draft?',
    message:
      'Discard this recovery copy and local edits. The saved file and other windows keep their contents.',
    acceptLabel: 'Discard This Copy',
    rejectLabel: 'Keep Editing',
    danger: true,
  })
  if (!confirmed) return
  if (
    workspace.active?.path !== target ||
    filePath.value !== path ||
    documents.get(target, path).revision !== localRevision
  )
    return
  const ok = await recovery.discard(target, documents.recoveryKind(path), path)
  if (!ok) {
    feedback.toast(
      'error',
      'Discard Outcome Unconfirmed',
      'The input was kept. Reconcile the backup before trying again.',
    )
    return
  }
  documents.discard(target, path)
}
const buffer = computed(() => documents.get(workspace.active?.path ?? '', filePath.value))
const content = computed({
  get: () => buffer.value.content,
  set: (value) => {
    documents.edit(workspace.active?.path ?? '', filePath.value, value)
  },
})
const loaded = ref(false)
/** Non-empty when the current file could not be opened (binary/too large). */
const error = ref('')
/** Whether the current file is shown as dirty (drives save + tab dot). */
const dirty = computed(() => documents.dirty(buffer.value))
watch(filePath, () => {
  preview.value = false
  diffing.value = false
  baseline.value = ''
  baselineSnapshot.value = ''
  compareSnapshot.value = ''
})
/** Last content observed on disk, kept for the conflict banner's reload. */
/** Whether the active file changed on disk while local edits are pending. */
const conflicted = computed(() => buffer.value.conflict)

const editorRef = ref<InstanceType<typeof CodeEditor>>()
const pane = ref<HTMLElement>()
/** Whether markdown is shown rendered instead of as source. */
const preview = ref(false)
/** Markdown source (`.md` / `.markdown`) can be read rendered. */
const isMarkdown = computed(() => /\.(md|markdown)$/i.test(filePath.value))
/** Rendered preview HTML, recomputed only when the text changes. */
const previewHtml = computed(() => (preview.value ? renderMarkdown(content.value) : ''))

/** Side-by-side comparison with a snapshot's recorded content. */
const diffing = ref(false)
const baseline = ref('')
const baselineSnapshot = ref('')
/** Snapshot chosen for the comparison (empty = the latest one). */
const compareSnapshot = ref('')
const snapshots = ref<Array<{ id: string; alias: string; description: string }>>([])

/** Loads the baseline text for the comparison view. */
async function loadBaseline() {
  const ws = workspace.active
  if (!ws) return
  const r = await gateway.getFileAtSnapshot(
    ws.path,
    filePath.value,
    compareSnapshot.value || undefined,
  )
  if (!r.ok) {
    feedback.toast('error', 'Compare failed', r.error)
    diffing.value = false
    return
  }
  // A file the snapshot did not track compares against nothing, which reads as
  // "everything here is new" — the useful answer, not an error.
  baseline.value = r.data.content
  baselineSnapshot.value = r.data.snapshotId
}

/** Toggles the comparison, loading the snapshot list on first use. */
async function toggleDiff() {
  const ws = workspace.active
  if (!ws) return
  if (diffing.value) {
    diffing.value = false
    return
  }
  if (!snapshots.value.length) {
    const r = await gateway.listSnapshots(ws.path)
    if (r.ok) {
      snapshots.value = r.data.map((s) => ({
        id: s.id,
        alias: s.alias ?? '',
        // `message` is the snapshot's description on the wire.
        description: s.message,
      }))
    }
  }
  await loadBaseline()
  if (baselineSnapshot.value || baseline.value) diffing.value = true
}

watch(compareSnapshot, () => {
  if (diffing.value) void loadBaseline()
})

const fileName = computed(() => {
  if (buffer.value.resource) return `${buffer.value.resource.content.header.name}.md`
  const seg = filePath.value.split(/[\\/]/).filter(Boolean)
  return seg[seg.length - 1] ?? filePath.value
})

/** Map a file path to an editor language token consumed by `CodeEditor`. */
function languageOf(path: string): string {
  const ext = path.split('.').pop() ?? ''
  const map: Record<string, string> = {
    mbp: 'mbp',
    json: 'json',
    toml: 'toml',
    ts: 'ts',
    tsx: 'ts',
    js: 'js',
    jsx: 'js',
    mjs: 'js',
    cjs: 'js',
    html: 'html',
    htm: 'html',
    css: 'css',
    scss: 'scss',
    less: 'less',
  }
  return map[ext] ?? (isMarkdown.value ? 'markdown' : 'plaintext')
}

let loadRevision = 0
async function load() {
  const revision = ++loadRevision
  const ws = workspace.active
  const path = filePath.value
  if (!ws || isBlueprint.value) {
    loaded.value = true
    error.value = ''
    return
  }
  loaded.value = false
  error.value = ''
  try {
    const doc = await documents.open(ws.path, path)
    // A slower read from the previously selected tab must not replace the
    // current buffer, including replacing it with an empty file.
    if (revision !== loadRevision) return
    error.value = doc.loaded ? '' : doc.error
  } catch (cause) {
    if (revision !== loadRevision) return
    error.value = cause instanceof Error ? cause.message : String(cause)
  } finally {
    if (revision === loadRevision) loaded.value = true
  }
}

/** Save the active file and refresh the explorer listing of its folder. */
async function handleSave() {
  return saveBuffer()
}
async function saveBuffer() {
  const ws = workspace.active
  if (!ws || isBlueprint.value) return
  const path = filePath.value
  if (await documents.save(ws.path, path)) workspace.invalidateDir(ws.path, dirOf(path))
  else {
    feedback.toast('error', 'Save Failed', documents.get(ws.path, path).error)
    await documents.sync(ws.path, path)
  }
}

/** Discard local edits and adopt the on-disk content. */
async function reloadFromDisk() {
  await reviewResourceConflict()
}
async function reviewResourceConflict() {
  const ws = workspace.active?.path,
    path = filePath.value
  if (!ws) return
  if (!(await documents.sync(ws, path))) {
    feedback.toast('error', 'Could Not Read Saved Version', documents.get(ws, path).error)
    return
  }
  if (workspace.active?.path !== ws || filePath.value !== path) return
  conflictReview.value = documents.review(ws, path)
}

/** Detect disk changes for the active file (auto-adopt when clean, flag when
 *  the buffer is dirty so the user can resolve and save manually). */
async function syncWithDisk() {
  const ws = workspace.active
  if (!ws || isBlueprint.value || !filePath.value || !loaded.value) return
  await documents.sync(ws.path, filePath.value)
}

function onWindowFocus() {
  void syncWithDisk()
}
function onVisibility() {
  if (document.visibilityState === 'visible') void syncWithDisk()
}

/** Take over Ctrl/Cmd+S so the browser's "save page" dialog never appears. */
function onGlobalKeydown(e: KeyboardEvent) {
  const owner = (e.target as HTMLElement | null)?.closest?.('[data-editor-pane]')
  if (owner ? owner !== pane.value : props.filePath) return
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's') {
    e.preventDefault()
    if (dirty.value && !isBlueprint.value) void handleSave()
  }
}

onMounted(() => {
  // Unknown/stale file hash: leave the paneless editor for the Explorer.
  if (!filePath.value) {
    router.replace(wurl('/explorer'))
    return
  }
  // Deep-link refresh: re-register the tab so the strip matches the editor.
  // The split pane shows a file the user opened deliberately next to the main
  // pane, so it must not add or activate a tab: doing so would move the tab
  // highlight away from whatever the primary pane is showing.
  window.addEventListener('keydown', onGlobalKeydown)
  window.addEventListener('focus', onWindowFocus)
  document.addEventListener('visibilitychange', onVisibility)
})
onBeforeUnmount(() => {
  loadRevision += 1
  window.removeEventListener('keydown', onGlobalKeydown)
  window.removeEventListener('focus', onWindowFocus)
  document.removeEventListener('visibilitychange', onVisibility)
})
watch(
  [filePath, () => workspace.active?.path],
  () => {
    conflictReview.value = null
    if (!props.filePath && filePath.value && workspace.active) tabs.openFile(filePath.value)
    void load()
  },
  { immediate: true },
)

// Disk sync is event-driven: react only when the daemon reports this exact
// file changed on disk (plus window focus as a fallback).
watch(
  () => fileWatch.events,
  (list) => {
    if (!fileWatch.active || isBlueprint.value) return
    if (list.some((e) => e.path === filePath.value)) void syncWithDisk()
  },
)

/** Editor change → update content and mark the tab dirty (per path). */
function onEdit(value: string) {
  content.value = value
}

function handleUndo() {
  editorRef.value?.undo()
}
function handleRedo() {
  editorRef.value?.redo()
}

/** Editor undo/redo availability, forwarded from CodeEditor (item 9). */
const canUndo = ref(false)
const canRedo = ref(false)
function onHistory(u: boolean, r: boolean) {
  canUndo.value = u
  canRedo.value = r
}

/** Open this file's version history in the closable right-hand panel (item 11). */
function openVersionPanel() {
  if (filePath.value)
    rightPanel.show(FileVersionPanel, 'Version History', { filePath: filePath.value })
}

/** Compile the open `.mbp` DSL text into a visual blueprint file next to it
 *  and open that file on the canvas. The DSL file itself is left untouched. */
async function compileToBlueprint() {
  const ws = workspace.active
  if (!ws || !filePath.value) return
  const sourcePath = filePath.value
  const r = await gateway.compileDsl(content.value, ws.path)
  if (workspace.active?.path !== ws.path || filePath.value !== sourcePath) return
  if (!r.ok) {
    feedback.toast('error', 'DSL compile failed', r.error)
    return
  }
  const name =
    sourcePath
      .split('/')
      .pop()
      ?.replace(/\.mbp$/i, '') || 'blueprint'
  const dir = dirOf(sourcePath)
  const target = dir ? `${dir}/${name}.blueprint` : `${name}.blueprint`
  const ok = await gateway.writeFile(
    ws.path,
    target,
    JSON.stringify({ ...r.data, name }, null, 2),
    { createOnly: true },
  )
  if (workspace.active?.path !== ws.path || filePath.value !== sourcePath) return
  if (!ok.ok) {
    feedback.toast('error', 'Write Blueprint Failed', ok.error)
    return
  }
  workspace.invalidateDir(ws.path, dirOf(target))
  tabs.openFile(target)
  feedback.toast('success', 'Blueprint created', target)
  router.push(fileRoute(target))
}
</script>

<template>
  <div ref="pane" data-editor-pane class="flex h-full flex-col bg-background">
    <div
      v-if="loaded && filePath"
      class="flex shrink-0 flex-wrap items-center gap-3 border-b border-border px-3 py-1.5 text-[13px]"
    >
      <span
        v-if="backup"
        role="status"
        :title="backup.error"
        :class="backup.state === 'backed_up' ? 'text-muted-foreground' : 'text-status-warning'"
        >{{ backupLabel }}</span
      >
      <button
        v-if="backup && !['backed_up', 'backing_up'].includes(backup.state)"
        class="btn"
        type="button"
        @click="
          recovery.flush(workspace.active?.path ?? '', documents.recoveryKind(filePath), filePath)
        "
      >
        Back Up
      </button>
      <button class="btn" type="button" @click="recoveryOpen = true">Recovery Copies</button>
      <details v-if="dirty || backup?.error" class="relative">
        <summary class="cursor-pointer text-muted-foreground">Draft Options</summary>
        <div
          class="absolute left-0 top-full z-30 mt-2 w-72 rounded-lg border border-border bg-popover p-3 shadow-popover"
        >
          <p v-if="backup?.error" class="mb-3 break-words text-[13px] text-muted-foreground">
            {{ backup.error }}
          </p>
          <button v-if="dirty" class="btn" type="button" @click="discardRecovery">
            Discard Draft
          </button>
        </div>
      </details>
    </div>
    <RecoveryCopies
      v-if="recoveryOpen"
      :key="`${workspace.active?.path}:${filePath}`"
      :workspace="workspace.active?.path ?? ''"
      :scope="recovery.scopeFor(filePath)"
      :entity="filePath"
      @close="recoveryOpen = false"
      @restore="restoreRecovery"
    />
    <p v-if="!loaded" class="px-4 py-3 text-[13px] text-muted-foreground" role="status">
      Loading {{ fileName }}…
    </p>
    <!-- Blueprint files are edited on the visual canvas. -->
    <template v-if="isBlueprint && loaded">
      <BlueprintView :file-path="filePath" />
    </template>

    <!-- Unsupported (binary) or oversized files show a plain hint instead of
         a stale or broken editor buffer. -->
    <template v-else-if="loaded && error">
      <div class="flex h-full flex-col items-center justify-center gap-2 px-6 text-center">
        <p class="text-[13px] font-medium text-foreground">Cannot open {{ fileName }}</p>
        <p class="max-w-sm text-[12px] text-muted-foreground">{{ error }}</p>
      </div>
    </template>

    <!-- Everything else: a code editor with an icon-only toolbar. -->
    <template v-else-if="loaded">
      <div class="flex h-10 shrink-0 items-center gap-3 border-b border-divider px-2">
        <span class="ml-1 mr-auto min-w-0 truncate text-[13px] font-medium">{{ fileName }}</span>
        <template v-if="buffer.resource">
          <span class="text-[11px] text-subtle"
            >{{ buffer.resource.reference.scope.scope }} · r{{
              buffer.resource.reference.revision
            }}</span
          >
          <button class="btn btn-outline" :disabled="dirty" @click="usePromptInChat">
            Use in Chat
          </button>
        </template>
        <span
          v-if="dirty"
          class="mr-1 h-1.5 w-1.5 shrink-0 rounded-full"
          style="background: var(--primary)"
          title="Unsaved changes"
        />
        <button
          v-if="isDsl"
          class="btn btn-primary h-7! shrink-0"
          type="button"
          title="Compile DSL into a visual blueprint"
          :aria-label="'Compile to blueprint'"
          @click="compileToBlueprint"
        >
          <Workflow class="h-3.5 w-3.5" /> To Blueprint
        </button>
        <button
          class="editor-tool-icon"
          type="button"
          title="Save"
          :aria-label="'Save'"
          :disabled="!dirty || buffer.saving"
          @click="handleSave"
        >
          <Save class="h-4 w-4" />
        </button>
        <button
          class="editor-tool-icon"
          type="button"
          title="Undo"
          aria-label="Undo"
          :disabled="!canUndo"
          @click="handleUndo"
        >
          <Undo2 class="h-4 w-4" />
        </button>
        <button
          class="editor-tool-icon"
          type="button"
          title="Redo"
          aria-label="Redo"
          :disabled="!canRedo"
          @click="handleRedo"
        >
          <Redo2 class="h-4 w-4" />
        </button>
        <button
          v-if="!buffer.resource"
          class="editor-tool-icon"
          type="button"
          title="Version history"
          aria-label="Version"
          @click="openVersionPanel"
        >
          <GitBranch class="h-4 w-4" />
        </button>
        <button
          v-if="!buffer.resource"
          class="editor-tool-icon"
          :class="diffing ? 'text-primary' : ''"
          type="button"
          :title="diffing ? 'Hide comparison' : 'Compare with snapshot'"
          :aria-label="diffing ? 'Hide comparison' : 'Compare with snapshot'"
          :aria-pressed="diffing"
          @click="toggleDiff"
        >
          <GitCompare class="h-4 w-4" />
        </button>
        <button
          v-if="isMarkdown"
          class="editor-tool-icon"
          :class="preview ? 'text-primary' : ''"
          type="button"
          :title="preview ? 'Show markdown source' : 'Preview markdown'"
          :aria-label="preview ? 'Show markdown source' : 'Preview markdown'"
          :aria-pressed="preview"
          @click="preview = !preview"
        >
          <Eye v-if="!preview" class="h-4 w-4" />
          <FileCode2 v-else class="h-4 w-4" />
        </button>
      </div>

      <div
        v-if="buffer.deleted"
        class="flex shrink-0 items-center gap-2 border-b border-divider bg-danger-soft px-3 py-2 text-[12px] text-danger"
        role="status"
      >
        <TriangleAlert class="h-3.5 w-3.5" />
        <span class="flex-1">This file was deleted. Your buffer is preserved.</span>
        <button type="button" class="underline" :disabled="buffer.saving" @click="handleSave">
          Save to Restore
        </button>
      </div>
      <!-- Conflict banner: local edits + disk changed → resolve manually. -->
      <div
        v-if="conflicted"
        class="flex shrink-0 items-center gap-2 border-b border-divider px-3 py-2 text-[13px]"
        style="background: var(--danger-soft); color: var(--danger)"
      >
        <TriangleAlert class="h-3.5 w-3.5 shrink-0" />
        <span class="min-w-0 truncate">File changed on disk while you have unsaved changes</span>
        <button
          class="shrink-0 underline underline-offset-2 hover:opacity-80"
          type="button"
          @click="reloadFromDisk"
        >
          Reload
        </button>
        <button
          v-if="!buffer.resource"
          class="shrink-0 underline underline-offset-2 hover:opacity-80"
          type="button"
          @click="reviewResourceConflict"
        >
          Compare Changes
        </button>
        <button
          v-else
          type="button"
          class="shrink-0 underline underline-offset-2"
          @click="reviewResourceConflict"
        >
          Review Revision
        </button>
      </div>

      <div v-if="buffer.readOnlyReason" class="min-h-0 flex-1 overflow-auto p-4 text-[13px]">
        <p role="status" class="mb-3">{{ buffer.readOnlyReason }}</p>
        <pre class="whitespace-pre-wrap">{{ content }}</pre>
      </div>
      <div v-else-if="diffing" class="flex min-h-0 flex-1 flex-col">
        <div
          class="flex shrink-0 items-center gap-2 border-b border-divider px-3 py-1.5 text-[11.5px] text-subtle"
        >
          <span>Compare with</span>
          <select v-model="compareSnapshot" class="input h-6! py-0! text-[11.5px]">
            <option value="">latest snapshot</option>
            <option v-for="snapshot in snapshots" :key="snapshot.id" :value="snapshot.id">
              {{ snapshot.alias || snapshot.description || snapshot.id.slice(0, 8) }}
            </option>
          </select>
          <span v-if="!snapshots.length">no snapshots yet — showing an empty baseline</span>
        </div>
        <div class="min-h-0 flex-1">
          <FileDiffView
            :file-path="filePath"
            :original="baseline"
            :modified="content"
            :snapshot-id="baselineSnapshot"
          />
        </div>
      </div>
      <div v-else-if="preview" class="min-h-0 flex-1 overflow-auto px-6 py-5">
        <div
          class="md-body mx-auto max-w-3xl text-[13.5px] leading-6 text-foreground"
          v-html="previewHtml"
        />
      </div>
      <div v-else class="min-h-0 flex-1 overflow-hidden">
        <CodeEditor
          :key="modelId"
          ref="editorRef"
          :model-value="content"
          :language="languageOf(filePath)"
          :model-id="modelId"
          :view-target="{ workspace: workspace.active?.path ?? '', path: filePath }"
          @update:model-value="onEdit"
          @history="onHistory"
        />
      </div>
    </template>
    <DocumentConflictDialog
      v-if="conflictReview"
      :review="conflictReview"
      @close="conflictReview = null"
      @saved="(ws, path) => workspace.invalidateDir(ws, dirOf(path))"
    />
  </div>
</template>
