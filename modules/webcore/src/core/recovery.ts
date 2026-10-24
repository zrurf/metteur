import type { Result } from './types'
import type { DaemonGateway } from './gateway'

export type RecoveryKind = 'text' | 'canvas' | 'chat' | 'new_chat' | 'supervision' | 'view_state' | 'workflow' | 'workflow_review'
export interface RecoveryInput {
  creation_epoch?: string | null
  id: string
  window_id: string
  kind: RecoveryKind
  entity: string
  edit_sequence: number
  raw: string
  baseline_identity: string | null
  selections: unknown[]
  pending_submission: string | null
  selected_model: string | null
  view: unknown
}
export interface RecoveryRecord {
  schema_version: 1
  revision: number
  input: RecoveryInput
  updated_ms: number
  discarded: boolean
}
export type BackupState =
  | 'unbacked'
  | 'backing_up'
  | 'backed_up'
  | 'unknown'
  | 'conflict'
  | 'discarding'
  | 'discarded'
  | 'unsupported'
type Transport = Pick<DaemonGateway, 'resourceRequest'>
// Recovery payloads are JSON. This also unwraps nested Vue proxies while
// preserving raw strings exactly; structuredClone rejects reactive records.
const clone = <T>(value: T): T => JSON.parse(JSON.stringify(value))

/** One window owns one input copy. Recovery forks other copies explicitly;
 * it never inherits an execution permission, old approval or old window ID. */
export class RecoveryCopy {
  input: RecoveryInput
  revision = 0
  acknowledgedSequence = 0
  state: BackupState = 'unbacked'
  error = ''
  private inFlight: Promise<boolean> | null = null
  private discardInFlight: Promise<boolean> | null = null
  private pending: {
    request: string
    expected: number
    input: RecoveryInput
    uncertain: boolean
  } | null = null
  private debounce?: ReturnType<typeof setTimeout>
  private maximum?: ReturnType<typeof setTimeout>
  private stopped = false
  private discarding = false
  private pendingDiscard: {
    request: string
    expected: number
    sequence: number
    localSequence: number
  } | null = null
  get hasPendingDiscard() {
    return this.pendingDiscard !== null
  }

