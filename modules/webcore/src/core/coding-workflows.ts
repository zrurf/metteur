/** Host-owned coding workflow contracts. User decisions never grant tool permissions. */
export type TemplateKind = 'fix_bug' | 'add_tests' | 'refactor' | 'add_feature'
export interface TemplateInfo {
  id: TemplateKind
  revision: number
  title: string
  description: string
  acceptance: string
}
export interface CommandCheck {
  id: string
  description: string
  command: string
  expected_exit_code: number
  required: boolean
}
export interface ManualCheck {
  id: string
  description: string
  required: boolean
}
export interface TemplateInput {
  instance_id: string
  template: TemplateKind
  name: string
  goal: string
  constraints: string
  scope_paths: string[]
  evidence_paths: string[]
  model: string
  max_iterations: number
  baseline: CommandCheck
  checks: CommandCheck[]
  manual_checks: ManualCheck[]
}
export type EvidenceStatus = 'passed' | 'failed' | 'missing' | 'skipped' | 'stale' | 'needs_review'
export interface CriterionResult {
  id: string
  description: string
  required: boolean
  phase: 'baseline' | 'acceptance'
  status: EvidenceStatus
  reason: string
  command?: string
  output?: string
  files: { path: string; status: string; digest?: string | null }[]
  decision?: {
    decision: 'accepted' | 'changes_requested'
    note: string
    current: boolean
    subject: string
  } | null
}
export interface WorkflowReview {
  source?: { kind: 'chat' | 'independent' | 'unavailable'; session?: string; task?: string | null }
  run_id: string
  goal: string
  status: string
  digest: string
  revision: number
  criteria: CriterionResult[]
  can_review: boolean
  coverage: string
}
export interface ReviewDecision {
  criterion_id: string
  decision: 'accepted' | 'changes_requested'
  note: string
}

export interface PendingReview {
  request: string
  expected: number
  input: ReviewDecision & { run_id: string; evidence_digest: string }
}
export function isReviewDecision(raw: unknown): raw is ReviewDecision {
  if (!raw || typeof raw !== 'object') return false
  const v = raw as Record<string, unknown>
  return (
    typeof v.criterion_id === 'string' &&
    typeof v.note === 'string' &&
    (v.decision === 'accepted' || v.decision === 'changes_requested')
  )
}
export function isPendingReview(raw: unknown, run: string): raw is PendingReview {
  if (!raw || typeof raw !== 'object') return false
  const v = raw as PendingReview
  return (
    typeof v.request === 'string' &&
    /^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$/i.test(v.request) &&
    Number.isSafeInteger(v.expected) &&
    v.expected >= 0 &&
    v.expected <= 64 &&
    isReviewDecision(v.input) &&
    v.input.run_id === run &&
    typeof v.input.evidence_digest === 'string' &&
    /^[0-9a-f]{64}$/.test(v.input.evidence_digest)
  )
}

/** A successful transport response alone is not a durable decision acknowledgement. */
export function matchesReviewReceipt(raw: unknown, pending: PendingReview): boolean {
  if (!raw || typeof raw !== 'object') return false
  const data = raw as Record<string, any>
  const receipt = data.receipt,
    value = receipt?.value
  const decision = value?.decisions?.[pending.input.criterion_id]
  return (
    data.request_id === pending.request &&
    receipt?.schema_version === 1 &&
    receipt.revision === pending.expected + 1 &&
    value?.run_id === pending.input.run_id &&
    typeof value.subject === 'string' &&
    value.subject.length > 0 &&
    decision?.subject === value.subject &&
    decision.decision === pending.input.decision &&
    decision.note === pending.input.note &&
    decision.evidence_digest === pending.input.evidence_digest
  )
}

/** Incomplete input is recoverable; malformed structure must not mount the form. */
export function isTemplateDraft(raw: unknown): raw is TemplateInput {
  if (!raw || typeof raw !== 'object') return false
  const x = raw as Record<string, any>
  const strings = (v: unknown) =>
    Array.isArray(v) && v.length <= 64 && v.every((s) => typeof s === 'string')
  const check = (v: any) =>
    !!v &&
    ['id', 'description', 'command'].every((k) => typeof v[k] === 'string') &&
    Number.isInteger(v.expected_exit_code) &&
    typeof v.required === 'boolean'
  return (
    ['fix_bug', 'add_tests', 'refactor', 'add_feature'].includes(x.template) &&
    ['instance_id', 'name', 'goal', 'constraints', 'model'].every(
      (k) => typeof x[k] === 'string',
    ) &&
    strings(x.scope_paths) &&
    strings(x.evidence_paths) &&
    Number.isInteger(x.max_iterations) &&
    check(x.baseline) &&
    Array.isArray(x.checks) &&
    x.checks.length <= 16 &&
    x.checks.every(check) &&
    Array.isArray(x.manual_checks) &&
    x.manual_checks.length <= 16 &&
    x.manual_checks.every(
      (v: any) =>
        v &&
        typeof v.id === 'string' &&
        typeof v.description === 'string' &&
        typeof v.required === 'boolean',
    )
  )
}

export function plannerIssues(input: TemplateInput): string[] {
  const issues: string[] = []
  if (!input.name.trim() || !input.goal.trim()) issues.push('Add a name and a concrete goal.')
  if (!input.model.trim()) issues.push('Select a configured model.')
  if (!input.scope_paths.length || !input.evidence_paths.length)
    issues.push('List the task scope and evidence files.')
  if (
    !input.baseline.command.trim() ||
    !input.checks.length ||
    input.checks.some((c) => !c.command.trim() || !c.description.trim())
  )
    issues.push('Add the baseline and final check commands and descriptions.')
  if (!input.checks.some((c) => c.required)) issues.push('Keep at least one required final check.')
  if (input.manual_checks.some((c) => !c.description.trim()))
    issues.push('Describe each human review requirement.')
  if (
    !Number.isInteger(input.baseline.expected_exit_code) ||
    input.baseline.expected_exit_code < 0 ||
    input.baseline.expected_exit_code > 255
  )
    issues.push('Use an expected baseline exit code from 0 to 255.')
  if (
    input.template === 'fix_bug'
      ? input.baseline.expected_exit_code <= 0
      : input.baseline.expected_exit_code !== 0
  )
    issues.push('A bug baseline expects a specific nonzero exit; other baselines expect zero.')
  if (
    !Number.isInteger(input.max_iterations) ||
    input.max_iterations < 1 ||
    input.max_iterations > 64
  )
    issues.push('Use 1 to 64 model iterations.')
  return issues
}
