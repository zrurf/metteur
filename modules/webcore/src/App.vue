<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import {
  ChevronDown,
  Folder,
  FolderOpen,
  Files,
  Loader2,
  Terminal,
  History,
  MessageSquare,
  Moon,
  Settings,
  Sun,
  WifiOff,
  Workflow,
  X,
} from '@lucide/vue'
import type { Component } from 'vue'
import { useWorkspaceStore } from '@/stores/workspace'
import { useConfigStore } from '@/stores/config'
import { useThemeStore } from '@/stores/theme'
import { useChatStore } from '@/stores/chat'
import { usePanelStore } from '@/stores/panel'
import { useRightPanelStore } from '@/stores/right-panel'
import { useTabsStore } from '@/stores/tabs'
import { useJobsStore } from '@/stores/jobs'
import { useFileWatchStore } from '@/stores/filewatch'
import { gateway } from '@/core'
import { projectName } from '@/lib/path'
import { rootRoute } from '@/lib/workspace-url'
import { pendingRoute } from '@/router'
import AppLogo from '@/components/AppLogo.vue'
import EditorPane from '@/views/EditorPane.vue'
import JobsPanel from '@/components/JobsPanel.vue'
import ApprovalDialog from '@/components/ApprovalDialog.vue'
import ConfirmDialog from '@/components/ConfirmDialog.vue'
import FolderOpenDialog from '@/components/FolderOpenDialog.vue'
import ToastHost from '@/components/ToastHost.vue'
import AppUpdateNotice from '@/components/AppUpdateNotice.vue'
import TabBar from '@/components/TabBar.vue'
import FileTree from '@/components/FileTree.vue'
import { useSurfaceNavigation } from '@/lib/surface'
import { useFeedbackStore } from '@/stores/feedback'

const route = useRoute()
const router = useRouter()
const workspace = useWorkspaceStore()
const configStore = useConfigStore()
const theme = useThemeStore()
const chat = useChatStore()
const panel = usePanelStore()
const rightPanel = useRightPanelStore()
const tabs = useTabsStore()
const jobs = useJobsStore()
const fileWatch = useFileWatchStore()
const feedback = useFeedbackStore()
const { openSurface } = useSurfaceNavigation()

/* Activity rail                                                        */

interface Activity {
  key: 'explorer' | 'chat' | 'version' | 'execution' | 'execution-preview'
  label: string
  icon: Component
}

const activities: Activity[] = [
  { key: 'explorer', label: 'Explorer', icon: Files },
  { key: 'execution', label: 'Execution', icon: Workflow },
  ...(import.meta.env.DEV && import.meta.env.VITE_MOCK === '1'
    ? [{ key: 'execution-preview' as const, label: 'Execution preview', icon: Workflow }] : []),
  { key: 'chat', label: 'Chat', icon: MessageSquare },
  { key: 'version', label: 'Version Flow', icon: History },
]

/** Route → which activity it belongs to (drives rail highlight when no panel
 *  owns the sidebar). `route.name` may be undefined before mount. */
function routeActivity(name: string | symbol | null | undefined): Activity['key'] | null {
  if (name === 'execution') return 'execution'
  if (name === 'execution-preview') return 'execution-preview'
  if (name === 'chat') return 'chat'
  if (name === 'version') return 'version'
  if (name === 'explorer' || name === 'file') return 'explorer'
  return null
}

/** Toggle the Explorer panel. `file` views keep their tab; the sidebar flips
 *  independently, exactly like VSCode's activity bar. */
function toggleExplorer() {
  if (panel.open && panel.owner === 'explorer') panel.clear()
  else panel.show('explorer', FileTree, 'Explorer')
}

/** Open a workspace surface from the activity rail. Sidebar ownership is
 *  asserted centrally in {@link openSurface} (lib/surface), mirrored by the
 *  tab bar, so both entry points restore the correct rail. */
function openActivity(a: Activity['key']) {
  if (a === 'execution-preview' || a === 'execution') {
    openSurface(a)
    return
  }
  if (a === 'explorer') {
    toggleExplorer()
    return
  }
  openSurface(a as 'chat' | 'version' | 'settings')
}

/** Settings lives at the bottom of the rail; it is also a reopenable tab. */
function openSettings() {
  openSurface('settings')
}

/** Rail highlight: full-page surfaces (chat) drive it from their route;
 *  otherwise an open panel wins, falling back to the route. */
const activeKey = computed<Activity['key'] | null>(() => {
  const r = routeActivity(route.name)
  if (r === 'chat' || r === 'execution-preview' || r === 'execution') return r
  const owner = panel.open ? panel.owner : null
  return owner === 'explorer' || owner === 'version' ? owner : r
})

