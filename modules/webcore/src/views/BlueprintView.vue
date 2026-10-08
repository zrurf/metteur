<script setup lang="ts">
import { blueprintEdgeVisual, CATEGORY_ACCENT as ACCENT } from '@/lib/blueprint-edges'
import { computed, nextTick, onBeforeUnmount, onMounted, provide, ref, watch } from 'vue'
import { useVueFlow, VueFlow, ConnectionMode } from '@vue-flow/core'
import { Background } from '@vue-flow/background'
import { Controls } from '@vue-flow/controls'
import {
  Activity,
  FileDown,
  FileUp,
  GitBranch,
  Pause,
  Play,
  Redo2,
  Save,
  Square,
  Undo2,
  Workflow,
} from '@lucide/vue'
import type {
  Node as FlowNode,
  Edge as FlowEdge,
  Connection,
  EdgeMouseEvent,
  NodeComponent,
  NodeDragEvent,
  NodeMouseEvent,
  OnConnectStartParams,
} from '@vue-flow/core'
import '@vue-flow/core/dist/style.css'
import '@vue-flow/core/dist/theme-default.css'
import type { Blueprint, BlueprintEdge, BlueprintNode, BlueprintPin, NodeCategory } from '@/core'
import { gateway } from '@/core'
import { useFeedbackStore } from '@/stores/feedback'
import { useWorkspaceStore } from '@/stores/workspace'
import { fileNameOf, useBlueprintStore } from '@/stores/blueprint'
import { useSurfaceNavigation } from '@/lib/surface'
import { useExecutionStore } from '@/stores/execution'
import { useTabsStore } from '@/stores/tabs'
import { useRightPanelStore } from '@/stores/right-panel'
import BlueprintNodeComp from '@/components/BlueprintNode.vue'
import BlueprintAuditDrawer from '@/components/BlueprintAuditDrawer.vue'
import ContextMenu, { type MenuGroup } from '@/components/ContextMenu.vue'
import FileVersionPanel from '@/components/FileVersionPanel.vue'
import NodeInspector from '@/components/NodeInspector.vue'
import FilePickerDialog, { type FilePick } from '@/components/FilePickerDialog.vue'
import { CATEGORIES, execInOf, isPinCompatible, makeCallFunctionNode, makeFlowNode, makeFileReference, categoryFor, NODE_PRESETS, uuid } from '@/lib/blueprint'

const workspace = useWorkspaceStore()
const store = useBlueprintStore()
const execution = useExecutionStore()
const { openSurface: openExecutionSurface } = useSurfaceNavigation()
const tabs = useTabsStore()
const rightPanel = useRightPanelStore()
const feedback = useFeedbackStore()
const props = defineProps<{ filePath?: string }>()
const { screenToFlowCoordinate, viewport, getNodes } = useVueFlow()

/** Palette categories start folded so a long node list stays navigable. */
const PALETTE_COLLAPSED = ['Events', 'Module', 'Actions', 'Flow', 'Functions']

/** Whether the docked audit drawer is open. */
const auditOpen = ref(false)

// Execution facts are rendered on the immutable runtime graph in Execution.
// This canvas remains the editable draft, including while another version runs.

const statusChip = computed(() => {
  switch (execution.status) {
    case 'running':
      return 'bg-primary/15 text-primary'
    case 'paused':
      return 'bg-amber-500/15 text-amber-600 dark:text-amber-400'
    case 'cancelled':
      return 'bg-danger-soft text-danger'
    case 'finished':
      return 'bg-emerald-500/15 text-emerald-600 dark:text-emerald-400'
    default:
      return 'text-muted-foreground'
  }
})

const BLUEPRINT_ID = 'bp-build-feature'
const BLUEPRINT_TITLE = 'build-feature.blueprint'

/** Editor cache key for the open file; the graph store is keyed by path. */
const fileKey = computed(() => props.filePath ?? `blueprints/${BLUEPRINT_ID}.blueprint`)
const blueprintTitle = computed(() => props.filePath?.split(/[\\/]/).pop() ?? BLUEPRINT_TITLE)

interface SelectableFlowNode {
  id: string
  position: { x: number; y: number }
  selected?: boolean
  data?: {
    kind?: string
    nodeType?: string
    data?: Record<string, unknown>
    title?: string
    category?: string
    inputs?: BlueprintPin[]
    outputs?: BlueprintPin[]
    values?: Record<string, string>
    filledIns?: string[]
    connectedIn?: string[]
    connectedOut?: string[]
    /** Visual-audit status set from the live execution trail. */
    status?: 'running' | 'done'
  }
}
const flowNodes = ref<SelectableFlowNode[]>([])
/** Light edge shape for the local canvas state. Vue-flow's generic `Edge`
 *  overflows Volar's instantiation depth whenever it gets inferred, so edges
 *  stay structurally minimal here and are widened only at the library
 *  boundary (see {@link canvasEdges}). */
interface LocalEdge {
  id: string
  source: string
  target: string
  sourceHandle?: string | null
  targetHandle?: string | null
  label?: string
  class?: string
  animated?: boolean
  style?: string | Record<string, string>
  labelStyle?: { fontSize?: number; fill?: string }
  labelBgStyle?: { fill?: string }
}
const flowEdges = ref<LocalEdge[]>([])
/** vue-flow's `edges` v-model bridge; casts at the boundary so the heavy
 *  generic never appears in local expressions. */
const canvasEdges = computed({
  get: () => flowEdges.value as unknown as FlowEdge[],
  set: (v: FlowEdge[]) => {
    flowEdges.value = v as unknown as LocalEdge[]
  },
})
const dirty = ref(false)
/** Serialized graph as last written to disk for the open file (kept in the
 *  store per blueprint id). The dirty watchers compare the live graph against
 *  this, so switching tabs to a cached-but-edited file still shows the unsaved
 *  dot — the baseline no longer resets to the in-memory (possibly dirty) graph. */
const savedBaseline = computed(() => store.savedKeys[fileKey.value] ?? '')
const flowRef = ref<InstanceType<typeof VueFlow>>()

/** Stable key of everything the user can edit, computed over the same
 *  node/edge shape the store persists (see `stores/blueprint` `keyFor`), so a
 *  saved file always matches its baseline exactly and never keeps a phantom
 *  unsaved dot. Pin connected-state is derived from wires and excluded. */
function graphKey(): string {
  const nodes = flowNodes.value.map(toBpNode)
  const edges = flowEdges.value.map((e) => ({
    id: e.id,
    source: e.source,
    sourceHandle: e.sourceHandle ?? undefined,
    target: e.target,
    targetHandle: e.targetHandle ?? undefined,
    label: typeof e.label === 'string' ? e.label : undefined,
  }))
  return store.keyFor(nodes, edges)
}

/** Selected node id, driving Delete and the node context menu. */
const selectedId = ref<string | null>(null)
/** The selected flow node, if any (drives the inspector side panel). */
const selectedNode = computed(() => flowNodes.value.find((n) => n.id === selectedId.value))
/** The selected node's pin payload, fed to the inspector. */
const selectedNodeData = computed(() => {
  const data = selectedNode.value?.data as
    | { title?: string; category?: string; inputs?: BlueprintPin[]; outputs?: BlueprintPin[]; values?: Record<string, string> }
    | undefined
  return data ?? null
})
/** Selected edge id, deletable with Delete. */
const selectedEdgeId = ref<string | null>(null)
/** Floating context menu payload, positioned in screen coordinates. */
const menu = ref<{
  kind: 'palette' | 'selection' | 'node' | 'edge' | 'pin'
  x: number
  y: number
  nodeId?: string
  edgeId?: string
  pinId?: string
  /** Set for a palette opened by releasing a connect-drag on blank, so the
   *  pane-click that follows does not tear it back down. */
  fromConnect?: boolean
} | null>(null)
/** A pending wire drag that landed on empty canvas, waiting for the add-node
 *  palette. Mirrors Unreal: a wire is always pulled from an *output* pin, and
 *  dropping it on blank opens the node palette with a fresh line from that
 *  output (item 2). */
