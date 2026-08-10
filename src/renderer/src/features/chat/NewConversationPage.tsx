import type { AgentContextWindowSnapshot } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { useProjectSettings } from '../../config/ProjectSettingsProvider'
import { ChatComposer } from './components/ChatComposer'
import type { ChatComposerDraft, ChatSubmitOptions } from './chatTypes'
import './NewConversationPage.css'

interface NewConversationPageProps {
  contextWindowIndicatorEnabled?: boolean
  contextWindowSnapshot?: AgentContextWindowSnapshot
  draft: ChatComposerDraft
  defaultProjectId?: string | null
  onDraftChange: (draft: ChatComposerDraft) => void
  onDraftMessageChange?: (draft: ChatComposerDraft) => void
  onSubmitMessage: (
    message: string,
    options: ChatSubmitOptions
  ) => boolean | void | Promise<boolean | void>
  permissionModeAvailability: {
    custom: boolean
    full: boolean
  }
  skillCatalogRefreshToken?: number
}

export function NewConversationPage({
  contextWindowIndicatorEnabled = false,
  contextWindowSnapshot,
  defaultProjectId = null,
  draft,
  onDraftChange,
  onDraftMessageChange,
  onSubmitMessage,
  permissionModeAvailability,
  skillCatalogRefreshToken
}: NewConversationPageProps) {
  const { t } = useFrontendConfig()
  const { projects } = useProjectSettings()
  const selectedProjectId = draft.projectId ?? defaultProjectId
  const selectedProject = projects.find((project) => project.id === selectedProjectId)
  const title = selectedProject
    ? t('chat.projectTitle').replace('{projectName}', selectedProject.name)
    : t('chat.title')

  return (
    <section className="new-conversation-page" aria-label={t('chat.newConversation')}>
      <div className="new-conversation-page__content">
        <h1>{title}</h1>
        <ChatComposer
          contextWindowIndicatorEnabled={contextWindowIndicatorEnabled}
          contextWindowSnapshot={contextWindowSnapshot}
          defaultProjectId={defaultProjectId}
          draft={draft}
          onDraftChange={onDraftChange}
          onDraftMessageChange={onDraftMessageChange}
          permissionModeAvailability={permissionModeAvailability}
          resetKey={`new:${defaultProjectId ?? 'root'}`}
          skillCatalogRefreshToken={skillCatalogRefreshToken}
          showProjectSelector
          onSubmitMessage={onSubmitMessage}
        />
      </div>
    </section>
  )
}
