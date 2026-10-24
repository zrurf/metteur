<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch } from 'vue'
import type { editor as MonacoEditor } from 'monaco-editor'
import { useThemeStore } from '@/stores/theme'
import { ensureMbpLanguage, bindMbpDiagnostics } from '@/lib/mbp-language'
import { acquireModel, releaseModel } from '@/lib/editor-models'
import { useWorkspaceViewStore } from '@/stores/workspace-view'
import type { DocumentPosition } from '@/core/workspace-view'

/**
 * Code editor backed by Monaco Editor, with a CodeMirror fallback for coarse
 * pointers (mobile). Both runtimes are imported lazily so the heavy editor
 * payload is fetched only when actually needed.
 *
 * The parent binds `modelValue` and receives edits via `update:modelValue`;
 * `undo()` / `redo()` are exposed so the shell toolbar can drive them.
 */
const props = defineProps<{
  modelValue: string
  language: string
  /**
   * Stable identity of the edited file (workspace-scoped path). Two editors
   * with the same identity share one Monaco model, so a file shown twice stays
   * in sync and its undo history survives a tab switch. Omit it for a
   * throwaway buffer.
   */
  modelId?: string
  viewTarget?: { workspace: string; path: string }
}>()
const emit = defineEmits<{
  (e: 'update:modelValue', value: string): void
  (e: 'history', canUndo: boolean, canRedo: boolean): void
}>()

const themeStore = useThemeStore()
const views = useWorkspaceViewStore()
let positionFrame = 0
// Host positions count the original text, including BOM and CRLF bytes as UTF-16.
const monacoBom = () => (monoModel?.getValue(undefined, true).startsWith('\uFEFF') ? 1 : 0)
const cmRawOffset = (offset: number) =>
  cmView
    ? offset + (cmView.state.doc.lineAt(offset).number - 1) * (cmView.state.lineBreak.length - 1)
    : offset
const cmEditorOffset = (offset: number) =>
  props.modelValue.slice(0, offset).replaceAll('\r\n', '\n').length
function savePosition() {
  if (!props.viewTarget || disposed || positionFrame) return
  positionFrame = requestAnimationFrame(() => {
    positionFrame = 0
    if (!props.viewTarget || disposed) return
    let position: DocumentPosition | undefined
    if (monoEditor && monoModel)
      position = {
        kind: 'text',
        top: monoEditor.getScrollTop(),
        left: monoEditor.getScrollLeft(),
        selections: (monoEditor.getSelections() ?? []).map((selection) => ({
          anchor: monoModel!.getOffsetAt(selection.getSelectionStart()) + monacoBom(),
          head: monoModel!.getOffsetAt(selection.getPosition()) + monacoBom(),
        })),
      }
    else if (cmView)
      position = {
        kind: 'text',
        top: cmView.scrollDOM.scrollTop,
        left: cmView.scrollDOM.scrollLeft,
        selections: cmView.state.selection.ranges.map((range) => ({
          anchor: cmRawOffset(range.anchor),
          head: cmRawOffset(range.head),
        })),
      }
    if (position) views.position(props.viewTarget.workspace, props.viewTarget.path, position)
  })
}
async function restoredPosition() {
  const target = props.viewTarget
  if (!target) return null
  const view = await views.ready(target.workspace)
  return view.positions[target.path]?.kind === 'text'
    ? (view.positions[target.path] as Extract<DocumentPosition, { kind: 'text' }>)
    : null
}

const container = ref<HTMLDivElement | null>(null)
const mode = ref<'loading' | 'monaco' | 'cm' | 'error'>('loading')
const loadError = ref('')
let disposed = false
let initializationFailed = false
/** Whether the active editor has anything to undo/redo (drives tool buttons). */
const canUndo = ref(false)
const canRedo = ref(false)

function syncUndoState() {
  // Monaco exposes canUndo/canRedo/isDisposed on the text *model* (absent from
  // IStandaloneCodeEditor), so drive the toolbar state from it.
  const model = monoModel as unknown as {
    isDisposed(): boolean
    canUndo(): boolean
    canRedo(): boolean
  }
  if (monoEditor && monoModel && !model.isDisposed()) {
    canUndo.value = model.canUndo()
    canRedo.value = model.canRedo()
    emit('history', canUndo.value, canRedo.value)
  }
}

