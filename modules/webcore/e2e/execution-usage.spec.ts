import { test, expect } from '@playwright/test'

test('usage is token-weighted and unknown, zero, failed and late snapshots remain distinct', async ({ page }) => {
  await page.goto('/')
  await page.getByLabel('Workspace path').fill('D:/metteur-demo/usage')
  await page.getByRole('button', { name: 'Open', exact: true }).click()
  await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeVisible()
  await page.evaluate(async () => {
    const c = '/src/core/index.ts', r = '/src/router.ts', u = '/src/lib/workspace-url.ts'
    const { gateway } = await import(c)
    gateway.connected.value = true
    const model = (name: string, inputTokens: number, cachedInputTokens: number) => ({ model: name, calls: 1, inputTokens, cachedInputTokens, outputTokens: 500, reasoningTokens: 30, cacheWriteInputTokens: 20, costMicros: 100, tokensComplete: true, cacheComplete: true, costComplete: true })
    gateway.usageTest = { id: 'first', error: false, delayed: false, release: null, data: { currency: 'USD', totalCostMicros: 200, oversight: { limit: 5000, charged: 4500, warning: true, exhausted: false, concierge_available: true, calls: [{ caller: 'concierge', charged: 4500, state: 'usage_unknown', cost_micros: null, currency: '' }] }, models: [model('small', 100, 100), model('large', 900, 0)] } }
    gateway.listExecutions = async () => ({ ok: true, data: [{ runId: gateway.usageTest.id, blueprintId: 'g', status: 'Completed', startedAt: 1, updatedAt: 2 }] })
    gateway.getExecutionUsage = async (_ws: string, id: string) => {
      if (id === 'second') return { ok: true, data: { currency: 'USD', totalCostMicros: 0, models: [] } }
      if (gateway.usageTest.delayed) return new Promise(resolve => { gateway.usageTest.release = resolve })
      return gateway.usageTest.error ? { ok: false, error: 'Usage query failed' } : { ok: true, data: structuredClone(gateway.usageTest.data) }
    }
    await (await import(r)).router.push((await import(u)).wurl('/execution'))
  })
  await page.getByRole('button', { name: 'Usage', exact: true }).click()
  const card = page.locator('.cache-card')
  await expect(card).toHaveText('Cache hit rate10%')
  await expect(page.getByRole('progressbar', { name: 'Cache hit rate' })).toHaveAttribute('aria-valuenow', '10')
  await expect(page.locator('.usage-grid').getByText('1,000', { exact: true })).toHaveCount(2)
  await expect(page.getByText('USD 0.000200', { exact: true })).toBeVisible()
  await expect(page.getByText('4,500 / 5,000', { exact: true })).toBeVisible()
  await expect(page.getByText('Budget warning', { exact: true })).toBeVisible()
  await expect(page.getByText('4,500 tokens (estimated / reserved)', { exact: true })).toBeVisible()
  await expect(page.getByText('Historical accounting:', { exact: false })).toBeVisible()
  for (let i = 0; i < 5; i++) await page.getByRole('separator', { name: 'Resize details', exact: true }).press('ArrowUp')
  await page.screenshot({ path: '../../.tmp/u03-usage.png' })
  // Successive snapshots replace totals; they never add a second copy.
  await expect.poll(async () => page.locator('.cache-card strong').innerText()).toBe('10%')
  await page.evaluate(async () => { const c = '/src/core/index.ts'; const t = (await import(c)).gateway.usageTest; t.data.models.forEach((m: { cachedInputTokens: number }) => { m.cachedInputTokens = 0 }) })
  await expect(card).toHaveText('Cache hit rate0%')
  await page.evaluate(async () => { const c = '/src/core/index.ts'; (await import(c)).gateway.usageTest.data.models[1].cacheComplete = false })
  await expect(card).toHaveText('Cache hit rateUnavailable')
  await expect(page.getByRole('progressbar', { name: 'Cache hit rate' })).not.toHaveAttribute('aria-valuenow')
  await page.evaluate(async () => { const c = '/src/core/index.ts'; (await import(c)).gateway.usageTest.error = true })
  await expect(page.locator('.details-content').getByRole('status')).toContainText('Usage query failed')
  await expect(card).toHaveText('Cache hit rateUnavailable')
  await page.evaluate(async () => { const c = '/src/core/index.ts'; const t = (await import(c)).gateway.usageTest; t.error = false; t.delayed = true })
  await expect.poll(() => page.evaluate(async () => { const c = '/src/core/index.ts'; return !!(await import(c)).gateway.usageTest.release })).toBe(true)
  await page.evaluate(async () => { const c = '/src/core/index.ts', s = '/src/stores/execution.ts'; (await import(c)).gateway.usageTest.id = 'second'; (await import(s)).useExecutionStore().runId = 'second'; await (await import(s)).useExecutionStore().reconcile() })
  await page.evaluate(async () => { const c = '/src/core/index.ts'; const t = (await import(c)).gateway.usageTest; t.data.models.forEach((m: { cacheComplete: boolean }) => { m.cacheComplete = true }); t.release({ ok: true, data: t.data }) })
  await expect(card).toHaveText('Cache hit rateUnavailable')
  await expect(page.getByText('USD 0.000200', { exact: true })).toHaveCount(0)
})

test('cache aggregation handles zero input, missing fields, cache writes and mixed coverage', async ({ page }) => {
  await page.goto('/')
  const result = await page.evaluate(async () => {
    const path = '/src/core/execution-usage.ts'; const { executionUsage } = await import(path)
    const model = { model: 'm', calls: 1, inputTokens: 100, cachedInputTokens: 20, cacheWriteInputTokens: 80, outputTokens: 900, reasoningTokens: 100, costMicros: 0, tokensComplete: true, cacheComplete: true, costComplete: false }
    const get = (models: unknown[]) => executionUsage({ currency: 'USD', totalCostMicros: 0, models })
    return { known: get([model]).cacheHitRate, cost: get([model]).estimatedCost, output: get([model]).output,
      zero: get([{ ...model, inputTokens: 0, cachedInputTokens: 0 }]).cacheHitRate,
      unknown: get([{ ...model, cacheComplete: undefined }]).cacheHitRate,
      partial: get([model, { ...model, tokensComplete: false }]).cacheHitRate,
      empty: get([]).cacheHitRate, invalid: get([{ ...model, cachedInputTokens: 101 }]).cacheHitRate }
  })
  expect(result).toEqual({ known: 0.2, cost: null, output: 900, zero: null, unknown: null, partial: null, empty: null, invalid: null })
})
