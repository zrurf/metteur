import { chromium, expect } from '@playwright/test'
import { createClient } from '@connectrpc/connect'
import { createGrpcWebTransport } from '@connectrpc/connect-web'
import { Daemon } from '../src/gen/metteur_pb.ts'
import { readFile, writeFile } from 'node:fs/promises'
const [readyPath, output, run] = process.argv.slice(2),
  ready = JSON.parse(await readFile(readyPath, 'utf8'))
const browser = await chromium.launch(),
  page = await browser.newPage({ viewport: { width: 1440, height: 1000 } })
const rpc = createClient(Daemon, createGrpcWebTransport({ baseUrl: ready.baseURL }))
const report = { checks: [], errors: [], run },
  writes = []
page.on('pageerror', (e) => report.errors.push(String(e)))
page.on('request', (r) => {
  if (/ExecuteBlueprint$|SendChat$/.test(r.url())) writes.push(r.url())
})
try {
  await rpc.openWorkspace({ path: ready.workspace })
  const record = (await rpc.listExecutions({ workspacePath: ready.workspace })).executions.find(
    (e) => e.runId === run,
  )
  expect(record?.status.toLowerCase()).toBe('completed')
  await page.goto(ready.baseURL + '/?open=' + encodeURIComponent(ready.workspace))
  await page
    .getByRole('navigation', { name: 'Main navigation' })
    .getByRole('button', { name: 'Execution', exact: true })
    .click()
  await page.getByRole('button', { name: 'Result', exact: true }).click()
  await page.getByRole('button', { name: 'Review delivery', exact: true }).click()
  await expect(page.getByText('Passed', { exact: true }).first()).toBeVisible()
  expect(writes).toHaveLength(0)
  report.checks.push(
    'The existing UI-confirmed run completed; reopening its real result shows passed acceptance evidence without another execution',
  )
} catch (e) {
  report.failure = String(e)
  await writeFile(output + '.page.txt', await page.locator('body').innerText())
}
await page.screenshot({ path: output + '.png' })
await writeFile(output, JSON.stringify(report, null, 2))
await browser.close()
console.log(JSON.stringify(report))
if (report.failure || report.errors.length) process.exitCode = 1
