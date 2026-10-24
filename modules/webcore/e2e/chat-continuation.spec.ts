import { expect, test, type Page } from '@playwright/test'

async function setup(page: Page, state = 'completed') {
  await page.goto('/')
  await page.evaluate(async (taskState) => {
    const corePath = '/src/core/index.ts',
      workspacePath = '/src/stores/workspace.ts',
      chatPath = '/src/stores/chat.ts'
    const { gateway } = await import(corePath)
    const { createPinia } = await import('/node_modules/.vite/deps/pinia.js')
    const pinia = createPinia()
    gateway.connected.value = true
    gateway.listChatSessions = async () => ({ ok: true, data: [] })
    const workspace = (await import(workspacePath)).useWorkspaceStore(pinia)
    workspace.active = { path: '/follow/a', locked: true }
    const chat = (await import(chatPath)).useChatStore(pinia)
    const fixture = (gateway.followFixture = { chat, reads: 0, sends: 0, methods: [] as string[] })
    const originalRequest = gateway.resourceRequest.bind(gateway)
    gateway.resourceRequest = async (method: string, _path: string, payload: { id: string }) => {
      if (method !== 'queryContinuations') return originalRequest(method, _path, payload)
      fixture.reads++
      fixture.methods.push(method)
      return {
        ok: true,
        data: { task: { id: payload.id, session: 'session-a', state: taskState } },
      }
    }
    gateway.getChatSession = async () => ({
      ok: true,
      data: {
        sessionId: 'session-a',
        createdAt: 1,
        history: [],
        transcript: [
          { id: 'answer', role: 'assistant', content: 'host continuation result', createdAt: 2 },
        ],
        todos: [],
      },
    })
    gateway.sendChat = async (
      _path: string,
      _text: string,
      _history: unknown,
      onMessage: (value: unknown) => void,
      _options: unknown,
      onSession: (id: string) => void,
    ) => {
      fixture.sends++
      onSession('session-a')
      onMessage({
        id: 'waiting',
        role: 'notice',
        content: 'Arbitrary display wording',
        createdAt: 1,
        detail: { continuation_task: 'owned-task' },
      })
      return { ok: true, data: undefined }
    }
  }, state)
  await expect
    .poll(() =>
      page.evaluate(
        async () => !(await import('/src/core/index.ts')).gateway.followFixture.chat.busy,
      ),
    )
    .toBe(true)
  await page.evaluate(async () => {
    const chat = (await import('/src/core/index.ts')).gateway.followFixture.chat
    await chat.send('wait for the owned result')
    chat.pendingFiles.push({ path: '/follow/a/new-draft.txt', name: 'new-draft.txt', kind: 'file' })
  })
}

test('a yielded chat receives the saved host result without a manual reload or new model turn', async ({
  page,
}) => {
  await setup(page)
  await expect
    .poll(() =>
      page.evaluate(async () =>
        (await import('/src/core/index.ts')).gateway.followFixture.chat.messages.some(
          (message) => message.content === 'host continuation result',
        ),
      ),
    )
    .toBe(true)
  const outcome = await page.evaluate(async () => {
    const f = (await import('/src/core/index.ts')).gateway.followFixture
    const result = {
      sends: f.sends,
      reads: f.reads,
      files: f.chat.pendingFiles.map((file) => file.name),
      notices: f.chat.messages.filter((message) => message.role === 'notice').length,
    }
    f.chat.$dispose()
    return result
  })
  expect(outcome).toEqual({ sends: 1, reads: 1, files: ['new-draft.txt'], notices: 0 })
})

test('stopped state ends observation without replaying the model or controlling the task', async ({
  page,
}) => {
  await setup(page, 'stopped')
  await expect
    .poll(() =>
      page.evaluate(async () =>
        (await import('/src/core/index.ts')).gateway.followFixture.chat.messages.some((message) =>
          message.content.includes('No automatic replay'),
        ),
      ),
    )
    .toBe(true)
  await page.waitForTimeout(2200)
  const outcome = await page.evaluate(async () => {
    const f = (await import('/src/core/index.ts')).gateway.followFixture
    const result = { sends: f.sends, reads: f.reads, methods: f.methods }
    f.chat.$dispose()
    return result
  })
  expect(outcome).toEqual({ sends: 1, reads: 1, methods: ['queryContinuations'] })
})

test('a late snapshot cannot overwrite a newer visit to the same conversation', async ({
  page,
}) => {
  await page.goto('/')
  await page.evaluate(async () => {
    const corePath = '/src/core/index.ts',
      workspacePath = '/src/stores/workspace.ts',
      chatPath = '/src/stores/chat.ts'
    const { gateway } = await import(corePath)
    const { createPinia } = await import('/node_modules/.vite/deps/pinia.js')
    const pinia = createPinia()
    gateway.connected.value = true
    gateway.listChatSessions = async () => ({ ok: true, data: [] })
    const workspace = (await import(workspacePath)).useWorkspaceStore(pinia)
    workspace.active = { path: '/follow/a', locked: true }
    const chat = (await import(chatPath)).useChatStore(pinia)
    const fixture = (gateway.followRace = { chat, calls: 0, finish: () => {} })
    const snapshot = (session: string, body: string, task: string) => ({
      sessionId: session,
      createdAt: 1,
      history: [],
      transcript: [
        { id: body, role: 'assistant', content: body, createdAt: 1 },
        {
          id: 'marker',
          role: 'notice',
          content: 'Yielded',
          createdAt: 2,
          detail: { continuation_task: task },
        },
      ],
    })
    gateway.getChatSession = async (_path: string, session: string) => {
      fixture.calls++
      if (fixture.calls === 2)
        return new Promise((resolve) => {
          fixture.finish = () =>
            resolve({ ok: true, data: snapshot('a', 'obsolete snapshot', 'old-task') })
        })
      return {
        ok: true,
        data: snapshot(
          session,
          fixture.calls === 1 ? 'original visit' : 'new visit',
          fixture.calls === 1 ? 'old-task' : 'new-task',
        ),
      }
    }
    gateway.resourceRequest = async (_method: string, _path: string, payload: { id: string }) => ({
      ok: true,
      data: { task: { id: payload.id, session: 'a', state: 'completed' } },
    })
    await chat.switchTo('a')
  })
  await expect
    .poll(() =>
      page.evaluate(async () => (await import('/src/core/index.ts')).gateway.followRace.calls),
    )
    .toBe(2)
  const messages = await page.evaluate(async () => {
    const f = (await import('/src/core/index.ts')).gateway.followRace
    await f.chat.switchTo('b')
    await f.chat.switchTo('a')
    f.finish()
    await new Promise((resolve) => setTimeout(resolve, 0))
    const content = f.chat.messages.map((message) => message.content)
    f.chat.$dispose()
    return content
  })
  expect(messages).toContain('new visit')
  expect(messages).not.toContain('obsolete snapshot')
})
