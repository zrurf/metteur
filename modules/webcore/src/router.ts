import { createRouter, createWebHistory } from 'vue-router'
import { useWorkspaceStore } from '@/stores/workspace'
import { workspaceIdOf } from '@/lib/workspace-url'

declare module 'vue-router' {
  interface RouteMeta {
    /** Whether this surface needs an open workspace before it can render. */
    workspace?: boolean
    /** Label shown in the title bar and used to derive `document.title`. */
    title?: string
  }
}

const routes = [
  { path: '/work/:wid/execution', name: 'execution', component: () => import('./views/ExecutionView.vue'), meta: { title: 'Execution', workspace: true } },
  ...(import.meta.env.DEV && import.meta.env.VITE_MOCK === '1' ? [{
    path: '/preview/execution',
    name: 'execution-preview',
    component: () => import('./views/ExecutionPreview.vue'),
    beforeEnter: async () => {
      const workspace = useWorkspaceStore()
      if (!workspace.hasActive) await workspace.open('F:/space/06_Projects/Agent/metteur')
    },
    meta: { title: 'Execution preview' },
  }] : []),
  {
    path: '/',
    name: 'root',
    component: () => import('./views/RootView.vue'),
    meta: { title: 'Metteur' },
  },
  {
    path: '/welcome',
    name: 'welcome',
    component: () => import('./views/HomeView.vue'),
    meta: { title: 'Welcome' },
  },
  {
    path: '/work/:wid',
    name: 'work',
    redirect: (to: any) => `/work/${to.params.wid}/explorer`,
    meta: { title: 'Metteur' },
  },
  {
    path: '/work/:wid/explorer',
    name: 'explorer',
    component: () => import('./views/EmptyEditor.vue'),
    meta: { title: 'Explorer', workspace: true },
  },
  {
    path: '/work/:wid/chat',
    name: 'chat',
    component: () => import('./views/ChatView.vue'),
    meta: { title: 'Chat', workspace: true },
  },
  {
    path: '/work/:wid/file',
    name: 'file',
    component: () => import('./views/EditorPane.vue'),
    meta: { title: 'File', workspace: true },
  },
  {
    path: '/work/:wid/version',
    name: 'version',
    component: () => import('./views/VersionView.vue'),
    meta: { title: 'Version Flow', workspace: true },
  },
  {
    path: '/settings',
    name: 'settings',
    component: () => import('./views/SettingsView.vue'),
    meta: { title: 'Settings' },
  },
]

export const router = createRouter({
  history: createWebHistory(),
  routes,
})

router.afterEach((to) => {
  const app = 'Metteur'
  document.title = to.meta.title ? `${app} | ${to.meta.title}` : app
})

// Single-workspace shell: workspace-gated surfaces land on the welcome page
// without an open workspace (stashing the intended route so it can be resumed
// after the async workspace restore), and their `:wid` must match the active
// workspace. A stray /welcome with an open workspace goes to Explorer.
const PENDING_KEY = 'metteur.pending-route'

router.beforeEach((to) => {
  const workspace = useWorkspaceStore()
  if (to.meta.workspace && !workspace.hasActive) {
    sessionStorage.setItem(PENDING_KEY, to.fullPath)
    return { name: 'welcome' }
  }
  if (to.meta.workspace && workspace.active && to.params.wid !== workspaceIdOf(workspace.active.path)) {
    return { name: to.name as string, params: { ...to.params, wid: workspaceIdOf(workspace.active.path) } }
  }
  if (to.name === 'welcome' && workspace.hasActive && workspace.active) {
    return { name: 'explorer', params: { wid: workspaceIdOf(workspace.active.path) } }
  }
  return true
})

export const pendingRoute = () => {
  const full = sessionStorage.getItem(PENDING_KEY)
  sessionStorage.removeItem(PENDING_KEY)
  return full
}
