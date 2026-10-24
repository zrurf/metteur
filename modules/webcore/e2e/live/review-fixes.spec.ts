import { test, expect } from '@playwright/test'
import { createClient } from '@connectrpc/connect'
import { createGrpcWebTransport } from '@connectrpc/connect-web'
import { Daemon } from '../../src/gen/metteur_pb'
import { mkdir, writeFile } from 'node:fs/promises'
import { join, dirname } from 'node:path'
import { randomUUID } from 'node:crypto'

const client = () =>
  createClient(Daemon, createGrpcWebTransport({ baseUrl: process.env.LIVE_BASE_URL! }))

test('production recovery compacts discarded copies and backs up a new editor draft', async ({
  page,
}, testInfo) => {
  test.setTimeout(90000)
  const workspace = join(dirname(process.env.LIVE_WORKSPACE!), `capacity-${randomUUID()}`),
    rpc = client()
  await mkdir(workspace)
  await writeFile(join(workspace, 'capacity.txt'), 'original saved file')
  await rpc.openWorkspace({ path: workspace })
  const recovery = async (action: unknown, revision = 0) =>
    rpc.recovery({
      schemaVersion: 1,
      requestId: randomUUID(),
      workspacePath: workspace,
      expectedRevision: BigInt(revision),
      payloadJson: JSON.stringify({ scope: 'project', action }),
    })
  let first: any
  for (let n = 0; n < 130; n++) {
    const caps = JSON.parse((await recovery({ operation: 'capabilities' })).payloadJson)
    const input = {
      id: randomUUID(),
      window_id: randomUUID(),
      creation_epoch: caps.creation_epoch,
      kind: 'text',
      entity: 'capacity.txt',
      edit_sequence: 1,
      raw: `saved copy ${n}`,
      baseline_identity: null,
      selections: [],
      pending_submission: null,
      selected_model: null,
      view: null,
    }
    if (!first) first = input
    expect((await recovery({ operation: 'backup', input })).errorJson).toBe('')
    expect(
      (await recovery({ operation: 'discard', id: input.id, saved_sequence: 1 }, 1)).errorJson,
    ).toBe('')
  }
  const late = await recovery({ operation: 'backup', input: first })
  expect(JSON.parse(late.errorJson).code).toBe('creation_epoch_required')
  await page.goto(`/?open=${encodeURIComponent(workspace)}`)
  await page
    .getByTestId('file-tree')
    .getByRole('button', { name: 'capacity.txt', exact: true })
    .click()
  await expect(page.locator('.monaco-editor .view-lines')).toContainText('original saved file')
  await page.locator('.monaco-editor .view-lines').click()
  await page.keyboard.press('ControlOrMeta+a')
  await page.keyboard.insertText('new draft protected after compaction 中文')
  await expect(page.getByText('Backed Up on Host', { exact: true })).toBeVisible()
  const rows: any[] = []
  let after: string | null = null
  do {
    const listed = JSON.parse((await recovery({ operation: 'list', after })).payloadJson)
    rows.push(...listed.items)
    after = listed.next
  } while (after)
  expect(rows.filter((r) => r.discarded).length).toBeLessThanOrEqual(128)
  const drafts = rows.filter((r) => !r.discarded && r.kind === 'text')
  expect(drafts).toHaveLength(1)
  const saved = JSON.parse((await recovery({ operation: 'read', id: drafts[0].id })).payloadJson)
  expect(saved.record.input.raw).toBe('new draft protected after compaction 中文')
  await testInfo.attach('compaction', {
    body: JSON.stringify(
      { rows: rows.length, tombstones: rows.filter((r) => r.discarded).length, saved },
      null,
      2,
    ),
    contentType: 'application/json',
  })
})

