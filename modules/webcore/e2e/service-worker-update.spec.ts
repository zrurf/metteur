import { expect, test } from '@playwright/test'

test('an update already installing at registration is announced and listeners are disposed', async ({
  page,
}) => {
  await page.goto('/')
  const result = await page.evaluate(async () => {
    const path = '/src/lib/service-worker-update.ts'
    const { observeServiceWorkerUpdate } = await import(path)
    const worker = Object.assign(new EventTarget(), { state: 'installing' })
    const registration = Object.assign(new EventTarget(), { installing: worker, waiting: null })
    let notices = 0
    const stop = observeServiceWorkerUpdate(
      registration as unknown as ServiceWorkerRegistration,
      () => notices++,
      () => true,
    )
    worker.state = 'installed'
    worker.dispatchEvent(new Event('statechange'))
    const announced = notices
    stop()
    worker.dispatchEvent(new Event('statechange'))
    registration.dispatchEvent(new Event('updatefound'))
    const disposed = notices
    registration.waiting = worker
    const stopWaiting = observeServiceWorkerUpdate(
      registration as unknown as ServiceWorkerRegistration,
      () => notices++,
      () => true,
    )
    stopWaiting()
    return { announced, disposed, waiting: notices }
  })
  expect(result).toEqual({ announced: 1, disposed: 1, waiting: 2 })
})
