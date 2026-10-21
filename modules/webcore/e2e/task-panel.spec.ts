import { expect, test } from '@playwright/test'

test('task continuation initializes without a setup error and opens its bounded task dialog', async ({ page }) => {
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  page.on('console', message => { if (message.type() === 'error') errors.push(message.text()) })
  await page.goto('/')
  await page.evaluate(async () => {
    const { gateway } = await import('/src/core/index.ts')
    gateway.connected.value = true
    gateway.resourceRequest = async () => ({ ok: true, data: { items: [], next: null } })
  })
  await page.getByLabel('Workspace path').fill('D:/metteur-demo/task-panel')
  await page.getByRole('button', { name: 'Open', exact: true }).click()
  await expect(page.getByTestId('file-tree')).toBeVisible()
  await page.getByRole('navigation', { name: 'Main navigation' }).getByRole('button', { name: 'Task continuation', exact: true }).click()
  await expect(page.getByRole('heading', { name: /^Task continuation/ })).toBeVisible()
  await page.getByRole('button', { name: 'New task', exact: true }).click()
  await expect(page.getByRole('dialog', { name: 'Start a continuation task' })).toBeVisible()
  expect(errors).toEqual([])
})
