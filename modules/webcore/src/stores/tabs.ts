import { defineStore } from 'pinia'
import type { Component } from 'vue'
import { computed, ref } from 'vue'
import { FileCode2, FileJson, FileText, History, MessageSquare, Settings, Workflow } from '@lucide/vue'
import { projectName } from '@/lib/path'
import { fileRoute } from '@/lib/file-token'
import { wurl } from '@/lib/workspace-url'
import { USER_CONFIG_PATH, WORKSPACE_CONFIG_PATH } from '@/lib/toml'

/**
 * IDE-style tabs for the current shell.
 *
 * Tabs cover both opened *files* and a set of app *surfaces* (Chat / Audit /
 * Version / Settings). Surfaces are ordinary closable tabs too: clicking the
 * activity bar re-opens them, and closing one does not hide the entry point.
 * Dirty state lives here so it survives tab switches and each file tab keeps
 * its own unsaved indicator.
 */
export interface TabItem {
  id: string
  label: string
  /** Workspace-relative path of the opened file; empty string for a surface. */
  path: string
  route: string
  icon: Component
}

/** App surfaces exposed as closable tabs, reopened from the activity bar. */
export interface SurfaceDef {
  key: 'chat' | 'version' | 'settings' | 'execution' | 'execution-preview'
  label: string
  /** Surface route fragment, e.g. `/chat`. */
  path: string
  /** Whether the surface lives under the workspace base URL. */
  workspace: boolean
  icon: Component
}

export const SURFACES: SurfaceDef[] = [
  { key: 'execution', label: 'Execution', path: '/execution', workspace: true, icon: Workflow },
  ...(import.meta.env.DEV && import.meta.env.VITE_MOCK === '1'
    ? [{ key: 'execution-preview' as const, label: 'Execution preview', path: '/preview/execution', workspace: false, icon: Workflow }] : []),
  { key: 'chat', label: 'Chat', path: '/chat', workspace: true, icon: MessageSquare },
  { key: 'version', label: 'Version Flow', path: '/version', workspace: true, icon: History },
  { key: 'settings', label: 'Settings', path: '/settings', workspace: false, icon: Settings },
]

/** Route for a surface: workspace surfaces are prefixed with the workspace base. */
export function surfaceRoute(def: SurfaceDef): string {
  return def.workspace ? wurl(def.path) : def.path
}

const HOME_ID = 'home'

function fileIcon(path: string): Component {
  if (path.endsWith('.blueprint')) return Workflow
  if (path.endsWith('.mbp')) return FileCode2
  if (path.endsWith('.json')) return FileJson
  return FileText
}

/** Friendly tab label for the virtual config documents. */
function configDocLabel(path: string): string {
  if (path === USER_CONFIG_PATH) return 'User config.toml'
  if (path === WORKSPACE_CONFIG_PATH) return 'Workspace config.toml'
  return projectName(path)
}

