import { test, expect, type Page } from '@playwright/test'
// L2 actual Supervision/Composer/controllers with a persistent synthetic host.
// This does not replace the native receipt/WAL or production-chain checks.
function host() {
  const run = crypto.randomUUID(),
    records = new Map<string, any>(),
    receipts = new Map<string, any>(),
    calls: any[] = []
  const flags = { choice: 'delayed', offline: false, sent: 0 }
  async function handle({ method, payload, expected = 0 }: any) {
    calls.push({ method, payload, expected })
    if (method === 'state')
      return {
        ok: true,
        data: {
          run_id: run,
          conversation_id: run,
          available: true,
          read_only: false,
          reason: '',
          consumer_enabled: false,
          messages: [...receipts.values()].flatMap((receipt) =>
            receipt.turn ? [receipt.turn] : [],
          ),
          requests: [],
        },
      }
    if (method === 'send') {
      flags.sent++
      const turn = {
        id: payload.id,
        conversation_id: run,
        original_text: payload.text,
        at_ms: 1,
        state: 'processing',
        answer: '',
        request_id: null,
        error: null,
      }
      const receipt = {
        schema_version: 1,
        id: payload.id,
        run,
        conversation_id: run,
        phase: 'accepted',
        turn,
      }
      receipts.set(payload.id, receipt)
      if (flags.choice === 'unknown') {
        flags.offline = true
        return { ok: false, error: 'Lost acknowledgement' }
      }
      return { ok: true, data: { receipt, choice: flags.choice } }
    }
    if (method === 'finish') {
      const receipt = receipts.get(payload.id)
      Object.assign(receipt.turn, { state: 'failed', error: 'Synthetic model failure', answer: '' })
      return { ok: true, data: receipt.turn }
    }
    if (method === 'conciergeDelivery') {
      if (payload.operation === 'capabilities')
        return {
          ok: true,
          data: { durable_intake: true, read_or_retire: true, pending_reconciliation: true },
        }
      if (payload.operation === 'reconcile_pending') return { ok: true, data: { items: [] } }
      if (flags.offline) return { ok: false, error: 'Disconnected receipt query' }
      if (!receipts.has(payload.id))
        receipts.set(payload.id, {
          schema_version: 1,
          id: payload.id,
          run,
          conversation_id: run,
          phase: 'retired',
          turn: null,
        })
      return { ok: true, data: { receipt: receipts.get(payload.id), source_available: true } }
    }
    if (method === 'recovery') {
      const action = payload.action,
        id = action.id ?? action.input?.id,
        current = records.get(id)
      if (action.operation === 'latest_view') return { ok: true, data: { record: null } }
      if (action.operation === 'list')
        return {
          ok: true,
          data: {
            items: [...records.values()].map((record) => ({
              id: record.input.id,
              kind: record.input.kind,
              entity: record.input.entity,
              window_id: record.input.window_id,
              revision: record.revision,
              edit_sequence: record.input.edit_sequence,
              updated_ms: 1,
              discarded: record.discarded,
              error: null,
            })),
            next: null,
          },
        }
      if (action.operation === 'read')
        return {
          ok: true,
          data: { record: current, source_available: true, requires_current_validation: true },
        }
      if ((current?.revision ?? 0) !== expected || current?.discarded)
        return { ok: false, error: 'revision_conflict' }
      const record =
        action.operation === 'discard'
          ? {
              ...current,
              revision: expected + 1,
              discarded: true,
              input: { ...current.input, raw: '', pending_submission: null },
            }
          : {
              schema_version: 1,
              revision: expected + 1,
              input: structuredClone(action.input),
              discarded: false,
              updated_ms: 1,
            }
      records.set(id, record)
      return { ok: true, data: structuredClone(record) }
    }
    return { ok: false, error: 'Unexpected fixture request' }
  }
  return { run, records, receipts, calls, flags, handle }
}

