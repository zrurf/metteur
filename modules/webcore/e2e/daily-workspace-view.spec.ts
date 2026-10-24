import { test, expect, type Page } from '@playwright/test'

const workspace = 'D:/metteur-demo/metrics'
function host() {
  const a = crypto.randomUUID(),
    b = crypto.randomUUID(),
    records = new Map<string, any>(),
    calls: any[] = []
  const flags = {
    missing: '',
    deleteFailed: false,
    delay: '',
    release: null as null | (() => void),
  }
  async function handle(value: any): Promise<any> {
    calls.push(structuredClone(value))
    if (value.method === 'sessions')
      return {
        ok: true,
        data: [b, a]
          .filter((id) => flags.missing !== id)
          .map((id, i) => ({
            sessionId: id,
            title: id === a ? 'Conversation A' : 'Conversation B',
            updatedAt: 2 - i,
            turns: 1,
            messageCount: 1,
          })),
      }
    if (value.method === 'session') {
      if (flags.delay === value.id)
        await new Promise<void>((resolve) => {
          flags.release = resolve
        })
      if (flags.missing === value.id) return { ok: false, error: 'Conversation source deleted' }
      const messages = [
        {
          id: value.id,
          role: 'user',
          content: value.id === a ? 'Original conversation A' : 'Latest conversation B',
          createdAt: 1,
        },
      ]
      return {
        ok: true,
        data: { sessionId: value.id, transcript: messages, history: messages, todos: [] },
      }
    }
    if (value.method === 'delete')
      return flags.deleteFailed
        ? { ok: false, error: 'Synthetic disk failure' }
        : { ok: true, data: undefined }
    const action = value.payload.action,
      id = action.id ?? action.input?.id,
      key = JSON.stringify([value.ws, id]),
      old = records.get(key)
    if (action.operation === 'latest_view')
      return {
        ok: true,
        data: {
          record:
            [...records.values()]
              .filter(
                (row) => row.ws === value.ws && !row.discarded && row.input.kind === 'view_state',
              )
              .at(-1) ?? null,
        },
      }
    if (action.operation === 'read')
      return old
        ? { ok: true, data: { record: old } }
        : { ok: false, error: 'Recovery source missing' }
    if (action.operation === 'list') return { ok: true, data: { items: [], next: null } }
    if ((old?.revision ?? 0) !== value.expected) return { ok: false, error: 'Revision conflict' }
    const next = {
      schema_version: 1,
      ws: value.ws,
      revision: value.expected + 1,
      discarded: action.operation === 'discard',
      input: structuredClone(action.input ?? old.input),
      updated_ms: records.size + 1,
    }
    records.set(key, next)
    return { ok: true, data: next }
  }
  return { a, b, flags, records, calls, handle }
}
async function install(page: Page, fixture: ReturnType<typeof host>) {
  await page.exposeFunction('dwViewTransport', fixture.handle)
  await page.addInitScript(() => {
    ;(window as any).dwViewReady = import('/src/core/index.ts').then(({ gateway }) => {
      gateway.connected.value = true
      const original = gateway.resourceRequest.bind(gateway),
        read = gateway.readFile.bind(gateway)
      gateway.resourceRequest = (method: string, ws: string, payload: unknown, expected = 0) =>
        method === 'recovery'
          ? (window as any).dwViewTransport({ method, ws, payload, expected })
          : original(method, ws, payload, expected)
      gateway.listChatSessions = () => (window as any).dwViewTransport({ method: 'sessions' })
      gateway.getChatSession = (_ws: string, id: string) =>
        (window as any).dwViewTransport({ method: 'session', id })
      gateway.deleteChatSession = (_ws: string, id: string) =>
        (window as any).dwViewTransport({ method: 'delete', id })
      gateway.readFile = (ws: string, path: string) =>
        path === 'metrics/README.md'
          ? Promise.resolve({
              ok: true,
              data: {
                content:
                  '# Metrics\n' +
                  Array.from({ length: 400 }, (_, i) => `line ${i} unchanged saved content`).join(
                    '\n',
                  ),
                language: 'markdown',
              },
            })
          : path.endsWith('.blueprint')
            ? Promise.resolve({ ok: true, data: { content: '' } })
            : read(ws, path)
    })
  })
}
async function open(page: Page) {
  await page.goto('/')
  await page.evaluate(() => (window as any).dwViewReady)
  await page.getByLabel('Workspace path').fill(workspace)
  await page.getByRole('button', { name: 'Open', exact: true }).click()
  await expect(page.getByTestId('file-tree')).toBeVisible()
}
async function file(page: Page, path: string) {
  await page.evaluate(async (value) => {
    const r = await import('/src/router.ts'),
      f = await import('/src/lib/file-token.ts')
    await r.router.push(f.fileRoute(value))
  }, path)
}
async function flush(page: Page) {
  return page.evaluate(async () => {
    await new Promise(requestAnimationFrame)
    const views = (await import('/src/stores/workspace-view.ts')).useWorkspaceViewStore(),
      ws = (await import('/src/stores/workspace.ts')).useWorkspaceStore().active!.path
    return views.forWorkspace(ws).flush()
  })
}
async function chat(page: Page) {
  await page.getByRole('button', { name: 'Chat', exact: true }).first().click()
  await expect(page.getByLabel('Message', { exact: true })).toBeVisible()
  await expect(page.getByRole('button', { name: /^Conversations/ })).toBeEnabled()
}
test('L2 host-confirmed tab order, split and text selection restore; a file deep link keeps priority', async ({
  page,
}) => {
  const fixture = host()
  await install(page, fixture)
  await open(page)
  await file(page, 'metrics/README.md')
  await expect(page.locator('.monaco-editor .view-lines')).toContainText('# Metrics')
  await file(page, 'metrics/src/api.ts')
  await expect(page.locator('.monaco-editor .view-lines')).toContainText('fetchMetrics')
  await chat(page)
  await file(page, 'metrics/README.md')
  await page.evaluate(async () => {
    const tabs = (await import('/src/stores/tabs.ts')).useTabsStore()
    tabs.openInSplit('metrics/src/api.ts')
    tabs.setSplitRatio(0.63)
    const monaco = (await import('/src/lib/monaco.ts')).default
    const editor = monaco.editor
      .getEditors()
      .find((item) => item.getModel()?.getValue().startsWith('# Metrics'))!
    editor.setSelection(new monaco.Selection(20, 3, 22, 8))
    editor.setScrollTop(240)
  })
  expect(await flush(page)).toBe(true)
  const before = await page.evaluate(async () => {
    const monaco = (await import('/src/lib/monaco.ts')).default,
      editor = monaco.editor
        .getEditors()
        .find((item) => item.getModel()?.getValue().startsWith('# Metrics'))!
    return { selection: editor.getSelection()!.toString(), top: editor.getScrollTop() }
  })
  await page.reload()
  await page.evaluate(() => (window as any).dwViewReady)
  await expect(page.locator('.monaco-editor')).toHaveCount(2)
  await expect(page.getByTestId('tab')).toHaveCount(2)
  await expect(page.getByTestId('tab-active')).toContainText('README.md')
  await expect
    .poll(() =>
      page.evaluate(async () => {
        const monaco = (await import('/src/lib/monaco.ts')).default,
          editor = monaco.editor
            .getEditors()
            .find((item) => item.getModel()?.getValue().startsWith('# Metrics'))
        return (
          editor && { selection: editor.getSelection()!.toString(), top: editor.getScrollTop() }
        )
      }),
    )
    .toEqual(before)
  const state = await page.evaluate(async () => {
    const tabs = (await import('/src/stores/tabs.ts')).useTabsStore()
    return { paths: tabs.items.map((item) => item.path || item.id), ratio: tabs.splitRatio }
  })
  expect(state).toEqual({ paths: ['metrics/README.md', 'metrics/src/api.ts', 'chat'], ratio: 0.63 })
  expect(
    [...fixture.records.values()].every(
      (row) =>
        row.input.kind !== 'view_state' ||
        (row.input.raw === '' && !row.input.selected_model && !row.input.pending_submission),
    ),
  ).toBe(true)
})
test('L2 selected older conversation reopens, deleted source remains unavailable and delete failures keep it', async ({
  page,
}) => {
  const fixture = host()
  await install(page, fixture)
  await open(page)
  await chat(page)
  await page.getByRole('button', { name: /^Conversations/ }).click()
  await page
    .getByRole('button', { name: /Conversation A/ })
    .first()
    .click()
  await expect(page.locator('.chat-thread-inner')).toContainText('Original conversation A')
  expect(await flush(page)).toBe(true)
  await page.reload()
  await page.evaluate(() => (window as any).dwViewReady)
  await expect(page.locator('.chat-thread-inner')).toContainText('Original conversation A')
  fixture.flags.deleteFailed = true
  await page.getByRole('button', { name: /^Conversations/ }).click()
  await page.getByRole('button', { name: 'Delete Conversation A', exact: true }).click()
  await page
    .getByRole('dialog')
    .getByRole('button', { name: 'Delete Conversation', exact: true })
    .click()
  await expect(page.getByText('Conversation Kept', { exact: true })).toBeVisible()
  await expect(page.locator('.chat-thread-inner')).toContainText('Original conversation A')
  fixture.flags.missing = fixture.a
  await page.reload()
  await page.evaluate(() => (window as any).dwViewReady)
  await expect(page.getByTestId('tab-active')).toContainText('Chat')
  await expect(page.locator('[data-chat-availability]')).toContainText('Conversation unavailable')
  await expect(page.locator('.chat-thread-inner')).not.toContainText('Latest conversation B')
  const result = await page.evaluate(async () => {
    const store = (await import('/src/stores/chat.ts')).useChatStore()
    store.draft = 'kept orphan draft'
    return { session: store.sessionId, sent: await store.send('Do not run') }
  })
  expect(result).toEqual({ session: fixture.a, sent: false })
})
test('L2 slow conversation reply cannot replace a newer explicit deep link', async ({ page }) => {
  const fixture = host()
  await install(page, fixture)
  await open(page)
  await chat(page)
  fixture.flags.delay = fixture.a
  await page.evaluate(async (id) => {
    void (await import('/src/stores/chat.ts')).useChatStore().switchTo(id)
  }, fixture.a)
  await expect.poll(() => !!fixture.flags.release).toBe(true)
  await page.evaluate(async (id) => {
    await (await import('/src/stores/chat.ts')).useChatStore().switchTo(id)
  }, fixture.b)
  fixture.flags.release!()
  await expect(page.locator('.chat-thread-inner')).toContainText('Latest conversation B')
  const session = await page.evaluate(
    async () => (await import('/src/stores/chat.ts')).useChatStore().sessionId,
  )
  expect(session).toBe(fixture.b)
})
test('L2 saved view unavailable keeps the selected source until an explicit user choice', async ({
  page,
}) => {
  const fixture = host()
  await install(page, fixture)
  await open(page)
  await chat(page)
  await page.getByRole('button', { name: /^Conversations/ }).click()
  await page
    .getByRole('button', { name: /Conversation A/ })
    .first()
    .click()
  await expect(page.locator('.chat-thread-inner')).toContainText('Original conversation A')
  const selected = await page.evaluate(async () => {
    const { gateway } = await import('/src/core/index.ts')
    Object.defineProperty(gateway, 'demo', { value: false, configurable: true })
    const views = (await import('/src/stores/workspace-view.ts')).useWorkspaceViewStore()
    views.forWorkspace('D:/metteur-demo/metrics').error = 'Saved view source is unavailable'
    const store = (await import('/src/stores/chat.ts')).useChatStore()
    await store.restore()
    return {
      session: store.sessionId,
      unavailable: store.sessionUnavailable,
      sent: await store.send('Do not send'),
    }
  })
  expect(selected).toEqual({ session: fixture.a, unavailable: true, sent: false })
  await expect(page.locator('.chat-thread-inner')).not.toContainText('Latest conversation B')
})
test('L2 canvas viewport and metadata copies stay independent across two windows', async ({
  page,
  context,
}) => {
  const fixture = host()
  await install(page, fixture)
  await open(page)
  await file(page, 'first.blueprint')
  await expect(page.locator('.vue-flow__node')).toHaveCount(2)
  await page.evaluate(async () => {
    const view = (await import('/src/stores/workspace-view.ts')).useWorkspaceViewStore()
    view.position('D:/metteur-demo/metrics', 'first.blueprint', {
      kind: 'canvas',
      x: 130,
      y: -40,
      zoom: 1.2,
    })
  })
  expect(await flush(page)).toBe(true)
  await page.reload()
  await page.evaluate(() => (window as any).dwViewReady)
  await expect(page.locator('.vue-flow__node')).toHaveCount(2)
  await expect
    .poll(() => page.locator('.vue-flow__transformationpane').getAttribute('style'))
    .toContain('scale(1.2)')
  const other = await context.newPage()
  await install(other, fixture)
  await open(other)
  await chat(other)
  expect(await flush(other)).toBe(true)
  const windows = new Set(
    [...fixture.records.values()]
      .filter((row) => row.input.kind === 'view_state')
      .map((row) => row.input.window_id),
  )
  expect(windows.size).toBeGreaterThanOrEqual(2)
  await expect(page.locator('.vue-flow__node')).toHaveCount(2)
  await other.close()
})
