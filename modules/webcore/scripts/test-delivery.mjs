import { test } from 'node:test'
import assert from 'node:assert/strict'
import { DeliveryAttempt } from '../src/core/delivery.ts'
const snapshot = () => ({
  workspace: 'workspace-a',
  session: '',
  newSession: crypto.randomUUID(),
  message: 'original\r\n中文',
  history: [],
  options: { model: 'current-model' },
})
function accepted(attempt, phase = 'started') {
  return {
    schema_version: 1,
    id: attempt.id,
    input_digest: 'fingerprint',
    session: attempt.snapshot.newSession,
    turn: crypto.randomUUID(),
    checkpoint: 'checkpoint',
    accepted_revision: 1,
    phase,
    outcome: null,
    accepted_ms: 1,
    updated_ms: 1,
  }
}
function transport(handler) {
  return {
    resourceRequest: async (_method, ws, payload) =>
      payload.operation === 'capabilities'
        ? { ok: true, data: { durable_manual_send: true, pending_reconciliation: true } }
        : payload.operation === 'reconcile_pending'
          ? { ok: true, data: { items: [], raw_copies_unchanged: true } }
          : handler(ws, payload),
  }
}

function liveSnapshot() {
  return {
    ...snapshot(),
    session: crypto.randomUUID(),
    live: { mode: 'queue', turn: crypto.randomUUID() },
  }
}

test('saved result context cannot downgrade to an old host', async () => {
  const fixed = snapshot()
  fixed.options.result_references = [
    { kind: 'run', runId: crypto.randomUUID(), digest: 'a'.repeat(64) },
  ]
  let sends = 0
  const attempt = new DeliveryAttempt(
    transport(() => {
      throw Error('unexpected query')
    }),
    fixed,
  )
  assert.equal(
    await attempt.submit(async () => {
      sends++
      return { ok: true, data: undefined }
    }),
    false,
  )
  assert.equal(sends, 0)
})

test('saved file context cannot silently downgrade on an older host', async () => {
  const fixed = snapshot()
  fixed.options.file_references = [
    { snapshotId: crypto.randomUUID(), path: 'a.txt', digest: 'a'.repeat(64) },
  ]
  let sends = 0
  const attempt = new DeliveryAttempt(
    transport(() => {
      throw Error('unexpected receipt query')
    }),
    fixed,
  )
  assert.equal(
    await attempt.submit(async () => {
      sends++
      return { ok: true, data: undefined }
    }),
    false,
  )
  assert.equal(sends, 0)
  assert.match(attempt.error, /does not provide.*receipts/i)
})
function liveReceipt(attempt, phase = 'live_queued') {
  return {
    ...accepted(attempt),
    session: attempt.snapshot.session,
    turn: attempt.snapshot.live.turn,
    accepted_revision: 0,
    phase,
    live: { mode: attempt.snapshot.live.mode, body_digest: 'a'.repeat(64) },
  }
}
function liveTransport(handler) {
  return {
    resourceRequest: async (_method, ws, payload) =>
      payload.operation === 'capabilities'
        ? {
            ok: true,
            data: {
              durable_manual_send: true,
              pending_reconciliation: true,
              durable_queue: true,
              durable_queue_channel: 'chat',
            },
          }
        : payload.operation === 'reconcile_pending'
          ? { ok: true, data: { items: [] } }
          : handler(ws, payload),
  }
}
test('live receipt binds mode and root turn, separates host reception from model use', async () => {
  for (const phase of [
    'live_queued',
    'live_claimed',
    'live_applied',
    'live_unconsumed',
    'live_unknown',
  ]) {
    let count = 0
    const attempt = new DeliveryAttempt(
      liveTransport(() => {
        throw Error('unexpected query')
      }),
      liveSnapshot(),
      () => count++,
    )
    assert(
      await attempt.submit(async (_fixed, onReceipt) => {
        onReceipt(liveReceipt(attempt, phase))
        return { ok: false, error: 'provider stopped' }
      }),
    )
    assert.equal(count, 1)
    assert.equal(attempt.receipt.phase, phase)
    assert.equal(
      await attempt.submit(async () => {
        throw Error('no replay')
      }),
      false,
    )
  }
})
test('live acknowledgement loss only reads its original ID', async () => {
  let runs = 0
  const calls = []
  const attempt = new DeliveryAttempt(
    liveTransport((_ws, payload) => {
      calls.push(payload)
      return { ok: true, data: { receipt: liveReceipt(attempt), source_available: true } }
    }),
    liveSnapshot(),
  )
  assert(
    await attempt.submit(async () => {
      runs++
      throw Error('lost ack')
    }),
  )
  assert.equal(runs, 1)
  assert.equal(calls.length, 1)
  assert.equal(calls[0].id, attempt.id)
})
test('manual and live receipts cannot be exchanged; wrong turn or mode keeps input unknown', () => {
  const manual = new DeliveryAttempt(
    transport(() => {}),
    snapshot(),
  )
  assert.equal(manual.acknowledge({ ...accepted(manual), phase: 'live_queued' }), false)
  const attempt = new DeliveryAttempt(
    liveTransport(() => {}),
    liveSnapshot(),
  )
  assert.equal(attempt.acknowledge({ ...liveReceipt(attempt), turn: crypto.randomUUID() }), false)
  assert.equal(
    attempt.acknowledge({
      ...liveReceipt(attempt),
      live: { mode: 'steer', body_digest: 'a'.repeat(64) },
    }),
    false,
  )
  assert.equal(attempt.acknowledge({ ...liveReceipt(attempt), phase: 'started' }), false)
  assert.equal(attempt.state, 'unknown')
})
test('old hosts cannot downgrade reliable chat input to legacy Interrupt', async () => {
  let invoked = 0
  const attempt = new DeliveryAttempt(
    transport(() => {}),
    liveSnapshot(),
  )
  assert.equal(
    await attempt.submit(async () => {
      invoked++
      return { ok: true }
    }),
    false,
  )
  assert.equal(attempt.state, 'unsupported')
  assert.equal(invoked, 0)
})

