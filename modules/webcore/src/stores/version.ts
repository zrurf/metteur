import { defineStore } from 'pinia'
import { ref, watch } from 'vue'
import { gateway } from '@/core'
import type { FileHistoryEntry, SnapshotInfo } from '@/core'
import type { VersionRef } from '@/core/execution-view'
import { useWorkspaceStore } from './workspace'

/**
 * Version timeline.
 *
 * Lists snapshots of the active workspace, supports rollback to any of them,
 * and shows the file history (with diffs) for a chosen file.
 */
export const useVersionStore = defineStore('version', () => {
  const workspace = useWorkspaceStore()
  const snapshots = ref<SnapshotInfo[]>([])
  const history = ref<FileHistoryEntry[]>([])
  const selectedFile = ref<string | null>(null)
  const selectedId = ref<string | null>(null)
  const reference = ref<{ version: VersionRef; label: string; runId: string; proposalId?: string } | null>(null)
  const error = ref('')
  let generation = 0, historyRequest = 0, snapshotRequest = 0
  watch(() => workspace.active?.path, () => {
    generation++; historyRequest++; snapshotRequest++
    snapshots.value = []; history.value = []; selectedFile.value = null; selectedId.value = null; reference.value = null; error.value = ''
  })

  async function refresh() {
    const ws = workspace.active
    if (!ws) return
    const ticket = generation, request = ++snapshotRequest
    const r = await gateway.listSnapshots(ws.path)
    if (ticket !== generation || request !== snapshotRequest || ws.path !== workspace.active?.path) return
    if (r.ok) {
      snapshots.value = r.data
      error.value = ''
    } else { snapshots.value = []; error.value = r.error }
  }

  function select(id: string | null) {
    selectedId.value = id
    reference.value = null
  }

  function openReference(version: VersionRef, runId: string, label: string, proposalId?: string) {
    selectedId.value = version.snapshot_id
    reference.value = { version, runId, label, proposalId }
    void pickFile(version.blueprint_uri)
  }

  async function pickFile(path: string) {
    selectedFile.value = path
    history.value = []
    error.value = ''
    const ws = workspace.active
    if (!ws) return
    const ticket = generation, request = ++historyRequest
    const r = await gateway.listFileHistory(ws.path, path)
    if (ticket !== generation || request !== historyRequest || ws.path !== workspace.active?.path) return
    if (r.ok) history.value = r.data
    else error.value = r.error
  }

  async function rollback(snapshotId: string) {
    const ws = workspace.active
    if (!ws) return
    const r = await gateway.rollback(ws.path, snapshotId)
    if (r.ok) await refresh()
  }

  /** Create a snapshot of the workspace with an optional alias. */
  async function create(description: string, alias?: string): Promise<boolean> {
    const ws = workspace.active
    if (!ws) return false
    const r = await gateway.createSnapshot(ws.path, description || 'snapshot', alias)
    if (r.ok) await refresh()
    return r.ok
  }

  return { snapshots, history, selectedFile, selectedId, reference, error, refresh, select, openReference, pickFile, rollback, create }
})
