import { test } from 'node:test'
import assert from 'node:assert/strict'
import { RecoveryCopy } from '../src/core/recovery.ts'

function deferred() {
  let resolve
  const promise = new Promise((r) => {
    resolve = r
  })
  return { promise, resolve }
}
function ack(payload, revision) {
  return {
    ok: true,
    data: {
      schema_version: 1,
      revision,
      input: structuredClone(payload.action.input),
      discarded: false,
    },
  }
}
function copy(transport) {
  return new RecoveryCopy(
    transport,
    'fixture-workspace',
    'project',
    'text',
    'src/a.ts',
    crypto.randomUUID(),
  )
}

test('compacted generation refreshes only definitively rejected new identities', async () => {
  const epoch = crypto.randomUUID(),
    calls = []
  const instance = copy({
    resourceRequest: async (_method, _ws, payload) => {
      calls.push(structuredClone(payload))
      if (payload.action.operation === 'capabilities')
        return { ok: true, data: { creation_epoch: epoch } }
      if (payload.action.input.creation_epoch !== epoch)
        return { ok: false, error: '{"code":"creation_epoch_required"}' }
      return ack(payload, 1)
    },
  })
  const oldId = instance.input.id
  instance.edit({ raw: 'retain exact input\r\n中文', selections: ['one', 'one'] })
  assert.equal(await instance.flush(), true)
  assert.notEqual(instance.input.id, oldId)
  assert.equal(instance.input.creation_epoch, epoch)
  assert.equal(instance.input.raw, 'retain exact input\r\n中文')
  assert.deepEqual(instance.input.selections, ['one', 'one'])
  assert.equal(calls.length, 3)
  instance.dispose()
})

test('a lost initial ACK followed by retirement never automatically recreates input', async () => {
  let count = 0
  const instance = copy({
    resourceRequest: async (_method, _ws, payload) => {
      assert.equal(payload.action.operation, 'backup')
      if (++count === 1) throw Error('lost ACK')
      return { ok: false, error: '{"code":"creation_epoch_required"}' }
    },
  })
  instance.edit({ raw: 'unknown old copy' })
  const id = instance.input.id
  assert.equal(await instance.flush(), false)
  assert.equal(await instance.flush(), false)
  assert.equal(instance.input.id, id)
  assert.equal(instance.state, 'conflict')
  assert.equal(instance.input.raw, 'unknown old copy')
  instance.dispose()
})

test('a compacted discard receipt closes only the acknowledged copy', async () => {
  const instance = copy({
    resourceRequest: async (_method, _ws, payload) =>
      payload.action.operation === 'backup'
        ? ack(payload, 1)
        : { ok: false, error: '{"code":"retired"}' },
  })
  instance.edit({ raw: 'saved' })
  assert.equal(await instance.flush(), true)
  assert.equal(await instance.discard(), true)
  assert.equal(instance.state, 'discarded')
  assert.equal(instance.input.raw, '')
})

test('backup ACK confirms its snapshot, then serially backs up a newer edit', async () => {
  const calls = [],
    gates = [deferred(), deferred()]
  const instance = copy({
    resourceRequest: async (_method, _ws, payload, expected, request) => {
      const index = calls.length
      calls.push({ payload: structuredClone(payload), expected, request })
      await gates[index].promise
      return ack(payload, index + 1)
    },
  })
  instance.edit({ raw: 'first\r\nraw', selections: ['personal', 'project', 'personal'] })
  const pending = instance.flush()
  instance.edit({ raw: 'second' })
  assert.equal(calls.length, 1)
  gates[0].resolve()
  await Promise.resolve()
  await Promise.resolve()
  assert.equal(instance.acknowledgedSequence, 1)
  assert.equal(instance.input.raw, 'second')
  assert.equal(calls.length, 2)
  assert.equal(calls[1].expected, 1)
  gates[1].resolve()
  assert.equal(await pending, true)
  assert.equal(instance.acknowledgedSequence, 2)
  assert.equal(instance.state, 'backed_up')
  assert.deepEqual(calls[0].payload.action.input.selections, ['personal', 'project', 'personal'])
  instance.dispose()
})

