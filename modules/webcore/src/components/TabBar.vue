<script setup lang="ts">
import { ref } from 'vue'
import { useRouter } from 'vue-router'
import { Plus, X } from '@lucide/vue'
import { SURFACES, useTabsStore, type TabItem } from '@/stores/tabs'
import { useWorkspaceStore } from '@/stores/workspace'
import { useSurfaceNavigation, type SurfaceKey } from '@/lib/surface'
import { wurl } from '@/lib/workspace-url'
import ContextMenu, { type MenuGroup } from '@/components/ContextMenu.vue'

/**
 * IDE-style file tab strip (single-workspace shell).
 *
 * Files and app surfaces share the same closable tab strip.
 * Closing the active tab re-navigates the editor to the next tab (or to
 * the empty Explorer view when none remain). The trailing **+** opens a menu of
 * built-in surfaces (Chat / Audit / Version / Settings) plus Open Workspace,
 * instead of directly dropping the workspace.
 */

const router = useRouter()
const tabs = useTabsStore()
const workspace = useWorkspaceStore()

const dragId = ref<string | null>(null)
const menu = ref<{ x: number; y: number; tab: TabItem } | null>(null)
/** The + popover (app-surface shortcuts + open workspace). */
const plusMenu = ref<{ x: number; y: number } | null>(null)
const { openSurface } = useSurfaceNavigation()

function activate(tab: TabItem) {
  // Surface tabs own a sidebar (e.g. Version's timeline rail), so route *and*
  // panel must be re-asserted — a plain route push loses the rail after the
  // panel was handed to another owner (Explorer).
  if (tabs.isSurface(tab.id)) {
    openSurface(tab.id as SurfaceKey)
    return
  }
  tabs.activate(tab.id)
  router.push(tab.route)
}

function closeTab(tab: TabItem) {
  const wasActive = tabs.activeId === tab.id
  tabs.close(tab.id)
  // Re-point the editor region so it follows the tab that just took focus.
  if (wasActive) {
    if (tabs.activeItem) router.push(tabs.activeItem.route)
    else router.push(wurl('/explorer'))
  }
}

function onMiddleClick(tab: TabItem, e: MouseEvent) {
  if (e.button === 1) {
    e.preventDefault()
    closeTab(tab)
  }
}

function onContext(e: MouseEvent, tab: TabItem) {
  e.preventDefault()
  if (tabs.isSurface(tab.id)) {
    openSurface(tab.id as SurfaceKey)
  } else {
    tabs.activate(tab.id)
    router.push(tab.route)
  }
  menu.value = { x: e.clientX, y: e.clientY, tab }
}

function onDragStart(tab: TabItem) {
  dragId.value = tab.id
}
function onDragOver(tab: TabItem) {
  if (dragId.value && dragId.value !== tab.id) tabs.move(dragId.value, tab.id)
}
function onDragEnd() {
  dragId.value = null
}
const isDragging = (id: string) => dragId.value === id

function menuGroupsOf(): MenuGroup[] {
  const actions = [
    { id: 'close', label: 'Close' },
    { id: 'close-others', label: 'Close Others' },
    { id: 'close-all', label: 'Close All' },
  ]
  return [
    { label: 'File', items: [{ id: 'copy-path', hint: 'relative', label: 'Copy Path' }] },
    { label: 'Tab', items: actions },
  ]
}

async function onMenuSelect(id: string) {
  const tab = menu.value?.tab
  if (!tab) return
  switch (id) {
    case 'copy-path':
      if (tab.path) await navigator.clipboard.writeText(tab.path)
      break
    case 'close':
      closeTab(tab)
      break
    case 'close-others':
      tabs.closeOthers(tab.id)
      if (tabs.activeItem) router.push(tabs.activeItem.route)
      else router.push(wurl('/explorer'))
      break
    case 'close-all':
      tabs.closeAll()
      router.push(wurl('/explorer'))
      break
  }
}

async function openWelcome() {
  await workspace.close()
  router.push('/')
}