test('a model failure after durable acceptance does not make the same input unsent', async () => {
  let cleared = 0,
    queries = 0
  const attempt = new DeliveryAttempt(
    transport(() => {
      queries++
      throw Error('unexpected')
    }),
    snapshot(),
    () => cleared++,
  )
  assert.equal(
    await attempt.submit(async (request, onReceipt) => {
      assert.equal(request.options.delivery.id, attempt.id)
      assert.equal(request.options.delivery.new_session, request.newSession)
      onReceipt(accepted(attempt))
      return { ok: false, error: 'model failed after acceptance' }
    }),
    true,
  )
  assert.equal(cleared, 1)
  assert.equal(attempt.state, 'accepted')
  assert.equal(queries, 0)
  assert.equal(attempt.snapshot.message, 'original\r\n中文')
  assert.equal(
    await attempt.submit(async () => {
      throw Error('must not run twice')
    }),
    false,
  )
})

test('a missing read is followed by atomic retirement before input is declared unreceived', async () => {
  const calls = []
  let runs = 0
  const attempt = new DeliveryAttempt(
    transport(async (ws, payload) => {
      calls.push({ ws, payload })
      return {
        ok: true,
        data: {
          receipt:
            payload.operation === 'read'
              ? null
              : { ...accepted(attempt), phase: 'retired', accepted_revision: 0 },
        },
      }
    }),
    snapshot(),
  )
  assert.equal(
    await attempt.submit(async () => {
      runs++
      throw Error('network outcome unknown')
    }),
    false,
  )
  assert.equal(attempt.state, 'rejected')
  assert.equal(runs, 1)
  assert.deepEqual(
    calls.map((call) => call.payload.operation),
    ['read', 'retire'],
  )
  assert(calls.every((call) => call.payload.id === attempt.id && call.ws === 'workspace-a'))
})

test('when acceptance wins the retirement race its original receipt is kept', async () => {
  let callback = 0
  const attempt = new DeliveryAttempt(
    transport(async (_ws, payload) => ({
      ok: true,
      data: { receipt: payload.operation === 'read' ? null : accepted(attempt) },
    })),
    snapshot(),
    () => callback++,
  )
  assert.equal(await attempt.submit(async () => ({ ok: false, error: 'connection lost' })), true)
  assert.equal(attempt.state, 'accepted')
  assert.equal(callback, 1)
})