interface WireIntent {
  kind: 'direct'
  source: string
  sourceHandle: string
}
const wireIntent = ref<WireIntent | null>(null)
/** Source of an in-flight connection drag (until it resolves to an edge or menu). */
const dragStart = ref<DragIntent | null>(null)
const edgeMade = ref(false)
/** A fresh wire from the source pin to the palette while choosing a node,
 *  drawn as an overlay so the pending connection stays visible until the menu
 *  closes (matching Unreal's release-on-blank → palette → auto-connect). */
const pendingWire = ref<{
  x1: number
  y1: number
  x2: number
  y2: number
  stroke: string
  width: number
} | null>(null)
/** Smooth bezier path for the pending wire, from the source pin to the
 *  palette anchor. */
const pendingWirePath = computed(() => {
  const w = pendingWire.value
  if (!w) return ''
  const mx = (w.x1 + w.x2) / 2
  return `M ${w.x1} ${w.y1} C ${mx} ${w.y1}, ${mx} ${w.y2}, ${w.x2} ${w.y2}`
})
/** Copied nodes for Ctrl+C / Ctrl+V / Ctrl+D. */
const clipboard = ref<SelectableFlowNode[]>([])
/** Right-button selection marquee, in client coordinates. */
const marquee = ref<{ x1: number; y1: number; x2: number; y2: number } | null>(null)
/** Bounds (client coords) of the region box-selected by the last right-drag,
 *  used to decide whether a plain right-click should open the select menu. */
const boxRect = ref<{ minX: number; minY: number; maxX: number; maxY: number } | null>(null)
/** Timestamp after which the right-click menu may open again (suppresses the
 *  contextmenu that VueFlow emits at the end of a right-drag box-select). */
let suppressMenuUntil = 0
/** Wrapper of the canvas, the origin for the marquee overlay coordinates. */
const flowWrap = ref<HTMLDivElement | null>(null)

/* Undo / redo stack                                                    */

interface GraphSnapshot {
  nodes: SelectableFlowNode[]
  /** Plain edge shape that re-imports into the canvas on restore. */
  edges: Array<{
    id: string
    source: string
    sourceHandle?: string
    target: string
    targetHandle?: string
    label?: string
    class?: string
    animated?: boolean
    style?: string
  }>
}
const past = ref<GraphSnapshot[]>([])
const future = ref<GraphSnapshot[]>([])

const graphSnapshot = (): GraphSnapshot => ({
  nodes: JSON.parse(JSON.stringify(flowNodes.value)),
  edges: JSON.parse(JSON.stringify(flowEdges.value)) as GraphSnapshot['edges'],
})

/** Record the current graph before a mutation (caps the stack at 50 steps). */
function commit() {
  past.value = [...past.value.slice(-49), graphSnapshot()]
  future.value = []
}

function restore(s: GraphSnapshot) {
  flowNodes.value = JSON.parse(JSON.stringify(s.nodes))
  flowEdges.value = JSON.parse(JSON.stringify(s.edges))
  dirty.value = true
}

function undoGraph() {
  if (!past.value.length) return
  future.value = [graphSnapshot(), ...future.value]
  restore(past.value.pop()!)
}

function redoGraph() {
  if (!future.value.length) return
  past.value.push(graphSnapshot())
  restore(future.value.shift()!)
}

/** Resolve the pin kind owning a handle id (exec pins are identified by kind,
 *  not by a name prefix, since ids are UUIDs). */
function pinKindOf(handleId?: string): BlueprintPin['kind'] | undefined {
  if (!handleId) return undefined
  for (const n of flowNodes.value) {
    const pin =
      (n.data?.inputs as BlueprintPin[] | undefined)?.find((p) => p.id === handleId) ??
      (n.data?.outputs as BlueprintPin[] | undefined)?.find((p) => p.id === handleId)
    if (pin) return pin.kind
  }
  return undefined
}
const isExecHandle = (h?: string): boolean => {
  const k = pinKindOf(h)
  return k === 'exec-in' || k === 'exec-out'
}

/** Category tint, aligned with the palette swatches in the context menu. */


/** The live registry controls available kinds, including extensions without visual presets. */
const kindList = computed(() => store.signatures.map((s) => s.kind))
const signatureOf = (kind: string) => store.signatures.find((s) => s.kind === kind)
const categoryOf = (kind: string) => categoryFor(kind, signatureOf(kind)?.nodeType)

/** Load the graph for the currently open blueprint file and reset editor state. */
async function loadBlueprint() {
  const ws = workspace.active
  if (!ws) return
  await store.load(ws.path, fileKey.value)
  flowNodes.value = store.nodes.map(toFlowNode)
  flowEdges.value = store.edges.map(toFlowEdge)
  // The store keeps the on-disk baseline per file, so a partially saved graph
  // that was edited again is still reported dirty after a tab round-trip.
  await nextTick()
  dirty.value = graphKey() !== savedBaseline.value
  past.value = []
  future.value = []
  selectedId.value = null
  selectedEdgeId.value = null
  clearDrag()
  clearPendingWire()
  requestAnimationFrame(() => flowRef.value?.fitView({ padding: 0.25 }))
}

onMounted(() => {
  void loadBlueprint()
  void store.loadFunctions(workspace.active?.path ?? '')
  window.addEventListener('keydown', onKeydown)
  // Capture phase: Vue Flow stops propagation on its own pane handlers, so a
  // bubble listener misses the canvas press. Capture catches it regardless.
  flowWrap.value?.addEventListener('mousedown', onCanvasMouseDown, true)
})

// Switching blueprint tabs loads that file's own graph instead of reusing the
// previous canvas (item 9).
watch(() => props.filePath, () => void loadBlueprint())

/** The explorer's "Add to Blueprint" lands a file reference on this canvas.
 *  Immediate mode also drains a reference queued while no blueprint was open. */
watch(
  () => store.pendingRef,
  (p) => {
    if (!p) return
    store.consumeReference()
    addReferenceNode(p.path)
  },
  { immediate: true },
)

onBeforeUnmount(() => {
  window.removeEventListener('keydown', onKeydown)
  flowWrap.value?.removeEventListener('mousedown', onCanvasMouseDown, true)
})

/** Keep each node's connected-state in sync with the wires (drives pin styling
 *  and whether data-input inline editors are hidden). */
watch(
  () => flowEdges.value,
  () => {
    const byTarget = new Map<string, Set<string>>()
    const bySource = new Map<string, Set<string>>()
    for (const e of flowEdges.value) {
      if (e.target && e.targetHandle) {
        const t = byTarget.get(e.target) ?? new Set<string>()
        t.add(e.targetHandle)
        byTarget.set(e.target, t)
      }
      if (e.source && e.sourceHandle) {
        const s = bySource.get(e.source) ?? new Set<string>()
        s.add(e.sourceHandle)
        bySource.set(e.source, s)
      }
    }
    for (const n of flowNodes.value) {
      const ins = byTarget.get(n.id) ?? new Set<string>()
      const outs = bySource.get(n.id) ?? new Set<string>()
      n.data!.filledIns = [...ins].filter((h) => pinOf(n.id, h)?.kind === 'data-in')
      n.data!.connectedIn = [...ins]
      n.data!.connectedOut = [...outs]
    }
  },
  { deep: false },
)

/** Structural changes to the graph (value edits, moves, added/removed nodes or
 *  wires) are compared against the file's on-disk baseline. The state we derive
 *  from wires (connectedIn/connectedOut) is excluded from {@link graphKey}, so
 *  it never spurious-dirties a pristine file (item 2c). */