/** Settings is its own rail button at the bottom, highlighted on its route. */
const settingsActive = computed(() => route.name === 'settings')

/** Open the Explorer by default as soon as a workspace mounts. */
watch(
  () => workspace.hasActive,
  (on) => {
    if (on && !panel.open) panel.show('explorer', FileTree, 'Explorer')
  },
  { immediate: true },
)

/** Keep the fs-watch and job subscriptions tied to the open workspace. */
watch(
  () => workspace.active?.path,
  (path) => {
    fileWatch.stop()
    jobs.stop()
    if (path) {
      fileWatch.start()
      void jobs.start()
    }
  },
  { immediate: true },
)

/** Opens the background-command panel beside the editor. */
function openJobs() {
  rightPanel.show(JobsPanel, 'Jobs', {})
}

// Reopen the last-used workspace so a refresh returns to the IDE shell, then
// resume the route that was stashed by the workspace gate (deep links).
onMounted(async () => {
  if (import.meta.env.DEV && import.meta.env.VITE_MOCK === '1'
    && window.location.pathname === '/preview/execution') return
  await workspace.restore()
  void configStore.load()
  if (workspace.hasActive) {
    const pending = pendingRoute()
    if (pending) router.replace(pending)
    else if (route.path === '/' || route.path === '/welcome') {
      const target = rootRoute()
      if (target) router.replace(target)
    }
  }
})

/* Workspace switcher                                                   */

const switchOpen = ref(false)
const connectionLabel = computed(() => (gateway.connected.value ? 'Connected' : 'Offline | demo'))
/** Real-gateway outage: blocks the UI until the heartbeat reconnects. */
const connectionLost = computed(() => !gateway.connected.value && !gateway.demo)

function toggleSwitcher() {
  switchOpen.value = !switchOpen.value
}

function pickRecent(path: string) {
  switchOpen.value = false
  // Second-step confirmation (this window / new window / cancel): the switcher
  // reuses the folder-open dialog so closing the current workspace stays
  // explicit and never happens implicitly.
  pendingOpen.value = path
}

/** Folder chosen by the OS dialog, awaiting the this-/new-window decision. */
const pendingOpen = ref<string | null>(null)
const openingFolder = ref(false)

/** Open the OS folder picker; the choice of where-to-open follows. */
async function openDir() {
  switchOpen.value = false
  if (openingFolder.value) return
  openingFolder.value = true
  try {
    const result = await gateway.pickDirectory()
    if (!result.ok) {
      feedback.toast('error', 'Folder picker unavailable', result.error)
      return
    }
    if (!result.data) return // The user cancelled the dialog.
    pendingOpen.value = result.data
  } finally {
    openingFolder.value = false
  }
}

/** "This window": switch the active workspace and enter the IDE shell. */
async function openInThisWindow() {
  const path = pendingOpen.value
  pendingOpen.value = null
  if (!path) return
  const r = await workspace.open(path)
  if (r.ok) {
    panel.show('explorer', FileTree, 'Explorer')
    router.push(rootRoute() || '/welcome')
  } else {
    feedback.toast('error', 'Failed to open workspace', r.message)
  }
}

/** "New window": open another tab pointed at `/?open=<path>`. */
function openInNewWindow() {
  const path = pendingOpen.value
  pendingOpen.value = null
  if (!path) return
  window.open(`${location.origin}/?open=${encodeURIComponent(path)}`, '_blank')
}

function cancelOpen() {
  pendingOpen.value = null
}

/** Close the current workspace, falling back to the welcome page. */
async function closeWorkspace() {
  switchOpen.value = false
  const ws = workspace.active
  if (!ws) return
  const ok = await feedback.confirm({
    header: 'Close workspace',
    message: `Close "${projectName(ws.path)}"? Unsaved changes in its open tabs will be lost.`,
    acceptLabel: 'Close',
    rejectLabel: 'Cancel',
    danger: true,
  })
  if (!ok) return
  await workspace.close()
  router.replace('/welcome')
}

function onGlobalDown(e: MouseEvent) {
  const target = e.target as HTMLElement
  if (switchOpen.value && !target.closest?.('[data-switcher]')) switchOpen.value = false
}

/** Drag to resize the sidebar; clamps before the editor becomes too narrow. */
function onResizeStart(e: MouseEvent) {
  e.preventDefault()
  const startX = e.clientX
  const startW = panel.width
  const move = (ev: MouseEvent) => {
    const w = Math.min(520, Math.max(180, startW + (ev.clientX - startX)))
    panel.width = Math.round(w)
  }
  const up = () => {
    window.removeEventListener('mousemove', move)
    window.removeEventListener('mouseup', up)
  }
  window.addEventListener('mousemove', move)
  window.addEventListener('mouseup', up)
}

