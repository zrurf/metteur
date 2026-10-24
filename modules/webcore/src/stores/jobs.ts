import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { gateway } from '@/core'
import type { JobInfo, JobNotice } from '@/core'
import { useWorkspaceStore } from '@/stores/workspace'
import { followStream } from '@/lib/follow-stream'

/** Bytes of live output kept per job in the panel. */
const MAX_OUTPUT_CHARS = 200_000

/** One job as the panel shows it: the snapshot plus its streamed output. */
export interface JobEntry extends JobInfo {
  /** Output received while subscribed (the snapshot only carries a tail). */
  output: string
  /** Set while more output can still arrive. */
  live: boolean
}

/**
 * Background commands of the active workspace.
 *
 * The daemon broadcasts job notices per workspace, so the panel stays live
 * without polling: `start()` lists what already ran and then follows the
 * stream. Jobs do not survive a daemon restart, which is why the list is
 * rebuilt from `listJobs` on every subscription.
 */
export const useJobsStore = defineStore('jobs', () => {
  const workspaceStore = useWorkspaceStore()
  const entries = ref<JobEntry[]>([])
  const active = ref(false)
  /** Job whose output the panel shows. */
  const selectedId = ref<string>('')
  let abort: AbortController | null = null

  /** Running jobs, newest first. */
  const running = computed(() => entries.value.filter((job) => job.live).length)

  /** The selected job, falling back to the newest one. */
  const selected = computed<JobEntry | null>(() => {
    const found = entries.value.find((job) => job.id === selectedId.value)
    if (found) return found
    const newest = [...entries.value].sort((a, b) => b.startedAt - a.startedAt)
    return newest[0] ?? null
  })

  /** Subscribes to the active workspace's jobs (idempotent). */
  async function start() {
    const ws = workspaceStore.active
    if (!ws || active.value) return
    active.value = true
    abort = new AbortController()
    const signal = abort.signal
    void followStream<JobNotice>(
      async (receive, subscriptionSignal) => {
        const listed = await gateway.listJobs(ws.path)
        if (subscriptionSignal.aborted || workspaceStore.active?.path !== ws.path)
          return { ok: true, data: undefined }
        if (listed.ok) {
          entries.value = listed.data.map((job) => ({
            ...job,
            output: job.tail,
            live: job.state === 'running',
          }))
        }
        return gateway.watchJobs(ws.path, receive, subscriptionSignal)
      },
      (notice) => {
        if (signal.aborted || workspaceStore.active?.path !== ws.path) return
        apply(notice)
      },
      signal,
    )
  }

  /** Unsubscribes; the in-flight stream is cancelled via the abort signal. */
  function stop() {
    active.value = false
    abort?.abort()
    abort = null
    entries.value = []
    selectedId.value = ''
  }

  /** Folds one notice into the list. */
  function apply(notice: JobNotice) {
    const index = entries.value.findIndex((job) => job.id === notice.jobId)
    if (index < 0) {
      // A job announced after the initial list (started by a run just now).
      entries.value = [
        ...entries.value,
        {
          id: notice.jobId,
          command: '',
          cwd: '',
          state: notice.state,
          exitCode: notice.exitCode,
          runId: '',
          startedAt: Date.now(),
          finishedAt: 0,
          outputBytes: 0,
          tail: '',
          output: notice.chunk,
          live: notice.state === 'running',
        },
      ]
      return
    }
    const job = entries.value[index]
    const output = notice.kind === 'output' ? trim(`${job.output}${notice.chunk}`) : job.output
    entries.value[index] = {
      ...job,
      output,
      state: notice.state,
      exitCode: notice.exitCode,
      // A lifecycle notice settles the job; output notices keep it live.
      live: notice.kind === 'finished' ? false : notice.state === 'running',
      finishedAt: notice.kind === 'finished' ? Date.now() : job.finishedAt,
      outputBytes: job.outputBytes + notice.chunk.length,
    }
  }

  /** Keeps the retained output bounded on very chatty commands. */
  function trim(text: string): string {
    return text.length > MAX_OUTPUT_CHARS ? text.slice(text.length - MAX_OUTPUT_CHARS) : text
  }

  function select(id: string) {
    selectedId.value = id
  }

  /** Terminates a running command; the stream reports the final state. */
  async function kill(id: string): Promise<boolean> {
    const ws = workspaceStore.active
    if (!ws) return false
    const result = await gateway.killJob(ws.path, id)
    if (!result.ok) return false
    const index = entries.value.findIndex((job) => job.id === id)
    if (index >= 0 && result.data.killed) {
      entries.value[index] = { ...entries.value[index], live: false }
    }
    return result.data.killed
  }

  return { entries, active, running, selected, select, start, stop, kill }
})
