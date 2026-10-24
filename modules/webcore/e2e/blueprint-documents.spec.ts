import { test, expect, type Page } from '@playwright/test'

async function open(page: Page) {
  await page.goto('/')
  await page.getByLabel('Workspace path').fill('D:/metteur-demo/metrics')
  await page.getByRole('button', { name: 'Open', exact: true }).click()
  await expect(page.getByTestId('file-tree')).toBeVisible()
  await page.evaluate(async () => {
    const core = '/src/core/index.ts'
    const { gateway } = await import(core)
    gateway.canvasDisk = new Map<string, string>()
    gateway.readFile = async (ws: string, file: string) => ({
      ok: true,
      data: { content: gateway.canvasDisk.get(JSON.stringify([ws, file])) ?? '' },
    })
    gateway.writeFile = async (
      ws: string,
      file: string,
      content: string,
      options?: { expectedContent?: string; createOnly?: boolean },
    ) => {
      const key = JSON.stringify([ws, file]),
        current = gateway.canvasDisk.get(key)
      if (options?.createOnly && current !== undefined)
        return { ok: false, error: 'already exists' }
      if (options?.expectedContent !== undefined && options.expectedContent !== (current ?? ''))
        return { ok: false, error: 'content changed on disk' }
      gateway.canvasDisk.set(key, content)
      return { ok: true, data: undefined }
    }
    gateway.saveBlueprint = async () => ({ ok: true, data: undefined })
    const route = '/src/router.ts',
      files = '/src/lib/file-token.ts'
    await (await import(route)).router.push((await import(files)).fileRoute('first.blueprint'))
  })
  await expect(page.locator('.vue-flow__node')).toHaveCount(2)
}

test('canvas edits survive file and surface switches, and explicit discard releases the draft', async ({
  page,
}) => {
  await open(page)
  await page.locator('.vue-flow__pane').click({ button: 'right', position: { x: 450, y: 180 } })
  await page.getByPlaceholder('Filter…').fill('Add')
  await page.getByRole('menuitem', { name: 'Add', exact: true }).click()
  await expect(page.locator('.vue-flow__node')).toHaveCount(3)
  await page.getByRole('button', { name: 'Chat', exact: true }).first().click()
  await page.getByTestId('tab').filter({ hasText: 'first.blueprint' }).click()
  await expect(page.locator('.vue-flow__node')).toHaveCount(3)
  await page.evaluate(async () => {
    const route = '/src/router.ts',
      files = '/src/lib/file-token.ts'
    await (await import(route)).router.push((await import(files)).fileRoute('second.blueprint'))
  })
  await expect(page.locator('.vue-flow__node')).toHaveCount(2)
  await page.getByTestId('tab').filter({ hasText: 'first.blueprint' }).click()
  await expect(page.locator('.vue-flow__node')).toHaveCount(3)
  await page.getByTestId('tab-active').getByLabel('Close tab').click()
  await expect(page.getByRole('dialog')).toContainText('Discard Unsaved Changes?')
  await page.getByRole('button', { name: 'Discard Changes', exact: true }).click()
  await page.evaluate(async () => {
    const route = '/src/router.ts',
      files = '/src/lib/file-token.ts'
    await (await import(route)).router.push((await import(files)).fileRoute('first.blueprint'))
  })
  await expect(page.locator('.vue-flow__node')).toHaveCount(2)
})

