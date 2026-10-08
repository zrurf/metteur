import type { VersionRef } from './execution-view'
export interface ProposalNode { id: string; kind: string; [key: string]: unknown }
export interface OversightProposal {
  proposal_id: string
  run_id?: string
  review_id?: string
  source_request_ids?: string[]
  original_requests?: Array<{ request_id: string; source: string; original_text: string }>
  state: string
  reason?: string
  decision_source?: string
  kind?: string
  result_refs: string[]
  result_version?: VersionRef | null
  diagnostic?: OversightDiagnostic | null
  binding?: {
    scope?: string; source?: string; summary?: string; base?: VersionRef
    affected_nodes?: string[]; before?: ProposalNode[]; after?: ProposalNode[]
    expected?: { base?: VersionRef; [key: string]: unknown }; rollback_notice?: string
  }
}
export interface ProposalTarget { proposal: OversightProposal; nodeId: string }
export interface ProposalVersion { proposal: OversightProposal; version: VersionRef; label: string }
export interface OversightDiagnostic {
  category: string
  stage: string
  message: string
  run_id: string
  review_id: string
  proposal_id?: string | null
  call_id?: string | null
  http_status?: number | null
}
export interface OversightReview {
  review_id: string
  run_id: string
  status: string
  diagnostic?: OversightDiagnostic | null
  verdict: string | null
  model_verdict?: string | null
  circuit_node?: string | null
  human_dispositions?: Array<{ node_id: string; at_ms: number; action: string }>
  summary: string
  source_request_ids: string[]
  triggers: string[]
  finished_at: number | null
  actual_action_refs: string[]
  cancel_result?: { rollback_requested: boolean; restored_operations: number | null; error: string | null; files: Array<{ path: string; phase: string }> } | null
  proposals?: OversightProposal[]
  work: { model: string; answers: string[]; notes: string[]; evidence: Array<{ entry_id: string; node_id: string | null; scope: string | null }> }
  usage?: Array<{ id: string; model: string; charged: number; state: string; accounting_version?: number; cost_micros: number | null; currency: string }>
}
export interface OversightReports { run_id: string; reports: OversightReview[]; closing?: { source: string; checkpoint_status: string; checkpoint_error?: string | null; recovery_required: boolean } | null }