watch(
  flowNodes,
  () => {
    dirty.value = graphKey() !== savedBaseline.value
  },
  { deep: true },
)
watch(flowEdges, () => {
  dirty.value = graphKey() !== savedBaseline.value
})

/** Keep the owning tab's unsaved dot synced to the graph's clean/dirty state in
 *  both directions — a clean reload clears any stale dot on this tab. */
watch(dirty, (d) => {
  if (props.filePath) tabs.markDirty(props.filePath, d)
})

/* Graph mapping                                                     */

function edgeVisual(
  sourceHandle: string | undefined,
  sourceNodeId?: string,
): { class: string; animated: boolean; style?: Record<string, string> } {
  const node = flowNodes.value.find((n) => n.id === sourceNodeId)
  return blueprintEdgeVisual(isExecHandle(sourceHandle), (node?.data?.category as NodeCategory) ?? 'module')
}

function toFlowNode(n: BlueprintNode): FlowNode {
  return {
    id: n.id,
    type: 'blueprint',
    position: n.position,
    data: {
      kind: n.type,
      nodeType: n.nodeType,
      data: n.data,
      title: n.title,
      category: n.category,
      inputs: n.inputs,
      outputs: n.outputs,
      values: n.values ?? {},
      filledIns: [] as string[],
      connectedIn: [] as string[],
      connectedOut: [] as string[],
    },
  }
}

function toFlowEdge(e: BlueprintEdge): LocalEdge {
  const visual = edgeVisual(e.sourceHandle, e.source)
  return {
    id: e.id,
    source: e.source,
    sourceHandle: e.sourceHandle,
    target: e.target,
    targetHandle: e.targetHandle,
    label: e.label,
    class: visual.class,
    animated: visual.animated,
    style: visual.style,
    labelStyle: { fontSize: 10, fill: 'var(--subtle)' },
    labelBgStyle: { fill: 'var(--surface)' },
  }
}

function toBpNode(f: SelectableFlowNode): BlueprintNode {
  return {
    id: f.id,
    type: String(f.data?.kind ?? f.data?.title ?? f.id),
    nodeType: f.data?.nodeType,
    data: f.data?.data,
    category: (f.data?.category as BlueprintNode['category']) ?? 'module',
    title: String(f.data?.title ?? f.id),
    position: f.position,
    inputs: (f.data?.inputs as BlueprintNode['inputs']) ?? [],
    outputs: (f.data?.outputs as BlueprintNode['outputs']) ?? [],
    values: (f.data?.values as BlueprintNode['values']) ?? undefined,
    selected: f.selected,
  }
}

/* Connection rules                                                  */

/** Resolve the pin that owns a handle id for a node (input or output side). */
function pinOf(nodeId: string, handleId?: string) {
  if (!handleId) return undefined
  const node = flowNodes.value.find((n) => n.id === nodeId)
  return (
    node?.data?.inputs?.find((p: BlueprintPin) => p.id === handleId) ??
    node?.data?.outputs?.find((p: BlueprintPin) => p.id === handleId)
  )
}

/** Validate a proposed connection: output→input, exec-out→exec-in for flow,
 *  data-out→data-in with compatible types (self-links rejected). */
function isValidConnection(c: Connection): boolean {
  if (!c.sourceHandle || !c.targetHandle) return false
  if (c.source === c.target) return false
  const src = pinOf(c.source, c.sourceHandle)
  const tgt = pinOf(c.target, c.targetHandle)
  if (!src || !tgt) return false
  const exec = src.kind === 'exec-out' && tgt.kind === 'exec-in'
  const data = src.kind === 'data-out' && tgt.kind === 'data-in'
  return exec || (data && isPinCompatible(src.type, tgt.type))
}

/* Unreal-style wire creation                                           */

/**
 * Create a wire the way Unreal does. Vue Flow runs in `strict` mode, so a drag
 * always originates from an output pin. Rules mirror the engine:
 *  - **exec output → exec input**: an exec output holds a single wire, so the
 *    new link replaces any prior wire from that output. An exec *input* may
 *    merge (several upstream paths converging into one is legal).
 *  - **data output → data input**: data inputs are single-slot (new link
 *    replaces the existing one), while data outputs may fan out freely.
 */
function onConnect(connection: Connection) {
  const { source, sourceHandle, target, targetHandle } = connection
  if (!source || !sourceHandle || !target || !targetHandle) return
  if (!isValidConnection(connection)) return
  commit()
  const srcPin = pinOf(source, sourceHandle)
  // Filter with light structural types: vue-flow's `Edge` generics overflow
  // Volar's deep-instantiation limit when inferred from the callback.
  let next = flowEdges.value
  if (srcPin?.kind === 'exec-out') {
    next = next.filter(
      (e: { source: string; sourceHandle?: string | null }) =>
        !(e.source === source && e.sourceHandle === sourceHandle),
    )
  } else if (srcPin?.kind === 'data-out') {
    next = next.filter(
      (e: { target: string; targetHandle?: string | null }) =>
        !(e.target === target && e.targetHandle === targetHandle),
    )
  }
  const edge = toFlowEdge({
    id: uuid(),
    source,
    sourceHandle,
    target,
    targetHandle,
  })
  flowEdges.value = [...next, edge]
  edgeMade.value = true
  dirty.value = true
}

/** Remember the output pin a drag started from (only sources initiate in
 *  strict mode, so this is always the wire's origin). */
function onConnectStart({ nodeId, handleId }: OnConnectStartParams) {
  if (!nodeId || !handleId) return
  dragStart.value = { nodeId, handleId }
  edgeMade.value = false
  wireIntent.value = null
  clearPendingWire()
}

/** A drag released over empty canvas opens the add-node palette filtered to
 *  nodes the source pin can actually feed, with a fresh wire suspended from
 *  that output — exactly Unreal's "release on blank → context menu →
 *  auto-connect" (item 2b). The event is the raw pointer/mouse release event
 *  (see the `connectEnd: MouseEvent | TouchEvent | undefined` hook type), so
 *  the palette opens right under the cursor; an object payload `{event}` from
 *  older versions is also tolerated. */
function onConnectEnd(p?: MouseEvent | TouchEvent | { event?: MouseEvent | TouchEvent }) {
  const start = dragStart.value
  dragStart.value = null
  if (!start || edgeMade.value) return
  const me = p && 'clientX' in p ? (p as MouseEvent) : (p as { event?: MouseEvent } | undefined)?.event
  const rect = flowWrap.value?.getBoundingClientRect()
  const x = me?.clientX ?? (rect?.left ?? 0) + (rect?.width ?? 0) / 2
  const y = me?.clientY ?? (rect?.top ?? 0) + (rect?.height ?? 0) / 2
  wireIntent.value = { kind: 'direct', source: start.nodeId, sourceHandle: start.handleId }
  menu.value = {
    kind: 'palette',
    fromConnect: true,
    x,
    y,
  }
  // Keep the wire visible from the source pin to the palette anchor while the
  // menu stays open; it is cleared once a node is chosen or the menu closes.
  const from = pendingWireFrom(start.nodeId, start.handleId)
  if (from && rect) {
    const srcPin = pinOf(start.nodeId, start.handleId)
    const isExec = srcPin?.kind === 'exec-out'
    const node = flowNodes.value.find((n) => n.id === start.nodeId)
    pendingWire.value = {
      x1: from.x - rect.left,
      y1: from.y - rect.top,
      x2: x - rect.left,
      y2: y - rect.top,
      stroke: isExec ? 'var(--exec-wire)' : (ACCENT[(node?.data?.category as NodeCategory) ?? 'module']),
      width: isExec ? 2 : 1.5,
    }
  }
}

/** Resolve the screen-centre of a node's pin handle, used to anchor the
 *  pending wire while the add-node palette is open. */
