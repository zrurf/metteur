import { chromium, expect } from '@playwright/test'
import { readFile, writeFile } from 'node:fs/promises'
const [readyPath, output] = process.argv.slice(2),
  ready = JSON.parse(await readFile(readyPath, 'utf8'))
const browser = await chromium.launch(),
  page = await browser.newPage({ viewport: { width: 1440, height: 1000 } })
const report = { checks: [], errors: [] }
const sends = []
page.on('request', (r) => {
  if (r.method() === 'POST' && /SendChat|\/api\/chat/.test(r.url())) sends.push(r.url())
})
page.on('pageerror', (e) => report.errors.push(String(e)))
try {
  await page.goto('http://127.0.0.1:19111/?open=' + encodeURIComponent(ready.workspace))
  await page
    .getByRole('navigation', { name: 'Main navigation' })
    .getByRole('button', { name: 'Execution', exact: true })
    .click()
  await page.getByRole('button', { name: 'Result', exact: true }).click()
  await page.getByRole('button', { name: 'Review delivery', exact: true }).click()
  await expect(page.getByText('Stale', { exact: true })).toBeVisible()
  if (process.argv[4] !== 'reopen-only') {
    await page.getByRole('button', { name: 'Review requirement', exact: true }).last().click()
    await page.getByRole('textbox').last().fill('Live UI review: recheck the changed file')
    await page.getByRole('button', { name: 'Request changes', exact: true }).click()
    await expect(
      page.getByText('Decision recorded. Execution permissions and task limits are unchanged.', {
        exact: true,
      }),
    ).toBeVisible()
    report.checks.push(
      'production result opens real requirements, displays stale evidence and records a human request for changes',
    )
  }
  await page.screenshot({ path: output + '.png' })
  await page.reload()
  await page
    .getByRole('navigation', { name: 'Main navigation' })
    .getByRole('button', { name: 'Execution', exact: true })
    .click()
  await page.getByRole('button', { name: 'Result', exact: true }).click()
  await page.getByRole('button', { name: 'Review delivery', exact: true }).click()
  await expect(page.getByText(/— Live UI review: recheck the changed file/).last()).toBeVisible()
  report.checks.push('recorded note survives browser refresh and reopening the original run')
  await page.getByRole('button', { name: 'Prepare follow-up in Chat', exact: true }).click()
  await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toHaveValue(
    /Prepare a new proposal for the saved result/,
  )
  expect(sends).toHaveLength(0)
  report.checks.push(
    'real result reference and unmet requirements reach the chat draft without sending a message or starting a run',
  )
} catch (e) {
  report.failure = String(e)
  await writeFile(output + '.page.txt', await page.locator('body').innerText())
  await page.screenshot({ path: output + '.png' })
}
await writeFile(output, JSON.stringify(report, null, 2))
await browser.close()
console.log(JSON.stringify(report))
if (report.failure || report.errors.length) process.exitCode = 1
