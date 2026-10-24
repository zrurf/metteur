import { chromium, expect } from '@playwright/test'
import { readFile, writeFile } from 'node:fs/promises'
const [readyPath, output] = process.argv.slice(2)
const ready = JSON.parse(await readFile(readyPath, 'utf8'))
const browser = await chromium.launch()
const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } })
const report = { checks: [], errors: [] },
  writes = [],
  executions = []
page.on('pageerror', (e) => report.errors.push(String(e)))
page.on('request', (r) => {
  if (/RecordWorkflowReview$/.test(r.url())) writes.push(r.postDataBuffer()?.toString('base64'))
  if (/ExecuteBlueprint$|SendChat$/.test(r.url())) executions.push(r.url())
})
async function openReview() {
  await page
    .getByRole('navigation', { name: 'Main navigation' })
    .getByRole('button', { name: 'Execution', exact: true })
    .click()
  await page.getByRole('button', { name: 'Result', exact: true }).click()
  await page.getByRole('button', { name: 'Review delivery', exact: true }).click()
}
async function decide(note) {
  await page.getByRole('button', { name: 'Review requirement', exact: true }).last().click()
  await page.getByRole('textbox').last().fill(note)
  await page.getByRole('button', { name: 'Request changes', exact: true }).click()
  await expect(page.getByText(/Review outcome unknown/)).toBeVisible()
}
try {
  await page.goto('http://127.0.0.1:19111/?open=' + encodeURIComponent(ready.workspace))
  await openReview()
  const route = '**/metteur.Daemon/RecordWorkflowReview'
  await page.route(
    route,
    async (r) => {
      await r.fetch()
      await r.abort('failed')
    },
    { times: 1 },
  )
  await decide('Lost ACK: reconcile the committed original request')
  await page.getByRole('button', { name: 'Check original receipt', exact: true }).click()
  await expect(page.getByText(/— Lost ACK: reconcile/)).toBeVisible()
  expect(writes).toHaveLength(1)
  report.checks.push(
    'Committed lost ACK reconciles through receipt lookup without another record request',
  )
  await page.route(route, (r) => r.abort('failed'), { times: 1 })
  await decide('Uncommitted request: recover its identity after refresh')
  expect(writes).toHaveLength(2)
  await page.reload()
  await openReview()
  await expect(page.getByText(/Recovered an unresolved review request/)).toBeVisible()
  await page.getByRole('button', { name: 'Check original receipt', exact: true }).click()
  await expect(page.getByText(/— Uncommitted request: recover/)).toBeVisible()
  expect(writes).toHaveLength(3)
  expect(writes[2]).toBe(writes[1])
  expect(executions).toHaveLength(0)
  report.checks.push(
    'Refresh automatically restores the unresolved host copy; missing receipt resubmits identical bytes and creates one decision without execution',
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
