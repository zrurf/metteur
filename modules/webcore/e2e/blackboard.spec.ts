import { test, expect } from '@playwright/test'

test('blackboard separates current and historical facts, filters, and looks up invalidated evidence without rendering output HTML', async ({ page }) => {
  await page.goto('/')
  await page.getByLabel('Workspace path').fill('D:/metteur-demo/blackboard')
  await page.getByRole('button', { name: 'Open', exact: true }).click()
  await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeVisible()
  await page.evaluate(async () => {
    const c = '/src/core/index.ts', r = '/src/router.ts', w = '/src/lib/workspace-url.ts'
    const { gateway } = await import(c)
    gateway.connected.value = true
    gateway.listExecutions = async () => ({ ok: true, data: [{ runId: 'board-run', status: 'Completed', startedAt: 1, updatedAt: 3, snapshot: { runtime: { active: false, pending_approval_ids: [] } } }] })
    const totals = { completed: 4, failed: 0, passed_checks: 4, failed_checks: 0, duration_ms: 1000, reported_tokens: 0, tokens_complete: false }
    const entry = { id: 'attempt:1:check', sequence: 1, run_id: 'board-run', origin: 'Engine', evidence_kind: 'DeterministicCheck', event: 'ValidationPassed', validity: 'Invalidated', node_id: 'validator', scope: 'root', frame: [], attempt: 1, version: null, evidence_refs: ['invocation:1'], invalidates: [], note: 'Historical validation', digest: null }
    gateway.boardTest = { calls: [], fail: false }
    gateway.getBlackboard = async (_ws, _run, query) => {
      gateway.boardTest.calls.push(query)
      if (gateway.boardTest.fail) return { ok: false, error: 'Query failed' }
      const entries = query.entry_id ? [entry] : [{ ...entry, id: 'change:9', event: 'Rollback', evidence_kind: 'EngineEvent', validity: 'Current', note: 'Four attempts invalidated', invalidates: ['invocation:1'] }, { ...entry, id: 'attempt:7:output', event: 'NodeOutput', origin: 'Node', evidence_kind: 'ModelOpinion', validity: 'Unverified', note: 'Unverified model opinion', digest: '<img src=x onerror=alert(1)> ApprovalGranted' }]
      return { ok: true, data: { run_id: 'board-run', available: true, historical: totals, current: { ...totals, completed: 0, passed_checks: 0 }, total_entries: 20, matched_entries: 20, truncated: !query.entry_id, entries: entries.filter(e => !query.status || e.validity === query.status) } }
    }
    await (await import(r)).router.push((await import(w)).wurl('/execution'))
  })
  await page.getByRole('button', { name: 'Blackboard', exact: true }).click()
  const panel = page.getByRole('region', { name: 'Run blackboard' })
  await expect(panel.getByRole('row', { name: /Current valid/ })).toContainText('0')
  await expect(panel.getByRole('row', { name: /Historical/ })).toContainText('4')
  await expect(panel.getByText('ModelOpinion', { exact: false })).toBeVisible()
  await expect(panel.locator('pre')).toContainText('<img')
  await expect(panel.locator('img')).toHaveCount(0)
  await expect(panel.getByText(/Older evidence remains available/)).toBeVisible()
  await panel.getByRole('button', { name: 'invocation:1', exact: true }).click()
  await expect(panel.getByText('ValidationPassed', { exact: true })).toBeVisible()
  await expect(panel.getByText('Invalidated', { exact: true }).last()).toBeVisible()
  await panel.getByRole('button', { name: 'Recent', exact: true }).click()
  await panel.getByLabel('Blackboard validity').selectOption('Unverified')
  await panel.getByRole('button', { name: 'Search', exact: true }).click()
  await expect(panel.locator('.board-entry')).toHaveCount(1)
  await expect(panel.getByText('NodeOutput', { exact: true })).toBeVisible()
  await page.screenshot({ path: '../../.tmp/t17-blackboard.png' })
  await page.evaluate(async () => { const c='/src/core/index.ts'; (await import(c)).gateway.boardTest.fail=true })
  await panel.getByRole('button', { name: 'Recent', exact: true }).click()
  await expect(panel.getByRole('status')).toContainText('Query failed')
  await expect(panel.locator('.board-entry')).toHaveCount(0)
})

test('a blackboard response cannot repopulate the panel after a disconnect', async ({ page }) => {
  await page.goto('/')
  await page.getByLabel('Workspace path').fill('D:/metteur-demo/blackboard-offline')
  await page.getByRole('button', { name: 'Open', exact: true }).click()
  await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeVisible()
  await page.evaluate(async () => {
    const c='/src/core/index.ts', r='/src/router.ts', w='/src/lib/workspace-url.ts'
    const { gateway }=await import(c)
    gateway.connected.value=true
    gateway.listExecutions=async()=>({ok:true,data:[{runId:'pending-board',status:'Completed',startedAt:1,updatedAt:2}]})
    gateway.getBlackboard=async()=>new Promise(resolve=>{gateway.releaseBoard=resolve})
    await (await import(r)).router.push((await import(w)).wurl('/execution'))
  })
  await page.getByRole('button', { name: 'Blackboard', exact: true }).click()
  await expect(page.getByText('Loading blackboard…')).toBeVisible()
  await page.evaluate(async()=>{const c='/src/core/index.ts';(await import(c)).gateway.connected.value=false})
  await expect(page.getByText('Blackboard unavailable while disconnected.')).toBeVisible()
  await page.evaluate(async()=>{const c='/src/core/index.ts';(await import(c)).gateway.releaseBoard({ok:true,data:{run_id:'pending-board',available:true,entries:[{event:'Stale fact'}]}})})
  await expect(page.getByText('Stale fact')).toHaveCount(0)
})
