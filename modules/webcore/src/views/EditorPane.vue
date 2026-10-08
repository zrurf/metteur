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
import { wurl } from '@/lib/workspace-url'

const route = useRoute()
const router = useRouter()
const workspace = useWorkspaceStore()
const tabs = useTabsStore()
const rightPanel = useRightPanelStore()
const fileWatch = useFileWatchStore()
const feedback = useFeedbackStore()

/** An explicit path renders this pane as the split view's right-hand side. */
const props = defineProps<{ filePath?: string }>()

/** The route-driven path comes from the `?f=` hash, resolved via the persisted
 *  hash→path map — never from raw URL bytes (no path traversal surface). */
const routePath = computed(() => pathByFileParam(String(route.query.f ?? '')))
const filePath = computed(() => props.filePath || routePath.value)
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
const modelId = computed(
  () => (filePath.value ? `${workspace.active?.path ?? ''}::${filePath.value}` : ''),
)

const content = ref('')
const loaded = ref(false)
/** Non-empty when the current file could not be opened (binary/too large). */
const error = ref('')
/** Whether the current file is shown as dirty (drives save + tab dot). */
const dirty = computed(() => tabs.isDirty(filePath.value))
watch(filePath, () => {
  preview.value = false
  diffing.value = false
  baseline.value = ''
  baselineSnapshot.value = ''
  compareSnapshot.value = ''
})
/** Last content observed on disk, kept for the conflict banner's reload. */
const onDiskContent = ref('')
/** Whether the active file changed on disk while local edits are pending. */
const conflicted = computed(() => tabs.isConflict(filePath.value))

const editorRef = ref<InstanceType<typeof CodeEditor>>()
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
  return map[ext] ?? 'plaintext'
}

/** Pretty-print JSON when opening an editor for a `.json` file. */
function normalize(language: string, text: string): string {
  if (language !== 'json') return text
  try {
    return JSON.stringify(JSON.parse(text), null, 2)
  } catch {
    return text
  }
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
    const r = await gateway.readFile(ws.path, path)
    // A slower read from the previously selected tab must not replace the
    // current buffer, including replacing it with an empty file.
    if (revision !== loadRevision) return
    if (r.ok) content.value = normalize(languageOf(path), r.data.content)
    else {
      content.value = ''
      error.value = r.error
    }
  } catch (cause) {
    if (revision !== loadRevision) return
    content.value = ''
    error.value = cause instanceof Error ? cause.message : String(cause)
  } finally {
    if (revision === loadRevision) loaded.value = true
  }
}

/** Save the active file and refresh the explorer listing of its folder. */
async function handleSave() {
  const ws = workspace.active
  if (!ws || isBlueprint.value) return
  const r = await gateway.writeFile(ws.path, filePath.value, content.value)
  if (r.ok) {
    tabs.markDirty(filePath.value, false)
    tabs.markConflict(filePath.value, false)
    onDiskContent.value = content.value
    workspace.invalidateDir(ws.path, dirOf(filePath.value))
  }
}

/** Discard local edits and adopt the on-disk content. */
function reloadFromDisk() {
  content.value = onDiskContent.value
  tabs.markDirty(filePath.value, false)
  tabs.markConflict(filePath.value, false)
}

/** Detect disk changes for the active file (auto-adopt when clean, flag when
 *  the buffer is dirty so the user can resolve and save manually). */
async function syncWithDisk() {
  const ws = workspace.active
  if (!ws || isBlueprint.value || !filePath.value || !loaded.value) return
  const r = await gateway.readFile(ws.path, filePath.value)
  if (!r.ok) return
  const disk = normalize(languageOf(filePath.value), r.data.content)
  if (disk === content.value) return
  onDiskContent.value = disk
  if (dirty.value) tabs.markConflict(filePath.value, true)
  else {
    tabs.markConflict(filePath.value, false)
    content.value = disk
  }
}