function pendingWireFrom(nodeId: string, handleId: string): { x: number; y: number } | null {
  const el = flowWrap.value?.querySelector<HTMLElement>(
    `[data-nodeid="${nodeId}"][data-handleid="${handleId}"]`,
  )
  if (!el) return null
  const r = el.getBoundingClientRect()
  return { x: r.left + r.width / 2, y: r.top + r.height / 2 }
}

/** Tear down any in-flight wire state (drag source + pending palette wire). */
function clearDrag() {
  dragStart.value = null
  wireIntent.value = null
  edgeMade.value = false
}

/** Drop the visible pending wire; called when the palette closes or the user
 *  cancels node placement (also used by `loadBlueprint` on file switches). */
function clearPendingWire() {
  pendingWire.value = null
}

/** Source of an in-flight connection drag (until it resolves to an edge or menu). */
interface DragIntent {
  nodeId: string
  handleId: string
}

/* Context menus                                                     */

function onNodeContextMenu(ev: NodeMouseEvent) {
  const e = ev.event as MouseEvent
  e.preventDefault()
  ev.node.selected = true
  selectedId.value = ev.node.id
  selectedEdgeId.value = null
  menu.value = { kind: 'node', x: e.clientX, y: e.clientY, nodeId: ev.node.id }
}

function onEdgeContextMenu(ev: EdgeMouseEvent) {
  const e = ev.event as MouseEvent
  e.preventDefault()
  ev.edge.selected = true
  selectedId.value = null
  selectedEdgeId.value = ev.edge.id
  menu.value = { kind: 'edge', x: e.clientX, y: e.clientY, edgeId: ev.edge.id }
}

/** Injected into node components: right-click a connected pin opens a menu with
 *  a "Disconnect" action (item 6). Unconnected pins do nothing. */
function onPinContext(
  e: MouseEvent,
  nodeId: string,
  pin: { id: string; kind: string },
) {
  e.preventDefault()
  e.stopPropagation()
  const isIn = pin.kind === 'exec-in' || pin.kind === 'data-in'
  const edge = flowEdges.value.find(
    isIn
      ? (x) => x.target === nodeId && x.targetHandle === pin.id
      : (x) => x.source === nodeId && x.sourceHandle === pin.id,
  )
  if (!edge) return
  menu.value = { kind: 'pin', x: e.clientX, y: e.clientY, nodeId, pinId: pin.id, edgeId: edge.id }
}

provide('bp-pin-context', onPinContext)

const paletteGroups = computed<MenuGroup[]>(() => {
  const groups = CATEGORIES.map((cat) => ({
    label: cat.label,
    color: ACCENT[cat.key],
    items: kindList.value
      .filter((k) => categoryOf(k) === cat.key)
      .map((k) => ({ id: k, label: NODE_PRESETS[k]?.title ?? k })),
  }))
  const fns = store.functions
  if (fns.length > 0 && signatureOf('CallFunction')) {
    groups.push({
      label: 'Functions',
      color: ACCENT.module,
      items: fns.map((f) => ({ id: `fn::${f.name}`, label: f.name })),
    })
  }
  return groups
})

/** Palette shown for the current menu. When a wire is being placed (released on
 *  blank), it is narrowed to nodes the source pin can feed — exec sources list
 *  exec-in nodes, data sources list nodes with a compatible data input — exactly
 *  Unreal's pin-filtered action menu (item 2b). Plain right-click keeps all. */
const paletteMenuGroups = computed<MenuGroup[]>(() => {
  const wire = wireIntent.value
  if (!wire) return paletteGroups.value
  const srcPin = pinOf(wire.source, wire.sourceHandle)
  if (!srcPin) return paletteGroups.value
  const compatible = (kind: string): boolean => {
    const signature = signatureOf(kind)
    if (!signature) return false
    if (srcPin.kind === 'exec-out') return signature.pins.some((p) => p.kind === 'exec-in')
    if (srcPin.kind === 'data-out') return signature.pins.some((p) => p.kind === 'data-in' && isPinCompatible(srcPin.type, p.type))
    return false
  }
  return CATEGORIES.map((cat) => ({
    label: cat.label,
    color: ACCENT[cat.key],
    items: kindList.value
      .filter((k) => categoryOf(k) === cat.key && compatible(k))
      .map((k) => ({ id: k, label: NODE_PRESETS[k]?.title ?? k })),
  })).filter((g) => g.items.length > 0)
})

const nodeMenuGroups = computed<MenuGroup[]>(() => {
  const node = flowNodes.value.find((n) => n.id === selectedId.value)
  // Only control-flow nodes that already expose multiple exec outputs (e.g. a
  // Sequence / Branch) gain or shed extra exec outlets. A single-exit node
  // (CallLLM, Tool, …) has no useful second stream, so no such option (item 15).
  const execOuts = node?.data?.outputs?.filter((p: { kind: string }) => p.kind === 'exec-out') ?? []
  const flowActions = execOuts.length > 1
    ? [
        { id: 'add-exec-out', label: 'Add Exec Output' },
        { id: 'remove-exec-out', label: 'Remove Exec Output' },
      ]
    : []
  return [
    {
      label: 'Node',
      items: [
        { id: 'inspect', label: 'Inspect' },
        { id: 'duplicate', label: 'Duplicate', hint: '⌃D' },
        { id: 'copy', label: 'Copy', hint: '⌃C' },
        { id: 'delete', label: 'Delete', hint: '⌫' },
      ],
    },
    ...(flowActions.length
      ? [
          {
            label: 'Flow',
            items: flowActions,
          },
        ]
      : []),
  ]
})

const edgeMenuGroups: MenuGroup[] = [
  {
    label: 'Edge',
    items: [
      { id: 'delete-edge', label: 'Disconnect', hint: '⌫' },
      { id: 'copy-edge', label: 'Copy ID' },
    ],
  },
]

const selectionMenuGroups: MenuGroup[] = [
  {
    label: 'Selection',
    items: [
      { id: 'sel-copy', label: 'Copy', hint: '⌃C' },
      { id: 'sel-duplicate', label: 'Duplicate', hint: '⌃D' },
      { id: 'sel-delete', label: 'Delete', hint: '⌫' },
      { id: 'sel-clear', label: 'Clear Selection' },
    ],
  },
]

/** Right-click on a connected pin → disconnect this wire. */
const pinMenuGroups: MenuGroup[] = [
  {
    label: 'Pin',
    items: [{ id: 'pin-disconnect', label: 'Disconnect', hint: '⌫' }],
  },
]

function wirePaletteTo(source: string, sourceHandle: string, newId: string) {
  const srcPin = pinOf(source, sourceHandle)
  if (!srcPin) return
  if (srcPin.kind === 'exec-out') {
    // Exec outlets are single-slot: placing a node replaces any prior wire.
    flowEdges.value = flowEdges.value.filter(
      (e) => !(e.source === source && e.sourceHandle === sourceHandle),
    )
    const node = flowNodes.value.find((n) => n.id === newId)
    const execIn = execInOf((node?.data?.inputs as BlueprintPin[] | undefined) ?? [])
    if (!execIn) return
    const eid = uuid()
    flowEdges.value = [...flowEdges.value, { id: eid, source, sourceHandle, target: newId, targetHandle: execIn.id, class: 'metteur-edge--exec', animated: false }]
    dirty.value = true
    return
  }
  if (srcPin.kind === 'data-out') {
    const node = flowNodes.value.find((n) => n.id === newId)
    const target = node?.data?.inputs?.find((p: BlueprintPin) => p.kind === 'data-in' && isPinCompatible(srcPin.type, p.type))
    if (target) {
      const eid = uuid()
      const visual = edgeVisual(sourceHandle, source)
      flowEdges.value = [...flowEdges.value, { source, sourceHandle, target: newId, targetHandle: target.id, id: eid, class: visual.class, animated: visual.animated, style: visual.style }]
      dirty.value = true
    }
  }
}

