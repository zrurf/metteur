import { test, expect } from '@playwright/test'

test('production stack persists a real local-provider tool turn across reloads', async ({
  page,
  request,
}) => {
  const root = await request.get('/')
  const html = await root.text()
  expect(html).toContain('<title>Metteur</title>')
  expect(html).toMatch(/<meta\s+name="metteur-gateway"\s+content="grpc"/)
  const fixtureURL = process.env.LIVE_FIXTURE_URL!
  const nonce = process.env.LIVE_NONCE!
  expect(html).toContain(`<meta name="metteur-live-fixture" content="${nonce}">`)
  const before = await (await request.get(`${fixtureURL}/fixture`)).json()
  expect(before.fixture).toBe('metteur-local-provider-v1')
  expect(before.nonce).toBe(nonce)
  expect(before.calls).toBe(0)

  const errors: string[] = []
  const rpcResponses: string[] = []
  page.on('pageerror', (error) => errors.push(error.message))
  page.on('response', (response) => {
    if (response.url().includes('/metteur.Daemon/') && response.ok())
      rpcResponses.push(response.url())
  })
  await page.goto(`/?open=${encodeURIComponent(process.env.LIVE_WORKSPACE!)}`)
  await expect(page.getByTestId('file-tree')).toBeVisible()
  await expect(page).toHaveURL(/\/work\/[^/]+\/explorer$/)
  await page.getByRole('button', { name: 'Chat', exact: true }).first().click()
  await expect(page.getByRole('log', { name: 'Conversation' })).toBeVisible()
  await page.getByRole('button', { name: 'New conversation' }).click()
  const input = page.getByRole('textbox', { name: 'Message' })
  await input.fill('Read app.ts and report its export.')
  await input.press('Enter')
  await expect(page.getByRole('button', { name: 'Copy answer' }).first()).toBeVisible({
    timeout: 40_000,
  })
  await expect(
    page.locator('.md-body').filter({ hasText: 'METTEUR_LOCAL_READY' }).first(),
  ).toContainText(nonce)
  await expect(page.getByText('execution error', { exact: false })).toHaveCount(0)
  await expect(page.getByText('model produced no final answer', { exact: false })).toHaveCount(0)
  await expect(page.getByRole('button', { name: /^Context:/ })).toBeVisible()
  const proof = await (await request.get(`${fixtureURL}/fixture`)).json()
  expect(proof.calls).toBe(2)
  expect(proof.toolRequests).toBe(1)
  expect(proof.finalRequests).toBe(1)
  expect(proof.sawToolResult).toBe(true)
  expect(rpcResponses.some((url) => url.endsWith('/OpenWorkspace'))).toBe(true)
  expect(rpcResponses.some((url) => url.endsWith('/ListChatSessions'))).toBe(true)

  await page.getByRole('button', { name: 'New conversation' }).click()
  await expect(page.locator('.chat-activity')).toHaveCount(0)
  await page.getByRole('button', { name: /Conversations/ }).click()
  await page.getByRole('button', { name: /^Read app\.ts and report its export\. \d+ messages/ }).click()
  await expect(
    page.locator('.md-body').filter({ hasText: 'METTEUR_LOCAL_READY' }).first(),
  ).toContainText(nonce)
  await page.reload()
  await page.getByRole('button', { name: 'Chat', exact: true }).first().click()
  await page.getByRole('button', { name: /Conversations/ }).click()
  await page.getByRole('button', { name: /^Read app\.ts and report its export\. \d+ messages/ }).click()
  await expect(
    page.locator('.md-body').filter({ hasText: 'METTEUR_LOCAL_READY' }).first(),
  ).toContainText(nonce)
  await expect(
    page.locator('.chat-activity .chat-tool-row, .chat-activity .chat-group-row').first(),
  ).toBeVisible()
  const after = await (await request.get(`${fixtureURL}/fixture`)).json()
  expect(after.calls).toBe(2)
  expect(errors).toEqual([])
})
