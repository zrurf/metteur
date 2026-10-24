import { test, expect, type Page } from '@playwright/test'

const sampleWorkspace = 'D:/metteur-demo/metrics',
  samplePath = 'metrics/README.md'
async function open(page: Page, useCoarse = false) {
  await page.addInitScript(
    ({ coarse }) => {
      if (coarse) {
        const original = window.matchMedia.bind(window)
        window.matchMedia = (query) =>
          query === '(pointer: coarse)'
            ? ({ ...original(query), matches: true } as MediaQueryList)
            : original(query)
      }
      const core = '/src/core/index.ts'
      ;(window as any).dwFilesReady = import(core).then(({ gateway }) => {
        gateway.connected.value = true
        const original = gateway.resourceRequest.bind(gateway),
          rows = new Map<string, any>()
        ;(window as any).dwFileRows = rows
        gateway.resourceRequest = async (
          method: string,
          workspace: string,
          payload: any,
          expected = 0,
        ) => {
          if (method !== 'recovery') return original(method, workspace, payload, expected)
          const action = payload.action
          if (action.operation === 'latest_view') return { ok: true, data: { record: null } }
          if (action.operation === 'list') return { ok: true, data: { items: [], next: null } }
          if (action.operation === 'read')
            return { ok: true, data: { record: rows.get(action.id), source_available: true } }
          const id = action.id ?? action.input?.id,
            previous = rows.get(id)
          if ((previous?.revision ?? 0) !== expected)
            return { ok: false, error: 'revision_conflict' }
          const record = {
            schema_version: 1,
            revision: expected + 1,
            discarded: action.operation === 'discard',
            input: action.input ?? previous?.input,
            updated_ms: 1,
          }
          rows.set(id, record)
          return { ok: true, data: record }
        }
      })
    },
    { coarse: useCoarse },
  )
  await page.goto('/')
  await page.evaluate(() => (window as any).dwFilesReady)
  await page.getByLabel('Workspace path').fill(sampleWorkspace)
  await page.getByRole('button', { name: 'Open', exact: true }).click()
  await expect(page.getByTestId('file-tree')).toBeVisible()
  await page.getByTestId('file-tree').getByRole('button', { name: 'metrics', exact: true }).click()
  await page
    .getByTestId('file-tree')
    .getByRole('button', { name: 'README.md', exact: true })
    .click()
  await expect(page.locator(useCoarse ? '.cm-editor' : '.monaco-editor').first()).toBeVisible()
}
async function external(page: Page, externalBody: string) {
  await page.evaluate(
    async ({ ws, path, body }) => {
      const c = '/src/core/index.ts',
        d = '/src/stores/documents.ts'
      const result = await (await import(c)).gateway.writeFile(ws, path, body)
      if (!result.ok) throw Error(result.error)
      await (await import(d)).useDocumentsStore().sync(ws, path)
    },
    { ws: sampleWorkspace, path: samplePath, body: externalBody },
  )
}
async function edit(page: Page, body: string) {
  await page.locator('.monaco-editor .view-lines').first().click()
  await page.keyboard.press('ControlOrMeta+a')
  await page.keyboard.insertText(body)
}

test('review keeps baseline and latest, refuses a second disk change, and Keep Both never replaces a file', async ({
  page,
}) => {
  await open(page)
  await edit(page, 'my draft 中文')
  await external(page, 'external one')
  await page.getByRole('button', { name: 'Compare Changes', exact: true }).click()
  const dialog = page.getByRole('dialog', { name: 'Compare Changes', exact: true })
  await expect(dialog).toContainText('external one')
  await dialog.getByRole('button', { name: 'Common Baseline', exact: true }).click()
  await expect(dialog).toContainText('# Metrics')
  await external(page, 'external two')
  await dialog.getByRole('button', { name: 'Save Reviewed Draft', exact: true }).click()
  await expect(dialog.getByRole('alert')).toContainText('changed again')
  await dialog.getByRole('button', { name: 'Review Latest', exact: true }).click()
  await expect(dialog).toContainText('external two')
  await dialog.getByRole('button', { name: 'Keep Both', exact: true }).click()
  await dialog.getByLabel('New file path').fill(samplePath)
  await dialog.getByRole('button', { name: 'Save New Copy', exact: true }).click()
  await expect(dialog.getByRole('alert')).toContainText('different ordinary')
  await dialog.getByLabel('New file path').fill('metrics/package.json')
  await dialog.getByRole('button', { name: 'Save New Copy', exact: true }).click()
  await expect(dialog.getByRole('alert')).toContainText('already exists')
  await dialog.getByLabel('New file path').fill('metrics/draft-copy.txt')
  await dialog.getByRole('button', { name: 'Save New Copy', exact: true }).click()
  await expect(dialog).toHaveCount(0)
  expect(
    await page.evaluate(
      async ({ ws, path }) => {
        const c = '/src/core/index.ts',
          d = '/src/stores/documents.ts',
          { gateway } = await import(c)
        return {
          original: (await gateway.readFile(ws, path)).data.content,
          copy: (await gateway.readFile(ws, 'metrics/draft-copy.txt')).data.content,
          local: (await import(d)).useDocumentsStore().get(ws, path).content,
        }
      },
      { ws: sampleWorkspace, path: samplePath },
    ),
  ).toEqual({ original: 'external two', copy: 'my draft 中文', local: 'my draft 中文' })
})

