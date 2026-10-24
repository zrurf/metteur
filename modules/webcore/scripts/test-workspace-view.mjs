import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  WorkspaceViewCopy,
  emptyWorkspaceView,
  parseWorkspaceView,
  viewPath,
} from '../src/core/workspace-view.ts'

const deferred = () => {
  let resolve
  const promise = new Promise((done) => {
    resolve = done
  })
  return { promise, resolve }
}
const record = (view) => ({
  schema_version: 1,
  revision: 1,
  discarded: false,
  input: {
    id: crypto.randomUUID(),
    window_id: crypto.randomUUID(),
    kind: 'view_state',
    entity: 'workspace',
    raw: '',
    selections: [],
    pending_submission: null,
    selected_model: null,
    view,
  },
})
function host(view = null) {
  const calls = [],
    rows = new Map()
  const transport = {
    resourceRequest: async (_m, _ws, payload, revision, request) => {
      calls.push({ payload: structuredClone(payload), revision, request })
      const action = payload.action
      if (action.operation === 'latest_view')
        return { ok: true, data: { record: view && record(view) } }
      if (action.operation === 'read') return { ok: true, data: { record: rows.get(action.id) } }
      const next = {
        schema_version: 1,
        revision: revision + 1,
        discarded: false,
        input: structuredClone(action.input),
      }
      rows.set(action.input.id, next)
      return { ok: true, data: next }
    },
  }
  return { transport, calls, rows }
}
const position = { kind: 'text', selections: [{ anchor: 3, head: 9 }], top: 100, left: 0 }

