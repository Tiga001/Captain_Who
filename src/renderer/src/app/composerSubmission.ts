import type { ChatComposerDraft, ChatSubmitOptions } from '../features/chat/chatTypes'
import { buildMessageContentWithAttachments } from './appShellConversationUtils'
import { buildMessageContentWithWorkspaceMentions } from '../features/chat/workspaceMentions'
import { createId } from './chatMessageFactory'

export function consumeSubmittedDraft(
  current: ChatComposerDraft,
  submitted: ChatComposerDraft,
  options: ChatSubmitOptions
): ChatComposerDraft {
  // A new version may contain newly typed text identical to the submitted text.
  if (current.updatedAt !== submitted.updatedAt) return current
  return {
    ...current,
    message: '',
    attachments: [],
    folderReferences: [],
    workspaceMentions: [],
    skills: [],
    modelId: options.modelId,
    permissionMode: options.permissionMode,
    projectId: options.projectId,
    updatedAt: Math.max(Date.now(), current.updatedAt + 1)
  }
}

export function restoreRejectedDraft(
  current: ChatComposerDraft,
  submitted: ChatComposerDraft,
  consumed: boolean,
  options: ChatSubmitOptions
): ChatComposerDraft {
  const hasNewContent =
    (consumed || current.updatedAt !== submitted.updatedAt) &&
    (current.message.trim().length > 0 ||
      current.attachments.length > 0 ||
      (current.folderReferences?.length ?? 0) > 0 ||
      (current.workspaceMentions?.length ?? 0) > 0 ||
      current.skills.length > 0)
  const now = Date.now()
  return {
    ...submitted,
    modelId: options.modelId,
    permissionMode: options.permissionMode,
    projectId: options.projectId,
    queuedMessages: hasNewContent
      ? [
          ...current.queuedMessages,
          {
            id: createId('queued-message'),
            clientMessageId: createId('guidance'),
            content: buildMessageContentWithWorkspaceMentions(
              buildMessageContentWithAttachments(current.message, current.attachments),
              current.workspaceMentions ?? []
            ),
            attachments: current.attachments,
            folderReferences: current.folderReferences ?? [],
            workspaceMentions: current.workspaceMentions ?? [],
            skills: current.skills,
            modelId: current.modelId,
            permissionMode: current.permissionMode,
            projectId: current.projectId,
            status: 'pending',
            createdAt: now
          }
        ]
      : current.queuedMessages,
    updatedAt: Math.max(now, current.updatedAt + 1)
  }
}
