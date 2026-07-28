import type { AgentDiffProposal, AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import type { ReactElement } from 'react'
import type { ChatReadActivity, ChatWebSearchActivity } from '../../chatTypes'
import type { ChatAgentRunView } from '../../chatTypes'
import { isReadActivityTool } from '../../agentReadActivities'
import { AttachmentListToolActivity } from './AttachmentListToolActivity'
import { ApplyPatchToolActivity } from './ApplyPatchToolActivity'
import { ConversationHistoryToolActivity } from './ConversationHistoryToolActivity'
import { GenericToolActivity } from './GenericToolActivity'
import { GitDiffToolActivity } from './GitDiffToolActivity'
import { ReadToolActivity } from './ReadToolActivity'
import { RunCommandToolActivity } from './RunCommandToolActivity'
import { SearchToolActivity } from './SearchToolActivity'
import { TodoUpdateToolActivity } from './TodoUpdateToolActivity'
import { WebSearchToolActivity } from './WebSearchToolActivity'
import { WorkspaceMapToolActivity } from './WorkspaceMapToolActivity'
import { OfficeToolActivity } from './OfficeToolActivity'
import { ImageGenerationToolActivity } from './ImageGenerationToolActivity'
import { SkillScriptToolActivity } from './SkillToolActivity'
import type { SettledToolStatus } from './toolActivityUtils'

interface AgentToolActivityProps {
  cancelled?: boolean
  readActivity?: ChatReadActivity
  webActivity?: ChatWebSearchActivity
  call: AgentToolCall
  diff?: AgentDiffProposal
  projectId?: string | null
  previousTodoResult?: AgentToolResult
  result?: AgentToolResult
  run: ChatAgentRunView
  settledStatus?: SettledToolStatus
  showImageGenerationPreview: boolean
}

export function AgentToolActivity({
  cancelled = false,
  readActivity,
  webActivity,
  call,
  diff,
  projectId,
  previousTodoResult,
  result,
  run,
  settledStatus,
  showImageGenerationPreview
}: AgentToolActivityProps): ReactElement {
  if (call.tool === 'attachments_list' || call.tool === 'attachments_list_project') {
    return (
      <AttachmentListToolActivity
        cancelled={cancelled && !result}
        call={call}
        result={result}
        settledStatus={settledStatus}
      />
    )
  }

  if (call.tool === 'web_search' || call.tool === 'web_fetch') {
    return (
      <WebSearchToolActivity
        activity={webActivity}
        call={call}
        result={result}
        settledStatus={settledStatus}
      />
    )
  }

  if (isReadActivityTool(call.tool)) {
    return (
      <ReadToolActivity
        activity={readActivity}
        call={call}
        projectId={projectId}
        result={result}
        settledStatus={settledStatus}
      />
    )
  }

  if (call.tool === 'workspace_map') {
    return (
      <WorkspaceMapToolActivity
        cancelled={cancelled && !result}
        result={result}
        settledStatus={settledStatus}
      />
    )
  }

  if (call.tool === 'search_files' || call.tool === 'search_code') {
    return (
      <SearchToolActivity
        cancelled={cancelled && !result}
        call={call}
        result={result}
        settledStatus={settledStatus}
      />
    )
  }

  if (call.tool === 'git_diff') {
    return (
      <GitDiffToolActivity
        cancelled={cancelled && !result}
        call={call}
        result={result}
        settledStatus={settledStatus}
      />
    )
  }

  if (call.tool === 'run_command') {
    return (
      <RunCommandToolActivity
        cancelled={cancelled && !result}
        call={call}
        liveOutput={run.commandOutputPreviews?.[call.id]}
        result={result}
        settledStatus={settledStatus}
      />
    )
  }

  if (call.tool === 'apply_patch') {
    return (
      <ApplyPatchToolActivity
        cancelled={cancelled && !result}
        call={call}
        diff={diff}
        projectId={projectId}
        result={result}
        settledStatus={settledStatus}
      />
    )
  }

  if (call.tool === 'todo_update') {
    return (
      <TodoUpdateToolActivity
        cancelled={cancelled && !result}
        previousResult={previousTodoResult}
        result={result}
        settledStatus={settledStatus}
      />
    )
  }

  if (call.tool === 'conversation_history') {
    return (
      <ConversationHistoryToolActivity
        items={[{ call, result, settledStatus: cancelled ? 'cancelled' : settledStatus }]}
      />
    )
  }

  if (
    call.tool === 'office_document' ||
    call.tool === 'office_spreadsheet' ||
    call.tool === 'office_presentation'
  ) {
    return <OfficeToolActivity call={call} run={run} settledStatus={settledStatus} />
  }

  if (call.tool === 'skills_run_script') {
    return <SkillScriptToolActivity callId={call.id} run={run} settledStatus={settledStatus} />
  }

  if (call.tool === 'image_generation') {
    return (
      <ImageGenerationToolActivity
        call={call}
        result={result}
        settledStatus={settledStatus}
        showArtifactPreview={showImageGenerationPreview}
      />
    )
  }

  return (
    <GenericToolActivity
      cancelled={cancelled && !result}
      call={call}
      result={result}
      settledStatus={settledStatus}
    />
  )
}