function monacoLanguage(lang: string): string {
  if (lang === 'mbp') return 'mbp'
  if (lang === 'json') return 'json'
  if (lang === 'markdown' || lang === 'toml') return lang
  if (lang.startsWith('ts')) return 'typescript'
  if (lang === 'js' || lang === 'jsx' || lang === 'mjs' || lang === 'cjs') return 'javascript'
  if (lang === 'html' || lang === 'htm') return 'html'
  if (lang === 'css' || lang === 'scss' || lang === 'less') return 'css'
  return 'plaintext'
}

let monacoMod: typeof import('@/lib/monaco').default | null = null
let monoEditor: MonacoEditor.IStandaloneCodeEditor | null = null
let monoModel: MonacoEditor.ITextModel | null = null

function applyMonacoTheme() {
  monacoMod?.editor.setTheme(themeStore.mode === 'dark' ? 'vs-dark' : 'vs')
}

async function setupMonaco() {
  const mod = (await import('@/lib/monaco')).default
  const savedPosition = await restoredPosition()
  if (disposed || initializationFailed || !container.value) return
  monacoMod = mod
  applyMonacoTheme()
  const lang = monacoLanguage(props.language)
  // The DSL language must exist before the model references it, so
  // tokenization/complete handlers are attached from the start.
  if (lang === 'mbp') ensureMbpLanguage(mod)
  // A shared model is created once per file; a second editor on the same file
  // attaches to it instead of creating a colliding URI.
  const model = props.modelId
    ? acquireModel(mod, props.modelId, lang, props.modelValue)
    : mod.editor.createModel(props.modelValue, lang)
  monoModel = model
  if (lang === 'mbp') bindMbpDiagnostics(mod, model)
  monoEditor = mod.editor.create(container.value!, {
    model,
    theme: themeStore.mode === 'dark' ? 'vs-dark' : 'vs',
    automaticLayout: true,
    minimap: { enabled: false },
    fontSize: 12.5,
    lineHeight: 20,
    tabSize: 2,
    scrollBeyondLastLine: false,
    padding: { top: 8, bottom: 8 },
    fontFamily: "'SFMono-Regular', Consolas, 'Liberation Mono', Menlo, monospace",
    renderWhitespace: 'selection',
    fixedOverflowWidgets: true,
  })
  if (savedPosition) {
    monoEditor.setSelections(
      savedPosition.selections.map((selection) => {
        const anchor = model.getPositionAt(Math.max(0, selection.anchor - monacoBom())),
          head = model.getPositionAt(Math.max(0, selection.head - monacoBom()))
        return new mod.Selection(anchor.lineNumber, anchor.column, head.lineNumber, head.column)
      }),
    )
    monoEditor.setScrollPosition({ scrollTop: savedPosition.top, scrollLeft: savedPosition.left })
  }
  monoEditor.onDidChangeCursorSelection(savePosition)
  monoEditor.onDidScrollChange(savePosition)
  monoEditor.onDidChangeModelContent(() => {
    const value = monoModel!.getValue(undefined, true)
    if (value !== props.modelValue) emit('update:modelValue', value)
    syncUndoState()
  })
  syncUndoState()
}

let cmView: import('@codemirror/view').EditorView | null = null
let cmUndo: ((v: unknown) => boolean) | null = null
let cmRedo: ((v: unknown) => boolean) | null = null
let applyCmTheme: (() => void) | null = null
let applyCmLineBreak: ((value: string) => void) | null = null

async function cmLanguage() {
  const jsmod = await import('@codemirror/lang-javascript')
  const jsonmod = await import('@codemirror/lang-json')
  // v6 only ships `javascript`; TypeScript files fall back to it for the
  // mobile CodeMirror path.
  const lang = props.language
  if (lang === 'json') return jsonmod.json()
  if (lang === 'js' || lang === 'jsx' || lang === 'mjs' || lang === 'cjs' || lang.startsWith('ts'))
    return jsmod.javascript()
  return null
}