const plusGroups: MenuGroup[] = [
  {
    label: 'Pages',
    items: SURFACES.map((s) => ({ id: s.key, label: s.label })),
  },
  {
    label: 'Workspace',
    items: [{ id: 'open-workspace', label: 'Open Workspace…' }],
  },
]

function onPlusClick(e: MouseEvent) {
  const rect = (e.currentTarget as HTMLElement).getBoundingClientRect()
  plusMenu.value = { x: rect.left, y: rect.bottom + 4 }
}

function onPlusSelect(id: string) {
  if (id === 'open-workspace') {
    void openWelcome()
    return
  }
  const def = SURFACES.find((s) => s.key === id)
  if (def) openSurface(def.key)
}
</script>

<template>
  <div
    v-if="tabs.items.length"
    class="group/tabbar flex h-8 shrink-0 items-end gap-0.5 overflow-x-auto border-b border-divider bg-background px-1.5"
  >
    <div
      v-for="tab in tabs.items"
      :key="tab.id"
      class="relative flex h-full min-w-0 max-w-50 select-none items-center gap-1.5 rounded-t-md px-3 pt-1 text-[12px] transition-colors duration-150"
      :class="[
        tabs.activeId === tab.id
          ? 'text-foreground'
          : 'text-muted-foreground hover:bg-hover hover:text-foreground',
        isDragging(tab.id) ? 'opacity-40' : '',
      ]"
      :title="tab.path"
      :data-testid="tabs.activeId === tab.id ? 'tab-active' : 'tab'"
      :draggable="true"
      role="button"
      :tabindex="0"
      @click="activate(tab)"
      @auxclick.prevent="onMiddleClick(tab, $event)"
      @contextmenu.prevent="onContext($event, tab)"
      @dragstart="onDragStart(tab)"
      @dragend="onDragEnd"
      @dragover.prevent="onDragOver(tab)"
      @drop.prevent
    >
      <span
        class="absolute inset-x-0 bottom-0 h-0.5 rounded-t-full transition-opacity duration-150"
        :class="tabs.activeId === tab.id ? 'opacity-100' : 'opacity-0'"
        style="background: var(--primary)"
      />
      <component :is="tab.icon" class="h-3 w-3 shrink-0 text-subtle" />
      <span class="min-w-0 flex-1 truncate font-medium">{{ tab.label }}</span>
      <span
        v-if="tabs.isDirty(tab.path) || tabs.isConflict(tab.path)"
        class="h-1.5 w-1.5 shrink-0 rounded-full"
        :style="{ background: tabs.isConflict(tab.path) ? '#e0a13c' : 'var(--primary)' }"
        :title="tabs.isConflict(tab.path) ? 'Changed on disk | resolve in the editor' : 'Unsaved changes'"
      />
      <span
        class="flex h-4 w-4 shrink-0 cursor-pointer items-center justify-center rounded opacity-0 transition-opacity duration-150 hover:bg-hover group-hover/tabbar:opacity-100"
        :class="tabs.activeId === tab.id ? 'opacity-100!' : ''"
        role="button"
        aria-label="Close tab"
        @click.stop.prevent="closeTab(tab)"
      >
        <X class="h-3 w-3" />
      </span>
    </div>

    <button
      class="mb-0.75 ml-0.5 flex h-6 w-6 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors duration-150 hover:bg-hover hover:text-foreground"
      type="button"
      title="New tab"
      aria-label="New tab"
      @click.stop="onPlusClick"
    >
      <Plus class="h-4 w-4" />
    </button>

    <ContextMenu
      v-if="menu"
      :x="menu.x"
      :y="menu.y"
      :groups="menuGroupsOf()"
      @select="onMenuSelect"
      @close="menu = null"
    />

    <ContextMenu
      v-if="plusMenu"
      :x="plusMenu.x"
      :y="plusMenu.y"
      :groups="plusGroups"
      @select="onPlusSelect"
      @close="plusMenu = null"
    />
  </div>
</template>
