import { fileRoute } from './file-token'
import { rootRoute, wurl } from './workspace-url'
import { VIEW_SURFACES, type WorkspaceView } from '@/core/workspace-view'

/** Call only after checking the active workspace; deep links always take priority. */
export function restoredRoute(view: WorkspaceView): string {
  const tab = view.tabs.find((item) =>
    'file' in item ? `file:${item.file}` === view.active : item.surface === view.active,
  )
  if (tab && 'file' in tab) return fileRoute(tab.file)
  if (tab && 'surface' in tab && VIEW_SURFACES.includes(tab.surface))
    return tab.surface === 'settings' ? '/settings' : wurl(`/${tab.surface}`)
  return rootRoute()
}
