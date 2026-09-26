import { AlertTriangle } from 'lucide-react'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import type { ChatAgentRunView } from '../chatTypes'
import type { ChatGuidanceTimelineItem } from '../chatTypes'
import { stripAttachmentSummary } from '../chatAttachments'
import { ChatMarkdown } from './ChatMarkdown'
import { ComposerFolderReferences } from './ComposerFolderReferences'
import { HumanInteractionAnswerContent } from '../../humanInteraction/HumanInteractionAnswerContent'
import {
  HumanInteractionTimelineEntry,
  type HumanInteractionTimelineController
} from '../../humanInteraction/HumanInteractionTimelineEntry'
import { readHumanInteractionGuidanceDisplay } from '../../humanInteraction/humanInteractionPresentation'
import {
  getConversationHistoryGroupItems,
  getFileChangeGroupItems,
  getMcpActivityGroupItems,
  getOfficeGroupItems,
  getPreviousSuccessfulTodoResult,
  getReadGroupItems,
  getRunCommandGroupItems,
  getSearchGroupItems,
  getSendMessageGroupItems,
  getSkillResourceGroupItems,
  getSettledToolStatus,
  getToolResult,
  getWebActivityGroupItems,
  isRunSettled,
  type RenderableTimelineItem
} from './chatMessageItemUtils'
import { AgentToolActivity } from './toolActivities/AgentToolActivity'
import { McpToolActivity, McpToolActivityGroup } from './toolActivities/McpToolActivity'
import { ContextCompactionActivity } from './toolActivities/ContextCompactionActivity'
import { ConversationHistoryToolActivity } from './toolActivities/ConversationHistoryToolActivity'
import { FileChangeToolActivityGroup } from './toolActivities/FileChangeToolActivity'
import { ReadToolActivityGroup } from './toolActivities/ReadToolActivity'
import { RunCommandToolActivityGroup } from './toolActivities/RunCommandToolActivity'
import { SendMessageToolActivityGroup } from './toolActivities/SendMessageToolActivity'
import { OfficeToolActivityGroup } from './toolActivities/OfficeToolActivity'
import { SearchToolActivityGroup } from './toolActivities/SearchToolActivity'
import { WebSearchToolActivityGroup } from './toolActivities/WebSearchToolActivity'
import { SkillLoadActivity, SkillResourceActivityGroup } from './toolActivities/SkillToolActivity'
import type { WorkspaceReferenceTarget } from '../workspaceMentions'
import { MessageAttachments } from './MessageAttachments'

export function GuidanceTimelineItemView({
  item,
  mode,
  onOpenWorkspaceReference,
  projectId
}: {
  item: ChatGuidanceTimelineItem
  mode: 'interactive' | 'observer'
  onOpenWorkspaceReference?: (target: WorkspaceReferenceTarget) => void
  projectId?: string | null
}) {
  const { t } = useFrontendConfig()
  const humanAnswer = readHumanInteractionGuidanceDisplay(item)
  const content = stripAttachmentSummary(item.content, item.attachments)
  const statusLabel =
    item.status === 'submitting'
      ? t('chat.guidanceSubmittingStatus')
      : item.status === 'queued'
        ? t('chat.guidanceQueued')
        : item.status === 'rejected'
          ? t('chat.guidanceInterrupted')
          : null

  return (
    <div
      className="chat-guidance"
      data-status={item.status}
      title={item.status === 'rejected' ? item.error : undefined}
    >
      {(item.folderReferences?.length ?? 0) > 0 && (
        <ComposerFolderReferences
          folders={item.folderReferences ?? []}
          label={t('chat.attachments')}
          removeLabel=""
        />
      )}
      {item.attachments.length > 0 && (
        <MessageAttachments attachments={item.attachments} messageId={item.id} mode={mode} />
      )}
      {(humanAnswer || content) && (
        <div className="chat-guidance__bubble">
          {humanAnswer ? (
            <HumanInteractionAnswerContent display={humanAnswer} />
          ) : (
            <ChatMarkdown
              content={content}
              onOpenWorkspaceReference={onOpenWorkspaceReference}
              projectId={projectId}
            />
          )}
        </div>
      )}
      {!humanAnswer && statusLabel && <span className="chat-guidance__status">{statusLabel}</span>}
    </div>
  )
}

