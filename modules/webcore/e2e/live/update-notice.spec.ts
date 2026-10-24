import { test, expect } from '@playwright/test'

test('first worker activation is silent; real update notice leaves chat usable and dismisses without losing draft', async ({
  page,
}, info) => {
  await page.addInitScript(() => {
    const container = new EventTarget()
    const registration = Object.assign(new EventTarget(), {
      installing: null,
      waiting: null,
      update: async () => undefined,
    })
    let controller: object | null = null
    Object.defineProperty(container, 'controller', { get: () => controller })
    Object.assign(container, { register: async () => registration })
    Object.defineProperty(navigator, 'serviceWorker', { value: container, configurable: true })
    ;(window as any).activateTestWorker = () => {
      controller = {}
      container.dispatchEvent(new Event('controllerchange'))
    }
  })
  await page.goto(`/?open=${encodeURIComponent(process.env.LIVE_WORKSPACE!)}`)
  await page
    .getByRole('navigation', { name: 'Main navigation' })
    .getByRole('button', { name: 'Chat', exact: true })
    .click()
  const input = page.getByRole('textbox', { name: 'Message', exact: true })
  await input.fill('Keep this draft while postponing the application update 中文')
  await page.evaluate(() => (window as any).activateTestWorker())
  const notice = page.getByRole('status', { name: 'Application update', exact: true })
  await expect(notice).toHaveCount(0)
  await page.evaluate(() => (window as any).activateTestWorker())
  await expect(notice).toBeVisible()
  await page.getByRole('button', { name: 'Send', exact: true }).click({ trial: true })
  await page.setViewportSize({ width: 800, height: 650 })
  await page.getByRole('button', { name: 'Send', exact: true }).click({ trial: true })
  await page.screenshot({ path: info.outputPath('update-notice.png') })
  await notice.getByRole('button', { name: 'Dismiss update notification' }).click()
  await expect(notice).toHaveCount(0)
  await expect(input).toHaveValue('Keep this draft while postponing the application update 中文')
})
