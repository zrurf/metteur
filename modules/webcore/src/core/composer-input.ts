import { hash } from 'ohash'
import { RecoveryCopy, type RecoveryInput, type RecoveryRecord } from './recovery.ts'
import { DeliveryAttempt, type DeliverySnapshot, type DeliveryState } from './delivery.ts'
import type { DaemonGateway } from './gateway'
import type { ChatDeliveryReceipt, Result } from './types'

type Transport = Pick<DaemonGateway, 'resourceRequest'>
const clone = <T>(value: T): T => JSON.parse(JSON.stringify(value))
export const inputFingerprint = (input: Pick<RecoveryInput, 'raw' | 'selections'>) =>
  hash({ raw: input.raw, selections: input.selections })
export interface PendingInput {
  id: string
  original: string
  session: string
  new_session?: string
  clear_sequence: number
  content_hash: string
  mode: 'send' | 'retry' | 'queue' | 'steer'
  turn?: string
}
export function pendingInput(input: RecoveryInput): PendingInput | null {
  const view = input.view as { pending_delivery?: PendingInput } | null
  const pending = view?.pending_delivery
  return pending &&
    pending.id === input.pending_submission &&
    typeof pending.session === 'string' &&
    typeof pending.original === 'string' &&
    typeof pending.content_hash === 'string' &&
    ['send', 'retry', 'queue', 'steer'].includes(pending.mode) &&
    (!['queue', 'steer'].includes(pending.mode) || typeof pending.turn === 'string') &&
    Number.isSafeInteger(pending.clear_sequence)
    ? clone(pending)
    : null
}
function viewWith(input: RecoveryInput, pending: PendingInput | null) {
  const view =
    typeof input.view === 'object' && input.view !== null && !Array.isArray(input.view)
      ? (clone(input.view) as Record<string, unknown>)
      : {}
  if (pending) view.pending_delivery = pending
  else delete view.pending_delivery
  return Object.keys(view).length ? view : null
}
/** Rebase only the exact restored input version, never an edit that followed it. */
export function restoredInput(record: RecoveryRecord): RecoveryRecord {
  const next = clone(record),
    pending = pendingInput(next.input)
  if (pending) {
    pending.clear_sequence =
      pending.clear_sequence === next.input.edit_sequence &&
      pending.content_hash === inputFingerprint(next.input)
        ? 1
        : -1
    next.input.view = viewWith(next.input, pending)
  }
  return next
}

/** A live composer and a separate immutable input copy for each logical send.
 * This retains both versions when typing continues while submission is pending. */
