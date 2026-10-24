<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from 'vue'
import { X } from '@lucide/vue'
import { observeServiceWorkerUpdate } from '@/lib/service-worker-update'

let registration: ServiceWorkerRegistration | undefined
const needRefresh = ref(false)
let reloadRequested = false
let disposed = false
let stopObserving: (() => void) | undefined
let previousController: ServiceWorker | null = null

function controllerChanged() {
  const controller = navigator.serviceWorker.controller
  if (reloadRequested) window.location.reload()
  else if (previousController && controller && controller !== previousController)
    needRefresh.value = true
  previousController = controller
}

function updateServiceWorker() {
  if (registration?.waiting) {
    reloadRequested = true
    registration.waiting.postMessage({ type: 'SKIP_WAITING' })
  } else window.location.reload()
}

onMounted(async () => {
  if (!import.meta.env.PROD || !('serviceWorker' in navigator)) return
  previousController = navigator.serviceWorker.controller
  try {
    registration = await navigator.serviceWorker.register('/sw.js', { updateViaCache: 'none' })
    if (disposed) return
    navigator.serviceWorker.addEventListener('controllerchange', controllerChanged)
    stopObserving = observeServiceWorkerUpdate(registration, () => {
      if (!disposed) needRefresh.value = true
    })
    await registration.update()
  } catch {
    // Offline/PWA support is optional; it must never block opening source.
  }
})

function checkForUpdate() {
  if (document.visibilityState === 'visible') void registration?.update().catch(() => undefined)
}
document.addEventListener('visibilitychange', checkForUpdate)
onBeforeUnmount(() => {
  disposed = true
  stopObserving?.()
  document.removeEventListener('visibilitychange', checkForUpdate)
  navigator.serviceWorker?.removeEventListener('controllerchange', controllerChanged)
})
</script>

<template>
  <div
    v-if="needRefresh"
    class="fixed top-14 right-4 z-50 w-80 max-w-[calc(100vw-2rem)] rounded-xl border border-border bg-surface p-4 shadow-popover"
    role="status"
    aria-label="Application update"
  >
    <div class="flex items-start justify-between gap-3">
      <p class="text-[13px] font-medium">A new interface version is ready</p>
      <button
        class="btn-icon shrink-0"
        type="button"
        aria-label="Dismiss update notification"
        @click="needRefresh = false"
      >
        <X class="h-4 w-4" />
      </button>
    </div>
    <p class="mt-1 text-[12px] text-muted-foreground">
      Save any unsaved files, then reload to use the latest fixes.
    </p>
    <button class="btn btn-primary mt-3" type="button" @click="updateServiceWorker()">
      Reload application
    </button>
  </div>
</template>
