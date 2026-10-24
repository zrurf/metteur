import { defineStore } from 'pinia'
import { reactive, watch } from 'vue'
import { gateway } from '@/core'
import {
  RecoveryCopy,
  type RecoveryKind,
  type RecoveryRecord,
  type BackupState,
} from '@/core/recovery'

export interface BackupStatus {
  state: BackupState
  error: string
  sequence: number
  acknowledged: number
  id: string
}
export interface RecoveryMetadata {
  id: string
  kind: RecoveryKind | null
  entity: string | null
  window_id: string | null
  revision: number | null
  edit_sequence: number | null
  updated_ms: number | null
  discarded: boolean
  error: string | null
}
type Scope = 'project' | 'personal'

/** Ephemeral per-window controllers; only the daemon keeps recovery content. */
export const useRecoveryStore = defineStore('recovery', () => {
  const windowId = crypto.randomUUID()
  const statuses = reactive(new Map<string, BackupStatus>())
  const copies = new Map<string, RecoveryCopy>()
  const newChats = reactive(new Map<string, string>())
  const errorMessage = (raw: string) => {
    try {
      return String(JSON.parse(raw).message ?? raw)
    } catch {
      return raw
    }
  }
  const key = (workspace: string, scope: Scope, kind: RecoveryKind, entity: string) =>
    JSON.stringify([workspace, scope, kind, entity])
  function scopeFor(entity: string): Scope {
    return entity.startsWith('@prompt/personal/') || entity === 'metteur://config/user.toml'
      ? 'personal'
      : 'project'
  }
  function copy(workspace: string, kind: RecoveryKind, entity: string, scope = scopeFor(entity)) {
    const identity = key(workspace, scope, kind, entity)
    let current = copies.get(identity)
    if (!current || current.state === 'discarded') {
      const publish = () => {
        const record = copies.get(identity)
        if (record)
          statuses.set(identity, {
            state: record.state,
            error: errorMessage(record.error),
            sequence: record.input.edit_sequence,
            acknowledged: record.acknowledgedSequence,
            id: record.input.id,
          })
      }
      current = new RecoveryCopy(gateway, workspace, scope, kind, entity, windowId, publish)
      copies.set(identity, current)
      publish()
    }
    return current
  }
  function status(workspace: string, kind: RecoveryKind, entity: string) {
    return statuses.get(key(workspace, scopeFor(entity), kind, entity))
  }
  function edit(
    workspace: string,
    kind: RecoveryKind,
    entity: string,
    update: Parameters<RecoveryCopy['edit']>[0],
  ) {
    // Inputs are JSON data. Unwrap nested Vue proxies without changing raw text.
    copy(workspace, kind, entity).edit(JSON.parse(JSON.stringify(update)))
  }
  async function flush(workspace: string, kind: RecoveryKind, entity: string) {
    const current = copies.get(key(workspace, scopeFor(entity), kind, entity))
    return current ? current.flush() : true
  }
  /** Detach an acknowledged original without discarding its host record.
   * Replacing a reviewed draft must not overwrite that copy on the next edit. */
  function retainConfirmed(workspace: string, kind: RecoveryKind, entity: string, raw: string) {
    const identity = key(workspace, scopeFor(entity), kind, entity),
      current = copies.get(identity)
    if (
      !current ||
      current.input.raw !== raw ||
      current.state !== 'backed_up' ||
      current.input.pending_submission ||
      current.acknowledgedSequence !== current.input.edit_sequence
    )
      return false
    current.dispose()
    copies.delete(identity)
    statuses.delete(identity)
    return true
  }
  async function saved(workspace: string, kind: RecoveryKind, entity: string, content: string) {
    const identity = key(workspace, scopeFor(entity), kind, entity),
      current = copies.get(identity)
    if (!current || current.input.raw !== content || current.input.pending_submission) return
    if (!(await current.flush()) || current.input.raw !== content) return
    if (await current.discard()) {
      copies.delete(identity)
      statuses.delete(identity)
    }
  }
  async function discard(workspace: string, kind: RecoveryKind, entity: string) {
    const identity = key(workspace, scopeFor(entity), kind, entity),
      current = copies.get(identity)
    if (!current) return true
    if (gateway.demo && current.state === 'unsupported') {
      current.dispose()
      copies.delete(identity)
      statuses.delete(identity)
      return true
    }
    if (current.state === 'unknown' && !current.hasPendingDiscard && !(await current.flush()))
      return false
    if (!(await current.discard())) return false
    copies.delete(identity)
    statuses.delete(identity)
    return true
  }
  function newChat(workspace: string) {
    if (!newChats.has(workspace)) newChats.set(workspace, crypto.randomUUID())
    return newChats.get(workspace)!
  }
  function rotateNewChat(workspace: string) {
    newChats.set(workspace, crypto.randomUUID())
  }
  async function list(workspace: string, scope: Scope, after: string | null = null) {
    return gateway.resourceRequest('recovery', workspace, {
      scope,
      action: { operation: 'list', after, limit: 50 },
    })
  }
  async function read(workspace: string, scope: Scope, id: string) {
    const result = await gateway.resourceRequest('recovery', workspace, {
      scope,
      action: { operation: 'read', id },
    })
    if (!result.ok) return result
    return {
      ok: true as const,
      data: result.data as {
        record: RecoveryRecord
        source_available: boolean
        requires_current_validation: true
      },
    }
  }
  function fork(workspace: string, kind: RecoveryKind, entity: string, record: RecoveryRecord) {
    const identity = key(workspace, scopeFor(entity), kind, entity)
    copies.get(identity)?.dispose()
    copies.delete(identity)
    statuses.delete(identity)
    return copy(workspace, kind, entity).loadAsNewCopy(record)
  }
  function renamed(workspace: string, from: string, to: string) {
    for (const [identity, current] of copies) {
      const entity = current.input.entity
      if (current.workspace !== workspace || (entity !== from && !entity.startsWith(from + '/')))
        continue
      // The host has transferred the old record. Fork current unsaved input
      // instead of guessing the new revision or overwriting another window.
      current.dispose()
      copies.delete(identity)
      statuses.delete(identity)
      const target = to + entity.slice(from.length)
      const next = copy(workspace, current.input.kind, target)
      next.edit({
        raw: current.input.raw,
        baseline_identity: current.input.baseline_identity,
        selections: current.input.selections,
        pending_submission: current.input.pending_submission,
        selected_model: current.input.selected_model,
        view: current.input.view,
      })
    }
  }
  watch(
    () => gateway.connected.value,
    (connected) => {
      if (connected) for (const current of copies.values()) void current.flush()
    },
  )
  return {
    windowId,
    statuses,
    copy,
    status,
    scopeFor,
    edit,
    flush,
    retainConfirmed,
    saved,
    discard,
    newChat,
    rotateNewChat,
    list,
    read,
    fork,
    renamed,
  }
})
