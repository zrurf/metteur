import { test } from 'node:test'
import assert from 'node:assert/strict'
import { RecoveryCopy } from '../src/core/recovery.ts'
import { ComposerInput, restoredInput } from '../src/core/composer-input.ts'
const clone = (value) => JSON.parse(JSON.stringify(value))
function deferred() {
  let resolve
  const promise = new Promise((done) => {
    resolve = done
  })
  return { promise, resolve }
}
async function until(check) {
  for (let n = 0; n < 100; n++) {
    if (check()) return
    await new Promise((done) => setImmediate(done))
  }
  assert.fail('Expected state did not settle')
}
function host() {
  const records = new Map(),
    receipts = new Map(),
    events = []
  let offline = false,
    failBackup = false,
    discardGate = null,
    sourceAvailable = true
  const transport = {
    resourceRequest: async (method, workspace, payload, expected = 0, request = '') => {
      events.push({ method, workspace, payload: clone(payload), expected, request })
      if (method === 'chatDelivery') {
        if (payload.operation === 'capabilities')
          return {
            ok: true,
            data: {
              durable_manual_send: true,
              pending_reconciliation: true,
              durable_queue: true,
              durable_queue_channel: 'chat',
            },
          }
        if (payload.operation === 'reconcile_pending')
          return { ok: true, data: { items: [], raw_copies_unchanged: true } }
        if (offline) return { ok: false, error: 'receipt connection unavailable' }
        if (payload.operation === 'read')
          return {
            ok: true,
            data: { receipt: receipts.get(payload.id) ?? null, source_available: sourceAvailable },
          }
        if (payload.operation === 'retire') {
          if (!receipts.has(payload.id))
            receipts.set(payload.id, {
              schema_version: 1,
              id: payload.id,
              phase: 'retired',
              accepted_revision: 0,
              session: '',
              turn: '',
            })
          return {
            ok: true,
            data: { receipt: receipts.get(payload.id), source_available: sourceAvailable },
          }
        }
      }
      const action = payload.action,
        id = action.id ?? action.input?.id,
        current = records.get(id)
      if (action.operation === 'read')
        return current
          ? { ok: true, data: { record: clone(current) } }
          : { ok: false, error: 'not found' }
      if (action.operation === 'backup' && failBackup)
        return { ok: false, error: '{"code":"scope_capacity","message":"Backup capacity reached"}' }
      if (current?.request === request) return { ok: true, data: clone(current) }
      if ((current?.revision ?? 0) !== expected)
        return { ok: false, error: '{"code":"revision_conflict"}' }
      if (current?.discarded) return { ok: false, error: '{"code":"discarded"}' }
      const record =
        action.operation === 'discard'
          ? {
              ...current,
              revision: expected + 1,
              discarded: true,
              input: { ...current.input, raw: '', selections: [], pending_submission: null },
            }
          : {
              schema_version: 1,
              revision: expected + 1,
              input: clone(action.input),
              discarded: false,
              updated_ms: 1,
            }
      record.request = request
      records.set(id, clone(record))
      if (action.operation === 'discard' && discardGate?.id === id) {
        discardGate.entered.resolve()
        await discardGate.release.promise
      }
      return { ok: true, data: clone(record) }
    },
  }
  const accept = (snapshot) => {
    const receipt = {
      schema_version: 1,
      id: snapshot.options.delivery.id,
      input_digest: 'fixture-digest',
      session: snapshot.session || snapshot.newSession,
      turn: crypto.randomUUID(),
      checkpoint: 'checkpoint',
      accepted_revision: 1,
      phase: 'started',
      outcome: null,
      accepted_ms: 1,
      updated_ms: 1,
    }
    receipts.set(receipt.id, receipt)
    return receipt
  }
  return {
    transport,
    records,
    receipts,
    events,
    accept,
    setSourceAvailable: (value) => {
      sourceAvailable = value
    },
    setOffline: (value) => {
      offline = value
    },
    setFailBackup: (value) => {
      failBackup = value
    },
    holdDiscard: (id) => {
      discardGate = { id, entered: deferred(), release: deferred() }
      return discardGate
    },
  }
}
test('Queue and Steer back up original IDs, preserve newer choices and restore without replay', async () => {
  for (const mode of ['queue', 'steer']) {
    const f = setup(),
      gate = deferred()
    f.snapshot.session = f.primary.input.entity
    f.snapshot.newSession = undefined
    f.snapshot.live = { mode, turn: crypto.randomUUID() }
    const task = f.input.submit(
      f.snapshot,
      async (snapshot, onReceipt) => {
        await gate.promise
        const receipt = {
          ...f.accept(snapshot),
          turn: snapshot.live.turn,
          phase: 'live_queued',
          live: { mode, body_digest: 'a'.repeat(64) },
        }
        f.receipts.set(receipt.id, receipt)
        onReceipt(receipt)
        return { ok: true }
      },
      { mode },
    )
    await until(() => f.input.state === 'submitting' && f.primary.input.pending_submission)
    const pending = f.primary.input.pending_submission
    assert.equal(f.primary.input.view.pending_delivery.mode, mode)
    assert.equal(f.primary.input.view.pending_delivery.turn, f.snapshot.live.turn)
    const original = [...f.records.values()].find((r) => r.input.id !== f.primary.input.id)
    assert.equal(original.input.raw, 'original\r\n中文')
    f.input.edit({ raw: 'newer', selections: [{ kind: 'file', path: 'new.ts' }] })
    gate.resolve()
    assert(await task)
    assert.equal(f.primary.input.raw, 'newer')
    assert.deepEqual(f.primary.input.selections, [{ kind: 'file', path: 'new.ts' }])
    assert(!f.records.get(original.input.id).discarded)
    const restored = new RecoveryCopy(
      f.transport,
      'workspace-a',
      'project',
      'chat',
      f.primary.input.entity,
      crypto.randomUUID(),
    )
    restored.loadAsNewCopy(restoredInput(original))
    const controller = new ComposerInput(f.transport, restored)
    assert(await controller.reconcile())
    assert.equal(restored.input.raw, '')
    assert.equal(f.receipts.get(pending).phase, 'live_queued')
    controller.dispose()
    restored.dispose()
    f.dispose()
  }
})
function setup() {
  const fixture = host(),
    entity = crypto.randomUUID()
  const primary = new RecoveryCopy(
    fixture.transport,
    'workspace-a',
    'project',
    'chat',
    entity,
    crypto.randomUUID(),
  )
  primary.edit({
    raw: 'original\r\n中文',
    selections: [
      { kind: 'prompt', id: 'p' },
      { kind: 'prompt', id: 'p' },
    ],
    selected_model: 'fixture-model',
  })
  const input = new ComposerInput(fixture.transport, primary)
  const snapshot = {
    workspace: 'workspace-a',
    session: '',
    newSession: entity,
    message: primary.input.raw,
    history: [],
    options: { model: 'fixture-model' },
  }
  return {
    ...fixture,
    primary,
    input,
    snapshot,
    dispose: () => {
      input.dispose()
      primary.dispose()
    },
  }
}

