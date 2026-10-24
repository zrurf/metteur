import type { WorkspaceChange } from '../gen/metteur_pb'
type Result = { ok: true; data: undefined } | { ok: false; error: string }
type Listener = { receive: (event: WorkspaceChange) => void; finish: (result: Result) => void }
type Subscription = {
  abort: AbortController
  listeners: Set<Listener>
  latest: WorkspaceChange | null
}

/** Reuse the host event sources over one HTTP connection per workspace/window. */
export class WorkspaceActivity {
  private streams = new Map<string, Subscription>()
  constructor(
    private open: (workspace: string, signal: AbortSignal) => AsyncIterable<WorkspaceChange>,
    private status: (error: string) => void = () => {},
  ) {}

  subscribe(
    workspace: string,
    receive: (event: WorkspaceChange) => void,
    signal?: AbortSignal,
  ): Promise<Result> {
    if (signal?.aborted) return Promise.resolve({ ok: true, data: undefined })
    let record = this.streams.get(workspace)
    const start = !record
    if (!record) {
      record = { abort: new AbortController(), listeners: new Set(), latest: null }
      this.streams.set(workspace, record)
    }
    const current = record
    return new Promise((resolve) => {
      let closed = false
      const listener: Listener = {
        receive,
        finish: (result) => {
          if (closed) return
          closed = true
          signal?.removeEventListener('abort', cancel)
          current.listeners.delete(listener)
          if (!current.listeners.size) {
            if (this.streams.get(workspace) === current) this.streams.delete(workspace)
            current.abort.abort()
          }
          resolve(result)
        },
      }
      const cancel = () => listener.finish({ ok: true, data: undefined })
      current.listeners.add(listener)
      signal?.addEventListener('abort', cancel, { once: true })
      if (signal?.aborted) cancel()
      else if (current.latest) this.deliver(current.latest, listener)
      if (start) void this.run(workspace, current)
    })
  }
  private deliver(event: WorkspaceChange, listener: Listener) {
    try {
      listener.receive(event)
    } catch (error) {
      listener.finish({ ok: false, error: String(error) })
    }
  }
  private async run(workspace: string, record: Subscription) {
    let result: Result = { ok: true, data: undefined }
    try {
      for await (const event of this.open(workspace, record.abort.signal)) {
        if (record.abort.signal.aborted || this.streams.get(workspace) !== record) break
        if (!event.activitySupported)
          throw Error(
            'The host does not support shared workspace activity. Update the host to enable live file, task and Job updates.',
          )
        this.status('')
        if (!event.fileEvent && !event.jobEvent) record.latest = event
        for (const listener of [...record.listeners])
          if (record.listeners.has(listener)) this.deliver(event, listener)
      }
    } catch (error) {
      if (!record.abort.signal.aborted)
        result = { ok: false, error: error instanceof Error ? error.message : String(error) }
      if (!result.ok) this.status(result.error)
    } finally {
      if (this.streams.get(workspace) === record) this.streams.delete(workspace)
      for (const listener of [...record.listeners]) listener.finish(result)
    }
  }
}
