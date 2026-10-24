import { defineStore } from 'pinia'
import { computed, reactive, ref } from 'vue'
import { useDocumentsStore, type EditorDocument } from './documents'
import { gateway } from '@/core'
import { nativeBlueprint, type NativeGraph } from '@/core/execution-view'
import { pinsFor } from '@/lib/blueprint'
import { inlineValue, valueText, nodeConfiguration } from '@/core/node-catalog'
import type {
  Blueprint,
  BlueprintEdge,
  BlueprintNode,
  BlueprintPin,
  FunctionItem,
  NodeKindInfo,
} from '@/core'

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
type KeyEdge = Pick<
  BlueprintEdge,
  'id' | 'source' | 'target' | 'sourceHandle' | 'targetHandle' | 'label'
>

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
  return (
    filePath
      .split(/[\\/]/)
      .pop()
      ?.replace(/\.blueprint$/i, '') || 'blueprint'
  )
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
  // Tool pins are parameter names from the selected tool's schema. Renaming
  // `command` to the legacy UI label `Command` changes its executable contract.
  const toolInstance = kind === 'Tool' && typeof n.data?.['tool_name'] === 'string'
  // Package instances retain the authored contract; the daemon rejects stale bindings.
  const signature =
    toolInstance || n.data?.['_addon_binding'] || n.data?.['_addon_function_binding']
      ? undefined
      : signatures.find((s) => s.kind === kind)
  const values = { ...n.values }
  for (const p of n.inputs ?? []) {
    const raw = inlineValue(n.data ?? {}, p)
    if (p.kind === 'data-in' && values[p.id] === undefined && raw !== undefined)
      values[p.id] = valueText(raw)
  }
  const byName = (p: BlueprintPin): BlueprintPin => {
    const contract = signature?.pins.find(
      (d) => d.kind === p.kind && ((p.key && d.key === p.key) || d.name === p.name),
    )
    const label = toolInstance ? p.name : contract?.name || (!signature && p.key ? LEGACY_PIN_NAMES[p.key] : undefined)
    const name = (label ?? p.name ?? '').replace(/\s+/g, '')
    return contract
      ? {
          ...contract,
          id: p.id,
          name,
          default: p.default === undefined ? contract.default : p.default,
        }
      : name !== p.name
        ? { ...p, name }
        : p
  }
  return {
    ...n,
    data: nodeConfiguration(n.data ?? {}, n.inputs ?? []),
    values,
    nodeType: signature?.nodeType ?? n.nodeType,
    type: n.type === kind ? n.type : kind,
    inputs: (n.inputs ?? []).map(byName),
    outputs: (n.outputs ?? []).map(byName),
  }
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
        values: Object.fromEntries(
          Object.entries(canonical.values ?? {}).map(([id, value]) => [
            pinIds.get(id) ?? id,
            value,
          ]),
        ),
        inputs: canonical.inputs.map((p) => ({ ...p, id: pinIds.get(p.id) ?? p.id })),
        outputs: canonical.outputs.map((p) => ({ ...p, id: pinIds.get(p.id) ?? p.id })),
      }
    }),
    edges: g.edges.map((e) => ({
      ...e,
      id: UUID_RE.test(e.id) ? e.id : crypto.randomUUID(),
      source: nodeIds.get(e.source) ?? e.source,
      target: nodeIds.get(e.target) ?? e.target,
      sourceHandle: e.sourceHandle
        ? (pinIds.get(e.sourceHandle) ?? e.sourceHandle)
        : e.sourceHandle,
      targetHandle: e.targetHandle
        ? (pinIds.get(e.targetHandle) ?? e.targetHandle)
        : e.targetHandle,
    })),
  }
}

