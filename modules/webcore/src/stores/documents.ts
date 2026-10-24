import { defineStore } from 'pinia'
import { reactive } from 'vue'
import { gateway } from '@/core'
import { readDocument, writeDocument, parsePromptFile } from '@/lib/prompt-file'
import type { PromptEntry, ResourceRef } from '@/core/resources'
import { useRecoveryStore } from './recovery'
import { hash } from 'ohash'

export interface EditorDocument {
  workspace: string
  path: string
  content: string
  baseline: string
  disk: string
  loaded: boolean
  deleted: boolean
  conflict: boolean
  revision: number
  saving: boolean
  error: string
  readOnlyReason?: string
  resource?: PromptEntry
  diskResource?: PromptEntry
}
export interface DocumentReview {
  workspace: string
  path: string
  revision: number
  local: string
  baseline: string
  latest: string
  deleted: boolean
  resource?: PromptEntry
}

/** Buffers belong to workspace documents, never to a mounted editor component. */
export const useDocumentsStore = defineStore('documents', () => {
  const recovery = useRecoveryStore()
  const recoveryKind = (path: string) =>
    path.endsWith('.blueprint') ? ('canvas' as const) : ('text' as const)
  const records = reactive(new Map<string, EditorDocument>())
  const reads = new Map<string, Promise<EditorDocument>>()
  const pendingSync = new Set<EditorDocument>()
  const syncTickets = new WeakMap<EditorDocument, number>()
  const invalidateSync = (doc: EditorDocument) => {
    const ticket = (syncTickets.get(doc) ?? 0) + 1
    syncTickets.set(doc, ticket)
    return ticket
  }
  const key = (workspace: string, path: string) => JSON.stringify([workspace, path])
  const peek = (workspace: string, path: string) => records.get(key(workspace, path))
  function get(workspace: string, path: string) {
    const id = key(workspace, path)
    if (!records.has(id))
      records.set(id, {
        workspace,
        path,
        content: '',
        baseline: '',
        disk: '',
        loaded: false,
        deleted: false,
        conflict: false,
        revision: 0,
        saving: false,
        error: '',
      })
    return records.get(id)!
  }
  const dirty = (doc: EditorDocument) => doc.deleted || doc.conflict || doc.content !== doc.baseline
  function edit(workspace: string, path: string, content: string) {
    const doc = get(workspace, path)
    if (doc.readOnlyReason) {
      doc.error = doc.readOnlyReason
      return
    }
    if (doc.content !== content) {
      doc.content = content
      doc.revision++
      recovery.edit(workspace, recoveryKind(path), path, {
        raw: content,
        baseline_identity: `xxh3:${hash(doc.baseline)}`,
        view: { baseline_content: doc.baseline },
      })
    }
  }
  async function open(workspace: string, path: string) {
    const doc = get(workspace, path),
      id = key(workspace, path)
    if (doc.loaded) {
      void sync(workspace, path)
      return doc
    }
    if (reads.has(id)) return reads.get(id)!
    const pending = (async () => {
      const revision = doc.revision
      try {
        const result = await readDocument(workspace, path)
        if (!result.ok) {
          doc.error = result.error
          return doc
        }
        if (revision === doc.revision) doc.content = doc.baseline = doc.disk = result.data.content
        doc.loaded = true
        doc.error = ''
        doc.resource = result.data.resource
        doc.readOnlyReason = result.data.readOnlyReason
        doc.diskResource = result.data.resource
        return doc
      } finally {
        reads.delete(id)
      }
    })()
    reads.set(id, pending)
    return pending
  }
  async function sync(workspace: string, path: string) {
    const doc = get(workspace, path)
    if (!doc.loaded) return false
    if (doc.saving) {
      pendingSync.add(doc)
      return false
    }
    const revision = doc.revision
    const ticket = invalidateSync(doc)
    const current = () =>
      ticket === syncTickets.get(doc) && revision === doc.revision && !doc.saving
    const result = await readDocument(workspace, path)
    if (!current()) return false
    if (!result.ok) {
      doc.error = result.error
      // Only a confirmed missing file is deletion; transport/permission errors preserve buffers.
      if (parsePromptFile(path)) {
        if (/not.found|missing|deleted/i.test(result.error)) doc.deleted = true
        return false
      }
      const stat = await gateway.statFile(workspace, path)
      if (!current()) return false
      if (!stat.ok && /not.found|no such file|cannot find|os error 2/i.test(stat.error))
        doc.deleted = true
      return false
    }
    const disk = result.data.content
    const wasDirty = dirty(doc)
    doc.deleted = false
    doc.disk = disk
    doc.diskResource = result.data.resource
    doc.readOnlyReason = result.data.readOnlyReason
    if (wasDirty && disk !== doc.content)
      doc.conflict =
        (disk !== doc.baseline && disk !== doc.content) ||
        (doc.resource !== undefined &&
          (doc.resource.reference.revision !== result.data.resource?.reference.revision ||
            doc.resource.reference.content_digest !==
              result.data.resource?.reference.content_digest))
    else {
      doc.content = doc.baseline = disk
      doc.conflict = false
      doc.resource = result.data.resource
      if (wasDirty) void recovery.saved(workspace, recoveryKind(path), path, disk)
    }
    doc.error = ''
    return true
  }
  async function save(workspace: string, path: string) {
    const doc = get(workspace, path)
    if (!doc.loaded || doc.readOnlyReason) {
      doc.error = doc.readOnlyReason || 'Load the document before saving.'
      return false
    }
    if (doc.saving) return false
    if (doc.conflict && !doc.deleted) {
      doc.error =
        'File content changed on disk; review the current version before saving this draft.'
      return false
    }
    return persist(doc, doc.content, doc.baseline, doc.resource, doc.deleted)
  }
  async function persist(
    doc: EditorDocument,
    content: string,
    expected: string,
    resource?: PromptEntry,
    createOnly = false,
  ) {
    if (doc.saving) return false
    const { workspace, path } = doc
    invalidateSync(doc)
    doc.saving = true
    try {
      const result = await writeDocument(
        workspace,
        path,
        content,
        resource,
        createOnly ? { createOnly: true } : { expectedContent: expected },
      )
      if (!result.ok) {
        doc.error = result.error
        return false
      }
      if (records.get(key(workspace, path)) !== doc || doc.path !== path) return true
      // Edits made while the save was in flight stay dirty against the saved snapshot.
      doc.baseline = doc.disk = content
      doc.deleted = false
      doc.conflict = false
      doc.error = ''
      if (result.data) doc.resource = doc.diskResource = result.data
      // Cleanup is conditional on the exact saved content. A newer edit or
      // an unknown backup receipt keeps its recovery copy intact.
      void recovery.saved(workspace, recoveryKind(path), path, content)
      return true
    } finally {
      doc.saving = false
      if (pendingSync.delete(doc) && records.get(key(workspace, path)) === doc)
        void sync(workspace, path)
    }
  }
  function review(workspace: string, path: string): DocumentReview | null {
    const doc = peek(workspace, path)
    if (!doc?.loaded || doc.saving) return null
    return Object.freeze({
      workspace,
      path,
      revision: doc.revision,
      local: doc.content,
      baseline: doc.baseline,
      latest: doc.disk,
      deleted: doc.deleted,
      ...(doc.diskResource ? { resource: JSON.parse(JSON.stringify(doc.diskResource)) } : {}),
    })
  }
  function matchesReview(snapshot: DocumentReview) {
    const doc = peek(snapshot.workspace, snapshot.path)
    return (
      !!doc &&
      !doc.saving &&
      doc.loaded &&
      doc.revision === snapshot.revision &&
      doc.content === snapshot.local &&
      doc.baseline === snapshot.baseline &&
      doc.disk === snapshot.latest &&
      doc.deleted === snapshot.deleted &&
      JSON.stringify(doc.diskResource?.reference) === JSON.stringify(snapshot.resource?.reference)
    )
  }
  function changedReview(snapshot: DocumentReview) {
    const doc = peek(snapshot.workspace, snapshot.path)
    if (doc) doc.error = 'Your draft or the saved file changed again. Review the latest versions.'
    return false
  }
  async function protectWorkspaceDrafts(workspace: string, paths?: string[]) {
    const drafts = [...records.values()].filter(
      (doc) =>
        doc.workspace === workspace &&
        doc.loaded &&
        dirty(doc) &&
        (!paths || paths.includes(doc.path)),
    )
    const versions = drafts.map((doc) => ({
      doc,
      revision: doc.revision,
      raw: doc.content,
      path: doc.path,
    }))
    for (const { doc, revision, raw, path } of versions) {
      if (doc.saving) {
        doc.error = 'Wait for the current save before restoring files.'
        return false
      }
      recovery.edit(workspace, recoveryKind(path), path, {
        raw,
        baseline_identity: `xxh3:${hash(doc.baseline)}`,
        view: { baseline_content: doc.baseline },
      })
      if (!(await recovery.flush(workspace, recoveryKind(path), path))) {
        doc.error =
          'The current draft could not be backed up. Keep or export it before replacing files.'
        return false
      }
      if (doc.revision !== revision || doc.content !== raw || doc.path !== path) {
        doc.error = 'A newer draft was kept. Review it before replacing files.'
        return false
      }
    }
    return versions.every(
      ({ doc, revision, raw, path }) =>
        doc.revision === revision && doc.content === raw && doc.path === path,
    )
  }
  async function saveReviewed(snapshot: DocumentReview) {
    if (!matchesReview(snapshot)) return changedReview(snapshot)
    const doc = get(snapshot.workspace, snapshot.path)
    // The latest saved side remains recoverable in existing Version Flow;
    // Prompt resources already preserve their append-only revision history.
    if (!snapshot.resource && !snapshot.deleted) {
      const backup = await gateway.createSnapshot(
        snapshot.workspace,
        `Before resolving conflict: ${snapshot.path}`,
      )
      if (!backup.ok) {
        doc.error = `The saved version could not be protected: ${backup.error}`
        return false
      }
    }
    if (!matchesReview(snapshot)) return changedReview(snapshot)
    return persist(doc, snapshot.local, snapshot.latest, snapshot.resource, snapshot.deleted)
  }
  async function useReviewedDisk(snapshot: DocumentReview) {
    if (!matchesReview(snapshot) || snapshot.deleted) return changedReview(snapshot)
    if (!(await sync(snapshot.workspace, snapshot.path))) return false
    if (!matchesReview(snapshot)) return changedReview(snapshot)
    if (
      !(await protectWorkspaceDrafts(snapshot.workspace, [snapshot.path])) ||
      !matchesReview(snapshot)
    )
      return false
    const doc = get(snapshot.workspace, snapshot.path)
    if (
      !recovery.retainConfirmed(
        snapshot.workspace,
        recoveryKind(snapshot.path),
        snapshot.path,
        snapshot.local,
      )
    ) {
      doc.error =
        'The original draft has no confirmed recovery copy. Keep editing or export it first.'
      return false
    }
    invalidateSync(doc)
    doc.content = doc.baseline = doc.disk = snapshot.latest
    doc.resource = doc.diskResource
    doc.conflict = false
    doc.error = ''
    doc.revision++
    return true
  }
  async function saveCopy(snapshot: DocumentReview, target: string) {
    if (!matchesReview(snapshot)) return changedReview(snapshot)
    const doc = get(snapshot.workspace, snapshot.path)
    if (
      !target.trim() ||
      target === snapshot.path ||
      parsePromptFile(target) ||
      snapshot.resource
    ) {
      doc.error = 'Choose a different ordinary file path for this copy.'
      return false
    }
    const result = await writeDocument(snapshot.workspace, target, snapshot.local, undefined, {
      createOnly: true,
    })
    if (!result.ok) {
      doc.error = result.error
      return false
    }
    doc.error = ''
    return true
  }
  function markDeleted(workspace: string, path: string) {
    for (const doc of records.values()) {
      if (doc.workspace === workspace && (doc.path === path || doc.path.startsWith(path + '/'))) {
        invalidateSync(doc)
        doc.deleted = true
      }
    }
  }
  function discard(workspace: string, path: string) {
    const doc = get(workspace, path)
    invalidateSync(doc)
    doc.content = doc.baseline = doc.disk
    doc.conflict = false
    doc.resource = doc.diskResource
    doc.revision++
  }
  function adoptReviewedResource(workspace: string, path: string, reviewed: ResourceRef) {
    const doc = get(workspace, path),
      latest = doc.diskResource
    if (
      !latest ||
      latest.reference.id !== reviewed.id ||
      latest.reference.revision !== reviewed.revision ||
      latest.reference.content_digest !== reviewed.content_digest
    )
      return false
    invalidateSync(doc)
    doc.resource = latest
    doc.baseline = doc.disk
    doc.conflict = false
    doc.error = ''
    return true
  }
  function forget(workspace: string, path: string) {
    records.delete(key(workspace, path))
  }
  function rename(workspace: string, from: string, to: string) {
    recovery.renamed(workspace, from, to)
    for (const [id, doc] of [...records]) {
      if (doc.workspace !== workspace || (doc.path !== from && !doc.path.startsWith(from + '/')))
        continue
      records.delete(id)
      doc.path = to + doc.path.slice(from.length)
      records.set(key(workspace, doc.path), doc)
    }
  }
  return {
    records,
    get,
    peek,
    open,
    edit,
    sync,
    save,
    review,
    matchesReview,
    saveReviewed,
    useReviewedDisk,
    saveCopy,
    protectWorkspaceDrafts,
    dirty,
    markDeleted,
    discard,
    adoptReviewedResource,
    forget,
    rename,
    recoveryKind,
  }
})
