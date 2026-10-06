import type { ChatAgentRunView } from '../../chatTypes'
import type { WorkspaceReferenceTarget } from '../../workspaceMentions'
import { getBasicToolRepresentative, type BasicToolItem } from '../basicToolTimeline'
import { getFileChangeGroupItems, getSettledToolStatus } from '../chatMessageItemUtils'
import { AttachmentListToolActivity } from './AttachmentListToolActivity'
import { ConversationHistoryToolActivity } from './ConversationHistoryToolActivity'
import { FileChangeRow } from './FileChangeToolActivity'
import { ReadToolActivity } from './ReadToolActivity'
import { RunCommandToolActivity } from './RunCommandToolActivity'
import { SearchToolActivity } from './SearchToolActivity'
import { WorkspaceMapToolActivity } from './WorkspaceMapToolActivity'

export interface BasicToolActivityItemProps {
  assistantMessageId?: string
  conversationId?: string
  item: BasicToolItem
  mode?: 'interactive' | 'observer'
  observerRootConversationId?: string
  onExpandedChange?: (open: boolean) => void
  onOpenWorkspaceReference?: (target: WorkspaceReferenceTarget) => void
  presentation?: 'default' | 'compact'
  projectId?: string | null
  run: ChatAgentRunView
}

/** One logical activity. Its component and first-call anchor stay stable as a span grows. */
export function BasicToolActivityItem({
  assistantMessageId,
  conversationId,
  item,
  observerRootConversationId,
  onExpandedChange,
  onOpenWorkspaceReference,
  presentation = 'default',
  projectId,
  run
}: BasicToolActivityItemProps) {
  const representative = getBasicToolRepresentative(run, item)
  if (!representative) return null
  const { call, result } = representative
  const settledStatus = getSettledToolStatus(run, result)
  const shared = {
    call,
    result,
    settledStatus,
    cancelled: settledStatus === 'cancelled',
    onExpandedChange,
    presentation
  }

  switch (item.category) {
    case 'read':
      return (
        <ReadToolActivity
          {...shared}
          activity={run.readActivities?.find((activity) => activity.callId === call.id)}
          assistantMessageId={assistantMessageId}
          conversationId={conversationId}
          observerRootConversationId={observerRootConversationId}
          onOpenWorkspaceReference={onOpenWorkspaceReference}
          projectId={projectId}
        />
      )
    case 'command':
      return (
        <RunCommandToolActivity
          {...shared}
          liveOutput={run.commandOutputPreviews?.[call.id]}
          session={run.commandSessions?.[call.id]}
        />
      )
    case 'edit': {
      const edit = getFileChangeGroupItems(run, item.callIds)[0]
      if (!edit) return null
      return (
        <div className="agent-activity agent-activity--basic-file-change">
          <FileChangeRow
            assistantMessageId={assistantMessageId}
            conversationId={conversationId}
            item={edit}
            key={item.id}
            observerRootConversationId={observerRootConversationId}
            onExpandedChange={onExpandedChange}
            projectId={projectId}
            runId={run.runId ?? undefined}
            showIcon
          />
        </div>
      )
    }
    case 'search':
      return <SearchToolActivity {...shared} />
    case 'attachments':
      return <AttachmentListToolActivity {...shared} />
    case 'workspace':
      return <WorkspaceMapToolActivity {...shared} />
    case 'history':
      return <ConversationHistoryToolActivity items={[{ call, result, settledStatus }]} />
  }
}
