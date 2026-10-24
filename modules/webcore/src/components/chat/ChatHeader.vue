<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { Check, ChevronDown, Loader2, MessageSquarePlus, Plug, Plus, Trash2 } from '@lucide/vue'
import type { AddonInfo, ChatSessionInfo } from '@/core'
import PageHeader from '@/components/PageHeader.vue'

/**
 * The chat header: which conversation, which model, which plugins.
 *
 * It owns the session's identity and its configuration rather than the
 * transcript, so the reading column stays free of chrome. Everything here is a
 * menu that closes on outside click or `Esc`.
 */
const props = defineProps<{
  threads: ChatSessionInfo[]
  sessionId: string
  activeTitle: string
  addons: AddonInfo[]
  addonsLoading: boolean
  busy: boolean
  onSelectSession: (id: string) => void
  onDeleteSession: (id: string) => void
  onNewSession: () => void
  onToggleAddon: (id: string, enabled: boolean) => void
}>()

const open = ref<'session' | 'plugins' | null>(null)

function toggle(menu: 'session' | 'plugins'): void {
  open.value = open.value === menu ? null : menu
}
function selectConversation(id: string): void {
  props.onSelectSession(id)
  open.value = null
}
function createConversation(): void {
  props.onNewSession()
  open.value = null
}

function onKeydown(event: KeyboardEvent): void {
  if (event.key === 'Escape') open.value = null
}
onMounted(() => window.addEventListener('keydown', onKeydown))
onBeforeUnmount(() => window.removeEventListener('keydown', onKeydown))

const enabledAddons = computed(() => props.addons.filter((a) => a.enabled))
</script>

<template>
  <PageHeader class="chat-header" title="Chat" toolbar>
    <div class="ml-auto flex items-center gap-1">
      <!-- Session switcher -->
      <div class="relative">
        <button
          class="chat-header-button"
          type="button"
          :disabled="busy"
          title="Conversations"
          :aria-label="`Conversations — ${activeTitle}`"
          :aria-expanded="open === 'session'"
          @click="toggle('session')"
        >
          <span class="max-w-40 truncate">{{ activeTitle }}</span>
          <ChevronDown class="h-3 w-3 shrink-0 text-subtle" />
        </button>
        <div v-if="open === 'session'" class="chat-menu absolute right-0 top-8 z-40 w-80">
          <p class="chat-menu-head">Conversations</p>
          <p v-if="!threads.length" class="px-2 py-1.5 text-[12px] text-subtle">
            No saved conversations yet.
          </p>
          <div
            v-for="thread in threads"
            :key="thread.sessionId"
            class="group/thread flex items-center gap-1 rounded-md hover:bg-hover"
          >
            <button
              class="flex min-w-0 flex-1 items-center gap-2 px-2 py-1.5 text-left"
              type="button"
              @click="selectConversation(thread.sessionId)"
            >
              <Check
                v-if="thread.sessionId === sessionId"
                class="h-3.5 w-3.5 shrink-0 text-primary"
              />
              <span v-else class="w-3.5 shrink-0" />
              <span class="min-w-0 flex-1">
                <span class="block truncate text-[12.5px]">{{ thread.title || '(untitled)' }}</span>
                <span class="block text-[10.5px] text-subtle">
                  {{ thread.messageCount }} messages · {{ thread.turns }} turns
                </span>
              </span>
            </button>
            <button
              class="mr-1 grid h-6 w-6 shrink-0 place-items-center rounded text-subtle opacity-0 transition-opacity duration-100 hover:text-status-error focus-visible:opacity-100 group-hover/thread:opacity-100"
              type="button"
              :aria-label="`Delete ${thread.title || 'conversation'}`"
              @click="onDeleteSession(thread.sessionId)"
            >
              <Trash2 class="h-3.5 w-3.5" />
            </button>
          </div>
          <div class="my-1 border-t border-divider" />
          <button class="chat-menu-item" type="button" @click="createConversation">
            <MessageSquarePlus class="h-3.5 w-3.5" /> New conversation
          </button>
        </div>
      </div>

      <button
        class="chat-header-icon"
        type="button"
        :disabled="busy"
        title="New conversation"
        aria-label="New conversation"
        @click="onNewSession()"
      >
        <Plus class="h-4 w-4" />
      </button>

      <!-- Plugins -->
      <div class="relative">
        <button
          class="chat-header-icon"
          type="button"
          title="Plugins"
          aria-label="Plugins"
          :aria-expanded="open === 'plugins'"
          @click="toggle('plugins')"
        >
          <Plug class="h-3.5 w-3.5" />
          <span v-if="enabledAddons.length" class="chat-badge">{{ enabledAddons.length }}</span>
        </button>
        <div v-if="open === 'plugins'" class="chat-menu absolute right-0 top-8 z-40 w-72">
          <p class="chat-menu-head">Plugins</p>
          <p
            v-if="addonsLoading"
            class="flex items-center gap-1.5 px-2 py-2 text-[12px] text-subtle"
          >
            <Loader2 class="h-3.5 w-3.5 animate-spin" /> Loading…
          </p>
          <p v-else-if="!addons.length" class="px-2 py-2 text-[12px] text-subtle">
            No plugins installed.
          </p>
          <div
            v-for="addon in addons"
            :key="addon.id"
            class="flex items-center gap-2.5 rounded-md px-2 py-1.5 hover:bg-hover"
          >
            <div class="min-w-0 flex-1">
              <div class="flex items-center gap-1.5">
                <span class="truncate text-[12.5px] font-medium">{{ addon.name }}</span>
                <span v-if="addon.scope" class="chip text-[9.5px]!">{{ addon.scope }}</span>
              </div>
              <p class="text-[11px] text-subtle">v{{ addon.version }}</p>
            </div>
            <button
              class="chat-switch"
              type="button"
              role="switch"
              :aria-checked="addon.enabled"
              :class="addon.enabled ? 'chat-switch-on' : ''"
              @click="onToggleAddon(addon.id, !addon.enabled)"
            >
              <span class="chat-switch-knob" :class="addon.enabled ? 'translate-x-3' : ''" />
            </button>
          </div>
        </div>
      </div>
    </div>

    <div v-if="open" class="fixed inset-0 z-30" @mousedown="open = null" />
  </PageHeader>
</template>