test('unknown reconciliation keeps the original ID and never automatically resends', async () => {
  const ids = []
  let connected = false,
    runs = 0,
    callback = 0
  const attempt = new DeliveryAttempt(
    transport(async (_ws, payload) => {
      ids.push(payload.id)
      return connected
        ? { ok: true, data: { receipt: accepted(attempt) } }
        : { ok: false, error: 'offline' }
    }),
    snapshot(),
    () => callback++,
  )
  assert.equal(
    await attempt.submit(async () => {
      runs++
      throw Error('offline')
    }),
    false,
  )
  assert.equal(attempt.state, 'unknown')
  assert.equal(
    await attempt.submit(async () => {
      runs++
      return { ok: true }
    }),
    false,
  )
  connected = true
  assert.equal(await attempt.reconcile(), true)
  assert(ids.every((id) => id === attempt.id))
  assert.equal(runs, 1)
  assert.equal(callback, 1)
})

test('an unsupported host or failed pending-draft backup cannot start a turn', async () => {
  let runs = 0
  const unsupported = new DeliveryAttempt(
    { resourceRequest: async () => ({ ok: false, error: 'Unimplemented' }) },
    snapshot(),
  )
  assert.equal(
    await unsupported.submit(async () => {
      runs++
      return { ok: true }
    }),
    false,
  )
  assert.equal(unsupported.state, 'unsupported')
  const backup = new DeliveryAttempt(
    transport(() => {
      throw Error('unused')
    }),
    snapshot(),
  )
  assert.equal(
    await backup.submit(
      async () => {
        runs++
        return { ok: true }
      },
      async () => false,
    ),
    false,
  )
  assert.equal(backup.state, 'draft')
  assert.equal(runs, 0)
})

test('recovered pending input only reconciles and rejects a receipt for another session', async () => {
  const saved = snapshot(),
    id = crypto.randomUUID()
  const attempt = new DeliveryAttempt(
    transport(async () => ({
      ok: true,
      data: { receipt: { ...accepted(attempt), session: 'different-session' } },
    })),
    saved,
    () => {
      throw Error('wrong target cleared')
    },
    () => {},
    id,
  )
  assert.equal(
    await attempt.submit(async () => {
      throw Error('recovered requests cannot auto-run')
    }),
    false,
  )
  assert.equal(await attempt.reconcile(), false)
  assert.equal(attempt.state, 'unknown')
  assert.equal(attempt.id, id)
})

test('duplicate submit calls during preflight still create at most one transport attempt', async () => {
  let release,
    runs = 0
  const gate = new Promise((resolve) => {
    release = resolve
  })
  const attempt = new DeliveryAttempt(
    transport(() => {
      throw Error('unused')
    }),
    snapshot(),
  )
  const first = attempt.submit(
    async (_request, onReceipt) => {
      runs++
      onReceipt(accepted(attempt))
      return { ok: true }
    },
    async () => {
      await gate
      return true
    },
  )
  assert.equal(
    await attempt.submit(async () => {
      runs++
      return { ok: true }
    }),
    false,
  )
  release()
  assert.equal(await first, true)
  assert.equal(runs, 1)
})

test('a host missing cross-reopen pending reconciliation cannot receive a protected send', async () => {
  const attempt = new DeliveryAttempt(
    { resourceRequest: async () => ({ ok: true, data: { durable_manual_send: true } }) },
    snapshot(),
  )
  assert.equal(
    await attempt.submit(async () => {
      throw Error('must not send to an older receipt host')
    }),
    false,
  )
  assert.equal(attempt.state, 'unsupported')
})

test('failure to reconcile host-backed pending inputs blocks a fresh send before backup or execution', async () => {
  const calls = []
  let backups = 0,
    runs = 0
  const attempt = new DeliveryAttempt(
    {
      resourceRequest: async (_method, _ws, payload) => {
        calls.push(payload.operation)
        return payload.operation === 'capabilities'
          ? { ok: true, data: { durable_manual_send: true, pending_reconciliation: true } }
          : { ok: false, error: 'pending receipt storage is unavailable' }
      },
    },
    snapshot(),
  )
  assert.equal(
    await attempt.submit(
      async () => {
        runs++
        return { ok: true }
      },
      async () => {
        backups++
        return true
      },
    ),
    false,
  )
  assert.deepEqual(calls, ['capabilities', 'reconcile_pending'])
  assert.equal(backups, 0)
  assert.equal(runs, 0)
  assert.equal(attempt.state, 'unknown')
})
