import { test, expect, type Page } from '@playwright/test'

/**
 * Chat surface: sending a turn, watching it stream, steering it while it runs.
 *
 * Runs against the demo gateway, which scripts both text and tool steps, so
 * every state the transcript renders is reachable without a daemon.
 */

/** Opens the demo workspace and the chat surface. */
async function openChat(page: Page): Promise<void> {
  await page.goto('/')
  await page.getByLabel('Workspace path').fill('D:/metteur-demo/metrics')
  await page.getByRole('button', { name: 'Open', exact: true }).click()
  // Wait for the workspace shell before using the activity rail.
  await expect(page.getByTestId('file-tree')).toBeVisible({ timeout: 15_000 })
  await page.getByRole('button', { name: 'Chat', exact: true }).first().click()
  await expect(page.getByRole('log', { name: 'Conversation' })).toBeVisible({ timeout: 15_000 })
}

/** Types a message and sends it with Enter. */
async function say(page: Page, text: string): Promise<void> {
  const input = page.getByRole('textbox', { name: 'Message' })
  await input.fill(text)
  await input.press('Enter')
}

for (const next of ['text', 'tool', 'tool_args', 'done', 'error'] as const) {
  test(`whole-turn timing continues through ${next} until the terminal event`, async ({ page }) => {
    await openChat(page)
    await page.clock.install()
    // Exercise the real store/component boundary with controlled stream timing.
    await page.evaluate(async (transition) => {
      const path = '/src/core/index.ts'
      const { gateway } = await import(path)
      gateway.sendChat = async (...args: unknown[]) => {
        ;(args[5] as ((session: string) => void) | undefined)?.('demo')
        const emit = args[3] as (message: Record<string, unknown>) => void
        const progress = args[10] as (value: { name: string; bytes: number }) => void
        const base = { id: 'timed-reasoning', role: 'assistant', createdAt: Date.now() }
        await new Promise((resolve) => setTimeout(resolve, 2000))
        emit({
          ...base,
          content: '',
          reasoning: 'Checking the workspace.',
          reasoningPending: true,
          pending: true,
        })
        await new Promise((resolve) => setTimeout(resolve, 4000))
        if (transition === 'error') return { ok: false, error: 'Test stream failure' }
        if (transition === 'done') return { ok: true, value: undefined }
        if (transition === 'tool_args') progress({ name: 'ReadFile', bytes: 256 })
        else if (transition === 'tool') {
          emit({
            id: 'timed-tool',
            role: 'tool',
            actor: 'ReadFile',
            content: '',
            pending: true,
            createdAt: Date.now(),
          })
        } else emit({ ...base, content: 'Answer in progress', pending: true })
        await new Promise((resolve) => setTimeout(resolve, 6000))
        emit({ ...base, content: 'Finished.', reasoningPending: false })
        await new Promise((resolve) => setTimeout(resolve, 1000))
        return { ok: true, value: undefined }
      }
    }, next)
    await say(page, 'Check the timer')
    const reasoning = page.locator('.chat-reasoning-trigger').first()
    await expect(page.getByRole('button', { name: 'Stop', exact: true })).toBeVisible()
    await page.clock.runFor(2100)
    await expect(reasoning).toHaveText('Working for 2s')
    await page.clock.runFor(1000)
    await expect(reasoning).toHaveText('Working for 3s')
    await page.clock.runFor(1000)
    await expect(reasoning).toHaveText('Working for 4s')
    await expect(reasoning.locator('.chat-reasoning-time')).toHaveCSS(
      'font-variant-numeric',
      'tabular-nums',
    )
    await expect(page.locator('.chat-status')).not.toContainText(/\d+s\b/)
    await page.clock.runFor(2000)
    await expect(reasoning).toHaveText(
      `${next === 'done' || next === 'error' ? 'Worked' : 'Working'} for 6s`,
    )
    // Keep counting through text/tool work; terminal events stop immediately.
    await page.clock.runFor(3000)
    await expect(reasoning).toHaveText(
      `${next === 'done' || next === 'error' ? 'Worked for 6' : 'Working for 9'}s`,
    )
    await page.clock.runFor(3000)
    await expect(reasoning).toHaveText(
      `${next === 'done' || next === 'error' ? 'Worked for 6' : 'Working for 12'}s`,
    )
    await page.clock.runFor(3000)
    await expect(reasoning).toHaveText(
      `Worked for ${next === 'done' || next === 'error' ? 6 : 13}s`,
    )
    // Unmount the conversation and return: timing belongs to the message.
    await page.evaluate(async () => {
      const path = '/src/router.ts'
      const { router } = await import(path)
      const previous = router.currentRoute.value.fullPath
      await router.push('/settings')
      await router.push(previous)
    })
    await expect(reasoning).toHaveText(
      `Worked for ${next === 'done' || next === 'error' ? 6 : 13}s`,
    )
  })
}

