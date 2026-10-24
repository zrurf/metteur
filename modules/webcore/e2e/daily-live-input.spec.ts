import { test, expect, type Page } from '@playwright/test'
test.afterEach(async ({ page }, info) => {
  if (info.status === info.expectedStatus) return
  const state = await page.evaluate(async () => {
    const path = '/src/stores/chat.ts',
      chat = (await import(path)).useChatStore(),
      fixture = (window as any).liveInput
    return {
      draft: chat.draft,
      session: chat.sessionId,
      turn: chat.activeTurn,
      error: chat.sendError,
      state: chat.composerInput?.state,
      input: chat.composerInput?.input,
      calls: fixture?.calls,
    }
  })
  await info.attach('input-state', {
    body: JSON.stringify(state, null, 2),
    contentType: 'application/json',
  })
})

// L2 uses actual Composer, draft controller and store with a synthetic host.
// The native dw_delivery suite verifies WAL, root context and turn closure.
async function setup(page: Page) {
  await page.goto('/')
  await page.getByLabel('Workspace path').fill('D:/metteur-demo/live-input')
  await page.getByRole('button', { name: 'Open', exact: true }).click()
  const editor = page.getByLabel('Message', { exact: true })
  await expect(editor).toHaveAttribute('placeholder', /Describe a task/)
  await page.evaluate(async () => {
    const core = '/src/core/index.ts',
      chatPath = '/src/stores/chat.ts',
      forePath = '/src/stores/foreground.ts'
    const { gateway } = await import(core),
      chat = (await import(chatPath)).useChatStore()
    const fixture = ((window as any).liveInput = {
      calls: [],
      records: new Map(),
      receipts: new Map(),
      hold: false,
      running: false,
      offline: false,
      phase: 'live_queued',
      session: crypto.randomUUID(),
      turn: crypto.randomUUID(),
    })
    Object.defineProperty(gateway, 'demo', { value: false, configurable: true })
    gateway.connected.value = true
    const original = gateway.resourceRequest.bind(gateway)
    gateway.resourceRequest = async (method: string, ws: string, payload: any, expected = 0) => {
      fixture.calls.push({ method, payload: structuredClone(payload) })
      if (method === 'chatDelivery') {
        if (payload.operation === 'admission')
          return {
            ok: true,
            data: {
              state: fixture.running ? 'chat' : 'idle',
              session: fixture.session,
              turn: fixture.turn,
              run: null,
              reason: 'Chat is running',
            },
          }
        if (payload.operation === 'capabilities')
          return {
            ok: true,
            data: {
              durable_manual_send: true,
              pending_reconciliation: true,
              durable_queue: true,
              durable_queue_channel: 'chat',
            },
          }
        if (payload.operation === 'reconcile_pending') return { ok: true, data: { items: [] } }
        if (payload.operation === 'session_revision')
          return { ok: true, data: { session: payload.session, revision: 1 } }
        if (payload.operation === 'live_submit') {
          if (fixture.hold) await new Promise<void>((resolve) => (fixture.release = resolve))
          if (fixture.offline) return { ok: false, error: 'Synthetic lost acknowledgement' }
          const receipt = {
            schema_version: 1,
            id: payload.id,
            session: payload.session,
            turn: payload.turn,
            accepted_revision: 0,
            phase: fixture.phase,
            live: { mode: payload.mode, body_digest: 'a'.repeat(64) },
            accepted_ms: 1,
            updated_ms: 1,
          }
          fixture.receipts.set(payload.id, receipt)
          return { ok: true, data: receipt }
        }
        if (fixture.offline) return { ok: false, error: 'Synthetic disconnected query' }
        if (payload.operation === 'live_receipts')
          return {
            ok: true,
            data: {
              items: payload.ids.map((id: string) => ({
                receipt: fixture.receipts.get(id),
                source_available: true,
              })),
            },
          }
        return {
          ok: true,
          data: { receipt: fixture.receipts.get(payload.id) ?? null, source_available: true },
        }
      }
      if (method === 'recovery') {
        const action = payload.action,
          id = action.id ?? action.input?.id,
          current = fixture.records.get(id)
        if (action.operation === 'latest_view') return { ok: true, data: { record: null } }
        if (action.operation === 'list') return { ok: true, data: { items: [], next: null } }
        if (action.operation === 'read') return { ok: true, data: { record: current } }
        const record =
          action.operation === 'discard'
            ? { ...current, discarded: true, revision: expected + 1 }
            : {
                schema_version: 1,
                input: structuredClone(action.input),
                revision: expected + 1,
                discarded: false,
                updated_ms: 1,
              }
        fixture.records.set(id, record)
        return { ok: true, data: record }
      }
      return original(method, ws, payload, expected)
    }
    gateway.sendInterrupt = async () => {
      throw Error('Reliable input must not downgrade')
    }
    gateway.sendChat = async (
      _ws: any,
      _body: any,
      _history: any,
      onMessage: any,
      options: any,
      _sessionCallback: any,
      _session: any,
      _usage: any,
      _todos: any,
      _signal: any,
      _progress: any,
      _context: any,
      _approval: any,
      onDelivery: any,
    ) => {
      fixture.onMessage = onMessage
      fixture.running = true
      const receipt = {
        schema_version: 1,
        id: options.delivery.id,
        input_digest: 'fixture',
        session: fixture.session,
        turn: fixture.turn,
        accepted_revision: 2,
        checkpoint: 'fixture',
        phase: 'started',
        accepted_ms: 1,
        updated_ms: 1,
      }
      fixture.receipts.set(receipt.id, receipt)
      onDelivery(receipt)
      return new Promise(
        (resolve) => (fixture.finish = () => resolve({ ok: true, data: undefined })),
      )
    }
    chat.sessionId = fixture.session
    chat.messages = []
    await (await import(forePath)).useForegroundStore().refresh()
    void chat.send('Opening input', { model: 'demo-chat' })
  })
  await expect(editor).toHaveAttribute('placeholder', /Queue a message/)
  return editor
}

