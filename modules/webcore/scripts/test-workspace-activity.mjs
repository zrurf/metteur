import { test } from 'node:test'
import assert from 'node:assert/strict'
import { WorkspaceActivity } from '../src/core/workspace-activity.ts'

function fixture() {
  const connections = []
  const activity = new WorkspaceActivity((workspace, signal) => {
    const values = [],
      waiters = []
    let closed = false
    const connection = {
      workspace,
      signal,
      push(event) {
        if (closed) return
        const waiter = waiters.shift()
        if (waiter) waiter({ value: event, done: false })
        else values.push(event)
      },
    }
    const close = () => {
      closed = true
      for (const waiter of waiters.splice(0)) waiter({ done: true })
    }
    signal.addEventListener('abort', close, { once: true })
    connections.push(connection)
    return {
      [Symbol.asyncIterator]() {
        return this
      },
      next() {
        return values.length
          ? Promise.resolve({ value: values.shift(), done: false })
          : closed
            ? Promise.resolve({ done: true })
            : new Promise((resolve) => waiters.push(resolve))
      },
      return() {
        close()
        return Promise.resolve({ done: true })
      },
    }
  })
  return { activity, connections }
}
const tick = () => new Promise((resolve) => setTimeout(resolve, 0))
const event = (extra) => ({ activitySupported: true, sequence: 1n, ...extra })

test('file, business and job subscribers share one stream; a late business subscriber reconciles and last cancellation closes it', async () => {
  const f = fixture(),
    abort = Array.from({ length: 4 }, () => new AbortController()),
    received = [[], [], [], []]
  const pending = [0, 1, 2].map((i) =>
    f.activity.subscribe('a', (value) => received[i].push(value), abort[i].signal),
  )
  assert.equal(f.connections.length, 1)
  for (const value of [
    event({}),
    event({ fileEvent: { path: 'a.txt', kind: 'modified' } }),
    event({ jobEvent: { jobId: 'j', kind: 'finished' } }),
  ])
    f.connections[0].push(value)
  await tick()
  assert.deepEqual(received.map((values) => values.length).slice(0, 3), [3, 3, 3])
  pending.push(f.activity.subscribe('a', (value) => received[3].push(value), abort[3].signal))
  assert.equal(received[3].length, 1)
  assert.equal(received[3][0].fileEvent, undefined)
  abort[0].abort()
  await pending[0]
  assert.equal(f.connections[0].signal.aborted, false)
  for (const controller of abort.slice(1)) controller.abort()
  await Promise.all(pending)
  assert.equal(f.connections[0].signal.aborted, true)
})
test('old hosts fail explicitly before delivering unsupported activity, while workspaces remain isolated', async () => {
  const f = fixture(),
    a = new AbortController(),
    b = new AbortController(),
    received = []
  const old = f.activity.subscribe('a', (value) => received.push(value), a.signal)
  const other = f.activity.subscribe('b', (value) => received.push(value), b.signal)
  f.connections[0].push({ activitySupported: false, sequence: 0n })
  const failure = await old
  assert.equal(failure.ok, false)
  assert.match(failure.error, /Update the host/)
  assert.equal(f.connections[1].signal.aborted, false)
  assert.equal(received.length, 0)
  b.abort()
  await other
})
test('a cancelled generation cannot deliver late data into its replacement', async () => {
  const f = fixture(),
    a = new AbortController(),
    b = new AbortController(),
    received = []
  const first = f.activity.subscribe('a', () => assert.fail('cancelled subscriber'), a.signal)
  a.abort()
  await first
  const second = f.activity.subscribe('a', (value) => received.push(value), b.signal)
  f.connections[0].push(event({ sequence: 99n }))
  f.connections[1].push(event({ sequence: 2n }))
  await tick()
  assert.deepEqual(
    received.map((value) => value.sequence),
    [2n],
  )
  b.abort()
  await second
})
