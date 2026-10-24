import { onScopeDispose, watch, ref, type WatchSource } from 'vue'
import { useWorkspaceEventsStore } from '@/stores/workspace-events'
import { useWorkspaceStore } from '@/stores/workspace'

/** Serialize snapshot reads and retain invalidations received during a read. */
export function useWorkspaceRefresh(
  refresh: () => unknown | Promise<unknown>,
  sources: WatchSource[] = [],
) {
  const events = useWorkspaceEventsStore(),
    workspace = useWorkspaceStore()
  const refreshError = ref('')
  let disposed = false,
    running = false,
    pending = false
  let timer: ReturnType<typeof setTimeout> | undefined
  let queue = Promise.resolve<unknown>(undefined)
  function run<T>(operation: () => T | Promise<T>): Promise<T | undefined> {
    const result = queue.then(() => (disposed ? undefined : operation()))
    queue = result.catch(() => {})
    return result
  }
  async function flush() {
    if (running || disposed) return
    running = true
    try {
      while (pending) {
        if (disposed) return
        pending = false
        try {
          await run(refresh)
          refreshError.value = ''
        } catch (error) {
          refreshError.value = String(error)
        }
      }
    } finally {
      running = false
    }
  }
  watch(
    [() => workspace.active?.path, () => events.revision, ...sources],
    () => {
      pending = true
      clearTimeout(timer)
      timer = setTimeout(flush, 60)
    },
    { immediate: true },
  )
  onScopeDispose(() => {
    disposed = true
    clearTimeout(timer)
  })
  return {
    run,
    get error() {
      return refreshError.value || events.error
    },
  }
}