test('L2 queue ACK clears only its input, retains newer text and references, activity does not imply use', async ({
  page,
}) => {
  const editor = await setup(page)
  await page.evaluate(async () => {
    const path = '/src/stores/chat.ts',
      chat = (await import(path)).useChatStore()
    chat.pendingFiles = [{ kind: 'file', name: 'old.ts', path: 'old.ts' }]
    ;(window as any).liveInput.hold = true
  })
  await editor.fill('  queued text  ')
  await editor.press('Tab')
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as any).liveInput.calls.filter(
            (call: any) => call.payload.operation === 'live_submit',
          ).length,
      ),
    )
    .toBe(1)
  await editor.fill('newer input')
  await page.evaluate(async () => {
    const path = '/src/stores/chat.ts',
      chat = (await import(path)).useChatStore()
    chat.pendingFiles = [{ kind: 'file', name: 'new.ts', path: 'new.ts' }]
    ;(window as any).liveInput.release()
  })
  await expect(page.getByRole('log', { name: 'Conversation' })).toContainText('Queued')
  await expect(editor).toHaveValue('newer input')
  const result = await page.evaluate(async () => {
    const path = '/src/stores/chat.ts',
      chat = (await import(path)).useChatStore(),
      fixture = (window as any).liveInput
    fixture.onMessage({
      id: 'unrelated',
      role: 'assistant',
      content: 'Other activity',
      createdAt: 1,
    })
    return {
      files: chat.pendingFiles.map((file: any) => file.path),
      payload: fixture.calls.find((call: any) => call.payload.operation === 'live_submit').payload,
      copies: [...fixture.records.values()].map((r: any) => r.input.raw),
    }
  })
  expect(result.files).toEqual(['new.ts'])
  expect(result.payload.message).toBe('@file old.ts\n  queued text  ')
  expect(result.copies).toContain('  queued text  ')
  await expect(page.getByRole('log', { name: 'Conversation' })).toContainText('Queued')
})
test('L2 Steer carries attachments and shows unconsumed input without extra dense text', async ({
  page,
}) => {
  const editor = await setup(page)
  await page.evaluate(async () => {
    const path = '/src/stores/chat.ts',
      chat = (await import(path)).useChatStore()
    chat.pendingFiles = [{ kind: 'file', name: 'selected.ts', path: 'selected.ts' }]
    ;(window as any).liveInput.phase = 'live_unconsumed'
  })
  await editor.fill('steer input')
  await editor.press('Control+Enter')
  await expect(editor).toHaveValue('')
  await expect(page.getByRole('log', { name: 'Conversation' })).toContainText(
    'Not used · input kept',
  )
  const result = await page.evaluate(async () => {
    const path = '/src/stores/chat.ts',
      chat = (await import(path)).useChatStore()
    return {
      files: chat.pendingFiles,
      submit: (window as any).liveInput.calls.find(
        (call: any) => call.payload.operation === 'live_submit',
      ).payload,
    }
  })
  expect(result.files).toEqual([])
  expect(result.submit.mode).toBe('steer')
  expect(result.submit.message).toBe('@file selected.ts\nsteer input')
})
test('L2 an unknown queue outcome keeps raw text and blocks repeat submission under another identity', async ({
  page,
}) => {
  const editor = await setup(page)
  await page.evaluate(() => ((window as any).liveInput.offline = true))
  await editor.fill('keep this input')
  await editor.press('Enter')
  await expect(editor).toHaveValue('keep this input')
  await expect
    .poll(() =>
      page.evaluate(async () => {
        const path = '/src/stores/chat.ts'
        return (await import(path)).useChatStore().composerInput?.state
      }),
    )
    .toBe('unknown')
  await editor.press('Enter')
  expect(
    await page.evaluate(
      () =>
        (window as any).liveInput.calls.filter(
          (call: any) => call.payload.operation === 'live_submit',
        ).length,
    ),
  ).toBe(1)
})
