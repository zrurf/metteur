import { test, expect, type Page } from '@playwright/test'

// L2 UI identity/admission fixtures; native dw_delivery covers the real slot.
async function setup(page: Page) {
  await page.goto('/')
  await page.getByLabel('Workspace path').fill('D:/metteur-demo/foreground')
  await page.getByRole('button', { name: 'Open', exact: true }).click()
  await expect(page.getByLabel('Message', { exact: true })).toBeVisible()
  await expect(page.getByLabel('Message', { exact: true })).toHaveAttribute(
    'placeholder',
    /Describe a task/,
  )
  await page.evaluate(async () => {
    const core = '/src/core/index.ts',
      chatPath = '/src/stores/chat.ts',
      forePath = '/src/stores/foreground.ts',
      execPath = '/src/stores/execution.ts'
    const { gateway } = await import(core),
      chat = (await import(chatPath)).useChatStore()
    const fixture = ((window as any).dwForeground = {
      state: 'blueprint',
      run: '00000000-0000-4000-8000-000000000001',
      session: null,
      turn: null,
      reason: 'Blueprint is running',
    })
    ;(window as any).dwControls = []
    ;(window as any).dwInputCalls = 0
    const request = gateway.resourceRequest.bind(gateway)
    gateway.resourceRequest = (method: string, ws: string, payload: any, ...rest: any[]) =>
      method === 'chatDelivery' && payload.operation === 'admission'
        ? Promise.resolve({ ok: true, data: { ...fixture } })
        : request(method, ws, payload, ...rest)
    gateway.listExecutions = async () => ({
      ok: true,
      data:
        fixture.state === 'blueprint'
          ? [
              {
                runId: fixture.run,
                blueprintId: 'fixture-blueprint',
                status: 'Running',
                startedAt: 1,
                updatedAt: Date.now(),
                executedNodes: 0,
                snapshot: {
                  runtime: {
                    active: true,
                    pause_requested: fixture.reason.includes('paused'),
                    cancel_requested: false,
                    pending_approval_ids: [],
                    pending_approvals: [],
                  },
                  view: {
                    root: { id: 'fixture-blueprint', name: 'Fixture run', nodes: [], edges: [] },
                    invocations: [],
                  },
                },
              },
            ]
          : [],
    })
    gateway.pause = async (ws: string, run: string) => {
      ;(window as any).dwControls.push({ action: 'pause', ws, run })
      return { ok: true, data: undefined }
    }
    gateway.cancel = async (ws: string, run: string) => {
      ;(window as any).dwControls.push({ action: 'cancel', ws, run })
      return { ok: true, data: undefined }
    }
    gateway.sendChat = async () => {
      ;(window as any).dwInputCalls++
      return { ok: false, error: 'Unexpected Send' }
    }
    gateway.sendInterrupt = async () => {
      ;(window as any).dwInputCalls++
      return { ok: false, error: 'Unexpected Interrupt' }
    }
    chat.sessionId = 'session-a'
    chat.messages = [
      { id: 'history-a', role: 'user', content: 'Original conversation A', createdAt: 1 },
    ]
    await (await import(forePath)).useForegroundStore().refresh()
    await (await import(execPath)).useExecutionStore().reconcile()
  })
}
async function setState(page: Page, nextState: string, nextReason: string) {
  await page.evaluate(
    async ({ state, reason }) => {
      Object.assign((window as any).dwForeground, { state, reason })
      const path = '/src/stores/foreground.ts',
        execPath = '/src/stores/execution.ts'
      if (state === 'idle') {
        const execution = (await import(execPath)).useExecutionStore()
        execution.current = null
        execution.status = 'finished'
      }
      await (await import(path)).useForegroundStore().refresh()
    },
    { state: nextState, reason: nextReason },
  )
}
test('L2 blueprint keeps the original conversation and editable draft; all composer delivery paths stay blocked', async ({
  page,
}) => {
  await setup(page)
  const editor = page.getByLabel('Message', { exact: true })
  await expect(page.getByRole('log', { name: 'Conversation' })).toContainText(
    'Original conversation A',
  )
  await expect(page.locator('.chat-surface .concierge-panel')).toHaveCount(0)
  await editor.fill('Draft while blueprint runs')
  await expect(editor).toBeEditable()
  await expect(page.getByRole('button', { name: 'Send', exact: true })).toBeDisabled()
  await editor.press('Enter')
  await editor.press('Control+Enter')
  await editor.press('Tab')
  await page.getByRole('button', { name: 'Delivery options', exact: true }).click()
  await expect(page.getByRole('button', { name: /Send now/ })).toBeDisabled()
  await expect(page.getByRole('button', { name: /Queue after/ })).toBeDisabled()
  expect(await page.evaluate(() => (window as any).dwInputCalls)).toBe(0)
  await expect(editor).toHaveValue('Draft while blueprint runs')
  await setState(page, 'blueprint', 'Blueprint is paused')
  await expect(page.locator('[data-chat-availability]')).toContainText('Blueprint is paused')
  await expect(page.getByRole('button', { name: 'Send', exact: true })).toBeDisabled()
})
test('L2 direct controls keep the concrete run and remain available without a model', async ({
  page,
}) => {
  await setup(page)
  await page.evaluate(async () => {
    const path = '/src/stores/config.ts',
      config = (await import(path)).useConfigStore()
    config.user = { llm: { models: {}, default_model: '' } }
    config.ws = { llm: { models: {}, default_model: '' } }
    config.defaults = {}
  })
  const banner = page.locator('[data-chat-availability]')
  await banner.getByRole('button', { name: 'Pause', exact: true }).click()
  await banner.getByRole('button', { name: 'Stop', exact: true }).click()
  expect(await page.evaluate(() => (window as any).dwControls)).toEqual([
    {
      action: 'pause',
      ws: 'D:/metteur-demo/foreground',
      run: '00000000-0000-4000-8000-000000000001',
    },
    {
      action: 'cancel',
      ws: 'D:/metteur-demo/foreground',
      run: '00000000-0000-4000-8000-000000000001',
    },
  ])
  await page.getByLabel('Message', { exact: true }).fill('No model still allows drafting')
  await expect(page.getByLabel('Message', { exact: true })).toBeEditable()
})

