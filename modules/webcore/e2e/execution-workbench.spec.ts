import { test, expect } from '@playwright/test'

test('concurrent node and supervisor approvals keep distinct decisions and withdraw expired requests', async ({ page }) => {
  await page.goto('/')
  await page.getByLabel('Workspace path').fill('D:/metteur-demo/approval-queue')
  await page.getByRole('button', { name: 'Open', exact: true }).click()
  await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeVisible()
  const result = await page.evaluate(async () => {
    const c = '/src/core/index.ts', e = '/src/stores/execution.ts'
    const g = (await import(c)).gateway, execution = (await import(e)).useExecutionStore()
    let active = false, finish!: () => void
    const pending = ['node-approval', 'supervisor-approval'], decisions: string[] = []
    g.listExecutions = async () => ({ ok: true, data: active ? [{ runId: 'parallel', status: 'Running', startedAt: 1, updatedAt: Date.now(), snapshot: { runtime: { active: true, pending_approval_ids: pending } } }] : [] })
    g.executeBlueprint = async (_ws, _bp, callback) => {
      active = true
      for (const id of pending) callback({ nodeId: 'node', kind: 'approval_request', message: id, detail: { run_id: 'parallel', request_type: id.startsWith('supervisor') ? 'replan_proposal' : 'sandbox', summary: id } })
      await new Promise<void>(resolve => { finish = resolve })
      return { ok: true }
    }
    g.respondApproval = async (_ws, id, allow) => { decisions.push(`${id}:${allow}`); pending.splice(pending.indexOf(id), 1); return { ok: true } }
    const task = execution.run('graph', { id: 'graph', name: 'Graph', nodes: [], edges: [] }, 'graph.blueprint')
    await new Promise(resolve => setTimeout(resolve, 0))
    const first = execution.approval?.id
    await execution.respond(false)
    const second = execution.approval?.id
    pending.length = 0
    await execution.reconcile()
    const expired = execution.approval === null
    finish(); await task
    return { first, second, expired, decisions }
  })
  expect(result).toEqual({ first: 'node-approval', second: 'supervisor-approval', expired: true, decisions: ['node-approval:false'] })
})

test('Execution is a workspace tab with reusable bounded panels and a disabled concierge before a run', async ({ page }) => {
  await page.goto('/')
  await page.getByLabel('Workspace path').fill('D:/metteur-demo/workbench')
  await page.getByRole('button', { name: 'Open', exact: true }).click()
  await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeVisible()
  await page.getByRole('button', { name: 'Execution', exact: true }).click()
  await expect(page).toHaveURL(/\/work\/[^/]+\/execution$/)
  await expect(page.getByRole('button', { name: 'Select blueprint', exact: true })).toBeVisible()
  await expect(page.getByRole('region', { name: 'Run concierge' }).getByRole('textbox', { name: 'Message', exact: true })).toBeDisabled()
  const divider = page.getByRole('separator', { name: 'Resize supervision' })
  const start = Number(await divider.getAttribute('aria-valuenow'))
  await divider.focus(); await page.keyboard.press('ArrowLeft')
  await expect(divider).toHaveAttribute('aria-valuenow', String(start + 20))
  await page.getByLabel('Close supervision', { exact: true }).click()
  await page.getByLabel('Supervision panel', { exact: true }).click()
  await expect(divider).toHaveAttribute('aria-valuenow', String(start + 20))
  await divider.dblclick(); await expect(divider).toHaveAttribute('aria-valuenow', String(start))
  const bottom = page.getByRole('separator', { name: 'Resize details' })
  await bottom.focus(); await page.keyboard.press('ArrowUp')
  await expect(bottom).toHaveAttribute('aria-valuenow', '220')
  await page.getByLabel('Close details', { exact: true }).click()
  await page.getByLabel('Node details panel', { exact: true }).click()
  await expect(bottom).toHaveAttribute('aria-valuenow', '220')
  await page.getByRole('button', { name: 'Execution', exact: true }).click()
  await expect(page.getByTestId('tab-active').filter({ hasText: 'Execution' })).toHaveCount(1)
  await page.setViewportSize({ width: 600, height: 650 })
  await expect(page.getByRole('button', { name: 'Run', exact: true })).toBeVisible()
})

