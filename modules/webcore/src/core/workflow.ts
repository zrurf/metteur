export type SavedResultReference =
  | { kind: 'task'; taskId: string; sourceId: string; digest: string }
  | { kind: 'run'; runId: string; digest: string }
export interface ResultChoice {
  label: string
  reference: SavedResultReference
}
export function validResultReference(value: unknown): value is SavedResultReference {
  if (!value || typeof value !== 'object') return false
  const r = value as Record<string, unknown>,
    uuid = /^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$/i
  return (
    typeof r.digest === 'string' &&
    /^[a-f0-9]{64}$/i.test(r.digest) &&
    (r.kind === 'task'
      ? typeof r.taskId === 'string' &&
        uuid.test(r.taskId) &&
        typeof r.sourceId === 'string' &&
        r.sourceId.length > 0
      : r.kind === 'run' && typeof r.runId === 'string' && uuid.test(r.runId))
  )
}
export interface WorkflowFacts {
  goal: string
  reason_code: string
  reason: string
  next_action: string
  allowed_actions: string[]
  waiting_on: { kind: string; terminal: string | null }[]
  target: { task: string; session: string; revision: number }
  origin: 'host_state'
}
