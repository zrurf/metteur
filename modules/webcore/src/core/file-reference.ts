import type { SavedFileReference } from './types'
export const validFileReference = (value: SavedFileReference | undefined, path: string) =>
  !!value &&
  value.path === path &&
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(value.snapshotId) &&
  /^[0-9a-f]{64}$/i.test(value.digest)
export async function fileTextDigest(raw: string) {
  const bytes = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(raw))
  return [...new Uint8Array(bytes)].map((byte) => byte.toString(16).padStart(2, '0')).join('')
}
