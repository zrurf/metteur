// Controllable provider fixture: refusal is a terminal fixture result, never
// authorization to submit the same request again. Not real-model behavior proof.
export function blueprintOutcome(messages) {
  const tools = messages.filter(m => m.role === 'tool')
  for (const m of tools.toReversed()) {
    let v; try { v = typeof m.content === 'string' ? JSON.parse(m.content) : m.content } catch { continue }
    if (v?.state === 'declined' && v.started === false) return 'declined'
    if (v?.state === 'confirmed_pending_safe_yield') return 'confirmed'
  }
  return 'pending'
}
