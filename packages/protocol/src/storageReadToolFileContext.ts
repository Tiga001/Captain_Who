export const STORAGE_LOAD_READ_TOOL_FILE_CONTEXT_METHOD = 'storage.loadReadToolFileContext'

export interface StorageReadToolFileContextRequest {
  conversationId: string
  assistantMessageId: string
  callId: string
}

/** A single-call projection from one Core read snapshot, never a file-access grant. */
export interface StorageReadToolFileContext {
  conversationId: string
  projectId: string | null
  assistantMessageId: string
  messageFound: boolean
  runId: string | null
  call: { id: string; tool: string | null; path: string | null; filePath: string | null } | null
  /** Null means a legacy receipt had no identity; malformed explicit identities stay present. */
  callIdentity: { type: string | null; toolName: string | null } | null
  resultPath: string | null
}

export function parseStorageReadToolFileContext(value: unknown): StorageReadToolFileContext | null {
  if (value === null) return null
  const object = (input: unknown): input is Record<string, unknown> =>
    input !== null && typeof input === 'object' && !Array.isArray(input)
  const nullableString = (input: unknown): input is string | null =>
    input === null || typeof input === 'string'
  if (
    !object(value) ||
    typeof value.conversationId !== 'string' ||
    !nullableString(value.projectId) ||
    typeof value.assistantMessageId !== 'string' ||
    typeof value.messageFound !== 'boolean' ||
    !nullableString(value.runId) ||
    !nullableString(value.resultPath) ||
    !(
      value.call === null ||
      (object(value.call) &&
        typeof value.call.id === 'string' &&
        nullableString(value.call.tool) &&
        nullableString(value.call.path) &&
        nullableString(value.call.filePath))
    ) ||
    !(
      value.callIdentity === null ||
      (object(value.callIdentity) &&
        nullableString(value.callIdentity.type) &&
        nullableString(value.callIdentity.toolName))
    )
  )
    throw new Error('Invalid read tool file context')
  return value as unknown as StorageReadToolFileContext
}