test('L2 supervision gateway forwards a real processing receipt and rejects a different input identity', async ({
  page,
}) => {
  await page.goto('/')
  const result = await page.evaluate(async () => {
    const path = '/src/core/grpc-gateway.ts',
      { GrpcGateway } = await import(path),
      gateway = new GrpcGateway('')
    const run = crypto.randomUUID(),
      id = crypto.randomUUID(),
      raw = '  raw\r\n中文  ',
      events: any[] = []
    let wrong = false
    gateway.client = {
      sendConciergeMessage: async function* () {
        yield { runId: run, messageId: id, kind: 'processing', detailJson: '{}' }
        yield {
          runId: run,
          messageId: id,
          kind: 'processing',
          detailJson: JSON.stringify({
            id: wrong ? crypto.randomUUID() : id,
            conversation_id: run,
            original_text: raw,
            state: 'processing',
          }),
        }
      },
    }
    const accepted = await gateway.sendConciergeMessage(
      'workspace',
      run,
      run,
      id,
      raw,
      (event: any) => events.push(event),
    )
    wrong = true
    const rejected = await gateway.sendConciergeMessage('workspace', run, run, id, raw, () => {})
    return { accepted, rejected, events, raw }
  })
  expect(result.accepted.ok).toBe(true)
  expect(result.rejected.ok).toBe(false)
  expect(result.events[0].turn).toBeUndefined()
  expect(result.events[1].turn.original_text).toBe(result.raw)
})
async function install(page: Page, fixture: ReturnType<typeof host>, reopen = false) {
  if (!reopen) await page.exposeFunction('supervisionHost', fixture.handle)
  await page.goto('/')
  if (await page.getByLabel('Workspace path').count()) {
    await page.getByLabel('Workspace path').fill('D:/metteur-demo/supervision-input')
    await page.getByRole('button', { name: 'Open', exact: true }).click()
  }
  await expect(page.getByRole('button', { name: 'Execution', exact: true })).toBeVisible()
  await page.evaluate(async (run) => {
    const core = '/src/core/index.ts',
      executionPath = '/src/stores/execution.ts',
      routerPath = '/src/router.ts',
      urlPath = '/src/lib/workspace-url.ts'
    const { gateway } = await import(core)
    Object.defineProperty(gateway, 'demo', { value: false, configurable: true })
    gateway.connected.value = true
    const original = gateway.resourceRequest.bind(gateway)
    gateway.resourceRequest = (
      method: string,
      ws: string,
      payload: any,
      expected = 0,
      ...rest: any[]
    ) =>
      ['recovery', 'conciergeDelivery'].includes(method)
        ? (window as any).supervisionHost({ method, payload, expected })
        : original(method, ws, payload, expected, ...rest)
    gateway.listExecutions = async () => ({
      ok: true,
      data: [
        {
          runId: run,
          blueprintId: 'fixture',
          status: 'Running',
          startedAt: Date.now(),
          updatedAt: Date.now(),
          snapshot: { runtime: { active: true, pending_approval_ids: [] } },
        },
      ],
    })
    gateway.getConciergeState = () =>
      (window as any).supervisionHost({ method: 'state', payload: {} })
    gateway.sendConciergeMessage = async (
      _ws: string,
      _run: string,
      _conversation: string,
      id: string,
      text: string,
      receive: any,
    ) => {
      const result = await (window as any).supervisionHost({
        method: 'send',
        payload: { id, text },
      })
      if (!result.ok) return result
      if (result.data.choice === 'delayed')
        await new Promise<void>((resolve) => ((window as any).supervisionDeliver = resolve))
      receive({ runId: run, messageId: id, kind: 'processing', turn: result.data.receipt.turn })
      await new Promise<void>((resolve) => ((window as any).supervisionFinish = resolve))
      const terminal = await (window as any).supervisionHost({ method: 'finish', payload: { id } })
      receive({ runId: run, messageId: id, kind: 'failed', turn: terminal.data })
      return { ok: false, error: 'Synthetic model failure after intake' }
    }
    await (await import(executionPath)).useExecutionStore().reconcile(run)
    await (await import(routerPath)).router.push((await import(urlPath)).wurl('/execution'))
  }, fixture.run)
  const panel = page.getByRole('region', { name: 'Run concierge' }),
    editor = panel.getByLabel('Message', { exact: true })
  await expect(editor).toHaveAttribute('placeholder', 'Ask about this run or send a request')
  return { panel, editor }
}
test('L2 supervision keeps newer typing while a delayed intake ACK and failed model preserve the original', async ({
  page,
}) => {
  const fixture = host(),
    { panel, editor } = await install(page, fixture)
  await editor.fill('  original supervision  ')
  await panel.getByRole('button', { name: 'Send', exact: true }).click()
  await expect
    .poll(() => page.evaluate(() => typeof (window as any).supervisionDeliver))
    .toBe('function')
  expect(fixture.flags.sent).toBe(1)
  expect(
    [...fixture.records.values()].filter(
      (record) => record.input.raw === '  original supervision  ',
    ).length,
  ).toBeGreaterThanOrEqual(2)
  await expect(editor).toHaveValue('  original supervision  ')
  await editor.fill('newer before receipt')
  await page.evaluate(() => (window as any).supervisionDeliver())
  await expect(panel.getByRole('log', { name: 'Conversation' })).toContainText(
    'original supervision',
  )
  await expect(editor).toHaveValue('newer before receipt')
  await expect
    .poll(() => page.evaluate(() => typeof (window as any).supervisionFinish))
    .toBe('function')
  await page.evaluate(() => (window as any).supervisionFinish())
  await expect(panel).toContainText('Synthetic model failure')
  await expect(editor).toHaveValue('newer before receipt')
  await expect(panel.locator('[data-supervision-draft-status]')).toContainText('Backed up on host')
  expect(
    [...fixture.records.values()].some((record) => record.input.raw === 'newer before receipt'),
  ).toBe(true)
  const size = await panel
    .locator('[data-supervision-draft-status]')
    .evaluate((element) => parseFloat(getComputedStyle(element).fontSize))
  expect(size).toBeGreaterThanOrEqual(13)
  await page.screenshot({ path: test.info().outputPath('supervision-input-density.png') })
})
test('L2 unknown supervision intake blocks another send and reconciles without another model call', async ({
  page,
}) => {
  const fixture = host()
  fixture.flags.choice = 'unknown'
  const { panel, editor } = await install(page, fixture)
  await editor.fill('keep unknown input')
  await editor.press('Enter')
  await expect(panel.getByRole('button', { name: 'Check Receipt', exact: true })).toBeVisible()
  await expect(editor).toHaveValue('keep unknown input')
  await editor.press('Enter')
  expect(fixture.flags.sent).toBe(1)
  fixture.flags.offline = false
  await panel.getByRole('button', { name: 'Check Receipt', exact: true }).click()
  await expect(editor).toHaveValue('')
  expect(fixture.flags.sent).toBe(1)
})
test('L2 host-confirmed supervision draft restores after reload without sending or choosing a model', async ({
  page,
}) => {
  const fixture = host()
  const { editor } = await install(page, fixture)
  await editor.fill('supervision draft across reload')
  await expect(page.locator('[data-supervision-draft-status]')).toContainText('Backed up on host')
  const reopened = await install(page, fixture, true)
  await expect(reopened.editor).toHaveValue('')
  await reopened.panel.getByRole('button', { name: 'Recovery Copies', exact: true }).click()
  await page.getByRole('button', { name: 'Review Copy', exact: true }).first().click()
  await page.getByRole('button', { name: 'Use This Copy', exact: true }).click()
  await expect(reopened.editor).toHaveValue('supervision draft across reload')
  expect(fixture.flags.sent).toBe(0)
})
