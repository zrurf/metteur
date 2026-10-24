import type { editor as MonacoEditor } from 'monaco-editor'
import { hash } from 'ohash'

/** The Monaco namespace as exported by the lazy loader. */
type Monaco = typeof import('@/lib/monaco').default

/**
 * Model registry: one Monaco text model per file, shared by every pane that
 * shows it.
 *
 * Monaco requires every model URI to be unique (a second `createModel` with an
 * existing URI throws) while VSCode semantics require two panes on the same
 * file to share one model — edits, undo history and dirty state are per file,
 * not per pane. This registry provides both: the URI is derived from the
 * workspace-scoped file identity. Hidden tabs retain their model and undo stack;
 * closing a tab disposes its model after the last pane detaches.
 *
 * Without it, opening the same file twice (or two `.mbp` files, which used to
 * share a hard-coded URI) threw inside `setupMonaco`, and the pane stayed
 * blank.
 */

interface Entry {
  model: MonacoEditor.ITextModel
  /** Number of live editors holding this model. */
  refs: number
  closing?: boolean
}

const entries = new Map<string, Entry>()

/** The URI handed to Monaco for one file identity. */
export function modelUri(identity: string): string {
  const name = identity.split(/[\\/]/).pop() || 'buffer'
  return `metteur://${hash(identity)}/${encodeURIComponent(name)}`
}

/**
 * Returns the model for `identity`, creating it on first use.
 *
 * The caller must call {@link releaseModel} with the same identity when its
 * editor is disposed.
 */
export function acquireModel(
  monaco: Monaco,
  identity: string,
  language: string,
  value: string,
): MonacoEditor.ITextModel {
  const existing = entries.get(identity)
  if (existing && !existing.model.isDisposed()) {
    existing.refs += 1
    if (existing.model.getValue(undefined, true) !== value) existing.model.setValue(value)
    if (existing.model.getLanguageId() !== language) {
      monaco.editor.setModelLanguage(existing.model, language)
    }
    return existing.model
  }

  const uri = monaco.Uri.parse(modelUri(identity))
  // A model the registry lost track of (disposed outside a release) would make
  // this throw; dropping the stale URI first keeps the acquisition total.
  monaco.editor.getModel(uri)?.dispose()
  const model = monaco.editor.createModel(value, language, uri)
  entries.set(identity, { model, refs: 1 })
  return model
}

/** A hidden tab retains its undo stack; only closing the document releases it. */
export function releaseModel(identity: string): void {
  const entry = entries.get(identity)
  if (!entry) return
  entry.refs -= 1
  if (entry.refs > 0 || !entry.closing) return
  entries.delete(identity)
  if (!entry.model.isDisposed()) entry.model.dispose()
}

export function closeModel(identity: string): void {
  const entry = entries.get(identity)
  if (!entry) return
  entry.closing = true
  if (entry.refs === 0) {
    entries.delete(identity)
    if (!entry.model.isDisposed()) entry.model.dispose()
  }
}

/** Number of live models, for tests and diagnostics. */
export function modelCount(): number {
  return entries.size
}
