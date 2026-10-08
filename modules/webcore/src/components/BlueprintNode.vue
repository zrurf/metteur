<script setup lang="ts">
import { computed, inject } from 'vue'
import { Handle, Position } from '@vue-flow/core'
import type { BlueprintPin } from '@/core'
import { DATA_COLORS } from '@/lib/blueprint'
import { valueText } from '@/core/node-catalog'

/**
 * Unreal-style blueprint node.
 *
 * Each pin is its own row inside the node body, so the box grows to fit its
 * pins and every handle anchors to the row that labels it (no floating pins).
 * Exec pins render as right-pointing rounded arrows on the control-flow axis
 * (they always face right, whether in or out) tinted like the exec wire; an
 * unconnected exec pin is hollow, a connected one is solid. Data pins are
 * circles coloured by value type — hollow when unwired, filled once connected.
 * Data input pins that aren't wired carry an inline, type-aware value editor
 * like Unreal's default-pin values, which is replaced by the pin label once
 * connected.
 */

const props = defineProps<{
  id: string
  data: {
    title: string
    category: string
    inputs: BlueprintPin[]
    outputs: BlueprintPin[]
    /** Per-pin default values for data inputs (pin id → raw value). */
    values?: Record<string, string>
    /** Input handle ids that already have an incoming wire. */
    connectedIn?: string[]
    /** Output handle ids that already have an outgoing wire. */
    connectedOut?: string[]
    /** Execution status driven by the live audit trail. */
    status?: 'running' | 'done'
  }
  readonly?: boolean
  selected?: boolean
}>()

/** Right-click a connected pin → daemon menu with a Disconnect action (item 5). */
const pinContext = inject<(e: MouseEvent, nodeId: string, pin: { id: string; kind: string }) => void>(
  'bp-pin-context',
)

/** Accent colour per category, mirroring Unreal's colour-coded node families. */
const ACCENT: Record<string, string> = {
  event: '#3f8cff',
  module: '#8b5cf6',
  action: '#f2994a',
  flow: '#2fbf8f',
}

/** Value editor flavour per pin type, matching Unreal's typed pin editors. */
const INPUT_TYPE: Record<string, string> = {
  number: 'number',
  float: 'number',
  int: 'number',
  bool: 'checkbox',
  string: 'text',
  choice: 'select',
}

const accent = computed(() => ACCENT[props.data.category] ?? '#8b8ba0')
/** Handles that are wired, keyed by side, drive solid/hollow pin styling. */
const connectedIn = computed(() => new Set(props.data.connectedIn ?? []))
const connectedOut = computed(() => new Set(props.data.connectedOut ?? []))
const valueOf = (pin: BlueprintPin) => props.data.values?.[pin.id] ?? valueText(pin.default)

/** Fill colour for a data pin, resolved from its value type. */
function dataColor(pin: BlueprintPin): string {
  return DATA_COLORS[pin.type ?? 'any'] ?? DATA_COLORS.any
}

function handleClass(kind: string): string {
  return [
    'metteur-handle',
    kind === 'exec-in' || kind === 'exec-out' ? 'metteur-handle--exec' : 'metteur-handle--data',
    kind === 'exec-out' || kind === 'data-out' ? 'metteur-handle--out' : 'metteur-handle--in',
  ].join(' ')
}

/** Exec pins are solid once wired, hollow otherwise (Unreal). */
function execClass(kind: string, connected: boolean): string {
  return kind.startsWith('exec') && connected ? 'is-connected' : ''
}

/** Data pins are small circles — a thick type-coloured rim when hollow, a solid
 *  fill once wired. Using an inset border (not a box-shadow halo) keeps the pin
 *  compact while the heavier hollow rim keeps it legible in both themes. */
function dataPinStyle(pin: BlueprintPin, connected: boolean): Record<string, string> {
  const c = dataColor(pin)
  return connected
    ? { background: c, border: `2px solid ${c}`, boxSizing: 'border-box' }
    : { background: 'transparent', border: `2.5px solid ${c}`, boxSizing: 'border-box' }
}

/** Whether a pin is an execution pin (exec in or exec out). */
function isExec(pin: BlueprintPin): boolean {
  return pin.kind === 'exec-in' || pin.kind === 'exec-out'
}

/** Whether an exec pin is connected on the relevant side. */
function execConnected(pin: BlueprintPin): boolean {
  return pin.kind === 'exec-in' ? connectedIn.value.has(pin.id) : connectedOut.value.has(pin.id)
}

