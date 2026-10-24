import { test, expect, type Page } from '@playwright/test'

// L2 transport fixture only. RocksDB durability has separate native tests.
function host() {
  const records = new Map<string, any>(),
    receipts = new Map<string, { fingerprint: string; result: any }>()
  const handle = async ({ payload, expected, request }: any) => {
    const action = payload.action,
      key = action.id ?? action.input?.id
    const fingerprint = JSON.stringify({ payload, expected })
    if (receipts.has(request)) {
      const previous = receipts.get(request)!
      return previous.fingerprint === fingerprint
        ? previous.result
        : {
            ok: false,
            error: JSON.stringify({
              code: 'request_conflict',
              message: 'Different request content',
            }),
          }
    }
    if (action.operation === 'latest_view') return { ok: true, data: { record: null } }
    if (action.operation === 'list')
      return {
        ok: true,
        data: {
          items: [...records.values()].map((row) => ({
            id: row.input.id,
            window_id: row.input.window_id,
            kind: row.input.kind,
            entity: row.input.entity,
            revision: row.revision,
            edit_sequence: row.input.edit_sequence,
            updated_ms: 1,
            discarded: row.discarded,
            error: null,
          })),
          next: null,
        },
      }
    if (action.operation === 'read')
      return {
        ok: true,
        data: {
          record: records.get(key),
          source_available: true,
          requires_current_validation: true,
        },
      }
    const current = records.get(key)
    if ((current?.revision ?? 0) !== expected)
      return {
        ok: false,
        error: JSON.stringify({ code: 'revision_conflict', message: 'Changed recovery revision' }),
      }
    if (current?.discarded)
      return { ok: false, error: JSON.stringify({ code: 'discarded', message: 'Discarded' }) }
    const record =
      action.operation === 'discard'
        ? {
            ...current,
            revision: expected + 1,
            discarded: true,
            input: { ...current.input, raw: '' },
          }
        : {
            schema_version: 1,
            revision: expected + 1,
            input: structuredClone(action.input),
            discarded: false,
            updated_ms: 1,
          }
    records.set(key, record)
    const result = { ok: true, data: record }
    receipts.set(request, { fingerprint, result })
    return result
  }
  return { records, handle }
}
async function install(page: Page, handler: (value: any) => Promise<any>) {
  await page.exposeFunction('dwRecoveryTransport', handler)
  await page.addInitScript(() => {
    const core = '/src/core/index.ts'
    ;(window as any).dwRecoveryReady = import(core).then(({ gateway }) => {
      gateway.connected.value = true // This test supplies an available L2 recovery transport.
      const original = gateway.resourceRequest.bind(gateway)
      gateway.resourceRequest = async (
        method: string,
        workspace: string,
        payload: unknown,
        expected = 0,
        request = crypto.randomUUID(),
      ) =>
        method === 'recovery'
          ? (window as any).dwRecoveryTransport({ workspace, payload, expected, request })
          : original(method, workspace, payload, expected, request)
    })
  })
}
async function open(page: Page) {
  await page.goto('/')
  await page.evaluate(() => (window as any).dwRecoveryReady)
  if (await page.getByLabel('Workspace path').count()) {
    await page.getByLabel('Workspace path').fill('D:/metteur-demo/metrics')
    await page.getByRole('button', { name: 'Open', exact: true }).click()
  }
  await page.getByTestId('file-tree').waitFor()
  await page.getByTestId('file-tree').getByRole('button', { name: 'metrics', exact: true }).click()
  await page
    .getByTestId('file-tree')
    .getByRole('button', { name: 'README.md', exact: true })
    .click()
  await expect(page.locator('.monaco-editor .view-lines')).toContainText('# Metrics')
}
async function edit(page: Page, value: string) {
  await page.locator('.monaco-editor .view-lines').first().click()
  await page.keyboard.press('ControlOrMeta+a')
  await page.keyboard.insertText(value)
  await expect(page.locator('.monaco-editor .view-lines')).toContainText(value)
  if (await page.getByRole('button', { name: 'Back Up', exact: true }).isVisible())
    await page.getByRole('button', { name: 'Back Up', exact: true }).click()
  await expect(page.getByText('Backed Up on Host', { exact: true })).toBeVisible()
}

