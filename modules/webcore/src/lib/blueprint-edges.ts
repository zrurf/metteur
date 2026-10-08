import type { NodeCategory } from '@/core'

export const CATEGORY_ACCENT: Record<NodeCategory, string> = {
  event: '#3f8cff', module: '#8b5cf6', action: '#f2994a', flow: '#2fbf8f',
}

/** Share the editor's Bezier wires and pin category colours with run views. */
export function blueprintEdgeVisual(exec: boolean, category: NodeCategory = 'module') {
  return { type: 'default', class: exec ? 'metteur-edge--exec' : 'metteur-edge--data',
    animated: false, style: exec ? undefined : { stroke: CATEGORY_ACCENT[category] } }
}
