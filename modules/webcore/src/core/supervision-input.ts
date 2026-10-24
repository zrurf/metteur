import { hash } from 'ohash'
import { RecoveryCopy, type RecoveryInput, type RecoveryRecord } from './recovery.ts'
import type { DaemonGateway } from './gateway'
import type { ConciergeTurn } from './concierge'
import type { DeliveryState } from './delivery'
import type { Result } from './types'
import { checkWorkflowReceipt, type WorkflowReceipt } from './workflow-receipts.ts'
type Transport = Pick<DaemonGateway, 'resourceRequest'>
type Pending = { id: string; run: string; original: string; sequence: number; raw_hash: string }
export interface SupervisionReceipt {
  schema_version: 1
  id: string
  run: string
  conversation_id: string
  phase: 'missing' | 'retired' | 'accepted' | 'unknown'
  turn: ConciergeTurn | null
}
const clone = <T>(value: T): T => JSON.parse(JSON.stringify(value))
function pendingOf(input: RecoveryInput): Pending | null {
  const value = (input.view as { pending_supervision?: Pending } | null)?.pending_supervision
  return value &&
    value.id === input.pending_submission &&
    value.run === input.entity &&
    typeof value.original === 'string' &&
    typeof value.raw_hash === 'string' &&
    Number.isSafeInteger(value.sequence)
    ? clone(value)
    : null
}
/** A run-owned draft with independent, immutable input originals. Reconciliation
 * can only inspect/retire the original identity, never call the model again. */
