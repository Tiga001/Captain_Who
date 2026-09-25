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
  restoreFocusRef
}: {
  graph: WorkflowDefinition
  issues: WorkflowIssue[]
  text: WorkflowText
  onClose: () => void
  restoreFocusRef: RefObject<HTMLButtonElement | null>
}) {
  if (issues.length === 0) return null
  const description = issues
    .map((issue) => {
      const node = graph.nodes.find((candidate) => candidate.id === issue.subject)
      return `• ${node ? `${workflowNodeLabel(node, text)}: ` : ''}${workflowIssueText(issue.code, text)}`
    })
    .join('\n')
  return (
    <ConfirmationDialog
      title={text('savedDraft')}
      description={`${text('saveIssueHint')}\n${description}`}
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
