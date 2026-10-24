import { chromium, expect } from '@playwright/test'
import { readFile, writeFile, mkdir } from 'node:fs/promises'
const [readyPath, output, workspace] = process.argv.slice(2)
const ready = JSON.parse(await readFile(readyPath, 'utf8'))
await mkdir(workspace, { recursive: true })
const browser = await chromium.launch(), page = await browser.newPage({ viewport: { width: 1440, height: 1000 } })
const report = { checks: [], errors: [], baseURL: ready.baseURL }, writes = []
page.on('pageerror', e => report.errors.push(String(e)))
page.on('request', r => { if (/ExecuteBlueprint$|SendChat$|SaveBlueprint$|RecordWorkflowReview$/.test(r.url())) writes.push(r.url()) })
try {
  await page.goto(ready.baseURL + '/?open=' + encodeURIComponent(workspace))
  await page.getByRole('navigation', { name: 'Main navigation' }).getByRole('button', { name: 'Chat', exact: true }).click()
  await page.getByRole('button', { name: 'Start from template', exact: true }).click()
  for (const name of ['Fix a bug', 'Add tests', 'Refactor safely', 'Add a small feature']) await expect(page.getByRole('button', { name: new RegExp(name) })).toBeVisible()
  expect(writes).toHaveLength(0)
  const usage = await (await fetch(ready.providerUrl + '/fixture')).json()
  expect(usage.calls).toBe(0)
  report.checks.push('Formal production assets open the real four-template catalog; original workspace data untouched, no run/send/save/review/model call')
} catch (error) { report.failure = String(error); await writeFile(output + '.page.txt', await page.locator('body').innerText()) }
await page.screenshot({ path: output + '.png' }); await writeFile(output, JSON.stringify(report, null, 2)); await browser.close()
console.log(JSON.stringify(report)); if (report.failure || report.errors.length) process.exitCode = 1