test('Use Saved Version retains a separately acknowledged raw draft and a later edit gets another identity', async ({
  page,
}) => {
  await open(page)
  await edit(page, 'keep this raw original')
  await external(page, 'saved replacement')
  await page.getByRole('button', { name: 'Compare Changes', exact: true }).click()
  await page
    .getByRole('dialog', { name: 'Compare Changes' })
    .getByRole('button', { name: 'Use Saved Version', exact: true })
    .click()
  await expect(page.getByRole('dialog', { name: 'Compare Changes' })).toHaveCount(0)
  await edit(page, 'next draft')
  const proof = await page.evaluate(
    async ({ ws, path }) => {
      const r = '/src/stores/recovery.ts'
      await (await import(r)).useRecoveryStore().flush(ws, 'text', path)
      return [...(window as any).dwFileRows.values()]
        .filter((row: any) => row.input.kind === 'text')
        .map((row: any) => ({ id: row.input.id, raw: row.input.raw, discarded: row.discarded }))
    },
    { ws: sampleWorkspace, path: samplePath },
  )
  expect(proof).toHaveLength(2)
  expect(new Set(proof.map((x) => x.id)).size).toBe(2)
  expect(proof.map((x) => [x.raw, x.discarded])).toEqual([
    ['keep this raw original', false],
    ['next draft', false],
  ])
})

test('Explorer attachment asks about the dirty file and captures the chosen saved version without inventing a message', async ({
  page,
}) => {
  await open(page)
  await edit(page, 'unsaved attachment draft')
  await page
    .getByTestId('file-tree')
    .getByRole('button', { name: 'README.md', exact: true })
    .click({ button: 'right' })
  await page.getByText('Add to Conversation', { exact: true }).click()
  const dialog = page.getByRole('dialog', { name: 'Reference File', exact: true })
  await expect(
    dialog.getByRole('button', { name: 'Use This Saved Version', exact: true }),
  ).toBeEnabled()
  await dialog.getByRole('button', { name: 'Use This Saved Version', exact: true }).click()
  await expect(dialog).toHaveCount(0)
  const result = await page.evaluate(async () => {
    const c = '/src/stores/chat.ts',
      d = '/src/stores/documents.ts'
    const chat = (await import(c)).useChatStore()
    return {
      chip: chat.pendingFiles[0],
      invented: chat.messages.some((m: any) => m.content.startsWith('@file ')),
      raw: (await import(d)).useDocumentsStore().get('D:/metteur-demo/metrics', 'metrics/README.md')
        .content,
    }
  })
  expect(result.chip.reference.digest).toMatch(/^[a-f0-9]{64}$/)
  expect(result.chip.reference.path).toBe(samplePath)
  expect(result.invented).toBe(false)
  expect(result.raw).toBe('unsaved attachment draft')
})

for (const coarse of [false, true])
  test(`UTF-8 BOM and CRLF survive ${coarse ? 'CodeMirror' : 'Monaco'} edits; mixed endings stay read only`, async ({
    page,
  }) => {
    await open(page, coarse)
    await external(page, '\uFEFFfirst\r\nsecond\r\n')
    const editor = page.locator(coarse ? '.cm-content' : '.monaco-editor .view-lines').first()
    await editor.click()
    await page.keyboard.press('ControlOrMeta+End')
    await page.keyboard.insertText('last')
    const savedText = await page.evaluate(
      async ({ ws, path }) => {
        const d = '/src/stores/documents.ts'
        const docs = (await import(d)).useDocumentsStore()
        const raw = docs.get(ws, path).content
        const saved = await docs.save(ws, path)
        return { raw, saved }
      },
      { ws: sampleWorkspace, path: samplePath },
    )
    expect(savedText).toEqual({ raw: '\uFEFFfirst\r\nsecond\r\nlast', saved: true })
    await external(page, 'one\r\ntwo\n')
    await expect(
      page.getByText('This file uses mixed or unsupported line endings.', { exact: false }),
    ).toBeVisible()
    expect(
      await page.evaluate(
        async ({ ws, path }) => {
          const d = '/src/stores/documents.ts'
          const docs = (await import(d)).useDocumentsStore()
          docs.edit(ws, path, 'normalized')
          return { saved: await docs.save(ws, path), raw: docs.get(ws, path).content }
        },
        { ws: sampleWorkspace, path: samplePath },
      ),
    ).toEqual({ saved: false, raw: 'one\r\ntwo\n' })
  })

test('rollback exposes host failure and protects drafts before restoring only the captured workspace', async ({
  page,
}) => {
  await open(page)
  await edit(page, 'must survive rollback failure')
  const proof = await page.evaluate(
    async ({ ws, path }) => {
      const c = '/src/core/index.ts',
        v = '/src/stores/version.ts',
        d = '/src/stores/documents.ts'
      const { gateway } = await import(c),
        versions = (await import(v)).useVersionStore(),
        docs = (await import(d)).useDocumentsStore()
      let calls = 0,
        protectedRaw = ''
      gateway.rollback = async (target: string) => {
        calls++
        if (target !== ws) throw Error('retargeted')
        protectedRaw = [...(window as any).dwFileRows.values()].find(
          (r: any) => r.input.kind === 'text',
        )?.input.raw
        return { ok: false, error: 'restore preflight failed' }
      }
      const wrong = await versions.rollback('old', 'foreign workspace'),
        failed = await versions.rollback('old', ws)
      return {
        wrong,
        failed,
        calls,
        protectedRaw,
        error: versions.error,
        raw: docs.get(ws, path).content,
      }
    },
    { ws: sampleWorkspace, path: samplePath },
  )
  expect(proof).toEqual({
    wrong: false,
    failed: false,
    calls: 1,
    protectedRaw: 'must survive rollback failure',
    error: 'restore preflight failed',
    raw: 'must survive rollback failure',
  })
})
