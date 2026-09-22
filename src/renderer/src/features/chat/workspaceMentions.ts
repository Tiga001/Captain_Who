import type { ChatWorkspaceMention } from './chatTypes'

export interface WorkspaceReferenceTarget {
  projectId: string
  alias: string
  path: string
  kind: 'file' | 'directory'
}

export function workspaceReferenceTargetFromMention(
  mention: Pick<ChatWorkspaceMention, 'projectId' | 'alias' | 'path' | 'kind'>
): WorkspaceReferenceTarget {
  return {
    projectId: mention.projectId,
    alias: mention.alias,
    path: mention.path,
    kind: mention.kind
  }
}

/** Reads the compact @workspace links stored in user message Markdown. */
export function parseWorkspaceReferenceTarget(
  href: string | undefined,
  projectId: string | null | undefined
): WorkspaceReferenceTarget | null {
  if (!href || !projectId || !href.startsWith('@workspace/')) return null
  const logicalPath = href.slice('@workspace/'.length).replaceAll('\\', '/')
  const separator = logicalPath.indexOf('/')
  if (separator <= 0) return null
  const alias = logicalPath.slice(0, separator)
  const path = logicalPath.slice(separator + 1)
  if (!alias || !path || path.includes('\0')) return null
  return { projectId, alias, path, kind: path.endsWith('/') ? 'directory' : 'file' }
}

/**
 * Keep @ workspace selections in durable message content until the transport has a dedicated
 * rich-text field. The logical workspace URL contains no native path and is resolved only by
 * workspace tools; Markdown keeps the reference visible and selectable in history.
 */
export function buildMessageContentWithWorkspaceMentions(
  content: string,
  mentions: readonly ChatWorkspaceMention[] = []
): string {
  const body = content.trim()
  const references = mentions
    .map((mention) => {
      const mentionPath =
        mention.kind === 'directory' ? `${mention.path.replace(/\/+$/, '')}/` : mention.path
      const logicalPath = [mention.alias, mentionPath]
        .filter(Boolean)
        .map((part) => part.replace(/[\\[\]<>]/g, ''))
        .join('/')
      const label = mention.displayName.replace(/[\\[\]<>]/g, '')
      if (body.includes(`](@workspace/${logicalPath})`)) return ''
      return `[${label}](@workspace/${logicalPath})`
    })
    .filter(Boolean)
  if (!body) return references.join('\n')
  return references.length > 0 ? `${body}\n\n${references.join('\n')}` : body
}