function onWindowFocus() {
  void syncWithDisk()
}
function onVisibility() {
  if (document.visibilityState === 'visible') void syncWithDisk()
}

/** Take over Ctrl/Cmd+S so the browser's "save page" dialog never appears. */
function onGlobalKeydown(e: KeyboardEvent) {
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
  if (!props.filePath) tabs.openFile(filePath.value)
  void load()
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
watch(filePath, () => void load())

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
  tabs.markDirty(filePath.value, true)
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
  if (filePath.value) rightPanel.show(FileVersionPanel, 'Version History', { filePath: filePath.value })
}

/** Compile the open `.mbp` DSL text into a visual blueprint file next to it
 *  and open that file on the canvas. The DSL file itself is left untouched. */
async function compileToBlueprint() {
  const ws = workspace.active
  if (!ws || !filePath.value) return
  const r = await gateway.compileDsl(content.value, ws.path)
  if (!r.ok) {
    feedback.toast('error', 'DSL compile failed', r.error)
    return
  }
  const name = fileName.value.replace(/\.mbp$/i, '') || 'blueprint'
  const dir = dirOf(filePath.value)
  const target = dir ? `${dir}/${name}.blueprint` : `${name}.blueprint`
  const ok = await gateway.writeFile(
    ws.path,
    target,
    JSON.stringify({ ...r.data, name }, null, 2),
  )
  if (!ok) {
    feedback.toast('error', 'Write blueprint failed', target)
    return
  }
  workspace.invalidateDir(ws.path, dirOf(target))
  tabs.openFile(target)
  feedback.toast('success', 'Blueprint created', target)
  router.push(fileRoute(target))
}
</script>

<template>
  <div class="flex h-full flex-col bg-background">
    <p v-if="!loaded" class="px-4 py-3 text-[13px] text-muted-foreground" role="status">Loading {{ fileName }}…</p>
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
          :disabled="!dirty"
          @click="handleSave"
        >
          <Save class="h-4 w-4" />
        </button>
        <button class="editor-tool-icon" type="button" title="Undo" aria-label="Undo" :disabled="!canUndo" @click="handleUndo">
          <Undo2 class="h-4 w-4" />
        </button>
        <button class="editor-tool-icon" type="button" title="Redo" aria-label="Redo" :disabled="!canRedo" @click="handleRedo">
          <Redo2 class="h-4 w-4" />
        </button>
        <button
          class="editor-tool-icon"
          type="button"
          title="Version history"
          aria-label="Version"
          @click="openVersionPanel"
        >
          <GitBranch class="h-4 w-4" />
        </button>
        <button
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

      <!-- Conflict banner: local edits + disk changed → resolve manually. -->
      <div
        v-if="conflicted"
        class="flex shrink-0 items-center gap-2 border-b border-divider px-3 py-1.5 text-[11.5px]"
        style="background: var(--danger-soft); color: var(--danger)"
      >
        <TriangleAlert class="h-3.5 w-3.5 shrink-0" />
        <span class="min-w-0 truncate">File changed on disk while you have unsaved changes</span>
        <button class="shrink-0 underline underline-offset-2 hover:opacity-80" type="button" @click="reloadFromDisk">Reload</button>
        <button class="shrink-0 underline underline-offset-2 hover:opacity-80" type="button" @click="handleSave">Overwrite</button>
      </div>

      <div
        v-if="diffing"
        class="flex min-h-0 flex-1 flex-col"
      >
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
        <div class="md-body mx-auto max-w-3xl text-[13.5px] leading-6 text-foreground" v-html="previewHtml" />
      </div>
      <div v-else class="min-h-0 flex-1 overflow-hidden">
        <CodeEditor
          :key="filePath"
          ref="editorRef"
          v-model="content"
          :language="languageOf(filePath)"
          :model-id="modelId"
          @update:model-value="onEdit"
          @history="onHistory"
        />
      </div>
    </template>
  </div>
</template>
