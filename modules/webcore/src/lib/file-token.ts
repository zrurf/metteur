import { hash } from 'ohash'
import { baseUrl } from '@/lib/workspace-url'

/**
 * File URLs use a stable XXH3 hash of the workspace-relative path carried as a
 * query param (`/work/{id}/file?f=<hash>`) instead of an opaque random token.
 *
 * The hash is deterministic — the same path always maps to the same URL — and
 * the reverse `hash → path` mapping is persisted in `localStorage`, so the
 * editor restores correctly even after a hard refresh. The URL never embeds
 * the raw path, keeping the anti-path-traversal property of the old scheme.
 */

const FILE_HASHES_KEY = 'metteur.file-hashes'

/** Stable, URL-safe short XXH3 hash of a workspace-relative path. */
function hashOf(path: string): string {
  const norm = path.replace(/[\\/]+/g, '/')
  return `f${hash(norm).slice(0, 11)}`
}

function rememberHash(hash_type: string, path: string) {
  try {
    const map = JSON.parse(localStorage.getItem(FILE_HASHES_KEY) ?? '{}')
    map[hash_type] = path.replaceAll('\\', '/')
    localStorage.setItem(FILE_HASHES_KEY, JSON.stringify(map))
  } catch {
    // Storage unavailable: the mapping just won't survive reloads.
  }
}

/** Stable, URL-safe hash of a workspace-relative file path. */
export function fileHashOf(path: string): string {
  const hash_type = hashOf(path)
  rememberHash(hash_type, path)
  return hash_type
}

/** Build the editor route for a file path (`/work/{id}/file?f=<hash>`). */
export function fileRoute(path: string): string {
  return `${baseUrl()}/file?f=${fileHashOf(path)}`
}

/** Resolve the `f` query param back to the remembered path ('' if unknown). */
export function pathByFileParam(f: string): string {
  if (!f) return ''
  try {
    const map = JSON.parse(localStorage.getItem(FILE_HASHES_KEY) ?? '{}')
    return typeof map[f] === 'string' ? map[f].replaceAll('\\', '/') : ''
  } catch {
    return ''
  }
}
