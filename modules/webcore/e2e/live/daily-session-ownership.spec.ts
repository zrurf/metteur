import { test, expect } from '@playwright/test'
import { createClient } from '@connectrpc/connect'
import { createGrpcWebTransport } from '@connectrpc/connect-web'
import { Daemon } from '../../src/gen/metteur_pb'
import { randomUUID } from 'node:crypto'
import type { TaskRecord } from '../../src/core/continuations'
import type { RecoveryRecord } from '../../src/core/recovery'

test('production A launch keeps B as main Chat, blocks occupied Send and reopens B with its separately recovered draft', async ({
  page,
  context,
  request,
}, info) => {
  test.setTimeout(60000)
  const workspace = process.env.LIVE_WORKSPACE!,
    client = createClient(Daemon, createGrpcWebTransport({ baseUrl: process.env.LIVE_BASE_URL! }))
  const proof = async () => (await request.get(`${process.env.LIVE_FIXTURE_URL!}/fixture`)).json()
  const errors: string[] = []
  page.on('pageerror', (e) => errors.push(e.message))
  async function resource(method: 'queryContinuations' | 'recovery', payload: unknown) {
    const response = await client[method]({
      schemaVersion: 1,
      requestId: randomUUID(),
      workspacePath: workspace,
      payloadJson: JSON.stringify(payload),
    })
    expect(response.errorJson).toBe('')
    return JSON.parse(response.payloadJson)
  }
  let sessionB = ''
  const titleB = `Daily B history ${randomUUID().slice(0, 8)}`
  for await (const event of client.sendChat({
    workspacePath: workspace,
    message: titleB,
    historyJson: '[]',
    optionsJson: JSON.stringify({
      provider: 'mock',
      mock_text: 'Saved B history',
      max_tokens: 128,
    }),
  })) {
    expect(event.kind).not.toBe('error')
    if (event.kind === 'session') sessionB = JSON.parse(event.detailJson).session_id
  }
  expect(sessionB).not.toBe('')
  const before = await proof()
  await page.goto(`/?open=${encodeURIComponent(workspace)}`)
  await page
    .getByRole('navigation', { name: 'Main navigation' })
    .getByRole('button', { name: 'Chat', exact: true })
    .click()
  await page.getByRole('button', { name: 'New conversation', exact: true }).click()
  const input = page.getByRole('textbox', { name: 'Message', exact: true })
  await input.fill(
    'M3_LOCAL_BLUEPRINT: retain this exact A result without an automatic return turn.',
  )
  await input.press('Enter')
  const launch = page.getByRole('dialog', { name: 'Review a Blueprint Run', exact: false })
  await expect(launch).toBeVisible()
  await launch
    .getByRole('checkbox', { name: 'Resume this conversation when the run finishes', exact: true })
    .uncheck()
  await launch
    .getByRole('checkbox', {
      name: 'I approve this version, handoff scope and completion policy',
      exact: true,
    })
    .check()
  await launch.getByRole('button', { name: 'Confirm and run', exact: true }).click()
  await expect(launch).toHaveCount(0)
  await expect.poll(async () => (await proof()).calls).toBe(before.calls + 2)
  const switcher = page.getByRole('button', { name: /^Conversations — / })
  await expect(switcher).toBeEnabled()
  await switcher.click()
  await page.locator('.chat-menu').getByRole('button').filter({ hasText: titleB }).click()
  await expect(switcher).toContainText(titleB)
  await expect(page.getByRole('region', { name: 'Run concierge', exact: true })).toHaveCount(0)
  await input.fill('B unsent draft 中文')
  await input.press('Enter')
  await expect(input).toHaveValue('B unsent draft 中文')
  await expect(page.getByText('Backed Up on Host', { exact: true })).toBeVisible()
  let task: TaskRecord | undefined
  await expect
    .poll(
      async () => {
        task = (
          (await resource('queryContinuations', { operation: 'list', after: null }))
            .items as TaskRecord[]
        ).find((r) => r.policy?.purpose === 'Review local evidence')
        return task?.task.state
      },
      { timeout: 20000 },
    )
    .toBe('completed')
  expect(task!.task.session).not.toBe(sessionB)
  await expect(switcher).toContainText(titleB)
  await expect(input).toHaveValue('B unsent draft 中文')
  expect(new URL(page.url()).pathname).toMatch(/\/chat$/)
  expect((await proof()).calls).toBe(before.calls + 2)
  const rows = await resource('recovery', {
    scope: 'project',
    action: { operation: 'list', after: null },
  })
  const metadata = rows.items.find(
    (row: { kind: string; entity: string }) => row.kind === 'chat' && row.entity === sessionB,
  )
  expect(metadata).toBeTruthy()
  const copy = (
    await resource('recovery', { scope: 'project', action: { operation: 'read', id: metadata.id } })
  ).record as RecoveryRecord
  expect(copy.input.raw).toBe('B unsent draft 中文')
  await expect
    .poll(async () =>
      JSON.stringify(
        await resource('recovery', { scope: 'project', action: { operation: 'latest_view' } }),
      ),
    )
    .toContain(sessionB)
  await page.close()
  const reopened = await context.newPage()
  reopened.on('pageerror', (e) => errors.push(e.message))
  await reopened.goto(`/?open=${encodeURIComponent(workspace)}`)
  await expect(reopened.getByRole('button', { name: /^Conversations — / })).toContainText(titleB)
  const restoredInput = reopened.getByRole('textbox', { name: 'Message', exact: true })
  await expect(restoredInput).toHaveValue('')
  await reopened.getByRole('button', { name: 'Recovery Copies', exact: true }).click()
  const dialog = reopened.getByRole('dialog', { name: 'Recovery Copies', exact: true })
  await dialog
    .locator('article')
    .filter({ hasText: sessionB })
    .filter({ hasText: copy.input.window_id })
    .getByRole('button', { name: 'Review Copy', exact: true })
    .click()
  await expect(dialog.locator('pre')).toContainText('B unsent draft 中文')
  await dialog.getByRole('button', { name: 'Use This Copy', exact: true }).click()
  await expect(restoredInput).toHaveValue('B unsent draft 中文')
  expect((await proof()).calls).toBe(before.calls + 2)
  const savedB = await client.getChatSession({ workspacePath: workspace, sessionId: sessionB })
  expect(savedB.transcriptJson).not.toContain('B unsent draft 中文')
  expect(errors).toEqual([])
  await info.attach('exact-source-and-draft', {
    body: JSON.stringify({ sessionB, task, copy, errors }, null, 2),
    contentType: 'application/json',
  })
})
