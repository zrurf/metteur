import { defineStore } from 'pinia'
import { ref, watch } from 'vue'
import { gateway } from '@/core'
import { useWorkspaceStore } from './workspace'
import { useWorkspaceRefresh } from '@/lib/workspace-refresh'

export interface ForegroundState {
  state: 'idle' | 'chat' | 'blueprint' | 'blocked' | 'unknown'
  run: string | null
  session: string | null
  turn: string | null
  reason: string
}
const unknown = (): ForegroundState => ({
  state: 'unknown',
  run: null,
  session: null,
  turn: null,
  reason: 'Checking workspace availability',
})
export const useForegroundStore = defineStore('foreground', () => {
  const workspace = useWorkspaceStore(),
    current = ref<ForegroundState>(unknown()),
    error = ref('')
  let epoch = 0,
    revision = 0,
    pending: { workspace: string; epoch: number; promise: Promise<boolean> } | null = null
  watch(
    [() => workspace.active?.path, () => gateway.connected.value],
    () => {
      epoch++
      revision++
      current.value = unknown()
      error.value = ''
      pending = null
      if (!gateway.demo && !gateway.connected.value)
        current.value.reason = 'Disconnected · workspace state is unknown'
    },
    { flush: 'sync' },
  )
  async function refresh(): Promise<boolean> {
    const path = workspace.active?.path,
      ticket = epoch
    if (!path || (!gateway.demo && !gateway.connected.value)) return false
    if (pending?.workspace === path && pending.epoch === ticket) return pending.promise
    const sequence = ++revision
    const request = { workspace: path, epoch: ticket, promise: Promise.resolve(false) }
    request.promise = Promise.resolve().then(async () => {
      try {
        const result = await gateway.resourceRequest('chatDelivery', path, {
          operation: 'admission',
        })
        if (epoch !== ticket || sequence !== revision || workspace.active?.path !== path)
          return false
        if (!result.ok) throw new Error(result.error)
        const state = result.data as ForegroundState
        if (
          !['idle', 'chat', 'blueprint', 'blocked'].includes(state.state) ||
          typeof state.reason !== 'string' ||
          (state.state === 'blueprint' && typeof state.run !== 'string') ||
          (state.state === 'chat' &&
            (typeof state.session !== 'string' || typeof state.turn !== 'string'))
        )
          throw new Error('Workspace ownership could not be verified')
        current.value = state
        error.value = ''
        return true
      } catch (reason) {
        if (epoch === ticket && sequence === revision) {
          current.value = unknown()
          current.value.reason = 'Workspace availability is unconfirmed'
          error.value = String(reason)
        }
        return false
      } finally {
        if (pending === request) pending = null
      }
    })
    pending = request
    return request.promise
  }
  async function permits(mode: 'send' | 'queue' | 'steer' | 'retry', session: string) {
    if (!(await refresh())) return false
    if (mode === 'send' || mode === 'retry') return current.value.state === 'idle'
    return current.value.state === 'chat' && current.value.session === session
  }
  useWorkspaceRefresh(refresh, [() => gateway.connected.value])
  return { current, error, refresh, permits }
})
