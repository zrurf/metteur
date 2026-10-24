import { defineStore } from 'pinia'
import { ref } from 'vue'
import {
  Braces,
  Coins,
  Cpu,
  FileText,
  History,
  Link2,
  Palette,
  Puzzle,
  Server,
  ShieldCheck,
} from '@lucide/vue'
import type { Component } from 'vue'

/**
 * Settings-surface navigation state.
 *
 * The left rail lists setting *groups* (VSCode-style), and the content pane
 * renders whichever group is highlighted. Scope is handled per-group inside
 * the view: every config-backed setting can be overridden at the user
 * (global) or workspace layer, workspace winning — the same layering model as
 * VSCode preferences.
 */

export interface SettingsSection {
  key: string
  label: string
  icon: Component
}

export const SETTINGS_GROUPS: SettingsSection[] = [
  { key: 'general', label: 'General', icon: Palette },
  { key: 'llm', label: 'LLM & Models', icon: Cpu },
  { key: 'sandbox', label: 'Sandbox', icon: ShieldCheck },
  { key: 'mcp', label: 'MCP Servers', icon: Link2 },
  { key: 'lsp', label: 'LSP Servers', icon: Braces },
  { key: 'addons', label: 'Addons', icon: Puzzle },
  { key: 'addon-pages', label: 'Addon Pages', icon: Puzzle },
  { key: 'versioning', label: 'Versioning', icon: History },
  { key: 'billing', label: 'Billing', icon: Coins },
  { key: 'daemon', label: 'Daemon', icon: Server },
  { key: 'toml', label: 'Settings TOML', icon: FileText },
]

/** Scope choices for the settings layer toggle (user vs workspace). */
export const SETTINGS_SCOPES = [
  { key: 'user', label: 'User' },
  { key: 'workspace', label: 'Workspace' },
] as const

export type SettingsLayer = (typeof SETTINGS_SCOPES)[number]['key']

export const useSettingsStore = defineStore('settings', () => {
  /** The highlighted group (drives the content pane). */
  const active = ref<string>('general')
  /** The layer being edited: `user` (global) or `workspace`. */
  const layer = ref<SettingsLayer>('user')

  function select(key: string) {
    active.value = key
  }

  return { active, layer, select }
})