test('closing and reopening Execution never launches or cancels a run; workspace state is isolated', async ({ page }) => {
  await page.goto('/')
  await page.getByLabel('Workspace path').fill('D:/metteur-demo/lifecycle')
  await page.getByRole('button', { name: 'Open', exact: true }).click()
  await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeVisible()
  const result = await page.evaluate(async () => {
    const core = '/src/core/index.ts', es = '/src/stores/execution.ts', ts = '/src/stores/tabs.ts', ws = '/src/stores/workspace.ts'
    const { gateway } = await import(core), execution = (await import(es)).useExecutionStore(), tabs = (await import(ts)).useTabsStore()
    let launches = 0, cancels = 0
    gateway.executeBlueprint = async () => { launches++; return { ok: true } }
    gateway.cancel = async () => { cancels++; return { ok: true } }
    gateway.listExecutions = async () => ({ ok: true, data: [] })
    execution.status = 'running'
    execution.blueprint = { id: 'active', name: 'Active', nodes: [], edges: [] }
    tabs.openSurface('execution'); tabs.openSurface('execution'); tabs.close('execution'); tabs.openSurface('execution')
    await execution.reconcile()
    const retained = execution.blueprint.id
    const count = tabs.items.filter((t: { id: string }) => t.id === 'execution').length
    const workspace = (await import(ws)).useWorkspaceStore()
    workspace.active = { ...workspace.active, path: 'D:/metteur-demo/other' }
    await new Promise(resolve => setTimeout(resolve, 0))
    return { launches, cancels, retained, count, cleared: execution.blueprint === null }
  })
  expect(result).toEqual({ launches: 0, cancels: 0, retained: 'active', count: 1, cleared: true })
})


test('two launch entry points converge and an existing daemon run wins without replacement', async ({ page }) => {
  await page.goto('/')
  await page.getByLabel('Workspace path').fill('D:/metteur-demo/admission')
  await page.getByRole('button', { name: 'Open', exact: true }).click()
  await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeVisible()
  const result = await page.evaluate(async () => {
    const core = '/src/core/index.ts', es = '/src/stores/execution.ts'
    const { gateway } = await import(core), execution = (await import(es)).useExecutionStore()
    let launches = 0, release!: () => void
    gateway.listExecutions = async () => ({ ok: true, data: [] })
    gateway.executeBlueprint = async () => { launches++; await new Promise<void>(r => { release = r }); return { ok: true } }
    const graph = { id: 'chosen', name: 'Chosen', nodes: [], edges: [] }
    const first = execution.run(graph.id, graph, 'chosen.blueprint')
    const second = execution.run(graph.id, graph, 'chosen.blueprint')
    await new Promise(resolve => setTimeout(resolve, 0))
    release(); await Promise.all([first, second])
    gateway.listExecutions = async () => ({ ok: true, data: [{ runId: 'external', blueprintId: 'other', status: 'Running', snapshot: { executed: [], pending: [], runtime: { active: true, pause_requested: false, cancel_requested: false, pending_approval_ids: [] }, view: { root: { ...graph, id: 'other' }, graphs: {}, invocations: [], edges: [], sequence: 0 } } }] })
    gateway.loadBlueprint = async () => ({ ok: true, data: { ...graph, id: 'other' } })
    await execution.run(graph.id, graph)
    return { launches, id: execution.runId, blueprint: execution.blueprint.id, error: execution.error }
  })
  expect(result).toMatchObject({ launches: 1, id: 'external', blueprint: 'other' })
  expect(result.error).toContain('already active')
})
