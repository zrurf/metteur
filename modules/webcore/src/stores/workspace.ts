import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { gateway } from '@/core'
import type { FileTreeNode, WorkspaceInfo } from '@/core'

const RECENTS_KEY = 'metteur.recent-workspaces'
const LAST_KEY = 'metteur.last-workspace'

function loadRecents(): WorkspaceInfo[] {
  try {
    const arr = JSON.parse(localStorage.getItem(RECENTS_KEY) ?? '[]')
    return Array.isArray(arr) ? arr.filter((w) => w && typeof w.path === 'string') : []
  } catch {
    return []
  }
}

function saveRecents(list: WorkspaceInfo[]) {
  try {
    localStorage.setItem(RECENTS_KEY, JSON.stringify(list))
  } catch {
    // Storage unavailable (e.g. private mode): recents stay in-memory.
  }
}

/**
 * The last-opened workspace is per-tab (`sessionStorage`) so that opening a
 * second workspace in a new window never rewrites the active workspace of an
 * existing tab. The same tab survives reloads; other tabs stay untouched.
 */
function saveLastWorkspace(path: string | null) {
  try {
    if (path) sessionStorage.setItem(LAST_KEY, path)
    else sessionStorage.removeItem(LAST_KEY)
  } catch {
    // Ignore storage failures; the workspace just won't reopen itself.
  }
}

function loadLastWorkspace(): string | null {
  try {
    return sessionStorage.getItem(LAST_KEY)
  } catch {
    return null
  }
}

/**
 * Active workspace management (single-workspace shell).
 *
 * Each window shows one workspace at a time, like a native IDE window, while
 * the shared daemon may keep several open (one per window). `recents`
 * remembers every workspace opened so the user can jump back without retyping
 * the path; the recent list survives reloads via `localStorage`, while the
 * last-active path is kept per-tab in `sessionStorage`. New windows inherit
 * the opener's `sessionStorage` via `window.open`, so a restored workspace is
 * never closed on the daemon by another window switching its active view.
 */
