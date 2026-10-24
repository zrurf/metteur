import { createApp } from 'vue'
import { createPinia } from 'pinia'
import '@/styles/main.css'
import App from '@/App.vue'
import { router } from '@/router'
import { useWorkspaceStore } from '@/stores/workspace'
import { useTabsStore } from '@/stores/tabs'
import Preview from './Preview.vue'
if (import.meta.env.VITE_MOCK !== '1')
  throw new Error('This isolated preview requires the fixture gateway.')
router.addRoute({
  path: '/src/previews/workflow/index.html',
  name: 'workflow-delivery-preview',
  component: Preview,
  meta: { title: 'Workflow preview · r2' },
})
const app = createApp(App),
  pinia = createPinia()
app.use(pinia)
await useWorkspaceStore(pinia).open('D:/workflow-fixture/metrics')
useTabsStore(pinia).openSurface('chat')
app.use(router)
await router.isReady()
app.mount('#app')