test('a turn streams an answer and settles', async ({ page }) => {
  await openChat(page)
  await say(page, 'Summarise the metrics workspace')

  // The user's own turn is echoed immediately.
  await expect(page.getByText('Summarise the metrics workspace').first()).toBeVisible()

  // The answer arrives as markdown and the thinking block reports itself.
  await expect(page.getByText(/Got it|I'll wire up a blueprint/).first()).toBeVisible({
    timeout: 15_000,
  })
  await expect(
    page
      .getByRole('log')
      .getByText(/^(Working|Worked|Thought)( for .+s)?$/)
      .first(),
  ).toBeVisible()

  // Copy affordances appear once the turn settles.
  await expect(page.getByRole('button', { name: 'Copy answer' }).first()).toBeVisible()
})

test('text-only turns include initial waiting and freeze when cancelled', async ({ page }) => {
  await openChat(page)
  await page.clock.install()
  await page.evaluate(async () => {
    const path = '/src/core/index.ts'
    const { gateway } = await import(path)
    gateway.sendChat = async (...args: unknown[]) => {
      ;(args[5] as ((session: string) => void) | undefined)?.('demo')
      const emit = args[3] as (message: Record<string, unknown>) => void
      const signal = args[9] as AbortSignal
      await new Promise((resolve) => setTimeout(resolve, 1000))
      emit({
        id: 'text-only',
        role: 'assistant',
        content: 'Still writing',
        pending: true,
        createdAt: Date.now(),
      })
      await new Promise((resolve) =>
        signal.addEventListener('abort', () => resolve(undefined), { once: true }),
      )
      return { ok: true, data: undefined }
    }
  })
  await say(page, 'No reasoning needed')
  await expect(page.getByRole('button', { name: 'Stop', exact: true })).toBeVisible()
  await page.clock.runFor(3100)
  const timer = page.locator('.chat-reasoning-trigger')
  await expect(timer).toHaveText('Working for 3s')
  await page.getByRole('button', { name: 'Stop', exact: true }).click()
  await page.clock.runFor(1000)
  await expect(timer).toHaveText('Worked for 3s')
  await page.clock.runFor(5000)
  await expect(timer).toHaveText('Worked for 3s')
})

test('restored transcripts retain measured durations and do not invent legacy timings', async ({
  page,
}) => {
  await openChat(page)
  await page.evaluate(async () => {
    const gatewayPath = '/src/core/grpc-gateway.ts'
    const corePath = '/src/core/index.ts'
    const storePath = '/src/stores/chat.ts'
    const { GrpcGateway } = await import(gatewayPath)
    const { gateway } = await import(corePath)
    const { useChatStore } = await import(storePath)
    const transport = new GrpcGateway('')
    // Keep the real wire-to-message decoder; stub only the RPC transport.
    transport.client = {
      getChatSession: async () => ({
        sessionId: 'timing-history',
        createdAt: 123,
        historyJson: '[]',
        todosJson: '[]',
        transcriptJson: JSON.stringify([
          {
            role: 'assistant',
            at: 123,
            content: 'Measured answer',
            reasoning: 'Measured reasoning',
            turn_elapsed_ms: 7300,
          },
          { role: 'assistant', at: 124, content: 'Legacy answer', reasoning: 'Legacy reasoning' },
          {
            role: 'assistant',
            at: 125,
            content: 'Short answer',
            reasoning: 'Short reasoning',
            turn_elapsed_ms: 120,
          },
        ]),
      }),
    }
    gateway.getChatSession = transport.getChatSession.bind(transport)
    await useChatStore().switchTo('timing-history')
  })
  const headings = page.locator('.chat-reasoning-trigger')
  await expect(headings).toHaveText(['Worked for 7s', 'Thought', 'Worked for <1s'])
})

