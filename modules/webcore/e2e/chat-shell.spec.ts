import { test, expect, type Page } from '@playwright/test'

/**
 * Shell-level regressions: the composer's controls, the transcript's scroll
 * affordances and the app-wide tooltip.
 */

async function openChat(page: Page): Promise<void> {
  await page.goto('/')
  await page.getByLabel('Workspace path').fill('D:/metteur-demo/metrics')
  await page.getByRole('button', { name: 'Open', exact: true }).click()
  await expect(page.getByTestId('file-tree')).toBeVisible({ timeout: 15_000 })
  await page.getByRole('button', { name: 'Chat', exact: true }).first().click()
  await expect(page.getByRole('log', { name: 'Conversation' })).toBeVisible({ timeout: 15_000 })
}

test('the composer has one send control, not two', async ({ page }) => {
  await openChat(page)
  const controls = page.locator('.chat-deliver-group')
  await expect(controls).toHaveCount(1)
  // One primary action plus its alternatives: a single send-lookalike, no more.
  await expect(controls.getByRole('button', { name: 'Send' })).toHaveCount(1)
  await expect(controls.getByRole('button', { name: 'Delivery options' })).toHaveCount(1)
})

test('the tooltip closes when the pointer leaves', async ({ page }) => {
  await openChat(page)
  // An icon-only control: its accessible label becomes the tooltip.
  await page.getByRole('button', { name: 'Add context' }).hover()
  const tooltip = page.locator('.app-tooltip')
  await expect(tooltip).toBeVisible({ timeout: 3_000 })
  await expect(tooltip).toHaveText('Add context')

  // Move the pointer somewhere without a tooltip: the bubble must go away.
  await page.mouse.move(10, 10)
  await expect(tooltip).toBeHidden({ timeout: 3_000 })
})

test('a short conversation shows no jump button', async ({ page }) => {
  await openChat(page)
  const input = page.getByRole('textbox', { name: 'Message' })
  await input.fill('Summarise the metrics workspace')
  await input.press('Enter')
  await expect(page.getByRole('button', { name: 'Copy answer' }).first()).toBeVisible({
    timeout: 15_000,
  })
  await expect(page.getByText('New messages')).toHaveCount(0)
})

test('the header keeps session controls and leaves the model to the composer', async ({ page }) => {
  await openChat(page)
  const header = page.locator('.chat-header')
  await expect(header.getByRole('button', { name: /Conversations/ })).toBeVisible()
  await expect(header.getByRole('button', { name: 'Plugins' })).toBeVisible()
  // The model, its effort and the permission mode live in the composer row only.
  await expect(header.getByRole('button', { name: /Model and reasoning effort/ })).toHaveCount(0)
  await expect(header.getByRole('button', { name: /Permission mode/ })).toHaveCount(0)
})

for (const theme of ['light', 'dark'] as const) {
  test(`the reading column and starter actions work in ${theme} mode`, async ({ page }) => {
    await page.setViewportSize({ width: 1600, height: 1000 })
    await page.addInitScript((mode) => localStorage.setItem('metteur.theme', mode), theme)
    await openChat(page)
    const thread = page.locator('.chat-thread-inner')
    const composer = page.locator('.chat-column-composer')
    // Workspace hydration can replace the empty transcript after it is visible.
    await expect.poll(async () => {
      const readingBox = await thread.boundingBox()
      const composerBox = await composer.boundingBox()
      return !!readingBox && !!composerBox && readingBox.width <= 920 && Math.abs(readingBox.x - composerBox.x) < 1
    }).toBe(true)
    await expect(page.locator('.chat-starter')).toHaveCount(3)
    await page.getByRole('button', { name: /Explain this workspace/ }).click()
    await expect(page.getByRole('textbox', { name: 'Message' })).toHaveValue(
      'Explain how this workspace is organised and what the main entry points are.',
    )
    await expect(page.locator('.chat-user-turn')).toHaveCount(0)
  })
}

test('a narrow short chat pane keeps the welcome heading and send control reachable', async ({ page }) => {
  await page.setViewportSize({ width: 800, height: 600 })
  await openChat(page)
  const log = page.getByRole('log', { name: 'Conversation' })
  await expect.poll(() => log.evaluate((el) => el.scrollTop)).toBe(0)
  await expect(page.getByRole('heading', { name: 'What should we work on?' })).toBeInViewport()
  await expect(page.getByRole('button', { name: 'Send', exact: true })).toBeInViewport()
  const overflow = await page.locator('.chat-surface').evaluate((el) => el.scrollWidth - el.clientWidth)
  expect(overflow).toBeLessThanOrEqual(1)
  const cards = await page.locator('.chat-starter').all()
  const first = await cards[0].boundingBox()
  const second = await cards[1].boundingBox()
  expect(second!.y).toBeGreaterThan(first!.y + first!.height)
})
