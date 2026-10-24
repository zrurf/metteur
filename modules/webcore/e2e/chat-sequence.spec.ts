import { expect, test } from '@playwright/test'

test('assistant reasoning, tools and the next answer retain their sequence live and after restore', async ({
  page,
}) => {
  await page.goto('/')
  await page.getByLabel('Workspace path').fill('D:/metteur-demo/ordered-chat')
  await page.getByRole('button', { name: 'Open', exact: true }).click()
  await expect(page.getByLabel('Message', { exact: true })).toBeVisible()
  await page.route('**/api/chat/stream', (route) =>
    route.fulfill({
      contentType: 'text/event-stream',
      body: [
        { kind: 'reasoning_delta', content: 'First reasoning segment' },
        { kind: 'assistant_delta', content: 'Before the file call' },
        {
          kind: 'assistant',
          content: 'Before the file call',
          detail_json: JSON.stringify({ reasoning: 'First reasoning segment' }),
        },
        {
          kind: 'tool_start',
          detail_json: JSON.stringify({ call_id: 'read-one', name: 'ReadFile' }),
        },
        {
          kind: 'tool',
          content: 'Synthetic file result',
          detail_json: JSON.stringify({ call_id: 'read-one', name: 'ReadFile', ok: true }),
        },
        { kind: 'reasoning_delta', content: 'Second reasoning segment' },
        {
          kind: 'assistant',
          content: 'After the file call',
          detail_json: JSON.stringify({ reasoning: 'Second reasoning segment' }),
        },
        { kind: 'done', detail_json: JSON.stringify({ turn_elapsed_ms: 2100 }) },
      ]
        .map((event) => `data: ${JSON.stringify(event)}\n\n`)
        .join(''),
    }),
  )
  await page.evaluate(async () => {
    const core = '/src/core/index.ts',
      transportPath = '/src/core/grpc-gateway.ts'
    const { gateway } = await import(core),
      { GrpcGateway } = await import(transportPath)
    const transport = new GrpcGateway('')
    gateway.sendChat = transport.sendChat.bind(transport)
  })
  await page.getByLabel('Message', { exact: true }).fill('Synthetic chronology probe')
  await page.getByLabel('Message', { exact: true }).press('Enter')
  await expect(page.getByRole('button', { name: 'Stop', exact: true })).toHaveCount(0)
  const rows = page.locator(
    '.chat-thread-inner > .chat-turn:not(:has(.chat-user-turn)), .chat-thread-inner > .chat-activity',
  )
  await expect(rows).toHaveCount(3)
  await expect(rows.nth(0)).toContainText('Before the file call')
  await expect(rows.nth(1)).toContainText('Synthetic file result')
  await expect(rows.nth(2)).toContainText('After the file call')
  await expect(page.locator('.chat-reasoning-trigger')).toHaveCount(2)
  const messages = await page.evaluate(async () => {
    const p = '/src/stores/chat.ts'
    return (await import(p))
      .useChatStore()
      .messages.filter((m) => m.role === 'assistant' || m.role === 'tool')
      .map((m) => ({ id: m.id, role: m.role, content: m.content, reasoning: m.reasoning }))
  })
  expect(messages.map((m) => m.role)).toEqual(['assistant', 'tool', 'assistant'])
  expect(messages[0]!.reasoning).toBe('First reasoning segment')
  expect(messages[2]!.reasoning).toBe('Second reasoning segment')
  expect(new Set(messages.map((m) => m.id)).size).toBe(3)
  await page.evaluate(async (snapshotMessages) => {
    const core = '/src/core/index.ts',
      transportPath = '/src/core/grpc-gateway.ts',
      chatPath = '/src/stores/chat.ts'
    const { gateway } = await import(core),
      { GrpcGateway } = await import(transportPath)
    const transport = new GrpcGateway('')
    transport.client = {
      getChatSession: async () => ({
        sessionId: 'ordered-saved',
        createdAt: 123,
        historyJson: '[]',
        todosJson: '[]',
        transcriptJson: JSON.stringify(
          snapshotMessages.map((m) => ({
            ...m,
            at: 123,
            tool: m.role === 'tool' ? 'ReadFile' : undefined,
          })),
        ),
      }),
    }
    gateway.getChatSession = transport.getChatSession.bind(transport)
    await (await import(chatPath)).useChatStore().switchTo('ordered-saved')
  }, messages)
  await expect(rows).toHaveCount(3)
  await expect(rows.nth(0)).toContainText('Before the file call')
  await expect(rows.nth(1)).toContainText('Synthetic file result')
  await expect(rows.nth(2)).toContainText('After the file call')
})