export const useTabsStore = defineStore('tabs', () => {
  const items = ref<TabItem[]>([])
  const activeId = ref(HOME_ID)
  /** Unsaved indicator per file path (`true` = shows the blue dot). */
  const dirty = ref<Record<string, boolean>>({})
  /** Disk-change conflict per file path (file edited both locally and on disk). */
  const conflicts = ref<Record<string, boolean>>({})

  /** Workspace-relative path shown in the right-hand editor pane, if split. */
  const splitPath = ref<string | null>(null)
  /** Fraction of the editor area given to the primary pane. */
  const splitRatio = ref(0.5)

  const byId = computed(() => new Map(items.value.map((t) => [t.id, t])))
  /** The active file tab, if any. */
  const activeItem = computed<TabItem | null>(() => byId.value.get(activeId.value) ?? null)
  /** Path used by the explorer to highlight the file being edited. */
  const activeFilePath = computed<string | null>(() => activeItem.value?.path ?? null)

  function activate(id: string) {
    if (byId.value.has(id)) activeId.value = id
  }

  /** Open (or focus) a file tab for a workspace-relative path. */
  function openFile(path: string): string {
    const existing = items.value.find((t) => t.path === path)
    if (existing) {
      activeId.value = existing.id
      return existing.id
    }
    const tab: TabItem = {
      id: `file:${path}`,
      label: configDocLabel(path),
      path,
      route: fileRoute(path),
      icon: fileIcon(path),
    }
    items.value = [...items.value, tab]
    activeId.value = tab.id
    return tab.id
  }

  function openConfigDoc(scope: 'user' | 'workspace'): string {
    return openFile(scope === 'user' ? USER_CONFIG_PATH : WORKSPACE_CONFIG_PATH)
  }

  /** Open (or focus) an app surface as a normal closable tab. */
  function openSurface(key: SurfaceDef['key']): string {
    const def = SURFACES.find((s) => s.key === key)
    if (!def) return ''
    const existing = items.value.find((t) => t.id === key)
    if (existing) {
      activeId.value = existing.id
      return existing.id
    }
    const tab: TabItem = {
      id: key,
      label: def.label,
      path: '',
      route: surfaceRoute(def),
      icon: def.icon,
    }
    items.value = [...items.value, tab]
    activeId.value = tab.id
    return tab.id
  }

  /** Whether the active tab is an app surface (e.g. Chat / Audit / Settings). */
  function isSurface(id: string): boolean {
    return SURFACES.some((s) => s.key === id)
  }

  function close(id: string) {
    if (!byId.value.has(id)) return
    items.value = items.value.filter((t) => t.id !== id)
    const path = byId.value.get(id)?.path
    if (path) {
      delete dirty.value[path]
      delete conflicts.value[path]
    }
    if (activeId.value === id) {
      const rest = items.value
      activeId.value = rest.length ? rest[rest.length - 1].id : HOME_ID
    }
    // A split still showing a file whose tab closed would be a dead pane.
    if (path && splitPath.value === path) splitPath.value = null
  }

  function closeOthers(id: string) {
    items.value = items.value.filter((t) => t.id === id)
    activeId.value = id
  }

  function closeAll() {
    items.value = []
    dirty.value = {}
    conflicts.value = {}
    activeId.value = HOME_ID
  }

  function move(fromId: string, toId: string) {
    const from = items.value.findIndex((t) => t.id === fromId)
    const to = items.value.findIndex((t) => t.id === toId)
    if (from < 0 || to < 0 || from === to) return
    const [moved] = items.value.splice(from, 1)
    const target = items.value.findIndex((t) => t.id === toId)
    items.value.splice(target < 0 ? items.value.length : target, 0, moved)
  }

  /** Show `path` in the right-hand pane, opening the split when needed. */
  function openInSplit(path: string) {
    splitPath.value = path
  }

  /** Close the right-hand pane. */
  function closeSplit() {
    splitPath.value = null
  }

  /** Set the divider position from a drag, clamped to a usable range. */
  function setSplitRatio(ratio: number) {
    splitRatio.value = Math.min(0.85, Math.max(0.15, ratio))
  }

  /** Set the unsaved flag for a file (drive the label's blue dot). */
  function markDirty(path: string, value: boolean) {
    if (value) dirty.value = { ...dirty.value, [path]: true }
    else {
      const next = { ...dirty.value }
      delete next[path]
      dirty.value = next
    }
  }

  function isDirty(path: string): boolean {
    return dirty.value[path] === true
  }

  function markConflict(path: string, value: boolean) {
    if (value) conflicts.value = { ...conflicts.value, [path]: true }
    else {
      const next = { ...conflicts.value }
      delete next[path]
      conflicts.value = next
    }
  }

  function isConflict(path: string): boolean {
    return conflicts.value[path] === true
  }

  return {
    items,
    activeId,
    activeItem,
    activeFilePath,
    splitPath,
    splitRatio,
    setSplitRatio,
    openInSplit,
    closeSplit,
    homeId: HOME_ID,
    openFile,
    openConfigDoc,
    openSurface,
    isSurface,
    activate,
    close,
    closeOthers,
    closeAll,
    move,
    markDirty,
    isDirty,
    markConflict,
    isConflict,
  }
})