function onMenuSelect(id: string) {
  if (!menu.value) return
  if (menu.value.kind === 'palette') {
    const pos = screenToFlowCoordinate({ x: menu.value.x, y: menu.value.y })
    const newId = addNodeAt(pos, id)
    const wire = wireIntent.value
    if (wire && newId) wirePaletteTo(wire.source, wire.sourceHandle, newId)
    wireIntent.value = null
    clearPendingWire()
  } else if (menu.value.kind === 'selection') {
    const sel = flowNodes.value.filter((n) => n.selected)
    if (id === 'sel-clear') {
      for (const n of sel) n.selected = false
      boxRect.value = null
    } else if (id === 'sel-delete') {
      commit()
      const ids = new Set(sel.map((n) => n.id))
      flowNodes.value = flowNodes.value.filter((n) => !ids.has(n.id))
      flowEdges.value = flowEdges.value.filter((e) => !ids.has(e.source) && !ids.has(e.target))
      selectedId.value = null
      boxRect.value = null
      dirty.value = true
    } else if (id === 'sel-duplicate' || id === 'sel-copy') {
      commit()
      for (const n of sel) duplicateNode(n.id)
      boxRect.value = null
    }
  } else if (menu.value.kind === 'node') {
    const nid = menu.value.nodeId
    if (!nid) return
    if (id === 'delete') deleteNode(nid)
    else if (id === 'duplicate') duplicateNode(nid)
    else if (id === 'copy') copyNode(nid)
    // 'inspect' needs no extra action: the inspector follows the selection,
    // which is already set on the node the menu was opened for.
    else if (id === 'add-exec-out') addExecOutput(nid)
    else if (id === 'remove-exec-out') removeExecOutput(nid)
  } else if (menu.value.kind === 'edge') {
    const eid = menu.value.edgeId
    if (!eid) return
    commit()
    if (id === 'delete-edge') flowEdges.value = flowEdges.value.filter((e) => e.id !== eid)
    else if (id === 'copy-edge') navigator.clipboard.writeText(eid)
    selectedEdgeId.value = null
    dirty.value = true
  } else if (menu.value.kind === 'pin') {
    const eid = menu.value.edgeId
    if (!eid) return
    commit()
    if (id === 'pin-disconnect') flowEdges.value = flowEdges.value.filter((e) => e.id !== eid)
    selectedEdgeId.value = null
    dirty.value = true
  }
}

/** Dismissing the add-node palette cancels the pending wire (a fresh line
 *  still being placed is simply not created). Other menus just close. */
function onPaletteClose() {
  wireIntent.value = null
  clearPendingWire()
  menu.value = null
}

/** Dismiss any open floating menu. Every cancel path (Escape, canvas press,
 *  pane click) routes through here so a pending wire is never orphaned. */
function dismissMenu() {
  if (menu.value?.kind === 'palette') onPaletteClose()
  else {
    menu.value = null
    clearPendingWire()
  }
}

/* Node CRUD                                                         */

function addNodeAt(pos: { x: number; y: number }, kind: string): string | null {
  const entry = kind.startsWith('fn::')
    ? store.functions.find((f) => `fn::${f.name}` === kind)
    : undefined
  // Node ids must be UUIDs: the daemon model keys nodes/edges/pins by `Uuid`.
  const id = uuid()
  const signature = signatureOf(entry ? 'CallFunction' : kind)
  if (!signature) return null
  commit()
  const node = entry ? makeCallFunctionNode(entry, signature, pos, id) : makeFlowNode(signature, pos, id)
  flowNodes.value = [...flowNodes.value, node]
  selectedId.value = id
  selectedEdgeId.value = null
  dirty.value = true
  return id
}

function deleteNode(id: string) {
  commit()
  flowNodes.value = flowNodes.value.filter((n) => n.id !== id)
  flowEdges.value = flowEdges.value.filter((e) => e.source !== id && e.target !== id)
  if (selectedId.value === id) selectedId.value = null
  dirty.value = true
}

/** Drop a file-reference node (from the explorer's "Add to Blueprint") into the
 *  canvas, pre-filled with the workspace-relative path of the chosen file. */
function addReferenceNode(filePath: string) {
  commit()
  const id = uuid()
  const node = makeFileReference(filePath, { x: 120, y: 120 }, id)
  flowNodes.value = [...flowNodes.value, node]
  selectedId.value = id
  selectedEdgeId.value = null
  dirty.value = true
}

function duplicateNode(id: string, offset = 32) {
  commit()
  const src = flowNodes.value.find((n) => n.id === id)
  if (!src) return
  const copy = cloneNode(src, offset)
  flowNodes.value = [...flowNodes.value, copy]
  selectedId.value = copy.id
  dirty.value = true
}

function cloneNode(src: SelectableFlowNode, offset: number): SelectableFlowNode {
  const id = uuid()
  return {
    ...src,
    id,
    position: { x: src.position.x + offset, y: src.position.y + offset },
    data: {
      ...src.data,
      values: src.data?.values ? { ...src.data.values } : {},
      filledIns: [...(src.data?.filledIns ?? [])],
    },
    selected: true,
  }
}

function copyNode(id: string) {
  const src = flowNodes.value.find((n) => n.id === id)
  clipboard.value = src ? [src] : []
}

/** Rebuild a Switch node's `Case_*` outlets from its edited case list. */
function recaseNode(id: string, cases: string[]) {
  const node = flowNodes.value.find((n) => n.id === id)
  const outputs = node?.data?.outputs
  if (!node || !outputs) return
  commit()
  const wanted = new Set(cases.map((c) => `Case_${c}`))
  const kept = outputs.filter(
    (p: BlueprintPin) => p.kind !== 'exec-out' || p.name === 'Default' || wanted.has(p.name),
  )
  const existing = new Set(kept.map((p) => p.name))
  const added: BlueprintPin[] = cases
    .filter((c) => !existing.has(`Case_${c}`))
    .map((c) => ({ id: uuid(), key: `x-out-Case_${c}`, name: `Case_${c}`, kind: 'exec-out' as const }))
  if (node?.data) {
    node.data.outputs = [...kept, ...added]
    // Drop wires from removed outlets.
    const live = new Set(node.data.outputs.map((p) => p.id))
    flowEdges.value = flowEdges.value.filter(
      (e) => !(e.source === id && e.sourceHandle && !live.has(e.sourceHandle)),
    )
  }
  dirty.value = true
}

/** Append another exec output outlet to a node (Unreal-style add-on outputs). */
function addExecOutput(id: string) {
  const node = flowNodes.value.find((n) => n.id === id)
  const outputs = node?.data?.outputs
  if (!node || !outputs) return
  commit()
  const count = outputs.filter((p: BlueprintPin) => p.kind === 'exec-out').length
  if (node?.data) {
    node.data.outputs = [
      ...outputs,
      { id: uuid(), key: `x-out-extra-${count}`, name: `Exec ${count}`, kind: 'exec-out' as const },
    ]
  }
  dirty.value = true
}

/** Remove the last added exec output outlet (keeps any pre-defined ones). */
function removeExecOutput(id: string) {
  const node = flowNodes.value.find((n) => n.id === id)
  if (!node?.data?.outputs) return
  const execOuts = node.data.outputs
    .map((p: { kind: string }, i: number) => ({ p, i }))
    .filter(({ p }: { p: { kind: string } }) => p.kind === 'exec-out')
  if (execOuts.length <= 1) return
  commit()
  const last = execOuts[execOuts.length - 1]
  const pinId = node.data.outputs[last.i].id
  node.data.outputs = node.data.outputs.filter((_: { kind: string }, i: number) => i !== last.i)
  // Drop any wires feeding the removed outlet.
  flowEdges.value = flowEdges.value.filter(
    (e) => !(e.source === id && e.sourceHandle === pinId),
  )
  dirty.value = true
}

/* Keyboard shortcuts                                                */

