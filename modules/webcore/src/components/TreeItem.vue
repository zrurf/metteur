<script setup lang="ts">
import { inject } from 'vue'
import type { Component } from 'vue'
import {
  ChevronRight,
  FileCode,
  FileCode2,
  FileJson,
  FileText,
  Folder,
  FolderOpen,
  Workflow,
} from '@lucide/vue'
import type { FileTreeNode } from '@/core'
import { TREE_API, TREE_CREATE, TREE_RENAME } from '@/lib/tree'
import TreeNameInput from './TreeNameInput.vue'

defineOptions({ name: 'TreeItem' })

const api = inject(TREE_API)!
const create = inject(TREE_CREATE, null)
/** Inline rename state (null when no row is being renamed). */
const rename = inject(TREE_RENAME, null)

const props = defineProps<{
  node: FileTreeNode
  depth: number
}>()

function icon(node: FileTreeNode): Component {
  if (node.kind === 'dir') return api.isOpen(node.path) ? FolderOpen : Folder
  if (node.path.endsWith('.blueprint')) return Workflow
  if (node.path.endsWith('.mbp')) return FileCode2
  if (node.path.endsWith('.json')) return FileJson
  if (node.path.endsWith('.ts') || node.path.endsWith('.js')) return FileCode
  return FileText
}

function fileName(path: string): string {
  const seg = path.split(/[\\/]/).filter(Boolean)
  return seg[seg.length - 1] ?? path
}
</script>

<template>
  <div class="tree-row text-[12.5px]">
    <!-- Inline rename: the label becomes an input while the row is renamed. -->
    <div
      v-if="rename && rename.activePath === node.path"
      class="flex items-center gap-1 py-0.5 pr-2"
      :style="{ paddingLeft: 8 + depth * 14 + 3 + 'px' }"
    >
      <input
        :value="rename.name"
        class="input h-6! w-full text-[12px]"
        autofocus
        @input="rename.setName(($event.target as HTMLInputElement).value)"
        @focus="($event.target as HTMLInputElement).select()"
        @keydown.enter="rename.confirm()"
        @keydown.esc="rename.cancel()"
        @blur="rename.confirm()"
      />
    </div>
    <button
      v-else
      class="relative flex w-full items-center gap-1 rounded-md py-1 pr-2 text-left text-muted-foreground transition-colors duration-100 hover:bg-hover hover:text-foreground"
      :class="api.rowCls(node.path)"
      :style="{ paddingLeft: 8 + depth * 14 + 'px' }"
      type="button"
      @click="api.select(node, $event)"
      @contextmenu.prevent="api.onRowContextMenu(node, $event)"
    >
      <!-- Fixed chevron column: a dir shows a caret, a file leaves it empty so
           icons at the same depth line up. -->
      <span class="flex w-3.5 shrink-0 items-center justify-center">
        <ChevronRight
          v-if="node.kind === 'dir'"
          class="h-3 w-3 text-subtle transition-transform duration-150"
          :class="api.isOpen(node.path) ? 'rotate-90' : ''"
        />
      </span>
      <component :is="icon(node)" class="h-3.5 w-3.5 shrink-0 text-subtle" />
      <span class="truncate font-medium">{{ fileName(node.path) }}</span>
    </button>

    <div v-if="node.kind === 'dir' && api.isOpen(node.path) && node.children" class="relative">
      <!-- Vertical guide from this folder's icon down to its children. -->
      <div
        class="pointer-events-none absolute bottom-0 top-0 w-px bg-divider"
        :style="{ left: 8 + depth * 14 + 7 + 'px' }"
      />
      <TreeNameInput
        v-if="create && create.active && create.active.dir === node.path"
        v-model="create.name"
        :kind="create.active.kind"
        :label="create.active.kind === 'file' ? 'New File Name' : 'New Folder Name'"
        :error="create.error"
        :busy="create.busy"
        :style="{ paddingLeft: 8 + (depth + 1) * 14 + 'px' }"
        @confirm="create.confirm"
        @cancel="create.cancel"
      />
      <TreeItem v-for="child in node.children" :key="child.path" :node="child" :depth="depth + 1" />
    </div>
  </div>
</template>