test('restore is read only and a new window forks view metadata without permissions or content', async () => {
  const view = {
    ...emptyWorkspaceView(),
    tabs: [{ file: 'src/a.ts' }, { surface: 'chat' }],
    active: 'chat',
    session: crypto.randomUUID(),
  }
  const fixture = host(view),
    a = new WorkspaceViewCopy(fixture.transport, 'ws', crypto.randomUUID(), null),
    b = new WorkspaceViewCopy(fixture.transport, 'ws', crypto.randomUUID(), null)
  await Promise.all([a.ready, b.ready])
  assert.deepEqual(a.value, view)
  assert.equal(fixture.rows.size, 0)
  a.position('src/a.ts', position)
  b.edit({ active: 'file:src/a.ts' })
  assert.equal(await a.flush(), true)
  assert.equal(await b.flush(), true)
  assert.notEqual(a.copy.input.id, b.copy.input.id)
  assert.equal(a.copy.input.raw, '')
  assert.deepEqual(a.copy.input.selections, [])
  assert.equal(a.copy.input.pending_submission, null)
  assert.equal(a.copy.input.selected_model, null)
  assert.equal(b.value.positions['src/a.ts'], undefined)
  a.dispose()
  b.dispose()
})
test('edits and individual positions made during a slow restore win without losing untouched fields', async () => {
  const gate = deferred(),
    fixture = host()
  const transport = {
    resourceRequest: async (...args) =>
      args[2].action.operation === 'latest_view'
        ? gate.promise
        : fixture.transport.resourceRequest(...args),
  }
  const current = new WorkspaceViewCopy(transport, 'ws', crypto.randomUUID(), null)
  current.edit({ session: '', active: 'chat' })
  current.position('local.ts', position)
  const saved = {
    ...emptyWorkspaceView(),
    tabs: [{ surface: 'version' }],
    session: crypto.randomUUID(),
    positions: { 'saved.ts': position, 'local.ts': { ...position, top: 50 } },
  }
  gate.resolve({ ok: true, data: { record: record(saved) } })
  await current.ready
  assert.equal(current.value.session, '')
  assert.equal(current.value.active, 'chat')
  assert.deepEqual(current.value.tabs, saved.tabs)
  assert.equal(current.value.positions['local.ts'].top, 100)
  assert.equal(current.value.positions['saved.ts'].top, 100)
  assert.equal(await current.flush(), true)
  current.dispose()
})
test('an inherited acknowledged source is read exactly and is never modified by its replacement', async () => {
  const fixture = host(),
    ids = [],
    first = new WorkspaceViewCopy(
      fixture.transport,
      'ws',
      crypto.randomUUID(),
      null,
      () => {},
      (id) => ids.push(id),
    )
  await first.ready
  first.edit({ session: '', tabs: [{ surface: 'chat' }], active: 'chat' })
  await first.flush()
  const old = structuredClone(fixture.rows.get(ids.at(-1))),
    second = new WorkspaceViewCopy(fixture.transport, 'ws', crypto.randomUUID(), ids.at(-1))
  await second.ready
  second.edit({ session: crypto.randomUUID() })
  await second.flush()
  assert.deepEqual(fixture.rows.get(old.input.id), old)
  assert.equal(
    fixture.calls.filter((call) => call.payload.action.operation === 'latest_view').length,
    1,
  )
  assert.equal(second.value.active, 'chat')
  first.dispose()
  second.dispose()
})
test('missing or invalid exact sources do not fall back to another window', async () => {
  for (const data of [
    { record: null },
    { record: record({ ...emptyWorkspaceView(), schema: 99 }) },
    { record: { ...record(emptyWorkspaceView()), discarded: true } },
  ]) {
    let calls = 0
    const current = new WorkspaceViewCopy(
      {
        resourceRequest: async () => {
          calls++
          return { ok: true, data }
        },
      },
      'ws',
      crypto.randomUUID(),
      crypto.randomUUID(),
    )
    await current.ready
    assert.ok(current.error)
    assert.equal(calls, 1)
    assert.equal(current.value.session, null)
    current.dispose()
  }
})
test('future schema, path traversal, model authority, position and tab overflow fail closed', () => {
  for (const update of [
    { schema: 2 },
    { permission_mode: 'full' },
    { tabs: [{ file: '../escape' }] },
    { tabs: [{ surface: 'arbitrary-route' }] },
    { split: 'https://elsewhere' },
    { ratio: NaN },
    { session: 'forged' },
    { positions: { 'a.ts': { ...position, selections: [{ anchor: -1, head: 2 }] } } },
    { tabs: Array.from({ length: 65 }, () => ({ surface: 'chat' })) },
  ])
    assert.equal(parseWorkspaceView({ ...emptyWorkspaceView(), ...update }), null)
  assert.equal(viewPath('src/a.ts'), true)
  assert.equal(viewPath('C:/outside'), false)
  assert.equal(viewPath('a\\b'), false)
})
test('backup failures retain current view and do not change the acknowledged browser pointer', async () => {
  const fixture = host(),
    pointers = [],
    current = new WorkspaceViewCopy(
      {
        resourceRequest: async (...args) =>
          args[2].action.operation === 'backup'
            ? { ok: false, error: 'Disk full' }
            : fixture.transport.resourceRequest(...args),
      },
      'ws',
      crypto.randomUUID(),
      null,
      () => {},
      (id) => pointers.push(id),
    )
  await current.ready
  current.edit({ active: 'chat', tabs: [{ surface: 'chat' }] })
  assert.equal(await current.flush(), false)
  assert.equal(current.value.active, 'chat')
  assert.deepEqual(pointers, [])
  current.dispose()
})
test('repeated unchanged cursor notifications do not create backup work and positions are bounded', async () => {
  const fixture = host(),
    current = new WorkspaceViewCopy(fixture.transport, 'ws', crypto.randomUUID(), null)
  await current.ready
  current.position('src/a.ts', position)
  await current.flush()
  const sequence = current.copy.input.edit_sequence
  for (let i = 0; i < 200; i++) current.position('src/a.ts', position)
  assert.equal(current.copy.input.edit_sequence, sequence)
  for (let i = 0; i < 130; i++) current.position(`file${i}.ts`, position)
  assert.equal(Object.keys(current.value.positions).length, 128)
  assert.match(current.error, /limit/)
  current.dispose()
})
