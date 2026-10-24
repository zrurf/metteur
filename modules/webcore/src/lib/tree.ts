import type { InjectionKey } from 'vue'
import type { FileTreeNode } from '@/core'

/**
 * Contract the explorer root provides to every recursive tree row.
 *
 * The expanded-set read goes through a method instead of exposing the raw
 * `Set`, so rows stay reactive when the set mutates. Split into this module
 * because `<script setup>` cannot re-export shared symbols.
 */
export interface TreeApi {
  /** Whether a directory row is expanded. */
  isOpen(path: string): boolean
  /** Toggle a directory row's expansion. */
  toggle(node: FileTreeNode): void
  /** Open a file row in the editor. */
  openFile(node: FileTreeNode): void
  /** Row highlight classset for selection or the file currently being edited. */
  rowCls(path: string): string
  /** Row click: plain selects-and-acts, Ctrl toggles, Shift spans a range. */
  select(node: FileTreeNode, event: MouseEvent): void
  /** Populate the explorer row context menu at the event position. */
  onRowContextMenu(node: FileTreeNode, event: MouseEvent): void
}

/** Injection token shared between the FileTree root and every nested TreeItem. */
export const TREE_API: InjectionKey<TreeApi> = Symbol('tree-api')

/** Pending in-tree file/folder creation state rendered as an inline row. */
export interface TreeCreateApi {
  /** Active creation target; `dir` empty means the workspace root. */
  active: { kind: 'file' | 'folder'; dir: string } | null
  /** Live name field of the inline input. */
  name: string
  error: string
  busy: boolean
  /** Commit the current name (Enter / blur). */
  confirm(): void
  /** Abort creation (Escape). */
  cancel(): void
}

/** Injection token so directory rows can host the inline creation input. */
export const TREE_CREATE: InjectionKey<TreeCreateApi> = Symbol('tree-create')

/** Pending in-tree file/folder rename state shown as the row's inline input. */
export interface TreeRenameApi {
  /** Path of the row being renamed; `null` when idle. */
  activePath: string | null
  /** Live name field of the inline input. */
  name: string
  /** Push a keystroke into the rename field. */
  setName(value: string): void
  /** Commit the current name (Enter / blur). */
  confirm(): void
  /** Abort the rename (Escape), reverting to the original name. */
  cancel(): void
}

/** Injection token so any row can become an inline rename input. */
export const TREE_RENAME: InjectionKey<TreeRenameApi> = Symbol('tree-rename')
