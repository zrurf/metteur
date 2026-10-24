import { defineStore } from 'pinia'
import { reactive } from 'vue'
import { gateway } from '@/core'
import { ApprovalDecision, type ApprovalSnapshot } from '@/core/approval-decision'

export const useApprovalDecisionsStore = defineStore('approval-decisions', () => {
  const entries = new Map<string, ApprovalDecision>(),
    versions = reactive(new Map<string, number>())
  const key = (workspace: string, id: string) => JSON.stringify([workspace, id])
  function entry(snapshot: ApprovalSnapshot) {
    const identity = key(snapshot.workspace, snapshot.id)
    let current = entries.get(identity)
    if (current && JSON.stringify(current.snapshot) !== JSON.stringify(snapshot))
      throw Error(
        'Approval identity was reused with different target or content. The original receipt was kept.',
      )
    if (!current) {
      if (entries.size >= 512) {
        for (const [id, item] of entries)
          if (item.state === 'accepted' || item.state === 'retired') {
            entries.delete(id)
            versions.delete(id)
            break
          }
        if (entries.size >= 512)
          throw Error('Unresolved approval receipt limit reached. Existing identities were kept.')
      }
      current = new ApprovalDecision(gateway, snapshot, () =>
        versions.set(identity, (versions.get(identity) ?? 0) + 1),
      )
      entries.set(identity, current)
    }
    return current
  }
  function read(workspace: string, id: string) {
    const identity = key(workspace, id)
    versions.get(identity)
    const current = entries.get(identity)
    return current
      ? {
          state: current.state,
          busy: current.busy,
          canRespond: current.canRespond,
          error: current.error,
        }
      : null
  }
  return { entry, read }
})
