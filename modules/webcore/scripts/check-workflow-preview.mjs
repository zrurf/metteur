import { chromium } from '@playwright/test'
import { mkdir, writeFile } from 'node:fs/promises'
import { resolve } from 'node:path'
const output = process.argv[2]
if (!output) throw new Error('Choose a new evidence directory.')
await mkdir(output, { recursive: false })
const browser = await chromium.launch({ headless: true })
const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } })
const errors = [],
  results = [],
  requests = []
page.on('pageerror', (error) => errors.push(String(error)))
page.on('request', (request) => {
  if (/\/metteur\.Daemon|\/api\//.test(request.url())) requests.push(request.url())
})
const url = 'http://127.0.0.1:19110/src/previews/workflow/index.html'
async function check(name, run) {
  if (process.argv[3] && !process.argv[3].split(',').includes(name)) return
  try {
    await run()
    results.push({ name, passed: true })
  } catch (error) {
    results.push({ name, passed: false, error: String(error) })
    await page.screenshot({ path: resolve(output, `${name}-failed.png`), animations: 'disabled' })
  }
}
async function open(group = 'WD-PV-01') {
  await page.goto(`${url}?preview=${group}`)
  await page.getByText('Isolated fixture · r2', { exact: true }).waitFor()
}
async function plan() {
  await open()
  await page.getByRole('button', { name: 'Start from template', exact: true }).click()
}
try {
  await check('four-template-plans', async () => {
    for (const name of ['Fix a bug', 'Add tests', 'Refactor safely', 'Add a small feature']) {
      await plan()
      await page.getByRole('button', { name: new RegExp(`^${name}`) }).click()
      if (await page.getByLabel('Maximum iterations').isVisible())
        throw new Error('Advanced settings should start collapsed')
      await page.getByRole('button', { name: 'Preview plan', exact: true }).click()
      await page.getByText('Creating a blueprint does not start it.', { exact: true }).waitFor()
      if (await page.getByRole('button', { name: 'Create blueprint', exact: true }).isDisabled())
        throw new Error('Creation unavailable after plan review')
    }
    await page.screenshot({ path: resolve(output, 'plan.png') })
  })
  await check('save-failure-and-version-confirmation', async () => {
    await plan()
    await page.getByRole('button', { name: /^Fix a bug/ }).click()
    await page.getByRole('button', { name: 'Preview plan', exact: true }).click()
    await page.getByLabel('Fixture scenario').selectOption('Save failed')
    await page.getByRole('button', { name: 'Create blueprint', exact: true }).click()
    await page.getByText(/The file could not be saved/).waitFor()
    await page.getByLabel('Fixture scenario').selectOption('Ready')
    await page.getByRole('button', { name: 'Create blueprint', exact: true }).click()
    await page.getByText('Fixture blueprint saved. It has not run.', { exact: true }).waitFor()
    await page.getByRole('button', { name: 'Review & run', exact: true }).click()
    const confirm = page.getByRole('button', { name: 'Confirm run · fixture', exact: true })
    if (!(await confirm.isDisabled())) throw new Error('Run requires exact confirmation')
    await page.getByLabel('Confirm this version, handoff scope and completion policy').check()
    await page.getByLabel('Fixture: change a dependency after review').check()
    if (!(await confirm.isDisabled())) throw new Error('Stale launch enabled')
    await page.screenshot({ path: resolve(output, 'stale-launch.png') })
    await page.getByLabel('Fixture: change a dependency after review').uncheck()
    await confirm.click()
    await page.getByLabel('Message draft').waitFor()
    await page.getByRole('button', { name: 'Review run result', exact: true }).click()
    await page.getByRole('heading', { name: 'Review delivery' }).waitFor()
  })
  await check('review-states-and-draft', async () => {
    await open('WD-PV-02')
    await page.getByRole('button', { name: 'Review requirement', exact: true }).first().click()
    await page.getByLabel('Review note').fill('Independent checks and scoped diff reviewed.')
    await page.getByRole('button', { name: 'Accept this requirement' }).click()
    await page.getByText(/Recorded review: Accepted/).waitFor()
    await page.getByLabel('Fixture scenario').selectOption('Stale')
    if (!(await page.getByRole('button', { name: 'Accept this requirement' }).isDisabled()))
      throw new Error('Stale evidence accepted')
    await page.screenshot({ path: resolve(output, 'review.png') })
    for (const state of ['Failed', 'Missing', 'Skipped', 'Unknown']) {
      await page.getByLabel('Fixture scenario').selectOption(state)
      if (!(await page.getByRole('button', { name: 'Accept this requirement' }).isDisabled()))
        throw new Error(`${state} evidence accepted`)
    }
    await page.getByRole('button', { name: 'Prepare follow-up in Chat' }).click()
    await page.getByText(/Nothing was sent/).waitFor()
    if (!(await page.getByLabel('Message draft').inputValue()).includes('Unmet requirements:'))
      throw new Error('Missing failed requirements in source draft')
  })
  await check('file-task-chat-continuity', async () => {
    await open('WD-PV-02')
    await page.getByRole('button', { name: 'Back to original Chat', exact: true }).click()
    await page.getByLabel('Message draft').fill('Keep my original draft.')
    await page.getByRole('button', { name: 'Review run result', exact: true }).click()
    await page.getByText('View evidence', { exact: true }).first().click()
    await page.getByRole('button', { name: 'metrics/normalize.py', exact: true }).first().click()
    await page.getByLabel('File content').fill('# local edit\n')
    await page.getByRole('button', { name: 'Back to result or source', exact: true }).click()
    await page.getByRole('button', { name: 'Keep editing', exact: true }).click()
    await page.getByLabel('Fixture scenario').selectOption('Save failed')
    await page.getByRole('button', { name: 'Save file · fixture', exact: true }).click()
    await page.getByText(/File save failed/).waitFor()
    if ((await page.getByLabel('File content').inputValue()) !== '# local edit\n')
      throw new Error('Save failure lost edits')
    await page.getByLabel('Fixture scenario').selectOption('Ready')
    await page.getByRole('button', { name: 'Save file · fixture', exact: true }).click()
    await page.getByRole('button', { name: 'Back to result or source', exact: true }).click()
    await page
      .getByRole('region', { name: 'Workflow acceptance review' })
      .getByText('Stale', { exact: true })
      .waitFor()
    await page.getByRole('button', { name: 'Prepare follow-up in Chat' }).click()
    if (
      !(await page.getByLabel('Message draft').inputValue()).startsWith('Keep my original draft.')
    )
      throw new Error('Follow-up replaced original draft')
    await page.getByRole('button', { name: 'View linked task', exact: true }).click()
    await page.getByRole('button', { name: 'Open supervision', exact: true }).click()
    await page.getByRole('heading', { name: 'Blueprint supervision', exact: true }).waitFor()
    await page.getByRole('button', { name: 'Back to original Chat', exact: true }).click()
    if (
      !(await page.getByLabel('Message draft').inputValue()).startsWith('Keep my original draft.')
    )
      throw new Error('Supervision changed source draft')
    await page.screenshot({ path: resolve(output, 'source-chat.png'), animations: 'disabled' })
  })
  await check('narrow-dark-keyboard', async () => {
    await plan()
    await page.setViewportSize({ width: 760, height: 900 })
    await page.getByRole('button', { name: 'Toggle theme' }).click()
    await page.getByRole('button', { name: /^Refactor safely/ }).focus()
    await page.keyboard.press('Enter')
    await page.getByRole('button', { name: 'Preview plan', exact: true }).waitFor()
    if (await page.evaluate(() => document.documentElement.scrollWidth > innerWidth + 1))
      throw new Error('Horizontal page overflow')
    await page.screenshot({ path: resolve(output, 'narrow-dark.png'), animations: 'disabled' })
  })
} finally {
  await writeFile(
    resolve(output, 'results.json'),
    JSON.stringify({ preview_revision: 'r2', fixture: true, results, errors, requests }, null, 2),
  )
  await browser.close()
}
console.log(JSON.stringify({ results, errors, requests }, null, 2))
if (errors.length || requests.length || results.some((r) => !r.passed)) process.exitCode = 1
