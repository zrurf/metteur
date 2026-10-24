import { test } from 'node:test'
import assert from 'node:assert/strict'
import { RecoveryCopy } from '../src/core/recovery.ts'
import { SupervisionInput } from '../src/core/supervision-input.ts'
const clone = (value) => JSON.parse(JSON.stringify(value))
function gate() {
  let resolve
  return { promise: new Promise((done) => (resolve = done)), release: () => resolve() }
}
function setup() {
  const run = crypto.randomUUID(),
    records = new Map(),
    receipts = new Map(),
    calls = []
  const flags = { offline: false, unsupported: false, source: true, failBackup: false }
  const temporary = new Map()
  const transport = {
    resourceRequest: async (method, _workspace, payload, expected = 0) => {
      calls.push({ method, payload: clone(payload) })
      if (method === 'chatDelivery' && payload.operation === 'workflow') {
        if (flags.offline) return { ok: false, error: 'Disconnected' }
        const q = payload.action
        let record = temporary.get(q.id)
        if (!record) {
          record = {
            schema: 1,
            id: q.id,
            target: q.target,
            operation: q.action,
            body: q.body,
            revision: 0,
            phase: 'retired',
          }
          temporary.set(q.id, record)
        }
        return { ok: true, data: { receipt: clone(record) } }
      }
      if (method === 'conciergeDelivery') {
        if (payload.operation === 'capabilities')
          return {
            ok: true,
            data: {
              durable_intake: !flags.unsupported,
              read_or_retire: true,
              pending_reconciliation: true,
            },
          }
        if (payload.operation === 'reconcile_pending') return { ok: true, data: { items: [] } }
        if (flags.offline) return { ok: false, error: 'Disconnected' }
        if (!receipts.has(payload.id))
          receipts.set(payload.id, {
            schema_version: 1,
            id: payload.id,
            run,
            conversation_id: run,
            phase: 'retired',
            turn: null,
          })
        return {
          ok: true,
          data: { receipt: clone(receipts.get(payload.id)), source_available: flags.source },
        }
      }
      const action = payload.action,
        id = action.id ?? action.input?.id,
        current = records.get(id)
      if (action.operation === 'backup' && flags.failBackup)
        return { ok: false, error: 'Backup full' }
      if ((current?.revision ?? 0) !== expected) return { ok: false, error: 'revision_conflict' }
      const record =
        action.operation === 'discard'
          ? { ...current, discarded: true, revision: expected + 1 }
          : {
              schema_version: 1,
              revision: expected + 1,
              input: clone(action.input),
              discarded: false,
              updated_ms: 1,
            }
      records.set(id, record)
      return { ok: true, data: clone(record) }
    },
  }
  const primary = new RecoveryCopy(
    transport,
    'workspace',
    'project',
    'supervision',
    run,
    crypto.randomUUID(),
  )
  primary.edit({ raw: '  original\r\n中文  ' })
  const input = new SupervisionInput(transport, primary)
  const receive = (id, text, state = 'processing') => {
    const turn = {
      id,
      conversation_id: run,
      original_text: text,
      at_ms: 1,
      answer: '',
      state,
      request_id: null,
      error: null,
    }
    receipts.set(id, { schema_version: 1, id, run, conversation_id: run, phase: 'accepted', turn })
    return clone(turn)
  }
  return {
    run,
    transport,
    records,
    receipts,
    temporary,
    calls,
    flags,
    primary,
    input,
    receive,
    dispose() {
      input.dispose()
      primary.dispose()
    },
  }
}

test('unreviewed recovered selections never disappear into a text-only supervision submission', async () => {
  const f = setup()
  f.input.edit({ selections: [{ kind: 'prompt', unverified: true }] })
  let executed = 0
  assert.equal(
    await f.input.submit(f.primary.input.raw, async () => {
      executed++
      return { ok: true }
    }),
    false,
  )
  assert.equal(executed, 0)
  assert.equal(f.primary.input.selections.length, 1)
  f.dispose()
})

