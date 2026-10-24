export const CONTINUATION_WAITING_MESSAGE =
  'Task is waiting. Automatic continuation follows its saved policy; manage it in Tasks.'

export interface TaskSource {
  id: string
  kind: string
  generation: number
  may_write: boolean
  terminal: string | null
  result: unknown
}
export interface Schedule {
  id: string
  generation: number
  rule: unknown
  next_due: string | null
  last_emitted: string | null
  max_occurrences: number
  occurrences: number
  expires_at: string
  paused: boolean
  cancelled: boolean
  params: unknown
  misfire: string
}
export interface TaskRecord {
  workflow?: import('./workflow').WorkflowFacts
  launch_intent?: string | null
  pending_approvals?: {
    request_id: string
    detail: string
    content_digest?: string
    target?: import('./approval-decision').ApprovalTarget
  }[]
  revision: number
  policy: { purpose: string; expires_ms: number } | null
  schedules: Schedule[]
  has_saved_context: boolean
  task: {
    id: string
    session: string
    state: string
    generation: number
    expected_revision: number
    paused: boolean
    attention: string | null
    budget: {
      max_turns: number
      used_turns: number
      token_limit: number
      reserved_tokens: number
      consumed_tokens: number
      unknown_usage: boolean
    }
    sources: Record<string, TaskSource>
    wait: { sources: string[]; mode: string; deadline_ms: number; params: unknown } | null
  }
}
export interface TaskPage {
  items: TaskRecord[]
  next: string | null
}
export function stateLabel(value: string) {
  return value
    .split('_')
    .map((x) => x[0]?.toUpperCase() + x.slice(1))
    .join(' ')
}