export function AgentTimelineItemView({
  assistantMessageId,
  conversationId,
  humanInteraction,
  item,
  mode,
  onOpenWorkspaceReference,
  observerRootConversationId,
  projectId,
  run
}: {
  assistantMessageId: string
  conversationId?: string
  humanInteraction?: HumanInteractionTimelineController
  item: RenderableTimelineItem
  mode: 'interactive' | 'observer'
  onOpenWorkspaceReference?: (target: WorkspaceReferenceTarget) => void
  observerRootConversationId?: string
  projectId?: string | null
  run: ChatAgentRunView
}) {
  if (item.type === 'skill_load_group') {
    return <SkillLoadActivity run={run} />
  }

  if (item.type === 'skill_resource_group') {
    const items = getSkillResourceGroupItems(run, item.callIds)
    if (items.length === 0) return null
    return <SkillResourceActivityGroup items={items} kind={item.kind} />
  }

  if (item.type === 'office_group') {
    const items = getOfficeGroupItems(run, item.callIds)
    if (items.length === 0) return null
    return <OfficeToolActivityGroup items={items} run={run} />
  }

  if (item.type === 'read_group') {
    const items = getReadGroupItems(run, item.callIds)
    if (items.length === 0) return null
    return (
      <ReadToolActivityGroup
        assistantMessageId={assistantMessageId}
        conversationId={conversationId}
        items={items}
        observerRootConversationId={observerRootConversationId}
        projectId={projectId}
      />
    )
  }

  if (item.type === 'search_group') {
    const items = getSearchGroupItems(run, item.callIds)
    if (items.length === 0) return null
    return <SearchToolActivityGroup items={items} kind={item.kind} />
  }

  if (item.type === 'web_activity_group') {
    const items = getWebActivityGroupItems(run, item.callIds)
    if (items.length === 0) return null
    return <WebSearchToolActivityGroup items={items} />
  }

  if (item.type === 'run_command_group') {
    const items = getRunCommandGroupItems(run, item.callIds)
    if (items.length === 0) return null
    return <RunCommandToolActivityGroup items={items} />
  }

  if (item.type === 'send_message_group') {
    return <SendMessageToolActivityGroup items={getSendMessageGroupItems(run, item.callIds)} />
  }

  if (item.type === 'mcp_activity_group') {
    const items = getMcpActivityGroupItems(run, item.invocationIds)
    if (items.length === 0) return null
    return <McpToolActivityGroup items={items} />
  }

  if (item.type === 'file_change_group') {
    const items = getFileChangeGroupItems(run, item.callIds)
    if (items.length === 0) return null
    return (
      <FileChangeToolActivityGroup
        assistantMessageId={assistantMessageId}
        conversationId={conversationId}
        items={items}
        observerRootConversationId={observerRootConversationId}
        projectId={projectId}
        runId={run.runId ?? undefined}
      />
    )
  }

  if (item.type === 'conversation_history_group') {
    const items = getConversationHistoryGroupItems(run, item.callIds)
    if (items.length === 0) return null
    return <ConversationHistoryToolActivity items={items} />
  }

  if (item.type === 'context_compaction') {
    return <ContextCompactionActivity status={item.status} />
  }

  if (item.type === 'message') {
    if (!item.content.trim()) return null
    return <ChatMarkdown className="chat-agent-text" content={item.content} />
  }

  if (item.type === 'user_guidance') {
    return (
      <GuidanceTimelineItemView
        item={item}
        mode={mode}
        onOpenWorkspaceReference={onOpenWorkspaceReference}
        projectId={projectId}
      />
    )
  }

  if (item.type === 'mcp_tool_call') {
    const invocation = run.mcpInvocations?.find(
      (candidate) => candidate.invocationId === item.invocationId
    )
    return <McpToolActivity invocation={invocation} />
  }

  if (item.type === 'tool_call') {
    const call = run.toolCalls.find((candidate) => candidate.id === item.callId)
    if (!call) return null
    if (call.tool === 'request_user_input' || call.tool === 'request_user_input_async') {
      const request = humanInteraction?.openRequests.find(
        (candidate) =>
          candidate.status === 'open' &&
          candidate.toolCallId === call.id &&
          candidate.runId === run.runId &&
          candidate.assistantMessageId === assistantMessageId
      )
      return request && humanInteraction ? (
        <HumanInteractionTimelineEntry request={request} interaction={humanInteraction} />
      ) : null
    }
    const mcpInvocation = run.mcpInvocations?.find((candidate) => candidate.callId === call.id)
    const webActivity = run.webSearchActivities?.find((candidate) => candidate.callId === call.id)
    const readActivity = run.readActivities?.find((candidate) => candidate.callId === call.id)
    const fileChangeProposal =
      call.tool === 'apply_patch'
        ? run.fileChangeProposals.find((candidate) => candidate.id === call.id)
        : undefined
    const result = getToolResult(run, call.id)
    const previousTodoResult =
      call.tool === 'todo_update' ? getPreviousSuccessfulTodoResult(run, call.id) : undefined
    const settledStatus = getSettledToolStatus(run, result)
    return (
      <AgentToolActivity
        assistantMessageId={assistantMessageId}
        cancelled={settledStatus === 'cancelled'}
        call={call}
        conversationId={conversationId}
        fileChangeProposal={fileChangeProposal}
        mcpInvocation={mcpInvocation}
        observerRootConversationId={observerRootConversationId}
        projectId={projectId}
        previousTodoResult={previousTodoResult}
        readActivity={readActivity}
        result={result}
        run={run}
        settledStatus={settledStatus}
        showImageGenerationPreview={!isRunSettled(run)}
        toolIdentity={item.identity}
        webActivity={webActivity}
      />
    )
  }

  return (
    <div className="agent-activity agent-activity--error">
      <AlertTriangle aria-hidden="true" />
      <span>{item.message}</span>
    </div>
  )
}