function cmBaseTheme() {
  return {
    '&': {
      color: 'var(--foreground)',
      backgroundColor: 'var(--background)',
      height: '100%',
      fontSize: '12.5px',
    },
    '.cm-content': {
      caretColor: 'var(--primary)',
      fontFamily: "'SFMono-Regular', Consolas, 'Liberation Mono', Menlo, monospace",
      padding: '8px 0',
    },
    '&.cm-focused': { outline: 'none' },
    '.cm-line': { padding: '0 12px' },
    '.cm-gutters': {
      backgroundColor: 'transparent',
      color: 'var(--subtle)',
      border: 'none',
    },
    '.cm-activeLine': { backgroundColor: 'var(--hover)' },
    '.cm-activeLineGutter': { backgroundColor: 'transparent' },
    '.cm-cursor': { borderLeftColor: 'var(--primary)' },
    '.cm-selectionBackground, &.cm-focused .cm-selectionBackground': {
      backgroundColor: 'var(--selection)',
    },
    '.cm-matchingBracket': {
      backgroundColor: 'color-mix(in srgb, var(--primary) 22%, transparent)',
      outline: '1px solid var(--grid-dot)',
    },
  }
}

async function setupCm() {
  const viewMod = await import('@codemirror/view')
  const stateMod = await import('@codemirror/state')
  const langMod = await import('@codemirror/language')
  const cmdsMod = await import('@codemirror/commands')
  const savedPosition = await restoredPosition()
  cmUndo = (v) => cmdsMod.undo(v as never) ?? true
  cmRedo = (v) => cmdsMod.redo(v as never) ?? true

  const stateSupport = (await cmLanguage()) as unknown
  if (disposed || initializationFailed || !container.value) return
  const themeCompartment = new stateMod.Compartment()
  const lineBreakCompartment = new stateMod.Compartment()
  const extensions = [
    viewMod.lineNumbers(),
    viewMod.highlightActiveLine(),
    viewMod.highlightActiveLineGutter(),
    viewMod.highlightSpecialChars(),
    viewMod.drawSelection(),
    viewMod.dropCursor(),
    stateMod.EditorState.allowMultipleSelections.of(true),
    lineBreakCompartment.of(
      stateMod.EditorState.lineSeparator.of(props.modelValue.includes('\r\n') ? '\r\n' : '\n'),
    ),
    langMod.indentOnInput(),
    langMod.bracketMatching(),
    langMod.syntaxHighlighting(langMod.defaultHighlightStyle, { fallback: true }),
    cmdsMod.history(),
    viewMod.keymap.of([...cmdsMod.defaultKeymap, ...cmdsMod.historyKeymap, cmdsMod.indentWithTab]),
    themeCompartment.of(
      viewMod.EditorView.theme(cmBaseTheme(), { dark: themeStore.mode === 'dark' }),
    ),
    viewMod.EditorView.updateListener.of((update: import('@codemirror/view').ViewUpdate) => {
      if (update.docChanged) {
        const text = update.state.sliceDoc()
        if (text !== props.modelValue) emit('update:modelValue', text)
      }
      if (update.selectionSet || update.docChanged) savePosition()
    }),
  ]
  if (stateSupport) extensions.push(stateSupport as never)

  const view = new viewMod.EditorView({
    parent: container.value!,
    state: stateMod.EditorState.create({
      doc: props.modelValue,
      extensions,
      selection: savedPosition
        ? stateMod.EditorSelection.create(
            savedPosition.selections.map((selection) =>
              stateMod.EditorSelection.range(
                cmEditorOffset(selection.anchor),
                cmEditorOffset(selection.head),
              ),
            ),
          )
        : undefined,
    }),
  })
  cmView = view
  applyCmLineBreak = (value) =>
    view.dispatch({
      effects: lineBreakCompartment.reconfigure(
        stateMod.EditorState.lineSeparator.of(value.includes('\r\n') ? '\r\n' : '\n'),
      ),
    })
  if (savedPosition) {
    view.scrollDOM.scrollTop = savedPosition.top
    view.scrollDOM.scrollLeft = savedPosition.left
  }
  view.scrollDOM.addEventListener('scroll', savePosition, { passive: true })
  applyCmTheme = () =>
    view.dispatch({
      effects: themeCompartment.reconfigure(
        viewMod.EditorView.theme(cmBaseTheme(), { dark: themeStore.mode === 'dark' }),
      ),
    })
}

const isCoarsePointer = () => window.matchMedia?.('(pointer: coarse)').matches ?? false

