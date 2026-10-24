import { test, expect, type Page } from '@playwright/test'

async function open(page: Page) {
  await page.goto('/')
  await page.getByLabel('Workspace path').fill('D:/metteur-demo/metrics')
  await page.getByRole('button', { name: 'Open', exact: true }).click()
  await expect(page.getByTestId('file-tree')).toBeVisible()
  await page.getByTestId('file-tree').getByRole('button', { name: 'metrics', exact: true }).click()
}
async function readme(page: Page) {
  await page
    .getByTestId('file-tree')
    .getByRole('button', { name: 'README.md', exact: true })
    .click()
  await expect(page.locator('.monaco-editor .view-lines')).toContainText('# Metrics')
}
async function replace(page: Page, text: string) {
  await page.locator('.monaco-editor .view-lines').first().click()
  await page.keyboard.press('ControlOrMeta+a')
  await page.keyboard.type(text)
  await expect(page.locator('.monaco-editor .view-lines').first()).toContainText(text)
}

test('unsaved buffers and undo survive file and surface switches', async ({ page }) => {
  await open(page)
  await readme(page)
  await replace(page, 'unsaved-buffer-must-survive')
  await page.getByRole('button', { name: 'Chat', exact: true }).first().click()
  await page.getByTestId('tab').filter({ hasText: 'README.md' }).click()
  await expect(page.locator('.monaco-editor .view-lines')).toContainText(
    'unsaved-buffer-must-survive',
  )
  await page.getByTestId('file-tree').getByRole('button', { name: 'src', exact: true }).click()
  await page.getByTestId('file-tree').getByRole('button', { name: 'api.ts', exact: true }).click()
  await page.getByTestId('tab').filter({ hasText: 'README.md' }).click()
  await expect(page.locator('.monaco-editor .view-lines')).toContainText(
    'unsaved-buffer-must-survive',
  )
  await page.getByRole('button', { name: 'Undo', exact: true }).click()
  await expect(page.locator('.monaco-editor .view-lines')).toContainText('# Metrics')
})

test('closing an unsaved document requires an explicit discard', async ({ page }) => {
  await open(page)
  await readme(page)
  await replace(page, 'keep-my-draft')
  await page.getByTestId('tab-active').getByLabel('Close tab').click()
  await expect(page.getByRole('dialog')).toContainText('Discard Unsaved Changes?')
  await page.getByRole('button', { name: 'Keep Editing', exact: true }).click()
  await expect(page.locator('.monaco-editor .view-lines')).toContainText('keep-my-draft')
})

test('deleted files keep their buffer, mark the tab and can be restored', async ({ page }) => {
  await open(page)
  await readme(page)
  await replace(page, 'restore-this-deleted-buffer')
  await page
    .getByTestId('file-tree')
    .getByRole('button', { name: 'README.md', exact: true })
    .click({ button: 'right' })
  await page.getByText('Delete', { exact: true }).click()
  await page.getByRole('dialog').getByRole('button', { name: 'Delete', exact: true }).click()
  await expect(page.getByTestId('tab-active').locator('.line-through')).toHaveText('README.md')
  await expect(page.locator('.monaco-editor .view-lines')).toContainText(
    'restore-this-deleted-buffer',
  )
  await page.getByRole('button', { name: 'Save to Restore', exact: true }).click()
  await expect(page.getByTestId('tab-active').locator('.line-through')).toHaveCount(0)
  await page.getByTestId('file-tree').getByRole('button', { name: 'Refresh', exact: true }).click()
  await expect(
    page.getByTestId('file-tree').getByRole('button', { name: 'README.md', exact: true }),
  ).toBeVisible()
})

test('inline creation receives focus, cancels cleanly and retains errors', async ({ page }) => {
  await open(page)
  const tree = page.getByTestId('file-tree')
  await tree.getByRole('button', { name: 'New File', exact: true }).click()
  const name = page.getByRole('textbox', { name: 'New File Name', exact: true })
  await expect(name).toBeFocused()
  await name.fill('metrics')
  await name.press('Enter')
  await expect(tree.getByRole('alert')).toContainText('already exists')
  await expect(name).toHaveValue('metrics')
  await name.press('Escape')
  await expect(name).toHaveCount(0)
  await tree.getByRole('button', { name: 'New File', exact: true }).click()
  await name.fill('created-from-explorer.txt')
  await name.press('Enter')
  await expect(page.getByTestId('tab-active')).toContainText('created-from-explorer.txt')
})

