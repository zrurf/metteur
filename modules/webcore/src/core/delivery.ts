import type { DaemonGateway } from './gateway'
import type { ChatDeliveryReceipt, ChatMessage, ChatOptions, Result } from './types'

export interface DeliverySnapshot {
  workspace: string
  session: string
  newSession?: string
  message: string
  history: ChatMessage[]
  options: ChatOptions
  live?: { mode: 'queue' | 'steer'; turn: string }
}
export type DeliveryState =
  | 'draft'
  | 'submitting'
  | 'accepted'
  | 'rejected'
  | 'unknown'
  | 'unsupported'
type Transport = Pick<DaemonGateway, 'resourceRequest'>
const clone = <T>(value: T): T => JSON.parse(JSON.stringify(value))

/** One logical input identity. Unknown outcomes only reconcile; a new user
 * submission requires a new instance after the old identity is proven closed. */
export class DeliveryAttempt {
  readonly id: string
  readonly snapshot: DeliverySnapshot
  state: DeliveryState = 'draft'
  error = ''
  receipt: ChatDeliveryReceipt | null = null
  execution: Result<unknown> | null = null
  private prepared = false
  private attempted = false
  private reconciling: Promise<boolean> | null = null
  private accepted = false
  constructor(
    private transport: Transport,
    snapshot: DeliverySnapshot,
    private onAccepted: (receipt: ChatDeliveryReceipt) => void = () => {},
    private changed: () => void = () => {},
    pendingId?: string,
  ) {
    this.id = pendingId ?? crypto.randomUUID()
    this.snapshot = clone(snapshot)
    if (pendingId) {
      this.attempted = true
      this.state = 'unknown'
    }
  }
  async prepare() {
    if (this.attempted) return false
    const capabilities = await this.transport.resourceRequest(
      'chatDelivery',
      this.snapshot.workspace,
      { operation: 'capabilities' },
    )
    if (
      !capabilities.ok ||
      (capabilities.data as { durable_manual_send?: boolean }).durable_manual_send !== true ||
      (capabilities.data as { pending_reconciliation?: boolean }).pending_reconciliation !== true ||
      (this.snapshot.options.result_references?.length &&
        (capabilities.data as { frozen_result_references?: boolean }).frozen_result_references !==
          true) ||
      (this.snapshot.options.file_references?.length &&
        (capabilities.data as { frozen_file_references?: boolean }).frozen_file_references !==
          true) ||
      (this.snapshot.live &&
        ((capabilities.data as { durable_queue?: boolean }).durable_queue !== true ||
          (capabilities.data as { durable_queue_channel?: string }).durable_queue_channel !==
            'chat'))
    ) {
      this.state = 'unsupported'
      this.error = capabilities.ok
        ? 'This host does not provide durable input receipts with pending-input reconciliation.'
        : capabilities.error
      this.changed()
      return false
    }
    const pending = await this.transport.resourceRequest('chatDelivery', this.snapshot.workspace, {
      operation: 'reconcile_pending',
    })
    if (!pending.ok) {
      this.state = 'unknown'
      this.error = pending.error
      this.changed()
      return false
    }
    let revision = 0
    if (this.snapshot.session && !this.snapshot.live) {
      const current = await this.transport.resourceRequest(
        'chatDelivery',
        this.snapshot.workspace,
        { operation: 'session_revision', session: this.snapshot.session },
      )
      if (!current.ok) {
        this.state = 'rejected'
        this.error = current.error
        this.changed()
        return false
      }
      const value = current.data as { session: string; revision: number }
      if (
        value.session !== this.snapshot.session ||
        !Number.isSafeInteger(value.revision) ||
        value.revision < 0
      ) {
        this.state = 'unknown'
        this.error = 'Conversation revision could not be verified.'
        this.changed()
        return false
      }
      revision = value.revision
    }
    this.snapshot.options = {
      ...this.snapshot.options,
      delivery: {
        id: this.id,
        expected_revision: revision,
        ...(!this.snapshot.session && this.snapshot.newSession
          ? { new_session: this.snapshot.newSession }
          : {}),
      },
    }
    this.prepared = true
    return true
  }
  acknowledge(receipt: ChatDeliveryReceipt) {
    if (
      receipt.schema_version !== 1 ||
      receipt.id !== this.id ||
      !(
        this.snapshot.live
          ? [
              'retired',
              'live_queued',
              'live_claimed',
              'live_applied',
              'live_unconsumed',
              'live_unknown',
            ]
          : ['retired', 'accepted', 'started', 'finished', 'needs_attention']
      ).includes(receipt.phase)
    ) {
      this.state = 'unknown'
      this.error = 'Input receipt could not be verified.'
      this.changed()
      return false
    }
    if (receipt.phase === 'retired') {
      this.receipt = clone(receipt)
      this.state = 'rejected'
      this.error = 'This input was not received. Your draft is kept.'
      this.changed()
      return false
    }
    const expected = this.snapshot.session || this.snapshot.newSession
    if (
      (expected && receipt.session !== expected) ||
      !receipt.session ||
      !receipt.turn ||
      !Number.isSafeInteger(receipt.accepted_revision) ||
      receipt.accepted_revision < (this.snapshot.live ? 0 : 1) ||
      (this.snapshot.live &&
        (receipt.turn !== this.snapshot.live.turn ||
          receipt.live?.mode !== this.snapshot.live.mode ||
          !/^[0-9a-f]{64}$/.test(receipt.live.body_digest)))
    ) {
      this.state = 'unknown'
      this.error = 'Input receipt belongs to a different conversation or version.'
      this.changed()
      return false
    }
    this.receipt = clone(receipt)
    this.state = 'accepted'
    this.error = ''
    if (!this.accepted) {
      this.accepted = true
      this.onAccepted(receipt)
    }
    this.changed()
    return true
  }
  async submit(
    run: (
      snapshot: DeliverySnapshot,
      onReceipt: (receipt: ChatDeliveryReceipt) => void,
    ) => Promise<Result<unknown>>,
    beforeSubmit: () => Promise<boolean> = async () => true,
  ) {
    if (this.attempted || this.state === 'submitting') return false
    this.state = 'submitting'
    this.changed()
    try {
      if (!this.prepared && !(await this.prepare())) return false
      if (!(await beforeSubmit())) {
        this.state = 'draft'
        this.error = 'Back up the pending input before submitting.'
        this.changed()
        return false
      }
      this.attempted = true
      try {
        this.execution = await run(clone(this.snapshot), (receipt) => this.acknowledge(receipt))
      } catch (error) {
        this.execution = { ok: false, error: String(error) }
      }
      if (this.accepted) return true // Model failure is an execution outcome.
      const accepted = await this.reconcile()
      if (!accepted && this.receipt?.phase === 'retired' && this.execution && !this.execution.ok) {
        this.error = this.execution.error || this.error
        this.changed()
      }
      return accepted
    } catch (error) {
      this.state = 'unknown'
      this.error = String(error)
      this.changed()
      return false
    }
  }
  async reconcile(): Promise<boolean> {
    if (this.accepted) return true
    if (this.reconciling) return this.reconciling
    this.reconciling = this.readOrRetire()
    try {
      return await this.reconciling
    } finally {
      this.reconciling = null
    }
  }
  private async readOrRetire() {
    this.state = 'unknown'
    this.changed()
    try {
      let result = await this.transport.resourceRequest('chatDelivery', this.snapshot.workspace, {
        operation: 'read',
        id: this.id,
      })
      if (!result.ok) {
        this.error = result.error
        this.changed()
        return false
      }
      const readReceipt = (value: unknown) => {
        const data = value as { receipt: ChatDeliveryReceipt | null; source_available?: boolean }
        return data.receipt && typeof data.source_available === 'boolean'
          ? { ...data.receipt, source_available: data.source_available }
          : data.receipt
      }
      let receipt = readReceipt(result.data)
      if (!receipt) {
        // An absent read alone does not fence a request still in transport.
        result = await this.transport.resourceRequest('chatDelivery', this.snapshot.workspace, {
          operation: 'retire',
          id: this.id,
        })
        if (!result.ok) {
          this.error = result.error
          this.changed()
          return false
        }
        receipt = readReceipt(result.data)
      }
      if (!receipt) {
        this.error = 'Input outcome remains unknown. Keep its original identity.'
        this.changed()
        return false
      }
      return this.acknowledge(receipt)
    } catch (error) {
      this.error = String(error)
      this.changed()
      return false
    }
  }
}
