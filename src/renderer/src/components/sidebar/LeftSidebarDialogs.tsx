import type { TranslationKey } from '../../config/frontendTranslations'
import type { AppProject } from '../../config/projectConfig'
import type { ChatConversation } from '../../features/chat/chatTypes'
import { ConfirmationDialog } from '../dialog/ConfirmationDialog'
import { TextInputDialog } from '../dialog/TextInputDialog'
import type { BulkArchiveScope } from './leftSidebarTypes'
import { formatTemplate } from './leftSidebarUtils'

interface LeftSidebarDialogsProps {
  archiveProjectCount: number
  bulkArchiveCount: number
  conversationRenameValue: string
  onArchiveAllProjectConversations: () => void
  onArchiveAllRootConversations: () => void
  onArchiveProjectConversations: (projectId: string) => void
  onCancelArchiveProject: () => void
  onCancelBulkArchive: () => void
  onCancelConversationRename: () => void
  onCancelProjectRename: () => void
  onCancelRemoveProject: () => void
  onConfirmConversationRename: () => void
  onConfirmProjectRename: () => void
  onConversationRenameValueChange: (value: string) => void
  onProjectRenameValueChange: (value: string) => void
  onRemoveProject: (projectId: string) => Promise<boolean>
  pendingArchiveProject: AppProject | null
  pendingBulkArchiveScope: BulkArchiveScope | null
  pendingRemoveProject: AppProject | null
  renameValue: string
  renamingConversation: ChatConversation | null
  renamingProject: AppProject | null
  t: (key: TranslationKey) => string
}

export function LeftSidebarDialogs({
  archiveProjectCount,
  bulkArchiveCount,
  conversationRenameValue,
  onArchiveAllProjectConversations,
  onArchiveAllRootConversations,
  onArchiveProjectConversations,
  onCancelArchiveProject,
  onCancelBulkArchive,
  onCancelConversationRename,
  onCancelProjectRename,
  onCancelRemoveProject,
  onConfirmConversationRename,
  onConfirmProjectRename,
  onConversationRenameValueChange,
  onProjectRenameValueChange,
  onRemoveProject,
  pendingArchiveProject,
  pendingBulkArchiveScope,
  pendingRemoveProject,
  renameValue,
  renamingConversation,
  renamingProject,
  t
}: LeftSidebarDialogsProps) {
  return (
    <>
      {renamingProject && (
        <TextInputDialog
          title={t('project.renameTitle')}
          description={t('project.renameDescription')}
          value={renameValue}
          confirmDisabled={!renameValue.trim()}
          cancelLabel={t('project.cancel')}
          confirmLabel={t('project.save')}
          onCancel={onCancelProjectRename}
          onConfirm={onConfirmProjectRename}
          onValueChange={onProjectRenameValueChange}
        />
      )}

      {renamingConversation && (
        <TextInputDialog
          title={t('conversation.renameTitle')}
          description={t('conversation.renameDescription')}
          value={conversationRenameValue}
          confirmDisabled={!conversationRenameValue.trim()}
          cancelLabel={t('project.cancel')}
          confirmLabel={t('project.save')}
          onCancel={onCancelConversationRename}
          onConfirm={onConfirmConversationRename}
          onValueChange={onConversationRenameValueChange}
        />
      )}

      {pendingBulkArchiveScope && (
        <ConfirmationDialog
          title={formatTemplate(t('sidebar.archiveAllTitle'), { count: bulkArchiveCount })}
          description={
            pendingBulkArchiveScope === 'projects'
              ? t('sidebar.archiveAllProjectsDescription')
              : t('sidebar.archiveAllRootDescription')
          }
          cancelLabel={t('project.cancel')}
          confirmLabel={t('project.archiveAll')}
          onCancel={onCancelBulkArchive}
          onConfirm={() => {
            if (pendingBulkArchiveScope === 'projects') {
              onArchiveAllProjectConversations()
            } else {
              onArchiveAllRootConversations()
            }
            onCancelBulkArchive()
          }}
        />
      )}

      {pendingArchiveProject && (
        <ConfirmationDialog
          title={formatTemplate(t('project.archiveTitle'), { count: archiveProjectCount })}
          description={formatTemplate(t('project.archiveDescription'), {
            projectName: pendingArchiveProject.name
          })}
          cancelLabel={t('project.cancel')}
          confirmLabel={t('project.archiveAll')}
          onCancel={onCancelArchiveProject}
          onConfirm={() => {
            onArchiveProjectConversations(pendingArchiveProject.id)
            onCancelArchiveProject()
          }}
        />
      )}

      {pendingRemoveProject && (
        <ConfirmationDialog
          title={formatTemplate(t('project.removeTitle'), {
            projectName: pendingRemoveProject.name
          })}
          description={t('project.removeDescription')}
          cancelLabel={t('project.cancel')}
          confirmLabel={t('project.confirmRemove')}
          onCancel={onCancelRemoveProject}
          onConfirm={async () => {
            if (await onRemoveProject(pendingRemoveProject.id)) {
              onCancelRemoveProject()
            }
          }}
        />
      )}
    </>
  )
}
