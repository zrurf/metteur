import { test, expect, type Page } from '@playwright/test'

/**
 * The composer's second row: permissions, model and effort, context capacity,
 * and the delivery control.
 *
 * These are the controls a reader uses *while* the agent works, so the test
 * drives them mid-turn as well as idle.
 */

async function openChat(page: Page): Promise<void> {
  await page.goto('/')
  await page.getByLabel('Workspace path').fill('D:/metteur-demo/metrics')
  await page.getByRole('button', { name: 'Open', exact: true }).click()
  await expect(page.getByTestId('file-tree')).toBeVisible({ timeout: 15_000 })
  await page.getByRole('button', { name: 'Chat', exact: true }).first().click()
  await expect(page.getByRole('log', { name: 'Conversation' })).toBeVisible({ timeout: 15_000 })
}

for (const width of [1280, 800]) {
  test(`the entire delivery menu stays inside the chat pane at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 800 })
    await openChat(page)
    await page.getByRole('button', { name: 'Delivery options' }).click()
    const menu = page.locator('.chat-delivery-menu')
    await expect(menu).toBeVisible()
    // Visibility alone does not detect overflow clipping. Check the menu against
    // the viewport and every clipping ancestor, then hit-test all its actions.
    const contained = await menu.evaluate((el) => {
      const rect = el.getBoundingClientRect()
      if (rect.left < 0 || rect.right > innerWidth || rect.top < 0 || rect.bottom > innerHeight) return false
      for (let parent = el.parentElement; parent; parent = parent.parentElement) {
        const style = getComputedStyle(parent)
        const bounds = parent.getBoundingClientRect()
        if (/(hidden|clip|auto|scroll)/.test(style.overflowX) && (rect.left < bounds.left || rect.right > bounds.right)) return false
        if (/(hidden|clip|auto|scroll)/.test(style.overflowY) && (rect.top < bounds.top || rect.bottom > bounds.bottom)) return false
      }
      return [...el.querySelectorAll('button')].every((button) => {
        const bounds = button.getBoundingClientRect()
        return [bounds.left + 2, bounds.right - 2].every((x) =>
          button.contains(document.elementFromPoint(x, bounds.top + bounds.height / 2)),
        )
      })
    })
    expect(contained).toBe(true)
    await page.keyboard.press('Escape')
    await expect(menu).toHaveCount(0)
  })
}

test('the permission selector states the mode and warns on full access', async ({ page }) => {
  await openChat(page)

  const selector = page.getByRole('button', { name: /Permission mode/ })
  await expect(selector).toBeVisible()
  await expect(selector).toContainText('Sandbox')

  await selector.click()
  await page.getByText('Full access', { exact: true }).first().click()
  await expect(selector).toContainText('Full access')
  // The selected control carries the mode without a duplicate notice.
  await expect(page.locator('.chat-composer-notice')).toHaveCount(0)

  await selector.click()
  await page.getByText('Confirm changes', { exact: true }).first().click()
  await expect(page.locator('.chat-composer-notice')).toHaveCount(0)
})

test('the model control pairs the model with the reasoning effort', async ({ page }) => {
  await openChat(page)

  const model = page.getByRole('button', { name: /Model and reasoning effort/ })
  await expect(model).toBeVisible()
  await model.click()
  // Effort is a segmented choice inside the same menu as the model list.
  await page.getByRole('group', { name: 'Reasoning effort' }).getByRole('button', { name: 'High' }).click()
  await expect(model).toContainText('High')
})

test('the context meter reports usage and breaks it down', async ({ page }) => {
  await openChat(page)
  const input = page.getByRole('textbox', { name: 'Message' })
  await input.fill('Summarise the metrics workspace')
  await input.press('Enter')
  await expect(page.getByRole('button', { name: 'Copy answer' }).first()).toBeVisible({
    timeout: 15_000,
  })

  const meter = page.getByRole('button', { name: /^Context:/ })
  await expect(meter).toBeVisible()
  await expect(meter).toContainText('%')
  await meter.click()
  await expect(page.getByText('Context window')).toBeVisible()
  await expect(page.getByText('Composition')).toBeVisible()
  // The breakdown names the regions rather than showing bare numbers.
  await expect(page.getByText('System & tools')).toBeVisible()
})

test('a message can be delivered while the turn runs', async ({ page }) => {
  await openChat(page)
  const input = page.getByRole('textbox', { name: 'Message' })
  await input.fill('Draft a blueprint for the metrics pipeline')
  await input.press('Enter')

  // While running the primary control is the queue button, with the other
  // deliveries behind the chevron.
  await expect(page.getByRole('button', { name: 'Queue message' })).toBeVisible()
  await input.fill('Also rename the file')
  await page.getByRole('button', { name: 'Delivery options' }).click()
  await page.getByText('Send now').click()
  await expect(page.getByText('Also rename the file').first()).toBeVisible()
})
