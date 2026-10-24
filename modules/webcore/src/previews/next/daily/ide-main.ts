import { createApp } from 'vue'
import { createPinia } from 'pinia'
import './ide/ide.css'
import { router } from '@/router'
import { useWorkspaceStore } from '@/stores/workspace'
import { useTabsStore } from '@/stores/tabs'
import { usePanelStore } from '@/stores/panel'
import FileTree from '@/components/FileTree.vue'
import { workspaceIdOf } from '@/lib/workspace-url'
import { fileRoute } from '@/lib/file-token'
import IdePreview from './IdePreview.vue'
import { fixture } from './ide-state'
const app=createApp(IdePreview)
const pinia=createPinia(); app.use(pinia)
const workspace=useWorkspaceStore(pinia)
await workspace.open('D:/metteur-daily-fixture/metrics')
app.use(router)
await router.isReady()
const surface=fixture.preview==='WF-PV-03'?'tasks':fixture.preview==='WF-PV-02'?'explorer':'chat'
const tabs=useTabsStore(pinia)
if(surface==='explorer') {
  tabs.openFile('metrics/README.md');usePanelStore(pinia).show('explorer',FileTree,'Explorer')
  await router.replace(fileRoute('metrics/README.md'))
} else {tabs.openSurface(surface);await router.replace(`/work/${workspaceIdOf(workspace.active!.path)}/${surface}`)}
app.mount('#app')
