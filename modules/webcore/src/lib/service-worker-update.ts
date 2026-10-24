/** Observe both a worker already installing when registration resolves and future updates. */
export function observeServiceWorkerUpdate(
  registration: ServiceWorkerRegistration,
  ready: () => void,
  hasController = () => navigator.serviceWorker.controller !== null,
) {
  let worker: ServiceWorker | null = null
  function changed() {
    if (registration.waiting || (worker?.state === 'installed' && hasController())) ready()
  }
  function observe() {
    worker?.removeEventListener('statechange', changed)
    worker = registration.installing
    worker?.addEventListener('statechange', changed)
    changed()
  }
  registration.addEventListener('updatefound', observe)
  observe()
  return () => {
    registration.removeEventListener('updatefound', observe)
    worker?.removeEventListener('statechange', changed)
  }
}
