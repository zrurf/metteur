// One fixed, synthetic two-file sample on two explicitly supplied production builds.
import { chromium, expect } from '@playwright/test'
import { createClient } from '@connectrpc/connect'
import { createGrpcWebTransport } from '@connectrpc/connect-web'
import { Daemon } from '../src/gen/metteur_pb.ts'
import { mkdir, writeFile, readFile } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { hostname, cpus, totalmem } from 'node:os'
import assert from 'node:assert/strict'

const [baseline, candidate, destination] = process.argv.slice(2)
if (!baseline || !candidate || !destination)
  throw Error('baseline URL, candidate URL and new output directory required')
const output = resolve(destination)
await mkdir(output, { recursive: false })
const result = {
  started: new Date().toISOString(),
  machine: {
    host: hostname(),
    platform: process.platform,
    cpu: cpus()[0]?.model,
    memory: totalmem(),
  },
  sample: {
    viewport: [1360, 900],
    files: 2,
    tabSamples: 20,
    keySamples: 30,
    startup:
      'fresh browser context to visible file tree; already-running native host; not cold daemon startup',
  },
  measurements: [],
}
const p95 = (values) => [...values].sort((a, b) => a - b)[Math.ceil(values.length * 0.95) - 1]
const browser = await chromium.launch({ headless: true })
try {
  const reuse = process.argv[5]
  if (reuse) {
    const previous = JSON.parse(await readFile(reuse, 'utf8'))
    assert.equal(previous.machine.host, result.machine.host)
    assert.equal(previous.sample.viewport.join('x'), result.sample.viewport.join('x'))
    const original = previous.measurements.find((measurement) => measurement.name === 'baseline')
    assert.equal(original.baseURL, baseline)
    result.measurements.push({ ...original, reusedFrom: reuse })
  }
  for (const [name, baseURL] of [
    ['baseline', baseline],
    ['candidate', candidate],
  ].filter(([kind]) => !reuse || kind === 'candidate')) {
    assert.equal(new URL(baseURL).hostname, '127.0.0.1')
    const workspace = join(output, name)
    await mkdir(workspace)
    await writeFile(
      join(workspace, 'alpha.ts'),
      'export const greeting = "中文 sample"\n'.repeat(100),
    )
    await writeFile(join(workspace, 'beta.txt'), 'second fixed sample\n'.repeat(100))
    const context = await browser.newContext({
      viewport: { width: 1360, height: 900 },
      serviceWorkers: 'block',
    })
    const page = await context.newPage(),
      errors = []
    page.on('pageerror', (error) => errors.push(error.message))
    const requests = [],
      started = performance.now()
    page.on('request', (request) => {
      if (request.url().includes('/metteur.Daemon/'))
        requests.push({ path: new URL(request.url()).pathname, at: performance.now() })
    })
    await page.goto(`${baseURL}/?open=${encodeURIComponent(workspace)}`)
    const tree = page.getByTestId('file-tree')
    await expect(tree).toBeVisible()
    const startupMs = performance.now() - started
    for (const file of ['alpha.ts', 'beta.txt']) {
      await tree.getByRole('button', { name: file, exact: true }).click()
      await expect(page.locator('.monaco-editor .view-lines')).toContainText(
        file === 'alpha.ts' ? 'greeting' : 'second fixed sample',
      )
    }
    const tabs = []
    for (let i = 0; i < 20; i++) {
      const file = i % 2 ? 'beta.txt' : 'alpha.ts',
        began = performance.now()
      await page.getByTestId('tab').filter({ hasText: file }).click()
      await expect(page.locator('.monaco-editor .view-lines')).toContainText(
        i % 2 ? 'second fixed sample' : 'greeting',
      )
      tabs.push(performance.now() - began)
    }
    await page.getByTestId('tab').filter({ hasText: 'alpha.ts' }).click()
    await page.locator('.monaco-editor .view-lines').click()
    await page.keyboard.press('ControlOrMeta+End')
    const keys = []
    for (let i = 1; i <= 30; i++) {
      const began = performance.now()
      await page.keyboard.insertText('x')
      await expect(page.locator('.monaco-editor .view-lines')).toContainText('x'.repeat(i))
      keys.push(performance.now() - began)
    }
    await expect(page.getByText('Backed Up on Host', { exact: true })).toBeVisible()
    const cdp = await context.newCDPSession(page),
      heap = await cdp.send('Runtime.getHeapUsage')
    const nav = page.getByRole('navigation', { name: 'Main navigation' })
    await nav.getByRole('button', { name: 'Tasks', exact: true }).click()
    await expect(page.getByRole('button', { name: 'Refresh tasks', exact: true })).toBeVisible()
    await page.waitForTimeout(500)
    const idleStart = performance.now()
    await page.waitForTimeout(10000)
    const idle = requests.filter((r) => r.at >= idleStart)
    const measurement = {
      name,
      baseURL,
      workspace,
      startupMs,
      tabs,
      tabP95: p95(tabs),
      keys,
      keyP95: p95(keys),
      heap,
      idleSeconds: 10,
      idleRequests: idle,
      requests: requests.length,
      errors,
    }
    result.measurements.push(measurement)
    await page.screenshot({ path: join(output, `${name}.png`) })
    assert.deepEqual(errors, [])
    await context.close()
    const client = createClient(Daemon, createGrpcWebTransport({ baseUrl: baseURL }))
    await client.closeWorkspace({ path: workspace })
    await writeFile(join(output, 'measurements.json'), JSON.stringify(result, null, 2))
  }
  const [before, after] = result.measurements
  result.thresholds = {
    tabP95: Math.max(500, before.tabP95 * 1.25),
    keyP95: Math.max(100, before.keyP95 * 1.25),
    startupMs: Math.max(5000, before.startupMs * 1.25),
  }
  result.checks = {
    tab: after.tabP95 <= result.thresholds.tabP95,
    key: after.keyP95 <= result.thresholds.keyP95,
    startup: after.startupMs <= result.thresholds.startupMs,
    idleBusinessPolling:
      after.idleRequests.filter((r) =>
        /QueryContinuations|GetConciergeState|GetOversightReviews/.test(r.path),
      ).length === 0,
  }
  assert(Object.values(result.checks).every(Boolean), JSON.stringify(result.checks))
  console.log(
    JSON.stringify({
      thresholds: result.thresholds,
      measurements: result.measurements.map((m) => ({
        name: m.name,
        startupMs: m.startupMs,
        tabP95: m.tabP95,
        keyP95: m.keyP95,
        heap: m.heap,
        idleRequests: m.idleRequests,
      })),
      checks: result.checks,
    }),
  )
} catch (error) {
  result.error = String(error)
  process.exitCode = 1
} finally {
  result.finished = new Date().toISOString()
  await writeFile(join(output, 'measurements.json'), JSON.stringify(result, null, 2))
  await browser.close()
}
