import { defineStore } from 'pinia'
import { ref, watch, onScopeDispose } from 'vue'
import { gateway } from '@/core'
import { useWorkspaceStore } from './workspace'
import { followStream } from '@/lib/follow-stream'

/** One coalescing runtime/resource stream per window, shared by all views. */
export const useWorkspaceEventsStore = defineStore('workspace-events', () => {
  const workspace = useWorkspaceStore()
  const revision = ref(0),
    connected = ref(false),
    error = ref('')
  let controller: AbortController | null = null
  let epoch = 0
  function stop() {
    epoch++
    controller?.abort()
    controller = null
    connected.value = false
    error.value = ''
  }
  function start() {
    stop()
    const path = workspace.active?.path,
      ticket = epoch
    if (!path || !gateway.connected.value) return
    const abort = new AbortController()
    controller = abort
    void followStream<void>(
      (receive, signal) => gateway.watchWorkspaceChanges(path, receive, signal),
      () => {
        if (ticket !== epoch) return
        connected.value = true
        error.value = ''
        revision.value++
      },
      abort.signal,
      (reason) => {
        if (ticket !== epoch || abort.signal.aborted) return
        connected.value = false
        error.value = reason
      },
    )
  }
  watch([() => workspace.active?.path, () => gateway.connected.value], start, { immediate: true })
  onScopeDispose(stop)
  return { revision, connected, error }
})
