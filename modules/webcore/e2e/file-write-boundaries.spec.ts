import { test, expect, type Page } from '@playwright/test'

const workspace = 'D:/metteur-demo/metrics'
async function open(page: Page) {
  await page.goto('/')
  await page.getByLabel('Workspace path').fill(workspace)
  await page.getByRole('button', { name: 'Open', exact: true }).click()
  await expect(page.getByTestId('file-tree')).toBeVisible()
  await page.getByTestId('file-tree').getByRole('button', { name: 'metrics', exact: true }).click()
}
async function navigate(page: Page, file: string) {
  await page.evaluate(async (path) => {
    const routes = '/src/router.ts',
      files = '/src/lib/file-token.ts'
    await (await import(routes)).router.push((await import(files)).fileRoute(path))
  }, file)
}

test('folder copies refuse their own descendants and clipboard paths keep their workspace owner', async ({
  page,
}) => {
  await open(page)
  await page.evaluate(async () => {
    const c = '/src/core/index.ts',
      clip = '/src/stores/fileclip.ts'
    const { gateway } = await import(c),
      original = gateway.createDir
    gateway.copyCreates = 0
    gateway.createDir = async (...args: any[]) => {
      gateway.copyCreates++
      return original.apply(gateway, args)
    }
    ;(await import(clip)).useFileClipboardStore().set('copy', ['metrics'])
  })
  await page
    .getByTestId('file-tree')
    .getByRole('button', { name: 'src', exact: true })
    .click({ button: 'right' })
  await page.getByText('Paste', { exact: true }).click()
  await expect(page.getByText('Copy failed', { exact: true })).toBeVisible()
  const result = await page.evaluate(async () => {
    const c = '/src/core/index.ts',
      clip = '/src/stores/fileclip.ts',
      w = '/src/stores/workspace.ts'
    const { gateway } = await import(c),
      clipboard = (await import(clip)).useFileClipboardStore()
    clipboard.set('copy', ['metrics/README.md'])
    const before = clipboard.active
    await (await import(w)).useWorkspaceStore().open('D:/metteur-demo/another-project')
    return { creates: gateway.copyCreates, before, after: clipboard.active }
  })
  expect(result).toEqual({ creates: 0, before: true, after: false })
})

test('DSL compilation cannot overwrite an existing canvas or retarget a late compile result', async ({
  page,
}) => {
  await open(page)
  await page.evaluate(async () => {
    const c = '/src/core/index.ts',
      { gateway } = await import(c)
    await gateway.writeFile(
      'D:/metteur-demo/metrics',
      'metrics/review.mbp',
      'blueprint "Review"\n',
      { createOnly: true },
    )
    await gateway.writeFile(
      'D:/metteur-demo/metrics',
      'metrics/review.blueprint',
      'ORIGINAL_CANVAS_BYTES',
      { createOnly: true },
    )
  })
  await navigate(page, 'metrics/review.mbp')
  await page.getByRole('button', { name: 'Compile to blueprint', exact: true }).click()
  await expect(page.getByText('Write Blueprint Failed', { exact: true })).toBeVisible()
  expect(
    await page.evaluate(async () => {
      const c = '/src/core/index.ts'
      return (
        await (
          await import(c)
        ).gateway.readFile('D:/metteur-demo/metrics', 'metrics/review.blueprint')
      ).data.content
    }),
  ).toBe('ORIGINAL_CANVAS_BYTES')
  await page.evaluate(async () => {
    const c = '/src/core/index.ts',
      { gateway } = await import(c),
      original = gateway.writeFile
    gateway.compileBoundary = { writes: 0, started: false, finished: false, release: () => {} }
    gateway.compileDsl = async () => {
      gateway.compileBoundary.started = true
      await new Promise<void>((resolve) => {
        gateway.compileBoundary.release = resolve
      })
      gateway.compileBoundary.finished = true
      return { ok: true, data: { id: 'synthetic', name: 'Late graph', nodes: [], edges: [] } }
    }
    gateway.writeFile = async (...args: any[]) => {
      gateway.compileBoundary.writes++
      return original.apply(gateway, args)
    }
  })
  await page.getByRole('button', { name: 'Compile to blueprint', exact: true }).click()
  await navigate(page, 'metrics/README.md')
  await expect(page.getByTestId('tab-active')).toContainText('README.md')
  await page.evaluate(async () => {
    const c = '/src/core/index.ts'
    ;(await import(c)).gateway.compileBoundary.release()
  })
  await expect
    .poll(() =>
      page.evaluate(async () => {
        const c = '/src/core/index.ts'
        return (await import(c)).gateway.compileBoundary.finished
      }),
    )
    .toBe(true)
  expect(
    await page.evaluate(async () => {
      const c = '/src/core/index.ts'
      return (await import(c)).gateway.compileBoundary.writes
    }),
  ).toBe(0)
  await expect(page.getByTestId('tab-active')).toContainText('README.md')
})

test('DSL export refuses an existing file and retains the save dialog for correction', async ({
  page,
}) => {
  await open(page)
  await page.evaluate(async () => {
    const c = '/src/core/index.ts',
      { gateway } = await import(c)
    await gateway.writeFile('D:/metteur-demo/metrics', 'metrics/export.mbp', 'ORIGINAL_DSL_BYTES', {
      createOnly: true,
    })
    await gateway.writeFile('D:/metteur-demo/metrics', 'metrics/canvas.blueprint', '', {
      createOnly: true,
    })
    gateway.decompileBlueprint = async () => ({ ok: true, data: 'NEW_EXPORT_TEXT' })
  })
  await navigate(page, 'metrics/canvas.blueprint')
  await expect(page.locator('.vue-flow__node')).toHaveCount(2)
  await page.getByRole('button', { name: 'Export DSL', exact: true }).click()
  const dialog = page.getByRole('dialog', { name: 'Export Blueprint as DSL', exact: true })
  await dialog.getByRole('button', { name: 'metrics', exact: true }).click()
  await dialog.getByPlaceholder('file name (.mbp)', { exact: true }).fill('export.mbp')
  await dialog.getByRole('button', { name: 'Save', exact: true }).click()
  await expect(page.getByText('DSL export failed', { exact: true })).toBeVisible()
  await expect(dialog).toBeVisible()
  await expect(dialog.getByPlaceholder('file name (.mbp)', { exact: true })).toHaveValue(
    'export.mbp',
  )
  expect(
    await page.evaluate(async () => {
      const c = '/src/core/index.ts'
      return (
        await (await import(c)).gateway.readFile('D:/metteur-demo/metrics', 'metrics/export.mbp')
      ).data.content
    }),
  ).toBe('ORIGINAL_DSL_BYTES')
  await dialog.getByPlaceholder('file name (.mbp)', { exact: true }).press('Escape')
  await expect(dialog).toHaveCount(0)
})
