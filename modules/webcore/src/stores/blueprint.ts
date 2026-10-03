import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { gateway } from '@/core'
import { pinsFor } from '@/lib/blueprint'
import { inlineValue, valueText, nodeConfiguration } from '@/core/node-catalog'
import type { Blueprint, BlueprintEdge, BlueprintNode, BlueprintPin, FunctionItem, NodeKindInfo } from '@/core'

/**
 * Blueprint graph state shared across the editor.
 *
 * Each blueprint *file* keeps its own graph, keyed by the file path, so
 * switching tabs loads and preserves a different canvas. The file is the
 * source of truth: load reads it, save writes it back and mirrors the graph
 * into the daemon's blueprint store (keyed by the file's stable UUID).
 */

interface GraphData {
  id: string
  nodes: BlueprintNode[]
  edges: BlueprintEdge[]
}

/** Edge fields participating in the dirty key. */
type KeyEdge = Pick<BlueprintEdge, 'id' | 'source' | 'target' | 'sourceHandle' | 'targetHandle' | 'label'>

/** Canonical dirty key for a graph, shared with the editor (BlueprintView).
 *  Pin connected-state (`filledIns`/`connectedIn`/`connectedOut`) is derived
 *  from wires and excluded so wires never phantom-dirty a pristine file. */
function keyFor(nodes: BlueprintNode[], edges: KeyEdge[]): string {
  return JSON.stringify({
    nodes: nodes.map((n) => ({
      id: n.id,
      type: n.type,
      nodeType: n.nodeType,
      data: n.data,
      position: n.position,
      title: n.title,
      category: n.category,
      inputs: n.inputs,
      outputs: n.outputs,
      values: n.values ?? {},
    })),
    edges: edges.map((e) => ({
      id: e.id,
      source: e.source,
      sourceHandle: e.sourceHandle,
      target: e.target,
      targetHandle: e.targetHandle,
      label: e.label,
    })),
  })
}

/** A fresh Start → End graph used to seed a brand-new blueprint file.
 *  Pins come from the current daemon catalogue. */
function seedGraph(signatures: NodeKindInfo[]): GraphData {
  const startSignature = signatures.find((n) => n.kind === 'Start')
  const endSignature = signatures.find((n) => n.kind === 'End')
  if (!startSignature || !endSignature) return { id: crypto.randomUUID(), nodes: [], edges: [] }
  const nodeId = () => crypto.randomUUID()
  const start = nodeId()
  const end = nodeId()
  const startPins = pinsFor(startSignature)
  const endPins = pinsFor(endSignature)
  return {
    id: crypto.randomUUID(),
    nodes: [
      {
        id: start,
        type: 'Start',
        nodeType: startSignature.nodeType,
        data: {},
        category: 'event',
        title: 'Start',
        // Match the canvas 12px grid so initialization does not dirty a fresh graph.
        position: { x: 36, y: 216 },
        inputs: startPins.inputs,
        outputs: startPins.outputs,
      },
      {
        id: end,
        type: 'End',
        nodeType: endSignature.nodeType,
        data: {},
        category: 'event',
        title: 'End',
        position: { x: 324, y: 216 },
        inputs: endPins.inputs,
        outputs: endPins.outputs,
      },
    ],
    edges: [
      {
        id: crypto.randomUUID(),
        source: start,
        // The first output pin of Start is its exec outlet, driving the wire.
        sourceHandle: startPins.outputs[0]?.id,
        target: end,
        targetHandle: endPins.inputs[0]?.id,
      },
    ],
  }
}

/** The on-disk name of a blueprint derived from its file path. */
export function fileNameOf(filePath: string): string {
  return filePath.split(/[\\/]/).pop()?.replace(/\.blueprint$/i, '') || 'blueprint'
}

const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i

/** Pin ids used by older canvases, mapped to their current canonical names.
 *  Presets renamed pins over time (`out` → `Result`, `a` → `A`, …); graphs
 *  saved with the old keys still need name alignment to round-trip through the
 *  DSL, whose parser also cannot express whitespace in pin names. */