test('malformed backup and discard receipts never clear input or claim success', async () => {
  const backup = copy({ resourceRequest: async () => ({ ok: true, data: null }) })
  backup.edit({ raw: 'keep malformed receipt input' })
  assert.equal(await backup.flush(), false)
  assert.equal(backup.state, 'unknown')
  assert.equal(backup.input.raw, 'keep malformed receipt input')
  backup.dispose()
  const discard = copy({
    resourceRequest: async (_method, _ws, payload) =>
      payload.action.operation === 'backup' ? ack(payload, 1) : { ok: true, data: null },
  })
  discard.edit({ raw: 'keep until valid discard receipt' })
  await discard.flush()
  assert.equal(await discard.discard(), false)
  assert.equal(discard.state, 'unknown')
  assert.equal(discard.input.raw, 'keep until valid discard receipt')
  discard.dispose()
})

test('lost ACK reuses the exact request and snapshot even after a newer edit', async () => {
  const calls = []
  const instance = copy({
    resourceRequest: async (_method, _ws, payload, expected, request) => {
      calls.push({ payload: structuredClone(payload), expected, request })
      if (calls.length === 1) throw Error('connection lost after durable commit')
      return ack(payload, payload.action.input.edit_sequence)
    },
  })
  instance.edit({ raw: 'unknown' })
  assert.equal(await instance.flush(), false)
  assert.equal(instance.state, 'unknown')
  instance.edit({ raw: 'newer input' })
  assert.equal(await instance.flush(), true)
  assert.deepEqual(calls[0], calls[1])
  assert.notEqual(calls[1].request, calls[2].request)
  assert.equal(instance.input.raw, 'newer input')
  instance.dispose()
})

test('unimplemented and capacity rejection never claim a protected copy', async () => {
  for (const error of ['Unimplemented', '{"code":"scope_capacity"}']) {
    const instance = copy({ resourceRequest: async () => ({ ok: false, error }) })
    instance.edit({ raw: 'keep raw {' })
    assert.equal(await instance.flush(), false)
    assert.equal(instance.acknowledgedSequence, 0)
    assert.equal(instance.input.raw, 'keep raw {')
    assert.equal(instance.state, error === 'Unimplemented' ? 'unsupported' : 'unbacked')
    instance.dispose()
  }
})

test('recovery forks the original copy without restoring permissions or window identity', () => {
  const instance = copy({ resourceRequest: async () => ({ ok: false, error: 'unused' }) })
  const original = {
    schema_version: 1,
    revision: 3,
    discarded: false,
    input: {
      ...instance.input,
      id: crypto.randomUUID(),
      window_id: crypto.randomUUID(),
      raw: 'orphaned source',
      pending_submission: 'unresolved-delivery',
      selections: ['p', 'p'],
    },
  }
  assert.equal(instance.loadAsNewCopy(original), true)
  assert.notEqual(instance.input.id, original.input.id)
  assert.notEqual(instance.input.window_id, original.input.window_id)
  assert.equal(instance.input.pending_submission, 'unresolved-delivery')
  assert.equal('permission_mode' in instance.input, false)
  assert.equal(instance.revision, 0)
  instance.dispose()
})

test('discard waits for in-flight ACK and does not send a later queued backup', async () => {
  const gate = deferred(),
    calls = []
  const instance = copy({
    resourceRequest: async (_method, _ws, payload, expected, request) => {
      calls.push({ payload: structuredClone(payload), expected, request })
      if (payload.action.operation === 'backup') {
        await gate.promise
        return ack(payload, 1)
      }
      return {
        ok: true,
        data: { schema_version: 1, revision: 2, discarded: true, input: { ...instance.input } },
      }
    },
  })
  instance.edit({ raw: 'first' })
  const backing = instance.flush()
  instance.edit({ raw: 'later edit to discard explicitly' })
  const discarding = instance.discard()
  gate.resolve()
  await backing
  assert.equal(await discarding, true)
  assert.equal(calls.length, 2)
  assert.equal(calls[1].payload.action.saved_sequence, 1)
  assert.equal(instance.state, 'discarded')
  assert.equal(instance.input.raw, '')
})

