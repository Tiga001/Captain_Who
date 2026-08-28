import type {
  AgentActionExecutionOutput,
  AgentApprovalStatus,
  AgentFileWritePreview,
  AgentProposedAction,
  AgentToolCall
} from '@mycopilot/protocol'
import type { ChatFileWritePreview, ChatSkillInstallationView } from '../chat/chatTypes'
import { getAgentActionId } from './agentActionUtils'
import { withActionApprovalStatus } from './actionProjection'
import { upsertById } from './agentEventReducerShared'

export function upsertFileWritePreview(
  previews: ChatFileWritePreview[],
  incoming: AgentFileWritePreview,
  receivedAt: number
): ChatFileWritePreview[] {
  const existing = previews.find((preview) => preview.previewId === incoming.previewId)
  const { contentDelta, contentOffsetBytes, ...snapshot } = incoming
  let content = existing?.content ?? ''

  if (!existing || contentOffsetBytes === 0) {
    content = contentDelta
  } else if (contentOffsetBytes === existing.generatedBytes) {
    content += contentDelta
  } else if (incoming.generatedBytes <= existing.generatedBytes) {
    return previews
  }

  return upsertById(
    previews,
    {
      ...snapshot,
      content,
      receivedAt
    },
    (preview) => preview.previewId
  )
}

export function bindFileChangePreviewsToCall(
  previews: ChatFileWritePreview[],
  call: AgentToolCall
): ChatFileWritePreview[] {
  if (call.tool !== 'apply_patch' && call.tool !== 'write_file') return previews
  const args =
    call.args && typeof call.args === 'object' && !Array.isArray(call.args)
      ? (call.args as Record<string, unknown>)
      : {}
  const transactionId =
    typeof args.transactionId === 'string'
      ? args.transactionId
      : typeof args.draftId === 'string'
        ? args.draftId
        : undefined
  const filePath = typeof args.filePath === 'string' ? args.filePath : undefined
  const directApply = call.tool === 'apply_patch' && args.action === 'apply'

  const previewIndex = previews.findIndex((preview) => {
    if (preview.toolCallId !== undefined) return false
    const matchesTransaction = transactionId !== undefined && preview.draftId === transactionId
    const matchesDirectPath = directApply && filePath !== undefined && preview.filePath === filePath
    return matchesTransaction || matchesDirectPath
  })
  if (previewIndex < 0) return previews
  return previews.map((preview, index) =>
    index === previewIndex ? { ...preview, toolCallId: call.id } : preview
  )
}

export function upsertAgentAction(actions: AgentProposedAction[], nextAction: AgentProposedAction) {
  return upsertById(actions, nextAction, getAgentActionId)
}

export function removeAgentAction(actions: AgentProposedAction[], actionId: string) {
  return actions.filter((action) => getAgentActionId(action) !== actionId)
}

function upsertSkillInstallation(
  installations: ChatSkillInstallationView[],
  next: ChatSkillInstallationView
) {
  return upsertById(installations, next, (installation) => installation.action.id)
}

export function mergeSkillInstallationApprovals(
  installations: ChatSkillInstallationView[] | undefined,
  actions: AgentProposedAction[]
) {
  return actions.reduce((current, action) => {
    if (action.type !== 'skill_installation') return current
    const existing = current.find(
      (installation) => installation.action.id === action.installation.id
    )
    return upsertSkillInstallation(current, {
      action: action.installation,
      status: existing?.status ?? 'waiting_for_approval'
    })
  }, installations ?? [])
}

export function applySkillInstallationDecision(
  installations: ChatSkillInstallationView[] | undefined,
  action: AgentProposedAction,
  decision: 'approved' | 'rejected'
) {
  if (action.type !== 'skill_installation') return installations ?? []
  const approvalStatus: AgentApprovalStatus = decision === 'approved' ? 'approved' : 'rejected'
  const approvedAction = withActionApprovalStatus(action, approvalStatus)
  if (approvedAction.type !== 'skill_installation') return installations ?? []
  return upsertSkillInstallation(installations ?? [], {
    action: approvedAction.installation,
    status: decision === 'approved' ? 'installing' : 'rejected'
  })
}

export function applySkillInstallationExecution(
  installations: ChatSkillInstallationView[] | undefined,
  execution: AgentActionExecutionOutput
) {
  const current = installations ?? []
  const existing = current.find((installation) => installation.action.id === execution.actionId)
  if (!existing) return current
  if (execution.status === 'rejected') {
    return upsertSkillInstallation(current, { ...existing, status: 'rejected' })
  }
  if (!execution.toolResult) {
    const status = execution.status === 'approved' ? 'installing' : 'failed'
    return upsertSkillInstallation(current, { ...existing, status })
  }
  if (!execution.toolResult.ok) {
    const details = execution.toolResult.result
    const uncertain =
      details &&
      typeof details === 'object' &&
      !Array.isArray(details) &&
      ((details as Record<string, unknown>).commitMayHaveSucceeded === true ||
        String((details as Record<string, unknown>).code ?? '')
          .toLowerCase()
          .includes('uncertain'))
    return upsertSkillInstallation(current, {
      ...existing,
      status: uncertain ? 'uncertain' : 'failed'
    })
  }
  const result = execution.toolResult.result
  const resultStatus =
    result && typeof result === 'object' && !Array.isArray(result)
      ? (result as Record<string, unknown>).status
      : undefined
  return upsertSkillInstallation(current, {
    ...existing,
    status: resultStatus === 'alreadyInstalled' ? 'already_installed' : 'installed'
  })
}