export class SupervisionInput {
  state: DeliveryState = 'draft'
  error = ''
  private pending: { info: Pending; original: RecoveryCopy | null } | null = null
  private retained = new Set<RecoveryCopy>()
  private reconciling: Promise<boolean> | null = null
  constructor(
    private transport: Transport,
    readonly primary: RecoveryCopy,
    private changed: () => void = () => {},
  ) {
    if (primary.input.pending_submission) this.state = 'unknown'
  }
  get canSubmit() {
    return !this.primary.input.pending_submission && !['submitting', 'unknown'].includes(this.state)
  }
  edit(update: Parameters<RecoveryCopy['edit']>[0]) {
    if (
      Object.entries(update).some(
        ([key, value]) =>
          JSON.stringify(this.primary.input[key as keyof RecoveryInput]) !== JSON.stringify(value),
      )
    )
      this.primary.edit(update)
  }
  capture() {
    return clone(this.primary.input)
  }
  async submit(
    text: string,
    execute: (
      id: string,
      text: string,
      receive: (turn: ConciergeTurn) => void,
    ) => Promise<Result<unknown>>,
    isCurrent: () => boolean = () => true,
  ) {
    if (!this.canSubmit) return false
    if (this.primary.input.selections.length) {
      this.error = 'Recovered references need review. Keep or copy the input before sending.'
      this.changed()
      return false
    }
    if (!text.trim() || new TextEncoder().encode(text).length > 16384) {
      this.error = 'Keep the draft and shorten this message before sending.'
      this.changed()
      return false
    }
    if (this.retained.size >= 32) {
      this.error = 'Keep or export the retained input copies before sending another message.'
      this.changed()
      return false
    }
    const source = this.capture(),
      original = new RecoveryCopy(
        this.transport,
        this.primary.workspace,
        this.primary.scope,
        'supervision',
        this.primary.input.entity,
        this.primary.input.window_id,
        this.changed,
      )
    Object.assign(original.input, { raw: text, selections: [] })
    this.retained.add(original)
    const info: Pending = {
        id: crypto.randomUUID(),
        run: this.primary.input.entity,
        original: original.input.id,
        sequence: -1,
        raw_hash: hash(text),
      },
      current = { info, original }
    this.pending = current
    this.state = 'submitting'
    this.error = ''
    this.changed()
    let received = false
    try {
      const capabilities = await this.transport.resourceRequest(
        'conciergeDelivery',
        this.primary.workspace,
        { operation: 'capabilities' },
      )
      const caps = capabilities.ok ? (capabilities.data as Record<string, unknown>) : null
      if (
        !caps ||
        caps.durable_intake !== true ||
        caps.read_or_retire !== true ||
        caps.pending_reconciliation !== true
      ) {
        this.state = 'unsupported'
        this.error = capabilities.ok
          ? 'This host cannot protect supervision input receipts.'
          : capabilities.error
        return false
      }
      const previous = await this.transport.resourceRequest(
        'conciergeDelivery',
        this.primary.workspace,
        { operation: 'reconcile_pending' },
      )
      if (!previous.ok) {
        this.state = 'unknown'
        this.error = previous.error
        return false
      }
      if (!isCurrent()) {
        this.state = 'draft'
        return false
      }
      original.edit({
        raw: text,
        selections: [],
        pending_submission: info.id,
        view: { pending_supervision: { ...info, sequence: 1 } },
      })
      if (!(await original.flush())) {
        this.state = 'draft'
        this.error = original.error
        return false
      }
      if (!isCurrent()) {
        this.state = 'draft'
        return false
      }
      info.sequence =
        this.primary.input.edit_sequence === source.edit_sequence &&
        this.primary.input.raw === text &&
        !this.primary.input.selections.length
          ? source.edit_sequence + 1
          : -1
      this.primary.edit({ pending_submission: info.id, view: { pending_supervision: info } })
      if (!(await this.primary.flush())) {
        this.state = 'unknown'
        this.error = this.primary.error
        return false
      }
      if (!isCurrent()) {
        this.state = 'unknown'
        return false
      }
      let result: Result<unknown>
      try {
        result = await execute(info.id, text, (turn) => {
          if (
            this.accept(
              {
                schema_version: 1,
                id: info.id,
                run: info.run,
                conversation_id: info.run,
                phase: 'accepted',
                turn,
              },
              true,
              current,
            )
          )
            received = true
        })
      } catch (error) {
        result = { ok: false, error: String(error) }
      }
      if (!received) received = await this.readOrRetire(current)
      if (!result.ok && !received) this.error = result.error || this.error
      return received
    } catch (error) {
      this.state = 'unknown'
      this.error = String(error)
      return false
    } finally {
      this.changed()
    }
  }
  private accept(
    receipt: SupervisionReceipt,
    sourceAvailable: boolean,
    current: { info: Pending; original: RecoveryCopy | null },
  ) {
    const { info } = current,
      turn = receipt.turn
    if (this.pending?.info.id !== info.id) {
      const verified =
        receipt.schema_version === 1 &&
        receipt.id === info.id &&
        receipt.run === info.run &&
        receipt.conversation_id === info.run &&
        receipt.phase === 'accepted' &&
        turn?.id === info.id &&
        turn.conversation_id === info.run &&
        hash(turn.original_text) === info.raw_hash
      if (verified && sourceAvailable && turn.state !== 'processing' && current.original)
        void this.cleanup(current.original)
      return verified
    }
    if (
      receipt.schema_version !== 1 ||
      receipt.id !== info.id ||
      receipt.run !== info.run ||
      receipt.conversation_id !== info.run
    ) {
      this.state = 'unknown'
      this.error = 'The supervision receipt could not be verified.'
      this.changed()
      return false
    }
    if (receipt.phase === 'retired') {
      if (this.primary.input.pending_submission === info.id) {
        this.primary.edit({ pending_submission: null, view: null })
        void this.primary.flush()
      }
      if (current.original) {
        current.original.edit({
          pending_submission: null,
          view: { disposition: 'unreceived_input' },
        })
        const original = current.original
        void original.flush().then((saved) => {
          if (saved) {
            this.retained.delete(original)
            original.dispose()
          }
        })
      }
      this.state = 'rejected'
      this.error = 'Input was not received. Your draft is kept.'
      this.changed()
      return false
    }
    if (
      receipt.phase !== 'accepted' ||
      !turn ||
      turn.id !== info.id ||
      turn.conversation_id !== info.run ||
      hash(turn.original_text) !== info.raw_hash
    ) {
      this.state = 'unknown'
      this.error = 'Input outcome is unconfirmed. Keep its original identity.'
      this.changed()
      return false
    }
    const matching = this.primary.input.pending_submission === info.id
    const exact =
      matching &&
      this.primary.input.edit_sequence === info.sequence &&
      hash(this.primary.input.raw) === info.raw_hash &&
      !this.primary.input.selections.length
    if (matching && (!exact || sourceAvailable)) {
      this.primary.edit({
        ...(exact ? { raw: '', selections: [] } : {}),
        pending_submission: null,
        view: null,
      })
      void this.primary.flush()
    }
    this.state = 'accepted'
    this.error =
      exact && !sourceAvailable ? 'The run is unavailable. Keep or copy this received input.' : ''
    this.changed()
    if (sourceAvailable && turn.state !== 'processing' && current.original)
      void this.cleanup(current.original)
    return true
  }
  private async cleanup(original: RecoveryCopy) {
    if (await original.discard()) {
      this.retained.delete(original)
      original.dispose()
    }
  }
  private async readOrRetire(current: { info: Pending; original: RecoveryCopy | null }) {
    const result = await this.transport.resourceRequest(
      'conciergeDelivery',
      this.primary.workspace,
      {
        operation: 'read_or_retire',
        run: current.info.run,
        conversation: current.info.run,
        id: current.info.id,
      },
    )
    if (!result.ok) {
      this.state = 'unknown'
      this.error = result.error
      this.changed()
      return false
    }
    const data = result.data as { receipt: SupervisionReceipt; source_available: boolean }
    return this.accept(data.receipt, data.source_available === true, current)
  }
  async reconcile(): Promise<boolean> {
    const temporary = (
      this.primary.input.view as {
        pending_temporary?: WorkflowReceipt & { sequence: number; raw_hash: string }
      } | null
    )?.pending_temporary
    if (temporary && temporary.id === this.primary.input.pending_submission)
      return this.reconcileTemporary(temporary)
    if (this.reconciling) return this.reconciling
    if (!this.pending) {
      const info = pendingOf(this.primary.input)
      if (!info) {
        this.state = 'unknown'
        this.error = 'Keep this input and inspect its original recovery copy.'
        this.changed()
        return false
      }
      this.pending = { info, original: null }
    }
    const current = this.pending
    this.reconciling = this.readOrRetire(current).catch((error) => {
      this.state = 'unknown'
      this.error = String(error)
      this.changed()
      return false
    })
    try {
      return await this.reconciling
    } finally {
      this.reconciling = null
    }
  }
  async backup() {
    return this.primary.flush()
  }
  private async reconcileTemporary(info: WorkflowReceipt & { sequence: number; raw_hash: string }) {
    const checked = await checkWorkflowReceipt(this.transport, this.primary.workspace, info)
    if (!checked.ok) {
      this.state = 'unknown'
      this.error = checked.error
      this.changed()
      return false
    }
    const record = checked.data
    if (record.phase === 'prepared' || record.phase === 'retired') {
      if (this.primary.input.pending_submission === info.id)
        this.primary.edit({ pending_submission: null, view: null })
      this.state = 'rejected'
      this.error = 'Temporary input was not received. Your draft was kept.'
      this.changed()
      void this.primary.flush()
      return false
    }
    if (['queued', 'claimed', 'applied', 'unconsumed', 'unknown'].includes(record.phase)) {
      const exact =
        this.primary.input.edit_sequence === info.sequence &&
        hash(this.primary.input.raw) === info.raw_hash
      if (this.primary.input.pending_submission === info.id)
        this.primary.edit({
          ...(exact && !['unconsumed', 'unknown'].includes(record.phase) ? { raw: '' } : {}),
          pending_submission: null,
          view: null,
        })
      this.state = 'accepted'
      this.error =
        record.phase === 'unconsumed'
          ? 'Input was received but not consumed. Its original is in Receipts.'
          : record.phase === 'unknown'
            ? 'Consumption is unconfirmed. Inspect the original receipt before sending again.'
            : ''
      this.changed()
      void this.primary.flush()
      return true
    }
    this.state = 'unknown'
    this.error = 'Receipt requires review; keep the input.'
    this.changed()
    return false
  }
  /** Temporary input has an immutable, host-backed identity before its RPC. */
  async temporary(
    execute: (text: string, id?: string) => Promise<Result<unknown>>,
    isCurrent: () => boolean = () => true,
  ) {
    if (!this.canSubmit || this.retained.size >= 32 || this.primary.input.selections.length)
      return false
    this.state = 'submitting'
    this.error = ''
    this.changed()
    const source = this.capture(),
      original = new RecoveryCopy(
        this.transport,
        this.primary.workspace,
        this.primary.scope,
        'supervision',
        this.primary.input.entity,
        this.primary.input.window_id,
        this.changed,
      )
    this.retained.add(original)
    const id = crypto.randomUUID()
    original.edit({
      raw: source.raw,
      pending_submission: id,
      view: { disposition: 'temporary_interrupt', run: this.primary.input.entity, receipt: id },
    })
    if (!(await original.flush()) || !isCurrent()) {
      this.state = 'rejected'
      this.error = original.error || 'Input was kept after the run changed.'
      this.changed()
      return false
    }
    const info: WorkflowReceipt & { sequence: number; raw_hash: string } = {
      schema: 1,
      id,
      target: { kind: 'blueprint', run: this.primary.input.entity },
      operation: 'temporary_input',
      revision: 0,
      phase: 'unknown',
      body: source.raw,
      sequence: -1,
      raw_hash: hash(source.raw),
    }
    const exact =
      this.primary.input.edit_sequence === source.edit_sequence &&
      this.primary.input.raw === source.raw
    this.primary.edit({ pending_submission: id, view: { pending_temporary: info } })
    info.sequence = exact ? this.primary.input.edit_sequence : -1
    this.primary.input.view = { pending_temporary: clone(info) }
    if (!(await this.primary.flush()) || !isCurrent()) {
      this.state = 'unknown'
      this.error = 'Pending input identity was kept; check its receipt before sending again.'
      this.changed()
      return false
    }
    this.state = 'submitting'
    this.error = ''
    this.changed()
    const result = await execute(source.raw, id)
    const received = await this.reconcileTemporary(info)
    if (received) {
      this.retained.delete(original)
      original.dispose()
    } else {
      if (!result.ok) this.error ||= result.error
      this.changed()
    }
    return received
  }
  /** Restoring forks the copy. It does not execute, grant permissions or select
   * a model. Rebase the clear sequence only for the exact original version. */
  static restored(record: RecoveryRecord) {
    const next = clone(record),
      pending = pendingOf(next.input)
    if (pending) {
      pending.sequence =
        pending.sequence === next.input.edit_sequence && hash(next.input.raw) === pending.raw_hash
          ? 1
          : -1
      next.input.view = { pending_supervision: pending }
    }
    const temporary = (
      next.input.view as {
        pending_temporary?: WorkflowReceipt & { sequence: number; raw_hash: string }
      } | null
    )?.pending_temporary
    if (temporary && temporary.id === next.input.pending_submission)
      temporary.sequence =
        temporary.sequence === next.input.edit_sequence &&
        hash(next.input.raw) === temporary.raw_hash
          ? 1
          : -1
    return next
  }
  get safeToDispose() {
    return [...this.retained].every((copy) => copy.revision > 0)
  }
  get localCopies(): RecoveryRecord[] {
    return [...this.retained]
      .filter((copy) => copy.revision === 0 && copy.input.raw)
      .map((copy) => ({
        schema_version: 1,
        revision: 0,
        input: clone(copy.input),
        updated_ms: 0,
        discarded: false,
      }))
  }
  dispose() {
    for (const copy of this.retained) copy.dispose()
    this.retained.clear()
  }
}
