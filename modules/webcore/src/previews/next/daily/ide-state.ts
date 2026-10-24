import { computed, reactive } from 'vue'
import { useChatStore } from '@/stores/chat'
import { useExecutionStore } from '@/stores/execution'

export const fixture = reactive({
  preview: new URLSearchParams(location.search).get('preview') || 'WF-PV-01',
  state: 'Running',
  notice: '',
  dialog: '',
  drafts: {} as Record<string, string>,
  backup: '',
  disk: '# Metrics\nSaved by another window.\n',
  diskRevision: 7,
  reviewed: 7,
  source: 'Chat Handoff',
  policy: 'Keep Results',
})
export const occupied = computed(() => !['Completed','Failed','Stopped'].includes(fixture.state))
export function draftKey(concierge: unknown) {
  return concierge ? `supervision:${useExecutionStore().runId ?? 'fixture-run'}` : `chat:${useChatStore().sessionId || 'new'}`
}
export function fixtureControl(action: string) {
  fixture.notice = fixture.state === 'Disconnected' ? `${action} outcome unknown. Reconcile this run.`
    : fixture.state === 'Permission Denied' ? `${action} rejected. Permission denied.`
    : `${action} confirmed for fixture-run. No real execution.`
  if (!['Disconnected','Permission Denied'].includes(fixture.state)) fixture.state = action === 'Stop' ? 'Stopped' : action === 'Pause' ? 'Paused' : 'Running'
}
