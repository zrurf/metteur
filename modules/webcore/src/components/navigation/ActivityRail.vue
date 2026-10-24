<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch, type Component } from 'vue'
import { Settings, SlidersHorizontal, ArrowUp, ArrowDown, RotateCcw } from '@lucide/vue'
import { moveRelative, moveStep, normalizeOrder } from './order'
export interface RailItem {
  key: string
  label: string
  icon: Component
}
const props = withDefaults(
  defineProps<{
    items: RailItem[]
    active: string | null
    settingsActive?: boolean
    storageKey?: string
  }>(),
  { storageKey: 'metteur.navigation.order.v1' },
)
const emit = defineEmits<{ select: [key: string]; settings: [] }>()
const order = ref<string[]>([]),
  dragged = ref(''),
  over = ref(''),
  after = ref(false),
  notice = ref('')
const menu = ref<{ key: string; x: number; y: number } | null>(null),
  menuEl = ref<HTMLElement | null>(null)
const buttons = new Map<string, HTMLButtonElement>()
let timer: ReturnType<typeof setTimeout> | undefined
const available = computed(() => props.items.map((i) => i.key))
const items = computed(() =>
  order.value.flatMap((k) => {
    const i = props.items.find((v) => v.key === k)
    return i ? [i] : []
  }),
)
function load() {
  let saved: unknown
  try {
    saved = JSON.parse(localStorage.getItem(props.storageKey) || 'null')
  } catch {}
  order.value = normalizeOrder(saved, available.value)
}
watch([available, () => props.storageKey], load, { immediate: true })
function say(text: string) {
  notice.value = text
  clearTimeout(timer)
  timer = setTimeout(() => (notice.value = ''), 2200)
}
function save(next: string[]) {
  order.value = normalizeOrder(next, available.value)
  try {
    localStorage.setItem(props.storageKey, JSON.stringify(order.value))
    say('Navigation order saved')
  } catch {
    say('Order changed for this session; browser storage is unavailable')
  }
}
function focus(key: string) {
  void nextTick(() => buttons.get(key)?.focus())
}
function step(key: string, delta: -1 | 1) {
  save(moveStep(order.value, key, delta))
  focus(key)
}
function begin(event: DragEvent, key: string) {
  dragged.value = key
  menu.value = null
  event.dataTransfer?.setData('application/x-metteur-navigation', key)
  if (event.dataTransfer) event.dataTransfer.effectAllowed = 'move'
}
function hover(event: DragEvent, key: string) {
  if (!dragged.value) return
  event.preventDefault()
  over.value = key
  const box = (event.currentTarget as HTMLElement).getBoundingClientRect()
  after.value = event.clientY > box.top + box.height / 2
  if (event.dataTransfer) event.dataTransfer.dropEffect = 'move'
}
function drop(event: DragEvent, key: string) {
  if (!dragged.value) return
  event.preventDefault()
  const moving = dragged.value
  if (event.dataTransfer?.getData('application/x-metteur-navigation') === moving)
    save(moveRelative(order.value, moving, key, after.value))
  end()
  focus(moving)
}
function end() {
  dragged.value = ''
  over.value = ''
}
function openMenu(key: string, event: MouseEvent | KeyboardEvent) {
  event.preventDefault()
  notice.value = ''
  clearTimeout(timer)
  const box = (event.currentTarget as HTMLElement).getBoundingClientRect()
  menu.value = {
    key,
    x: Math.max(8, Math.min(box.right + 8, innerWidth - 218)),
    y: Math.max(8, Math.min(box.top, innerHeight - 152)),
  }
  void nextTick(() =>
    menuEl.value?.querySelector<HTMLButtonElement>('button:not(:disabled)')?.focus(),
  )
}
function close() {
  const key = menu.value?.key
  menu.value = null
  if (key) focus(key)
}
function action(kind: 'up' | 'down' | 'reset') {
  const key = menu.value?.key || props.active || ''
  if (kind === 'reset') {
    save(available.value)
    focus(key)
  } else step(key, kind === 'up' ? -1 : 1)
  menu.value = null
}
function outside(event: PointerEvent) {
  if (menu.value && !menuEl.value?.contains(event.target as Node)) menu.value = null
}
function menuKeys(e: KeyboardEvent) {
  if (e.key === 'Escape') {
    e.preventDefault()
    close()
    return
  }
  if (e.key === 'Tab') {
    menu.value = null
    return
  }
  if (!['ArrowUp', 'ArrowDown', 'Home', 'End'].includes(e.key)) return
  e.preventDefault()
  const entries = [...menuEl.value!.querySelectorAll<HTMLButtonElement>('button:not(:disabled)')]
  const index = entries.indexOf(document.activeElement as HTMLButtonElement)
  const next =
    e.key === 'Home'
      ? 0
      : e.key === 'End'
        ? entries.length - 1
        : (index + (e.key === 'ArrowDown' ? 1 : -1) + entries.length) % entries.length
  entries[next]?.focus()
}
function storage(e: StorageEvent) {
  if (e.key === props.storageKey) load()
}
onMounted(() => {
  document.addEventListener('pointerdown', outside)
  window.addEventListener('storage', storage)
})
onBeforeUnmount(() => {
  clearTimeout(timer)
  document.removeEventListener('pointerdown', outside)
  window.removeEventListener('storage', storage)
})
</script>
<template>
  <nav class="activity-rail" aria-label="Main navigation">
    <div class="rail-items">
      <button
        v-for="item in items"
        :key="item.key"
        :ref="
          (el) => {
            if (el) buttons.set(item.key, el as HTMLButtonElement)
            else buttons.delete(item.key)
          }
        "
        class="rail-item"
        :class="{
          active: active === item.key,
          dragging: dragged === item.key,
          'drop-before': over === item.key && !after,
          'drop-after': over === item.key && after,
        }"
        type="button"
        :aria-label="item.label"
        :aria-current="active === item.key ? 'page' : undefined"
        :title="item.label"
        :data-nav-key="item.key"
        draggable="true"
        @click="!dragged && emit('select', item.key)"
        @dragstart="begin($event, item.key)"
        @dragover="hover($event, item.key)"
        @drop="drop($event, item.key)"
        @dragend="end"
        @contextmenu="openMenu(item.key, $event)"
        @keydown.alt.up.prevent="step(item.key, -1)"
        @keydown.alt.down.prevent="step(item.key, 1)"
        @keydown.shift.f10="openMenu(item.key, $event)"
      >
        <component :is="item.icon" :size="18" aria-hidden="true" />
      </button>
    </div>
    <div class="rail-bottom">
      <button
        class="rail-item"
        aria-label="Navigation order"
        title="Navigation Order"
        @click="openMenu(active || order[0] || '', $event)"
      >
        <SlidersHorizontal :size="17" />
      </button>
      <button
        class="rail-item"
        :class="{ active: settingsActive }"
        aria-label="Settings"
        title="Settings"
        @click="emit('settings')"
      >
        <Settings :size="18" />
      </button>
    </div>
  </nav>
  <Teleport to="body"
    ><div
      v-if="menu"
      ref="menuEl"
      class="rail-menu"
      role="menu"
      aria-label="Navigation order"
      :style="{ left: `${menu.x}px`, top: `${menu.y}px` }"
      @keydown="menuKeys"
    >
      <button role="menuitem" :disabled="order.indexOf(menu.key) <= 0" @click="action('up')">
        <ArrowUp :size="15" />Move up<span>Alt ↑</span></button
      ><button
        role="menuitem"
        :disabled="order.indexOf(menu.key) >= order.length - 1"
        @click="action('down')"
      >
        <ArrowDown :size="15" />Move down<span>Alt ↓</span>
      </button>
      <div class="rail-menu-divider" />
      <button role="menuitem" @click="action('reset')"><RotateCcw :size="15" />Reset order</button>
    </div>
    <div v-if="notice" class="rail-notice" role="status">{{ notice }}</div></Teleport
  >
