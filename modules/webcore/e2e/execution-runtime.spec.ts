import { test, expect } from '@playwright/test'

test('runtime graph follows recorded paths, stops motion offline, and uses live controls with exact run identity', async ({ page }) => {
  await page.goto('/')
  await page.getByLabel('Workspace path').fill('D:/metteur-demo/runtime')
  await page.getByRole('button', { name: 'Open', exact: true }).click()
  await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeVisible()
  await page.evaluate(async () => {
    const core = '/src/core/index.ts', rt = '/src/router.ts', url = '/src/lib/workspace-url.ts'
    const { gateway } = await import(core)
    gateway.connected.value = true
    const node = (id: string, x: number, y: number) => ({ id, title: id, type: 'Start', nodeType: 'Event', position: { x, y }, category: 'event', inputs: [{ id: id+'-in', name: 'In', kind: 'exec-in', type: 'void' }], outputs: [{ id: id+'-out', name: 'Out', kind: 'exec-out', type: 'void' }] })
    const graph = { id: 'graph', name: 'Recorded graph', nodes: [node('branch',0,0),node('taken',300,0),node('untaken',300,150)], edges: [{ id: 'yes', source: 'branch', sourceHandle: 'branch-out', target: 'taken', targetHandle: 'taken-in' }, { id: 'no', source: 'branch', sourceHandle: 'branch-out', target: 'untaken', targetHandle: 'untaken-in' }] }
    const invocation = (node_id: string, sequence: number, status: string) => ({ node_id, sequence, scope: 'graph', frame: [], attempt: 1, current: true, inputs: { Condition: true }, outputs: {}, started_at: 1000, finished_at: status === 'Completed' ? 1500 : null, status, messages: [] })
    gateway.runtimeTest = { offline: false, calls: [] as string[], run: { runId: 'real-run', blueprintId: 'graph', status: 'Running', startedAt: 1000, updatedAt: 2000, snapshot: { executed: ['branch'], pending: [], view: { root: graph, graphs: {}, sequence: 2, invocations: [invocation('branch',1,'Completed'), invocation('taken',2,'Running')], edges: [{ edge_id: 'yes', scope: 'graph', frame: [], sequence: 1 }] }, runtime: { active: true, pause_requested: false, cancel_requested: false, pending_approval_ids: [] } } } }
    gateway.listExecutions = async () => gateway.runtimeTest.offline ? { ok: false, error: 'Connection lost' } : { ok: true, data: [structuredClone(gateway.runtimeTest.run)] }
    gateway.continueExecution = async () => { throw new Error('Checkpoint recovery must not be called') }
    gateway.pause = async (_ws: string, id: string) => { gateway.runtimeTest.calls.push('pause:'+id); gateway.runtimeTest.run.snapshot.runtime.pause_requested = true; return { ok: true } }
    gateway.resume = async (_ws: string, id: string) => { gateway.runtimeTest.calls.push('resume:'+id); gateway.runtimeTest.run.snapshot.runtime.pause_requested = false; return { ok: true } }
    gateway.cancel = async (_ws: string, id: string) => { gateway.runtimeTest.calls.push('cancel:'+id); return { ok: false, error: 'Control rejected' } }
    await (await import(rt)).router.push((await import(url)).wurl('/execution'))
  })
  await expect(page.locator('.vue-flow__node')).toHaveCount(3)
  await expect(page.locator('.vue-flow__edge.animated')).toHaveCount(1)
  await expect(page.locator('.vue-flow__edge.is-traversed')).toHaveCount(1)
  await page.getByRole('button', { name: 'Pause', exact: true }).click()
  await expect(page.locator('.vue-flow__edge.animated')).toHaveCount(0)
  await page.getByRole('button', { name: 'Continue', exact: true }).click()
  await expect(page.locator('.vue-flow__edge.animated')).toHaveCount(1)
  await page.getByRole('button', { name: 'Stop', exact: true }).click()
  await expect(page.locator('.run-header').getByRole('alert')).toContainText('Control rejected')
  await page.locator('.vue-flow__node').filter({ hasText: 'taken' }).filter({ hasNotText: 'untaken' }).click()
  await expect(page.getByText('Attempt 1', { exact: false })).toBeVisible()
  await page.screenshot({ path: '../../.tmp/u02-runtime.png' })
  await page.evaluate(async () => { const p = '/src/core/index.ts'; (await import(p)).gateway.runtimeTest.offline = true })
  await expect(page.getByText('State unknown', { exact: true })).toBeVisible()
  await expect(page.locator('.vue-flow__edge.animated')).toHaveCount(0)
  const calls = await page.evaluate(async () => { const p = '/src/core/index.ts'; return (await import(p)).gateway.runtimeTest.calls })
  expect(calls).toEqual(['pause:real-run', 'resume:real-run', 'cancel:real-run'])
})

