import { expect, test, type Page } from '@playwright/test'

async function openMemory(page: Page) {
  await page.goto('/')
  await page.getByLabel('Workspace path').fill('D:/metteur-demo/memory-race')
  await page.getByRole('button', { name: 'Open', exact: true }).click()
  await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeVisible()
  await page.evaluate(async () => {
    const corePath = '/src/core/index.ts', routerPath = '/src/router.ts', urlPath = '/src/lib/workspace-url.ts'
    const { gateway } = await import(corePath)
    gateway.connected.value = true
    const note = (id: string, body = `${id} body`, revision = 1) => ({
      reference: { scope: { scope: 'project', owner: 'D:/metteur-demo/memory-race' }, id, revision, content_digest: id },
      content: { title: `${id} note`, body, origin: 'user_note', validity: 'current', author: 'user', source: `run:${id}`, files: [], confirmation: null },
    })
    const fixture = gateway.memoryRace = { writes: 0, writtenId: '', writtenRevision: 0, reads: 0, hideFirst: false, finishWrite: () => {}, finishRead: () => {} }
    gateway.listExecutions = async () => ({ ok: true, data: [] })
    gateway.resourceRequest = async (method: string, _workspace: string, payload: { operation?: string; id?: string; body?: string }, revision = 0) => {
      if (method === 'queryProjectMemory' && payload.operation === 'query') return { ok: true, data: { entries: fixture.hideFirst ? [note('Second')] : [note('First'), note('Second')], next: null, truncated: false } }
      if (method === 'queryProjectMemory' && payload.operation === 'evidence') {
        fixture.reads++
        return new Promise(resolve => { fixture.finishRead = () => resolve({ ok: true, data: { entries: [{ note: 'FIRST_SOURCE_EVIDENCE' }] } }) })
      }
      if (method === 'writeProjectNote') {
        fixture.writes++; fixture.writtenId = payload.id ?? ''; fixture.writtenRevision = revision
        return new Promise(resolve => { fixture.finishWrite = () => resolve({ ok: true, data: note(payload.id!, payload.body, 2) }) })
      }
      return { ok: false, error: 'Unexpected memory fixture operation' }
    }
    await (await import(routerPath)).router.push((await import(urlPath)).wurl('/memory'))
  })
  const surface = page.getByRole('region', { name: 'Project memory', exact: true })
  await expect(surface.getByRole('heading', { name: 'First note', exact: true })).toBeVisible()
  return surface
}

test('late source evidence cannot appear under a different memory entry', async ({ page }) => {
  const surface = await openMemory(page)
  await surface.getByText('Source details', { exact: true }).click()
  await surface.getByRole('button', { name: 'run:First ↗', exact: true }).click()
  await expect.poll(() => page.evaluate(async () => (await import('/src/core/index.ts')).gateway.memoryRace.reads)).toBe(1)
  await surface.getByRole('button', { name: /Second note/ }).click()
  await page.evaluate(async () => (await import('/src/core/index.ts')).gateway.memoryRace.finishRead())
  await expect(surface.locator('.pv-source-output')).toHaveCount(0)
})

test('filtering memory entries cannot send the draft to another note', async ({ page }) => {
  const surface = await openMemory(page)
  await surface.getByLabel('More actions', { exact: true }).click()
  await surface.getByRole('button', { name: 'Edit note', exact: true }).click()
  await surface.getByLabel('Note body', { exact: true }).fill('first edited body')
  await surface.getByRole('textbox', { name: 'Search memory', exact: true }).fill('Second')
  await surface.getByRole('button', { name: 'Save new revision', exact: true }).click()
  await expect.poll(() => page.evaluate(async () => (await import('/src/core/index.ts')).gateway.memoryRace.writtenId)).toBe('First')
  await surface.getByLabel('Note body', { exact: true }).fill('newer draft for the same original note')
  await page.evaluate(async () => (await import('/src/core/index.ts')).gateway.memoryRace.finishWrite())
  await expect(surface.getByLabel('Note body', { exact: true })).toHaveValue('newer draft for the same original note')
  await surface.getByRole('button', { name: 'Save new revision', exact: true }).click()
  await expect.poll(() => page.evaluate(async () => (await import('/src/core/index.ts')).gateway.memoryRace.writtenRevision)).toBe(2)
})

test('saving a memory revision preserves text typed while the save is pending', async ({ page }) => {
  const surface = await openMemory(page)
  await surface.getByLabel('More actions', { exact: true }).click()
  await surface.getByRole('button', { name: 'Edit note', exact: true }).click()
  await surface.getByLabel('Note body', { exact: true }).fill('submitted body')
  await surface.getByRole('button', { name: 'Save new revision', exact: true }).click()
  await expect.poll(() => page.evaluate(async () => (await import('/src/core/index.ts')).gateway.memoryRace.writes)).toBe(1)
  await surface.getByLabel('Note body', { exact: true }).fill('newer unsaved body')
  await page.evaluate(async () => (await import('/src/core/index.ts')).gateway.memoryRace.finishWrite())
  await expect(surface.getByLabel('Note body', { exact: true })).toHaveValue('newer unsaved body')
})

test('reloading a page that no longer contains the edited note preserves its draft', async ({ page }) => {
  const surface = await openMemory(page)
  await surface.getByLabel('More actions', { exact: true }).click()
  await surface.getByRole('button', { name: 'Edit note', exact: true }).click()
  await surface.getByLabel('Note body', { exact: true }).fill('keep the original note draft')
  await page.evaluate(async () => { (await import('/src/core/index.ts')).gateway.memoryRace.hideFirst = true })
  await surface.getByRole('button', { name: 'Reload', exact: true }).click()
  await expect(surface.getByLabel('Note body', { exact: true })).toHaveValue('keep the original note draft')
  await surface.getByRole('button', { name: 'Save new revision', exact: true }).click()
  await expect.poll(() => page.evaluate(async () => (await import('/src/core/index.ts')).gateway.memoryRace.writtenId)).toBe('First')
  await surface.getByLabel('Note body', { exact: true }).fill('newer off-page draft')
  await page.evaluate(async () => (await import('/src/core/index.ts')).gateway.memoryRace.finishWrite())
  await expect(surface.getByLabel('Note body', { exact: true })).toHaveValue('newer off-page draft')
  await surface.getByRole('button', { name: 'Save new revision', exact: true }).click()
  await expect.poll(() => page.evaluate(async () => (await import('/src/core/index.ts')).gateway.memoryRace.writtenRevision)).toBe(2)
})