test('a save acknowledges only its own snapshot, preserving newer edits and failures', async ({
  page,
}) => {
  await open(page)
  const result = await page.evaluate(async () => {
    const stores = '/src/stores/documents.ts',
      core = '/src/core/index.ts'
    const { useDocumentsStore } = await import(stores)
    const { gateway } = await import(core)
    const docs = useDocumentsStore(),
      ws = 'D:/metteur-demo/metrics',
      path = 'metrics/README.md'
    await docs.open(ws, path)
    docs.edit(ws, path, 'saved-snapshot')
    const original = gateway.writeFile
    let acknowledge: (() => void) | undefined
    const gate = new Promise<void>((resolve) => {
      acknowledge = resolve
    })
    gateway.writeFile = async () => {
      await gate
      return { ok: true, data: undefined }
    }
    const save = docs.save(ws, path)
    docs.edit(ws, path, 'newer-unsaved-snapshot')
    acknowledge!()
    await save
    const doc = docs.get(ws, path)
    const saved = { content: doc.content, baseline: doc.baseline, dirty: docs.dirty(doc) }
    gateway.writeFile = async () => ({ ok: false, error: 'simulated disk full' })
    await docs.save(ws, path)
    const failed = { content: doc.content, baseline: doc.baseline, error: doc.error }
    gateway.writeFile = original
    return { saved, failed }
  })
  expect(result.saved).toEqual({
    content: 'newer-unsaved-snapshot',
    baseline: 'saved-snapshot',
    dirty: true,
  })
  expect(result.failed).toEqual({
    content: 'newer-unsaved-snapshot',
    baseline: 'saved-snapshot',
    error: 'simulated disk full',
  })
})

test('idle tasks perform no polling and schedules share their navigation entry', async ({
  page,
}) => {
  await open(page)
  await page.getByRole('button', { name: 'Tasks', exact: true }).first().click()
  await expect(page.getByRole('heading', { level: 1, name: 'Tasks', exact: true })).toBeVisible()
  await page.evaluate(async () => {
    const path = '/src/core/index.ts'
    const { gateway } = await import(path)
    const original = gateway.resourceRequest.bind(gateway)
    const counts = { reads: 0 }
    ;(window as unknown as { taskReadCounts: typeof counts }).taskReadCounts = counts
    gateway.resourceRequest = async (...args: Parameters<typeof original>) => {
      if (args[0] === 'queryContinuations') counts.reads++
      return original(...args)
    }
  })
  await page.clock.install()
  await page.clock.runFor(7000)
  expect(
    await page.evaluate(
      () => (window as unknown as { taskReadCounts: { reads: number } }).taskReadCounts.reads,
    ),
  ).toBeLessThanOrEqual(1)
  await page.getByRole('tab', { name: 'Schedules', exact: true }).click()
  await expect(page).toHaveURL(/\/tasks\?view=schedules$/)
  await expect(page.locator('[data-nav-key="schedules"]')).toHaveCount(0)
  await expect(page.locator('[title*="Drag to reorder"]')).toHaveCount(0)
})

test('default model selection tracks valid workspace configuration and removed models', async ({
  page,
}) => {
  await open(page)
  await page.getByRole('button', { name: 'Chat', exact: true }).first().click()
  await page.evaluate(async () => {
    const path = '/src/stores/config.ts'
    const { useConfigStore } = await import(path)
    const config = useConfigStore()
    config.user = {
      llm: {
        default_model: 'preferred',
        models: {
          preferred: { model_id: 'synthetic-preferred', display_name: 'Preferred Model' },
          alternative: { model_id: 'synthetic-alternative' },
        },
      },
    }
    config.ws = {}
    config.loaded = true
  })
  await expect(page.getByRole('button', { name: /^Model and reasoning effort:/ })).toContainText(
    'Preferred Model',
  )
  await page.evaluate(async () => {
    const path = '/src/stores/config.ts'
    const { useConfigStore } = await import(path)
    const config = useConfigStore()
    config.user = {
      llm: {
        default_model: 'removed',
        models: {
          alternative: { model_id: 'synthetic-alternative', display_name: 'Fallback Model' },
        },
      },
    }
  })
  await expect(page.getByRole('button', { name: /^Model and reasoning effort:/ })).toContainText(
    'Fallback Model',
  )
})

test('context capacity distinguishes unknown, zero and full, and supports Escape', async ({
  page,
}) => {
  await open(page)
  await page.getByRole('button', { name: 'Chat', exact: true }).first().click()
  await page.getByRole('button', { name: 'Context: not measured', exact: true }).click()
  await expect(page.getByRole('dialog', { name: 'Context Window' })).toContainText('Not Measured')
  await page.getByRole('button', { name: 'Close Context Window' }).click()
  await page.evaluate(async () => {
    const path = '/src/stores/chat.ts'
    const { useChatStore } = await import(path)
    useChatStore().contextStats = { tokens: 0, limit: 1000, assumedLimit: false, regions: [] }
  })
  await page.getByRole('button', { name: 'Context: 0% of the window in use', exact: true }).click()
  await expect(page.getByRole('progressbar', { name: 'Context Used' })).toHaveAttribute(
    'aria-valuenow',
    '0',
  )
  await page.getByRole('button', { name: 'Close Context Window' }).press('Escape')
  await expect(page.getByRole('dialog', { name: 'Context Window' })).toHaveCount(0)
})

