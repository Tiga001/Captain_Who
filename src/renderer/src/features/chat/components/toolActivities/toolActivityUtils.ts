import type { AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import type { TranslationKey } from '../../../../config/frontendTranslations'
import { formatTranslation, type Translate } from '../../../../config/translationFormat'

export type SettledToolStatus = 'completed' | 'failed' | 'cancelled'

export function formatToolDetails(value: unknown) {
  if (typeof value === 'string') return value

  try {
    return JSON.stringify(value, null, 2)
  } catch {
    return String(value)
  }
}

export function getToolDisplayName(tool: string, t: Translate) {
  const labels: Record<string, TranslationKey> = {
    apply_patch: 'tool.applyPatch',
    write_file: 'tool.writeFile',
    attachments_list: 'tool.attachmentsList',
    attachments_list_project: 'tool.attachmentsListProject',
    conversation_history: 'tool.conversationHistory',
    git_diff: 'tool.gitDiff',
    read_file: 'tool.readFile',
    read_image: 'tool.readImage',
    read_pdf: 'tool.readPdf',
    read_presentation: 'tool.readPresentation',
    read_spreadsheet: 'tool.readSpreadsheet',
    read_word: 'tool.readWord',
    run_command: 'tool.runCommand',
    search_code: 'tool.searchCode',
    search_files: 'tool.searchFiles',
    skills_materialize_resource: 'tool.skillsMaterializeResource',
    skills_run_script: 'tool.skillsRunScript',
    office_document: 'tool.officeDocument',
    office_spreadsheet: 'tool.officeSpreadsheet',
    office_presentation: 'tool.officePresentation',
    workspace_map: 'tool.workspaceMap',
    web_fetch: 'tool.webFetch',
    web_search: 'tool.webSearch'
  }

  const labelKey = labels[tool]
  return labelKey ? t(labelKey) : tool
}

export function getToolCallLabel(
  call: AgentToolCall,
  result: AgentToolResult | undefined,
  t: Translate,
  options: { cancelled?: boolean; settledStatus?: SettledToolStatus } = {}
) {
  const tool = getToolDisplayName(call.tool, t)

  if (!result) {
    if (options.cancelled || options.settledStatus === 'cancelled') {
      return formatTranslation(t, 'agent.tool.cancelled', { tool })
    }
    if (options.settledStatus === 'failed') {
      return formatTranslation(t, 'agent.tool.failed', { tool })
    }
    if (options.settledStatus === 'completed') {
      return formatTranslation(t, 'agent.tool.completed', { tool })
    }
    return call.approvalStatus === 'required'
      ? formatTranslation(t, 'agent.tool.waitingApproval', { tool })
      : formatTranslation(t, 'agent.tool.running', { tool })
  }

  if (!result.ok) {
    return formatTranslation(t, 'agent.tool.failed', { tool })
  }

  return formatTranslation(t, 'agent.tool.completed', { tool })
}
