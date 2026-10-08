import type { BlueprintPin, FunctionItem, NodeCategory, NodeKindInfo } from '@/core/types'
import type { Node as FlowNode } from '@vue-flow/core'
import { pinType } from '@/core/node-catalog'

export const DATA_COLORS: Record<string, string> = {
  string: '#ec6b7e',
  float: '#e2a13c',
  number: '#e2a13c',
  int: '#e2a13c',
  bool: '#b78be0',
  object: '#6aa7ec',
  json: '#6aa7ec',
  context: '#4ec9a4',
  choice: '#b78be0',
  any: '#8a8f98',
}

/** Split structural fields without splitting nested object/list types. */
function fields(type: string): Record<string, string> {
  const body = type.slice(7, -1)
  let depth = 0, start = 0
  const result: Record<string, string> = {}
  for (let i = 0; i <= body.length; i++) {
    const c = body[i]
    if (c === '<' || c === '{') depth++
    if (c === '>' || c === '}') depth--
    if (i === body.length || (c === ',' && depth === 0)) {
      const field = body.slice(start, i), colon = field.indexOf(':')
      if (colon >= 0) result[field.slice(0, colon).trim()] = field.slice(colon + 1).trim()
      start = i + 1
    }
  }
  return result
}

/** Mirrors shared::model::compatible, including directional numeric widening. */
export function isPinCompatible(src = 'any', dst = 'any'): boolean {
  const a = pinType(src)
  const b = pinType(dst)
  if (['any', 'json'].includes(a) || ['any', 'json'].includes(b)) return true
  if (a === 'context' || b === 'context') return a === b
  if (a.startsWith('list<') && b.startsWith('list<')) return isPinCompatible(a.slice(5, -1), b.slice(5, -1))
  if (a.startsWith('object{') && b.startsWith('object{')) {
    const target = fields(b)
    return Object.entries(fields(a)).every(([name, type]) => !target[name] || isPinCompatible(type, target[name]))
  }
  if (a === 'int' && b === 'float') return true
  if (['int', 'float', 'bool'].includes(a) && b === 'string') return true
  if (['choice', 'string'].includes(a) && ['choice', 'string'].includes(b)) return true
  return ['int', 'float', 'bool', 'string', 'void', 'choice'].includes(a) && a === b
}

/** Visual preferences only; capabilities and pins always come from the daemon. */
export const NODE_PRESETS: Record<string, { category: NodeCategory; title?: string }> = {
  Tool: { category: 'action' }, RequestApproval: { category: 'action' },
}
export function categoryFor(kind: string, nodeType?: string): NodeCategory {
  return NODE_PRESETS[kind]?.category ?? (nodeType === 'Event' ? 'event'
    : nodeType === 'Pure' || nodeType === 'Control' ? 'flow' : 'module')
}

/** Generate a fresh UUID usable as a node/pin/edge id. */
export const uuid = (): string => crypto.randomUUID()

/** Find a node's pin by its semantic key (e.g. `x-in`, `prompt`). */
export function pinByKey(pins: BlueprintPin[], key: string): BlueprintPin | undefined {
  return pins.find((p) => p.key === key)
}

/** The exec-input pin of a node, if any. */
export function execInOf(pins: BlueprintPin[]): BlueprintPin | undefined {
  return pins.find((p) => p.kind === 'exec-in')
}

/** The exec-output pin of a node, if any. */
export function execOutOf(pins: BlueprintPin[]): BlueprintPin | undefined {
  return pins.find((p) => p.kind === 'exec-out')
}

/** Instantiate a daemon contract without sharing mutable pins between nodes. */
export function pinsFor(signature: NodeKindInfo): { inputs: BlueprintPin[]; outputs: BlueprintPin[] } {
  const pins = signature.pins.map((p) => ({ ...p, choices: p.choices ? [...p.choices] : undefined, id: uuid() }))
  return {
    inputs: pins.filter((p) => p.kind === 'exec-in' || p.kind === 'data-in'),
    outputs: pins.filter((p) => p.kind === 'exec-out' || p.kind === 'data-out'),
  }
}
export function makeFlowNode(signature: NodeKindInfo, position: { x: number; y: number }, id: string): FlowNode {
  return {
    id, type: 'blueprint', position,
    data: { ...pinsFor(signature), kind: signature.kind, nodeType: signature.nodeType,
      title: NODE_PRESETS[signature.kind]?.title ?? signature.kind,
      category: categoryFor(signature.kind, signature.nodeType), data: signature.addonBinding ? { _addon_binding: JSON.parse(JSON.stringify(signature.addonBinding)) } : {}, values: {} },
  }
}

/** Function pins are resolved from the daemon library; fixed pins use its executor contract. */
export function makeCallFunctionNode(entry: FunctionItem, signature: NodeKindInfo, position: { x: number; y: number }, id: string): FlowNode {
  const node = makeFlowNode(signature, position, id)
  const pins = (items: FunctionItem['inputs'], kind: BlueprintPin['kind']) => items.map((p) => ({
    ...p, id: uuid(), key: p.name, kind,
  }))
  node.data!.inputs.push(...pins(entry.inputs, 'data-in'))
  node.data!.outputs.push(...pins(entry.outputs, 'data-out'))
  node.data!.title = entry.name
  node.data!.data = { function: entry.name }
  if (entry.addonBinding) node.data!.data['_addon_function_binding'] = JSON.parse(JSON.stringify(entry.addonBinding))
  return node
}

/** A local canvas annotation, excluded from the executable graph. */
export function makeFileReference(filePath: string, position: { x: number; y: number }, id: string): FlowNode {
  const pin: BlueprintPin = { id: uuid(), key: 'path', name: 'Path', kind: 'data-in', type: 'string' }
  return { id, type: 'blueprint', position, data: { kind: 'FileReference',
    title: filePath.split(/[\\/]/).pop() ?? filePath, category: 'module', data: {},
    inputs: [pin], outputs: [], values: { [pin.id]: filePath } } }
}

/** Category ordering + label used by the palette grouping. */
export const CATEGORIES: Array<{ key: NodeCategory; label: string }> = [
  { key: 'event', label: 'Events' },
  { key: 'module', label: 'Module' },
  { key: 'action', label: 'Actions' },
  { key: 'flow', label: 'Flow' },
]
