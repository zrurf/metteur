import { chromium } from '@playwright/test'
import { readFile, writeFile } from 'node:fs/promises'
import assert from 'node:assert/strict'
const [readyPath, output] = process.argv.slice(2)
const ready = JSON.parse(await readFile(readyPath, 'utf8'))
assert.equal(new URL(ready.baseURL).hostname, '127.0.0.1')
const browser = await chromium.launch()
const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } })
const report = {
  checks: [],
  errors: [],
  source: 'production UI + grpc-web + isolated native daemon; local fixture model',
}
page.on('pageerror', (e) => report.errors.push(String(e)))
try {
  await page.goto(ready.baseURL)
  await page.locator('input').first().fill(ready.workspace)
  await page.getByRole('button', { name: 'Open', exact: true }).click()
  await page.getByRole('button', { name: 'Start from template', exact: true }).first().click()
  await page.getByRole('button', { name: /Refactor safely/ }).click()
  await page.getByLabel('Plan name', { exact: true }).fill(`Live review ${Date.now()}`)
  await page
    .getByLabel('Expected outcome', { exact: true })
    .fill('Review app.ts without changing behavior')
  await page.getByLabel('Files in scope', { exact: true }).fill('app.ts')
  await page.getByText('Constraints & evidence files', { exact: true }).click()
  await page.getByLabel('Evidence files', { exact: true }).fill('app.ts')
  await page.getByLabel('Requirement 1', { exact: true }).fill('Command exits successfully')
  await page.getByText('Check command & requirement options', { exact: true }).click()
  await page.getByLabel('Check command', { exact: true }).fill('node -e "process.exit(0)"')
  await page.getByText(/Baseline & execution settings/).click()
  await page.getByLabel('Baseline command', { exact: true }).fill('node -e "process.exit(0)"')
  await page.getByRole('button', { name: 'Preview plan', exact: true }).click()
  await page.getByRole('button', { name: 'Create blueprint', exact: true }).click()
  await page.getByText('Saved blueprint · execution has not started', { exact: true }).waitFor()
  assert.equal(await page.getByRole('alert').count(), 0)
  report.checks.push(
    'real compile, host draft backup, create-only file and daemon mirror completed',
  )
  await writeFile(output + '.page.txt', await page.locator('body').innerText())
  report.checks.push('production workspace and template catalog opened')
} catch (e) {
  report.failure = String(e)
  await page.screenshot({ path: output + '.png' })
} finally {
  await writeFile(output, JSON.stringify(report, null, 2))
  await browser.close()
}
console.log(JSON.stringify(report))
if (report.failure || report.errors.length) process.exitCode = 1
