import { gateway, type Result, type FileContent } from '@/core'
import type { PromptEntry } from '@/core/resources'

function textConstraint(content: string) {
  return /\r(?!\n)/.test(content) || (/\r\n/.test(content) && /(?<!\r)\n/.test(content))
    ? 'This file uses mixed or unsupported line endings. Read it here and edit it externally to preserve its original bytes.'
    : undefined
}

/** Resource-backed Markdown uses the same editor, with the resource store's CAS. */
export const promptFilePath = (entry: PromptEntry) =>
  `@prompt/${entry.reference.scope.scope}/${entry.reference.id}.md`
export function parsePromptFile(path: string) {
  const match = /^@prompt\/(project|personal)\/([a-zA-Z0-9-]+)\.md$/.exec(path)
  return match ? { scope: match[1]!, id: match[2]! } : null
}
export async function readDocument(
  workspace: string,
  path: string,
): Promise<Result<FileContent & { resource?: PromptEntry }>> {
  const prompt = parsePromptFile(path)
  if (!prompt) {
    const result = await gateway.readFile(workspace, path)
    return result.ok
      ? ({
          ok: true,
          data: {
            ...result.data,
            readOnlyReason: path.endsWith('.blueprint')
              ? undefined
              : textConstraint(result.data.content),
          },
        } as const)
      : result
  }
  const result = await gateway.resourceRequest('queryPromptResources', workspace, {
    operation: 'read',
    ...prompt,
  })
  if (!result.ok) return result
  const resource = result.data as PromptEntry
  return {
    ok: true,
    data: {
      content: resource.content.body,
      language: 'markdown',
      resource,
      readOnlyReason: textConstraint(resource.content.body),
    },
  }
}
export async function writeDocument(
  workspace: string,
  path: string,
  content: string,
  resource?: PromptEntry,
  options?: { expectedContent?: string; createOnly?: boolean },
): Promise<Result<PromptEntry | undefined>> {
  if (!parsePromptFile(path)) {
    const result = await gateway.writeFile(workspace, path, content, options)
    return result.ok ? { ok: true, data: undefined } : result
  }
  if (!resource) return { ok: false, error: 'Reload the prompt source before saving.' }
  const result = await gateway.resourceRequest(
    'writePromptResource',
    workspace,
    {
      scope: resource.reference.scope.scope,
      draft: {
        id: resource.content.header.id,
        name: resource.content.header.name,
        tags: resource.content.header.tags,
        body: content,
      },
    },
    resource.reference.revision,
  )
  return result.ok ? { ok: true, data: result.data as PromptEntry } : result
}