/** Whether the split view's right-hand pane should be on screen. */
const splitVisible = computed(() => tabs.splitPath !== null)

/** Drag the divider between the two editor panes. */
function onSplitResizeStart(e: MouseEvent) {
  e.preventDefault()
  const area = (e.currentTarget as HTMLElement).parentElement
  if (!area) return
  const bounds = area.getBoundingClientRect()
  const move = (ev: MouseEvent) => {
    tabs.setSplitRatio((ev.clientX - bounds.left) / bounds.width)
  }
  const up = () => {
    window.removeEventListener('mousemove', move)
    window.removeEventListener('mouseup', up)
  }
  window.addEventListener('mousemove', move)
  window.addEventListener('mouseup', up)
}

/** Drag to resize the right-hand panel; clamps to a sane width range. */
function onRightResizeStart(e: MouseEvent) {
  e.preventDefault()
  const startX = e.clientX
  const startW = rightPanel.width
  const move = (ev: MouseEvent) => {
    const w = Math.min(560, Math.max(200, startW + (startX - ev.clientX)))
    rightPanel.width = Math.round(w)
  }
  const up = () => {
    window.removeEventListener('mousemove', move)
    window.removeEventListener('mouseup', up)
  }
  window.addEventListener('mousemove', move)
  window.addEventListener('mouseup', up)
}
</script>

