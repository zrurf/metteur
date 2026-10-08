import { test, expect } from '@playwright/test'

test('execution and resume persistence errors stay failed instead of finished', async ({ page }) => {
  await page.goto('/')
  await page.getByLabel('Workspace path').fill('D:/metteur-demo/metrics')
  await page.getByRole('button', { name: 'Open', exact: true }).click()
  await expect(page.getByTestId('file-tree')).toBeVisible()
  const result = await page.evaluate(async () => {
    const corePath = '/src/core/index.ts'
    const storePath = '/src/stores/execution.ts'
    const { gateway } = await import(corePath)
    const { useExecutionStore } = await import(storePath)
    const execution = useExecutionStore()
    gateway.executeBlueprint = async () => ({ ok: false, error: 'Checkpoint failed; manual recovery required' })
    gateway.continueExecution = async () => ({ ok: false, error: 'Uncommitted outcome; replay refused' })
    await execution.run('test')
    const runStatus = execution.status
    execution.runId = 'test'
    await execution.recover()
    return { runStatus, resumeStatus: execution.status, messages: execution.events.map((e: { message: string }) => e.message) }
  })
  expect(result.runStatus).toBe('failed')
  expect(result.resumeStatus).toBe('failed')
  expect(result.messages).toContain('Checkpoint failed; manual recovery required')
  expect(result.messages).toContain('Uncommitted outcome; replay refused')
})

test('cancelled execution retains partial rollback diagnostics', async ({ page }) => {
  await page.goto('/')
  await page.getByLabel('Workspace path').fill('D:/metteur-demo/metrics')
  await page.getByRole('button', { name: 'Open', exact: true }).click()
  await expect(page.getByTestId('file-tree')).toBeVisible()
  const result = await page.evaluate(async () => {
    const corePath = '/src/core/index.ts'
    const storePath = '/src/stores/execution.ts'
    const { gateway } = await import(corePath)
    const { useExecutionStore } = await import(storePath)
    const execution = useExecutionStore()
    gateway.cancel = async () => { gateway.listExecutions = async () => ({ ok: true, data: [{ runId: 'cancelled-run', blueprintId: 'test', status: 'Cancelled' }] }); return { ok: true, data: undefined } }
    gateway.executeBlueprint = async (_path: string, _id: string, onEvent: (event: unknown) => void) => {
      onEvent({ nodeId: 'write', kind: 'started', message: '', detail: { run_id: 'cancelled-run', stream_id: 'stream', sequence: 1 } })
      await execution.cancel()
      onEvent({ nodeId: 'write', kind: 'message', message: 'cancelled: rollback incomplete: restored 1 operation(s); file conflict at user.txt; manual recovery required' })
      return { ok: false, error: 'cancelled with partial rollback' }
    }
    await execution.run('test')
    return { status: execution.status, messages: execution.events.map((e: { message: string }) => e.message) }
  })
  expect(result.status).toBe('cancelled')
  expect(result.messages.some((m: string) => m.includes('rollback incomplete') && m.includes('user.txt'))).toBe(true)
  expect(result.messages.some((m: string) => m.includes('rollback complete'))).toBe(false)
})
