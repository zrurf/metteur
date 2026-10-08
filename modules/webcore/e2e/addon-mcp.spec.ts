import { test, expect } from '@playwright/test'

test('MCP status shows scoped addon ownership, failures and user overrides', async ({ page }) => {
  await page.goto('/')
  await page.getByLabel('Workspace path').fill('D:/metteur-demo/mcp-a')
  await page.getByRole('button', { name: 'Open', exact: true }).click()
  await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeVisible()
  await page.evaluate(async () => {
    const path = '/src/core/index.ts'
    const { gateway } = await import(path)
    gateway.listMcpServers = async (scope?: string) => ({ ok: true, data: [
      { name: 'ComTestLocal', owner: 'com.test', scopeRoot: scope, status: 'Connected', toolCount: 1, error: '' },
      { name: 'ComOtherRemote', owner: 'com.other', scopeRoot: scope, status: 'Failed', toolCount: 0, error: 'Addon MCP initialization timed out' },
      { name: 'ComUserLocal', owner: 'com.user', scopeRoot: scope, status: 'Overridden', toolCount: 0, error: '' },
    ] })
  })
  await page.getByRole('button', { name: 'Settings', exact: true }).click()
  await page.getByRole('button', { name: 'MCP Servers', exact: true }).click()
  const status = page.getByTestId('mcp-status')
  await expect(status).toContainText('ComTestLocal · Connected')
  await expect(status).toContainText('Addon com.test · D:/metteur-demo/mcp-a · 1 tools')
  await expect(status).toContainText('ComOtherRemote · Failed')
  await expect(status).toContainText('Addon MCP initialization timed out')
  await expect(status).toContainText('ComUserLocal · Overridden')
})
