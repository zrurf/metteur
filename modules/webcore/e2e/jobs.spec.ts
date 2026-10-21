import { expect, test } from '@playwright/test'

test('a killed job cannot emit later output or a successful completion', async ({ page }) => {
  await page.goto('/')
  const observed = await page.evaluate(async () => {
    const modulePath = '/src/core/mock-gateway.ts'
    const { MockGateway } = await import(modulePath)
    const gateway = new MockGateway()
    const notices: Array<{ kind: string; state: string }> = []
    let started: (id: string) => void = () => {}
    const ready = new Promise<string>(resolve => { started = resolve })
    const watching = gateway.watchJobs('/demo/workspace', (event: { jobId: string; kind: string; state: string }) => {
      notices.push({ kind: event.kind, state: event.state })
      if (event.kind === 'started') started(event.jobId)
    })
    const id = await ready
    const killed = await gateway.killJob('/demo/workspace', id)
    await watching
    const jobs = await gateway.listJobs('/demo/workspace')
    return { killed, notices, job: jobs.ok ? jobs.data.find((job: { id: string }) => job.id === id) : null }
  })
  expect(observed.killed).toEqual({ ok: true, data: { killed: true, state: 'killed' } })
  expect(observed.notices).toEqual([
    { kind: 'started', state: 'running' },
    { kind: 'finished', state: 'killed' },
  ])
  expect(observed.job).toMatchObject({ state: 'killed', exitCode: -1, tail: '' })
})
