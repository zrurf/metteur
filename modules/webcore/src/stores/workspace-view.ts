import { defineStore } from 'pinia'
import { ref, watch } from 'vue'
import { gateway } from '@/core'
import { WorkspaceViewCopy, type DocumentPosition, type WorkspaceView } from '@/core/workspace-view'
import { useRecoveryStore } from './recovery'

const sourceKey = (workspace: string) => `metteur.view-source:${workspace}`
// Only an opaque host copy ID is retained in the browser, never input content.
function source(workspace: string) {
  try {
    return sessionStorage.getItem(sourceKey(workspace))
  } catch {
    return null
  }
}
export const useWorkspaceViewStore = defineStore('workspace-view', () => {
  const recovery = useRecoveryStore(),
    version = ref(0)
  const copies = new Map<string, WorkspaceViewCopy>()
  function forWorkspace(workspace: string) {
    let current = copies.get(workspace)
    if (!current) {
      current = new WorkspaceViewCopy(
        gateway,
        workspace,
        recovery.windowId,
        source(workspace),
        () => version.value++,
        (id) => {
          try {
            sessionStorage.setItem(sourceKey(workspace), id)
          } catch {
            /* Optional opaque pointer only. */
          }
        },
      )
      copies.set(workspace, current)
    }
    return current
  }
  async function ready(workspace: string) {
    const current = forWorkspace(workspace)
    await current.ready
    return current.value
  }
  function view(workspace: string) {
    void version.value
    return forWorkspace(workspace).value
  }
  function edit(workspace: string, update: Partial<Omit<WorkspaceView, 'schema' | 'positions'>>) {
    if (workspace) forWorkspace(workspace).edit(update)
  }
  function position(workspace: string, path: string, value: DocumentPosition) {
    if (workspace) forWorkspace(workspace).position(path, value)
  }
  watch(
    () => gateway.connected.value,
    (connected) => {
      if (connected) for (const copy of copies.values()) void copy.flush()
    },
  )
  return { version, forWorkspace, ready, view, edit, position }
})
