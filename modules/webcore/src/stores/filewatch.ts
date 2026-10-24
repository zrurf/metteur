import { defineStore } from 'pinia'
import { ref } from 'vue'
import { gateway } from '@/core'
import type { WatchEvent } from '@/core'
import { useWorkspaceStore } from '@/stores/workspace'
import { useDocumentsStore } from './documents'
import { followStream } from '@/lib/follow-stream'

/**
 * Subscribes to the daemon's live file-change stream for the active workspace
 * and fans events out to surfaces (explorer refresh, editor disk sync).
 */
export const useFileWatchStore = defineStore('filewatch', () => {
  const workspaceStore = useWorkspaceStore()
  const documents = useDocumentsStore()
  /** Latest invalidation batch; old events must not be replayed on every update. */
  const events = ref<WatchEvent[]>([])
  const active = ref(false)
  let abort: AbortController | null = null

  /** Subscribe to the active workspace's fs events (idempotent). */
  function start() {
    const ws = workspaceStore.active
    if (!ws || active.value) return
    active.value = true
    abort = new AbortController()
    const signal = abort.signal
    void followStream<WatchEvent>(
      (receive, subscriptionSignal) => {
        // Rebuild after reconnect: changes during disconnection cannot be replayed.
        events.value = [{ path: '', kind: 'modified' }]
        for (const doc of documents.records.values())
          if (doc.workspace === ws.path) void documents.sync(ws.path, doc.path)
        return gateway.watchWorkspace(ws.path, receive, subscriptionSignal)
      },
      (e) => {
        if (signal.aborted || workspaceStore.active?.path !== ws.path) return
        const event = { ...e, path: e.path.replaceAll('\\', '/') }
        events.value = [event]
        if (event.kind === 'removed') {
          documents.markDeleted(ws.path, event.path)
          for (const doc of documents.records.values())
            if (
              doc.workspace === ws.path &&
              (doc.path === event.path || doc.path.startsWith(event.path + '/'))
            )
              void documents.sync(ws.path, doc.path)
        } else if (documents.peek(ws.path, event.path)) void documents.sync(ws.path, event.path)
      },
      signal,
    )
  }

  /** Unsubscribe; the in-flight stream is cancelled via the abort signal. */
  function stop() {
    active.value = false
    abort?.abort()
    abort = null
    events.value = []
  }

  return { events, active, start, stop }
})