const LEGACY_PIN_NAMES: Record<string, string> = {
  a: 'A',
  b: 'B',
  in: 'In',
  out: 'Result',
  input: 'Input',
  find: 'Find',
  replaceWith: 'ReplaceWith',
  start: 'Start',
  length: 'Length',
  list: 'List',
  item: 'Item',
  index: 'Index',
  object: 'Object',
  path: 'Path',
  value: 'Value',
  itemA: 'ItemA',
  itemB: 'ItemB',
  itemC: 'ItemC',
  system: 'System',
  prompt: 'Prompt',
  context: 'Context',
  keep: 'Keep',
  ms: 'Ms',
  message: 'Message',
  allowed: 'Allowed',
  tool_name: 'ToolName',
  command: 'Command',
  result: 'Result',
  cond: 'Condition',
  case: 'Case',
  iteration: 'Iteration',
  name: 'Name',
  task: 'Task',
  description: 'Description',
  alias: 'Alias',
  max_iterations: 'MaxIterations',
}

/** Renamed node kinds: older canvases used a single `Arithmetic` entry where
 *  the daemon knows the real `Add` kind. */
const KIND_ALIASES: Record<string, string> = { Arithmetic: 'Add' }

/**
 * Aligns existing pins with the daemon contract while preserving their IDs (older
 * canvases stored lowercase/`out` names that no longer match the daemon's
 * executor lookups or the DSL templates). Whitespace is stripped so exported
 * data wires never break the DSL parser.
 */
function canonicalizePinNames(n: BlueprintNode, signatures: NodeKindInfo[]): BlueprintNode {
  const kind = KIND_ALIASES[n.type] ?? n.type
  const signature = signatures.find((s) => s.kind === kind)
  const values = { ...n.values }
  for (const p of n.inputs ?? []) {
    const raw = inlineValue(n.data ?? {}, p)
    if (p.kind === 'data-in' && values[p.id] === undefined && raw !== undefined) values[p.id] = valueText(raw)
  }
  const byName = (p: BlueprintPin): BlueprintPin => {
    const contract = signature?.pins.find((d) => d.kind === p.kind && ((p.key && d.key === p.key) || d.name === p.name))
    const label = contract?.name || (!signature && p.key ? LEGACY_PIN_NAMES[p.key] : undefined)
    const name = (label ?? p.name ?? '').replace(/\s+/g, '')
    return contract ? { ...contract, id: p.id, name, default: p.default === undefined ? contract.default : p.default } : name !== p.name ? { ...p, name } : p
  }
  return { ...n, data: nodeConfiguration(n.data ?? {}, n.inputs ?? []), values, nodeType: signature?.nodeType ?? n.nodeType, type: n.type === kind ? n.type : kind, inputs: (n.inputs ?? []).map(byName), outputs: (n.outputs ?? []).map(byName) }
}

/**
 * Rewrites ids that are not UUIDs (older canvases used `n-…` node ids) so the
 * graph satisfies the daemon model, which keys nodes/edges/pins by `Uuid`, and
 * aligns pins with the current daemon signature. Idempotent; works for cached
 * graphs too, since name canonicalization never depends on id migration.
 */
