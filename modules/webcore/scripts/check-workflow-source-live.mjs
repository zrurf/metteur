import { chromium, expect } from '@playwright/test'
import { readFile, writeFile } from 'node:fs/promises'
const [readyPath, output] = process.argv.slice(2),
  ready = JSON.parse(await readFile(readyPath, 'utf8'))
const browser = await chromium.launch(),
  page = await browser.newPage({ viewport: { width: 1440, height: 1000 } })
const report = { checks: [], errors: [] },
  sends = [],
  runs = []
page.on('pageerror', (e) => report.errors.push(String(e)))
page.on('request', (r) => {
  if (/SendChat$/.test(r.url())) sends.push(r.url())
  if (/ExecuteBlueprint$/.test(r.url())) runs.push(r.url())
})
try {
  await page.goto(ready.baseURL + '/?open=' + encodeURIComponent(ready.workspace))
  await page
    .getByRole('navigation', { name: 'Main navigation' })
    .getByRole('button', { name: 'Chat', exact: true })
    .click()
  const message = page.getByRole('textbox', { name: 'Message', exact: true })
  await message.fill('Read app.ts and summarize its constant.')
  await page.getByRole('button', { name: 'Send', exact: true }).click()
  await expect(page.getByText(/METTEUR_LOCAL_READY/).first()).toBeVisible({ timeout: 30000 })
  await expect(page.getByRole('button', { name: 'Send', exact: true })).toBeVisible()
  await message.fill('Keep this original draft.')
  const sent = sends.length
  await page.getByRole('button', { name: 'Start from template', exact: true }).first().click()
  await page.getByRole('button', { name: /Refactor safely/ }).click()
  const name = 'Source review ' + Date.now()
  await page.getByLabel('Plan name', { exact: true }).fill(name)
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
  await page.getByRole('button', { name: 'Prepare run in original Chat', exact: true }).click()
  await expect(message).toHaveValue(/Keep this original draft\.[\s\S]*RequestBlueprintRun/)
  expect(sends).toHaveLength(sent)
  expect(runs).toHaveLength(0)
  report.checks.push(
    'Template from an existing conversation returns to the original draft, preserving prior text, without sending or running',
  )
  // Record the saved native file identity; editor/run coverage is in the confirmation check.
  const file = name + '.blueprint'
  const graph = JSON.parse(await readFile(ready.workspace + '/' + file, 'utf8'))
  report.blueprint = { id: graph.id, file }
} catch (e) {
  report.failure = String(e)
  await writeFile(output + '.page.txt', await page.locator('body').innerText())
}
await page.screenshot({ path: output + '.png' })
await writeFile(output, JSON.stringify(report, null, 2))
await browser.close()
console.log(JSON.stringify(report))
if (report.failure || report.errors.length) process.exitCode = 1