</template>
<style scoped>
.activity-rail {
  width: 52px;
  flex: none;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 12px;
  padding: 10px 5px;
  background: var(--sidebar);
  border-right: 1px solid var(--divider);
  min-height: 0;
}
.rail-items {
  display: flex;
  flex-direction: column;
  gap: 4px;
  overflow-y: auto;
  overflow-x: hidden;
  scrollbar-width: none;
  width: 42px;
  padding: 2px;
}
.rail-item {
  position: relative;
  display: grid;
  place-items: center;
  width: 38px;
  height: 38px;
  min-height: 38px;
  border: 1px solid transparent;
  border-radius: 9px;
  color: var(--muted-foreground);
  background: transparent;
  transition:
    background 0.14s,
    color 0.14s,
    opacity 0.14s;
  cursor: pointer;
}
.rail-item:hover {
  background: var(--hover);
  color: var(--foreground);
}
.rail-item.active {
  color: var(--primary);
  background: var(--primary-soft);
  border-color: color-mix(in srgb, var(--primary) 15%, transparent);
}
.rail-item.active:before {
  content: '';
  position: absolute;
  left: -4px;
  width: 2px;
  height: 16px;
  border-radius: 2px;
  background: var(--primary);
}
.rail-item.dragging {
  opacity: 0.35;
}
.rail-item.drop-before:after,
.rail-item.drop-after:after {
  content: '';
  position: absolute;
  left: 1px;
  right: 1px;
  height: 2px;
  border-radius: 2px;
  background: var(--primary);
}
.rail-item.drop-before:after {
  top: -3px;
}
.rail-item.drop-after:after {
  bottom: -3px;
}
.rail-bottom {
  display: grid;
  gap: 4px;
  margin-top: auto;
  padding-top: 10px;
  border-top: 1px solid var(--divider);
}
.rail-item:focus-visible {
  outline: 2px solid var(--primary);
  outline-offset: -2px;
}
.rail-menu {
  position: fixed;
  z-index: 120;
  min-width: 210px;
  padding: 6px;
  border: 1px solid var(--border);
  border-radius: 12px;
  background: var(--popover);
  backdrop-filter: blur(20px);
  box-shadow: var(--shadow-popover);
}
.rail-menu button {
  width: 100%;
  display: flex;
  align-items: center;
  gap: 9px;
  font-size: 13px;
  text-align: left;
  padding: 10px 9px;
  border-radius: 7px;
}
.rail-menu button:hover,
.rail-menu button:focus-visible {
  background: var(--hover);
  outline: none;
}
.rail-menu button:disabled {
  opacity: 0.35;
  cursor: default;
}
.rail-menu button span {
  margin-left: auto;
  color: var(--muted-foreground);
  font-size: 11px;
}
.rail-menu-divider {
  height: 1px;
  background: var(--divider);
  margin: 5px;
}
.rail-notice {
  position: fixed;
  bottom: 22px;
  left: 68px;
  z-index: 125;
  padding: 11px 16px;
  max-width: 380px;
  border: 1px solid var(--border);
  border-radius: 10px;
  background: var(--popover);
  box-shadow: var(--shadow-popover);
  font-size: 13px;
  color: var(--foreground);
}
@media (prefers-reduced-motion: reduce) {
  .rail-item {
    transition: none;
  }
}
</style>
