import { defineStore } from 'pinia'
import { reactive } from 'vue'
import { gateway } from '@/core'
import { SupervisionInput } from '@/core/supervision-input'
import type { RecoveryRecord } from '@/core/recovery'
import { useRecoveryStore } from './recovery'

export const useSupervisionInputsStore = defineStore('supervision-inputs', () => {
  const recovery = useRecoveryStore(),
    versions = reactive(new Map<string, number>())
  const entries = new Map<string, SupervisionInput>(),
    retained = new Set<SupervisionInput>()
  const key = (workspace: string, run: string) => JSON.stringify([workspace, run])
  function controller(workspace: string, run: string) {
    const identity = key(workspace, run),
      primary = recovery.copy(workspace, 'supervision', run)
    let current = entries.get(identity)
    if (current && current.primary !== primary) {
      retained.add(current)
      entries.delete(identity)
      current = undefined
    }
    if (!current) {
      current = new SupervisionInput(gateway, primary, () =>
        versions.set(identity, (versions.get(identity) ?? 0) + 1),
      )
      entries.set(identity, current)
    }
    return current
  }
  function read(workspace: string, run: string) {
    const entry = controller(workspace, run)
    versions.get(key(workspace, run))
    return {
      controller: entry,
      input: entry.primary.input,
      state: entry.state,
      error: entry.error,
      canSubmit: entry.canSubmit,
      backup: recovery.status(workspace, 'supervision', run),
    }
  }
  function localCopies(workspace: string) {
    const copies: RecoveryRecord[] = []
    for (const entry of new Set([...entries.values(), ...retained])) {
      if (entry.primary.workspace !== workspace) continue
      copies.push(...entry.localCopies)
      const primary = entry.primary
      if (primary.input.raw && primary.acknowledgedSequence < primary.input.edit_sequence)
        copies.push({
          schema_version: 1,
          revision: 0,
          input: JSON.parse(JSON.stringify(primary.input)),
          updated_ms: 0,
          discarded: false,
        })
    }
    return copies
  }
  async function restore(workspace: string, run: string, record: RecoveryRecord) {
    const entry = controller(workspace, run)
    if (
      record.input.kind !== 'supervision' ||
      record.input.entity !== run ||
      ['submitting', 'unknown'].includes(entry.state)
    )
      return false
    const sequence = entry.primary.input.edit_sequence
    if (!(await entry.backup()) || sequence !== entry.primary.input.edit_sequence) return false
    const ok = recovery.fork(workspace, 'supervision', run, SupervisionInput.restored(record))
    controller(workspace, run)
    return ok
  }
  return {
    read,
    localCopies,
    restore,
    get hasUnbacked() {
      return [...entries.values(), ...retained].some(
        (entry) =>
          !entry.safeToDispose ||
          (entry.primary.input.raw &&
            entry.primary.acknowledgedSequence < entry.primary.input.edit_sequence),
      )
    },
  }
})
