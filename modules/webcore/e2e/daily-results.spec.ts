import { test, expect, type Page } from '@playwright/test'
const sampleRoot = 'D:/metteur-demo/results'
async function open(page: Page) {
  await page.goto('/')
  await page.getByLabel('Workspace path').fill(sampleRoot)
  await page.getByRole('button', { name: 'Open', exact: true }).click()
  await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeEnabled()
  await page.evaluate(async () => {
    const c = '/src/core/index.ts',
      chatModule = '/src/stores/chat.ts',
      digestModule = '/src/core/file-reference.ts'
    const { gateway } = await import(c),
      chat = (await import(chatModule)).useChatStore()
    gateway.connected.value = true
    chat.draft = 'keep existing draft'
    const first = crypto.randomUUID(),
      second = crypto.randomUUID(),
      session = crypto.randomUUID()
    const task = (id: string, goal: string) => ({
      revision: 2,
      policy: { purpose: goal, expires_ms: Date.now() + 100_000 },
      schedules: [],
      has_saved_context: true,
      workflow: {
        goal,
        reason_code: 'conversation_changed',
        reason: 'Newer input superseded this wait',
        next_action: 'Review the old result or prepare a new proposal.',
        allowed_actions: ['view_results', 'add_to_draft', 'new_plan', 'stop'],
        waiting_on: [],
        target: { task: id, session, revision: 2 },
        origin: 'host_state',
      },
      task: {
        id,
        session,
        state: 'needs_attention',
        generation: 1,
        expected_revision: 1,
        paused: false,
        attention: 'A newer session revision superseded this wait',
        budget: {
          max_turns: 4,
          used_turns: 2,
          token_limit: 100_000,
          reserved_tokens: 0,
          consumed_tokens: 1200,
          unknown_usage: false,
        },
        sources: {
          saved: {
            id: 'saved',
            kind: 'job',
            generation: 1,
            may_write: false,
            terminal: 'succeeded',
            result: { artifact_ref: 'fixture' },
          },
        },
        wait: null,
      },
    })
    const fixture = (gateway.resultFixture = {
      rows: [task(first, 'First task'), task(second, 'Second task')],
      controls: [] as any[],
      sends: 0,
      captures: 0,
      reviews: 0,
      usage: 0,
      version: 1,
    })
    const body = JSON.stringify({
      run: { status: 'succeeded' },
      recorded_checks: [
        { check: false, current: true },
        { check: true, current: false },
      ],
      outputs: { model_text: 'Everything passed according to a model' },
    })
    const hash = await (await import(digestModule)).fileTextDigest(body)
    const original = gateway.resourceRequest.bind(gateway)
    gateway.resourceRequest = async (method: string, root: string, payload: any, expected = 0) => {
      if (method === 'recovery') {
        const action = payload.action
        if (action.operation === 'latest_view') return { ok: true, data: { record: null } }
        if (action.operation === 'list') return { ok: true, data: { items: [], next: null } }
        return {
          ok: true,
          data: {
            schema_version: 1,
            revision: expected + 1,
            discarded: action.operation === 'discard',
            input: action.input,
            updated_ms: 1,
          },
        }
      }
      if (method === 'queryContinuations') {
        if (payload.operation === 'capture_result') {
          fixture.captures++
          return {
            ok: true,
            data: {
              reference: { ...payload.reference, digest: hash },
              content: body,
              origin: 'source_data',
            },
          }
        }
        return { ok: true, data: { items: structuredClone(fixture.rows), next: null } }
      }
      if (method === 'controlContinuation') {
        fixture.controls.push({ root, payload, expected })
        return {
          ok: true,
          data: { ...fixture.rows.find((row) => row.task.id === payload.id), revision: 3 },
        }
      }
      return original(method, root, payload, expected)
    }
    gateway.sendChat = async () => {
      fixture.sends++
      return { ok: true, data: undefined }
    }
    gateway.listExecutions = async () => ({
      ok: true,
      data: [
        {
          runId: first,
          blueprintId: second,
          status: 'Completed',
          startedAt: 1,
          updatedAt: 2,
          executedNodes: 2,
        },
      ],
    })
    gateway.listOversightReports = async (_root: string, run: string) => {
      fixture.reviews++
      return { ok: true, data: { run_id: run, reports: [], closing: null } }
    }
    gateway.getExecutionUsage = async () => {
      fixture.usage++
      return { ok: true, data: { currency: 'USD', totalCostMicros: 0, models: [] } }
    }
  })
}
async function tasks(page: Page) {
  await page
    .getByRole('navigation', { name: 'Main navigation' })
    .getByRole('button', { name: 'Tasks', exact: true })
    .click()
  await expect(page.getByRole('heading', { name: 'First task', exact: true })).toBeVisible()
}

