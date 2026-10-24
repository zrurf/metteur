<script setup lang="ts">
import { onBeforeUnmount, onMounted, provide, reactive, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { ClipboardPaste, FilePlus2, BookOpen, FolderPlus, RefreshCw } from '@lucide/vue'
import { gateway } from '@/core'
import type { FileTreeNode } from '@/core'
import { useWorkspaceStore } from '@/stores/workspace'
import { useDocumentsStore } from '@/stores/documents'
import TreeNameInput from './TreeNameInput.vue'
import { useTabsStore } from '@/stores/tabs'
import { useChatStore } from '@/stores/chat'
import { useBlueprintStore } from '@/stores/blueprint'
import { useFeedbackStore } from '@/stores/feedback'
import { useFileWatchStore } from '@/stores/filewatch'
import { useFileClipboardStore } from '@/stores/fileclip'
import { fileRoute } from '@/lib/file-token'
import { useSurfaceNavigation } from '@/lib/surface'
import { TREE_API, TREE_CREATE, TREE_RENAME } from '@/lib/tree'
import TreeItem from '@/components/TreeItem.vue'
import ContextMenu, { type MenuGroup, type MenuItem } from '@/components/ContextMenu.vue'
import FilePropertiesModal from '@/components/FilePropertiesModal.vue'
import { dirOf } from '@/lib/path'
import { wurl } from '@/lib/workspace-url'

/**
 * Left-side resource explorer.
 *
 * Loads the top-level listing lazily and exposes an API (expand, open file,
 * active highlight) through TREE_API. Every row — at any depth — is the
 * recursive TreeItem component. The toolbar offers New File / New Folder /
 * Refresh; those plus in-place creation also live on the blank-area and
 * folder context menus.
 */

const workspace = useWorkspaceStore()
const tabs = useTabsStore()
const chat = useChatStore()
const blueprint = useBlueprintStore()
const router = useRouter()
const route = useRoute()
const feedback = useFeedbackStore()
const fileWatch = useFileWatchStore()

const root = ref<FileTreeNode[]>([])
const expanded = ref<Set<string>>(new Set())
const loading = ref(false)
const loadError = ref('')
let loadGeneration = 0
const menu = ref<{ x: number; y: number; node: FileTreeNode | null } | null>(null)
/** Multi-selection (Ctrl/Shift): paths of the currently highlighted rows. */
const selected = ref<Set<string>>(new Set())
/** Anchor for Shift-range selection, in visible row order. */
let lastSelectedPath: string | null = null
/** New-file/folder name input, anchored to the toolbar. */
const creating = ref<{ kind: 'file' | 'folder'; dir: string } | null>(null)
const newName = ref('')
const createError = ref('')
const creatingBusy = ref(false)
const { openSurface } = useSurfaceNavigation()

/** Loads (or reuses the cached) listing for `dir`; fills `into.children` too. */
async function loadDir(dir: string, into: FileTreeNode | null): Promise<FileTreeNode[] | null> {
  const wsPath = workspace.active?.path ?? ''
  const generation = loadGeneration
  const cached = workspace.cachedDir(wsPath, dir)
  if (cached) {
    if (into) into.children = cached
    return cached
  }
  const r = await gateway.listFiles(wsPath, dir)
  if (workspace.active?.path !== wsPath || generation !== loadGeneration) return null
  if (!r.ok) {
    loadError.value = r.error
    return null
  }
  if (into) into.children = r.data
  workspace.cacheDir(wsPath, dir, r.data)
  return r.data
}

async function loadRoot() {
  const generation = ++loadGeneration,
    wsPath = workspace.active?.path
  loading.value = true
  loadError.value = ''
  try {
    const nodes = await loadDir('', null)
    if (nodes && generation === loadGeneration && workspace.active?.path === wsPath)
      root.value = nodes
  } finally {
    if (generation === loadGeneration) loading.value = false
  }
}

async function toggle(node: FileTreeNode) {
  if (node.kind !== 'dir') return
  const willOpen = !expanded.value.has(node.path)
  const wsPath = workspace.active?.path
  const generation = loadGeneration
  if (willOpen && node.children === undefined) await loadDir(node.path, node)
  if (workspace.active?.path !== wsPath || generation !== loadGeneration) return
  if (expanded.value.has(node.path)) expanded.value.delete(node.path)
  else expanded.value.add(node.path)
  expanded.value = new Set(expanded.value)
}

function openFile(node: FileTreeNode) {
  if (node.kind !== 'file') return
  tabs.openFile(node.path)
  router.push(fileRoute(node.path))
}

/** Row highlight for selection (multi-select) or the open file. */
function rowCls(path: string): string {
  return selected.value.has(path) || tabs.activeFilePath === path ? 'bg-accent text-foreground' : ''
}

/** Visible rows in render order, for Shift-range selection. */
function visibleOrder(): string[] {
  const out: string[] = []
  const walk = (nodes: FileTreeNode[]) => {
    for (const n of nodes) {
      out.push(n.path)
      if (n.kind === 'dir' && expanded.value.has(n.path) && n.children) walk(n.children)
    }
  }
  walk(root.value)
  return out
}

/** Row click: plain selects-and-acts, Ctrl toggles, Shift spans the range. */
function onRowSelect(node: FileTreeNode, e: MouseEvent) {
  const path = node.path
  if (e.shiftKey && lastSelectedPath && lastSelectedPath !== path) {
    const order = visibleOrder()
    const a = order.indexOf(lastSelectedPath)
    const b = order.indexOf(path)
    if (a !== -1 && b !== -1) {
      const next = new Set(selected.value)
      const [lo, hi] = a < b ? [a, b] : [b, a]
      for (let i = lo; i <= hi; i++) next.add(order[i])
      selected.value = next
    }
  } else if (e.ctrlKey || e.metaKey) {
    const next = new Set(selected.value)
    if (next.has(path)) next.delete(path)
    else next.add(path)
    selected.value = next
  } else {
    selected.value = new Set([path])
    if (node.kind === 'dir') toggle(node)
    else openFile(node)
  }
  lastSelectedPath = path
}

/** Targets of a context action: the whole selection when the clicked row is
 *  part of a multi-selection, otherwise that row alone. */
function menuTargets(): string[] {
  const node = menu.value?.node
  if (node && selected.value.has(node.path) && selected.value.size > 1) return [...selected.value]
  return node ? [node.path] : []
}

/** Refresh listing for the root and every expanded folder. */
async function refreshTree() {
  const ws = workspace.active
  if (!ws) return
  workspace.invalidateTree(ws.path)
  await loadRoot()
  if (workspace.active?.path !== ws.path) return
  await reloadExpanded(root.value)
}

async function reloadExpanded(nodes: FileTreeNode[]) {
  for (const n of nodes) {
    if (n.kind !== 'dir' || !expanded.value.has(n.path)) continue
    await loadDir(n.path, n)
    await reloadExpanded(n.children ?? [])
  }
}

function findNode(path: string, nodes: FileTreeNode[]): FileTreeNode | null {
  for (const n of nodes) {
    if (n.path === path) return n
    if (n.children) {
      const hit = findNode(path, n.children)
      if (hit) return hit
    }
  }
  return null
}

function startCreate(kind: 'file' | 'folder', dir: string) {
  createError.value = ''
  creating.value = { kind, dir }
  newName.value = ''
  // Reveal the target folder so the inline row is visible at its level.
  if (dir) {
    const node = findNode(dir, root.value)
    if (node && !expanded.value.has(dir)) void toggle(node)
  }
}

function cancelCreate() {
  creating.value = null
}

/** Create at `dir`; conflicts and empty names are rejected with a toast. */
async function confirmCreate() {
  const ws = workspace.active
  if (!ws || !creating.value || creatingBusy.value) return
  const { kind, dir } = creating.value
  const intent = creating.value
  const name = newName.value.trim()
  if (
    !name ||
    /[\\/:*?"<>|]/.test(name) ||
    /^\.+$/.test(name) ||
    /[. ]$/.test(name) ||
    /^(con|prn|aux|nul|com[1-9]|lpt[1-9])(\.|$)/i.test(name)
  ) {
    createError.value = 'Enter a valid file or folder name.'
    return
  }
  const path = dir ? `${dir}/${name}` : name
  if (await exists(ws.path, dir, path)) {
    if (workspace.active?.path !== ws.path || creating.value !== intent) return
    createError.value = `${name} already exists.`
    return
  }
  if (workspace.active?.path !== ws.path || creating.value !== intent) return
  creatingBusy.value = true
  const r =
    kind === 'folder'
      ? await gateway.createDir(ws.path, path)
      : await gateway.writeFile(ws.path, path, '', { createOnly: true })
  if (workspace.active?.path !== ws.path || creating.value !== intent) return
  creatingBusy.value = false
  if (!r.ok) {
    createError.value = r.error
    return
  }
  creating.value = null
  if (kind === 'file') {
    tabs.openFile(path)
    router.push(fileRoute(path))
  }
  await refreshTree()
}

/** Whether a path already exists in the (possibly cached) listing of `dir`. */
async function exists(wsPath: string, dir: string, path: string): Promise<boolean> {
  const cached = workspace.cachedDir(wsPath, dir)
  if (cached) return cached.some((n) => n.path === path)
  const r = await gateway.listFiles(wsPath, dir)
  return r.ok ? r.data.some((n) => n.path === path) : false
}

/* Clipboard (copy / cut / paste) -------------------------------------- */

const clip = useFileClipboardStore()

/** Base file-name of a workspace-relative path. */
function baseName(path: string): string {
  const seg = path.split(/[\\/]/).filter(Boolean)
  return seg[seg.length - 1] ?? path
}

/** Recursively copies a file or directory tree (daemon has no copy RPC). */
async function copyPath(ws: string, from: string, to: string): Promise<boolean> {
  if (to === from || to.startsWith(from + '/')) return false
  const st = await gateway.statFile(ws, from)
  if (st.ok && st.data.isDir) {
    if (!(await gateway.createDir(ws, to)).ok) return false
    const list = await gateway.listFiles(ws, from)
    if (!list.ok) return false
    for (const entry of list.data) {
      if (!(await copyPath(ws, `${from}/${entry.name}`, `${to}/${entry.name}`))) return false
    }
    return true
  }
  const r = await gateway.readFile(ws, from)
  if (!r.ok) return false
  return (await gateway.writeFile(ws, to, r.data.content, { createOnly: true })).ok
}

/** First name inside `dir` that does not collide yet (`name`, `name (1)`, …). */
async function uniqueDest(ws: string, dir: string, name: string): Promise<string> {
  const dot = name.lastIndexOf('.')
  const stem = dot > 0 ? name.slice(0, dot) : name
  const ext = dot > 0 ? name.slice(dot) : ''
  let candidate = dir ? `${dir}/${name}` : name
  let i = 1
  while (await exists(ws, dir, candidate)) {
    candidate = dir ? `${dir}/${stem} (${i})${ext}` : `${stem} (${i})${ext}`
    i++
  }
  return candidate
}

/** Cut = move (rename RPC); copy = recursive copy into the target folder. */
async function pasteInto(dir: string) {
  const ws = workspace.active
  if (!ws || !clip.active) return
  const op = clip.op
  const sources = [...clip.sources]
  clip.clear()
  for (const src of sources) {
    const name = baseName(src)
    if (op === 'cut') {
      const dest = dir ? `${dir}/${name}` : name
      if (dest === src) continue
      const r = await gateway.renameFile(ws.path, src, dest)
      if (!r.ok) feedback.toast('error', 'Cut failed', r.error)
      else if (workspace.active?.path === ws.path) tabs.renameFile(src, dest)
    } else {
      const dest = await uniqueDest(ws.path, dir, name)
      if (!(await copyPath(ws.path, src, dest))) feedback.toast('error', 'Copy failed', src)
    }
  }
  await refreshTree()
}

/* Rename (inline row editor) ------------------------------------------- */

const renamingPath = ref<string | null>(null)
const renameName = ref('')
function startRename(node: FileTreeNode) {
  renamingPath.value = node.path
  renameName.value = baseName(node.path)
}
async function commitRename() {
  const ws = workspace.active
  const path = renamingPath.value
  renamingPath.value = null
  if (!ws || !path) return
  const name = renameName.value.trim()
  const dir = dirOf(path)
  if (!name || /[\\/]/.test(name)) {
    feedback.toast('error', 'Invalid name', 'Name must be non-empty and contain no path separators')
    return
  }
  const target = dir ? `${dir}/${name}` : name
  if (target === path) return
  if (await exists(ws.path, dir, target)) {
    feedback.toast('error', `${name} already exists`)
    return
  }
  const r = await gateway.renameFile(ws.path, path, target)
  if (workspace.active?.path !== ws.path) return
  if (!r.ok) {
    feedback.toast('error', 'Rename failed', r.error)
    return
  }
  tabs.renameFile(path, target)
  if (tabs.activeItem?.path) void router.push(tabs.activeItem.route)
  await refreshTree()
}

/* Delete / properties / reveal ----------------------------------------- */

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`
  return `${(n / 1024 / 1024).toFixed(1)} MB`
}

/** Shows a modal with stat-derived metadata for the selected entry. */
const properties = ref<{ title: string; rows: { label: string; value: string }[] } | null>(null)
const propertiesName = ref('')
async function showProperties(node: FileTreeNode) {
  const ws = workspace.active
  if (!ws) return
  const r = await gateway.statFile(ws.path, node.path)
  const kind = node.kind === 'dir' ? 'Folder' : node.path.split('.').pop()?.toUpperCase() || 'File'
  const size = r.ok ? formatBytes(r.data.len) : '—'
  propertiesName.value = baseName(node.path)
  properties.value = {
    title: `${propertiesName.value} properties`,
    rows: [
      { label: 'Name', value: propertiesName.value },
      { label: 'Kind', value: kind },
      { label: 'Path', value: node.path },
      { label: 'Location', value: dirOf(node.path) || 'Workspace root' },
      { label: 'Size', value: size },
    ],
  }
}

async function revealNode(node: FileTreeNode) {
  const ws = workspace.active
  if (!ws) return
  const r = await gateway.revealInExplorer(ws.path, node.path)
  if (!r.ok) feedback.toast('error', 'Reveal failed', r.error)
}

function openMenu(x: number, y: number, node: FileTreeNode | null) {
  menu.value = { x, y, node }
}

/** Right-click on the tree's blank area (not on a row). */
function onBlankContext(e: MouseEvent) {
  if ((e.target as HTMLElement).closest('.tree-row')) return
  openMenu(e.clientX, e.clientY, null)
}

/** Row right-click forwarded by TreeItem. Right-clicking outside the current
 *  selection collapses it to that row so single-node actions stay predictable. */
function onRowContextMenu(node: FileTreeNode, event: MouseEvent) {
  if (!selected.value.has(node.path) || selected.value.size < 2) {
    selected.value = new Set([node.path])
    lastSelectedPath = node.path
  }
  openMenu(event.clientX, event.clientY, node)
}

function menuGroupsOf(node: FileTreeNode | null): MenuGroup[] {
  const onNode = node !== null
  const isFile = node?.kind === 'file'
  const main: MenuItem[] = []
  if (isFile) {
    main.push({ id: 'open', label: 'Open' })
    main.push({ id: 'open-split', label: 'Open in Split View' })
    main.push({ id: 'add-to-chat', label: 'Add to Conversation' })
    main.push({ id: 'add-to-blueprint', label: 'Add to Blueprint' })
  } else {
    main.push({ id: 'new-file', label: 'New File…' })
    main.push({ id: 'new-folder', label: 'New Folder…' })
    main.push({ id: 'refresh', label: 'Refresh' })
  }
  const clipboard: MenuItem[] = []
  if (onNode) {
    clipboard.push({ id: 'copy', label: 'Copy', hint: '' })
    clipboard.push({ id: 'cut', label: 'Cut' })
  }
  clipboard.push({ id: 'paste', label: 'Paste', hint: '' })
  const actions: MenuItem[] = []
  if (onNode) {
    actions.push({ id: 'rename', label: 'Rename' })
    actions.push({ id: 'delete', label: 'Delete' })
    actions.push({ id: 'reveal', label: 'Reveal in File Explorer' })
  }
  actions.push({ id: 'properties', label: 'Properties' })
  actions.push({ id: 'copy-path', label: 'Copy Path' })
  return [
    { label: isFile ? 'File' : 'Actions', items: main },
    { label: 'Clipboard', items: clipboard },
    { label: 'Actions', items: actions },
  ]
}

/**
 * Open a file in the split view's right-hand pane.
 *
 * The split lives in the editor area, so a surface route (Chat, Settings) is
 * first replaced by the explorer: otherwise the pane would be created off
 * screen. An already-open file is left in the primary pane, which is what
 * makes the split useful for comparing two files.
 */
function openInSplit(path: string) {
  if (!isEditorRoute()) openExplorer()
  tabs.openInSplit(path)
}

/** Whether the current route renders the editor area. */
function isEditorRoute(): boolean {
  const name = String(route.name ?? '')
  return name === 'explorer' || name === 'file'
}

/** Route the primary pane to the explorer (empty editor) state. */
function openExplorer() {
  void router.push(wurl('/explorer'))
}

async function onMenuSelect(id: string) {
  const node = menu.value?.node ?? null
  const targets = menuTargets()
  menu.value = null
  const ws = workspace.active
  switch (id) {
    case 'copy-path':
      if (node) await navigator.clipboard.writeText(node.path)
      return
    case 'new-file':
      startCreate('file', node?.kind === 'dir' ? node.path : '')
      return
    case 'new-folder':
      startCreate('folder', node?.kind === 'dir' ? node.path : '')
      return
    case 'refresh':
      void refreshTree()
      return
    case 'copy':
      if (ws && targets.length > 1) {
        clip.set('copy', targets)
        return
      }
      if (ws && node) clip.set('copy', [node.path])
      return
    case 'cut':
      if (ws && targets.length > 1) {
        clip.set('cut', targets)
        return
      }
      if (ws && node) clip.set('cut', [node.path])
      return
    case 'paste':
      // Paste lands in the clicked folder, the parent of a clicked file, or
      // the root for blank areas.
      void pasteInto(node ? (node.kind === 'dir' ? node.path : dirOf(node.path)) : '')
      return
  }
  if (!node) return
  switch (id) {
    case 'open':
      openFile(node)
      break
    case 'open-split':
      openInSplit(node.path)
      break
    case 'add-to-chat':
      if (node.kind !== 'file') return
      chat.attach(node.path)
      openSurface('chat')
      break
    case 'add-to-blueprint':
      blueprint.requestAddReference(node.path)
      break
    case 'rename':
      startRename(node)
      break
    case 'delete':
      void (async () => {
        const ok = await feedback.confirm({
          header: targets.length > 1 ? `Delete ${targets.length} items` : `Delete ${node?.kind}`,
          message:
            targets.length > 1
              ? `Delete ${targets.length} selected items? This cannot be undone.`
              : `Delete "${baseName(node.path)}"? This cannot be undone.`,
          acceptLabel: 'Delete',
          rejectLabel: 'Cancel',
          danger: true,
        })
        if (!ok || !ws) return
        for (const path of targets) {
          const r = await gateway.removeFile(ws.path, path)
          if (!r.ok) feedback.toast('error', 'Delete failed', r.error)
          else useDocumentsStore().markDeleted(ws.path, path)
        }
        selected.value = new Set()
        await refreshTree()
      })()
      break
    case 'properties':
      void showProperties(node)
      break
    case 'reveal':
      void revealNode(node)
      break
  }
}

provide(TREE_API, {
  isOpen: (path) => expanded.value.has(path),
  toggle,
  openFile,
  rowCls,
  select: onRowSelect,
  onRowContextMenu,
})
/** In-tree creation state, consumed by TreeItem to host the inline input. */
const createState = reactive({
  active: creating,
  name: newName,
  error: createError,
  busy: creatingBusy,
  confirm: () => void confirmCreate(),
  cancel: cancelCreate,
})
provide(TREE_CREATE, createState)

/** In-line rename state consumed by TreeItem to edit a row's name. */
const renameState = reactive({
  activePath: renamingPath,
  name: renameName,
  setName: (v: string) => (renameName.value = v),
  confirm: () => void commitRename(),
  cancel: () => {
    renamingPath.value = null
  },
})
provide(TREE_RENAME, renameState)

watch(
  () => workspace.active?.path,
  (path) => {
    loadGeneration++
    root.value = []
    expanded.value = new Set()
    selected.value = new Set()
    creating.value = null
    creatingBusy.value = false
    renamingPath.value = null
    menu.value = null
    if (path) void loadRoot()
  },
  { immediate: true, flush: 'sync' },
)
onMounted(() => {
  window.addEventListener('focus', onWindowFocus)
  // Refresh on live fs events (debounced bursts from external editors/CLI).
  watch(
    () => fileWatch.events,
    () => {
      if (!fileWatch.active || !workspace.hasActive) return
      clearTimeout(refreshTimer)
      refreshTimer = setTimeout(() => void refreshTree(), 150)
    },
  )
})
onBeforeUnmount(() => {
  loadGeneration++
  window.removeEventListener('focus', onWindowFocus)
  clearTimeout(refreshTimer)
})

/** Re-sync the listing with the disk when the window regains focus. */
function onWindowFocus() {
  if (workspace.hasActive) void refreshTree()
}

let refreshTimer: ReturnType<typeof setTimeout> | undefined
</script>

<template>
  <div class="flex h-full flex-col bg-sidebar" data-testid="file-tree">
    <!-- Toolbar (VSCode-style explorer header) -->
    <div class="flex shrink-0 items-center gap-0.5 border-b border-divider px-2 py-1">
      <span class="panel-heading flex-1 px-0!">Explorer</span>
      <button
        class="btn-icon h-6! w-6!"
        title="Prompt Library"
        aria-label="Prompt Library"
        @click="openSurface('prompts')"
      >
        <BookOpen class="h-3.5 w-3.5" />
      </button>
      <button
        class="btn-icon h-6! w-6!"
        type="button"
        title="New File"
        :aria-label="'New File'"
        @click="startCreate('file', '')"
      >
        <FilePlus2 class="h-3.5 w-3.5" />
      </button>
      <button
        class="btn-icon h-6! w-6!"
        type="button"
        title="New Folder"
        :aria-label="'New Folder'"
        @click="startCreate('folder', '')"
      >
        <FolderPlus class="h-3.5 w-3.5" />
      </button>
      <button
        v-if="clip.active"
        class="btn-icon h-6! w-6! text-primary!"
        type="button"
        title="Paste into workspace root"
        :aria-label="'Paste'"
        @click="pasteInto('')"
      >
        <ClipboardPaste class="h-3.5 w-3.5" />
      </button>
      <button
        class="btn-icon h-6! w-6!"
        type="button"
        title="Refresh"
        :aria-label="'Refresh'"
        @click="refreshTree"
      >
        <RefreshCw class="h-3.5 w-3.5" />
      </button>
    </div>

    <p v-if="loadError" class="m-2 rounded-md bg-danger-soft p-2 text-xs text-danger" role="alert">
      {{ loadError }} <button type="button" class="underline" @click="refreshTree">Retry</button>
    </p>
    <!-- Inline name input for creating a file/folder (rendered inside the
         tree at the target level; tree folder rows host their own). -->
    <div class="min-h-0 flex-1 overflow-y-auto p-1" @contextmenu.prevent="onBlankContext">
      <TreeNameInput
        v-if="createState.active?.dir === ''"
        v-model="createState.name"
        :kind="createState.active.kind"
        :label="createState.active.kind === 'file' ? 'New File Name' : 'New Folder Name'"
        :error="createState.error"
        :busy="createState.busy"
        class="pl-5"
        @confirm="createState.confirm"
        @cancel="createState.cancel"
      />
      <p v-if="loading" class="px-2 py-1 text-[11px] text-subtle">Loading…</p>
      <TreeItem v-for="node in root" v-else :key="node.path" :node="node" :depth="0" />
    </div>

    <ContextMenu
      v-if="menu"
      :x="menu.x"
      :y="menu.y"
      :groups="menuGroupsOf(menu.node)"
      @select="onMenuSelect"
      @close="menu = null"
    />
    <FilePropertiesModal
      v-if="properties"
      :title="properties.title"
      :rows="properties.rows"
      @close="properties = null"
    />
  </div>
</template>
