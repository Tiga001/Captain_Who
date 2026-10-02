import type { ChatComposerDraft } from '../features/chat/chatTypes'

export type WorkflowDraftPreferences = Pick<ChatComposerDraft, 'modelId' | 'permissionMode'>

/** Change only fields still matching the live values captured before the asynchronous refresh. */
export function mergeStoredWorkflowPreferences(
  current: ChatComposerDraft,
  stored: Partial<WorkflowDraftPreferences>,
  expected: Partial<WorkflowDraftPreferences>
): ChatComposerDraft {
  const modelId =
    Object.hasOwn(expected, 'modelId') &&
    stored.modelId !== undefined &&
    current.modelId === expected.modelId
      ? stored.modelId
      : current.modelId
  const permissionMode =
    Object.hasOwn(expected, 'permissionMode') &&
    stored.permissionMode !== undefined &&
    current.permissionMode === expected.permissionMode
      ? stored.permissionMode
      : current.permissionMode
  return modelId === current.modelId && permissionMode === current.permissionMode
    ? current
    : { ...current, modelId, permissionMode }
}