test('rebind requires concrete confirmation and retries a lost receipt with the same identity', async ({
  page,
}) => {
  await open(page)
  await page.evaluate(async () => {
    const c = '/src/core/index.ts',
      { gateway } = await import(c)
    const f = gateway.resultFixture,
      row = f.rows[0]
    row.workflow.allowed_actions.push('rebind')
    row.task.wait = { sources: ['saved'], mode: 'all', deadline_ms: Date.now() + 60000, params: {} }
    const original = gateway.resourceRequest.bind(gateway)
    f.rebinds = []
    gateway.resourceRequest = async (
      method: string,
      root: string,
      payload: any,
      expected: number,
      request: string,
    ) => {
      if (method === 'queryContinuations' && payload.operation === 'prepare_rebind')
        return {
          ok: true,
          data: {
            task: row.task.id,
            revision: row.revision,
            session: row.task.session,
            session_revision: 3,
            digest: 'a'.repeat(64),
            goal: row.policy.purpose,
            latest_user_input: 'Preserve the existing API',
            budget: row.task.budget,
            policy: row.policy,
            wait: row.task.wait,
          },
        }
      if (method === 'controlContinuation' && payload.action === 'rebind') {
        f.rebinds.push({ root, payload, expected, request })
        if (f.rebinds.length === 1)
          return { ok: false, error: 'Connection lost after confirmation' }
        row.workflow.allowed_actions = ['stop']
        return { ok: true, data: row }
      }
      return original(method, root, payload, expected, request)
    }
  })
  await tasks(page)
  await page.getByRole('button', { name: 'Review new binding', exact: true }).click()
  const dialog = page.getByRole('dialog', { name: 'Review new binding' })
  await expect(dialog).toContainText('Preserve the existing API')
  await expect(dialog).toContainText('2 of 4 turns used')
  expect(
    await page.evaluate(
      async () => (await import('/src/core/index.ts')).gateway.resultFixture.rebinds.length,
    ),
  ).toBe(0)
  await dialog.getByRole('button', { name: 'Confirm binding and continue' }).click()
  await expect(dialog.getByRole('alert')).toContainText('Connection lost')
  await dialog.getByRole('button', { name: 'Confirm binding and continue' }).click()
  await expect(dialog).toBeHidden()
  const result = await page.evaluate(async () => {
    const c = '/src/core/index.ts',
      s = '/src/stores/chat.ts'
    const f = (await import(c)).gateway.resultFixture
    return {
      calls: f.rebinds,
      sends: f.sends,
      draft: (await import(s)).useChatStore().draft,
      budget: f.rows[0].task.budget,
    }
  })
  expect(result.calls).toHaveLength(2)
  expect(result.calls[0]).toEqual(result.calls[1])
  expect(result.sends).toBe(0)
  expect(result.draft).toBe('keep existing draft')
  expect(result.budget.consumed_tokens).toBe(1200)
})

test('saved results keep failed checks visible and enter the existing draft without a model call', async ({
  page,
}) => {
  await open(page)
  await tasks(page)
  await expect(page.getByText('Newer input superseded this wait', { exact: true })).toBeVisible()
  await expect(
    page.getByRole('button', { name: 'Pause automatic continuation', exact: true }),
  ).toBeDisabled()
  await page.getByText('job · saved result', { exact: true }).click()
  await expect(page.getByText('1 check(s) failed', { exact: true })).toBeVisible()
  await expect(
    page.getByText('Everything passed according to a model', { exact: false }),
  ).not.toBeVisible()
  await page.getByRole('button', { name: 'Add result to draft', exact: true }).click()
  await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toHaveValue(
    'keep existing draft\n\nReview the selected saved result.',
  )
  await expect(page.getByLabel('Selected saved results')).toContainText('job · Saved result')
  expect(
    await page.evaluate(async () => {
      const c = '/src/core/index.ts',
        d = '/src/stores/chat.ts'
      return {
        sends: (await import(c)).gateway.resultFixture.sends,
        selected: (await import(d)).useChatStore().pendingResults.length,
      }
    }),
  ).toEqual({ sends: 0, selected: 1 })
})

