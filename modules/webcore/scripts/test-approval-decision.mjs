import { test } from 'node:test'
import assert from 'node:assert/strict'
import { ApprovalDecision, approvalDigest } from '../src/core/approval-decision.ts'

const deferred = () => {
  let resolve
  const promise = new Promise((done) => {
    resolve = done
  })
  return { promise, resolve }
}
function fixture(mode = 'accept') {
  const snapshot = {
    workspace: 'fixture-project',
    id: crypto.randomUUID(),
    target: { kind: 'chat', session: crypto.randomUUID(), turn: crypto.randomUUID() },
    content_digest: 'a'.repeat(64),
  }
  const calls = [],
    gate = deferred()
  const state = {
    mode,
    receipt: null,
    offline: false,
    unsupported: false,
    pending: structuredClone({
      id: snapshot.id,
      target: snapshot.target,
      content_digest: snapshot.content_digest,
    }),
  }
  const transport = {
    resourceRequest: async (_method, _ws, payload) => {
      calls.push({ method: 'query', payload: structuredClone(payload) })
      if (state.offline) return { ok: false, error: 'Disconnected receipt query' }
      if (payload.operation === 'capabilities')
        return {
          ok: true,
          data: {
            durable_decisions: !state.unsupported,
            fixed_target_content: true,
            read_or_retire: true,
          },
        }
      if (payload.operation === 'describe')
        return { ok: true, data: { receipt: state.receipt, pending: state.pending } }
      if (payload.operation === 'read_or_retire' && !state.receipt)
        state.receipt = {
          schema_version: 1,
          id: snapshot.id,
          target: structuredClone(snapshot.target),
          content_digest: snapshot.content_digest,
          phase: 'retired',
          decision: null,
        }
      return { ok: true, data: { receipt: state.receipt } }
    },
    respondApproval: async (workspace, id, allow, binding) => {
      calls.push({ method: 'respond', workspace, id, allow, binding: structuredClone(binding) })
      if (state.mode === 'reject') return { ok: false, error: 'Request rejected before acceptance' }
      state.receipt = {
        schema_version: 1,
        id,
        target: structuredClone(binding.target),
        content_digest: binding.content_digest,
        phase: state.mode === 'claimed' ? 'claimed' : 'accepted',
        decision: allow ? 'AllowOnce' : 'DenyOnce',
      }
      if (state.mode === 'delay') await gate.promise
      if (state.mode === 'lost') {
        state.offline = true
        return { ok: false, error: 'Lost response' }
      }
      if (state.mode === 'wrong-receipt') state.receipt.id = crypto.randomUUID()
      if (state.mode === 'invalid-decision') state.receipt.decision = null
      return { ok: true, data: undefined }
    },
  }
  return {
    snapshot,
    calls,
    gate,
    state,
    transport,
    controller: new ApprovalDecision(transport, snapshot),
  }
}
test('an exact target is frozen, duplicate clicks execute once, and only the matching receipt resolves', async () => {
  const f = fixture('delay'),
    original = structuredClone(f.snapshot)
  const first = f.controller.respond(true)
  assert.equal(await f.controller.respond(false), false)
  while (!f.calls.some((call) => call.method === 'respond')) await Promise.resolve()
  f.snapshot.target.turn = crypto.randomUUID()
  f.gate.resolve()
  assert.equal(await first, true)
  assert.equal(f.controller.state, 'accepted')
  const sent = f.calls.filter((call) => call.method === 'respond')
  assert.equal(sent.length, 1)
  assert.equal(sent[0].binding.target.turn, original.target.turn)
  assert.equal(await f.controller.respond(true), false)
})
test('rejection without receipt stays unknown until the original ID is explicitly fenced', async () => {
  const f = fixture('reject')
  assert.equal(await f.controller.respond(false), false)
  assert.equal(f.controller.state, 'unknown')
  assert.equal(await f.controller.respond(true), false)
  assert.equal(await f.controller.reconcile(), false)
  assert.equal(f.controller.state, 'retired')
  assert.equal(f.calls.filter((call) => call.method === 'respond').length, 1)
  assert.equal(f.calls.at(-1).payload.id, f.controller.snapshot.id)
})
test('a lost reply reconciles the accepted decision without replaying it', async () => {
  const f = fixture('lost')
  assert.equal(await f.controller.respond(true), false)
  assert.equal(f.controller.state, 'unknown')
  f.state.offline = false
  assert.equal(await f.controller.reconcile(), true)
  assert.equal(f.controller.state, 'accepted')
  assert.equal(f.calls.filter((call) => call.method === 'respond').length, 1)
})
test('reopening a claimed crash-window decision never grants it a second time', async () => {
  const f = fixture('claimed')
  assert.equal(await f.controller.respond(true), false)
  const reopened = new ApprovalDecision(f.transport, f.controller.snapshot)
  assert.equal(await reopened.respond(true), false)
  assert.equal(reopened.state, 'unknown')
  assert.equal(f.calls.filter((call) => call.method === 'respond').length, 1)
})
test('a changed target, changed content and unsupported host never receive a decision', async () => {
  for (const kind of ['target', 'content', 'host']) {
    const f = fixture()
    if (kind === 'target') f.state.pending.target.turn = crypto.randomUUID()
    if (kind === 'content') f.state.pending.content_digest = 'b'.repeat(64)
    if (kind === 'host') f.state.unsupported = true
    assert.equal(await f.controller.respond(true), false)
    assert.equal(f.calls.filter((call) => call.method === 'respond').length, 0)
  }
})
test('wrong identity and invalid accepted decisions cannot clear the pending source', async () => {
  for (const mode of ['wrong-receipt', 'invalid-decision']) {
    const f = fixture(mode)
    assert.equal(await f.controller.respond(true), false)
    assert.equal(f.controller.state, 'unknown')
    assert.equal(f.controller.receipt, null)
  }
})
test('a different earlier accepted choice is reported honestly without applying the new one', async () => {
  const f = fixture()
  await f.controller.respond(false)
  const reopened = new ApprovalDecision(f.transport, f.snapshot)
  assert.equal(await reopened.respond(true), true)
  assert.match(reopened.error, /earlier DenyOnce/)
  assert.equal(f.calls.filter((call) => call.method === 'respond').length, 1)
})
test('content identity uses exact UTF-8 text including line endings', async () => {
  const a = await approvalDigest('动作\r\nraw'),
    b = await approvalDigest('动作\nraw')
  assert.equal(a.length, 64)
  assert.notEqual(a, b)
})
