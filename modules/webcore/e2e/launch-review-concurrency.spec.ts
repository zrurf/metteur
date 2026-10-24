import { expect, test, type Page } from '@playwright/test'

async function mountReview(page: Page) {
  await page.goto('/')
  await page.getByLabel('Workspace path').fill('D:/metteur-demo/launch-review')
  await page.getByRole('button', { name: 'Open', exact: true }).click()
  await expect(page.getByLabel('Message', { exact: true })).toBeVisible()
  await page.evaluate(async () => {
    const corePath = '/src/core/index.ts',
      chatPath = '/src/stores/chat.ts'
    const { gateway } = await import(corePath)
    const chat = (await import(chatPath)).useChatStore()
    const { watch } = await import('/node_modules/.vite/deps/vue.js')
    gateway.connected.value = true
    let proposal = {
      request_type: 'blueprint_launch',
      intent_id: 'intent-one',
      digest: 'digest-one',
      blueprint_name: 'Review fixture',
      manifest: {},
      handoff: {
        goal: 'First reviewed goal',
        constraints: [],
        model_summary: '',
        message_end: 4,
        allow_source_read: false,
      },
      policy: { max_turns: 2, token_limit: 1000, expires_ms: Date.now() + 60000 },
      resume_on_completion: false,
    }
    let requestId = 'request-one'
    const show = () => {
      chat.approval = { requestId, detail: JSON.stringify(proposal) }
    }
    const fixture = (gateway.launchRace = {
      writes: 0,
      resolved: 0,
      decision: {},
      finish: () => {},
      updateVersion: () => {
        proposal = {
          ...proposal,
          digest: 'digest-two',
          handoff: { ...proposal.handoff, goal: 'New unreviewed goal' },
        }
        show()
      },
      replaceRequest: () => {
        requestId = 'request-two'
        proposal = { ...proposal, intent_id: 'intent-two', digest: 'digest-two' }
        show()
      },
    })
    gateway.resourceRequest = (_method: string, _workspace: string, payload: unknown) => {
      fixture.writes++
      fixture.decision = payload
      return new Promise((resolve) => {
        fixture.finish = () => resolve({ ok: true, data: {} })
      })
    }
    watch(
      () => chat.approval,
      (value) => {
        if (!value) fixture.resolved++
      },
    )
    show()
  })
  return page.getByRole('dialog', { name: /Review a Blueprint Run/ })
}

test('a changed proposal digest invalidates the previous approval checkbox', async ({ page }) => {
  const dialog = await mountReview(page)
  const approve = dialog.getByRole('checkbox', {
    name: 'I approve this version, handoff scope and completion policy',
    exact: true,
  })
  await approve.check()
  await expect(dialog.getByRole('button', { name: 'Confirm and run', exact: true })).toBeEnabled()
  await page.evaluate(async () =>
    (await import('/src/core/index.ts')).gateway.launchRace.updateVersion(),
  )
  await expect(approve).not.toBeChecked()
  await expect(dialog.getByRole('button', { name: 'Confirm and run', exact: true })).toBeDisabled()
})

test('an old launch acknowledgement cannot resolve the replacement request', async ({ page }) => {
  const dialog = await mountReview(page)
  await dialog
    .getByRole('checkbox', {
      name: 'I approve this version, handoff scope and completion policy',
      exact: true,
    })
    .check()
  await dialog.getByRole('button', { name: 'Confirm and run', exact: true }).click()
  await expect
    .poll(() =>
      page.evaluate(async () => (await import('/src/core/index.ts')).gateway.launchRace.writes),
    )
    .toBe(1)
  await page.evaluate(async () => {
    const fixture = (await import('/src/core/index.ts')).gateway.launchRace
    fixture.replaceRequest()
    fixture.finish()
  })
  await expect(dialog.getByRole('button', { name: 'Confirm and run', exact: true })).toBeDisabled()
  expect(
    await page.evaluate(
      async () => (await import('/src/core/index.ts')).gateway.launchRace.resolved,
    ),
  ).toBe(0)
})

test('policy changes require fresh confirmation and send the exact reviewed binding', async ({
  page,
}) => {
  const dialog = await mountReview(page)
  const approve = dialog.getByRole('checkbox', {
    name: 'I approve this version, handoff scope and completion policy',
    exact: true,
  })
  await approve.check()
  await dialog
    .getByRole('checkbox', { name: 'Resume this conversation when the run finishes', exact: true })
    .check()
  await expect(approve).not.toBeChecked()
  await approve.check()
  await dialog.getByRole('button', { name: 'Conversation handoff', exact: true }).click()
  await dialog
    .getByRole('checkbox', { name: 'Allow bounded reading through message 4', exact: true })
    .check()
  await expect(approve).not.toBeChecked()
  await approve.check()
  await dialog.getByRole('button', { name: 'Confirm and run', exact: true }).click()
  expect(
    await page.evaluate(
      async () => (await import('/src/core/index.ts')).gateway.launchRace.decision,
    ),
  ).toEqual({
    request_id: 'request-one',
    intent_id: 'intent-one',
    original_digest: 'digest-one',
    allow: true,
    resume_on_completion: true,
    allow_source_read: true,
  })
  await page.evaluate(async () => (await import('/src/core/index.ts')).gateway.launchRace.finish())
  await expect
    .poll(() =>
      page.evaluate(async () => (await import('/src/core/index.ts')).gateway.launchRace.resolved),
    )
    .toBe(1)
})

test('decline records a negative decision without requiring approval', async ({ page }) => {
  const dialog = await mountReview(page)
  await dialog.getByRole('button', { name: 'Decline', exact: true }).click()
  expect(
    await page.evaluate(
      async () => (await import('/src/core/index.ts')).gateway.launchRace.decision,
    ),
  ).toMatchObject({ request_id: 'request-one', original_digest: 'digest-one', allow: false })
  await page.evaluate(async () => (await import('/src/core/index.ts')).gateway.launchRace.finish())
  await expect
    .poll(() =>
      page.evaluate(async () => (await import('/src/core/index.ts')).gateway.launchRace.resolved),
    )
    .toBe(1)
})