test('both the immutable submission and pending live input are backed up before execution', async () => {
  const fixture = setup()
  let invoked = 0
  assert.equal(
    await fixture.input.submit(fixture.snapshot, async (snapshot, onReceipt) => {
      invoked++
      const copies = [...fixture.records.values()]
      assert.equal(copies.length, 2)
      assert(copies.every((row) => row.input.pending_submission === snapshot.options.delivery.id))
      assert(copies.every((row) => row.input.raw === 'original\r\n中文'))
      onReceipt(fixture.accept(snapshot))
      return { ok: false, error: 'model failed after acceptance' }
    }),
    true,
  )
  assert.equal(invoked, 1)
  assert.equal(fixture.primary.input.raw, '')
  await until(() => fixture.primary.state === 'discarded')
  fixture.dispose()
})

test('a received recovery copy with a deleted source is kept and cannot be silently resent', async () => {
  const fixture = setup()
  let snapshot
  fixture.setOffline(true)
  await fixture.input.submit(fixture.snapshot, async (value) => {
    snapshot = value
    return { ok: false, error: 'receipt lost' }
  })
  await fixture.primary.flush()
  const record = clone(fixture.records.get(fixture.primary.input.id))
  fixture.dispose()
  const primary = new RecoveryCopy(
    fixture.transport,
    'workspace-a',
    'project',
    'chat',
    record.input.entity,
    crypto.randomUUID(),
  )
  primary.loadAsNewCopy(restoredInput(record))
  const input = new ComposerInput(fixture.transport, primary)
  fixture.accept(snapshot)
  fixture.setOffline(false)
  fixture.setSourceAvailable(false)
  assert.equal(await input.reconcile(), true)
  assert.equal(primary.input.raw, 'original\r\n中文')
  assert(primary.input.pending_submission)
  assert.equal(input.canSubmit, false)
  assert.match(input.error, /source is unavailable/)
  assert.equal(
    await input.submit(fixture.snapshot, async () => {
      throw Error('must not resend an orphaned accepted input')
    }),
    false,
  )
  input.dispose()
  primary.dispose()
})