function onKeydown(e: KeyboardEvent) {
  const t = e.target as HTMLElement
  if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable)) return

  if (e.key === 'Escape') dismissMenu()

  const mod = e.ctrlKey || e.metaKey

  // Undo / redo (Ctrl+Z, Ctrl+Shift+Z / Ctrl+Y).
  if (mod && e.key.toLowerCase() === 'z') {
    e.preventDefault()
    if (e.shiftKey) redoGraph()
    else undoGraph()
    return
  }
  if (mod && e.key.toLowerCase() === 'y') {
    e.preventDefault()
    redoGraph()
    return
  }

  if (mod && (e.key === 's' || e.key === 'S')) {
    e.preventDefault()
    handleSave()
    return
  }
  if (mod && (e.key === 'a' || e.key === 'A')) {
    e.preventDefault()
    for (const n of flowNodes.value) n.selected = true
    selectedId.value = null
    selectedEdgeId.value = null
    return
  }
  if (mod && (e.key === 'c' || e.key === 'C')) {
    if (selectedId.value) {
      copyNode(selectedId.value)
      e.preventDefault()
    }
    return
  }
  if (mod && (e.key === 'v' || e.key === 'V')) {
    for (const node of clipboard.value) duplicateNode(node.id)
    e.preventDefault()
    return
  }
  if (mod && (e.key === 'd' || e.key === 'D')) {
    if (selectedId.value) {
      duplicateNode(selectedId.value)
      e.preventDefault()
    }
    return
  }

  if (e.key === 'Delete' || e.key === 'Backspace') {
    if (selectedEdgeId.value) {
      commit()
      flowEdges.value = flowEdges.value.filter((edge) => edge.id !== selectedEdgeId.value)
      selectedEdgeId.value = null
      dirty.value = true
      e.preventDefault()
    } else if (selectedId.value) {
      deleteNode(selectedId.value)
      e.preventDefault()
    }
  }
}

/* Selection / save / run                                            */

function onNodeClick({ node }: NodeMouseEvent) {
  selectedId.value = node.id
  selectedEdgeId.value = null
  store.select(node.id)
}

function onEdgeClick({ edge }: EdgeMouseEvent) {
  selectedId.value = null
  selectedEdgeId.value = edge.id
}

/** Snapshot the graph as dragging begins so a node move is undoable: the live
 *  v-model already tracks the new position by the time drag-stop fires, so
 *  committing there would capture the *moved* state instead of the pre-move one. */
function onNodeDragStart() {
  commit()
}

function onNodeDragStop({ node }: NodeDragEvent) {
  store.setNodePosition(node.id, node.position)
  dirty.value = true
}

function onPaneClick() {
  selectedId.value = null
  selectedEdgeId.value = null
  store.select(null)
  // A connect-drag released on blank opens the palette (via connect-end); Vue
  // Flow then fires a pane-click for the same release, which must not tear the
  // freshly opened menu back down. A plain pane click still dismisses.
  if (menu.value?.kind === 'palette' && menu.value.fromConnect) return
  dismissMenu()
  wireIntent.value = null
}

/** Wrapper-level mousedown, run in the capture phase so it fires even though
 *  Vue Flow's own pane handlers stop propagation on their child nodes.
 *  - Any non-right press closes the floating menu and clears a stale box
 *    (item 4).
 *  - A right press on the empty canvas starts a box-select marquee (item 3).
 *    Vue Flow does not emit `pane-mouse-down`, so this is the only reliable
 *    way to catch the canvas press. Right-clicks on nodes / handles / edges are
 *    owned by their own menus and are skipped here (their handlers run later in
 *    the bubble phase, so not preventDefault-ing keeps their menus working). */
function onCanvasMouseDown(e: MouseEvent) {
  if (e.button !== 2) {
    if (menu.value) dismissMenu()
    boxRect.value = null
    return
  }
  const el = e.target as HTMLElement
  if (el.closest('.vue-flow__node, .vue-flow__handle, .vue-flow__edge-component, .vue-flow__edge')) return
  startMarquee(e)
}

/** Paint a marquee while dragging with the right button and box-select the
 *  nodes it covers on release. Selection compares in flow space; the overlay is
 *  positioned in wrapper space, so the origin comes from the flow wrapper. */
function startMarquee(e: MouseEvent) {
  const rect = flowWrap.value?.getBoundingClientRect()
  const offX = rect?.left ?? 0
  const offY = rect?.top ?? 0
  const startClient = { x: e.clientX, y: e.clientY }
  marquee.value = {
    x1: startClient.x - offX,
    y1: startClient.y - offY,
    x2: startClient.x - offX,
    y2: startClient.y - offY,
  }
  let moved = false
  const move = (ev: MouseEvent) => {
    if (marquee.value) {
      marquee.value.x2 = ev.clientX - offX
      marquee.value.y2 = ev.clientY - offY
      if (Math.abs(ev.clientX - startClient.x) > 4 || Math.abs(ev.clientY - startClient.y) > 4) moved = true
    }
  }
  const up = () => {
    const m = marquee.value
    marquee.value = null
    window.removeEventListener('mousemove', move)
    window.removeEventListener('mouseup', up)
    if (m && moved) {
      applyMarquee({ x1: startClient.x, y1: startClient.y, x2: m.x2 + offX, y2: m.y2 + offY })
      // Swallow the contextmenu that fires at the end of the drag so it doesn't
      // pop a menu right after a box-select.
      suppressMenuUntil = Date.now() + 250
    }
  }
  window.addEventListener('mousemove', move)
  window.addEventListener('mouseup', up, { once: true })
}

/** Convert a client-space rect to flow space, select the contained nodes, and
 *  remember the region (client space) for later box-select menu decisions.
 *  `screenToFlowCoordinate` expects window coordinates and subtracts the flow
 *  element's own origin, so raw client points are passed through unchanged.
 *  A node is selected when its box overlaps the marquee at all, not only when
 *  its origin point falls inside. */
function applyMarquee(m: { x1: number; y1: number; x2: number; y2: number }) {
  const a = screenToFlowCoordinate({ x: m.x1, y: m.y1 })
  const b = screenToFlowCoordinate({ x: m.x2, y: m.y2 })
  const box = {
    minX: Math.min(a.x, b.x),
    minY: Math.min(a.y, b.y),
    maxX: Math.max(a.x, b.x),
    maxY: Math.max(a.y, b.y),
  }
  /** Node size as measured by vue-flow, falling back to an estimate. */
  const nodeRect = (id: string): { width: number; height: number } => {
    const n = getNodes.value.find((x) => x.id === id)
    if (n?.dimensions?.width && n?.dimensions?.height) {
      return { width: n.dimensions.width, height: n.dimensions.height }
    }
    const node = flowNodes.value.find((x) => x.id === id)
    const rows = Math.max(node?.data?.inputs?.length ?? 0, node?.data?.outputs?.length ?? 0)
    return { width: 208, height: 24 + 12 + rows * 22 }
  }
  for (const n of flowNodes.value) {
    const { width, height } = nodeRect(n.id)
    n.selected = !(
      box.maxX < n.position.x ||
      box.minX > n.position.x + width ||
      box.maxY < n.position.y ||
      box.minY > n.position.y + height
    )
  }
  selectedId.value = null
  selectedEdgeId.value = null
  boxRect.value = {
    minX: Math.min(m.x1, m.x2),
    minY: Math.min(m.y1, m.y2),
    maxX: Math.max(m.x1, m.x2),
    maxY: Math.max(m.y1, m.y2),
  }
}

function insideBox(x: number, y: number): boolean {
  const b = boxRect.value
  return !!b && x >= b.minX && x <= b.maxX && y >= b.minY && y <= b.maxY
}

/** Close the box-select menu and drop the remembered selection region. */
function closeGraphMenu() {
  menu.value = null
  boxRect.value = null
}

