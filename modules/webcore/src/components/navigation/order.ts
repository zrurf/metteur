/** Local view preference only. Never changes routing, permissions or workspace data. */
export function normalizeOrder(stored: unknown, available: readonly string[]): string[] {
  const valid = new Set(available)
  const seen = new Set<string>()
  const result: string[] = []
  for (const key of Array.isArray(stored) ? stored : []) {
    if (typeof key === 'string' && valid.has(key) && !seen.has(key)) { result.push(key); seen.add(key) }
  }
  for (const key of available) if (!seen.has(key)) { result.push(key); seen.add(key) }
  return result
}
export function moveRelative(order: readonly string[], key: string, anchor: string, after: boolean): string[] {
  if (key === anchor || !order.includes(key) || !order.includes(anchor)) return [...order]
  const next = order.filter(k => k !== key)
  next.splice(next.indexOf(anchor) + (after ? 1 : 0), 0, key)
  return next
}
export function moveStep(order: readonly string[], key: string, delta: -1 | 1): string[] {
  const index = order.indexOf(key), target = index + delta
  if (index < 0 || target < 0 || target >= order.length) return [...order]
  const next = [...order]; [next[index], next[target]] = [next[target]!, next[index]!]
  return next
}