onMounted(async () => {
  let timeout: ReturnType<typeof setTimeout> | undefined
  try {
    const nextMode = isCoarsePointer() ? 'cm' : 'monaco'
    await Promise.race([
      nextMode === 'cm' ? setupCm() : setupMonaco(),
      new Promise<never>((_resolve, reject) => {
        timeout = setTimeout(
          () => reject(new Error('Editor loading timed out. Reload to retry.')),
          15_000,
        )
      }),
    ])
    if (!disposed) mode.value = nextMode
  } catch (error) {
    initializationFailed = true
    if (disposed) return
    loadError.value = error instanceof Error ? error.message : String(error)
    mode.value = 'error'
  } finally {
    clearTimeout(timeout)
  }
})

onBeforeUnmount(() => {
  cancelAnimationFrame(positionFrame)
  positionFrame = 0
  // Capture the final selection synchronously before disposing the editor.
  if (props.viewTarget && monoEditor && monoModel && !monoModel.isDisposed())
    views.position(props.viewTarget.workspace, props.viewTarget.path, {
      kind: 'text',
      top: monoEditor.getScrollTop(),
      left: monoEditor.getScrollLeft(),
      selections: (monoEditor.getSelections() ?? []).map((selection) => ({
        anchor: monoModel!.getOffsetAt(selection.getSelectionStart()) + monacoBom(),
        head: monoModel!.getOffsetAt(selection.getPosition()) + monacoBom(),
      })),
    })
  else if (props.viewTarget && cmView)
    views.position(props.viewTarget.workspace, props.viewTarget.path, {
      kind: 'text',
      top: cmView.scrollDOM.scrollTop,
      left: cmView.scrollDOM.scrollLeft,
      selections: cmView.state.selection.ranges.map((range) => ({
        anchor: cmRawOffset(range.anchor),
        head: cmRawOffset(range.head),
      })),
    })
  disposed = true
  // The editor always owns its view; the model is owned by the registry when it
  // is shared, and disposed there with the last reference.
  if (monoEditor) monoEditor.dispose()
  if (props.modelId && monoModel) releaseModel(props.modelId)
  else if (monoModel && !monoModel.isDisposed()) monoModel.dispose()
  if (cmView) cmView.destroy()
})

watch(
  () => themeStore.mode,
  () => {
    applyMonacoTheme()
    applyCmTheme?.()
  },
)

// Push externally-set content (reload-from-disk / tab restore) into the live
// editor instead of leaving the model stale.
watch(
  () => props.modelValue,
  (value) => {
    if (monoEditor && monoModel && !monoModel.isDisposed()) {
      if (monoModel?.getValue(undefined, true) !== value) monoEditor.setValue(value)
    } else if (cmView) {
      const cur = cmView.state.sliceDoc()
      if (cur !== value) {
        applyCmLineBreak?.(value)
        cmView.dispatch({ changes: { from: 0, to: cmView.state.doc.length, insert: value } })
      }
    }
  },
)

defineExpose({
  undo() {
    if (monoEditor) {
      monoEditor.trigger('toolbar', 'undo', undefined)
      setTimeout(syncUndoState, 0)
    } else if (cmView && cmUndo) cmUndo(cmView)
  },
  redo() {
    if (monoEditor) {
      monoEditor.trigger('toolbar', 'redo', undefined)
      setTimeout(syncUndoState, 0)
    } else if (cmView && cmRedo) cmRedo(cmView)
  },
  canUndo,
  canRedo,
})
</script>

<template>
  <div class="relative flex h-full w-full min-h-0 flex-col overflow-hidden bg-background">
    <div
      v-if="mode === 'loading'"
      class="absolute inset-0 z-10 flex min-h-0 flex-col bg-background"
    >
      <p class="px-3 py-2 text-[12px] text-muted-foreground" role="status">
        Loading editor… Source is available below.
      </p>
      <textarea
        class="min-h-0 flex-1 resize-none bg-background p-3 font-mono text-[13px] text-foreground"
        aria-label="Loading file source"
        readonly
        :value="modelValue"
      />
    </div>
    <template v-if="mode === 'error'">
      <p class="shrink-0 px-3 py-2 text-[12px] text-danger" role="alert">
        Editor could not load. Showing read-only source. {{ loadError }}
      </p>
      <textarea
        class="min-h-0 flex-1 resize-none bg-background p-3 font-mono text-[13px] text-foreground"
        aria-label="Read-only file source"
        readonly
        :value="modelValue"
      />
    </template>
    <div v-show="mode !== 'error'" ref="container" class="min-h-0 w-full flex-1" />
  </div>
</template>
