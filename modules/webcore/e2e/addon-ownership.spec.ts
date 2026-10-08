import { test, expect } from '@playwright/test'

test('addon toggles use owning workspace identity and surface failed admission', async ({ page }) => {
  await page.goto('/')
  await page.getByLabel('Workspace path').fill('D:/metteur-demo/addon-a')
  await page.getByRole('button', { name: 'Open', exact: true }).click()
  await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeVisible()
  await page.evaluate(async () => {
    const c = '/src/core/index.ts', a = '/src/stores/addon.ts'
    const { gateway } = await import(c)
    const store = (await import(a)).useAddonStore()
    const entries = ['a', 'b'].map((scope) => ({
      id: 'com.example.same', name: `Package ${scope}`, version: '1.0.0', description: '',
      scope: 'workspace', scopeRoot: `D:/metteur-demo/addon-${scope}`, fingerprint: scope.repeat(64),
      status: scope === 'a' ? 'Loaded' : 'Failed', error: scope === 'b' ? 'Package fingerprint changed; reinstall with explicit grants' : '',
      enabled: scope === 'a', toolCount: 1, fragmentCount: 1, grantedPermissions: scope === 'a' ? ['fs:read'] : [],
    }))
    gateway.listAddons = async () => ({ ok: true, data: entries })
    gateway.setAddonEnabled = async (_id, _enabled, scope) => ({ ok: false, error: `Active run owns ${scope}; finish it before changing addons` })
    await store.refresh()
  })
  await page.getByRole('button', { name: 'Settings', exact: true }).click()
  await page.getByRole('button', { name: 'Addons', exact: true }).click()
  await expect(page.getByText('Package a', { exact: true })).toBeVisible()
  await expect(page.getByText('Package b', { exact: true })).toBeVisible()
  await expect(page.getByText('Package fingerprint changed; reinstall with explicit grants')).toBeVisible()
  await page.getByRole('button', { name: 'Disabled', exact: true }).click()
  await expect(page.getByRole('alert')).toContainText('D:/metteur-demo/addon-b')
  await expect(page.getByText('Granted: fs:read')).toBeVisible()
  const chatScopes = await page.evaluate(async () => {
    const path = '/src/stores/chat.ts'
    const chat = (await import(path)).useChatStore()
    await chat.loadAddons()
    return chat.addons.map((addon) => addon.scopeRoot)
  })
  expect(chatScopes).toEqual(['D:/metteur-demo/addon-a'])
})