test('acceptance of an older input never clears newer text or its duplicate ordered selections', async () => {
  const fixture = setup(),
    entered = deferred(),
    finish = deferred()
  let onReceipt, sent
  const sending = fixture.input.submit(fixture.snapshot, async (snapshot, ack) => {
    sent = snapshot
    onReceipt = ack
    entered.resolve()
    await finish.promise
    return { ok: true }
  })
  await entered.promise
  fixture.primary.edit({
    raw: 'newer text',
    selections: [
      { kind: 'prompt', id: 'p' },
      { kind: 'prompt', id: 'p' },
      { kind: 'file', path: 'new.ts' },
    ],
  })
  onReceipt(fixture.accept(sent))
  finish.resolve()
  assert.equal(await sending, true)
  assert.equal(fixture.primary.input.raw, 'newer text')
  assert.equal(fixture.primary.input.selections.length, 3)
  assert.equal(fixture.primary.input.pending_submission, null)
  await fixture.primary.flush()
  fixture.dispose()
})

test('rejection retains the older submitted original separately from the newer live draft', async () => {
  const fixture = setup(),
    entered = deferred(),
    finish = deferred()
  const sending = fixture.input.submit(fixture.snapshot, async () => {
    entered.resolve()
    await finish.promise
    return { ok: false, error: 'request rejected' }
  })
  await entered.promise
  fixture.primary.edit({ raw: 'newer after click' })
  finish.resolve()
  assert.equal(await sending, false)
  assert.equal(fixture.primary.input.raw, 'newer after click')
  await until(() =>
    [...fixture.records.values()].some(
      (row) =>
        !row.discarded &&
        row.input.id !== fixture.primary.input.id &&
        row.input.raw === 'original\r\n中文' &&
        !row.input.pending_submission,
    ),
  )
  fixture.dispose()
})

for (const edited of [false, true])
  test(`recovered pending input reconciles without execution and ${edited ? 'keeps newer edits' : 'clears only the submitted version'}`, async () => {
    const fixture = setup()
    let originalSnapshot
    fixture.setOffline(true)
    assert.equal(
      await fixture.input.submit(fixture.snapshot, async (snapshot) => {
        originalSnapshot = snapshot
        if (edited) fixture.primary.edit({ raw: 'newer before reload' })
        await fixture.primary.flush()
        return { ok: false, error: 'receipt lost' }
      }),
      false,
    )
    await fixture.primary.flush()
    const record = clone(fixture.records.get(fixture.primary.input.id))
    fixture.dispose()
    const restored = new RecoveryCopy(
      fixture.transport,
      'workspace-a',
      'project',
      'chat',
      record.input.entity,
      crypto.randomUUID(),
    )
    assert.equal(restored.loadAsNewCopy(restoredInput(record)), true)
    const input = new ComposerInput(fixture.transport, restored)
    fixture.accept(originalSnapshot)
    fixture.setOffline(false)
    assert.equal(await input.reconcile(), true)
    assert.equal(restored.input.raw, edited ? 'newer before reload' : '')
    assert.equal(restored.input.pending_submission, null)
    input.dispose()
    restored.dispose()
  })

test('failure to back up the pending submission never invokes the model', async () => {
  const fixture = setup()
  let models = 0
  fixture.setFailBackup(true)
  assert.equal(
    await fixture.input.submit(fixture.snapshot, async () => {
      models++
      return { ok: true }
    }),
    false,
  )
  assert.equal(models, 0)
  assert.equal(fixture.primary.input.raw, 'original\r\n中文')
  fixture.dispose()
})

test('a late primary cleanup receipt forks and preserves text typed after acceptance', async () => {
  const fixture = setup(),
    oldId = fixture.primary.input.id,
    gate = fixture.holdDiscard(oldId)
  assert.equal(
    await fixture.input.submit(fixture.snapshot, async (snapshot, onReceipt) => {
      onReceipt(fixture.accept(snapshot))
      return { ok: true }
    }),
    true,
  )
  await gate.entered.promise
  fixture.primary.edit({ raw: 'typing after accepted input was cleared' })
  gate.release.resolve()
  await until(() => fixture.primary.state === 'backed_up')
  assert.equal(fixture.primary.input.raw, 'typing after accepted input was cleared')
  assert.notEqual(fixture.primary.input.id, oldId)
  assert.equal(fixture.records.get(oldId).discarded, true)
  fixture.dispose()
})