test('a lost discard receipt retries the original decision identity', async () => {
  const calls = []
  let failed = false
  const instance = copy({
    resourceRequest: async (_method, _ws, payload, expected, request) => {
      if (payload.action.operation === 'backup') return ack(payload, 1)
      calls.push({ payload: structuredClone(payload), expected, request })
      if (!failed) {
        failed = true
        throw Error('discard receipt lost')
      }
      return {
        ok: true,
        data: { schema_version: 1, revision: 2, discarded: true, input: { ...instance.input } },
      }
    },
  })
  instance.edit({ raw: 'to discard' })
  await instance.flush()
  assert.equal(await instance.discard(), false)
  assert.equal(instance.input.raw, 'to discard')
  assert.equal(await instance.discard(), true)
  assert.deepEqual(calls[0], calls[1])
})

test('concurrent cleanup callers share one in-flight discard decision', async () => {
  const gate = deferred()
  let discards = 0
  const instance = copy({
    resourceRequest: async (_method, _workspace, payload) => {
      if (payload.action.operation === 'backup') return ack(payload, 1)
      discards++
      await gate.promise
      return {
        ok: true,
        data: { schema_version: 1, revision: 2, discarded: true, input: { id: payload.action.id } },
      }
    },
  })
  instance.edit({ raw: 'one input' })
  await instance.flush()
  const first = instance.discard(),
    second = instance.discard()
  assert.equal(discards, 1)
  gate.resolve()
  assert.equal(await first, true)
  assert.equal(await second, true)
  assert.equal(discards, 1)
  instance.dispose()
})

test('an old discard receipt never clears edits typed while it was in flight', async () => {
  const gate = deferred(),
    calls = []
  const instance = copy({
    resourceRequest: async (_method, _ws, payload, expected, request) => {
      calls.push({ payload: structuredClone(payload), expected, request })
      if (payload.action.operation === 'backup') return ack(payload, 1)
      await gate.promise
      return {
        ok: true,
        data: { schema_version: 1, revision: 2, discarded: true, input: { id: payload.action.id } },
      }
    },
  })
  instance.edit({ raw: 'confirmed version' })
  await instance.flush()
  const original = instance.input.id,
    discarding = instance.discard()
  instance.edit({ raw: 'new typing during discard' })
  gate.resolve()
  assert.equal(await discarding, false)
  assert.equal(instance.input.raw, 'new typing during discard')
  assert.notEqual(instance.input.id, original)
  await instance.flush()
  assert.equal(instance.state, 'backed_up')
  assert.equal(calls.at(-1).expected, 0)
  instance.dispose()
})

test('editing after an unknown discard preserves the original retry and forks newer input', async () => {
  const calls = []
  let lost = true
  const instance = copy({
    resourceRequest: async (_method, _ws, payload, expected, request) => {
      calls.push({ payload: structuredClone(payload), expected, request })
      if (payload.action.operation === 'backup') return ack(payload, 1)
      if (lost) {
        lost = false
        throw Error('discard outcome unknown')
      }
      return {
        ok: true,
        data: { schema_version: 1, revision: 2, discarded: true, input: { id: payload.action.id } },
      }
    },
  })
  instance.edit({ raw: 'old' })
  await instance.flush()
  const original = instance.input.id
  assert.equal(await instance.discard(), false)
  instance.edit({ raw: 'keep newer' })
  assert.equal(await instance.flush(), false)
  assert.equal(await instance.discard(), false)
  assert.deepEqual(calls[1], calls[2])
  assert.notEqual(instance.input.id, original)
  assert.equal(instance.input.raw, 'keep newer')
  await instance.flush()
  assert.equal(instance.state, 'backed_up')
  instance.dispose()
})