<template>
  <div class="flex h-full flex-col bg-background" @mousedown="onGlobalDown">
    <!-- Title bar -->
    <header class="relative flex h-11 shrink-0 items-center gap-3 border-b border-divider px-3">
      <div class="flex items-center gap-2">
        <AppLogo :size="24" />
        <span class="text-[13px] font-semibold tracking-tight">Metteur</span>
      </div>

      <span class="h-4 w-px bg-divider" />

      <!-- Workspace switcher -->
      <button
        data-switcher
        class="flex min-w-0 items-center gap-1.5 rounded-md px-2 py-1 text-[13px] transition-colors duration-150 hover:bg-hover"
        type="button"
        title="Switch workspace"
        :aria-label="'Switch workspace'"
        @click.stop="toggleSwitcher"
      >
        <span class="max-w-60 truncate font-medium">
          {{ workspace.active ? projectName(workspace.active.path) : 'No workspace' }}
        </span>
        <ChevronDown class="h-3.5 w-3.5 shrink-0 text-subtle" />
      </button>

      <div class="ml-auto flex items-center gap-1">
        <button class="btn-icon" type="button" :title="theme.mode" @click="theme.toggle()">
          <Sun v-if="theme.mode === 'dark'" class="h-4 w-4" />
          <Moon v-else class="h-4 w-4" />
        </button>
      </div>

      <!-- Switcher popover -->
      <div
        v-if="switchOpen"
        data-switcher
        class="glass absolute left-24 top-11 z-40 w-72 rounded-lg border border-divider py-1 shadow-card"
      >
        <p class="px-3 pb-1 pt-1.5 text-[10px] font-semibold uppercase tracking-wider text-subtle">Recent workspaces</p>
        <div v-if="workspace.recents.length" class="px-1">
          <button
            v-for="w in workspace.recents"
            :key="w.path"
            class="flex w-full items-center gap-2 rounded-md px-2.5 py-1.5 text-left text-[12.5px] hover:bg-accent"
            type="button"
            :class="workspace.active?.path === w.path ? 'bg-accent' : ''"
            @click="pickRecent(w.path)"
          >
            <Folder class="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
            <span class="min-w-0">
              <span class="block truncate font-medium">{{ projectName(w.path) }}</span>
              <span class="block truncate text-[10.5px] text-subtle">{{ w.path }}</span>
            </span>
          </button>
        </div>
        <p v-else class="px-3 py-1.5 text-[12px] text-muted-foreground">Nothing yet.</p>
        <div class="mt-1 border-t border-divider px-1 pt-1">
          <button
            class="flex w-full items-center gap-2 rounded-md px-2.5 py-1.5 text-left text-[12.5px] hover:bg-accent"
            type="button"
            @click="openDir"
          >
            <FolderOpen class="h-3.5 w-3.5 text-primary" />
            <span class="font-medium">Open a folder…</span>
          </button>
          <button
            v-if="workspace.active"
            class="flex w-full items-center gap-2 rounded-md px-2.5 py-1.5 text-left text-[12.5px] hover:bg-danger-soft"
            type="button"
            style="color: var(--danger)"
            @click="closeWorkspace"
          >
            <X class="h-3.5 w-3.5" />
            <span class="font-medium">Close workspace</span>
          </button>
        </div>
      </div>
    </header>

    <!-- IDE shell: only when a workspace is open. -->
    <template v-if="workspace.hasActive">
      <TabBar />
      <div class="flex min-h-0 flex-1">
        <!-- Activity rail: workspace features on top, Settings pinned to the
             bottom of the same rail (item 14). -->
        <nav class="flex w-12 shrink-0 flex-col items-center border-r border-divider bg-sidebar py-1.5">
          <button
            v-for="a in activities"
            :key="a.key"
            class="relative flex h-10 w-10 items-center justify-center rounded-lg p-0 text-muted-foreground transition-colors duration-150 hover:bg-hover hover:text-foreground"
            :class="activeKey === a.key ? 'bg-primary-soft! text-primary!' : ''"
            type="button"
            :title="a.label"
            :aria-label="a.label"
            @click="openActivity(a.key)"
          >
            <span
              v-if="activeKey === a.key"
              class="absolute left-0 top-1/2 h-5 w-0.5 -translate-y-1/2 rounded-full"
              style="background: var(--primary)"
            />
            <component :is="a.icon" class="h-4.5 w-4.5" />
          </button>

          <div class="min-h-2 flex-1" />

          <div class="flex w-full flex-col items-center gap-1 border-t border-divider py-1.5">
            <button
              class="relative flex h-10 w-10 items-center justify-center rounded-lg p-0 text-muted-foreground transition-colors duration-150 hover:bg-hover hover:text-foreground"
              :class="settingsActive ? 'bg-primary-soft! text-primary!' : ''"
              type="button"
              title="Settings"
              :aria-label="'Settings'"
              @click="openSettings"
            >
              <span
                v-if="settingsActive"
                class="absolute left-0 top-1/2 h-5 w-0.5 -translate-y-1/2 rounded-full"
                style="background: var(--primary)"
              />
              <Settings class="h-4.5 w-4.5" />
            </button>
          </div>
        </nav>
        <aside
          v-if="panel.open"
          class="workspace-file-panel relative flex shrink-0 flex-col bg-sidebar"
          :style="{ width: panel.width + 'px' }"
        >
          <div
            class="flex h-8 shrink-0 items-center gap-1.5 border-b border-divider px-3 text-[11px] font-semibold uppercase tracking-wider text-subtle"
          >
            <Workflow v-if="panel.owner !== 'version'" class="h-3.5 w-3.5" />
            <span class="truncate">{{ panel.title }}</span>
          </div>
          <div class="min-h-0 flex-1 overflow-hidden">
            <component :is="panel.component" />
          </div>
          <!-- Vertical drag handle for resizing. -->
          <div
            class="group absolute -right-1 top-0 z-20 h-full w-2 cursor-col-resize"
            role="separator"
            aria-orientation="vertical"
            @mousedown.prevent="onResizeStart"
          >
            <div
              class="absolute right-0.75 top-0 h-full w-px bg-transparent transition-colors group-hover:bg-foreground/20"
            />
          </div>
        </aside>

        <!-- Main editor area: one pane, or two when a file is opened to the side. -->
        <main class="workspace-editor-surface flex min-w-0 flex-1 overflow-hidden">
          <div class="min-w-0 flex-1 overflow-hidden" :style="splitVisible ? { flex: `0 0 ${tabs.splitRatio * 100}%` } : undefined">
            <RouterView />
          </div>
          <template v-if="splitVisible">
            <div
              class="group relative z-10 w-px shrink-0 cursor-col-resize bg-divider"
              role="separator"
              aria-orientation="vertical"
              title="Drag to resize"
              @mousedown.prevent="onSplitResizeStart"
            >
              <div class="absolute -left-1 top-0 h-full w-3" />
            </div>
            <div class="flex min-w-0 flex-1 flex-col overflow-hidden">
              <div
                class="flex h-8 shrink-0 items-center gap-1.5 border-b border-divider px-3 text-[11px] text-subtle"
              >
                <Files class="h-3.5 w-3.5" />
                <span class="mono truncate">{{ tabs.splitPath }}</span>
                <button
                  class="btn-icon ml-auto h-5! w-5!"
                  type="button"
                  title="Close split view"
                  aria-label="Close split view"
                  @click="tabs.closeSplit()"
                >
                  <X class="h-3.5 w-3.5" />
                </button>
              </div>
              <div class="min-h-0 flex-1 overflow-hidden">
                <EditorPane :key="tabs.splitPath ?? ''" :file-path="tabs.splitPath ?? undefined" />
              </div>
            </div>
          </template>
        </main>

        <!-- Right-hand panel (VSCode-style). Owned by the editor, e.g. for
             per-file version history; closable and resizable (item 11). -->
        <aside
          v-if="rightPanel.open"
          class="relative flex shrink-0 flex-col border-l border-divider bg-sidebar"
          :style="{ width: rightPanel.width + 'px' }"
        >
          <div
            class="flex h-8 shrink-0 items-center gap-1.5 border-b border-divider px-3 text-[11px] font-semibold uppercase tracking-wider text-subtle"
          >
            <Workflow class="h-3.5 w-3.5" />
            <span class="truncate">{{ rightPanel.title }}</span>
            <button
              class="btn-icon ml-auto h-5! w-5!"
              type="button"
              title="Close panel"
              :aria-label="'Close panel'"
              @click="rightPanel.close()"
            >
              <X class="h-3.5 w-3.5" />
            </button>
          </div>
          <div class="min-h-0 flex-1 overflow-hidden">
            <component :is="rightPanel.component" v-bind="rightPanel.props" />
          </div>
          <!-- Vertical drag handle for resizing. -->
          <div
            class="group absolute -left-1 top-0 z-20 h-full w-2 cursor-col-resize"
            role="separator"
            aria-orientation="vertical"
            @mousedown.prevent="onRightResizeStart"
          >
            <div
              class="absolute left-0.75 top-0 h-full w-px bg-transparent transition-colors group-hover:bg-foreground/20"
            />
          </div>
        </aside>
      </div>
    </template>

    <!-- No workspace: welcome page fills the window, no tabs/explorer. -->
    <main v-else class="min-h-0 flex-1 overflow-hidden">
      <RouterView />
    </main>

    <!-- Status bar -->
    <footer
      class="flex h-6 shrink-0 items-center gap-4 border-t border-divider bg-sidebar px-3 text-[11px] text-muted-foreground"
    >
      <span v-if="workspace.active" class="max-w-75 truncate">{{ workspace.active.path }}</span>
      <span v-else>{{ workspace.recents.length }} recent workspace(s)</span>

      <button
        v-if="jobs.entries.length"
        class="ml-auto flex items-center gap-1.5 rounded px-1.5 py-0.5 transition-colors duration-150 hover:bg-hover hover:text-foreground"
        type="button"
        :title="jobs.running ? `${jobs.running} command(s) running` : 'Background commands'"
        @click="openJobs"
      >
        <Loader2 v-if="jobs.running" class="h-3 w-3 animate-spin" style="color: var(--primary)" />
        <Terminal v-else class="h-3 w-3" />
        <span>{{ jobs.running ? `${jobs.running} running` : `${jobs.entries.length} job(s)` }}</span>
      </button>

      <span class="flex items-center gap-1.5" :class="jobs.entries.length ? '' : 'ml-auto'">
        <span class="h-1.5 w-1.5 rounded-full" :class="gateway.connected.value ? 'bg-emerald-500' : 'bg-subtle'" />
        {{ connectionLabel }}
      </span>
    </footer>

    <ApprovalDialog />
    <ToastHost />
    <AppUpdateNotice />
    <ConfirmDialog />
    <FolderOpenDialog
      v-if="pendingOpen"
      :path="pendingOpen"
      @this-window="openInThisWindow"
      @new-window="openInNewWindow"
      @cancel="cancelOpen"
    />

    <!-- Blocking outage overlay: the daemon powers every save, so a lost link
         freezes the app until the heartbeat reconnects (no manual dismiss). -->
    <Teleport to="body">
      <div
        v-if="connectionLost"
        class="fixed inset-0 z-100 grid place-items-center bg-black/60 backdrop-blur-sm"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="lost-title"
      >
        <div class="w-100 rounded-2xl border border-divider bg-surface p-6 text-center shadow-2xl">
          <div class="mx-auto mb-3 grid h-12 w-12 place-items-center rounded-full bg-danger/10 text-danger">
            <WifiOff class="h-6 w-6" />
          </div>
          <h2 id="lost-title" class="mb-1 text-[18px] font-semibold text-foreground">
            Daemon connection lost
          </h2>
          <p class="text-[15px] leading-relaxed text-muted-foreground">
            The daemon is unreachable, please check whether the daemon is running properly.
          </p>
          <p class="mt-3 flex items-center justify-center gap-1.5 text-[11.5px] text-subtle">
            <span class="h-1.5 w-1.5 animate-pulse rounded-full" style="background: var(--primary)" />
            Reconnecting…
          </p>
        </div>
      </div>
    </Teleport>
  </div>
</template>