export const useBlueprintStore = defineStore('blueprint', () => {
  const documents = useDocumentsStore()
  const loaded = ref(false)
  // These refs are the active canvas projection. Draft ownership is exclusively
  // in Documents, including workspace identity, close/rename and disk conflicts.
  const nodes = ref<BlueprintNode[]>([])
  const edges = ref<BlueprintEdge[]>([])
  const nodeKinds = ref<string[]>([])
  const signatures = ref<NodeKindInfo[]>([])
  const catalogMessage = ref('Loading node signatures…')
  const functions = ref<FunctionItem[]>([])
  const currentFile = ref('')
  const currentWorkspace = ref('')
  interface CanvasRecord {
    source: string
    baseline: string
    baselineKey: string
    graph: GraphData
  }
  // Parsed graphs are derived caches keyed by the owning document object. They
  // cannot survive an explicit discard/close or collide across workspaces.
  const parsed = reactive(new WeakMap<EditorDocument, CanvasRecord>())
  const mirrors = reactive(new Set<EditorDocument>())
  const catalogs = new Map<string, NodeKindInfo[]>()
  const catalogRequests = new Map<string, number>()
  let loadGeneration = 0
  function decode(content: string, catalog: NodeKindInfo[]): GraphData {
    if (!content.trim()) return seedGraph(catalog)
    let graph = JSON.parse(content) as Blueprint & { entry_node_id?: string }
    if (graph.entry_node_id) graph = nativeBlueprint(graph as unknown as NativeGraph)
    if (!Array.isArray(graph.nodes) || !Array.isArray(graph.edges))
      throw new Error('Blueprint must contain nodes and edges')
    return migrateIds(
      { id: graph.id || crypto.randomUUID(), nodes: graph.nodes, edges: graph.edges },
      catalog,
    )
  }
  function record(doc: EditorDocument): CanvasRecord {
    let cached = parsed.get(doc)
    if (!cached || cached.source !== doc.content || cached.baseline !== doc.baseline) {
      const catalog = catalogs.get(doc.workspace) ?? []
      const baseline = decode(doc.baseline, catalog)
      const graph = doc.content === doc.baseline ? baseline : decode(doc.content, catalog)
      cached = {
        source: doc.content,
        baseline: doc.baseline,
        baselineKey: keyFor(baseline.nodes, baseline.edges),
        graph,
      }
      parsed.set(doc, cached)
    }
    return cached
  }
  const currentDocuments = computed(() =>
    [...documents.records.values()].filter(
      (doc) => doc.workspace === currentWorkspace.value && doc.path.endsWith('.blueprint'),
    ),
  )
  const graphs = computed(() =>
    Object.fromEntries(
      currentDocuments.value
        .filter((doc) => parsed.has(doc))
        .map((doc) => [doc.path, parsed.get(doc)!.graph]),
    ),
  )
  const savedKeys = computed(() =>
    Object.fromEntries(
      currentDocuments.value.map((doc) => {
        if (!doc.loaded || !parsed.has(doc)) return [doc.path, '']
        try {
          return [doc.path, record(doc).baselineKey]
        } catch {
          return [doc.path, '']
        }
      }),
    ),
  )
  const saveErrors = computed(() =>
    Object.fromEntries(currentDocuments.value.map((doc) => [doc.path, doc.error])),
  )
  const saving = computed(() =>
    Object.fromEntries(
      currentDocuments.value.map((doc) => [doc.path, doc.saving || mirrors.has(doc)]),
    ),
  )
  const saved = computed(() => {
    const doc = documents.peek(currentWorkspace.value, currentFile.value)
    return (
      !!doc?.loaded &&
      parsed.has(doc) &&
      !doc.error &&
      !doc.deleted &&
      keyFor(nodes.value, edges.value) === record(doc).baselineKey
    )
  })
  function setError(path: string, file: string, error: string) {
    documents.get(path, file).error = error
  }
  function updateDraft(
    path: string,
    file: string,
    draftNodes: BlueprintNode[],
    draftEdges: BlueprintEdge[],
    entryNodeId?: string,
  ) {
    const doc = documents.get(path, file)
    if (!doc.loaded) return
    const cached = record(doc)
    const graph = { id: cached.graph.id, nodes: draftNodes, edges: draftEdges }
    const content =
      keyFor(graph.nodes, graph.edges) === cached.baselineKey
        ? doc.baseline
        : JSON.stringify(
            {
              ...graph,
              name: fileNameOf(file),
              entryNodeId:
                entryNodeId ??
                graph.nodes.find((n) => n.type === 'Start')?.id ??
                graph.nodes[0]?.id,
            },
            null,
            2,
          )
    documents.edit(path, file, content)
    cached.source = content
    cached.graph = graph
    if (currentWorkspace.value === path && currentFile.value === file) {
      nodes.value = graph.nodes
      edges.value = graph.edges
    }
  }

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

  let catalogGeneration = 0
  async function listKinds(path = '') {
    const ticket = ++catalogGeneration
    catalogRequests.set(path, ticket)
    const r = await gateway.listNodeKinds(path)
    if (catalogRequests.get(path) === ticket)
      catalogs.set(path, r.ok && r.data.ready ? r.data.nodes : [])
    if (ticket !== catalogGeneration) return
    nodeKinds.value = r.ok ? r.data.kinds : []
    signatures.value = r.ok && r.data.ready ? r.data.nodes : []
    catalogMessage.value =
      r.ok && r.data.ready
        ? ''
        : r.ok
          ? 'This daemon does not provide supported pin signatures. Existing graphs remain editable; node creation requires an updated daemon.'
          : `Node signatures unavailable: ${r.error}`
  }

  /** Load the function library (`path` empty = builtin + global). */
  let functionGeneration = 0
  async function loadFunctions(path = '') {
    const generation = ++functionGeneration
    const r = await gateway.listFunctions(path)
    if (generation !== functionGeneration) return
    if (r.ok) functions.value = r.data
  }

  /** The blueprint UUID of a file path, if it has been loaded. */
  function uuidFor(filePath: string, workspace = currentWorkspace.value): string | undefined {
    const doc = documents.peek(workspace, filePath)
    return doc?.loaded ? record(doc).graph.id : undefined
  }
  function graphFor(workspace: string, filePath: string) {
    const doc = documents.peek(workspace, filePath)
    if (!doc?.loaded) return null
    try {
      return record(doc).graph
    } catch {
      return null
    }
  }

  async function load(path: string, filePath: string) {
    const generation = ++loadGeneration
    currentWorkspace.value = path
    currentFile.value = filePath
    loaded.value = false
    await listKinds(path)
    const doc = await documents.open(path, filePath)
    if (!doc.loaded) return null
    try {
      const graph = record(doc).graph
      if (generation === loadGeneration) {
        nodes.value = graph.nodes
        edges.value = graph.edges
        loaded.value = true
      }
      return graph
    } catch (error) {
      doc.error = `Cannot read blueprint: ${String(error)}`
      return null // Never replace corrupt source with a fresh graph.
    }
  }

  async function save(
    path: string,
    filePath: string,
    entryNodeId?: string,
    draft?: Pick<GraphData, 'nodes' | 'edges'>,
  ): Promise<boolean> {
    const doc = documents.get(path, filePath)
    if (doc.saving || mirrors.has(doc)) return false
    if (
      !doc.loaded ||
      (!draft && (currentWorkspace.value !== path || currentFile.value !== filePath))
    ) {
      doc.error = 'Blueprint is not loaded. Save and run are unavailable.'
      return false
    }
    updateDraft(
      path,
      filePath,
      draft?.nodes ?? nodes.value,
      draft?.edges ?? edges.value,
      entryNodeId,
    )
    const cached = record(doc)
    // Both writes use one immutable snapshot; later edits remain a draft.
    const blueprint: Blueprint = JSON.parse(
      JSON.stringify({
        ...cached.graph,
        name: fileNameOf(filePath),
        entryNodeId:
          entryNodeId ??
          cached.graph.nodes.find((n) => n.type === 'Start')?.id ??
          cached.graph.nodes[0]?.id,
      }),
    )
    const snapshotKey = keyFor(blueprint.nodes, blueprint.edges)
    // Even a pristine, newly created empty file must receive the seeded graph.
    documents.edit(path, filePath, JSON.stringify(blueprint, null, 2))
    cached.source = doc.content
    mirrors.add(doc)
    let fileWritten = false
    try {
      if (!(await documents.save(path, filePath))) throw new Error(doc.error)
      fileWritten = true
      cached.baseline = doc.baseline
      cached.baselineKey = snapshotKey
      const mirror = await gateway.saveBlueprint(path, blueprint, filePath)
      if (!mirror.ok) throw new Error(mirror.error)
      doc.error = ''
      return true
    } catch (error) {
      doc.error = `${fileWritten ? 'File saved, but daemon mirror is not synchronized' : 'File save failed'}: ${error instanceof Error ? error.message : String(error)}. Run is blocked; retry saving.`
      return false
    } finally {
      mirrors.delete(doc)
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
    byId,
    currentFile,
    currentWorkspace,
    updateDraft,
    setError,
    pendingRef,
    requestAddReference,
    consumeReference,
    uuidFor,
    graphFor,
    listKinds,
    loadFunctions,
    load,
    save,
    select,
    setNodePosition,
    keyFor,
  }
})
