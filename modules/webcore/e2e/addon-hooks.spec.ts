import { test, expect } from '@playwright/test'

test('addon hook delivery status stays distinct from package activation and refreshes safely', async ({ page }) => {
  await page.goto('/')
  await page.getByLabel('Workspace path').fill('D:/metteur-demo/hooks')
  await page.getByRole('button', { name: 'Open', exact: true }).click()
  await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeVisible()
  await page.evaluate(async () => {
    const path = '/src/core/index.ts'
    const { gateway } = await import(path)
    let count = 0
    gateway.listAddons = async () => ({ ok: true, data: [{
      id: 'com.test.observer', name: 'Observer package', version: '1.0.0', description: '', scope: 'workspace',
      scopeRoot: 'D:/metteur-demo/hooks', enabled: true, toolCount: 0, fragmentCount: 0, status: 'Loaded',
      hooks: [
        { name: 'Finished', event: 'node.finished', scopeRoot: 'D:/metteur-demo/hooks', eventId: `event-${++count}`, status: 'Succeeded', completed: count, failed: 0, error: '' },
        { name: 'Terminal', event: 'run.terminal', scopeRoot: 'D:/metteur-demo/hooks', eventId: 'failed-event', status: 'Failed', completed: 0, failed: 1, error: 'Hook callback failed, exceeded its limits or timed out' },
      ],
    }] })
    const storePath = '/src/stores/addon.ts'
    await (await import(storePath)).useAddonStore().refresh()
  })
  await page.getByRole('button', { name: 'Settings', exact: true }).click()
  await page.getByRole('button', { name: 'Addons', exact: true }).click()
  const status = page.getByTestId('addon-hook-status')
  await expect(status).toContainText('Finished · node.finished · Succeeded')
  await expect(status).toContainText('Terminal · run.terminal · Failed')
  await expect(status).toContainText('Hook callback failed, exceeded its limits or timed out')
  await expect(page.getByRole('button', { name: 'Enabled', exact: true })).toBeVisible()
  const before = await status.textContent()
  await page.getByRole('button', { name: 'Refresh addon status', exact: true }).click()
  await expect.poll(() => status.textContent()).not.toBe(before)
  await expect(status).toContainText('not replayed after restart')
})
