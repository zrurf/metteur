import type { BlueprintPin, NodeCatalog } from './types'

export interface WirePin {
  id: string
  key: string
  name: string
  pinType: string
  dataType: string
  defaultJson?: string
  optional?: boolean
  choices?: string[]
  description?: string
}

export function parseDefault(raw?: string): unknown {
  if (!raw) return undefined
  try { return JSON.parse(raw) } catch { return undefined }
}

export function pinType(raw: string): string {
  const type = raw.toLowerCase()
  if (['number', 'double', 'f64'].includes(type)) return 'float'
  if (['list', 'array'].includes(type)) return 'list<any>'
  if (['integer', 'i64'].includes(type)) return 'int'
  if (type === 'boolean') return 'bool'
  if (type === 'str') return 'string'
  if (['null', 'none'].includes(type)) return 'void'
  if (type === 'enum') return 'choice'
  if (['object', 'map', 'dict'].includes(type)) return 'object{}'
  return ['string', 'float', 'int', 'bool', 'json', 'context', 'any', 'void', 'choice'].includes(type) ? type : raw
}

export function fromWirePin(p: WirePin): BlueprintPin {
  const kind = p.pinType === 'ExecInput' ? 'exec-in' : p.pinType === 'ExecOutput' ? 'exec-out'
    : p.pinType === 'DataInput' ? 'data-in' : 'data-out'
  return {
    id: p.id, key: p.key || p.name || undefined,
    name: kind.startsWith('exec') && ['x-in', 'x-out'].includes(p.name) ? '' : p.name,
    kind, type: pinType(p.dataType), choices: p.choices,
    default: parseDefault(p.defaultJson), optional: p.optional, description: p.description,
  }
}

/** Legacy/partial catalogues never synthesize execution contracts. */
export function fromWireCatalog(raw: {
  kinds: string[]
  signatureVersion?: number
  infos?: Array<{ kind: string; nodeType: string; pins: WirePin[]; dynamicPins: boolean; description: string; addonBindingJson?: string }>
}): NodeCatalog {
  const kinds = [...new Set(raw.kinds)]
  const infos = raw.infos ?? []
  const ready = raw.signatureVersion === 1 && kinds.every((k) => infos.filter((n) => n.kind === k).length === 1)
  return {
    kinds, signatureVersion: raw.signatureVersion ?? 0, ready,
    nodes: ready ? infos.filter((n) => kinds.includes(n.kind)).map((n) => ({ ...n, addonBinding: parseDefault(n.addonBindingJson) as Record<string, unknown> | undefined, pins: n.pins.map(fromWirePin) })) : [],
  }
}

export function valueText(value: unknown): string {
  return value === undefined ? '' : typeof value === 'string' ? value : JSON.stringify(value)
}

/** Match the daemon's name/key/id lookup, retaining explicit JSON null. */
export function inlineValue(data: Record<string, unknown>, pin: { name: string; key?: string; id: string }): unknown {
  for (const key of [pin.name, pin.key, pin.id]) {
    if (key && Object.hasOwn(data, key)) return data[key]
  }
  return undefined
}

/** Keep non-pin configuration separately so clearing an editor cannot resurrect a stale value. */
export function nodeConfiguration(data: Record<string, unknown>, pins: BlueprintPin[]): Record<string, unknown> {
  const config = { ...data }
  for (const p of pins.filter((pin) => pin.kind === 'data-in')) {
    for (const key of [p.name, p.key, p.id]) if (key) delete config[key]
  }
  return config
}
