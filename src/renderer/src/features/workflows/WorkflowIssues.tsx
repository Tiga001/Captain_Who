import type { WorkflowDefinition, WorkflowIssue } from '@mycopilot/protocol'
import { workflowNodeLabel } from './workflowModelPresentation'
import type { RefObject } from 'react'
import { ConfirmationDialog } from '../../components/dialog/ConfirmationDialog'
import { workflowIssueText, type WorkflowText } from './workflowText'

/** Displays only the validation result returned by an explicit save. */
export function WorkflowIssues({
  graph,
  issues,
  text,
  onClose,
  restoreFocusRef,
  title,
  hint
}: {
  graph: WorkflowDefinition
  issues: WorkflowIssue[]
  text: WorkflowText
  onClose: () => void
  restoreFocusRef: RefObject<HTMLButtonElement | null>
  title?: string
  hint?: string
}) {
  if (issues.length === 0) return null
  const description = issues
    .map((issue) => {
      const node = graph.nodes.find((candidate) => candidate.id === issue.subject)
      const department = graph.departments?.find((candidate) => candidate.id === issue.subject)
      const label = node ? workflowNodeLabel(node, text) : department?.name
      return `• ${label ? `${label}: ` : ''}${workflowIssueText(issue.code, text)}`
    })
    .join('\n')
  return (
    <ConfirmationDialog
      title={title ?? text('savedDraft')}
      description={`${hint ?? text('saveIssueHint')}\n${description}`}
      descriptionClassName="workflow-save-issues-description"
      dialogRole="alertdialog"
      cancelLabel={text('close')}
      confirmLabel={text('acknowledge')}
      confirmVariant="primary"
      showCancelButton={false}
      restoreFocusRef={restoreFocusRef}
      onCancel={onClose}
      onConfirm={onClose}
    />
  )
}