export class ComposerInput {
  error = ''
  private pending: {
    attempt: DeliveryAttempt
    info: PendingInput
    original: RecoveryCopy | null
  } | null = null
  private retained = new Set<RecoveryCopy>()
  constructor(
    private transport: Transport,
    readonly primary: RecoveryCopy,
    private changed: () => void = () => {},
  ) {}
  get state(): DeliveryState {
    return (
      this.pending?.attempt.state ?? (this.primary.input.pending_submission ? 'unknown' : 'draft')
    )
  }
  get receipt() {
    return this.pending?.attempt.receipt ?? null
  }
  get canSubmit() {
    return !this.primary.input.pending_submission && !['submitting', 'unknown'].includes(this.state)
  }
  capture() {
    return clone(this.primary.input)
  }
  edit(update: Parameters<RecoveryCopy['edit']>[0]) {
    if (
      Object.entries(update).every(
        ([key, value]) =>
          JSON.stringify(this.primary.input[key as keyof RecoveryInput]) === JSON.stringify(value),
      )
    )
      return
    this.primary.edit(update)
  }
  async submit(
    snapshot: DeliverySnapshot,
    run: (
      snapshot: DeliverySnapshot,
      onReceipt: (receipt: ChatDeliveryReceipt) => void,
    ) => Promise<Result<unknown>>,
    options: {
      source?: RecoveryInput
      mode?: 'send' | 'retry' | 'queue' | 'steer'
      isCurrent?: () => boolean
      accepted?: (receipt: ChatDeliveryReceipt, cleared: boolean) => void
    } = {},
  ) {
    if (!this.canSubmit) {
      this.error = 'Reconcile the pending input before sending again.'
      this.changed()
      return false
    }
    this.error = ''
    const source = clone(options.source ?? this.primary.input),
      mode = options.mode ?? 'send'
    const bytes = (value: unknown) => new TextEncoder().encode(JSON.stringify(value)).byteLength
    if (
      this.retained.size >= 32 ||
      bytes(source) > 1024 * 1024 ||
      [...this.retained].reduce((sum, copy) => sum + bytes(copy.input), bytes(source)) >
        16 * 1024 * 1024
    ) {
      this.error =
        'Unconfirmed input copies reached the local capacity. Keep or export them before submitting another.'
      this.changed()
      return false
    }
    const original = new RecoveryCopy(
      this.transport,
      this.primary.workspace,
      this.primary.scope,
      this.primary.input.kind,
      this.primary.input.entity,
      this.primary.input.window_id,
      this.changed,
    )
    // Retain the click's input in memory before preflight. It has no backup
    // timer or pending host identity until pending-input reconciliation ends.
    Object.assign(original.input, {
      raw: mode === 'retry' ? snapshot.message : source.raw,
      selections: mode === 'retry' ? [] : clone(source.selections),
      selected_model: snapshot.options.model ?? source.selected_model,
    })
    const info: PendingInput = {
      id: '',
      original: original.input.id,
      session: snapshot.session,
      new_session: snapshot.newSession,
      clear_sequence: -1,
      content_hash: inputFingerprint(source),
      mode,
      turn: snapshot.live?.turn,
    }
    const attempt = new DeliveryAttempt(
      this.transport,
      snapshot,
      (receipt) => this.accept(receipt, info, original, options.accepted),
      () => {
        this.error = attempt.error || this.error
        this.changed()
      },
    )
    info.id = attempt.id
    this.pending = { attempt, info, original }
    this.retained.add(original)
    const accepted = await attempt.submit(run, async () => {
      if (options.isCurrent && !options.isCurrent()) return false
      const originalInfo = {
        ...info,
        mode: info.mode === 'retry' ? ('send' as const) : info.mode,
        clear_sequence: 1,
        content_hash: inputFingerprint(original.input),
      }
      original.edit({
        raw: original.input.raw,
        selections: original.input.selections,
        selected_model: original.input.selected_model,
        pending_submission: attempt.id,
        view: { pending_delivery: originalInfo },
      })
      if (!(await original.flush())) {
        this.error = original.error
        this.changed()
        return false
      }
      if (options.isCurrent && !options.isCurrent()) return false
      info.clear_sequence =
        mode !== 'retry' &&
        this.primary.input.edit_sequence === source.edit_sequence &&
        inputFingerprint(this.primary.input) === info.content_hash
          ? this.primary.input.edit_sequence + 1
          : -1
      this.primary.edit({
        pending_submission: attempt.id,
        view: viewWith(this.primary.input, info),
      })
      if (!(await this.primary.flush())) {
        this.error = this.primary.error
        this.changed()
        return false
      }
      return !options.isCurrent || options.isCurrent()
    })
    if (attempt.state === 'rejected' || attempt.state === 'unsupported') this.reject(info, original)
    if (accepted) await this.finalizePending({ info, original })
    this.error = attempt.error || this.error
    this.changed()
    return accepted
  }
  private accept(
    receipt: ChatDeliveryReceipt,
    info: PendingInput,
    original: RecoveryCopy | null,
    callback?: (receipt: ChatDeliveryReceipt, cleared: boolean) => void,
  ) {
    const matching = this.primary.input.pending_submission === info.id
    const exact =
      matching &&
      info.mode !== 'retry' &&
      this.primary.input.edit_sequence === info.clear_sequence &&
      inputFingerprint(this.primary.input) === info.content_hash
    const sourceVerified =
      receipt.source_available === true ||
      (receipt.source_available === undefined && original !== null)
    const cleared = exact && sourceVerified
    const keepReceivedCopy = exact && !sourceVerified
    if (matching && !keepReceivedCopy) {
      this.primary.edit({
        ...(cleared ? { raw: '', selections: [] } : {}),
        pending_submission: null,
        view: viewWith(this.primary.input, null),
      })
      void this.flushPrimary(cleared)
    }
    this.error = keepReceivedCopy
      ? 'This input was received but its source is unavailable. Keep or copy it before starting a new draft.'
      : ''
    callback?.(receipt, cleared)
    this.changed()
  }
  private reject(info: PendingInput, original: RecoveryCopy | null) {
    if (this.primary.input.pending_submission === info.id) {
      this.primary.edit({ pending_submission: null, view: viewWith(this.primary.input, null) })
      void this.primary.flush()
    }
    if (original) {
      original.edit({ pending_submission: null, view: { disposition: 'unreceived_input' } })
      void this.retainRejected(original)
    }
    // The independent original remains available in Recovery Copies. Never
    // replace newer text with the older rejected submission.
    this.changed()
  }
  async reconcile(callback?: (receipt: ChatDeliveryReceipt, cleared: boolean) => void) {
    if (!this.pending) {
      const info = pendingInput(this.primary.input)
      if (!info) {
        this.error = 'The pending identity needs its original recovery copy. Input was kept.'
        this.changed()
        return false
      }
      const attempt = new DeliveryAttempt(
        this.transport,
        {
          workspace: this.primary.workspace,
          session: info.session,
          newSession: info.new_session,
          message: '',
          history: [],
          options: {},
          ...((info.mode === 'queue' || info.mode === 'steer') && info.turn
            ? { live: { mode: info.mode, turn: info.turn } }
            : {}),
        },
        (receipt) => this.accept(receipt, info, null, callback),
        () => this.changed(),
        info.id,
      )
      this.pending = { attempt, info, original: null }
    }
    const current = this.pending
    const accepted = await current.attempt.reconcile()
    if (current.attempt.state === 'rejected') this.reject(current.info, current.original)
    if (accepted) await this.finalizePending(current)
    this.error = current.attempt.error || this.error
    this.changed()
    return accepted
  }
  /** Demo-only acknowledgment: it never claims host durability. */
  acceptLocal(source: RecoveryInput) {
    if (
      this.primary.input.pending_submission ||
      source.edit_sequence !== this.primary.input.edit_sequence ||
      inputFingerprint(source) !== inputFingerprint(this.primary.input)
    )
      return false
    this.primary.edit({ raw: '', selections: [] })
    return true
  }
  private async flushPrimary(clearEmpty: boolean) {
    const sequence = this.primary.input.edit_sequence
    if (
      (await this.primary.flush()) &&
      clearEmpty &&
      sequence === this.primary.input.edit_sequence &&
      !this.primary.input.raw &&
      !this.primary.input.selections.length &&
      !this.primary.input.pending_submission
    )
      await this.primary.discard()
  }
  private async cleanupOriginal(original: RecoveryCopy) {
    if (original.state === 'discarded') {
      this.retained.delete(original)
      return
    }
    if (await original.discard()) {
      this.retained.delete(original)
      original.dispose()
    }
  }
  /** A received input copy survives until the execution has a durable terminal
   * record and its source can still be read. A missing source keeps the copy. */
  private async finalizePending(current: { info: PendingInput; original: RecoveryCopy | null }) {
    try {
      const result = await this.transport.resourceRequest('chatDelivery', this.primary.workspace, {
        operation: 'read',
        id: current.info.id,
      })
      if (!result.ok) return
      const data = result.data as {
        receipt: ChatDeliveryReceipt | null
        source_available?: boolean
      }
      if (
        data.receipt?.id !== current.info.id ||
        !['finished', 'live_applied'].includes(data.receipt.phase) ||
        data.source_available !== true
      )
        return
      if (this.primary.input.raw || this.primary.input.selections.length) {
        if (!(await this.backup())) return
      }
      if (current.original) {
        if (this.retained.has(current.original)) await this.cleanupOriginal(current.original)
      } else await this.cleanupRecoveredOriginal(current.info)
    } catch {
      /* Retain the original on an uncertain terminal or cleanup. */
    }
  }
  async backup() {
    if (this.primary.hasPendingDiscard) await this.primary.discard()
    return this.primary.state === 'discarded' || this.primary.flush()
  }
  private async retainRejected(original: RecoveryCopy) {
    if (await original.flush()) {
      this.retained.delete(original)
      original.dispose()
    }
  }
  private async cleanupRecoveredOriginal(info: PendingInput) {
    try {
      const result = await this.transport.resourceRequest('recovery', this.primary.workspace, {
        scope: this.primary.scope,
        action: { operation: 'read', id: info.original },
      })
      if (!result.ok) return
      const record = (result.data as { record: RecoveryRecord }).record
      const pending = pendingInput(record.input)
      if (
        record.discarded ||
        record.input.edit_sequence !== 1 ||
        record.input.pending_submission !== info.id ||
        pending?.content_hash !== inputFingerprint(record.input)
      )
        return
      await this.transport.resourceRequest(
        'recovery',
        this.primary.workspace,
        {
          scope: this.primary.scope,
          action: { operation: 'discard', id: info.original, saved_sequence: 1 },
        },
        record.revision,
        crypto.randomUUID(),
      )
    } catch {
      /* Cleanup is optional; the original remains recoverable. */
    }
  }
  async reconnect() {
    for (const copy of this.retained)
      if (copy.hasPendingDiscard) await this.cleanupOriginal(copy)
      else if (!copy.input.pending_submission) await this.retainRejected(copy)
      else await copy.flush()
    if (this.pending?.attempt.state === 'accepted') await this.finalizePending(this.pending)
  }
  dispose() {
    for (const copy of this.retained) copy.dispose()
    this.retained.clear()
  }
  get safeToDispose() {
    return [...this.retained].every((copy) => copy.revision > 0)
  }
  get localCopies(): RecoveryRecord[] {
    return [...this.retained]
      .filter((copy) => copy.revision === 0 && (copy.input.raw || copy.input.selections.length))
      .map((copy) => ({
        schema_version: 1,
        revision: 0,
        input: clone(copy.input),
        updated_ms: 0,
        discarded: false,
      }))
  }
}
