import { test, expect } from '@playwright/test'

test('S3 production task hands off one approved blueprint, retains the result and adds a verified reference before explicit Send', async ({
  page,
  request,
}) => {
  test.setTimeout(120_000)
  const workspace = process.env.LIVE_WORKSPACE!,
    provider = process.env.LIVE_FIXTURE_URL!,
    nonce = process.env.LIVE_NONCE!
  const proof = async () => (await request.get(`${provider}/fixture`)).json()
  const errors: string[] = []
  page.on('pageerror', (error) => errors.push(error.message))
  await page.goto(`/?open=${encodeURIComponent(workspace)}`)
  await expect(page.getByTestId('file-tree')).toBeVisible()
  const navigation = page.getByRole('navigation', { name: 'Main navigation' })
  await navigation.getByRole('button', { name: 'Tasks', exact: true }).click()
  await page.getByRole('button', { name: 'New task', exact: true }).click()
  const task = page.getByRole('dialog', { name: 'Start a Continuation Task', exact: true })
  await task
    .getByRole('textbox', { name: 'Goal', exact: true })
    .fill(
      'M3_LOCAL_BLUEPRINT: review the saved local evidence. Retain the run result for manual review.',
    )
  await task
    .getByRole('checkbox', {
      name: 'Allow this task to continue when this browser disconnects',
      exact: false,
    })
    .uncheck()
  await task
    .getByRole('checkbox', {
      name: 'Authorize this exact goal, model, time and limits',
      exact: false,
    })
    .check()
  await task.getByRole('button', { name: 'Start task', exact: true }).click()
  const launch = page.getByRole('dialog', { name: 'Review a Blueprint Run', exact: false })
  await expect(launch).toBeVisible({ timeout: 30_000 })
  await launch
    .getByRole('checkbox', { name: 'Resume this conversation when the run finishes', exact: true })
    .uncheck()
  await launch.getByRole('button', { name: 'Bound versions', exact: true }).click()
  await expect(launch).toContainText('Local review')
  await launch.getByRole('button', { name: 'Conversation handoff', exact: true }).click()
  await expect(launch).toContainText('Original roles remain visible')
  await launch
    .getByRole('checkbox', {
      name: 'I approve this version, handoff scope and completion policy',
      exact: true,
    })
    .check()
  await launch.getByRole('button', { name: 'Confirm and run', exact: true }).click()
  await expect(launch).toHaveCount(0)
  await expect(page.locator('.nx-live-detail > .nx-live-badge')).toHaveText('Completed', {
    timeout: 40_000,
  })
  await expect(
    page.getByText('Result retained for review; no automatic conversation turn.', { exact: true }),
  ).toBeVisible()
  const received = await proof()
  const result = page.locator('.workflow-result').first()
  await result.locator('summary').first().click()
  await expect(result.getByText('No deterministic check recorded', { exact: true })).toBeVisible()
  await expect(result.getByText('Not recorded', { exact: true })).toBeVisible()
  await page.getByRole('button', { name: 'Open execution', exact: true }).click()
  await page.getByRole('button', { name: 'Result', exact: true }).click()
  await expect(page.getByText('No deterministic check recorded', { exact: true })).toBeVisible()
  await navigation.getByRole('button', { name: 'Tasks', exact: true }).click()
  if ((await result.getAttribute('open')) === null) await result.locator('summary').first().click()
  await result.getByRole('button', { name: 'Add result to draft', exact: true }).click()
  const input = page.getByRole('textbox', { name: 'Message', exact: true })
  await expect(input).toHaveValue('Review the selected saved result.')
  await expect(page.getByLabel('Selected saved results')).toBeVisible()
  expect((await proof()).calls).toBe(received.calls)
  await input.press('Enter')
  await expect(
    page.locator('.md-body').filter({ hasText: 'S3_RESULT_REVIEW_READY' }),
  ).toContainText(nonce, { timeout: 30_000 })
  const sent = await proof()
  expect(sent.resultReferenceRequests).toBe(1)
  expect(sent.sawResultData).toBe(true)
  expect(sent.calls).toBe(received.calls + 1)
  await page.reload()
  await navigation.getByRole('button', { name: 'Chat', exact: true }).click()
  await expect(
    page.locator('.md-body').filter({ hasText: 'S3_RESULT_REVIEW_READY' }),
  ).toContainText(nonce)
  const user = page.locator('.chat-user-turn').last()
  await expect(user).toContainText('Review the selected saved result.')
  await expect(user.locator('details > summary')).toHaveText('Selected context & full message')
  await expect(user.locator('details')).not.toHaveAttribute('open')
  expect((await proof()).calls).toBe(sent.calls)
  expect(errors).toEqual([])
})
