/** Show the exact server-bound plan change on both approval surfaces. */
export function blueprintApproval(payload: Record<string, unknown>) {
  const type = payload.request_type
  if (type === 'circuit_tripped') return {
    title: 'Circuit stopped for your decision',
    detail: 'Allow once to retry the failed node without claiming validation. Deny once to stop this run.',
    command: JSON.stringify({ nodeId: payload.node_id, failures: payload.failures, summary: payload.summary }, null, 2),
  }
  if (type === 'oversight_gate') return {
    title: 'Review gate needs your decision',
    detail: 'Allow once to continue without changing the review conclusion. Deny once to retry the review. Stop cancels the run.',
    command: JSON.stringify({ reviewId: payload.review_id, status: payload.status, verdict: payload.verdict, summary: payload.summary }, null, 2),
  }
  if (type === 'oversight_control') return {
    title: payload.tool === 'CancelRun' ? 'Confirm cancellation of this run' : 'Confirm pause of this run',
    detail: `${typeof payload.summary === 'string' ? payload.summary : ''} This is a separate confirmation for one specific action. ${typeof payload.rollback_notice === 'string' ? payload.rollback_notice : ''}`,
    command: JSON.stringify({ action: payload.tool, dangerous: payload.dangerous, runId: payload.run_id, proposalId: payload.proposal_id, sourceRequestIds: payload.source_request_ids, originalRequests: payload.original_requests, expected: payload.expected }, null, 2),
  }
  if (type !== 'replan_proposal'  && type !== 'blueprint_save') return null
  const base = payload.base as Record<string, unknown> | undefined
  const file = typeof payload.path === 'string' ? payload.path : base?.blueprint_uri
  return {
    title: type === 'blueprint_save' ? 'Approve blueprint save' : 'Approve revised plan',
    detail: typeof payload.summary === 'string' ? payload.summary
      : `Review the proposed blueprint before saving${file ? ` to ${file}` : ''}.`,
    command: JSON.stringify({
      file, baseVersion: base ?? null, source: payload.source,
      proposalId: payload.proposal_id, runId: payload.run_id, sourceRequestIds: payload.source_request_ids, originalRequests: payload.original_requests,
      before: payload.before, after: payload.after,
      affectedNodes: payload.affected_nodes, retryNode: payload.retry_node, changes: payload.edits ?? payload.blueprint,
    }, null, 2),
  }
}