test('a file invalidation during save is applied after acknowledgement without replacing a newer draft', async ({
  page,
}) => {
  await open(page)
  const result = await page.evaluate(async () => {
    const core = '/src/core/index.ts',
      stores = '/src/stores/documents.ts'
    const { gateway } = await import(core),
      { useDocumentsStore } = await import(stores)
    const docs = useDocumentsStore(),
      ws = 'D:/metteur-demo/metrics',
      path = 'save-event.txt'
    let disk = 'original',
      release: () => void = () => {}
    const read = gateway.readFile,
      write = gateway.writeFile
    gateway.readFile = async () => ({ ok: true, data: { content: disk, language: 'text' } })
    gateway.writeFile = async () => {
      await new Promise<void>((resolve) => {
        release = resolve
      })
      return { ok: true, data: undefined }
    }
    try {
      await docs.open(ws, path)
      docs.edit(ws, path, 'saving snapshot')
      const pending = docs.save(ws, path)
      docs.edit(ws, path, 'newer local draft')
      disk = 'newer external edit'
      await docs.sync(ws, path)
      release()
      await pending
      await new Promise((resolve) => setTimeout(resolve, 0))
      const doc = docs.get(ws, path)
      return {
        content: doc.content,
        baseline: doc.baseline,
        disk: doc.disk,
        conflict: doc.conflict,
      }
    } finally {
      gateway.readFile = read
      gateway.writeFile = write
    }
  })
  expect(result).toEqual({
    content: 'newer local draft',
    baseline: 'saving snapshot',
    disk: 'newer external edit',
    conflict: true,
  })
})

test('prompt metadata changes require review and a save binds the reviewed revision', async ({
  page,
}) => {
  await open(page)
  const result = await page.evaluate(async () => {
    const core = '/src/core/index.ts',
      stores = '/src/stores/documents.ts'
    const { gateway } = await import(core),
      { useDocumentsStore } = await import(stores)
    const docs = useDocumentsStore(),
      ws = 'D:/metteur-demo/metrics',
      path = '@prompt/project/prompt-cas.md'
    const entry = (revision: number) => ({
      reference: {
        scope: { scope: 'project', owner: ws },
        id: 'prompt-cas',
        revision,
        content_digest: `revision-${revision}`,
      },
      content: {
        header: { schema_version: 1, id: 'prompt-cas', name: `Prompt ${revision}`, tags: [] },
        body: 'unchanged body',
        raw_digest: 'raw',
        body_digest: 'body',
        deleted: false,
        copied_from: null,
      },
    })
    let source = entry(1),
      writtenRevision = 0
    const request = gateway.resourceRequest
    gateway.resourceRequest = async (method, _ws, payload, revision = 0) => {
      if (method === 'queryPromptResources') return { ok: true, data: source }
      if (method === 'writePromptResource') {
        writtenRevision = revision
        if (revision !== source.reference.revision) return { ok: false, error: 'revision_conflict' }
        if ('schema_version' in payload.draft) throw Error('Unexpected metadata in draft')
        source = entry(revision + 1)
        source.content.body = payload.draft.body
        return { ok: true, data: source }
      }
      return request(method, _ws, payload, revision)
    }
    try {
      await docs.open(ws, path)
      docs.edit(ws, path, 'keep my edited body')
      source = entry(2)
      await docs.sync(ws, path)
      const conflict = docs.get(ws, path).conflict
      const rejected = await docs.save(ws, path)
      const obsolete = { ...source.reference }
      source = entry(3)
      await docs.sync(ws, path)
      const staleReview = docs.adoptReviewedResource(ws, path, obsolete)
      const reviewed = docs.adoptReviewedResource(ws, path, { ...source.reference })
      const saved = await docs.save(ws, path)
      return {
        conflict,
        rejected,
        staleReview,
        reviewed,
        saved,
        writtenRevision,
        content: source.content.body,
      }
    } finally {
      gateway.resourceRequest = request
    }
  })
  expect(result).toEqual({
    conflict: true,
    rejected: false,
    staleReview: false,
    reviewed: true,
    saved: true,
    writtenRevision: 3,
    content: 'keep my edited body',
  })
})