test('L2 confirmed raw draft can be reviewed after reload without overwriting the saved file', async ({
  page,
}) => {
  const fixture = host()
  await install(page, fixture.handle)
  await open(page)
  await edit(page, 'raw incomplete draft 中文')
  expect([...fixture.records.values()].find((row) => row.input.kind === 'text').input.raw).toBe(
    'raw incomplete draft 中文',
  )
  page.once('dialog', (dialog) => dialog.accept())
  await page.reload()
  await page.evaluate(() => (window as any).dwRecoveryReady)
  await page.getByTestId('file-tree').getByRole('button', { name: 'metrics', exact: true }).click()
  await page
    .getByTestId('file-tree')
    .getByRole('button', { name: 'README.md', exact: true })
    .click()
  await page.getByRole('button', { name: 'Recovery Copies', exact: true }).click()
  await page.getByRole('button', { name: 'Review Copy', exact: true }).first().click()
  await expect(page.getByRole('dialog').locator('pre')).toContainText('raw incomplete draft 中文')
  await expect(page.locator('.monaco-editor .view-lines')).toContainText('# Metrics')
  await page.getByRole('button', { name: 'Use This Copy', exact: true }).click()
  await expect(page.locator('.monaco-editor .view-lines')).toContainText(
    'raw incomplete draft 中文',
  )
  const disk = await page.evaluate(async () => {
    const core = '/src/core/index.ts'
    return (await import(core)).gateway.readFile('D:/metteur-demo/metrics', 'metrics/README.md')
  })
  expect(disk.ok && disk.data.content).toContain('# Metrics')
})

test('L2 two windows keep independent recovery copies and explicit discard fences one copy', async ({
  page,
  context,
}) => {
  const fixture = host()
  await install(page, fixture.handle)
  await open(page)
  await edit(page, 'window one')
  const other = await context.newPage()
  await install(other, fixture.handle)
  await open(other)
  await edit(other, 'window two')
  const rows = [...fixture.records.values()].filter((row) => row.input.kind === 'text')
  expect(rows).toHaveLength(2)
  expect(rows[0].input.window_id).not.toBe(rows[1].input.window_id)
  await page.getByText('Draft Options', { exact: true }).click()
  await page.getByRole('button', { name: 'Discard Draft', exact: true }).click()
  await page
    .getByRole('dialog')
    .getByRole('button', { name: 'Discard This Copy', exact: true })
    .click()
  await expect(page.locator('.monaco-editor .view-lines')).toContainText('# Metrics')
  expect([...fixture.records.values()].filter((r) => r.discarded)).toHaveLength(1)
  expect(
    [...fixture.records.values()].find((r) => !r.discarded && r.input.kind === 'text').input.raw,
  ).toBe('window two')
  await other.close()
})

test('L2 backup rejection keeps current input and never shows a protected state', async ({
  page,
}) => {
  await install(page, async () => ({
    ok: false,
    error: JSON.stringify({
      code: 'scope_capacity',
      message: 'Recovery capacity reached. Existing copies retained.',
    }),
  }))
  await open(page)
  await page.locator('.monaco-editor .view-lines').first().click()
  await page.keyboard.press('ControlOrMeta+a')
  await page.keyboard.insertText('keep on capacity rejection')
  await page.getByRole('button', { name: 'Back Up', exact: true }).click()
  await expect(page.getByRole('status')).toContainText('Storage Full')
  await expect(page.getByText('Backed Up on Host', { exact: true })).toHaveCount(0)
  await expect(page.locator('.monaco-editor .view-lines')).toContainText(
    'keep on capacity rejection',
  )
})
