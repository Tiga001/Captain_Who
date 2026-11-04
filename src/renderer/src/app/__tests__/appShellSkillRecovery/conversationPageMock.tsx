import type {
  AgentProposedAction,
  AgentApprovalScope,
  AgentProviderTransitionOperation,
  StorageConversationForkPoint
} from '@mycopilot/protocol'
import { vi } from 'vitest'
import type {
  ChatComposerDraft,
  ChatConversation,
  ChatConversationContinuationOrigin,
  ChatQueuedMessage,
  ChatSubmitOptions
} from '../../../features/chat/chatTypes'
import type { ComposerCommand } from '../../../features/chat/components/ComposerCommands'
import { skillSelection, latestSkillSelection } from './turnFixtures'
import { queuedMessage } from './conversationFixtures'

vi.mock('../../../features/chat/ChatConversationPage', () => ({
  ChatConversationPage: ({
    messagePredictionsEnabled,
    commands,
    composerDraft,
    conversation,
    onComposerDraftChange,
    onApproveAgentAction,
    onRejectAgentAction,
    onCancelAgentAction,
    onContinueInNewTask,
    onEditLastUserMessage,
    onGuideQueuedMessage,
    onToggleQueueAutoSend,
    queueAutoSendEnabled,
    onModelTransitionCancel,
    onModelTransitionConfirm,
    onModelTransitionRetry,
    onOpenContinuationOrigin,
    onStopGenerating,
    onResumeGenerating,
    canResume,
    isResumeStarting,
    onSubmitMessage,
    modelTransitionConfirmation,
    modelTransitionOperations,
    scrollTargetMessageId,
    skillCatalogRefreshToken
  }: {
    messagePredictionsEnabled?: boolean
    commands?: readonly ComposerCommand[]
    composerDraft: ChatComposerDraft
    conversation: ChatConversation
    onApproveAgentAction?: (
      messageId: string,
      action: AgentProposedAction,
      scope: AgentApprovalScope
    ) => unknown
    onRejectAgentAction?: (messageId: string, action: AgentProposedAction) => unknown
    onCancelAgentAction?: (messageId: string, action: AgentProposedAction) => unknown
    onComposerDraftChange: (draft: ChatComposerDraft) => void
    onContinueInNewTask?: (forkPoint: StorageConversationForkPoint) => void
    onEditLastUserMessage: (messageId: string, content: string) => Promise<void>
    onGuideQueuedMessage?: (message: ChatQueuedMessage) => void
    onToggleQueueAutoSend?: () => void
    queueAutoSendEnabled?: boolean
    onModelTransitionCancel?: () => void
    onModelTransitionConfirm?: () => void
    onModelTransitionRetry?: (operation: AgentProviderTransitionOperation) => void
    onOpenContinuationOrigin?: (origin: ChatConversationContinuationOrigin) => void
    onStopGenerating: () => void
    onResumeGenerating?: () => void | Promise<void>
    canResume?: boolean
    isResumeStarting?: boolean
    onSubmitMessage: (message: string, options: ChatSubmitOptions) => void | Promise<boolean | void>
    modelTransitionConfirmation?: { reason: string }
    modelTransitionOperations?: AgentProviderTransitionOperation[]
    scrollTargetMessageId?: string | null
    skillCatalogRefreshToken?: number
  }) => (
    <div>
      {commands?.map((command) => (
        <button
          key={command.id}
          type="button"
          disabled={Boolean(command.disabledReason)}
          title={command.disabledReason}
          onClick={() => {
            if (command.id !== 'capabilities' && command.id !== 'model') void command.execute()
          }}
        >
          command-{command.id}
        </button>
      ))}
      <output data-testid="draft-skills">
        {composerDraft.skills.map((selection) => `${selection.id}@${selection.revision}`).join(',')}
      </output>
      <output data-testid="skill-catalog-refresh-token">{skillCatalogRefreshToken ?? 0}</output>
      <output data-testid="last-assistant-status">
        {conversation.messages.at(-1)?.status ?? ''}
      </output>
      <output data-testid="agent-run-status">
        {conversation.messages.at(-1)?.agentRun?.status ?? ''}
      </output>
      <output data-testid="conversation-message-ids">
        {conversation.messages.map((message) => message.id).join(',')}
      </output>
      <output data-testid="conversation-message-contents">
        {conversation.messages.map((message) => message.content).join('|')}
      </output>
      {(['approve', 'reject', 'cancel'] as const).map((operation) => (
        <button
          key={operation}
          onClick={() => {
            const message = conversation.messages.at(-1)
            const action = message?.agentRun?.approvals[0]
            if (!message || !action) return
            if (operation === 'approve')
              void onApproveAgentAction?.(message.id, action, 'singleAction')
            if (operation === 'reject') void onRejectAgentAction?.(message.id, action)
            if (operation === 'cancel') void onCancelAgentAction?.(message.id, action)
          }}
        >
          {operation}-pending-action
        </button>
      ))}
      <output data-testid="agent-run-projection">
        {JSON.stringify(conversation.messages.at(-1)?.agentRun)}
      </output>
      <output data-testid="approval-count">
        {conversation.messages.at(-1)?.agentRun?.approvals.length ?? 0}
      </output>
      <output
        data-testid="active-conversation-id"
        data-prediction-enabled={Boolean(messagePredictionsEnabled)}
      >
        {conversation.id}
      </output>
      <output data-testid="scroll-target-message-id">{scrollTargetMessageId ?? 'none'}</output>
      <output data-testid="conversation-updated-at">{conversation.updatedAt}</output>
      <output data-testid="draft-model-id">{composerDraft.modelId}</output>
      <output data-testid="draft-updated-at">{composerDraft.updatedAt}</output>
      <output data-testid="draft-message">{composerDraft.message}</output>
      <output data-testid="draft-payload">
        {JSON.stringify({
          attachments: composerDraft.attachments,
          message: composerDraft.message,
          modelId: composerDraft.modelId,
          permissionMode: composerDraft.permissionMode,
          projectId: composerDraft.projectId,
          skills: composerDraft.skills
        })}
      </output>
      <output data-testid="model-transition-confirmation">
        {modelTransitionConfirmation?.reason ?? ''}
      </output>
      <output data-testid="model-transition-operations">
        {(modelTransitionOperations ?? [])
          .map((operation) => `${operation.operationId}:${operation.status}`)
          .join(',')}
      </output>
      <output data-testid="activated-skill-ids">
        {conversation.messages
          .at(-1)
          ?.agentRun?.activatedSkills?.map((skill) => skill.id)
          .join(',') ?? ''}
      </output>
      <output data-testid="queue-auto-send-enabled">{String(queueAutoSendEnabled ?? false)}</output>
      <button type="button" onClick={onToggleQueueAutoSend}>
        toggle-queue-auto-send
      </button>
      <button
        type="button"
        onClick={() =>
          onComposerDraftChange({
            ...composerDraft,
            queuedMessages: [
              ...composerDraft.queuedMessages,
              queuedMessage('queue-late', 'message queued after completion', 40)
            ]
          })
        }
      >
        append-queued-message
      </button>
      <button
        type="button"
        onClick={() =>
          onComposerDraftChange({
            ...composerDraft,
            queuedMessages: [...composerDraft.queuedMessages].reverse()
          })
        }
      >
        reverse-queued-messages
      </button>
      <output data-testid="queued-message-ids">
        {composerDraft.queuedMessages.map((message) => message.id).join(',')}
      </output>
      <output data-testid="queued-message-payloads">
        {JSON.stringify(composerDraft.queuedMessages)}
      </output>
      <output data-testid="guidance-timeline">
        {conversation.messages
          .at(-1)
          ?.agentRun?.timeline.filter((item) => item.type === 'user_guidance')
          .map((item) => `${item.clientMessageId}:${item.status}`)
          .join(',') ?? ''}
      </output>
      <output data-testid="command-sessions">
        {JSON.stringify(conversation.messages.at(-1)?.agentRun?.commandSessions ?? {})}
      </output>
      <output data-testid="llm-retry">
        {JSON.stringify(conversation.messages.at(-1)?.agentRun?.llmRetry ?? {})}
      </output>
      <output data-testid="model-activity">
        {JSON.stringify(conversation.messages.at(-1)?.agentRun?.modelActivity ?? {})}
      </output>
      <output data-testid="final-answer-ready">
        {String(conversation.messages.at(-1)?.agentRun?.finalAnswerReady === true)}
      </output>
      <output data-testid="command-output">
        {conversation.messages
          .at(-1)
          ?.agentRun?.commandOutputPreviews?.['command-call']?.chunks.map((chunk) => chunk.output)
          .join('') ?? ''}
      </output>
      <button
        type="button"
        onClick={() =>
          void onEditLastUserMessage(conversation.messages.at(-2)?.id ?? '', 'edited').catch(
            () => undefined
          )
        }
      >
        edit-last-message
      </button>
      <output data-testid="resume-starting">{String(isResumeStarting ?? false)}</output>
      {canResume && (
        <button
          type="button"
          disabled={isResumeStarting}
          onClick={() => void onResumeGenerating?.()}
        >
          resume-task
        </button>
      )}
      <button type="button" onClick={onStopGenerating}>
        stop-generating
      </button>
      <button
        type="button"
        onClick={() =>
          onContinueInNewTask?.({
            kind: 'assistant_reply',
            assistantMessageId: conversation.messages.at(-1)?.id ?? ''
          })
        }
      >
        continue-in-new-task
      </button>
      {conversation.continuationOrigin && (
        <button
          type="button"
          onClick={() => onOpenContinuationOrigin?.(conversation.continuationOrigin!)}
        >
          open-continuation-origin
        </button>
      )}
      <button
        type="button"
        onClick={() => {
          const queuedMessage = composerDraft.queuedMessages[0]
          if (queuedMessage) onGuideQueuedMessage?.(queuedMessage)
        }}
      >
        guide-first-message
      </button>
      <button
        type="button"
        onClick={() => {
          const queuedMessage = composerDraft.queuedMessages[1]
          if (queuedMessage) onGuideQueuedMessage?.(queuedMessage)
        }}
      >
        guide-second-message
      </button>
      <button
        type="button"
        onClick={() =>
          onSubmitMessage('follow up', {
            modelId: composerDraft.modelId,
            permissionMode: 'full',
            projectId: 'project-a',
            skills: [skillSelection]
          })
        }
      >
        submit-with-skill
      </button>
      <button
        type="button"
        onClick={() =>
          onSubmitMessage('follow up', {
            modelId: composerDraft.modelId,
            permissionMode: 'full',
            projectId: 'project-a',
            skills: []
          })
        }
      >
        submit-without-skill
      </button>
      <button
        type="button"
        onClick={() =>
          onSubmitMessage(composerDraft.message, {
            attachments: composerDraft.attachments,
            modelId: composerDraft.modelId,
            permissionMode: composerDraft.permissionMode,
            projectId: composerDraft.projectId,
            skills: composerDraft.skills
          })
        }
      >
        submit-draft-content
      </button>
      <button
        type="button"
        onClick={() =>
          onComposerDraftChange({
            ...composerDraft,
            modelId: 'model-2',
            updatedAt: Math.max(Date.now(), composerDraft.updatedAt + 1)
          })
        }
      >
        select-model-2
      </button>
      <button
        type="button"
        onClick={() =>
          onComposerDraftChange({
            ...composerDraft,
            permissionMode: 'custom',
            updatedAt: Math.max(Date.now(), composerDraft.updatedAt + 1)
          })
        }
      >
        select-custom-permissions
      </button>
      <button type="button" onClick={onModelTransitionCancel}>
        cancel-model-transition
      </button>
      <button type="button" onClick={onModelTransitionConfirm}>
        confirm-model-transition
      </button>
      <button
        type="button"
        onClick={() => {
          const operation = modelTransitionOperations?.find(
            (candidate) => candidate.status === 'failed'
          )
          if (operation) onModelTransitionRetry?.(operation)
        }}
      >
        retry-model-transition
      </button>
      <button
        type="button"
        onClick={() =>
          onComposerDraftChange({
            ...composerDraft,
            skills: [latestSkillSelection],
            updatedAt: Date.now()
          })
        }
      >
        select-latest-skill
      </button>
      <button
        type="button"
        onClick={() =>
          onComposerDraftChange({
            ...composerDraft,
            message: 'new draft after failure',
            updatedAt: Date.now()
          })
        }
      >
        edit-composer-after-transition-failure
      </button>
    </div>
  )
}))
