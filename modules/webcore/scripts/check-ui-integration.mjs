// UI-only production acceptance. No Send, Run, Start task or model request.
import { chromium, expect } from '@playwright/test'
import { createClient } from '@connectrpc/connect'
import { createGrpcWebTransport } from '@connectrpc/connect-web'
import { Daemon } from '../src/gen/metteur_pb.ts'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { randomUUID } from 'node:crypto'

const [baseURL, destination, ledgerPath] = process.argv.slice(2)
if (new URL(baseURL).hostname !== '127.0.0.1') throw Error('Owned production loopback required')
const output = resolve(destination),
  workspace = join(output, 'workspace')
await mkdir(output)
await mkdir(workspace)
await writeFile(join(workspace, 'app.ts'), 'export const titleAudit = "界面一致性"\n')
const client = createClient(Daemon, createGrpcWebTransport({ baseUrl: baseURL }))
const report = { baseURL, workspace, started: new Date().toISOString(), pages: [], errors: [] }
const calls = async () => JSON.parse(await readFile(ledgerPath, 'utf8')).calls.length
report.paidCallsBefore = await calls()
await client.openWorkspace({ path: workspace })
const graph = await client.compileDsl({
  workspacePath: workspace,
  source: 'entry start: Start\nend: End\nstart -> end',
})
graph.name = '统一标题与长名称截断检查：保留原有蓝图编辑器和监督布局'
await client.saveBlueprint({
  workspacePath: workspace,
  blueprint: graph,
  filePath: 'title.blueprint',
})
const resource = async (method, payload) => {
  const response = await client[method]({
    schemaVersion: 1,
    requestId: randomUUID(),
    workspacePath: workspace,
    payloadJson: JSON.stringify(payload),
  })
  if (response.errorJson) throw Error(response.errorJson)
  return JSON.parse(response.payloadJson)
}
const tasksBefore = await resource('queryContinuations', { operation: 'list', after: null })
const browser = await chromium.launch({ headless: true })
const context = await browser.newContext({
  viewport: { width: 1280, height: 900 },
  serviceWorkers: 'block',
})
await context.addInitScript(() => localStorage.setItem('metteur.theme', 'light'))
await context.tracing.start({ screenshots: true, snapshots: true })
const page = await context.newPage()
page.on('pageerror', (error) => report.errors.push(error.message))
async function heading(name, container = '.surface-page') {
  const title = page.locator(container).getByRole('heading', { level: 1 })
  await expect(title).toHaveText(name)
  expect(await title.evaluate((element) => getComputedStyle(element).fontSize)).toBe('18px')
  report.pages.push({ title: name, viewport: page.viewportSize() })
}
try {
  await page.goto(`${baseURL}/?open=${encodeURIComponent(workspace)}`)
  await expect(page.locator('meta[name="metteur-gateway"]')).toHaveAttribute('content', 'grpc')
  await expect(page.getByTestId('file-tree')).toBeVisible()
  const nav = page.getByRole('navigation', { name: 'Main navigation' })
  await nav.getByRole('button', { name: 'Chat', exact: true }).click()
  await heading('Chat', '.chat-header')
  await page.getByRole('button', { name: /^Conversations —/ }).click()
  await page
    .locator('.chat-menu')
    .getByRole('button', { name: 'New conversation', exact: true })
    .click()
  await expect(page.locator('.chat-menu')).toHaveCount(0)
  await page
    .getByRole('textbox', { name: 'Message', exact: true })
    .fill('只检查界面，不发送模型请求。')
  await expect(page.getByText('Backed Up on Host', { exact: true })).toBeVisible()
  await nav.getByRole('button', { name: 'Tasks', exact: true }).click()
  await heading('Tasks')
  await page.getByRole('button', { name: 'New task', exact: true }).click()
  await expect(
    page.getByRole('dialog', { name: 'Start a Continuation Task', exact: true }),
  ).toBeVisible()
  await page.keyboard.press('Escape')
  await expect(page.getByRole('dialog')).toHaveCount(0)
  await page.getByRole('button', { name: 'Refresh tasks', exact: true }).click()
  await page.getByRole('tab', { name: 'Tasks', exact: true }).focus()
  await page.keyboard.press('ArrowRight')
  await expect(page.getByRole('tab', { name: 'Schedules', exact: true })).toBeFocused()
  await nav.getByRole('button', { name: 'Execution', exact: true }).click()
  await heading('Execution', '.run-header')
  await page.getByRole('button', { name: 'Select blueprint', exact: true }).click()
  const picker = page.getByRole('dialog', { name: 'Select blueprint', exact: true })
  await picker.getByRole('button', { name: 'title.blueprint', exact: true }).click()
  await picker.getByRole('button', { name: 'Import', exact: true }).click()
  await heading(graph.name, '.run-header')
  await expect(page.getByRole('button', { name: 'Run', exact: true })).toBeEnabled()
  await page.screenshot({ path: join(output, 'execution-wide.png') })
  await nav.getByRole('button', { name: 'Project Memory', exact: true }).click()
  await heading('Project Memory')
  await nav.getByRole('button', { name: 'Explorer', exact: true }).click()
  await page.getByRole('button', { name: 'Prompt Library', exact: true }).click()
  await heading('Prompt Library')
  await nav.getByRole('button', { name: 'Version Flow', exact: true }).click()
  await heading('Version Flow')
  await expect(page.getByRole('button', { name: 'Create snapshot', exact: true })).toBeVisible()
  await page.getByRole('button', { name: 'Settings', exact: true }).click()
  await page.getByRole('button', { name: 'LLM & Models', exact: true }).click()
  await heading('LLM & Models')
  await page.setViewportSize({ width: 800, height: 900 })
  await page.locator('header').first().locator('button.btn-icon').click()
  await expect(page.locator('html')).toHaveClass(/dark/)
  await heading('LLM & Models')
  expect(
    await page.locator('.surface-page').evaluate((el) => el.scrollWidth > el.clientWidth + 1),
  ).toBe(false)
  await page.screenshot({ path: join(output, 'settings-narrow-dark.png') })
  const tasksAfter = await resource('queryContinuations', { operation: 'list', after: null })
  expect(tasksAfter.items.length).toBe(tasksBefore.items.length)
  const audit = await client.listAuditLog({ workspacePath: workspace })
  expect(audit.entries.filter((entry) => entry.operation.startsWith('llm.'))).toHaveLength(0)
  report.paidCallsAfter = await calls()
  expect(report.paidCallsAfter).toBe(report.paidCallsBefore)
  expect(report.errors).toEqual([])
  report.passed = true
} catch (error) {
  report.passed = false
  report.failure = String(error)
  await page.screenshot({ path: join(output, 'first-failure.png') }).catch(() => {})
} finally {
  await context.tracing.stop({ path: join(output, 'trace.zip') })
  await browser.close()
  await client.closeWorkspace({ path: workspace })
  report.finished = new Date().toISOString()
  await writeFile(join(output, 'result.json'), JSON.stringify(report, null, 2))
}
console.log(JSON.stringify(report))
if (!report.passed) process.exitCode = 1
