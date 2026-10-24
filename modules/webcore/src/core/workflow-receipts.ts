import type { DaemonGateway } from './gateway'
import type { Result } from './types'

export type WorkflowTarget =
  | { kind: 'blueprint'; run: string }
  | { kind: 'chat'; session: string; turn: string }
export type WorkflowAction = 'stop' | 'pause' | 'resume' | 'temporary_input'
export interface WorkflowReceipt {
  schema: 1
  id: string
  target: WorkflowTarget
  operation: WorkflowAction
  revision: number
  phase:
    | 'prepared'
    | 'claimed'
    | 'accepted'
    | 'queued'
    | 'applied'
    | 'unconsumed'
    | 'unknown'
    | 'retired'
  body: string | null
}
type Transport = Pick<DaemonGateway, 'resourceRequest'>
export const localWorkflowReceipts = new Map<
  string,
  { workspace: string; receipt: WorkflowReceipt }
>()
export const workflowQuery = (transport: Transport, workspace: string, action: unknown) =>
  transport.resourceRequest('chatDelivery', workspace, { operation: 'workflow', action })
const equalTarget = (a: WorkflowTarget, b: WorkflowTarget) =>
  a.kind === b.kind &&
  (a.kind === 'blueprint'
    ? a.run === (b as Extract<WorkflowTarget, { kind: 'blueprint' }>).run
    : a.session === (b as Extract<WorkflowTarget, { kind: 'chat' }>).session &&
      a.turn === (b as Extract<WorkflowTarget, { kind: 'chat' }>).turn)
export function matchesReceipt(actual: WorkflowReceipt | undefined, expected: WorkflowReceipt) {
  return (
    actual?.schema === 1 &&
    actual.id === expected.id &&
    equalTarget(actual.target, expected.target) &&
    actual.operation === expected.operation &&
    actual.body === expected.body
  )
}
export async function checkWorkflowReceipt(
  transport: Transport,
  workspace: string,
  expected: WorkflowReceipt,
): Promise<Result<WorkflowReceipt>> {
  const result = await workflowQuery(transport, workspace, {
    operation: 'fence',
    id: expected.id,
    target: expected.target,
    action: expected.operation,
    body: expected.body,
  })
  if (!result.ok) return result
  const record = (result.data as { receipt?: WorkflowReceipt }).receipt
  if (!matchesReceipt(record, expected))
    return {
      ok: false,
      error: 'Receipt target or content could not be verified. Keep the original identity.',
    }
  localWorkflowReceipts.set(expected.id, { workspace, receipt: record! })
  return { ok: true, data: record! }
}
/** One narrow RPC per identity. Reconciliation only reads/fences the same ID. */
export async function submitWorkflow(
  transport: Transport,
  workspace: string,
  target: WorkflowTarget,
  action: WorkflowAction,
  body: string | null,
  execute: (receiptJson: string) => Promise<Result<unknown>>,
  id: string = crypto.randomUUID(),
): Promise<Result<void>> {
  const snapshot: WorkflowReceipt = {
    schema: 1,
    id,
    target: JSON.parse(JSON.stringify(target)),
    operation: action,
    revision: 0,
    phase: 'unknown',
    body,
  }
  if (localWorkflowReceipts.size >= 512) {
    for (const [key, value] of localWorkflowReceipts)
      if (['accepted', 'applied', 'unconsumed', 'retired'].includes(value.receipt.phase))
        localWorkflowReceipts.delete(key)
    if (localWorkflowReceipts.size >= 512 && action !== 'stop')
      return { ok: false, error: 'Inspect unresolved receipts before submitting more input.' }
  }
  localWorkflowReceipts.set(id, { workspace, receipt: snapshot })
  let failure = ''
  try {
    // This capability query never waits for file admission. Verify old-host
    // target support before Stop; no prepare, backup or earlier control is awaited.
    if (action === 'stop') {
      const supported = await transport.resourceRequest('chatDelivery', workspace, {
        operation: 'capabilities',
      })
      if (
        !supported.ok ||
        (supported.data as { targeted_controls?: boolean }).targeted_controls !== true
      )
        return {
          ok: false,
          error: supported.ok
            ? 'Host cannot verify the exact Stop target. Update the host.'
            : supported.error,
        }
    }
    if (action !== 'stop') {
      const caps = await workflowQuery(transport, workspace, { operation: 'capabilities' })
      if (!caps.ok || (caps.data as { durable_controls?: boolean }).durable_controls !== true)
        return {
          ok: false,
          error: caps.ok ? 'Host receipt capability is unavailable; input was kept.' : caps.error,
        }
      const described = await workflowQuery(transport, workspace, {
        operation: 'describe',
        target: snapshot.target,
      })
      if (!described.ok) return described
      const revision = (described.data as { revision?: number }).revision
      if (!Number.isSafeInteger(revision) || revision! < 0)
        return { ok: false, error: 'Control revision could not be verified.' }
      snapshot.revision = revision!
      const prepared = await workflowQuery(transport, workspace, {
        operation: 'prepare',
        identity: { id, revision },
        target: snapshot.target,
        action,
        body,
      })
      if (!prepared.ok) return prepared
      const receipt = (prepared.data as { receipt?: WorkflowReceipt }).receipt
      if (!matchesReceipt(receipt, snapshot))
        return { ok: false, error: 'Prepared receipt could not be verified. Keep this identity.' }
      if (receipt!.phase !== 'prepared')
        return {
          ok: false,
          error: 'This identity already has a receipt. Check it without resubmitting.',
        }
    }
    const result = await execute(JSON.stringify({ id, revision: snapshot.revision }))
    if (!result.ok) failure = result.error
  } catch (error) {
    failure = String(error)
  }
  const checked = await checkWorkflowReceipt(transport, workspace, snapshot)
  if (!checked.ok)
    return {
      ok: false,
      error: `Outcome unconfirmed (${id}). Check Receipts before another request. ${failure || checked.error}`,
    }
  const receipt = checked.data
  if (
    action === 'temporary_input' &&
    ['queued', 'claimed', 'applied', 'unconsumed', 'unknown'].includes(receipt.phase)
  )
    return { ok: true, data: undefined }
  if (receipt.phase === 'accepted') return { ok: true, data: undefined }
  return {
    ok: false,
    error:
      receipt.phase === 'retired'
        ? `Request was not applied (${id}). Your input was kept.`
        : `Outcome unconfirmed (${id}). Check Receipts. ${failure}`,
  }
}