function onPaneContextMenu(ev: MouseEvent) {
  ev.preventDefault()
  // Mute the contextmenu that immediately follows a right-drag box-select.
  if (Date.now() < suppressMenuUntil) return
  wireIntent.value = null
  clearPendingWire()
  // Right-click inside the recently box-selected region → select menu.
  if (insideBox(ev.clientX, ev.clientY)) {
    menu.value = { kind: 'selection', x: ev.clientX, y: ev.clientY }
    return
  }
  menu.value = { kind: 'palette', x: ev.clientX, y: ev.clientY }
}

async function handleSave(): Promise<boolean> {
  const ws = workspace.active
  const file = fileKey.value
  if (!ws || store.saving[file]) return false
  store.nodes = flowNodes.value.map(toBpNode)
  store.edges = flowEdges.value.map((e) => ({
    id: e.id,
    source: e.source,
    sourceHandle: e.sourceHandle ?? undefined,
    target: e.target,
    targetHandle: e.targetHandle ?? undefined,
    label: typeof e.label === 'string' ? e.label : undefined,
  }))
  const ok = await store.save(ws.path, file, entryNodeIdOf())
  if (workspace.active?.path === ws.path && fileKey.value === file) {
    // Recompute against the fresh baseline instead of forcing false, so the
    // dot and the baseline can never disagree.
    dirty.value = graphKey() !== savedBaseline.value
    if (props.filePath) tabs.markDirty(props.filePath, dirty.value)
  }
  if (!ok) feedback.toast('error', 'Blueprint not synchronized', store.saveErrors[file])
  return ok
}

async function handleRun() {
  const ws = workspace.active
  if (!ws) return
  if (execution.running) { openExecutionSurface('execution'); return }
  const file = fileKey.value
  const key = graphKey()
  // A failed file write or mirror must stop Run, with the draft left intact.
  if (!await handleSave()) return
  if (workspace.active?.path !== ws.path || fileKey.value !== file) return
  if (graphKey() !== key) {
    store.saveErrors[file] = 'The graph changed while saving. Save again before running.'
    return
  }
  const id = store.uuidFor(fileKey.value) ?? uuid()
  const blueprint: Blueprint = {
    id,
    // The name matches what `save` mirrors, so both agree on the identity.
    name: fileNameOf(fileKey.value),
    entryNodeId: entryNodeIdOf(),
    nodes: store.nodes,
    edges: store.edges,
  }
  openExecutionSurface('execution')
  try {
    await execution.run(id, blueprint, file)
  } catch (err) {
    feedback.toast('error', 'Run failed', String(err))
  }
}

/** DSL export/import pickers (workspace file dialogs). */
const dslSaveOpen = ref(false)
const dslPickOpen = ref(false)

/** Pick the graph entry for export: a `Start` node when present, else the
 *  first node without an incoming exec wire (the exec-flow root). Falling back
 *  to `flowNodes[0]` can wrongly mark `End` as entry when the list order
 *  drifts, which the DSL compiler would then treat as the start of the graph. */
function entryNodeIdOf(): string {
  const hasIncomingExec = new Set<string>()
  for (const e of flowEdges.value) {
    const srcPin = pinOf(e.source, e.sourceHandle ?? '')
    if (srcPin?.kind === 'exec-out') hasIncomingExec.add(e.target)
  }
  return (
    flowNodes.value.find((n) => n.data?.title === 'Start')?.id ??
    flowNodes.value.find((n) => !hasIncomingExec.has(n.id))?.id ??
    flowNodes.value[0]?.id ??
    ''
  )
}

/** Render the current canvas to DSL text by sending it to the daemon inline
 *  (no dependency on the archived copy, which may be stale or from an older
 *  storage format). */
async function dslExportText(): Promise<string | null> {
  const ws = workspace.active
  if (!ws) return null
  if (flowNodes.value.length === 0) {
    feedback.toast('error', 'DSL export failed', 'The canvas is empty')
    return null
  }
  const bp: Blueprint = {
    id: store.uuidFor(fileKey.value) ?? uuid(),
    name: fileKey.value.split(/[\\/]/).pop() ?? 'blueprint',
    entryNodeId: entryNodeIdOf(),
    nodes: flowNodes.value.map(toBpNode),
    edges: flowEdges.value.map((e) => ({
      id: e.id,
      source: e.source,
      sourceHandle: e.sourceHandle ?? undefined,
      target: e.target,
      targetHandle: e.targetHandle ?? undefined,
      label: typeof e.label === 'string' ? e.label : undefined,
    })),
  }
  const r = await gateway.decompileBlueprint(ws.path, bp)
  if (!r.ok) {
    feedback.toast('error', 'DSL export failed', r.error)
    return null
  }
  return r.data
}

/** Rebuild the canvas from a `.mbp` file picked in the workspace. */
async function importDslFrom(filePath: string) {
  const ws = workspace.active
  if (!ws) return
  const file = await gateway.readFile(ws.path, filePath)
  if (!file.ok) {
    feedback.toast('error', 'DSL read failed', file.error)
    return
  }
  const compiled = await gateway.compileDsl(file.data.content, ws.path)
  if (!compiled.ok) {
    feedback.toast('error', 'DSL compile failed', compiled.error)
    return
  }
  commit()
  flowNodes.value = compiled.data.nodes.map((n) => toFlowNode(n) as unknown as SelectableFlowNode)
  flowEdges.value = compiled.data.edges.map(toFlowEdge)
  selectedId.value = null
  selectedEdgeId.value = null
  dirty.value = true
}

/** Save-mode confirm: write the exported DSL into the picked directory. */
async function onDslExportConfirm(p: FilePick) {
  dslSaveOpen.value = false
  const ws = workspace.active
  if (!ws || !p.name) return
  const dsl = await dslExportText()
  if (dsl === null) return
  const base = p.name.toLowerCase().endsWith('.mbp') ? p.name : `${p.name}.mbp`
  const rel = p.dir ? `${p.dir}/${base}` : base
  const r = await gateway.writeFile(ws.path, rel, dsl)
  if (r.ok) feedback.toast('success', 'DSL exported', rel)
  else feedback.toast('error', 'DSL export failed', r.error)
}

/** Open-mode confirm: import the picked `.mbp` file. */
async function onDslImportConfirm(p: FilePick) {
  if (!p.filePath) return
  dslPickOpen.value = false
  await importDslFrom(p.filePath)
}

/** Open the per-file version history in the closable right-hand panel. */
function openVersionPanel() {
  if (props.filePath) rightPanel.show(FileVersionPanel, 'Version History', { filePath: props.filePath })
}
</script>