test('late query responses cannot replace a newer workspace or newer run snapshot', async ({ page }) => {
  await page.goto('/')
  await page.getByLabel('Workspace path').fill('D:/metteur-demo/query-order')
  await page.getByRole('button', { name: 'Open', exact: true }).click()
  await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeVisible()
  const result = await page.evaluate(async () => {
    const c = '/src/core/index.ts', s = '/src/stores/execution.ts', w = '/src/stores/workspace.ts'
    const { gateway } = await import(c), store = (await import(s)).useExecutionStore()
    let release!: (r: unknown) => void
    gateway.listExecutions = async () => new Promise(r => { release = r })
    const old = store.reconcile()
    gateway.listExecutions = async () => ({ ok: true, data: [{ runId: 'new', status: 'Completed', updatedAt: 20 }] })
    await store.reconcile(); release({ ok: true, data: [{ runId: 'old', status: 'Running', updatedAt: 10 }] }); await old
    const id = store.runId
    gateway.listExecutions = async () => new Promise(r => { release = r })
    const pending = store.reconcile()
    const ws = (await import(w)).useWorkspaceStore(); ws.active = { ...ws.active, path: 'another' }
    release({ ok: true, data: [{ runId: 'foreign', status: 'Running' }] }); await pending
    return { id, cleared: store.runId === null }
  })
  expect(result).toEqual({ id: 'new', cleared: true })
})


test('stream duplicates, late sequence numbers, foreign runs and old workspaces cannot alter current events', async ({ page }) => {
  await page.goto('/')
  await page.getByLabel('Workspace path').fill('D:/metteur-demo/events')
  await page.getByRole('button', { name: 'Open', exact: true }).click()
  await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeVisible()
  const result = await page.evaluate(async () => {
    const c = '/src/core/index.ts', s = '/src/stores/execution.ts', w = '/src/stores/workspace.ts'
    const { gateway } = await import(c), store = (await import(s)).useExecutionStore()
    let send!: (e: unknown) => void, finish!: (r: unknown) => void
    gateway.listExecutions = async () => ({ ok: true, data: [] })
    gateway.executeBlueprint = async (_ws: string, _id: string, callback: typeof send) => { send = callback; return new Promise(r => { finish = r }) }
    const running = store.run('graph')
    await new Promise(r => setTimeout(r, 0))
    const event = (sequence: number, run_id = 'run') => ({ nodeId: 'node', kind: 'message', message: String(sequence), detail: { run_id, stream_id: 'stream', sequence } })
    send(event(2)); send(event(2)); send(event(1)); send(event(3, 'foreign')); send(event(3))
    const messages = store.events.map(e => e.message)
    const ws = (await import(w)).useWorkspaceStore(); ws.active = { ...ws.active, path: 'next-workspace' }
    send(event(4)); finish({ ok: true }); await running
    return { messages, empty: store.events.length === 0 && store.runId === null }
  })
  expect(result).toEqual({ messages: ['2', '3'], empty: true })
})
