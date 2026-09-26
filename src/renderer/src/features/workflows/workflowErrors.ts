/** Preserve the host's diagnostic message without exposing payloads, stacks, or graph data. */
export function workflowErrorDetail(error: unknown): string {
  const record =
    error && typeof error === 'object' ? (error as { message?: unknown; code?: unknown }) : null
  const message =
    typeof error === 'string' ? error : typeof record?.message === 'string' ? record.message : ''
  const normalized = Array.from(message)
    .filter((character) => {
      const code = character.charCodeAt(0)
      return (code >= 32 && code !== 127) || '\n\r\t'.includes(character)
    })
    .join('')
    .trim()
  const bounded = normalized.length > 1200 ? `${normalized.slice(0, 1200)}…` : normalized
  const code =
    typeof record?.code === 'number' && Number.isFinite(record.code) ? `[${record.code}]` : ''
  return [code, bounded].filter(Boolean).join(' ')
}