test('canvas documents isolate workspaces, follow rename, reject stale saves and restore deletions', async ({
  page,
}) => {
  await open(page)
  const result = await page.evaluate(async () => {
    const blueprints = '/src/stores/blueprint.ts',
      documents = '/src/stores/documents.ts',
      core = '/src/core/index.ts'
    const store = (await import(blueprints)).useBlueprintStore(),
      docs = (await import(documents)).useDocumentsStore(),
      { gateway } = await import(core)
    const ws = 'D:/metteur-demo/metrics',
      other = 'D:/metteur-demo/other',
      file = 'shared.blueprint'
    const first = await store.load(ws, file)
    first.nodes[0].title = 'My workspace draft'
    store.updateDraft(ws, file, first.nodes, first.edges)
    const second = await store.load(other, file)
    const isolated = second.nodes[0].title
    const again = await store.load(ws, file)
    docs.rename(ws, file, 'renamed.blueprint')
    const renamed = await store.load(ws, 'renamed.blueprint')
    const doc = docs.get(ws, 'renamed.blueprint')
    gateway.canvasDisk.set(
      JSON.stringify([ws, 'renamed.blueprint']),
      '{"external":"newer version"}',
    )
    await docs.sync(ws, 'renamed.blueprint')
    const conflicted = doc.conflict
    const staleSaved = await store.save(ws, 'renamed.blueprint')
    const preservedDisk = gateway.canvasDisk.get(JSON.stringify([ws, 'renamed.blueprint']))
    gateway.canvasDisk.delete(JSON.stringify([ws, 'renamed.blueprint']))
    docs.markDeleted(ws, 'renamed.blueprint')
    const restored = await store.save(ws, 'renamed.blueprint')
    return {
      isolated,
      returned: again.nodes[0].title,
      renamed: renamed.nodes[0].title,
      conflicted,
      staleSaved,
      preservedDisk,
      restored,
      deleted: doc.deleted,
      restoredTitle: JSON.parse(gateway.canvasDisk.get(JSON.stringify([ws, 'renamed.blueprint'])))
        .nodes[0].title,
    }
  })
  expect(result).toEqual({
    isolated: 'Start',
    returned: 'My workspace draft',
    renamed: 'My workspace draft',
    conflicted: true,
    staleSaved: false,
    preservedDisk: '{"external":"newer version"}',
    restored: true,
    deleted: false,
    restoredTitle: 'My workspace draft',
  })
})

test('a clean canvas follows external contents while a dirty canvas preserves its draft', async ({
  page,
}) => {
  await open(page)
  await page.evaluate(async () => {
    const b = '/src/stores/blueprint.ts',
      d = '/src/stores/documents.ts',
      c = '/src/core/index.ts'
    const store = (await import(b)).useBlueprintStore(),
      docs = (await import(d)).useDocumentsStore(),
      { gateway } = await import(c)
    const ws = 'D:/metteur-demo/metrics',
      file = 'first.blueprint',
      graph = store.graphFor(ws, file)
    const changed = JSON.parse(JSON.stringify(graph))
    changed.nodes[0].title = 'External revision'
    gateway.canvasDisk.set(JSON.stringify([ws, file]), JSON.stringify(changed))
    await docs.sync(ws, file)
  })
  await expect(page.locator('.metteur-node').filter({ hasText: 'External revision' })).toBeVisible()
  await page.locator('.vue-flow__pane').click({ button: 'right', position: { x: 450, y: 180 } })
  await page.getByPlaceholder('Filter…').fill('Add')
  await page.getByRole('menuitem', { name: 'Add', exact: true }).click()
  await page.evaluate(async () => {
    const d = '/src/stores/documents.ts',
      c = '/src/core/index.ts'
    const docs = (await import(d)).useDocumentsStore(),
      { gateway } = await import(c)
    const ws = 'D:/metteur-demo/metrics',
      file = 'first.blueprint',
      key = JSON.stringify([ws, file])
    const changed = JSON.parse(gateway.canvasDisk.get(key))
    changed.nodes[0].title = 'Later disk revision'
    gateway.canvasDisk.set(key, JSON.stringify(changed))
    await docs.sync(ws, file)
  })
  await expect(page.locator('.vue-flow__node')).toHaveCount(3)
  await expect(page.locator('.metteur-node').filter({ hasText: 'External revision' })).toBeVisible()
  await expect(page.getByRole('alert')).toContainText('This file changed on disk')
  await page.getByRole('button', { name: 'Save', exact: true }).click()
  await expect(page.getByTestId('blueprint-save-error')).toContainText('content changed on disk')
  await expect(page.locator('.vue-flow__node')).toHaveCount(3)
})