<template>
  <div class="flex h-full flex-col">
    <p v-if="store.saveErrors[fileKey]" role="alert" data-testid="blueprint-save-error" class="border-b border-divider px-3 py-2 text-xs text-danger">{{ store.saveErrors[fileKey] }}</p>
    <p v-if="store.catalogMessage" data-testid="node-catalog-status" class="border-b border-divider px-3 py-2 text-xs text-muted-foreground">{{ store.catalogMessage }}</p>
    <!-- Editor toolbar -->
    <div class="flex h-11 shrink-0 items-center gap-2 border-b border-divider px-3">
      <Workflow class="h-4 w-4 text-muted-foreground" />
      <span class="text-[13px] font-medium">{{ blueprintTitle }}</span>
      <span
        v-if="dirty"
        class="h-1.5 w-1.5 rounded-full"
        style="background: var(--primary)"
      />
      <span v-else class="h-1.5 w-1.5 rounded-full bg-subtle" />

      <!-- Run state: chip + controls run the graph in place (Unreal debug). -->
      <span
        v-if="execution.status !== 'idle'"
        class="chip ml-1"
        :class="statusChip"
      >
        <span
          v-if="execution.status === 'running'"
          class="h-1.5 w-1.5 animate-pulse rounded-full bg-primary"
        />
        {{ execution.status }}
      </span>
      <button
        v-if="execution.status === 'running'"
        class="btn btn-outline ml-1"
        type="button"
        @click="execution.pause()"
      >
        <Pause class="h-3.5 w-3.5" /> Pause
      </button>
      <button
        v-if="execution.status === 'paused'"
        class="btn btn-outline ml-1"
        type="button"
        @click="execution.resume()"
      >
        <Play class="h-3.5 w-3.5" /> Resume
      </button>
      <button
        v-if="execution.running"
        class="btn btn-danger-outline ml-1"
        type="button"
        @click="execution.cancel()"
      >
        <Square class="h-3.5 w-3.5" /> Cancel
      </button>

      <div class="pr-1"><!-- spacer keeps icons from hugging the edge --></div>
      <div class="ml-auto flex items-center gap-3">
        <button
          class="editor-tool-icon"
          type="button"
          title="Undo (Ctrl+Z)"
          aria-label="Undo"
          :disabled="!past.length"
          @click="undoGraph"
        >
          <Undo2 class="h-4 w-4" />
        </button>
        <button
          class="editor-tool-icon"
          type="button"
          title="Redo (Ctrl+Shift+Z)"
          aria-label="Redo"
          :disabled="!future.length"
          @click="redoGraph"
        >
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
        <button class="editor-tool-icon" type="button" title="Save (Ctrl+S)" aria-label="Save" :disabled="store.saving[fileKey]" @click="handleSave">
          <Save class="h-4 w-4" />
        </button>
        <button
          class="editor-tool-icon"
          type="button"
          title="Export blueprint as DSL file"
          aria-label="Export DSL"
          @click="dslSaveOpen = true"
        >
          <FileDown class="h-4 w-4" />
        </button>
        <button
          class="editor-tool-icon"
          type="button"
          title="Import blueprint from .mbp file"
          aria-label="Import DSL"
          @click="dslPickOpen = true"
        >
          <FileUp class="h-4 w-4" />
        </button>
        <button
          class="editor-tool-icon"
          type="button"
          title="Audit drawer"
          aria-label="Audit"
          :class="auditOpen ? 'bg-hover text-foreground' : ''"
          @click="auditOpen = !auditOpen"
        >
          <Activity class="h-4 w-4" />
        </button>
        <button
          class="inline-flex h-5 w-5 shrink-0 cursor-pointer items-center justify-center rounded-md p-0 text-primary-foreground transition-colors duration-150 hover:brightness-105"
          type="button"
          title="Run (Ctrl+Enter)"
          aria-label="Run"
          :disabled="store.saving[fileKey]"
          style="background: var(--primary)"
          @click="handleRun"
        >
          <Play class="h-4 w-4" />
        </button>
      </div>
    </div>

    <!-- Canvas -->
    <div ref="flowWrap" class="relative min-w-0 flex-1">
      <VueFlow
        ref="flowRef"
        v-model:nodes="flowNodes"
        v-model:edges="canvasEdges"
        class="blueprint-flow"
        :node-types="{ blueprint: BlueprintNodeComp as unknown as NodeComponent }"
        :snap-to-grid="true"
        :snap-grid="[12, 12]"
        :default-viewport="{ zoom: 0.8 }"
        :min-zoom="0.25"
        :max-zoom="2"
        :pan-on-drag="[0, 1]"
        :pan-on-scroll="true"
        :connection-mode="ConnectionMode.Strict"
        :is-valid-connection="isValidConnection"
        @connect="onConnect"
        @connect-start="onConnectStart"
        @connect-end="onConnectEnd"
        @node-click="onNodeClick"
        @node-context-menu="onNodeContextMenu"
        @node-drag-start="onNodeDragStart"
        @node-drag-stop="onNodeDragStop"
        @edge-click="onEdgeClick"
        @edge-context-menu="onEdgeContextMenu"
        @pane-click="onPaneClick"
        @pane-context-menu="onPaneContextMenu"
      >
        <!-- Gap/dot scale with the inverse zoom so dots keep a fixed on-screen
             size & spacing; colour from --grid-dot (readable in both themes). -->
        <Background
          :gap="Math.max(20 / viewport.zoom, 8)"
          :dot-size="Math.max(1.5 / viewport.zoom, 0.6)"
          pattern-color="var(--grid-dot)"
        />
        <Controls position="bottom-left" :show-interactive="false" />
      </VueFlow>
      <div
        v-if="marquee"
        class="pointer-events-none absolute z-10 rounded-sm"
        :style="{
          left: Math.min(marquee.x1, marquee.x2) + 'px',
          top: Math.min(marquee.y1, marquee.y2) + 'px',
          width: Math.abs(marquee.x2 - marquee.x1) + 'px',
          height: Math.abs(marquee.y2 - marquee.y1) + 'px',
          border: '1.5px dashed rgba(59, 130, 246, 0.85)',
          background: 'rgba(59, 130, 246, 0.12)',
        }"
      />
      <!-- Fresh wire suspended while the add-node palette is open: it stays
           visible from the source pin to the palette anchor until the menu
           closes (Unreal's release-on-blank → palette → auto-connect). -->
      <svg
        v-if="pendingWire"
        class="pointer-events-none absolute inset-0 z-10 h-full w-full overflow-visible"
        aria-hidden="true"
      >
        <path
          :d="pendingWirePath"
          fill="none"
          :stroke="pendingWire.stroke"
          :stroke-width="pendingWire.width"
          stroke-linecap="round"
        />
      </svg>

      <!-- Docked execution audit (Unreal-style debug trail). -->
      <BlueprintAuditDrawer v-if="auditOpen" v-model:open="auditOpen" :node-id="selectedId" />

      <!-- Selected-node inspector (pin defaults / types). -->
      <NodeInspector
        v-if="selectedNodeData && !auditOpen"
        :node="selectedNodeData"
        @close="selectedId = null"
        @change="dirty = true"
        @recase="(cases: string[]) => selectedId && recaseNode(selectedId, cases)"
      />
    </div>

    <!-- DSL file dialogs (workspace file picker). -->
    <FilePickerDialog
      v-if="workspace.active && dslSaveOpen"
      :open="dslSaveOpen"
      mode="save"
      title="Export blueprint as DSL"
      :workspace-path="workspace.active.path"
      :extensions="['.mbp']"
      @close="dslSaveOpen = false"
      @confirm="onDslExportConfirm"
    />
    <FilePickerDialog
      v-if="workspace.active && dslPickOpen"
      :open="dslPickOpen"
      mode="open"
      title="Import blueprint from DSL"
      :workspace-path="workspace.active.path"
      :extensions="['.mbp']"
      @close="dslPickOpen = false"
      @confirm="onDslImportConfirm"
    />

    <!-- Floating context menus -->
    <ContextMenu
      v-if="menu?.kind === 'palette'"
      :x="menu.x"
      :y="menu.y"
      :groups="paletteMenuGroups"
      :default-collapsed="PALETTE_COLLAPSED"
      searchable
      @select="onMenuSelect"
      @close="onPaletteClose"
    />
    <ContextMenu
      v-if="menu?.kind === 'selection'"
      :x="menu.x"
      :y="menu.y"
      :groups="selectionMenuGroups"
      @select="onMenuSelect"
      @close="closeGraphMenu"
    />
    <ContextMenu
      v-if="menu?.kind === 'node'"
      :x="menu.x"
      :y="menu.y"
      :groups="nodeMenuGroups"
      @select="onMenuSelect"
      @close="menu = null"
    />
    <ContextMenu
      v-if="menu?.kind === 'edge'"
      :x="menu.x"
      :y="menu.y"
      :groups="edgeMenuGroups"
      @select="onMenuSelect"
      @close="menu = null"
    />
    <ContextMenu
      v-if="menu?.kind === 'pin'"
      :x="menu.x"
      :y="menu.y"
      :groups="pinMenuGroups"
      @select="onMenuSelect"
      @close="menu = null"
    />
  </div>
</template>
