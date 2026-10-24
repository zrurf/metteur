// Read-only behavioural observations against the frozen R8 product, not AC passes.
import { chromium } from '@playwright/test'
import { mkdir, writeFile } from 'node:fs/promises'
import { execFileSync } from 'node:child_process'
import { resolve } from 'node:path'

const output = resolve(process.argv[2])
await mkdir(output, { recursive: true })
const observations = []
const browser = await chromium.launch({ headless: true })
const page = await browser.newPage({ viewport: { width: 1360, height: 900 } })
const started = Date.now()
try {
  await page.goto('http://127.0.0.1:19080/')
  await page.getByLabel('Workspace path').fill('D:/metteur-demo/metrics')
  await page.getByRole('button', { name: 'Open', exact: true }).click()
  await page.getByTestId('file-tree').waitFor()
  await page.getByRole('button', { name: 'Chat', exact: true }).first().click()
  const message = page.getByRole('textbox', { name: 'Message' })
  await message.waitFor()
  observations.push({ id: 'startup', elapsed_ms: Date.now() - started, mode: 'development/mock/cold-vite' })
  await message.fill('draft-before-page-switch')
  await page.getByRole('button', { name: 'Tasks', exact: true }).first().click()
  await page.getByRole('button', { name: 'Chat', exact: true }).first().click()
  observations.push({ id: 'G02/G03', action: 'chat draft -> Tasks -> Chat', value: await message.inputValue(), defect: (await message.inputValue()) !== 'draft-before-page-switch' })
  await page.evaluate(async () => {
    const core = await import('/src/core/index.ts')
    core.gateway.sendChat = async () => ({ ok: false, error: 'DW_REJECTED_BEFORE_ACCEPTANCE' })
  })
  await message.fill('keep-on-rejection')
  await message.press('Enter')
  await page.getByText('DW_REJECTED_BEFORE_ACCEPTANCE', { exact: true }).waitFor()
  observations.push({ id: 'G03', action: 'known rejection before acceptance', value: await message.inputValue(), defect: (await message.inputValue()) !== 'keep-on-rejection' })
  await message.fill('中文候选')
  await message.dispatchEvent('keydown', { key: 'Enter', code: 'Enter', isComposing: true, keyCode: 229 })
  observations.push({ id: 'G14', action: 'composition Enter with isComposing/keyCode229', value: await message.inputValue(), defect: (await message.inputValue()) !== '中文候选', limit: 'synthetic event; actual Windows IME remains a user check' })
  const rollback = await page.evaluate(async () => {
    const { useVersionStore } = await import('/src/stores/version.ts')
    const { gateway } = await import('/src/core/index.ts')
    gateway.rollback = async () => ({ ok: false, error: 'DW_ROLLBACK_FAILED' })
    const store = useVersionStore()
    await store.rollback('missing-snapshot')
    return store.error
  })
  observations.push({ id: 'G06', action: 'rollback rejected', error: rollback, defect: rollback !== 'DW_ROLLBACK_FAILED' })
  await page.evaluate(async () => {
    const { useExecutionStore } = await import('/src/stores/execution.ts')
    const store = useExecutionStore()
    store.current = { snapshot: { runtime: { active: true } } }
    store.runId = 'baseline-blueprint-run'
  })
  await page.getByRole('region', { name: 'Run concierge' }).waitFor()
  observations.push({ id: 'G01', action: 'blueprint becomes active', main_chat_replaced: true, defect: true })
  await page.screenshot({ path: `${output}/baseline-blueprint-replaces-chat.png`, fullPage: true })
} finally {
  await browser.close()
  await writeFile(`${output}/baseline-browser.json`, JSON.stringify({
    candidate: execFileSync('git', ['rev-parse', 'HEAD'], { encoding: 'utf8' }).trim(),
    source_status: execFileSync('git', ['status', '--short'], { encoding: 'utf8' }),
    platform: process.platform, mode: 'L2 existing product mock', paid_calls: 0,
    command: process.argv.join(' '), observations,
  }, null, 2))
}