  constructor(
    private transport: Transport,
    readonly workspace: string,
    readonly scope: 'project' | 'personal',
    kind: RecoveryKind,
    entity: string,
    windowId: string,
    private changed: () => void = () => {},
  ) {
    this.input = {
      id: crypto.randomUUID(),
      window_id: windowId,
      kind,
      entity,
      edit_sequence: 0,
      raw: '',
      baseline_identity: null,
      selections: [],
      pending_submission: null,
      selected_model: null,
      view: null,
    }
  }
  edit(
    update: Partial<
      Pick<
        RecoveryInput,
        | 'raw'
        | 'baseline_identity'
        | 'selections'
        | 'pending_submission'
        | 'selected_model'
        | 'view'
      >
    >,
  ) {
    if (this.stopped) return
    Object.assign(this.input, clone(update))
    this.input.edit_sequence++
    this.state = this.discarding
      ? 'discarding'
      : this.pending || this.pendingDiscard
        ? 'unknown'
        : 'unbacked'
    this.changed()
    if (this.discarding || this.pendingDiscard) return
    clearTimeout(this.debounce)
    this.debounce = setTimeout(() => {
      void this.flush()
    }, 750)
    this.maximum ??= setTimeout(() => {
      void this.flush()
    }, 5000)
  }
  private clearTimers() {
    clearTimeout(this.debounce)
    clearTimeout(this.maximum)
    this.debounce = this.maximum = undefined
  }
  async flush(): Promise<boolean> {
    this.clearTimers()
    if (this.stopped || this.discarding) return false
    if (this.pendingDiscard) {
      this.state = 'unknown'
      this.error = 'Reconcile the pending discard before backing up this copy.'
      this.changed()
      return false
    }
    if (this.inFlight) return this.inFlight
    this.inFlight = this.backupLoop()
    try {
      return await this.inFlight
    } finally {
      this.inFlight = null
    }
  }
  private async backupLoop(): Promise<boolean> {
    let epochRefreshes = 0
    while (
      !this.stopped &&
      !this.discarding &&
      (this.pending || this.acknowledgedSequence < this.input.edit_sequence)
    ) {
      this.pending ??= {
        request: crypto.randomUUID(),
        expected: this.revision,
        input: clone(this.input),
        uncertain: false,
      }
      const pending = this.pending
      this.state = 'backing_up'
      this.changed()
      let result: Result<unknown>
      try {
        result = await this.transport.resourceRequest(
          'recovery',
          this.workspace,
          {
            scope: this.scope,
            action: { operation: 'backup', input: pending.input },
          },
          pending.expected,
          pending.request,
        )
      } catch (error) {
        result = { ok: false, error: String(error) }
      }
      if (!result.ok) {
        this.error = result.error
        let code = ''
        try {
          code = JSON.parse(result.error).code ?? ''
        } catch {
          /* Transport outcome remains unknown. */
        }
        if (
          code === 'creation_epoch_required' &&
          pending.expected === 0 &&
          !pending.uncertain &&
          epochRefreshes++ < 2
        ) {
          // Only a definitive first-write rejection permits a fresh identity.
          // Never turn a lost ACK into a new copy of possibly discarded input.
          try {
            const caps = await this.transport.resourceRequest('recovery', this.workspace, {
              scope: this.scope,
              action: { operation: 'capabilities' },
            })
            const epoch = caps.ok && (caps.data as { creation_epoch?: string }).creation_epoch
            if (epoch && /^[0-9a-f-]{36}$/i.test(epoch)) {
              this.input.id = crypto.randomUUID()
              this.input.creation_epoch = epoch
              this.pending = null
              continue
            }
          } catch {
            /* Keep the rejected snapshot and all local input. */
          }
        }
        if (!code) pending.uncertain = true
        this.state = /unimplemented|unsupported_schema/i.test(result.error)
          ? 'unsupported'
          : [
                'revision_conflict',
                'identity_conflict',
                'discarded',
                'stale_edit',
                'creation_epoch_required',
              ].includes(code)
            ? 'conflict'
            : code
              ? 'unbacked'
              : 'unknown'
        // Keep the exact request on unknown outcomes; never mint a replacement.
        // Structured rejection also stays inspectable until explicit resolution.
        this.changed()
        return false
      }
      const record = result.data as RecoveryRecord
      if (
        !record ||
        record.schema_version !== 1 ||
        record.input?.id !== this.input.id ||
        record.input.edit_sequence !== pending.input.edit_sequence ||
        record.discarded ||
        !Number.isSafeInteger(record.revision) ||
        record.revision <= this.revision
      ) {
        pending.uncertain = true
        this.state = 'unknown'
        this.error = 'Invalid recovery acknowledgment; reconcile this copy.'
        this.changed()
        return false
      }
      this.revision = record.revision
      this.acknowledgedSequence = pending.input.edit_sequence
      this.pending = null
      this.error = ''
      this.state = this.acknowledgedSequence === this.input.edit_sequence ? 'backed_up' : 'unbacked'
      this.changed()
    }
    return this.acknowledgedSequence === this.input.edit_sequence
  }
  /** A recovered record is input data only. Resource/model validation precedes use. */
  loadAsNewCopy(record: RecoveryRecord) {
    if (
      this.revision ||
      this.pending ||
      this.input.edit_sequence ||
      record.discarded ||
      record.schema_version !== 1
    )
      return false
    const { raw, baseline_identity, selections, pending_submission, selected_model, view } =
      record.input
    this.edit({ raw, baseline_identity, selections, pending_submission, selected_model, view })
    return true
  }
  async discard(): Promise<boolean> {
    if (this.discardInFlight) return this.discardInFlight
    this.discardInFlight = this.discardOnce()
    try {
      return await this.discardInFlight
    } finally {
      this.discardInFlight = null
    }
  }
  private async discardOnce(): Promise<boolean> {
    const localSequence = this.input.edit_sequence
    this.clearTimers()
    this.discarding = true
    if (this.inFlight) await this.inFlight
    // A lost ACK must be resolved with the original request before discarding.
    if (this.pending) {
      this.discarding = false
      this.error = 'Reconcile the pending backup before discarding.'
      this.changed()
      return false
    }
    if (this.revision) {
      this.state = 'discarding'
      this.changed()
      this.pendingDiscard ??= {
        request: crypto.randomUUID(),
        expected: this.revision,
        sequence: this.acknowledgedSequence,
        localSequence,
      }
      const pending = this.pendingDiscard
      let result: Result<unknown>
      try {
        result = await this.transport.resourceRequest(
          'recovery',
          this.workspace,
          {
            scope: this.scope,
            action: { operation: 'discard', id: this.input.id, saved_sequence: pending.sequence },
          },
          pending.expected,
          pending.request,
        )
      } catch (error) {
        result = { ok: false, error: String(error) }
      }
      if (!result.ok) {
        let retired = false
        try {
          retired = JSON.parse(result.error).code === 'retired'
        } catch {
          /* Unknown outcome. */
        }
        if (retired) {
          // The host only retires discarded records; active copies are retained.
          result = {
            ok: true,
            data: { discarded: true, input: { id: this.input.id }, revision: this.revision + 1 },
          }
        }
      }
      if (!result.ok) {
        this.discarding = false
        this.state = 'unknown'
        this.error = result.error
        this.changed()
        return false
      }
      const record = result.data as RecoveryRecord
      if (!record || !record.discarded || record.input?.id !== this.input.id) {
        this.discarding = false
        this.state = 'unknown'
        this.error = 'Discard outcome requires reconciliation.'
        this.changed()
        return false
      }
      this.revision = record.revision
      this.pendingDiscard = null
      if (this.input.edit_sequence !== pending.localSequence) {
        // The confirmation covered an older local version. Its tombstone must
        // not clear text typed while the discard receipt was in flight/lost.
        this.input.id = crypto.randomUUID()
        this.input.edit_sequence = 1
        this.revision = 0
        this.acknowledgedSequence = 0
        this.discarding = false
        this.state = 'unbacked'
        this.error = 'Newer edits were kept in a separate recovery copy.'
        this.changed()
        void this.flush()
        return false
      }
    }
    this.stopped = true
    this.state = 'discarded'
    this.input.raw = ''
    this.input.selections = []
    this.input.pending_submission = null
    this.changed()
    return true
  }
  /** Disposal is not discard. Host-confirmed data remains available. */
  dispose() {
    this.stopped = true
    this.clearTimers()
  }
}
