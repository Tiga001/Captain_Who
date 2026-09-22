import type { ChatWorkspaceMention } from './chatTypes'

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
      const logicalPath = [mention.alias, mention.path]
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