test('a Stop confirmation stays bound to the original task while background selection changes', async ({
  page,
}) => {
  await open(page)
  await tasks(page)
  const first = await page.evaluate(async () => {
    const c = '/src/core/index.ts'
    return (await import(c)).gateway.resultFixture.rows[0].task.id
  })
  await page.getByRole('button', { name: 'Stop task', exact: true }).click()
  await expect(page.getByRole('dialog', { name: 'Stop task' })).toContainText('First task')
  await page.evaluate(async () => {
    const c = '/src/core/index.ts',
      e = '/src/stores/workspace-events.ts'
    ;(await import(c)).gateway.resultFixture.rows.reverse()
    ;(await import(e)).useWorkspaceEventsStore().revision++
  })
  await expect(page.getByRole('heading', { name: 'Second task', exact: true })).toBeVisible()
  await page
    .getByRole('dialog', { name: 'Stop task' })
    .getByRole('button', { name: 'Stop task', exact: true })
    .click()
  expect(
    await page.evaluate(async () => {
      const c = '/src/core/index.ts'
      return (await import(c)).gateway.resultFixture.controls.map((x: any) => x.payload.id)
    }),
  ).toEqual([first])
})

test('expired authorization without a source opens a new-plan draft without resuming or resetting the task', async ({
  page,
}) => {
  await open(page)
  await page.evaluate(async () => {
    const c = '/src/core/index.ts'
    const row = (await import(c)).gateway.resultFixture.rows[0]
    row.task.sources = {}
    row.workflow.reason_code = 'policy_expired'
    row.workflow.reason = 'Continuation authorization expired'
  })
  await tasks(page)
  await page.getByRole('button', { name: 'Prepare new plan in draft', exact: true }).click()
  await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toHaveValue(
    /keep existing draft\n\nPrepare a new plan/,
  )
  const result = await page.evaluate(async () => {
    const c = '/src/core/index.ts',
      s = '/src/stores/chat.ts'
    const f = (await import(c)).gateway.resultFixture
    return {
      draft: (await import(s)).useChatStore().draft,
      sends: f.sends,
      controls: f.controls.length,
      budget: f.rows[0].task.budget,
    }
  })
  expect(result.draft).toContain('keep existing draft\n\nPrepare a new plan')
  expect(result.draft).toContain('Continuation authorization expired')
  expect(result).toMatchObject({
    sends: 0,
    controls: 0,
    budget: { used_turns: 2, consumed_tokens: 1200 },
  })
})

test('reviews and usage stay idle for ten seconds and coalesce duplicate business notifications', async ({
  page,
}) => {
  test.setTimeout(30_000)
  await open(page)
  await page.getByRole('button', { name: 'Execution', exact: true }).first().click()
  await page.getByRole('button', { name: 'Reviews', exact: true }).click()
  await page.getByRole('button', { name: 'Usage', exact: true }).click()
  await expect(page.getByText('No supervisor reviews recorded.', { exact: true })).toBeVisible()
  await expect(page.getByText('Cache hit rate', { exact: true })).toBeVisible()
  const count = () =>
    page.evaluate(async () => {
      const c = '/src/core/index.ts'
      const f = (await import(c)).gateway.resultFixture
      return { reviews: f.reviews, usage: f.usage }
    })
  await expect.poll(count).toEqual({ reviews: 1, usage: 1 })
  await page.waitForTimeout(10_000)
  expect(await count()).toEqual({ reviews: 1, usage: 1 })
  await page.evaluate(async () => {
    const e = '/src/stores/workspace-events.ts',
      events = (await import(e)).useWorkspaceEventsStore()
    events.revision++
    events.revision++
    events.revision++
  })
  await expect.poll(count).toEqual({ reviews: 2, usage: 2 })
})