test('production binding confirmation resumes once in the original task and preserves the draft', async ({
  page,
}, testInfo) => {
  test.setTimeout(90000)
  const workspace = join(dirname(process.env.LIVE_WORKSPACE!), `rebind-${randomUUID()}`),
    rpc = client()
  await mkdir(workspace)
  await rpc.openWorkspace({ path: workspace })
  const resource = async (
    method: 'queryContinuations' | 'controlContinuation',
    payload: unknown,
    revision = 0,
  ) => {
    const reply = await rpc[method]({
      schemaVersion: 1,
      requestId: randomUUID(),
      workspacePath: workspace,
      expectedRevision: BigInt(revision),
      payloadJson: JSON.stringify(payload),
    })
    expect(reply.errorJson).toBe('')
    return JSON.parse(reply.payloadJson)
  }
  let taskId = ''
  for await (const event of rpc.sendChat({
    workspacePath: workspace,
    message: 'M2_LOCAL_JOB: review explicit rebinding',
    historyJson: '[]',
    optionsJson: JSON.stringify({
      provider: 'openai-chat',
      model: 'fixture',
      max_tokens: 512,
      max_iterations: 7,
      permission_mode: 'ask',
      continuation_policy: {
        detached: true,
        purpose: 'Retain this original task and its limits',
        max_turns: 4,
        token_limit: 300000,
        expires_ms: Date.now() + 3600000,
      },
    }),
  })) {
    expect(event.kind, event.content).not.toBe('error')
    if (event.kind === 'approval') {
      expect(JSON.parse(event.content).command).toBe('node -e "setTimeout(() => {}, 8000)"')
      await rpc.respondApproval({
        workspacePath: workspace,
        requestId: JSON.parse(event.detailJson).request_id,
        decision: 'AllowOnce',
      })
    }
    if (event.kind === 'waiting') taskId = JSON.parse(event.detailJson).task_id
  }
  expect(taskId).toBeTruthy()
  const read = () => resource('queryContinuations', { operation: 'read', id: taskId })
  let before = await read()
  await resource('controlContinuation', { id: taskId, action: 'pause' }, before.revision)
  await expect
    .poll(async () => Object.values((await read()).task.sources).every((s: any) => !!s.terminal), {
      timeout: 20000,
    })
    .toBe(true)
  before = await read()
  for await (const event of rpc.sendChat({
    workspacePath: workspace,
    sessionId: before.task.session,
    message: 'New instruction: preserve the existing API',
    historyJson: '[]',
    optionsJson: JSON.stringify({
      provider: 'mock',
      mock_text: 'New constraint saved',
      max_tokens: 128,
    }),
  }))
    expect(event.kind, event.content).not.toBe('error')
  const stale = await read()
  expect(stale.workflow.reason_code).toBe('conversation_changed')
  await page.goto(`/?open=${encodeURIComponent(workspace)}`)
  const nav = page.getByRole('navigation', { name: 'Main navigation' })
  await nav.getByRole('button', { name: 'Chat', exact: true }).click()
  await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Keep this unsent draft')
  await nav.getByRole('button', { name: 'Tasks', exact: true }).click()
  await page.getByRole('button', { name: 'Review new binding', exact: true }).click()
  const dialog = page.getByRole('dialog', { name: 'Review new binding' })
  await expect(dialog).toContainText('preserve the existing API')
  await expect(dialog).toContainText('1 of 4 turns used')
  await page.screenshot({ path: testInfo.outputPath('binding-review.png') })
  expect((await read()).task.budget).toEqual(before.task.budget)
  await dialog.getByRole('button', { name: 'Confirm binding and continue' }).click()
  await expect(dialog).toBeHidden()
  await expect.poll(async () => (await read()).task.state, { timeout: 20000 }).toBe('completed')
  const after = await read()
  expect(after.task.id).toBe(before.task.id)
  expect(after.task.session).toBe(before.task.session)
  expect(after.task.generation).toBe(before.task.generation)
  expect(after.task.budget.used_turns).toBe(2)
  expect(after.task.budget.consumed_tokens).toBeGreaterThan(before.task.budget.consumed_tokens)
  expect(Object.values(after.task.events).every((e: any) => e.consumed)).toBe(true)
  expect(after.task.dispatch_history).toHaveLength(1)
  await nav.getByRole('button', { name: 'Chat', exact: true }).click()
  await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toHaveValue(
    'Keep this unsent draft',
  )
  await page.reload()
  expect((await read()).task.budget.used_turns).toBe(2)
  await testInfo.attach('binding-ledger', {
    body: JSON.stringify({ before, stale, after }, null, 2),
    contentType: 'application/json',
  })
})