/** Whether a data input pin shows its inline value editor instead of a label. */
function editsValue(pin: BlueprintPin): boolean {
  return !props.readonly && pin.kind === 'data-in' && !connectedIn.value.has(pin.id)
}

function setValue(pin: BlueprintPin, value: string) {
  const values = props.data.values ?? (props.data.values = {})
  if (value === '') delete values[pin.id]
  else values[pin.id] = value
}

/** Read the raw value from a text/number input or a checkbox. */
function onValue(pin: BlueprintPin, e: Event) {
  const el = e.target as HTMLInputElement
  setValue(pin, pin.type === 'bool' ? (el.checked ? 'true' : 'false') : el.value)
}
</script>

<template>
  <div
    class="metteur-node"
    :class="{
      'metteur-node--selected': selected,
      'metteur-node--running': data.status === 'running',
      'metteur-node--done': data.status === 'done',
    }"
    style="width: 208px"
  >
    <div class="metteur-node__header" :style="{ background: accent }">
      <span v-if="data.status === 'running'" class="metteur-node__live-dot" />
      <span>{{ data.title }}</span>
    </div>

    <div class="metteur-node__body">
      <div class="metteur-node__io">
        <div
          v-for="p in data.inputs"
          :key="p.id"
          class="metteur-pin"
          :class="p.kind === 'data-in' ? 'metteur-pin--data' : ''"
          @contextmenu="pinContext?.( $event, id, p )"
        >
          <Handle
            :id="p.id"
            type="target"
            :position="Position.Left"
            class="metteur-handle"
            :class="[handleClass(p.kind), p.kind === 'exec-in' ? execClass(p.kind, execConnected(p)) : '']"
            :style="p.kind === 'data-in' ? dataPinStyle(p, connectedIn.has(p.id)) : {}"
          />
          <svg
            v-if="isExec(p)"
            class="metteur-exec-glyph metteur-exec-glyph--in"
            :class="execConnected(p) ? 'is-connected' : ''"
            viewBox="0 0 14 14"
            aria-hidden="true"
          >
            <path class="metteur-exec-glyph__path" d="M2.6 1.8 L2.6 12.2 C2.6 12.8 3.2 13.05 3.7 12.7 L11.6 7.6 C12.05 7.3 12.05 6.7 11.6 6.4 L3.7 1.3 C3.2 0.95 2.6 1.2 2.6 1.8 Z" />
          </svg>
          <template v-if="editsValue(p)">
            <select
              v-if="p.choices?.length"
              class="metteur-value nodrag"
              :value="valueOf(p)"
              @change="onValue(p, $event)"
            >
              <option v-for="c in p.choices ?? []" :key="c" :value="c">{{ c }}</option>
            </select>
            <input
              v-else
              :type="INPUT_TYPE[p.type ?? 'string'] ?? 'text'"
              class="metteur-value nodrag"
              :placeholder="p.name"
              :value="valueOf(p)"
              :checked="p.type === 'bool' ? valueOf(p) === 'true' : undefined"
              @input="onValue(p, $event)"
            />
          </template>
          <span v-else-if="p.name" class="metteur-pin__name">{{ p.name }}</span>
        </div>
      </div>

      <div class="metteur-node__io metteur-node__io--right">
        <div
          v-for="p in data.outputs"
          :key="p.id"
          class="metteur-pin metteur-pin--out"
          :class="p.kind === 'data-out' ? 'metteur-pin--data' : ''"
          @contextmenu="pinContext?.( $event, id, p )"
        >
          <span v-if="p.name" class="metteur-pin__name">{{ p.name }}</span>
          <Handle
            :id="p.id"
            type="source"
            :position="Position.Right"
            class="metteur-handle"
            :class="[handleClass(p.kind), p.kind === 'exec-out' ? execClass(p.kind, execConnected(p)) : '']"
            :style="p.kind === 'data-out' ? dataPinStyle(p, connectedOut.has(p.id)) : {}"
          />
          <svg
            v-if="isExec(p)"
            class="metteur-exec-glyph metteur-exec-glyph--out"
            :class="execConnected(p) ? 'is-connected' : ''"
            viewBox="0 0 14 14"
            aria-hidden="true"
          >
            <path class="metteur-exec-glyph__path" d="M2.6 1.8 L2.6 12.2 C2.6 12.8 3.2 13.05 3.7 12.7 L11.6 7.6 C12.05 7.3 12.05 6.7 11.6 6.4 L3.7 1.3 C3.2 0.95 2.6 1.2 2.6 1.8 Z" />
          </svg>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.metteur-node {
  position: relative;
  border-radius: 8px;
  background: var(--surface);
  box-shadow: var(--shadow-card);
  font-size: 12px;
}