function migrateIds(g: GraphData, signatures: NodeKindInfo[]): GraphData {
  const nodeIds = new Map<string, string>()
  const pinIds = new Map<string, string>()
  for (const n of g.nodes) {
    if (!UUID_RE.test(n.id)) nodeIds.set(n.id, crypto.randomUUID())
    for (const p of [...(n.inputs ?? []), ...(n.outputs ?? [])]) {
      if (!UUID_RE.test(p.id)) pinIds.set(p.id, crypto.randomUUID())
    }
  }
  return {
    id: UUID_RE.test(g.id) ? g.id : crypto.randomUUID(),
    nodes: g.nodes.map((n) => {
      const canonical = canonicalizePinNames(n, signatures)
      return {
        ...canonical,
        id: nodeIds.get(n.id) ?? n.id,
        values: Object.fromEntries(Object.entries(canonical.values ?? {}).map(([id, value]) => [pinIds.get(id) ?? id, value])),
        inputs: canonical.inputs.map((p) => ({ ...p, id: pinIds.get(p.id) ?? p.id })),
        outputs: canonical.outputs.map((p) => ({ ...p, id: pinIds.get(p.id) ?? p.id })),
      }
    }),
    edges: g.edges.map((e) => ({
      ...e,
      id: UUID_RE.test(e.id) ? e.id : crypto.randomUUID(),
      source: nodeIds.get(e.source) ?? e.source,
      target: nodeIds.get(e.target) ?? e.target,
      sourceHandle: e.sourceHandle ? (pinIds.get(e.sourceHandle) ?? e.sourceHandle) : e.sourceHandle,
      targetHandle: e.targetHandle ? (pinIds.get(e.targetHandle) ?? e.targetHandle) : e.targetHandle,
    })),
  }
}

