import type { WorkflowDefinition, WorkflowInstance, WorkflowIssue } from '@mycopilot/protocol'
import type { RefObject } from 'react'
import { ConfirmationDialog } from '../../../components/dialog/ConfirmationDialog'
import { WorkflowIssues } from '../WorkflowIssues'
import { workflowText } from '../workflowText'
import { projectWorkflowText } from './projectWorkflowText'

interface WorkflowPageDialogsProps {
  bindingNotice: string
  confirmDiscard: boolean
  foreground: boolean
  graph: WorkflowDefinition | null
  language: string
  mergeNotice: string
  onCancelDelete: () => void
  onCancelDiscard: () => void
  onConfirmDiscard: () => void
  onDelete: () => Promise<void>
  onDismissBindingNotice: () => void
  onDismissMergeNotice: () => void
  onDismissValidationIssues: () => void
  pendingDelete: WorkflowInstance | null
  saveButtonRef: RefObject<HTMLButtonElement | null>
  validationIssues: WorkflowIssue[]
}

export function WorkflowPageDialogs({
  bindingNotice,
  confirmDiscard,
  foreground,
  graph,
  language,
  mergeNotice,
  onCancelDelete,
  onCancelDiscard,
  onConfirmDiscard,
  onDelete,
  onDismissBindingNotice,
  onDismissMergeNotice,
  onDismissValidationIssues,
  pendingDelete,
  saveButtonRef,
  validationIssues
}: WorkflowPageDialogsProps) {
  const t = projectWorkflowText(language)
  const text = workflowText(language)

  return (
    <>
      {foreground && mergeNotice && (
        <ConfirmationDialog
          title={t('部分修改需要检查', 'Review affected edits')}
          description={mergeNotice}
          cancelLabel={t('关闭', 'Close')}
          confirmLabel={t('知道了', 'Got it')}
          confirmVariant="primary"
          showCancelButton={false}
          onCancel={onDismissMergeNotice}
          onConfirm={onDismissMergeNotice}
        />
      )}
      {graph && validationIssues.length > 0 && (
        <WorkflowIssues
          graph={graph}
          issues={validationIssues}
          text={text}
          restoreFocusRef={saveButtonRef}
          title={t('请完善组织配置', 'Complete organization configuration')}
          hint={t('补全以下配置后即可保存。', 'Complete these settings before saving.')}
          onClose={onDismissValidationIssues}
        />
      )}
      {bindingNotice && (
        <ConfirmationDialog
          title={t('无法分配对话', 'Cannot assign conversation')}
          description={bindingNotice}
          cancelLabel={t('关闭', 'Close')}
          confirmLabel={t('知道了', 'Got it')}
          confirmVariant="primary"
          showCancelButton={false}
          onCancel={onDismissBindingNotice}
          onConfirm={onDismissBindingNotice}
        />
      )}
      {pendingDelete && (
        <ConfirmationDialog
          title={t('移除组织？', 'Remove organization?')}
          description={t(
            `“${pendingDelete.name}”的绑定和颜色标记将被移除，所有对话都会保留。`,
            `Bindings and color markers for “${pendingDelete.name}” will be removed. All conversations will remain.`
          )}
          cancelLabel={t('取消', 'Cancel')}
          confirmLabel={t('移除', 'Remove')}
          onCancel={onCancelDelete}
          onConfirm={onDelete}
        />
      )}
      {confirmDiscard && (
        <ConfirmationDialog
          title={t('放弃未保存的修改？', 'Discard unsaved changes?')}
          description={t(
            '这次选择不会应用到对话，已有组织保持不变。',
            'These selections will not be applied to conversations. Existing organizations will remain unchanged.'
          )}
          cancelLabel={t('继续编辑', 'Keep editing')}
          confirmLabel={t('放弃修改', 'Discard changes')}
          onCancel={onCancelDiscard}
          onConfirm={onConfirmDiscard}
        />
      )}
    </>
  )
}
