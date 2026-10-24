import { defineStore } from 'pinia'
import { reactive, watch, onScopeDispose } from 'vue'
import { gateway } from '@/core'
import { ComposerInput, restoredInput } from '@/core/composer-input'
import type { DeliverySnapshot } from '@/core/delivery'
import type { RecoveryInput, RecoveryRecord } from '@/core/recovery'
import type { ChatDeliveryReceipt, FileTreeNode, Result } from '@/core/types'
import type { PromptSnapshot } from '@/core/resources'
import { useRecoveryStore } from './recovery'
import { validResultReference, type ResultChoice } from '@/core/workflow'

export type ComposerSelection =
  | { kind: 'result'; result: ResultChoice }
  | { kind: 'file'; file: FileTreeNode }
  | {
      kind: 'prompt'
      selection: { snapshot: PromptSnapshot; status: 'current' | 'changed' | 'missing' }
    }
export const useComposerInputsStore = defineStore('composer-inputs', () => {
  const recovery = useRecoveryStore(),
    versions = reactive(new Map<string, number>())
  const entries = new Map<string, ComposerInput>(),
    retired = new Set<ComposerInput>(),
    jobs = new Map<ComposerInput, number>()
  const key = (workspace: string, entity: string) => JSON.stringify([workspace, entity])
  function collect() {
    for (const entry of retired)
      if (!jobs.has(entry) && entry.safeToDispose) {
        entry.dispose()
        retired.delete(entry)
      }
  }
  function controller(workspace: string, entity: string) {
    const identity = key(workspace, entity),
      kind =
        entries.get(identity)?.primary.input.kind ??
        (entity === recovery.newChat(workspace) ? 'new_chat' : 'chat'),
      primary = recovery.copy(workspace, kind, entity)
    let current = entries.get(identity)
    if (current && current.primary !== primary) {
      retired.add(current)
      entries.delete(identity)
      current = undefined
      collect()
    }
    if (!current) {
      const entry = new ComposerInput(gateway, primary, () => {
        if (entries.get(identity) === entry)
          versions.set(identity, (versions.get(identity) ?? 0) + 1)
      })
      entries.set(identity, entry)
      current = entry
    }
    return current
  }
  function read(workspace: string, entity: string) {
    const entry = controller(workspace, entity)
    // Reactive projections wrap plain controllers, keeping Vue proxies out of
    // the immutable asynchronous input snapshots.
    versions.get(key(workspace, entity))
    return {
      controller: entry,
      input: entry.primary.input,
      backup: recovery.status(workspace, entry.primary.input.kind, entity),
      state: entry.state,
      error: entry.error,
      receipt: entry.receipt,
      canSubmit: entry.canSubmit,
    }
  }
  function edit(workspace: string, entity: string, update: Parameters<ComposerInput['edit']>[0]) {
    controller(workspace, entity).edit(update)
  }
  async function runJob<T>(entry: ComposerInput, run: () => Promise<T>) {
    jobs.set(entry, (jobs.get(entry) ?? 0) + 1)
    try {
      return await run()
    } finally {
      const count = (jobs.get(entry) ?? 1) - 1
      if (count) jobs.set(entry, count)
      else jobs.delete(entry)
      collect()
    }
  }
  async function submit(
    workspace: string,
    entity: string,
    snapshot: DeliverySnapshot,
    execute: (
      snapshot: DeliverySnapshot,
      onReceipt: (receipt: ChatDeliveryReceipt) => void,
    ) => Promise<Result<unknown>>,
    options: Parameters<ComposerInput['submit']>[2],
  ) {
    const entry = controller(workspace, entity)
    return runJob(entry, () => entry.submit(snapshot, execute, options))
  }
  async function reconcile(
    workspace: string,
    entity: string,
    accepted?: (receipt: ChatDeliveryReceipt, cleared: boolean) => void,
  ) {
    const entry = controller(workspace, entity)
    return runJob(entry, () => entry.reconcile(accepted))
  }
  async function backup(workspace: string, entity: string) {
    return controller(workspace, entity).backup()
  }
  async function discard(workspace: string, entity: string, expectedSequence: number) {
    const entry = controller(workspace, entity)
    if (
      entry.primary.input.edit_sequence !== expectedSequence ||
      ['submitting', 'unknown'].includes(entry.state)
    )
      return false
    return recovery.discard(workspace, entry.primary.input.kind, entity)
  }
  async function restore(workspace: string, entity: string, record: RecoveryRecord) {
    if (!['chat', 'new_chat'].includes(record.input.kind)) return false
    const entry = controller(workspace, entity),
      sequence = entry.primary.input.edit_sequence
    if (['submitting', 'unknown'].includes(entry.state)) return false
    if (
      (entry.primary.input.raw || entry.primary.input.selections.length) &&
      !(await entry.backup())
    )
      return false
    if (entry.primary.input.edit_sequence !== sequence) return false
    const ok = recovery.fork(workspace, entry.primary.input.kind, entity, restoredInput(record))
    controller(workspace, entity)
    return ok
  }
  function selections(input: RecoveryInput): ComposerSelection[] {
    return input.selections
      .filter((value): value is ComposerSelection => {
        if (!value || typeof value !== 'object') return false
        const entry = value as Partial<ComposerSelection>
        if (entry.kind === 'result')
          return (
            'result' in entry &&
            typeof entry.result?.label === 'string' &&
            validResultReference(entry.result?.reference)
          )
        if (entry.kind === 'file')
          return (
            'file' in entry && typeof entry.file?.path === 'string' && entry.file.kind === 'file'
          )
        if (entry.kind !== 'prompt' || !('selection' in entry)) return false
        const snapshot = entry.selection?.snapshot,
          reference = snapshot?.reference
        return (
          typeof snapshot?.id === 'string' &&
          typeof snapshot.body === 'string' &&
          typeof reference?.id === 'string' &&
          typeof reference.content_digest === 'string' &&
          ['project', 'personal'].includes(reference.scope?.scope) &&
          ['current', 'changed', 'missing'].includes(entry.selection?.status ?? '')
        )
      })
      .map((value) => JSON.parse(JSON.stringify(value)))
  }
  function currentSelections(workspace: string, entity: string) {
    return selections(controller(workspace, entity).primary.input)
  }
  function localCopies(workspace: string): RecoveryRecord[] {
    const result = new Map<string, RecoveryRecord>()
    for (const entry of new Set([...entries.values(), ...retired])) {
      if (entry.primary.workspace !== workspace) continue
      const primary = entry.primary
      if (
        primary.acknowledgedSequence < primary.input.edit_sequence &&
        (primary.input.raw || primary.input.selections.length)
      )
        result.set(primary.input.id, {
          schema_version: 1,
          revision: primary.revision,
          input: JSON.parse(JSON.stringify(primary.input)),
          updated_ms: 0,
          discarded: false,
        })
      for (const record of entry.localCopies) result.set(record.input.id, record)
    }
    return [...result.values()]
  }
  watch(
    () => gateway.connected.value,
    (connected) => {
      if (connected)
        void (async () => {
          for (const entry of new Set([...entries.values(), ...retired])) await entry.reconnect()
          collect()
        })()
    },
  )
  onScopeDispose(() => {
    for (const entry of new Set([...entries.values(), ...retired])) entry.dispose()
  })
  return {
    read,
    edit,
    submit,
    reconcile,
    backup,
    discard,
    restore,
    selections,
    currentSelections,
    localCopies,
    hasUnbackedInput: () =>
      [...new Set([...entries.values(), ...retired])].some(
        (entry) =>
          entry.localCopies.length ||
          (entry.primary.acknowledgedSequence < entry.primary.input.edit_sequence &&
            (entry.primary.input.raw || entry.primary.input.selections.length)),
      ),
    entity: (workspace: string, session: string) => session || recovery.newChat(workspace),
    rotateNewChat: recovery.rotateNewChat,
  }
})