export const useBlueprintStore = defineStore('blueprint', () => {
  const loaded = ref(false)
  const saved = ref(false)
  const nodes = ref<BlueprintNode[]>([])
  const edges = ref<BlueprintEdge[]>([])
  const nodeKinds = ref<string[]>([])
  const signatures = ref<NodeKindInfo[]>([])
  const catalogMessage = ref('Loading node signatures…')
  /** Registered blueprint functions surfaced by the daemon library. */
  const functions = ref<FunctionItem[]>([])
  /** Unsaved graph per blueprint file path, so different files stay independent. */
  const graphs = ref<Record<string, GraphData>>({})
  /** Serialized graph as last written to disk per file path. The editor
   *  compares its live graph against this to decide the unsaved marker, so a
   *  cached-but-edited file stays dirty when you switch tabs and come back. */
  const savedKeys = ref<Record<string, string>>({})
  const saveErrors = ref<Record<string, string>>({})
  const saving = ref<Record<string, boolean>>({})
  /** Stable blueprint UUID per file path, persisted into the file itself. */
  const ids = ref<Record<string, string>>({})
  const currentFile = ref('')

  /** A file-reference the explorer asked to drop into the current blueprint.
   *  The mounted editor consumes it via {@link consumeReference}; the salt makes
   *  repeated requests for the same file each trigger the editor's watcher. */
  const pendingRef = ref<{ path: string; token: number } | null>(null)
  let refSalt = 0

  function requestAddReference(path: string) {
    pendingRef.value = { path, token: ++refSalt }
  }

  /** Take the pending reference (clearing it) if the editor is mounted. */
  function consumeReference() {
    const p = pendingRef.value
    pendingRef.value = null
    return p
  }

  const byId = computed(() => new Map(nodes.value.map((n) => [n.id, n])))

  async function listKinds() {
    const r = await gateway.listNodeKinds()
    nodeKinds.value = r.ok ? r.data.kinds : []
    signatures.value = r.ok && r.data.ready ? r.data.nodes : []
    catalogMessage.value = r.ok && r.data.ready ? '' : r.ok
      ? 'This daemon does not provide supported pin signatures. Existing graphs remain editable; node creation requires an updated daemon.'
      : `Node signatures unavailable: ${r.error}`
  }

  /** Load the function library (`path` empty = builtin + global). */
  async function loadFunctions(path = '') {
    const r = await gateway.listFunctions(path)
    if (r.ok) functions.value = r.data
  }

  /** The blueprint UUID of a file path, if it has been loaded. */
  function uuidFor(filePath: string): string | undefined {
    return ids.value[filePath]
  }

  async function load(path: string, filePath: string) {
    currentFile.value = filePath
    await listKinds()
    const cached = graphs.value[filePath]
    if (cached) {
      const graph = migrateIds(cached, signatures.value)
      graphs.value[filePath] = graph
      nodes.value = graph.nodes
      edges.value = graph.edges
      ids.value[filePath] = graph.id
    } else {
      const file = await gateway.readFile(path, filePath)
      let graph = seedGraph(signatures.value)
      if (file.ok && file.data.content.trim()) {
        try {
          const parsed = JSON.parse(file.data.content) as Blueprint
          if (Array.isArray(parsed.nodes) && Array.isArray(parsed.edges)) {
            graph = { id: parsed.id || crypto.randomUUID(), nodes: parsed.nodes, edges: parsed.edges }
          }
        } catch {
          // Corrupt files fall back to a fresh graph.
        }
      }
      graph = migrateIds(graph, signatures.value)
      graphs.value[filePath] = graph
      ids.value[filePath] = graph.id
      // The freshly loaded graph is, by definition, the on-disk baseline.
      savedKeys.value[filePath] = keyFor(graph.nodes, graph.edges)
      nodes.value = graph.nodes
      edges.value = graph.edges
    }
    loaded.value = true
    saved.value = !saveErrors.value[filePath] && keyFor(nodes.value, edges.value) === savedKeys.value[filePath]
  }

  /** Persist the currently open graph (the one keyed by `filePath`): write the
   *  file and mirror it into the daemon blueprint store for execution. The
   *  graph comes from the live `nodes`/`edges` refs, which the editor assigns
   *  before saving, so nothing falls back to a stale cached snapshot.
   *
   *  The on-disk file is the source of truth for the dirty baseline: it is
   *  updated as soon as the file write succeeds, even if the daemon mirror
   *  fails, so a successful save never leaves a phantom unsaved dot. */
  async function save(path: string, filePath: string, entryNodeId?: string): Promise<boolean> {
    if (saving.value[filePath]) return false
    const id = ids.value[filePath]
    if (!id) {
      saveErrors.value[filePath] = 'Blueprint is not loaded. Save and run are unavailable.'
      return false
    }
    const graph: GraphData = {
      id,
      nodes: nodes.value,
      edges: edges.value,
    }
    graphs.value[filePath] = graph
    // Both writes use one immutable snapshot; edits made while awaiting I/O
    // must neither change the mirror nor become the saved dirty baseline.
    const blueprint: Blueprint = JSON.parse(JSON.stringify({
      id,
      name: fileNameOf(filePath),
      entryNodeId: entryNodeId ?? nodes.value.find((n) => n.type === 'Start')?.id ?? nodes.value[0]?.id,
      nodes: nodes.value,
      edges: edges.value,
    }))
    const snapshotKey = keyFor(blueprint.nodes, blueprint.edges)
    saving.value[filePath] = true
    saved.value = false
    let fileWritten = false
    try {
      const file = await gateway.writeFile(path, filePath, JSON.stringify(blueprint, null, 2))
      if (!file.ok) throw new Error(file.error)
      fileWritten = true
      savedKeys.value[filePath] = snapshotKey
      const mirror = await gateway.saveBlueprint(path, blueprint)
      if (!mirror.ok) throw new Error(mirror.error)
      delete saveErrors.value[filePath]
      saved.value = currentFile.value === filePath && keyFor(nodes.value, edges.value) === snapshotKey
      return true
    } catch (error) {
      saveErrors.value[filePath] = `${fileWritten ? 'File saved, but daemon mirror is not synchronized' : 'File save failed'}: ${error instanceof Error ? error.message : String(error)}. Run is blocked; retry saving.`
      return false
    } finally {
      saving.value[filePath] = false
    }
  }

  function select(id: string | null) {
    for (const node of nodes.value) node.selected = node.id === id
  }

  function setNodePosition(id: string, position: { x: number; y: number }) {
    const node = byId.value.get(id)
    if (node) node.position = position
  }

  return {
    loaded,
    saved,
    nodes,
    edges,
    nodeKinds,
    signatures,
    catalogMessage,
    functions,
    graphs,
    savedKeys,
    saveErrors,
    saving,
    ids,
    byId,
    currentFile,
    pendingRef,
    requestAddReference,
    consumeReference,
    uuidFor,
    listKinds,
    loadFunctions,
    load,
    save,
    select,
    setNodePosition,
    keyFor,
  }
})
