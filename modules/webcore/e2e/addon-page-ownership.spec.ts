import { test, expect, type Page } from '@playwright/test'

async function setup(page: Page) {
  await page.goto('/')
  await page.getByLabel('Workspace path').fill('D:/metteur-demo/addon-a')
  await page.getByRole('button', { name: 'Open', exact: true }).click()
  await expect(page.getByTestId('file-tree')).toBeVisible()
  await page.evaluate(async () => {
    const core = '/src/core/index.ts',
      { gateway } = await import(core)
    const original = gateway.resourceRequest
    const state = (gateway.pageOwnershipTest = {
      delayOpen: false,
      openPending: false,
      heartbeatPending: false,
      reviewPending: false,
      closed: [],
      releaseOpen: () => {},
      releaseHeartbeat: () => {},
      releaseReview: () => {},
    })
    const identity = {
      id: 'com.test.pages',
      version: '1.0.0',
      fingerprint: 'synthetic',
      scope: 'workspace',
    }
    const entry = (id: string) => ({
      package: identity,
      page: {
        id,
        title: `Inspector ${id}`,
        capabilities: ['selected-preview', 'inspect-preview', 'own-reports'],
      },
    })
    const html =
      '<!doctype html><button>Inspect</button><script>addEventListener("message",e=>{if(e.data.type!=="metteur-addon-connect")return;const p=e.ports[0];p.onmessage=e=>{if(e.data.type==="target-changed")p.postMessage({bridge_version:1,request_id:"00000000-0000-4000-8000-000000000001",method:"inspect_selected",params:{}})};p.start()})</script>'
    const instance = (id: string) => ({
      instance_id: id,
      nonce: `nonce-${id}`,
      page_id: id,
      package: identity,
      title: `Inspector ${id}`,
      html,
      csp: "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'",
      capabilities: entry(id).page.capabilities,
      lease_seconds: 900,
    })
    const report = (id: string) => ({
      id: `report-${id}`,
      digest: `digest-${id}`,
      target: { id: 'preview', title: `Evidence ${id}`, revision: 1 },
      human_acceptance: null,
      scope: id,
    })
    gateway.resourceRequest = async (
      method: string,
      root: string,
      payload: any,
      revision?: number,
    ) => {
      if (method !== 'addonPages') return original.call(gateway, method, root, payload, revision)
      switch (payload.operation) {
        case 'list':
          return {
            ok: true,
            data: { pages: root.endsWith('addon-b') ? [entry('B')] : [entry('A'), entry('B')] },
          }
        case 'targets':
          return {
            ok: true,
            data: { targets: [{ id: 'preview', title: 'Synthetic Preview', revision: 1 }] },
          }
        case 'open':
          if (payload.page_id === 'A' && state.delayOpen) {
            state.openPending = true
            await new Promise<void>((resolve) => {
              state.releaseOpen = resolve
            })
          }
          return { ok: true, data: instance(payload.page_id) }
        case 'close':
          state.closed.push({ root, id: payload.instance_id })
          return { ok: true, data: {} }
        case 'select_target':
          return { ok: true, data: {} }
        case 'message':
          return {
            ok: true,
            data: {
              bridge_version: 1,
              request_id: payload.message.request_id,
              result: report(payload.instance_id),
            },
          }
        case 'review':
          if (payload.instance_id === 'A') {
            state.reviewPending = true
            await new Promise<void>((resolve) => {
              state.releaseReview = resolve
            })
          }
          return {
            ok: true,
            data: {
              ...report(payload.instance_id),
              human_acceptance: { accepted: payload.accepted },
            },
          }
        case 'heartbeat':
          if (payload.instance_id === 'A') {
            state.heartbeatPending = true
            await new Promise<void>((resolve) => {
              state.releaseHeartbeat = resolve
            })
            return { ok: false, error: 'Old A lease expired' }
          }
          return { ok: true, data: { valid: true } }
        default:
          throw Error(`Unhandled fixture operation ${payload.operation}`)
      }
    }
  })
  await page.getByRole('button', { name: 'Settings', exact: true }).first().click()
  await page.getByRole('button', { name: 'Addon Pages', exact: true }).click()
  await expect(page.getByRole('button', { name: 'Inspector A', exact: true })).toBeVisible()
}
async function fixtureValue(page: Page, field: string) {
  return page.evaluate(async (name) => {
    const path = '/src/core/index.ts'
    return (await import(path)).gateway.pageOwnershipTest[name]
  }, field)
}

test('a late open releases its original workspace lease without replacing the new page', async ({
  page,
}) => {
  await setup(page)
  await page.evaluate(async () => {
    const path = '/src/core/index.ts'
    ;(await import(path)).gateway.pageOwnershipTest.delayOpen = true
  })
  await page.getByRole('button', { name: 'Open addon page', exact: true }).click()
  await expect.poll(() => fixtureValue(page, 'openPending')).toBe(true)
  await page.evaluate(async () => {
    const path = '/src/stores/workspace.ts'
    await (await import(path)).useWorkspaceStore().open('D:/metteur-demo/addon-b')
  })
  await expect(page.getByRole('button', { name: 'Inspector B', exact: true })).toBeVisible()
  await page.getByRole('button', { name: 'Open addon page', exact: true }).click()
  await expect(page.locator('iframe[title="Inspector B"]')).toBeVisible()
  await page.evaluate(async () => {
    const path = '/src/core/index.ts'
    ;(await import(path)).gateway.pageOwnershipTest.releaseOpen()
  })
  await expect
    .poll(() => fixtureValue(page, 'closed'))
    .toContainEqual({ root: 'D:/metteur-demo/addon-a', id: 'A' })
  await expect(page.locator('iframe[title="Inspector B"]')).toBeVisible()
})

test('old review and heartbeat replies cannot accept or revoke a replacement page', async ({
  page,
}) => {
  await setup(page)
  await page.getByRole('button', { name: 'Open addon page', exact: true }).click()
  await page.getByLabel('Preview target', { exact: true }).selectOption('preview')
  await expect(page.getByRole('heading', { name: 'Your Review', exact: true })).toBeVisible()
  await page.getByRole('checkbox', { name: 'I reviewed this revision and its evidence' }).check()
  await page.getByRole('button', { name: 'Accept this revision', exact: true }).click()
  await expect.poll(() => fixtureValue(page, 'reviewPending')).toBe(true)
  await expect.poll(() => fixtureValue(page, 'heartbeatPending')).toBe(true)
  await page.getByRole('button', { name: 'Inspector B', exact: true }).click()
  await page.getByRole('button', { name: 'Open addon page', exact: true }).click()
  await page.getByLabel('Preview target', { exact: true }).selectOption('preview')
  await expect(page.getByRole('heading', { name: 'Your Review', exact: true })).toBeVisible()
  await page.evaluate(async () => {
    const path = '/src/core/index.ts',
      fixture = (await import(path)).gateway.pageOwnershipTest
    fixture.releaseReview()
    fixture.releaseHeartbeat()
  })
  await expect(page.locator('iframe[title="Inspector B"]')).toBeVisible()
  await expect(page.getByText('Accepted for this revision', { exact: true })).toHaveCount(0)
  await expect(
    page.getByRole('button', { name: 'Accept this revision', exact: true }),
  ).toBeDisabled()
  await expect(page.getByRole('alert')).toHaveCount(0)
})