test('a buffered stream uses the daemon duration instead of near-zero arrival time', async ({
  page,
}) => {
  await openChat(page)
  await page.route('**/api/chat/stream', (route) =>
    route.fulfill({
      contentType: 'text/event-stream',
      body: [
        { kind: 'reasoning_delta', content: 'Reasoning from the server.' },
        {
          kind: 'assistant',
          content: 'The answer.',
          detail_json: JSON.stringify({
            reasoning: 'Reasoning from the server.',
            reasoning_elapsed_ms: 1200,
          }),
        },
        { kind: 'done', content: '', detail_json: JSON.stringify({ turn_elapsed_ms: 9300 }) },
      ]
        .map((event) => `data: ${JSON.stringify(event)}\n\n`)
        .join(''),
    }),
  )
  await page.evaluate(async () => {
    const gatewayPath = '/src/core/grpc-gateway.ts'
    const corePath = '/src/core/index.ts'
    const { GrpcGateway } = await import(gatewayPath)
    const { gateway } = await import(corePath)
    const transport = new GrpcGateway('')
    gateway.sendChat = transport.sendChat.bind(transport)
  })
  await say(page, 'Check buffered timing')
  await expect(page.locator('.chat-reasoning-trigger')).toHaveText('Worked for 9s')
})

test('a tool call is visible while it runs and settles with a duration', async ({ page }) => {
  await openChat(page)
  // The demo scripts an EditFile step with a diff when the prompt mentions a
  // blueprint.
  await say(page, 'Draft a blueprint for the metrics pipeline')

  const activity = page.locator('.chat-activity').first()
  await expect(activity).toBeVisible({ timeout: 15_000 })
  await expect(activity.locator('.chat-tool-row, .chat-group-row').first()).toBeVisible()
  await expect(page.getByText('Edit').first()).toBeVisible()

  // Once the turn settles the group collapses; open it, then the row itself.
  await expect(page.getByRole('button', { name: 'Copy answer' }).first()).toBeVisible({
    timeout: 15_000,
  })
  const diff = page.locator('.diff-add').first()
  const editRow = page.locator('.chat-tool-row').filter({ hasText: 'Edit' }).first()
  if (!(await editRow.isVisible().catch(() => false))) {
    await page.locator('.chat-group-row').first().click()
  }
  await editRow.click()
  await expect(diff).toBeVisible({ timeout: 10_000 })
})

test('the composer stays usable while the agent works', async ({ page }) => {
  await openChat(page)
  await say(page, 'Draft a blueprint for the metrics pipeline')

  const input = page.getByRole('textbox', { name: 'Message' })
  await expect(input).toBeEnabled()
  // Enter during a run queues the message instead of dropping it.
  await input.fill('Also rename the file')
  await input.press('Enter')
  await expect(page.getByText('Also rename the file').first()).toBeVisible()

  // The turn can be stopped from the composer.
  await expect(page.getByRole('button', { name: 'Stop' })).toBeVisible()
})

test('switching away and back keeps the tool calls', async ({ page }) => {
  const pageErrors: string[] = []
  page.on('pageerror', (err) => pageErrors.push(err.message))

  await openChat(page)
  await say(page, 'Draft a blueprint for the metrics pipeline')
  await expect(page.locator('.chat-activity').first()).toBeVisible({ timeout: 15_000 })
  await expect(page.getByRole('button', { name: 'Copy answer' }).first()).toBeVisible({
    timeout: 15_000,
  })
  const toolsBefore = await page.locator('.chat-tool-row, .chat-group-row').count()
  expect(toolsBefore).toBeGreaterThan(0)

  // A new conversation clears the surface, then the stored thread is reopened.
  await page.getByRole('button', { name: 'New conversation' }).click()
  await expect(page.locator('.chat-activity')).toHaveCount(0)
  await page.getByRole('button', { name: /Conversations/ }).click()
  await page.getByText('Draft a blueprint for the metrics pipeline').first().click()

  await expect(page.locator('.chat-tool-row, .chat-group-row').first()).toBeVisible({
    timeout: 10_000,
  })
  expect(await page.locator('.chat-tool-row, .chat-group-row').count()).toBe(toolsBefore)
  expect(pageErrors).toEqual([])
})