.metteur-node--selected {
  box-shadow: var(--shadow-card), 0 0 0 2px var(--ring);
}

.metteur-node__header {
  display: flex;
  height: 24px;
  align-items: center;
  justify-content: center;
  border-radius: 8px 8px 0 0;
  color: #ffffff;
  font-size: 12px;
  font-weight: 600;
  letter-spacing: 0.01em;
}

.metteur-node__body {
  display: flex;
  gap: 4px;
  padding: 6px 10px;
}

.metteur-node__io {
  display: flex;
  min-width: 0;
  flex: 1;
  flex-direction: column;
}

.metteur-node__io--right {
  align-items: flex-end;
}

.metteur-pin {
  position: relative;
  display: flex;
  min-height: 22px;
  align-items: center;
  gap: 6px;
  color: var(--muted-foreground);
}

.metteur-pin--out {
  justify-content: flex-end;
}

.metteur-pin--data .metteur-pin__name {
  color: var(--foreground);
  opacity: 0.9;
}

.metteur-pin__name {
  font-size: 11px;
  white-space: nowrap;
}

/* Inline value editor for unconnected data inputs (Unreal default-pin style). */
.metteur-value {
  width: 110px;
  height: 17px;
  border: 0;
  border-radius: 4px;
  background: var(--input);
  box-shadow: inset 0 0 0 1px var(--border);
  padding: 0 6px;
  font-size: 11px;
  color: var(--foreground);
  outline: none;
  transition: box-shadow 120ms ease;
}

.metteur-value:focus {
  box-shadow: inset 0 0 0 1px var(--ring);
}

.metteur-node :deep(.vue-flow__handle.metteur-handle) {
  position: absolute;
  top: 50%;
}

.metteur-node :deep(.vue-flow__handle.metteur-handle--in) {
  left: -7px;
  transform: translate(-50%, -50%);
}

.metteur-node :deep(.vue-flow__handle.metteur-handle--out) {
  right: -7px;
  transform: translate(50%, -50%);
}

/* Exec pins are right-pointing rounded arrows that always face right (out of
   the control-flow axis), tinted like the exec wire. Hollow until wired,
   solid when connected — matching Unreal. The arrow is an inline <svg> stroked
   directly with `var(--exec-wire)` so it always picks up the theme exec-wire
   colour (a data-URI background image cannot see CSS variables, which is why it
   previously rendered black in either theme). The handle itself stays
   transparent and only serves as the connect target. */
.metteur-node :deep(.vue-flow__handle.metteur-handle--exec) {
  width: 15px;
  height: 15px;
  border: 0;
  background: transparent;
}

/* Layout the arrow overlay at the same spot as the handle it sits over. */
.metteur-exec-glyph {
  position: absolute;
  top: 50%;
  width: 15px;
  height: 15px;
  color: var(--exec-wire);
  pointer-events: none;
  z-index: 1;
}
.metteur-exec-glyph--in {
  left: -7px;
  transform: translate(-50%, -50%);
}
.metteur-exec-glyph--out {
  right: -7px;
  transform: translate(50%, -50%);
}
.metteur-exec-glyph__path {
  fill: none;
  stroke: var(--exec-wire);
  stroke-width: 1.25;
  stroke-linecap: round;
  stroke-linejoin: round;
}
.metteur-exec-glyph.is-connected .metteur-exec-glyph__path {
  fill: var(--exec-wire);
}

/* Data pins are circles; inline `background`/`box-shadow` decide hollow vs
   solid at render time. The base size keeps a hollow pin compact while its
   thicker rim stays legible in both themes without washing out the type hue. */
.metteur-node :deep(.vue-flow__handle.metteur-handle--data) {
  width: 8px;
  height: 8px;
  border-radius: 999px;
  border: 0;
}

.metteur-node :deep(.vue-flow__handle.connectingto),
.metteur-node :deep(.vue-flow__handle.valid) {
  box-shadow: 0 0 0 3px var(--selection);
}
</style>