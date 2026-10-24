import { test, expect } from '@playwright/test'
import { createClient } from '@connectrpc/connect'
import { createGrpcWebTransport } from '@connectrpc/connect-web'
import { Daemon } from '../../src/gen/metteur_pb'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { join, dirname } from 'node:path'
import { randomUUID } from 'node:crypto'
import type { RecoveryRecord } from '../../src/core/recovery'
import type { TaskRecord } from '../../src/core/continuations'

test('production ACKed two-window raw copies survive client reload and require conflict review after disk changes', async ({
  page,
  context,
}, testInfo) => {
  test.setTimeout(60000)
  const workspace = join(
    dirname(process.env.LIVE_WORKSPACE!),
    `recovery-two-windows-${randomUUID()}`,
  )
  await mkdir(workspace, { recursive: true })
  await writeFile(join(workspace, 'recovery.txt'), 'original saved content\n')
  const client = createClient(
    Daemon,
    createGrpcWebTransport({ baseUrl: process.env.LIVE_BASE_URL! }),
  )
  const errors: string[] = []
  page.on('pageerror', (e) => errors.push(e.message))
  async function edit(target: typeof page, text: string) {
    await target.goto(`/?open=${encodeURIComponent(workspace)}`)
    await target
      .getByTestId('file-tree')
      .getByRole('button', { name: 'recovery.txt', exact: true })
      .click()
    await expect(target.locator('.monaco-editor .view-lines')).toContainText(
      'original saved content',
    )
    await target.locator('.monaco-editor .view-lines').click()
    await target.keyboard.press('ControlOrMeta+a')
    await target.keyboard.insertText(text)
    await expect(target.getByText('Backed Up on Host', { exact: true })).toBeVisible()
  }
  async function recovery(action: unknown) {
    const response = await client.recovery({
      schemaVersion: 1,
      requestId: randomUUID(),
      workspacePath: workspace,
      payloadJson: JSON.stringify({ scope: 'project', action }),
    })
    expect(response.errorJson).toBe('')
    return JSON.parse(response.payloadJson)
  }
  await edit(page, 'window one raw 中文')
  const other = await context.newPage()
  other.on('pageerror', (e) => errors.push(e.message))
  await edit(other, 'window two raw 中文')
  const listed = await recovery({ operation: 'list', after: null })
  const copies: RecoveryRecord[] = []
  for (const row of listed.items.filter((r: { kind: string }) => r.kind === 'text'))
    copies.push((await recovery({ operation: 'read', id: row.id })).record)
  expect(copies).toHaveLength(2)
  expect(new Set(copies.map((c) => c.input.window_id)).size).toBe(2)
  expect(copies.map((c) => c.input.raw).sort()).toEqual(
    ['window one raw 中文', 'window two raw 中文'].sort(),
  )
  await other.close()
  await writeFile(join(workspace, 'recovery.txt'), 'later external saved version\n')
  page.once('dialog', (d) => d.accept())
  await page.reload()
  await page
    .getByTestId('file-tree')
    .getByRole('button', { name: 'recovery.txt', exact: true })
    .click()
  await expect(page.locator('.monaco-editor .view-lines')).toContainText(
    'later external saved version',
  )
  await page.getByRole('button', { name: 'Recovery Copies', exact: true }).click()
  const dialog = page.getByRole('dialog', { name: 'Recovery Copies', exact: true })
  const original = copies.find((c) => c.input.raw === 'window one raw 中文')!
  await dialog
    .locator('article')
    .filter({ hasText: original.input.window_id })
    .getByRole('button', { name: 'Review Copy', exact: true })
    .click()
  await expect(dialog.locator('pre')).toContainText('window one raw 中文')
  await dialog.getByRole('button', { name: 'Use This Copy', exact: true }).click()
  await expect(page.locator('.monaco-editor .view-lines')).toContainText('window one raw 中文')
  await expect(page.getByRole('button', { name: 'Compare Changes', exact: true })).toBeVisible()
  expect(await readFile(join(workspace, 'recovery.txt'), 'utf8')).toBe(
    'later external saved version\n',
  )
  const retained = await recovery({
    operation: 'read',
    id: copies.find((c) => c.input.raw === 'window two raw 中文')!.input.id,
  })
  expect(retained.record.input.raw).toBe('window two raw 中文')
  expect(errors).toEqual([])
  await testInfo.attach('host-confirmed-copies', {
    body: JSON.stringify({ workspace, copies, retained }, null, 2),
    contentType: 'application/json',
  })
})