test('temporary input keeps newer typing and reconciles its own backed-up identity after reload', async () => {
  const f = setup()
  let effects = 0,
    originalId
  const accepted = f.input.temporary(async (text, id) => {
    effects++
    originalId = id
    f.temporary.set(id, {
      schema: 1,
      id,
      target: { kind: 'blueprint', run: f.run },
      operation: 'temporary_input',
      body: text,
      revision: 0,
      phase: 'queued',
    })
    f.input.edit({ raw: 'newer draft' })
    f.flags.offline = true
    return { ok: false, error: 'Lost ACK' }
  })
  assert.equal(await accepted, false)
  assert.equal(f.input.state, 'unknown')
  assert.equal(f.primary.input.raw, 'newer draft')
  await f.primary.flush()
  const record = clone(f.records.get(f.primary.input.id))
  const restored = new RecoveryCopy(
    f.transport,
    'workspace',
    'project',
    'supervision',
    f.run,
    crypto.randomUUID(),
  )
  restored.loadAsNewCopy(SupervisionInput.restored(record))
  const input = new SupervisionInput(f.transport, restored)
  f.flags.offline = false
  assert(await input.reconcile())
  assert.equal(restored.input.raw, 'newer draft')
  assert.equal(restored.input.pending_submission, null)
  assert.equal(effects, 1)
  assert(
    f.calls
      .filter((c) => c.method === 'chatDelivery')
      .every((c) => c.payload.action.id === originalId),
  )
  input.dispose()
  restored.dispose()
  f.dispose()
})
test('temporary input without a receipt is fenced and keeps the original raw text', async () => {
  const f = setup()
  assert.equal(await f.input.temporary(async () => ({ ok: false, error: 'Not sent' })), false)
  assert.equal(f.primary.input.raw, '  original\r\n中文  ')
  assert.equal(f.primary.input.pending_submission, null)
  assert.equal(f.input.canSubmit, true)
  f.dispose()
})