test('L2 Stop stays available while a prior pause request has no response', async ({ page }) => {
  await setup(page)
  await page.evaluate(async () => {
    const core = '/src/core/index.ts',
      { gateway } = await import(core)
    gateway.pause = (ws: string, run: string) => {
      ;(window as any).dwControls.push({ action: 'pause', ws, run })
      return new Promise(() => {})
    }
  })
  const banner = page.locator('[data-chat-availability]')
  await banner.getByRole('button', { name: 'Pause', exact: true }).click()
  await expect(banner.getByRole('button', { name: 'Stop', exact: true })).toBeEnabled()
  await banner.getByRole('button', { name: 'Stop', exact: true }).click()
  await expect
    .poll(() => page.evaluate(() => (window as any).dwControls.map((item: any) => item.action)))
    .toEqual(['pause', 'cancel'])
})
test('L2 supervision opens only after an explicit action and completion never navigates or sends the draft', async ({
  page,
}) => {
  await setup(page)
  const initial = page.url(),
    editor = page.getByLabel('Message', { exact: true })
  await editor.fill('Keep in main chat')
  await setState(page, 'idle', '')
  await expect(page).toHaveURL(initial)
  await expect(editor).toHaveValue('Keep in main chat')
  expect(await page.evaluate(() => (window as any).dwInputCalls)).toBe(0)
  await setState(page, 'blueprint', 'Blueprint is running')
  await page.getByRole('button', { name: 'Open Supervision', exact: true }).click()
  await expect(page).toHaveURL(/\/execution$/)
  await expect(page.locator('.supervisor-heading')).toContainText('Supervision')
  expect(
    await page.evaluate(async () => {
      const path = '/src/stores/chat.ts',
        chat = (await import(path)).useChatStore()
      return { session: chat.sessionId, draft: chat.draft }
    }),
  ).toEqual({ session: 'session-a', draft: 'Keep in main chat' })
})
test('L2 unknown ownership blocks send without clearing input and recovers on an explicit state check', async ({
  page,
}) => {
  await setup(page)
  await page.getByLabel('Message', { exact: true }).fill('Keep through unknown')
  await page.evaluate(async () => {
    const core = '/src/core/index.ts',
      path = '/src/stores/foreground.ts',
      exec = '/src/stores/execution.ts'
    const { gateway } = await import(core),
      execution = (await import(exec)).useExecutionStore()
    execution.current = null
    execution.status = 'unknown'
    const previous = gateway.resourceRequest.bind(gateway)
    ;(window as any).dwRestoreAdmission = () => {
      gateway.resourceRequest = previous
    }
    gateway.resourceRequest = (method: string, ws: string, payload: any, ...rest: any[]) => {
      if (method === 'chatDelivery' && payload.operation === 'admission')
        throw new Error('Synthetic synchronous disconnect')
      return previous(method, ws, payload, ...rest)
    }
    await (await import(path)).useForegroundStore().refresh()
  })
  await expect(page.getByRole('button', { name: 'Send', exact: true })).toBeDisabled()
  await page.getByLabel('Message', { exact: true }).press('Control+Enter')
  await expect(page.getByLabel('Message', { exact: true })).toHaveValue('Keep through unknown')
  expect(await page.evaluate(() => (window as any).dwInputCalls)).toBe(0)
  await page.evaluate(() => {
    ;(window as any).dwRestoreAdmission()
    Object.assign((window as any).dwForeground, { state: 'idle', reason: '' })
  })
  await page.getByRole('button', { name: 'Check State', exact: true }).click()
  await expect(page.getByRole('button', { name: 'Send', exact: true })).toBeEnabled()
  await expect(page.getByLabel('Message', { exact: true })).toHaveValue('Keep through unknown')
})

