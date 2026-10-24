import type { DaemonGateway } from './gateway'

export interface ApprovalTarget {
  kind: 'chat' | 'blueprint'
  session?: string | null
  turn?: string | null
  run?: string | null
}
export interface ApprovalBinding {
  target: ApprovalTarget
  content_digest: string
}
export interface ApprovalSnapshot extends ApprovalBinding {
  workspace: string
  id: string
}
type Receipt = {
  schema_version: 1
  id: string
  target: ApprovalTarget
  content_digest: string
  decision: string | null
  phase: 'claimed' | 'accepted' | 'unknown' | 'retired'
}
type State =
  | 'ready'
  | 'checking'
  | 'submitting'
  | 'accepted'
  | 'retired'
  | 'unknown'
  | 'unsupported'
  | 'rejected'
const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i
const normalized = (target: ApprovalTarget) => ({
  kind: target.kind,
  session: target.session ?? null,
  turn: target.turn ?? null,
  run: target.run ?? null,
})
const sameTarget = (a: ApprovalTarget, b: ApprovalTarget) =>
  JSON.stringify(normalized(a)) === JSON.stringify(normalized(b))
function valid(target: ApprovalTarget) {
  return target.kind === 'chat'
    ? uuid.test(target.session ?? '') && uuid.test(target.turn ?? '') && !target.run
    : target.kind === 'blueprint' && uuid.test(target.run ?? '') && !target.session && !target.turn
}
type Transport = Pick<DaemonGateway, 'resourceRequest' | 'respondApproval'>
/** Uses the existing broker UUID as the single logical decision identity.
 * Queries never grant, retry or restore an authorization. */
export class ApprovalDecision {
  readonly snapshot: ApprovalSnapshot
  state: State = 'ready'
  error = ''
  receipt: Receipt | null = null
  private capable = false
  get busy() {
    return this.state === 'checking' || this.state === 'submitting'
  }
  get canRespond() {
    return this.state === 'ready'
  }
  constructor(
    private transport: Transport,
    snapshot: ApprovalSnapshot,
    private changed: () => void = () => {},
  ) {
    this.snapshot = JSON.parse(JSON.stringify(snapshot))
    if (
      !uuid.test(snapshot.id) ||
      !valid(snapshot.target) ||
      !/^[0-9a-f]{64}$/.test(snapshot.content_digest)
    ) {
      this.state = 'rejected'
      this.error =
        'The approval target or content is unconfirmed. Keep this request and refresh its source.'
    }
  }
  private publish(state: State, error = '') {
    this.state = state
    this.error = error
    this.changed()
  }
  private async query(payload: unknown) {
    const result = await this.transport.resourceRequest(
      'approvalDelivery',
      this.snapshot.workspace,
      payload,
    )
    if (!result.ok) throw Error(result.error)
    return result.data as {
      receipt?: Receipt | null
      pending?: { id: string; target: ApprovalTarget; content_digest: string } | null
      durable_decisions?: boolean
      fixed_target_content?: boolean
      read_or_retire?: boolean
    }
  }
  private async capabilities() {
    if (this.capable) return true
    const caps = await this.query({ operation: 'capabilities' })
    if (!caps.durable_decisions || !caps.fixed_target_content || !caps.read_or_retire) {
      this.publish(
        'unsupported',
        'Update the host before answering a protected approval. Direct Stop remains available.',
      )
      return false
    }
    this.capable = true
    return true
  }
  private acceptReceipt(receipt: Receipt, intended?: string) {
    if (
      !receipt ||
      receipt.schema_version !== 1 ||
      receipt.id !== this.snapshot.id ||
      !receipt.target ||
      !sameTarget(receipt.target, this.snapshot.target) ||
      receipt.content_digest !== this.snapshot.content_digest ||
      !['claimed', 'accepted', 'unknown', 'retired'].includes(receipt.phase)
    )
      throw Error('The approval receipt does not match this request, target and content.')
    if (
      receipt.phase !== 'retired' &&
      ![
        'AllowOnce',
        'AllowRun',
        'AllowWorkspace',
        'AllowGlobal',
        'DenyOnce',
        'DenyRun',
        'DenyWorkspace',
        'DenyGlobal',
      ].includes(receipt.decision ?? '')
    )
      throw Error('The approval receipt has an invalid decision. Keep the original identity.')
    this.receipt = JSON.parse(JSON.stringify(receipt))
    if (receipt.phase === 'accepted') {
      this.publish(
        'accepted',
        intended && receipt.decision !== intended
          ? `An earlier ${receipt.decision} decision was already accepted. Your new choice was not applied.`
          : '',
      )
      return true
    }
    if (receipt.phase === 'retired') {
      this.publish(
        'retired',
        'Decision not accepted. This request was closed without granting permission.',
      )
      return false
    }
    this.publish(
      'unknown',
      'Decision outcome is unconfirmed. Inspect this original receipt; it will not be replayed.',
    )
    return false
  }
  async respond(allow: boolean) {
    if (!this.canRespond) return false
    this.publish('checking')
    const intended = allow ? 'AllowOnce' : 'DenyOnce'
    let attempted = false
    try {
      if (!(await this.capabilities())) return false
      const described = await this.query({ operation: 'describe', id: this.snapshot.id })
      if (described.receipt) return this.acceptReceipt(described.receipt, intended)
      const pending = described.pending
      if (
        !pending ||
        pending.id !== this.snapshot.id ||
        !sameTarget(pending.target, this.snapshot.target) ||
        pending.content_digest !== this.snapshot.content_digest
      ) {
        this.publish(
          'rejected',
          'Approval source, target or content changed. No decision was submitted.',
        )
        return false
      }
      this.publish('submitting')
      attempted = true
      const result = await this.transport.respondApproval(
        this.snapshot.workspace,
        this.snapshot.id,
        allow,
        { target: this.snapshot.target, content_digest: this.snapshot.content_digest },
      )
      const checked = await this.query({ operation: 'read', id: this.snapshot.id })
      if (checked.receipt) return this.acceptReceipt(checked.receipt, intended)
      this.publish(
        'unknown',
        result.ok
          ? 'The decision reply arrived but its receipt is unavailable. Check the original receipt.'
          : result.error,
      )
      return false
    } catch (error) {
      this.publish(
        attempted
          ? 'unknown'
          : /unimplemented|unsupported/i.test(String(error))
            ? 'unsupported'
            : 'rejected',
        String(error),
      )
      return false
    }
  }
  async reconcile() {
    if (this.busy || ['accepted', 'retired'].includes(this.state)) return this.state === 'accepted'
    this.publish('checking')
    try {
      if (!(await this.capabilities())) return false
      const data = await this.query({
        operation: 'read_or_retire',
        id: this.snapshot.id,
        target: this.snapshot.target,
        content_digest: this.snapshot.content_digest,
      })
      if (!data.receipt)
        throw Error('The host did not return a decision receipt. Keep the original identity.')
      return this.acceptReceipt(data.receipt)
    } catch (error) {
      this.publish('unknown', String(error))
      return false
    }
  }
}
export async function approvalDigest(detail: string) {
  return [
    ...new Uint8Array(await crypto.subtle.digest('SHA-256', new TextEncoder().encode(detail))),
  ]
    .map((byte) => byte.toString(16).padStart(2, '0'))
    .join('')
}
