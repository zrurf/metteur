import { test, expect, type Page } from '@playwright/test'
import { createClient } from '@connectrpc/connect'
import { createGrpcWebTransport } from '@connectrpc/connect-web'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { dirname, join } from 'node:path'
import { randomUUID } from 'node:crypto'
import { Daemon } from '../../src/gen/metteur_pb'
import type { TaskRecord, TaskSource } from '../../src/core/continuations'

let workspace = ''
const client = () =>
  createClient(Daemon, createGrpcWebTransport({ baseUrl: process.env.LIVE_BASE_URL! }))
test.beforeEach(async () => {
  const fixture = await (await fetch(`${process.env.LIVE_FIXTURE_URL!}/fixture`)).json()
  expect(fixture.nonce).toBe(process.env.LIVE_NONCE)
  workspace = join(dirname(process.env.LIVE_WORKSPACE!), `daily-story-${randomUUID()}`)
  await mkdir(join(workspace, 'blueprints'), { recursive: true })
  await writeFile(
    join(workspace, 'app.ts'),
    `export const evidence = '${process.env.LIVE_NONCE}'\n`,
  )
  await client().openWorkspace({ path: workspace })
})
async function records() {
  const response = await client().queryContinuations({
    schemaVersion: 1,
    requestId: randomUUID(),
    workspacePath: workspace,
    payloadJson: JSON.stringify({ operation: 'list', after: null }),
  })
  expect(response.errorJson).toBe('')
  return JSON.parse(response.payloadJson).items as TaskRecord[]
}
async function graph(id: string, file: string, command: string) {
  const source = `blueprint "Daily workflow check"\nentry start: Start\nrun: ExecuteCommand(command = ${JSON.stringify(command)})\nverify: Validator(Actual <- run.Result, mode = "regex", Expected = 0)\nend: End\nstart -> run\nrun -> verify\nverify -> end`
  const blueprint = await client().compileDsl({ workspacePath: workspace, source })
  const check = blueprint.nodes.find((node) => node.kind === 'Validator')!
  check.dataJson = JSON.stringify({
    ...JSON.parse(check.dataJson),
    regex: '"exit_code"\\s*:\\s*0(?:,|})',
  })
  blueprint.id = id
  await client().saveBlueprint({
    workspacePath: workspace,
    blueprint,
    filePath: `blueprints/${file}.blueprint`,
  })
}
async function start(page: Page, goal: string) {
  const previous = new Set((await records()).map((r) => r.task.id))
  await page
    .getByRole('navigation', { name: 'Main navigation' })
    .getByRole('button', { name: 'Tasks', exact: true })
    .click()
  await page.getByRole('button', { name: 'New task', exact: true }).click()
  const dialog = page.getByRole('dialog', { name: 'Start a Continuation Task', exact: true })
  await dialog.getByRole('textbox', { name: 'Goal', exact: true }).fill(goal)
  await dialog
    .getByRole('checkbox', {
      name: 'Allow this task to continue when this browser disconnects',
      exact: false,
    })
    .uncheck()
  await dialog
    .getByRole('checkbox', {
      name: 'Authorize this exact goal, model, time and limits',
      exact: false,
    })
    .check()
  await dialog.getByRole('button', { name: 'Start task', exact: true }).click()
  await expect
    .poll(async () =>
      (await records()).some((r) => r.policy?.purpose === goal && !previous.has(r.task.id)),
    )
    .toBe(true)
  return (await records()).find((r) => r.policy?.purpose === goal && !previous.has(r.task.id))!.task
    .id
}
async function approvePhase(page: Page) {
  const approval = page.getByRole('dialog').filter({ hasText: 'Approve file change' })
  const launch = page.getByRole('dialog', { name: 'Review a Blueprint Run', exact: false })
  // File approval is required only when the current sandbox policy asks for it.
  await expect
    .poll(
      async () => {
        await commandApproval(page)
        return (await approval.isVisible()) || (await launch.isVisible())
      },
      { timeout: 30000 },
    )
    .toBe(true)
  if (await approval.isVisible())
    await approval.getByRole('button', { name: 'Allow', exact: true }).click()
  await expect
    .poll(
      async () => {
        await reviewRequested(page)
        return launch.isVisible()
      },
      { timeout: 30000 },
    )
    .toBe(true)
  await launch
    .getByRole('checkbox', { name: 'Resume this conversation when the run finishes', exact: true })
    .check()
  await launch
    .getByRole('checkbox', {
      name: 'I approve this version, handoff scope and completion policy',
      exact: true,
    })
    .check()
  await launch.getByRole('button', { name: 'Confirm and run', exact: true }).click()
  await expect(launch).toHaveCount(0)
}
async function commandApproval(page: Page) {
  await reviewRequested(page)
  const dialog = page.getByRole('dialog').filter({ hasText: 'Approve shell command' })
  if (await dialog.isVisible()) {
    await expect(dialog.locator('pre')).toHaveText(
      /^py -3\.12 -m unittest test_(?:shared\.py|stage1\.py(?: test_stage2\.py)?)$/,
    )
    await dialog.getByRole('button', { name: 'Allow', exact: true }).click()
  }
}
async function reviewRequested(page: Page) {
  const review = page.getByRole('button', { name: 'Review requested action', exact: true })
  if (
    (await page.getByRole('dialog').count()) === 0 &&
    (await review.isVisible()) &&
    (await review.isEnabled())
  )
    await review.click()
}
async function finished(page: Page, taskId: string, count: number, maxTurns = 4) {
  await expect
    .poll(
      async () => {
        await commandApproval(page)
        const record = (await records()).find((r) => r.task.id === taskId)
        return record?.task.state
      },
      { timeout: 45000 },
    )
    .toBe('completed')
  const record = (await records()).find((r) => r.task.id === taskId)!
  const sources = Object.values(record.task.sources).filter(
    (s) => s.kind === 'blueprint',
  ) as TaskSource[]
  expect(sources).toHaveLength(count)
  expect(Object.values(record.task.sources).filter((s) => s.kind === 'job')).toHaveLength(count)
  expect(record.task.budget.used_turns).toBe(count + 1)
  expect(record.task.budget.max_turns).toBe(maxTurns)
  expect(record.task.budget.unknown_usage).toBe(false)
  expect(record.task.budget.consumed_tokens).toBeGreaterThan(0)
  expect(Object.values(record.task.sources).every((s) => s.terminal === 'succeeded')).toBe(true)
  for (const source of sources) {
    const response = await client().queryContinuations({
      schemaVersion: 1,
      requestId: randomUUID(),
      workspacePath: workspace,
      payloadJson: JSON.stringify({
        operation: 'capture_result',
        reference: { kind: 'task', taskId: record.task.id, sourceId: source.id, digest: '' },
      }),
    })
    expect(response.errorJson).toBe('')
    const result = JSON.parse(JSON.parse(response.payloadJson).content)
    expect(result.recorded_checks.filter((c: { current: boolean }) => c.current)).toHaveLength(1)
    expect(
      result.recorded_checks
        .filter((c: { current: boolean; check: boolean }) => c.current)
        .every((c: { check: boolean }) => c.check),
    ).toBe(true)
  }
  return record
}
test('Story A: real cross-file fix, host-recorded unittest, original dirty draft and retained result', async ({
  page,
}, testInfo) => {
  test.setTimeout(120000)
  const original = 'def normalize(value):\n    return value.strip()\n'
  const files: Record<string, string> = {
    'shared.py': original,
    'alpha.py': 'from shared import normalize\ndef alpha(value):\n    return normalize(value)\n',
    'beta.py': 'from shared import normalize\ndef beta(value):\n    return normalize(value)\n',
    'test_shared.py':
      'import unittest, time\nfrom alpha import alpha\nfrom beta import beta\nclass SharedTest(unittest.TestCase):\n    def test_both_callers(self):\n        time.sleep(8)\n        self.assertEqual(alpha(" A "), "a")\n        self.assertEqual(beta(" B "), "b")\n',
  }
  for (const [name, content] of Object.entries(files))
    await writeFile(join(workspace, name), content)
  await graph(
    '5726e3bf-35c8-5e17-8942-78756af9c2cc',
    'review',
    'py -3.12 -m unittest test_shared.py',
  )
  const promptName = `Normalization rules ${randomUUID().slice(0, 8)}`
  for (const scope of ['personal', 'project']) {
    const response = await client().writePromptResource({
      schemaVersion: 1,
      requestId: randomUUID(),
      workspacePath: workspace,
      payloadJson: JSON.stringify({
        scope,
        draft: {
          id: randomUUID(),
          name: promptName,
          tags: [],
          body:
            scope === 'personal'
              ? 'DW_PERSONAL_NORMALIZATION: explain changes and preserve unrelated callers.'
              : 'DW_PROJECT_NORMALIZATION: keep both public caller signatures and validate normalization.',
        },
      }),
    })
    expect(response.errorJson).toBe('')
  }
  await page.goto(`/?open=${encodeURIComponent(workspace)}`)
  const tree = page.getByTestId('file-tree')
  await expect(tree).toBeVisible()
  await tree.getByRole('button', { name: 'shared.py', exact: true }).click()
  await expect(page.locator('.monaco-editor .view-lines')).toContainText('return value.strip()')
  await page.locator('.monaco-editor .view-lines').first().click()
  await page.keyboard.press('ControlOrMeta+a')
  await page.keyboard.insertText(original + '# my unsaved note\n')
  await expect(page.getByText('Backed Up on Host', { exact: true })).toBeVisible()
  const goal =
    'DW_STORY_A: fix normalization for both callers, validate, return here and retain the result.'
  await page.getByRole('button', { name: 'Chat', exact: true }).first().click()
  await tree.getByRole('button', { name: 'shared.py', exact: true }).click({ button: 'right' })
  await page.getByText('Add to Conversation', { exact: true }).click()
  await page
    .getByRole('dialog', { name: 'Reference File', exact: true })
    .getByRole('button', { name: 'Use This Saved Version', exact: true })
    .click()
  for (const scope of ['Personal', 'Project']) {
    await tree.getByRole('button', { name: 'Prompt Library', exact: true }).click()
    const library = page.getByRole('region', { name: 'Prompt Library', exact: true })
    await library.getByRole('tab', { name: scope, exact: true }).click()
    await library.getByRole('button', { name: `${scope} · r1 ${promptName}`, exact: true }).click()
    await expect(library.getByRole('heading', { name: promptName, exact: true })).toBeVisible()
    await library.getByRole('button', { name: 'Use in Chat', exact: true }).click()
  }
  const input = page.getByRole('textbox', { name: 'Message', exact: true })
  await input.fill(goal)
  await input.press('Enter')
  await approvePhase(page)
  await expect
    .poll(
      async () => {
        await commandApproval(page)
        return (await records()).some(
          (r) =>
            r.policy?.purpose === 'Validate story A, phase 1' &&
            r.task.state === 'waiting' &&
            Object.values(r.task.sources).some((s) => s.kind === 'job'),
        )
      },
      { timeout: 15000 },
    )
    .toBe(true)
  await page
    .getByRole('navigation', { name: 'Main navigation' })
    .getByRole('button', { name: 'Tasks', exact: true })
    .click()
  await page.getByRole('button', { name: 'Open execution', exact: true }).click()
  const supervision = page.getByRole('region', { name: 'Run concierge', exact: true })
  await expect(supervision.getByRole('textbox', { name: 'Message', exact: true })).toBeEnabled()
  await supervision
    .getByRole('textbox', { name: 'Message', exact: true })
    .fill(
      'DW03_SUPERVISION_MESSAGE: report the current host-recorded test progress; do not change files.',
    )
  await supervision.getByRole('button', { name: 'Send', exact: true }).click()
  await expect(supervision).toContainText('Recorded progress read through the local provider.', {
    timeout: 20000,
  })
  const taskId = (await records()).find((r) => r.policy?.purpose === 'Validate story A, phase 1')!
    .task.id
  const record = await finished(page, taskId, 1, 3)
  const proof = await (await page.request.get(`${process.env.LIVE_FIXTURE_URL!}/fixture`)).json()
  expect(proof.dailyStorySelectedPersonal).toBe(true)
  expect(proof.dailyStorySelectedProject).toBe(true)
  expect(proof.dailyStorySelectedFile).toBe(true)
  const memory = await client().queryProjectMemory({
    schemaVersion: 1,
    requestId: randomUUID(),
    workspacePath: workspace,
    payloadJson: JSON.stringify({ operation: 'read', id: 'c52dfc43-22cd-424a-8888-000000000002' }),
  })
  expect(memory.errorJson).toBe('')
  const note = JSON.parse(memory.payloadJson)
  expect(note.content.origin).toBe('model_opinion')
  expect(note.content.confirmation).toBeNull()
  expect(note.content.source).toMatch(/^run:/)
  expect(note.content.files).toHaveLength(3)
  expect(await readFile(join(workspace, 'shared.py'), 'utf8')).toContain(
    'return value.strip().lower()',
  )
  for (const name of ['alpha.py', 'beta.py'])
    expect(await readFile(join(workspace, name), 'utf8')).toBe(files[name])
  await page.getByTestId('tab').filter({ hasText: 'shared.py' }).click()
  await page.getByRole('button', { name: 'Compare Changes', exact: true }).click()
  const comparison = page.getByRole('dialog', { name: 'Compare Changes', exact: true })
  await expect(comparison).toContainText('my unsaved note')
  await expect(comparison).toContainText('return value.strip().lower()')
  await comparison.getByRole('button', { name: 'Keep Both', exact: true }).click()
  await comparison.getByLabel('New file path').fill('shared-my-draft.py')
  await comparison.getByRole('button', { name: 'Save New Copy', exact: true }).click()
  await expect(comparison).toHaveCount(0)
  expect(await readFile(join(workspace, 'shared-my-draft.py'), 'utf8')).toContain('my unsaved note')
  expect(await readFile(join(workspace, 'shared.py'), 'utf8')).toContain(
    'return value.strip().lower()',
  )
  await testInfo.attach('host-task', {
    body: JSON.stringify(record, null, 2),
    contentType: 'application/json',
  })
})
test('Story B: two real implementation/check phases preserve one parent and its consumed budget', async ({
  page,
}, testInfo) => {
  test.setTimeout(150000)
  await writeFile(
    join(workspace, 'feature.py'),
    'def pick(values, query, limit=None):\n    return []\n',
  )
  await writeFile(
    join(workspace, 'test_stage1.py'),
    'import unittest\nfrom feature import pick\nclass StageOne(unittest.TestCase):\n    def test_filter(self):\n        self.assertEqual(pick(["Alpha", "beta", "ALP"], "al"), ["Alpha", "ALP"])\n',
  )
  await writeFile(
    join(workspace, 'test_stage2.py'),
    'import unittest\nfrom feature import pick\nclass StageTwo(unittest.TestCase):\n    def test_limit(self):\n        self.assertEqual(pick(["a", "aa"], "a", 1), ["a"])\n        self.assertEqual(pick(["a"], "a", 0), [])\n        with self.assertRaises(ValueError):\n            pick(["a"], "a", -1)\n',
  )
  await graph(
    'b0548572-8998-4d1b-9f71-27d5946b0001',
    'stage-one',
    'py -3.12 -m unittest test_stage1.py',
  )
  await graph(
    'b0548572-8998-4d1b-9f71-27d5946b0002',
    'stage-two',
    'py -3.12 -m unittest test_stage1.py test_stage2.py',
  )
  await page.goto(`/?open=${encodeURIComponent(workspace)}`)
  await expect(page.getByTestId('file-tree')).toBeVisible()
  const goal =
    'DW_STORY_B_STAGED: implement filtering and validate stage one, then add limit handling and validate both stages under the same task.'
  const taskId = await start(page, goal)
  await approvePhase(page)
  await approvePhase(page)
  const record = await finished(page, taskId, 2)
  expect(await readFile(join(workspace, 'feature.py'), 'utf8')).toContain('return selected[:limit]')
  await page.reload()
  const reopened = (await records()).find((r) => r.task.id === record.task.id)
  expect(reopened!.task.budget).toEqual(record.task.budget)
  expect(Object.keys(reopened!.task.sources)).toEqual(Object.keys(record.task.sources))
  await testInfo.attach('host-task', {
    body: JSON.stringify(record, null, 2),
    contentType: 'application/json',
  })
})