test('an older terminal event cannot replace the status or raw input of a newer pending submission', async () => {
  const f = setup(),
    entered = gate(),
    release = gate()
  let late
  const first = f.input.submit(f.primary.input.raw, async (id, text, ack) => {
    ack(f.receive(id, text))
    late = () => ack(f.receive(id, text, 'failed'))
    entered.release()
    await release.promise
    return { ok: true }
  })
  await entered.promise
  f.input.edit({ raw: 'new pending input' })
  assert.equal(
    await f.input.submit('new pending input', async (id, text) => {
      f.receive(id, text)
      f.flags.offline = true
      return { ok: false, error: 'unknown newer input' }
    }),
    false,
  )
  const pending = f.primary.input.pending_submission
  late()
  assert.equal(f.input.state, 'unknown')
  assert.equal(f.primary.input.pending_submission, pending)
  assert.equal(f.primary.input.raw, 'new pending input')
  release.release()
  await first
  f.dispose()
})
test('supervision keeps exact raw text until its durable processing receipt, independently of model failure', async () => {
  const f = setup()
  assert(
    await f.input.submit(f.primary.input.raw, async (id, text, ack) => {
      assert.equal(f.records.size, 2)
      assert([...f.records.values()].every((record) => record.input.pending_submission === id))
      ack(f.receive(id, text))
      assert.equal(f.primary.input.raw, '')
      f.input.edit({ raw: 'newer after acceptance' })
      ack(f.receive(id, text, 'failed'))
      return { ok: false, error: 'Model failed' }
    }),
  )
  assert.equal(f.primary.input.raw, 'newer after acceptance')
  assert.equal(f.input.state, 'accepted')
  f.dispose()
})
test('late processing ACK never clears a newer draft; original input is independently recoverable', async () => {
  const f = setup(),
    entered = gate(),
    release = gate()
  const send = f.input.submit(f.primary.input.raw, async (id, text, ack) => {
    entered.release()
    await release.promise
    ack(f.receive(id, text))
    return { ok: true }
  })
  await entered.promise
  f.input.edit({ raw: 'newer before receipt' })
  release.release()
  assert(await send)
  assert.equal(f.primary.input.raw, 'newer before receipt')
  assert(
    [...f.records.values()].some(
      (record) =>
        record.input.id !== f.primary.input.id && record.input.raw === '  original\r\n中文  ',
    ),
  )
  f.dispose()
})
test('reopened pending supervision only reconciles its original ID and never starts a model', async () => {
  const f = setup()
  let executions = 0
  assert.equal(
    await f.input.submit(f.primary.input.raw, async (id, text) => {
      executions++
      f.receive(id, text)
      f.flags.offline = true
      return { ok: false, error: 'Lost receipt' }
    }),
    false,
  )
  const pending = f.primary.input.pending_submission,
    record = clone(f.records.get(f.primary.input.id))
  assert.equal(f.input.state, 'unknown')
  assert.equal(
    await f.input.submit('retry', async () => {
      executions++
      return { ok: true }
    }),
    false,
  )
  const restored = new RecoveryCopy(
    f.transport,
    'workspace',
    'project',
    'supervision',
    f.run,
    crypto.randomUUID(),
  )
  restored.loadAsNewCopy(SupervisionInput.restored(record))
  const input = new SupervisionInput(f.transport, restored)
  f.flags.offline = false
  assert(await input.reconcile())
  assert.equal(restored.input.raw, '')
  assert.equal(executions, 1)
  assert(
    f.calls
      .filter((call) => call.payload.operation === 'read_or_retire')
      .every((call) => call.payload.id === pending),
  )
  input.dispose()
  restored.dispose()
  f.dispose()
})
test('missing outcome is retired before new input is allowed; raw draft remains', async () => {
  const f = setup()
  assert.equal(
    await f.input.submit(f.primary.input.raw, async () => ({ ok: false, error: 'Refused' })),
    false,
  )
  assert.equal(f.input.state, 'rejected')
  assert.equal(f.primary.input.raw, '  original\r\n中文  ')
  assert.equal(f.primary.input.pending_submission, null)
  f.dispose()
})
test('wrong turn identity and unavailable source do not clear the protected original', async () => {
  const f = setup()
  assert.equal(
    await f.input.submit(f.primary.input.raw, async (id, text, ack) => {
      ack({ ...f.receive(id, text), conversation_id: crypto.randomUUID() })
      f.flags.offline = true
      return { ok: true }
    }),
    false,
  )
  assert.equal(f.primary.input.raw, '  original\r\n中文  ')
  f.flags.offline = false
  f.flags.source = false
  assert(await f.input.reconcile())
  assert.equal(f.primary.input.raw, '  original\r\n中文  ')
  assert.equal(f.input.canSubmit, false)
  f.dispose()
})
test('unsupported host and failed backup never execute; duplicate clicks while backing up execute once', async () => {
  for (const flag of ['unsupported', 'failBackup']) {
    const f = setup()
    f.flags[flag] = true
    let executions = 0
    assert.equal(
      await f.input.submit(f.primary.input.raw, async () => {
        executions++
        return { ok: true }
      }),
      false,
    )
    assert.equal(executions, 0)
    assert.equal(f.primary.input.raw, '  original\r\n中文  ')
    f.dispose()
  }
  const f = setup(),
    entered = gate(),
    release = gate()
  let executions = 0
  const first = f.input.submit(f.primary.input.raw, async (id, text, ack) => {
    executions++
    entered.release()
    await release.promise
    ack(f.receive(id, text))
    return { ok: true }
  })
  await entered.promise
  assert.equal(
    await f.input.submit(f.primary.input.raw, async () => {
      executions++
      return { ok: true }
    }),
    false,
  )
  release.release()
  assert(await first)
  assert.equal(executions, 1)
  f.dispose()
})
