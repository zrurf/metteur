import type { Result } from '@/core/types'

/** Reconnect a push subscription after transport failure, without polling snapshots. */
export async function followStream<T>(
  subscribe: (receive: (value: T) => void, signal: AbortSignal) => Promise<Result<void>>,
  receive: (value: T) => void,
  signal: AbortSignal,
  disconnected: (error: string) => void = () => {},
) {
  let failures = 0
  while (!signal.aborted) {
    try {
      const result = await subscribe((value) => {
        if (signal.aborted) return
        failures = 0
        receive(value)
      }, signal)
      if (signal.aborted) return
      disconnected(result.ok ? 'Connection ended. Reconnecting…' : result.error)
    } catch (error) {
      if (signal.aborted) return
      disconnected(String(error))
    }
    await new Promise<void>((resolve) => {
      const done = () => {
        clearTimeout(timer)
        signal.removeEventListener('abort', done)
        resolve()
      }
      const timer = setTimeout(done, Math.min(30000, 500 * 2 ** Math.min(failures++, 6)))
      signal.addEventListener('abort', done, { once: true })
      if (signal.aborted) done()
    })
  }
}
