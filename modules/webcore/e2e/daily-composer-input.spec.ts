import { test, expect, type Page } from '@playwright/test'

// L2: production Composer/store/controllers, synthetic transport. This does
// not stand in for durable RocksDB or a production provider chain.
function host() {
  const records = new Map<string, any>(),
    receipts = new Map<string, any>(),
    calls: any[] = []
  let failBackup = false,
    offline = false,
    unsupported = false
  const handle = async ({ method, payload, expected = 0, request }: any) => {
    calls.push({ method, payload, expected, request })
    if (method === 'chatDelivery') {
      if (payload.operation === 'admission')
        return {
          ok: true,
          data: { state: 'idle', run: null, session: null, turn: null, reason: '' },
        }
      if (unsupported) return { ok: false, error: 'Unimplemented' }
      if (offline) return { ok: false, error: 'Disconnected' }
      if (payload.operation === 'capabilities')
        return { ok: true, data: { durable_manual_send: true, pending_reconciliation: true } }
      if (payload.operation === 'reconcile_pending') return { ok: true, data: { items: [] } }
      if (payload.operation === 'session_revision')
        return { ok: true, data: { session: payload.session, revision: 1 } }
      if (payload.operation === 'retire' && !receipts.has(payload.id))
        receipts.set(payload.id, {
          schema_version: 1,
          id: payload.id,
          phase: 'retired',
          accepted_revision: 0,
          session: '',
          turn: '',
        })
      return {
        ok: true,
        data: { receipt: receipts.get(payload.id) ?? null, source_available: true },
      }
    }
    if (method === 'send') {
      if (payload.choice === 'reject') return { ok: false, error: 'Synthetic admission refused' }
      if (payload.choice === 'unknown') {
        offline = true
        return { ok: false, error: 'Connection lost before receipt' }
      }
      const receipt = {
        schema_version: 1,
        id: payload.options.delivery.id,
        input_digest: 'fixture',
        session: payload.session || payload.options.delivery.new_session,
        turn: crypto.randomUUID(),
        checkpoint: 'fixture-checkpoint',
        accepted_revision: 2,
        phase: 'started',
        outcome: null,
        accepted_ms: 1,
        updated_ms: 1,
      }
      receipts.set(receipt.id, receipt)
      return { ok: true, data: receipt }
    }
    if (method === 'finish') {
      const receipt = receipts.get(payload.id)
      if (receipt)
        Object.assign(receipt, { phase: 'finished', outcome: payload.failed ? 'error' : 'done' })
      return { ok: true, data: null }
    }
    const action = payload.action,
      id = action.id || action.input?.id,
      current = records.get(id)
    if (action.operation === 'latest_view') return { ok: true, data: { record: null } }
    if (action.operation === 'list')
      return {
        ok: true,
        data: {
          items: [...records.values()].map((r) => ({
            id: r.input.id,
            kind: r.input.kind,
            entity: r.input.entity,
            window_id: r.input.window_id,
            revision: r.revision,
            edit_sequence: r.input.edit_sequence,
            updated_ms: 1,
            discarded: r.discarded,
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
    if (action.operation === 'backup' && failBackup)
      return { ok: false, error: '{"code":"scope_capacity","message":"Backup full"}' }
    if (current?.request === request) return { ok: true, data: current }
    if ((current?.revision ?? 0) !== expected || current?.discarded)
      return { ok: false, error: '{"code":"revision_conflict"}' }
    const record =
      action.operation === 'discard'
        ? {
            ...current,
            revision: expected + 1,
            discarded: true,
            input: { ...current.input, raw: '', selections: [], pending_submission: null },
          }
        : {
            schema_version: 1,
            revision: expected + 1,
            input: structuredClone(action.input),
            discarded: false,
            updated_ms: 1,
          }
    record.request = request
    records.set(id, record)
    return { ok: true, data: structuredClone(record) }
  }
  return {
    records,
    receipts,
    calls,
    handle,
    set failBackup(value: boolean) {
      failBackup = value
    },
    set offline(value: boolean) {
      offline = value
    },
    set unsupported(value: boolean) {
      unsupported = value
    },
  }
}
async function install(page: Page, fixture: ReturnType<typeof host>, reopen = false) {
  if (!reopen) await page.exposeFunction('dwInputTransport', fixture.handle)
  await page.goto('/')
  if (await page.getByLabel('Workspace path').count()) {
    await page.getByLabel('Workspace path').fill('D:/metteur-demo/inputs')
    await page.getByRole('button', { name: 'Open', exact: true }).click()
  }
  await expect(page.getByLabel('Message', { exact: true })).toBeVisible()
  await page.evaluate(async () => {
    const path = '/src/core/index.ts',
      { gateway } = await import(path)
    Object.defineProperty(gateway, 'demo', { value: false, configurable: true })
    gateway.connected.value = true
    const original = gateway.resourceRequest.bind(gateway)
    gateway.resourceRequest = (
      method: string,
      _ws: string,
      payload: any,
      expected = 0,
      request = crypto.randomUUID(),
    ) =>
      ['recovery', 'chatDelivery'].includes(method)
        ? (window as any).dwInputTransport({ method, payload, expected, request })
        : original(method, _ws, payload, expected, request)
    gateway.sendChat = async (
      _ws: string,
      message: string,
      history: any,
      onMessage: any,
      options: any,
      _onSession: any,
      session: string,
      _usage: any,
      _todos: any,
      _signal: any,
      _progress: any,
      _context: any,
      _approval: any,
      onDelivery: any,
    ) => {
      const result = await (window as any).dwInputTransport({
        method: 'send',
        payload: {
          message,
          history,
          options,
          session,
          choice: (window as any).dwChoice ?? 'reject',
        },
      })
      if (!result.ok) return result
      if ((window as any).dwChoice === 'delayed')
        await new Promise<void>((resolve) => {
          ;(window as any).dwDeliver = resolve
        })
      onDelivery(result.data)
      await new Promise<void>((resolve) => {
        ;(window as any).dwRelease = resolve
      })
      onMessage({
        id: 'fixture-answer',
        role: 'assistant',
        content: 'Synthetic result',
        createdAt: 1,
      })
      await (window as any).dwInputTransport({
        method: 'finish',
        payload: { id: options.delivery.id, failed: true },
      })
      return { ok: false, error: 'Synthetic execution failure after acceptance' }
    }
  })
  await expect(page.getByLabel('Message', { exact: true })).toHaveAttribute(
    'placeholder',
    /Describe a task/,
  )
}
async function choose(page: Page, value: string) {
  await page.evaluate((choice) => {
    ;(window as any).dwChoice = choice
  }, value)
}
async function state(page: Page) {
  return page.evaluate(async () => {
    const path = '/src/stores/chat.ts',
      chat = (await import(path)).useChatStore()
    return {
      draft: chat.draft,
      entity: chat.draftEntity,
      session: chat.sessionId,
      busy: chat.busy,
      streaming: chat.streaming,
      input: JSON.parse(JSON.stringify(chat.composerInput?.input)),
      state: chat.composerInput?.state,
      files: chat.pendingFiles,
      messages: chat.messages.map((m) => ({ role: m.role, content: m.content })),
    }
  })
}

test('L2 rejection preserves exact raw text and attachment, with no optimistic accepted turn', async ({
  page,
}) => {
  const fixture = host()
  await install(page, fixture)
  await page.evaluate(async () => {
    const path = '/src/stores/chat.ts'
    ;(await import(path))
      .useChatStore()
      .queueFile({ kind: 'file', name: 'test.ts', path: 'src/test.ts' })
  })
  const editor = page.getByLabel('Message', { exact: true }),
    raw = '  中文原稿\n  keep trailing  '
  await editor.fill(raw)
  await editor.press('Enter')
  await expect.poll(async () => (await state(page)).busy).toBe(false)
  await expect(editor).toHaveValue(raw)
  const result = await state(page)
  expect(result.files.map((f) => f.path)).toEqual(['src/test.ts'])
  expect(result.messages.some((m) => m.role === 'user')).toBe(false)
  expect(fixture.calls.filter((c) => c.method === 'send')).toHaveLength(1)
  expect([...fixture.records.values()].some((r) => !r.discarded && r.input.raw === raw)).toBe(true)
})
test('L2 accepted version clears at receipt; execution failure preserves new typing', async ({
  page,
}) => {
  const fixture = host()
  await install(page, fixture)
  await choose(page, 'accept')
  const editor = page.getByLabel('Message', { exact: true })
  await editor.fill('Sent original')
  await editor.press('Enter')
  await expect(editor).toHaveValue('')
  await expect.poll(async () => (await state(page)).streaming).toBe(true)
  await editor.fill('Newer input while running')
  await page.evaluate(() => (window as any).dwRelease())
  await expect.poll(async () => (await state(page)).busy).toBe(false)
  await expect(editor).toHaveValue('Newer input while running')
  await expect(page.locator('.chat-thread-inner')).toContainText('Synthetic execution failure')
  const calls = fixture.calls,
    sendAt = calls.findIndex((c) => c.method === 'send')
  expect(
    calls
      .slice(0, sendAt)
      .filter((c) => c.method === 'recovery' && c.payload.action.operation === 'backup').length,
  ).toBeGreaterThanOrEqual(2)
})
test('L2 unknown receipt blocks repeat sends and only reconciles the original identity', async ({
  page,
}) => {
  const fixture = host()
  await install(page, fixture)
  await choose(page, 'unknown')
  const editor = page.getByLabel('Message', { exact: true })
  await editor.fill('Unknown input')
  await editor.press('Enter')
  await expect(page.getByRole('button', { name: 'Check Receipt', exact: true })).toBeVisible()
  await editor.press('Control+Enter')
  await editor.press('Enter')
  expect(fixture.calls.filter((c) => c.method === 'send')).toHaveLength(1)
  const id = (await state(page)).input.pending_submission
  fixture.offline = false
  await page.getByRole('button', { name: 'Check Receipt', exact: true }).click()
  await expect.poll(async () => (await state(page)).input.pending_submission).toBeNull()
  expect(fixture.receipts.get(id)?.phase).toBe('retired')
  await expect(editor).toHaveValue('Unknown input')
})
test('L2 IME candidate confirmation, repeated keys and Shift Enter never submit', async ({
  page,
}) => {
  const fixture = host()
  await install(page, fixture)
  const editor = page.getByLabel('Message', { exact: true })
  await editor.fill('输入法候选')
  await editor.dispatchEvent('compositionstart')
  await editor.dispatchEvent('keydown', { key: 'Enter', isComposing: true, keyCode: 229 })
  await editor.dispatchEvent('compositionend')
  await editor.dispatchEvent('keydown', { key: 'Enter' })
  await editor.dispatchEvent('keydown', { key: 'Enter', repeat: true })
  await editor.dispatchEvent('keydown', { key: 'Enter', shiftKey: true })
  expect(fixture.calls.filter((c) => c.method === 'send')).toHaveLength(0)
  await expect(editor).toHaveValue('输入法候选')
  await page.getByRole('button', { name: 'Send', exact: true }).click()
  await expect.poll(() => fixture.calls.filter((c) => c.method === 'send').length).toBe(1)
})
test('L2 backup refusal and old-host capability never start a model call', async ({ page }) => {
  const fixture = host()
  await install(page, fixture)
  fixture.failBackup = true
  const editor = page.getByLabel('Message', { exact: true })
  await editor.fill('Keep on backup refusal')
  await editor.press('Enter')
  await expect.poll(async () => (await state(page)).busy).toBe(false)
  await expect(editor).toHaveValue('Keep on backup refusal')
  expect(fixture.calls.filter((c) => c.method === 'send')).toHaveLength(0)
  fixture.failBackup = false
  fixture.unsupported = true
  await page.evaluate(async () => {
    const path = '/src/stores/chat.ts'
    await (await import(path)).useChatStore().clear()
  })
  await editor.fill('Keep on old host')
  await editor.press('Enter')
  await expect.poll(async () => (await state(page)).busy).toBe(false)
  await expect(editor).toHaveValue('Keep on old host')
  expect(fixture.calls.filter((c) => c.method === 'send')).toHaveLength(0)
})
test('L2 a late old-session stream cannot repaint a newly selected conversation', async ({
  page,
}) => {
  const fixture = host()
  await install(page, fixture)
  await choose(page, 'accept')
  const editor = page.getByLabel('Message', { exact: true })
  await editor.fill('Old session input')
  await editor.press('Enter')
  await expect(editor).toHaveValue('')
  await page.evaluate(async () => {
    const path = '/src/stores/chat.ts'
    const { gateway } = await import('/src/core/index.ts')
    gateway.getChatSession = async (_ws: string, id: string) => ({
      ok: true,
      data: { sessionId: id, transcript: [], history: [], todos: [] },
    })
    await (await import(path)).useChatStore().switchTo(crypto.randomUUID())
  })
  await editor.fill('Draft in another conversation')
  await page.evaluate(() => (window as any).dwRelease())
  await expect(editor).toHaveValue('Draft in another conversation')
  expect((await state(page)).messages.some((m) => m.content === 'Synthetic result')).toBe(false)
  expect((await state(page)).streaming).toBe(false)
})
test('L2 typing before acceptance retains the newer version and its attachment', async ({
  page,
}) => {
  const fixture = host()
  await install(page, fixture)
  await choose(page, 'delayed')
  const editor = page.getByLabel('Message', { exact: true })
  await editor.fill('Original awaiting receipt')
  await editor.press('Enter')
  await expect.poll(() => fixture.calls.filter((c) => c.method === 'send').length).toBe(1)
  await editor.fill('Typed before receipt')
  await page.evaluate(async () => {
    const path = '/src/stores/chat.ts'
    ;(await import(path))
      .useChatStore()
      .queueFile({ kind: 'file', name: 'new.ts', path: 'src/new.ts' })
  })
  await page.evaluate(() => (window as any).dwDeliver())
  await expect.poll(async () => (await state(page)).streaming).toBe(true)
  await expect(editor).toHaveValue('Typed before receipt')
  expect((await state(page)).files.map((f) => f.path)).toEqual(['src/new.ts'])
  await page.evaluate(() => (window as any).dwRelease())
  await expect.poll(async () => (await state(page)).busy).toBe(false)
  await expect(editor).toHaveValue('Typed before receipt')
})
test('L2 confirmed new-chat raw input and repeated prompt references restore after reload without restoring permission', async ({
  page,
}) => {
  const fixture = host()
  await install(page, fixture)
  const editor = page.getByLabel('Message', { exact: true })
  await editor.fill('  restore raw 中文\n  ')
  await page.evaluate(async () => {
    const chatPath = '/src/stores/chat.ts',
      promptPath = '/src/stores/prompt-resources.ts'
    const chat = (await import(chatPath)).useChatStore(),
      prompts = (await import(promptPath)).usePromptResourcesStore()
    chat.permissionMode = 'full'
    const reference = {
      scope: { scope: 'project', owner: 'fixture' },
      id: crypto.randomUUID(),
      revision: 1,
      content_digest: 'fixture',
    }
    prompts.selections = ['first', 'second'].map((id) => ({
      status: 'current',
      snapshot: {
        id,
        reference,
        name: id,
        body: `raw ${id}`,
        selected_by: 'fixture',
        selected_at: 1,
        purpose: { kind: 'chat' },
      },
    }))
  })
  await expect(page.locator('[data-chat-draft-status]')).toContainText('Backed Up on Host')
  const old = await state(page)
  const record = [...fixture.records.values()].find(
    (r) => r.input.id === old.input.id && !r.discarded,
  )
  expect(record?.input.kind).toBe('new_chat')
  expect(record.input.selections.map((s: any) => s.selection.snapshot.body)).toEqual([
    'raw first',
    'raw second',
  ])
  await install(page, fixture, true)
  await expect(editor).toHaveValue('')
  await page.getByRole('button', { name: 'Recovery Copies', exact: true }).click()
  await page.getByRole('button', { name: 'Review Copy', exact: true }).first().click()
  await page.getByRole('button', { name: 'Use This Copy', exact: true }).click()
  await expect(editor).toHaveValue('  restore raw 中文\n  ')
  const restored = await state(page)
  expect(restored.input.selections.map((s: any) => s.selection.snapshot.body)).toEqual([
    'raw first',
    'raw second',
  ])
  expect(
    await page.evaluate(async () => {
      const path = '/src/stores/chat.ts'
      return (await import(path)).useChatStore().permissionMode
    }),
  ).toBe('sandbox')
  expect(fixture.calls.filter((c) => c.method === 'send')).toHaveLength(0)
})

test('L2 reload recovers a pending identity and reconciles it without resending', async ({
  page,
}) => {
  const fixture = host()
  await install(page, fixture)
  await choose(page, 'unknown')
  const editor = page.getByLabel('Message', { exact: true })
  await editor.fill('Pending across reload')
  await editor.press('Enter')
  await expect(page.getByRole('button', { name: 'Check Receipt', exact: true })).toBeVisible()
  const id = (await state(page)).input.pending_submission
  await install(page, fixture, true)
  await page.getByRole('button', { name: 'Recovery Copies', exact: true }).click()
  await page.getByRole('button', { name: 'Review Copy', exact: true }).first().click()
  await page.getByRole('button', { name: 'Use This Copy', exact: true }).click()
  await expect(editor).toHaveValue('Pending across reload')
  expect((await state(page)).input.pending_submission).toBe(id)
  await editor.press('Control+Enter')
  expect(fixture.calls.filter((c) => c.method === 'send')).toHaveLength(1)
  fixture.offline = false
  await page.getByRole('button', { name: 'Check Receipt', exact: true }).click()
  await expect.poll(async () => (await state(page)).input.pending_submission).toBeNull()
  await expect(editor).toHaveValue('Pending across reload')
  expect(fixture.receipts.get(id).phase).toBe('retired')
  expect(fixture.calls.filter((c) => c.method === 'send')).toHaveLength(1)
})
