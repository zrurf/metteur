import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  submitWorkflow,
  checkWorkflowReceipt,
  localWorkflowReceipts,
} from '../src/core/workflow-receipts.ts'
const clone = (v) => JSON.parse(JSON.stringify(v))
function fixture() {
  localWorkflowReceipts.clear()
  const records = new Map(),
    calls = [],
    target = { kind: 'blueprint', run: crypto.randomUUID() }
  const flags = { offline: false, unsupported: false, revision: 0, wrong: false }
  const transport = {
    resourceRequest: async (method, ws, payload) => {
      assert.equal(method, 'chatDelivery')
      if (payload.operation === 'capabilities')
        return { ok: true, data: { targeted_controls: !flags.unsupported } }
      assert.equal(payload.operation, 'workflow')
      const q = payload.action
      calls.push(clone(q))
      if (flags.offline) return { ok: false, error: 'Disconnected' }
      if (q.operation === 'capabilities')
        return { ok: true, data: { durable_controls: !flags.unsupported } }
      if (q.operation === 'describe')
        return { ok: true, data: { target: q.target, revision: flags.revision } }
      if (q.operation === 'prepare') {
        const receipt = {
          schema: 1,
          id: q.identity.id,
          target: q.target,
          operation: q.action,
          body: q.body,
          revision: q.identity.revision,
          phase: 'prepared',
        }
        records.set(receipt.id, receipt)
        return { ok: true, data: { receipt: clone(receipt) } }
      }
      let r = records.get(q.id)
      if (!r) {
        r = {
          schema: 1,
          id: q.id,
          target: q.target,
          operation: q.action,
          body: q.body,
          revision: 0,
          phase: q.action === 'stop' ? 'unknown' : 'retired',
        }
        records.set(q.id, r)
      }
      if (r.phase === 'prepared') r.phase = 'retired'
      return {
        ok: true,
        data: {
          receipt: {
            ...clone(r),
            ...(flags.wrong ? { target: { kind: 'blueprint', run: crypto.randomUUID() } } : {}),
          },
        },
      }
    },
  }
  const receive = (raw, action = 'pause', body = null) => {
    const { id, revision } = JSON.parse(raw)
    const r = records.get(id) ?? {
      schema: 1,
      id,
      target: clone(target),
      operation: action,
      body,
      revision,
    }
    if (r.phase === 'retired') return false
    r.phase = action === 'temporary_input' ? 'queued' : 'accepted'
    records.set(id, r)
    return true
  }
  return { records, calls, target, flags, transport, receive }
}
test('lost ACK reads the original receipt and never sends a control twice', async () => {
  const f = fixture()
  let effects = 0
  const result = await submitWorkflow(f.transport, 'ws', f.target, 'pause', null, async (raw) => {
    effects++
    f.receive(raw)
    return { ok: false, error: 'ACK lost' }
  })
  assert(result.ok)
  assert.equal(effects, 1)
  assert.equal(f.calls.filter((q) => q.operation === 'fence').length, 1)
})
test('missing receipt fences a late request before another decision; draft body is retained', async () => {
  const f = fixture()
  let identity
  const result = await submitWorkflow(
    f.transport,
    'ws',
    f.target,
    'temporary_input',
    '  中文\r\n  ',
    async (raw) => {
      identity = raw
      return { ok: false, error: 'Disconnected before receive' }
    },
  )
  assert(!result.ok)
  assert.equal(f.receive(identity, 'temporary_input', '  中文\r\n  '), false)
  const record = [...f.records.values()][0]
  assert.equal(record.body, '  中文\r\n  ')
  assert.equal(record.phase, 'retired')
})
test('unknown receipt check never executes and verifies the exact immutable target', async () => {
  const f = fixture()
  const run = f.target.run
  const result = await submitWorkflow(f.transport, 'ws', f.target, 'pause', null, async (raw) => {
    f.receive(raw)
    f.target.run = crypto.randomUUID()
    f.flags.offline = true
    return { ok: false, error: 'lost' }
  })
  assert(!result.ok)
  const local = [...localWorkflowReceipts.values()][0]
  assert.equal(local.receipt.target.run, run)
  f.flags.offline = false
  const checked = await checkWorkflowReceipt(f.transport, 'ws', local.receipt)
  assert(checked.ok)
  assert.equal(checked.data.phase, 'accepted')
  f.flags.wrong = true
  assert(!(await checkWorkflowReceipt(f.transport, 'ws', local.receipt)).ok)
})
test('Stop verifies target support but never waits for draft preparation or an earlier control', async () => {
  const f = fixture()
  f.flags.offline = true
  let effects = 0
  const result = await submitWorkflow(f.transport, 'ws', f.target, 'stop', null, async (raw) => {
    assert.equal(f.calls.length, 0)
    effects++
    f.receive(raw, 'stop')
    return { ok: true }
  })
  assert.equal(effects, 1)
  assert(!result.ok)
  assert(!f.calls.some((q) => ['describe', 'prepare', 'capabilities'].includes(q.operation)))
})
test('unsupported host never receives Pause or temporary input', async () => {
  const f = fixture()
  f.flags.unsupported = true
  let effects = 0
  for (const action of ['pause', 'temporary_input'])
    assert(
      !(
        await submitWorkflow(
          f.transport,
          'ws',
          f.target,
          action,
          action === 'pause' ? null : 'draft',
          async () => {
            effects++
            return { ok: true }
          },
        )
      ).ok,
    )
  assert.equal(effects, 0)
})