test('L2 A/B conversation drafts and attachments remain separate while blueprint completion leaves B selected', async ({
  page,
}, testInfo) => {
  await setup(page)
  await page.evaluate(async () => {
    const core = '/src/core/index.ts',
      chatPath = '/src/stores/chat.ts',
      { gateway } = await import(core)
    gateway.getChatSession = async (_ws: string, id: string) => ({
      ok: true,
      data: {
        sessionId: id,
        createdAt: 1,
        history: [],
        todos: [],
        transcript: [
          { id: `history-${id}`, role: 'user', content: `Original ${id}`, createdAt: 1 },
        ],
      },
    })
    ;(await import(chatPath))
      .useChatStore()
      .queueFile({ kind: 'file', path: 'src/a.ts', name: 'a.ts' })
  })
  const editor = page.getByLabel('Message', { exact: true })
  await editor.fill('Draft A')
  await page.evaluate(async () => {
    const path = '/src/stores/chat.ts'
    await (await import(path)).useChatStore().switchTo('session-b')
  })
  await expect(editor).toHaveValue('')
  await editor.fill('Draft B')
  await page.screenshot({ path: testInfo.outputPath('main-chat-blueprint.png') })
  await setState(page, 'idle', '')
  await expect(editor).toHaveValue('Draft B')
  expect(
    await page.evaluate(async () => {
      const path = '/src/stores/chat.ts'
      return (await import(path)).useChatStore().sessionId
    }),
  ).toBe('session-b')
  await page.evaluate(async () => {
    const path = '/src/stores/chat.ts'
    await (await import(path)).useChatStore().switchTo('session-a')
  })
  await expect(editor).toHaveValue('Draft A')
  expect(
    await page.evaluate(async () => {
      const path = '/src/stores/chat.ts'
      return (await import(path)).useChatStore().pendingFiles.map((f) => f.path)
    }),
  ).toEqual(['src/a.ts'])
  await expect(page.getByRole('log', { name: 'Conversation' })).toContainText('Original session-a')
  expect(await page.evaluate(() => (window as any).dwInputCalls)).toBe(0)
})
