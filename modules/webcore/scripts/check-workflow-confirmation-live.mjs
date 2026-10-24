import { chromium, expect } from '@playwright/test'
import { readFile, writeFile } from 'node:fs/promises'
import { fromBinary } from '@bufbuild/protobuf'
import { WriteFileRequestSchema } from '../src/gen/metteur_pb.ts'
const [readyPath, output, file] = process.argv.slice(2),
  ready = JSON.parse(await readFile(readyPath, 'utf8'))
const browser = await chromium.launch(),
  page = await browser.newPage({ viewport: { width: 1440, height: 1000 } })
const report = { checks: [], errors: [] },
  runs = [],
  sends = []
page.on('pageerror', (e) => report.errors.push(String(e)))
page.on('request', (r) => {
  if (/ExecuteBlueprint$/.test(r.url())) runs.push(r.url())
  if (/SendChat$/.test(r.url())) sends.push(r.url())
})
page.on('request', (r) => {
  if (/WriteFile$/.test(r.url())) {
    try {
      const b = r.postDataBuffer(),
        v = fromBinary(WriteFileRequestSchema, b[0] === 0 ? b.subarray(5) : b)
      report.lastWrite = JSON.parse(v.content)
    } catch {}
  }
})
try {
  await page.goto(ready.baseURL + '/?open=' + encodeURIComponent(ready.workspace))
  if (process.argv[5] !== 'execution-only') {
    await page.getByText(file, { exact: true }).first().dblclick()
    const graph = JSON.parse(await readFile(ready.workspace + '/' + file, 'utf8'))
    await page.getByRole('button', { name: 'Save', exact: true }).click()
    const saved = JSON.parse(await readFile(ready.workspace + '/' + file, 'utf8'))
    expect(saved.nodes.find((n) => n.data?.['_workflow_contract']).data['_workflow_contract']).toEqual(
      graph.nodes.find((n) => n.data?.['_workflow_contract']).data['_workflow_contract'],
    )
    report.checks.push('Ordinary blueprint save preserves acceptance metadata')
    await page.getByRole('button', { name: 'Run', exact: true }).click()
    const dialog = page.getByRole('dialog', { name: 'Review & run' })
    await expect(dialog).toBeVisible()
    await dialog.getByText('Commands & exact dependencies', { exact: true }).click()
    await expect(dialog).toContainText('node -e')
    await dialog.getByRole('button', { name: 'Cancel', exact: true }).click()
    expect(runs).toHaveLength(0)
    report.checks.push('Editor cancellation sends no execution request')
  }
  await page
    .getByRole('navigation', { name: 'Main navigation' })
    .getByRole('button', { name: 'Execution', exact: true })
    .click()
  await page.getByRole('button', { name: 'Select blueprint', exact: true }).click()
  await page.getByRole('dialog').getByRole('button', { name: file, exact: true }).dblclick()
  await page.getByRole('button', { name: 'Run', exact: true }).click()
  const dialog2 = page.getByRole('dialog', { name: 'Review & run' })
  await expect(dialog2).toBeVisible()
  await dialog2.getByRole('button', { name: 'Cancel', exact: true }).click()
  expect(runs).toHaveLength(0)
  expect(sends).toHaveLength(0)
  report.checks.push(
    'Execution requires saved-version confirmation; cancellation sends no execution or chat request',
  )
  if (process.argv.includes('--accept')) {
    await page.getByRole('button', { name: 'Run', exact: true }).click()
    await page.getByRole('button', { name: 'Run confirmed version', exact: true }).click()
    const until = Date.now() + 30000
    while (Date.now() < until) {
      const allow = page.getByRole('button', { name: 'Allow', exact: true })
      if (await allow.isVisible()) {
        await expect(page.getByRole('dialog')).toContainText('node -e')
        await allow.click()
      }
      if (
        await page
          .locator('.status')
          .filter({ hasText: /^finished$/i })
          .first()
          .isVisible()
      )
        break
      await page.waitForTimeout(100)
    }
    await expect(
      page
        .locator('.status')
        .filter({ hasText: /^finished$/i })
        .first(),
    ).toBeVisible()
    expect(runs).toHaveLength(1)
    await page.getByRole('button', { name: 'Result', exact: true }).click()
    await page.getByRole('button', { name: 'Review delivery', exact: true }).click()
    await expect(page.getByText('Passed', { exact: true }).first()).toBeVisible()
    expect(sends).toHaveLength(0)
    report.checks.push(
      'Confirmed edited template runs once through production UI, real shell checks and local provider, then opens host acceptance evidence',
    )
  }
} catch (e) {
  report.failure = String(e)
  await writeFile(output + '.page.txt', await page.locator('body').innerText())
}
await page.screenshot({ path: output + '.png' })
await writeFile(output, JSON.stringify(report, null, 2))
await browser.close()
console.log(JSON.stringify(report))
if (report.failure || report.errors.length) process.exitCode = 1
