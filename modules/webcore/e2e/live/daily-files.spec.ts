import { test, expect } from '@playwright/test'
import { createClient } from '@connectrpc/connect'
import { createGrpcWebTransport } from '@connectrpc/connect-web'
import { readFile, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { Daemon } from '../../src/gen/metteur_pb'

test('DW04 production file workflow keeps a dirty draft, sends a frozen saved version and conditions conflict resolution', async ({
  page,
  request,
}) => {
  test.setTimeout(100_000)
  const ws = process.env.LIVE_WORKSPACE!,
    nonce = process.env.LIVE_NONCE!,
    fixture = process.env.LIVE_FIXTURE_URL!
  const client = createClient(
    Daemon,
    createGrpcWebTransport({ baseUrl: process.env.LIVE_BASE_URL! }),
  )
  const original = '\uFEFFDW04_SAVED_VERSION 中文\r\n',
    local = 'DW04_LOCAL_DRAFT 中文\r\n'
  await client.writeFile({
    workspacePath: ws,
    path: 'workflow.txt',
    content: original,
    createOnly: true,
  })
  const errors: string[] = []
  page.on('pageerror', (error) => errors.push(error.message))
  await page.goto(`/?open=${encodeURIComponent(ws)}`)
  const tree = page.getByTestId('file-tree')
  await expect(tree).toBeVisible()
  await tree.getByRole('button', { name: 'workflow.txt', exact: true }).click()
  await expect(page.locator('.monaco-editor .view-lines')).toContainText('DW04_SAVED_VERSION')
  await page.locator('.monaco-editor .view-lines').first().click()
  await page.keyboard.press('ControlOrMeta+a')
  await page.keyboard.insertText(local)
  await expect(page.getByText('Backed Up on Host', { exact: true })).toBeVisible()
  await page.getByRole('button', { name: 'Chat', exact: true }).first().click()
  const input = page.getByRole('textbox', { name: 'Message' })
  await expect(input).toBeEnabled()
  await tree.getByRole('button', { name: 'workflow.txt', exact: true }).click({ button: 'right' })
  await page.getByText('Add to Conversation', { exact: true }).click()
  const reference = page.getByRole('dialog', { name: 'Reference File', exact: true })
  await expect(
    reference.getByRole('button', { name: 'Use This Saved Version', exact: true }),
  ).toBeEnabled()
  await reference.getByText('Saved Version & Content', { exact: true }).click()
  await expect(reference).toContainText('DW04_SAVED_VERSION')
  await reference.getByRole('button', { name: 'Use This Saved Version', exact: true }).click()
  await writeFile(join(ws, 'workflow.txt'), `DW04_CURRENT_DISK ${nonce}\r\n`)
  await input.fill(
    'DW04_FILE_WORKFLOW: compare the selected saved version with a current ReadFile of workflow.txt.',
  )
  await input.press('Enter')
  await expect(page.getByRole('button', { name: 'Copy answer' }).first()).toBeVisible({
    timeout: 40_000,
  })
  await expect(page.locator('.md-body').filter({ hasText: 'DW04_FILE_READY' })).toContainText(nonce)
  const proof = await (await request.get(`${fixture}/fixture`)).json()
  expect(proof.fileReferenceRequests).toBe(2)
  expect(proof.sawFrozenFile).toBe(true)
  expect(proof.sawCurrentFileRead).toBe(true)
  await page.getByTestId('tab').filter({ hasText: 'workflow.txt' }).click()
  await expect(page.getByRole('button', { name: 'Compare Changes', exact: true })).toBeVisible()
  await page.getByRole('button', { name: 'Compare Changes', exact: true }).click()
  const comparison = page.getByRole('dialog', { name: 'Compare Changes', exact: true })
  await expect(comparison).toContainText('DW04_CURRENT_DISK')
  await writeFile(join(ws, 'workflow.txt'), `DW04_CHANGED_AGAIN ${nonce}\r\n`)
  await comparison.getByRole('button', { name: 'Save Reviewed Draft', exact: true }).click()
  await expect(comparison.getByRole('alert')).toBeVisible()
  expect(await readFile(join(ws, 'workflow.txt'), 'utf8')).toContain('DW04_CHANGED_AGAIN')
  await comparison.getByRole('button', { name: 'Review Latest', exact: true }).click()
  await expect(comparison).toContainText('DW04_CHANGED_AGAIN')
  await comparison.getByRole('button', { name: 'Keep Both', exact: true }).click()
  await comparison.getByLabel('New file path').fill('app.ts')
  await comparison.getByRole('button', { name: 'Save New Copy', exact: true }).click()
  await expect(comparison.getByRole('alert')).toContainText('exists')
  await comparison.getByLabel('New file path').fill('draft-copy.txt')
  await comparison.getByRole('button', { name: 'Save New Copy', exact: true }).click()
  await expect(comparison).toHaveCount(0)
  expect(await readFile(join(ws, 'draft-copy.txt'), 'utf8')).toContain('DW04_LOCAL_DRAFT')
  expect(await readFile(join(ws, 'workflow.txt'), 'utf8')).toContain('DW04_CHANGED_AGAIN')
  await page.getByRole('button', { name: 'Compare Changes', exact: true }).click()
  await comparison.getByRole('button', { name: 'Save Reviewed Draft', exact: true }).click()
  await expect(comparison).toHaveCount(0)
  expect(await readFile(join(ws, 'workflow.txt'), 'utf8')).toContain('DW04_LOCAL_DRAFT')
  const snapshots = await client.listSnapshots({ workspacePath: ws })
  expect(snapshots.snapshots.some((s) => s.description.includes('Before resolving conflict'))).toBe(
    true,
  )
  await page.reload()
  await page.getByRole('button', { name: 'Chat', exact: true }).first().click()
  await expect(page.locator('.md-body').filter({ hasText: 'DW04_FILE_READY' })).toContainText(nonce)
  expect((await (await request.get(`${fixture}/fixture`)).json()).calls).toBe(proof.calls)
  expect(errors).toEqual([])
})
