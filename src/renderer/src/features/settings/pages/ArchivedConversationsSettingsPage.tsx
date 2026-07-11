import { Archive, ChevronDown, Folder, RotateCcw, Trash2 } from 'lucide-react'
import { useMemo, useState } from 'react'
import { ConfirmationDialog } from '../../../components/dialog/ConfirmationDialog'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import type { AppProject } from '../../../config/projectConfig'
import type { ChatConversation } from '../../chat/chatTypes'
import './ArchivedConversationsSettingsPage.css'

interface ArchivedConversationsSettingsPageProps {
  conversations: ChatConversation[]
  onDeleteArchivedConversations: (conversationIds: string[]) => void
  onDeleteConversation: (conversationId: string) => void
  onUnarchiveConversation: (conversationId: string) => void
  projects: AppProject[]
}

type ProjectFilter = 'all' | 'none' | string
type PendingDeleteConfirmation =
  | { conversationId: string; type: 'single' }
  | { conversationIds: string[]; type: 'all' }
  | { conversationIds: string[]; scopeName: string; type: 'filtered' }

function formatArchivedDate(timestamp: number, language: string) {
  const date = new Date(timestamp)

  if (language === 'zh-CN') {
    return new Intl.DateTimeFormat('zh-CN', {
      year: 'numeric',
      month: 'long',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
      hour12: false
    }).format(date)
  }

  return new Intl.DateTimeFormat('en-US', {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
    hour: 'numeric',
    minute: '2-digit'
  }).format(date)
}

function getProjectName(projectId: string | null, projects: AppProject[], noProjectLabel: string) {
  if (!projectId) return noProjectLabel
  return projects.find((project) => project.id === projectId)?.name ?? noProjectLabel
}

function getProjectFilterName(
  projectFilter: ProjectFilter,
  projects: AppProject[],
  noProjectLabel: string
) {
  if (projectFilter === 'none') return noProjectLabel
  return projects.find((project) => project.id === projectFilter)?.name ?? noProjectLabel
}

export function ArchivedConversationsSettingsPage({
  conversations,
  onDeleteArchivedConversations,
  onDeleteConversation,
  onUnarchiveConversation,
  projects
}: ArchivedConversationsSettingsPageProps) {
  const { language, t } = useFrontendConfig()
  const [projectFilter, setProjectFilter] = useState<ProjectFilter>('all')
  const [pendingDeleteConfirmation, setPendingDeleteConfirmation] =
    useState<PendingDeleteConfirmation | null>(null)
  const archivedConversations = useMemo(
    () =>
      conversations
        .filter((conversation) => conversation.archivedAt)
        .sort((a, b) => (b.archivedAt ?? b.updatedAt) - (a.archivedAt ?? a.updatedAt)),
    [conversations]
  )
  const filteredConversations = archivedConversations.filter((conversation) => {
    if (projectFilter === 'all') return true
    if (projectFilter === 'none') return !conversation.projectId
    return conversation.projectId === projectFilter
  })
  const pendingDeleteTitle =
    pendingDeleteConfirmation?.type === 'filtered'
      ? t('archive.deleteFilteredConfirmTitle').replace(
          '{scopeName}',
          pendingDeleteConfirmation.scopeName
        )
      : pendingDeleteConfirmation?.type === 'all'
        ? t('archive.deleteAllConfirmTitle')
        : t('archive.deleteConfirmTitle')
  const pendingDeleteDescription =
    pendingDeleteConfirmation?.type === 'filtered'
      ? t('archive.deleteFilteredConfirmDescription').replace(
          '{scopeName}',
          pendingDeleteConfirmation.scopeName
        )
      : pendingDeleteConfirmation?.type === 'all'
        ? t('archive.deleteAllConfirmDescription')
        : t('archive.deleteConfirmDescription')

  return (
    <article className="archived-conversations-page">
      <div className="archived-conversations-page__header">
        <h1>{t('settings.page.archivedConversations')}</h1>
        <button
          className="archived-conversations-page__delete-all"
          type="button"
          disabled={filteredConversations.length === 0}
          onClick={() => {
            setPendingDeleteConfirmation(
              projectFilter === 'all'
                ? {
                    conversationIds: archivedConversations.map((conversation) => conversation.id),
                    type: 'all'
                  }
                : {
                    conversationIds: filteredConversations.map((conversation) => conversation.id),
                    scopeName: getProjectFilterName(
                      projectFilter,
                      projects,
                      t('archive.noProject')
                    ),
                    type: 'filtered'
                  }
            )
          }}
        >
          <Trash2 aria-hidden="true" />
          <span>{t('archive.deleteAll')}</span>
        </button>
      </div>

      <section
        className="archived-conversations-panel"
        aria-label={t('settings.page.archivedConversations')}
      >
        <div className="archived-conversations-panel__toolbar">
          <label className="archived-conversations-project-filter">
            <Folder aria-hidden="true" />
            <select
              value={projectFilter}
              aria-label={t('archive.projectFilter')}
              onChange={(event) => setProjectFilter(event.target.value)}
            >
              <option value="all">{t('archive.allProjects')}</option>
              <option value="none">{t('archive.noProject')}</option>
              {projects.map((project) => (
                <option value={project.id} key={project.id}>
                  {project.name}
                </option>
              ))}
            </select>
            <ChevronDown aria-hidden="true" />
          </label>
        </div>

        <div className="archived-conversations-list">
          {filteredConversations.length === 0 ? (
            <div className="archived-conversations-empty">
              <Archive aria-hidden="true" />
              <span>{t('archive.empty')}</span>
            </div>
          ) : (
            filteredConversations.map((conversation) => (
              <div className="archived-conversation-row" key={conversation.id}>
                <div className="archived-conversation-row__main">
                  <strong>{conversation.title}</strong>
                  <span>
                    {formatArchivedDate(conversation.updatedAt, language)}
                    {' · '}
                    {getProjectName(conversation.projectId, projects, t('archive.noProject'))}
                  </span>
                </div>

                <div className="archived-conversation-row__actions">
                  <button
                    className="archived-conversation-row__icon-button"
                    type="button"
                    aria-label={t('archive.deleteConversation')}
                    title={t('archive.deleteConversation')}
                    onClick={() =>
                      setPendingDeleteConfirmation({
                        conversationId: conversation.id,
                        type: 'single'
                      })
                    }
                  >
                    <Trash2 aria-hidden="true" />
                  </button>
                  <button
                    className="archived-conversation-row__restore-button"
                    type="button"
                    onClick={() => onUnarchiveConversation(conversation.id)}
                  >
                    <RotateCcw aria-hidden="true" />
                    <span>{t('archive.unarchive')}</span>
                  </button>
                </div>
              </div>
            ))
          )}
        </div>
      </section>

      {pendingDeleteConfirmation && (
        <ConfirmationDialog
          title={pendingDeleteTitle}
          description={pendingDeleteDescription}
          cancelLabel={t('configuration.cancel')}
          confirmLabel={t('configuration.delete')}
          onCancel={() => setPendingDeleteConfirmation(null)}
          onConfirm={() => {
            if (
              pendingDeleteConfirmation.type === 'all' ||
              pendingDeleteConfirmation.type === 'filtered'
            ) {
              onDeleteArchivedConversations(pendingDeleteConfirmation.conversationIds)
            } else {
              onDeleteConversation(pendingDeleteConfirmation.conversationId)
            }
            setPendingDeleteConfirmation(null)
          }}
        />
      )}
    </article>
  )
}
