import { renderSettingsNodes, settingLabel } from '../settingsDefinition'
import {
  archiveDeleteSettings,
  archiveProjectFilterSettings,
  archiveConversationListSettings,
  archiveConversationDeleteSettings,
  archiveConversationRestoreSettings
} from './managementSettings.definition'
import { Archive, Folder, RotateCcw, Trash2 } from 'lucide-react'
import { useMemo, useState } from 'react'
import { ConfirmationDialog } from '../../../components/dialog/ConfirmationDialog'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import type { AppLanguage } from '../../../config/frontendTranslations'
import type { AppProject } from '../../../config/projectConfig'
import type { ChatConversation } from '../../chat/chatTypes'
import { SettingsSelect, type SettingsSelectOption } from '../components/SettingsSelect'
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

interface ArchivedConversationGroup {
  key: string
  name: string
  conversations: ChatConversation[]
}

const archivedDateFormatters = new Map<AppLanguage, Intl.DateTimeFormat>()

function getArchivedDateFormatter(language: AppLanguage) {
  let formatter = archivedDateFormatters.get(language)

  if (!formatter) {
    formatter = new Intl.DateTimeFormat(language, {
      dateStyle: 'medium',
      timeStyle: 'short'
    })
    archivedDateFormatters.set(language, formatter)
  }

  return formatter
}

function formatArchivedDate(timestamp: number, language: AppLanguage) {
  return getArchivedDateFormatter(language).format(timestamp)
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

function groupArchivedConversations(
  conversations: ChatConversation[],
  projects: AppProject[],
  noProjectLabel: string
): ArchivedConversationGroup[] {
  const groups: ArchivedConversationGroup[] = []
  const indexByKey = new Map<string, number>()
  for (const conversation of conversations) {
    const key = conversation.projectId ?? 'none'
    let index = indexByKey.get(key)
    if (index === undefined) {
      index = groups.length
      indexByKey.set(key, index)
      groups.push({
        key,
        name: getProjectName(conversation.projectId, projects, noProjectLabel),
        conversations: []
      })
    }
    groups[index].conversations.push(conversation)
  }
  return groups
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
  const projectFilterOptions = useMemo<Array<SettingsSelectOption<ProjectFilter>>>(
    () => [
      { value: 'all', label: t('archive.allProjects') },
      { value: 'none', label: t('archive.noProject') },
      ...projects.map((project) => ({ value: project.id, label: project.name }))
    ],
    [projects, t]
  )
  const archivedConversations = useMemo(
    () =>
      conversations
        .filter((conversation) => conversation.archivedAt)
        .sort((a, b) => (b.archivedAt ?? b.updatedAt) - (a.archivedAt ?? a.updatedAt)),
    [conversations]
  )
  const filteredConversations = useMemo(
    () =>
      archivedConversations.filter((conversation) => {
        if (projectFilter === 'all') return true
        if (projectFilter === 'none') return !conversation.projectId
        return conversation.projectId === projectFilter
      }),
    [archivedConversations, projectFilter]
  )
  const conversationGroups = useMemo(
    () => groupArchivedConversations(filteredConversations, projects, t('archive.noProject')),
    [filteredConversations, projects, t]
  )
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
        {renderSettingsNodes(archiveDeleteSettings, (node) => (
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
            <span>{settingLabel(node, t)}</span>
          </button>
        ))}
      </div>

      {renderSettingsNodes(archiveProjectFilterSettings, (node) => (
        <div className="archived-conversations-page__toolbar">
          <SettingsSelect
            ariaLabel={settingLabel(node, t)}
            className="archived-conversations-project-filter"
            leadingIcon={<Folder />}
            onChange={setProjectFilter}
            options={projectFilterOptions}
            value={projectFilter}
          />
        </div>
      ))}

      {renderSettingsNodes(archiveConversationListSettings, () =>
        filteredConversations.length === 0 ? (
          <div className="archived-conversations-empty">
            <Archive aria-hidden="true" />
            <span>{t('archive.empty')}</span>
          </div>
        ) : (
          <div className="archived-conversations-groups">
            {conversationGroups.map((group) => (
              <section className="archived-conversations-group" key={group.key}>
                <div className="archived-conversations-group__header">
                  <h2 className="archived-conversations-group__title">
                    <Folder aria-hidden="true" />
                    {group.name}
                  </h2>
                  <span className="archived-conversations-group__count">
                    {t('archive.groupCount').replace('{count}', String(group.conversations.length))}
                  </span>
                </div>
                <div className="archived-conversations-list">
                  {group.conversations.map((conversation) => (
                    <div className="archived-conversation-row" key={conversation.id}>
                      <div className="archived-conversation-row__main">
                        <strong>{conversation.title}</strong>
                        <span>{formatArchivedDate(conversation.updatedAt, language)}</span>
                      </div>

                      <div className="archived-conversation-row__actions">
                        {renderSettingsNodes(archiveConversationDeleteSettings, (item) => (
                          <button
                            className="archived-conversation-row__icon-button"
                            type="button"
                            aria-label={settingLabel(item, t)}
                            title={settingLabel(item, t)}
                            onClick={() =>
                              setPendingDeleteConfirmation({
                                conversationId: conversation.id,
                                type: 'single'
                              })
                            }
                          >
                            <Trash2 aria-hidden="true" />
                          </button>
                        ))}
                        {renderSettingsNodes(archiveConversationRestoreSettings, (item) => (
                          <button
                            className="archived-conversation-row__restore-button"
                            type="button"
                            onClick={() => onUnarchiveConversation(conversation.id)}
                          >
                            <RotateCcw aria-hidden="true" />
                            <span>{settingLabel(item, t)}</span>
                          </button>
                        ))}
                      </div>
                    </div>
                  ))}
                </div>
              </section>
            ))}
          </div>
        )
      )}

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
