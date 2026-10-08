import { useRouter } from 'vue-router'
import VersionSidebar from '@/components/VersionSidebar.vue'
import SettingsNav from '@/components/SettingsNav.vue'
import { useTabsStore } from '@/stores/tabs'
import { usePanelStore } from '@/stores/panel'
import { wurl } from '@/lib/workspace-url'

/**
 * Surface navigation with its owning sidebar panel asserted.
 *
 * Surface ownership is a single source of truth so that re-opening a surface
 * (from the activity rail *or* from its tab) always restores the correct left
 * rail — fixing the "Version sidebar vanishes after toggling the Explorer and
 * returning" bug where a tab click only re-navigated the route and never
 * re-asserted the panel.
 *
 * This must be used as a composable (called during a component's `setup()`):
 * `useRouter()` and the Pinia stores rely on active-injection, which does not
 * exist at event-handler time. Each consumer resolves them once in setup and
 * reuses the returned `openSurface`.
 */

type SurfaceKey = 'chat' | 'version' | 'settings' | 'execution' | 'execution-preview'
type PanelSurface = Exclude<SurfaceKey, 'chat'>

interface SurfacePanel {
  owner: 'version' | 'settings'
  comp: typeof VersionSidebar | typeof SettingsNav
  title: string
}

/** Sidebar each surface should show; `null` means full-width (no rail). */
function surfacePanel(surface: PanelSurface): SurfacePanel | null {
  if (surface === 'version') return { owner: 'version', comp: VersionSidebar, title: 'Version Flow' }
  if (surface === 'settings') return { owner: 'settings', comp: SettingsNav, title: 'Settings' }
  return null
}

export function useSurfaceNavigation() {
  const router = useRouter()
  const panel = usePanelStore()
  const tabs = useTabsStore()

  /** Open (or focus) a surface tab, navigate to it, and assert its sidebar. */
  function openSurface(key: SurfaceKey) {
    tabs.openSurface(key)
    const target = key === 'execution-preview' ? '/preview/execution'
      : key === 'settings' ? '/settings' : wurl(`/${key}`)
    router.push(target)
    const p = surfacePanel(key as PanelSurface)
    if (key === 'execution-preview' || key === 'execution') panel.clear()
    if (p) panel.show(p.owner, p.comp, p.title)
    // chat is a full-page surface: it owns no sidebar, so the current panel
    // (e.g. the Explorer) stays open instead of being cleared.
  }

  return { openSurface }
}

export type { SurfaceKey }