test('production task presents actual pending approval, Stop fences it, and a source-free new plan only appends a draft', async ({
  page,
}, testInfo) => {
  test.setTimeout(45000)
  const workspace = join(dirname(process.env.LIVE_WORKSPACE!), `approval-facts-${randomUUID()}`)
  await mkdir(workspace, { recursive: true })
  const original = 'def pick(values, query, limit=None):\n    return []\n'
  await writeFile(join(workspace, 'feature.py'), original)
  const client = createClient(
    Daemon,
    createGrpcWebTransport({ baseUrl: process.env.LIVE_BASE_URL! }),
  )
  await client.openWorkspace({ path: workspace })
  const goal = `DW_STORY_B_STAGED approval facts ${randomUUID()}`,
    events: unknown[] = []
  const before = await (await page.request.get(`${process.env.LIVE_FIXTURE_URL!}/fixture`)).json()
  async function rows() {
    const response = await client.queryContinuations({
      schemaVersion: 1,
      requestId: randomUUID(),
      workspacePath: workspace,
      payloadJson: JSON.stringify({ operation: 'list', after: null }),
    })
    expect(response.errorJson).toBe('')
    return JSON.parse(response.payloadJson).items as TaskRecord[]
  }
  const stream = (async () => {
    for await (const event of client.sendChat({
      workspacePath: workspace,
      message: goal,
      historyJson: '[]',
      optionsJson: JSON.stringify({
        provider: 'openai-chat',
        model: 'fixture',
        max_tokens: 128,
        permission_mode: 'ask',
        continuation_policy: {
          detached: false,
          max_turns: 4,
          token_limit: 200000,
          expires_ms: Date.now() + 60000,
          purpose: goal,
        },
      }),
    }))
      events.push(event)
  })()
  let record: TaskRecord | undefined
  await expect
    .poll(async () => {
      record = (await rows()).find((r) => r.policy?.purpose === goal)
      return record?.pending_approvals?.length
    })
    .toBe(1)
  expect(record!.workflow!.reason_code).toBe('approval_required')
  await page.goto(`/?open=${encodeURIComponent(workspace)}`)
  await page
    .getByRole('navigation', { name: 'Main navigation' })
    .getByRole('button', { name: 'Tasks', exact: true })
    .click()
  await expect(page.getByRole('region', { name: 'Task next action', exact: true })).toContainText(
    'Approval is required',
  )
  await expect(
    page.getByRole('button', { name: 'Review requested action', exact: true }),
  ).toBeVisible()
  await page.getByRole('button', { name: 'Stop task', exact: true }).click()
  await page
    .getByRole('dialog', { name: 'Stop task', exact: true })
    .getByRole('button', { name: 'Stop task', exact: true })
    .click()
  await expect
    .poll(async () => (await rows()).find((r) => r.task.id === record!.task.id)?.task.state)
    .toBe('stopped')
  await stream
  expect(await readFile(join(workspace, 'feature.py'), 'utf8')).toBe(original)
  const stopped = (await rows()).find((r) => r.task.id === record!.task.id)!
  const stoppedCalls = (
    await (await page.request.get(`${process.env.LIVE_FIXTURE_URL!}/fixture`)).json()
  ).calls
  expect(stoppedCalls).toBeGreaterThan(before.calls)
  await page.getByRole('button', { name: 'Prepare new plan in draft', exact: true }).click()
  await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toHaveValue(
    new RegExp(record!.task.id),
  )
  const later = (await rows()).find((r) => r.task.id === record!.task.id)!
  expect(later.task.budget).toEqual(stopped.task.budget)
  expect(later.task.state).toBe('stopped')
  expect(
    (await (await page.request.get(`${process.env.LIVE_FIXTURE_URL!}/fixture`)).json()).calls,
  ).toBe(stoppedCalls)
  await testInfo.attach('host-approval-and-stop', {
    body: JSON.stringify({ record, stopped, later, events }, null, 2),
    contentType: 'application/json',
  })
})