export const useWorkspaceStore = defineStore('workspace', () => {
  /** Workspaces opened earlier, for the switcher / welcome-page list. */
  const recents = ref<WorkspaceInfo[]>(loadRecents())
  /** The currently open workspace, or `null` to fall back to the welcome page. */
  const active = ref<WorkspaceInfo | null>(null)
  const busy = ref(false)
  let selectionGeneration = 0
  /** Explorer directory listings keyed by `workspacePath|dir`. */
  const dirCache = ref<Map<string, FileTreeNode[]>>(new Map())
  const hasActive = computed(() => active.value !== null)

  /** Back-compat alias of `recents`. */
  const workspaces = computed(() => recents.value)

  /** Cached listing for a directory, if present. */
  function cachedDir(ws: string, dir: string): FileTreeNode[] | undefined {
    return dirCache.value.get(`${ws}|${dir}`)
  }

  /** Store a freshly fetched directory listing. */
  function cacheDir(ws: string, dir: string, nodes: FileTreeNode[]) {
    dirCache.value.set(`${ws}|${dir}`, nodes)
    dirCache.value = new Map(dirCache.value)
  }

  /** Drop a directory's listing so the next view refetches it. */
  function invalidateDir(ws: string, dir: string) {
    dirCache.value.delete(`${ws}|${dir}`)
    dirCache.value = new Map(dirCache.value)
  }

  /** Drop every cached listing for a workspace (used when the tree refreshes). */
  function invalidateTree(ws: string) {
    const prefix = `${ws}|`
    const next = new Map<string, FileTreeNode[]>()
    for (const [k, v] of dirCache.value) if (!k.startsWith(prefix)) next.set(k, v)
    dirCache.value = next
  }

  /** Remember a workspace for the switcher (idempotent, newest first). */
  function remember(ws: WorkspaceInfo) {
    recents.value = [ws, ...recents.value.filter((w) => w.path !== ws.path)]
    saveRecents(recents.value)
  }

  /** Reopen the most recently used workspace (called on app boot). */
  async function restore() {
    if (active.value) return
    // A deep link (`?open=<path>`, emitted by "Open in new window") already
    // names the boot workspace; ignore the sessionStorage copy inherited from
    // the opener tab (window.open duplicates sessionStorage in browsers).
    if (typeof window !== 'undefined' && window.location.search.includes('open=')) return
    const path = loadLastWorkspace()
    if (!path) return
    const generation = selectionGeneration
    const r = await gateway.openWorkspace(path)
    if (generation !== selectionGeneration || busy.value || active.value) return
    if (r.ok) {
      dirCache.value = new Map()
      active.value = r.data
      remember(r.data)
    }
  }

  /** Open a workspace, returning the failure reason when it cannot be opened. */
  async function open(path: string): Promise<{ ok: boolean; message?: string }> {
    const target = path.trim()
    if (!target) return { ok: false, message: 'Path is empty' }
    if (busy.value) return { ok: false, message: 'A workspace operation is already in progress' }
    busy.value = true
    selectionGeneration++
    try {
      // A window selects a workspace; it does not own the shared daemon's
      // workspace lifetime. Validate the new target before detaching locally.
      const r = await gateway.openWorkspace(target)
      if (!r.ok) return { ok: false, message: r.error }
      dirCache.value = new Map()
      active.value = r.data
      remember(r.data)
      saveLastWorkspace(target)
      return { ok: true }
    } finally {
      busy.value = false
    }
  }

  /**
   * Takes a checkpoint of the active workspace.
   *
   * Returns `null` when no workspace is open or the daemon refuses (versioning
   * disabled, no changed files): a missing checkpoint is not an error, it just
   * means this turn cannot be undone from the transcript.
   */
  async function createCheckpoint(description: string): Promise<string | null> {
    const ws = active.value
    if (!ws) return null
    const r = await gateway.createSnapshot(ws.path, description)
    return r.ok ? r.data.id : null
  }

  /** Rolls the active workspace back to a checkpoint. */
  async function restoreCheckpoint(snapshotId: string): Promise<boolean> {
    const ws = active.value
    if (!ws) return false
    const r = await gateway.rollback(ws.path, snapshotId)
    if (!r.ok) return false
    dirCache.value = new Map()
    return true
  }

  /** Switch to a previously opened workspace without a filesystem call. */
  async function switchTo(path: string): Promise<void> {
    if (active.value?.path === path) return
    await open(path)
  }

  /** Close a workspace and fall back to the welcome page (kept in recents). */
  async function close() {
    const ws = active.value
    if (!ws) return
    return closePath(ws.path)
  }

  async function closePath(path: string) {
    if (busy.value) return false
    selectionGeneration++
    busy.value = true
    try {
      const result = await gateway.closeWorkspace(path)
      if (!result.ok) return false
      if (active.value?.path === path) {
        active.value = null
        saveLastWorkspace(null)
      }
      return true
    } finally {
      busy.value = false
    }
  }

  async function select(path: string) {
    return open(path)
  }

  /** Remove a workspace from the switcher list (and close it if active). */
  async function forget(path: string) {
    if (active.value?.path === path && !(await closePath(path))) return
    recents.value = recents.value.filter((w) => w.path !== path)
    saveRecents(recents.value)
  }

  /** Merge the daemon's open workspaces into the persisted recent list. */
  async function refresh() {
    const r = await gateway.listWorkspaces()
    if (!r.ok) return
    const seen = new Set<string>()
    recents.value = [...r.data, ...recents.value].filter((w) => {
      if (seen.has(w.path)) return false
      seen.add(w.path)
      return true
    })
    saveRecents(recents.value)
    if (active.value && !r.data.some((w) => w.path === active.value!.path)) active.value = null
  }

  return {
    recents,
    workspaces,
    active,
    hasActive,
    busy,
    cachedDir,
    cacheDir,
    invalidateDir,
    invalidateTree,
    restore,
    open,
    switchTo,
    close,
    closePath,
    select,
    forget,
    refresh,
    createCheckpoint,
    restoreCheckpoint,
  }
})
